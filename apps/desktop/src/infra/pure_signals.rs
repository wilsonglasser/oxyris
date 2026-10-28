//! Pure-mode signal detection — backend port of `apps/web/src/lib/pureTurn.ts`.
//!
//! Pure (PTY `claude` TUI) sessions have no structured turn-event stream; the
//! TUI is opaque bytes. The frontend used to sniff those bytes for "needs
//! input" / "turn ended" markers, but it ran in the WebView, where the idle
//! timer and xterm render loop are throttled while the window is in the
//! background — that's the "fails when the window hasn't had focus" bug.
//!
//! Running the same detection here, in the PTY reader path, is immune to GUI
//! focus (the PTY is a plain byte pipe). It's also the foundation the auto-pilot
//! needs: the supervisor must react to prompts with the window unfocused or
//! minimized. Keep the regexes in lockstep with `pureTurn.ts`.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;

/// Rolling-tail cap (chars). Matches `pureTurn.ts`'s 2000-char window — large
/// enough that a prompt split across several PTY writes still accumulates.
const TAIL_CAP_CHARS: usize = 2000;

/// How much of the tail's end the LIVE prompt check looks at. A real menu (its
/// header, a few numbered options, the footer) fits easily; anything older has
/// scrolled up under newer output and is no longer the active frame. Keeps
/// assistant prose that mentions a prompt phrase from latching the red dot.
const RECENT_PROMPT_CHARS: usize = 1000;

/// The last [`RECENT_PROMPT_CHARS`] chars of `s`, on a char boundary (returns all
/// of `s` when shorter).
fn recent_window(s: &str) -> &str {
    let len = s.chars().count();
    if len <= RECENT_PROMPT_CHARS {
        return s;
    }
    let start = s
        .char_indices()
        .nth(len - RECENT_PROMPT_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    &s[start..]
}

/// Cap on the spaces one cursor-forward (`ESC[nC`) expands to — a sane bound
/// on a single row, so a bogus count can't balloon the tail.
const MAX_CUF_SPACES: usize = 512;

/// Strip ANSI escape sequences (CSI + OSC) so prompt text matches across the
/// TUI's redraws (char-scan, not regex, to handle the OSC `BEL`/`ESC \`
/// terminators cleanly).
///
/// Cursor movement is kept as whitespace rather than dropped. ConPTY (every
/// Windows-native PTY) re-encodes claude's output as a diff of the screen: a
/// space between words becomes `ESC[1C` (cursor forward) and a line break
/// becomes an absolute `ESC[row;colH`. Dropping those glued the text into
/// "✻Crunched for 7s" / "doyouwanttoproceed", which no detector matched. So
/// cursor-forward expands to that many spaces, an absolute move to a newline
/// indented to its column, other vertical moves to a newline, and a
/// horizontal-absolute move to one space.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // Iterate in place over a peekable char stream — no intermediate
    // `Vec<char>` allocation of the whole chunk (this runs on the PTY reader
    // hot path, once per output chunk).
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // CSI: parameter bytes up to a final byte in @–~ (0x40–0x7e),
                // inclusive. The first two numeric parameters are kept — the
                // cursor-forward count, and the column of an absolute move.
                let mut params: [Option<usize>; 2] = [None, None];
                let mut idx = 0;
                let mut fin = None;
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        fin = Some(f);
                        break;
                    }
                    if f == ';' {
                        idx += 1;
                    } else if let (Some(d), Some(slot)) = (f.to_digit(10), params.get_mut(idx)) {
                        *slot = Some(
                            slot.unwrap_or(0)
                                .saturating_mul(10)
                                .saturating_add(d as usize),
                        );
                    }
                }
                match fin {
                    Some('C') => {
                        let n = params[0].unwrap_or(1).clamp(1, MAX_CUF_SPACES);
                        out.extend(std::iter::repeat_n(' ', n));
                    }
                    // Absolute move: a new line, indented to the target column,
                    // so an indented row ("  2. No") keeps its indent.
                    Some('H' | 'f') => {
                        out.push('\n');
                        let col = params[1].unwrap_or(1).clamp(1, MAX_CUF_SPACES);
                        out.extend(std::iter::repeat_n(' ', col - 1));
                    }
                    Some('A' | 'B' | 'E' | 'F' | 'd') => out.push('\n'),
                    Some('G' | '`') => out.push(' '),
                    _ => {}
                }
            }
            Some(']') => {
                // OSC: skip until BEL (7) or ESC (start of the ST terminator).
                while let Some(&n) = chars.peek() {
                    if n == '\u{7}' {
                        chars.next(); // consume the BEL terminator
                        break;
                    }
                    if n == '\u{1b}' {
                        // ESC terminator (ST = ESC `\`) — leave it for the next
                        // iteration's lone-ESC branch to drop.
                        break;
                    }
                    chars.next();
                }
            }
            // Lone ESC or a 2-byte escape — ESC + the following byte already
            // consumed by `chars.next()` above; nothing to emit.
            _ => {}
        }
    }
    out
}

/// The last window title (OSC 0 / OSC 2) set in `raw`, if any. claude keeps
/// its run state there: the title starts with a half-circle spinner (`◐`/`◑`,
/// flipping about once a second while the terminal is focused, frozen on one
/// glyph while it is blurred) for the whole time a turn is live — through
/// long silent shell commands too — and with `✳` once it settles. Unlike the
/// painted spinner row, it doesn't depend on how the screen is redrawn.
pub fn osc_title(raw: &str) -> Option<&str> {
    let start = raw
        .rfind("\x1b]0;")
        .or_else(|| raw.rfind("\x1b]2;"))
        .map(|i| i + 4)?;
    let rest = &raw[start..];
    let end = rest.find(['\u{7}', '\u{1b}'])?;
    Some(&rest[..end])
}

/// A turn-state reading of claude's window title: `Some(true)` while a turn
/// runs (`◐◑◒◓`), `Some(false)` once settled (`✳`), `None` for anything else
/// (startup titles, a title-less build, another program).
fn classify_title(title: &str) -> Option<bool> {
    match title.trim_start().chars().next()? {
        '\u{25D0}'..='\u{25D3}' => Some(true),
        '\u{2733}' => Some(false),
        _ => None,
    }
}

/// A busy title that hasn't flipped for this long is no longer trusted (the
/// spinner flips about once a second; a frozen one means claude stopped
/// painting it) — the dot falls back to the screen heuristics.
///
/// Only applies while the terminal reports **focus**. claude gates the title
/// spinner's flip interval on its `isTerminalFocused` context: on a focus-out
/// (`ESC[O`, which xterm.js sends on blur / when the pane is torn down) the
/// glyph freezes on ◐/◑ for the rest of the turn, while the title still flips
/// to ✳ the instant the turn settles. So a frozen busy title on a blurred
/// terminal is a trustworthy level, not a stuck heartbeat — ageing it out there
/// made every background / hidden thread flap blue↔orange (or blue↔green on the
/// active one) through each silent stretch of a running turn.
const TITLE_STALE: Duration = Duration::from_secs(5);

/// The terminal focus report claude asked for (DECSET 1004) as it rides the
/// *input* stream: `ESC[I` = focus in, `ESC[O` = focus out. Returns the last
/// one in `data`, or `None` when the chunk carries neither.
pub fn focus_event(data: &str) -> Option<bool> {
    let fin = data.rfind("\x1b[I");
    let fout = data.rfind("\x1b[O");
    match (fin, fout) {
        (None, None) => None,
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (Some(i), Some(o)) => Some(i > o),
    }
}

/// The claude TUI renders a numbered menu whenever it needs the user to approve
/// a tool or answer a question. While it's on screen the thread "wants input".
/// Port of `PURE_PROMPT_RE`.
fn prompt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            // Branch 1 (numbered menu): the selection marker glyph varies across
            // builds/menus — `❯` (U+276F) for tool-approval menus, `›` (U+203A)
            // for the multi-question AskUserQuestion menu — so accept both plus a
            // plain `>`. The gap to the second item is wide (800) because option
            // descriptions can be long. Branch 2/3 (footer hints) are
            // glyph-independent: any selectable prompt paints a "…to navigate" /
            // "Esc to cancel" footer, which the busy footer ("esc to interrupt")
            // never does — so they catch menus whose layout branch 1 misses.
            r"(?i)([❯›>]\s*\d+\.\s+\S[\s\S]{0,800}\n\s+\d+\.\s)|(keys to navigate)|(esc to cancel)|(enter to select[\s\S]{0,200}navigate)|(ctrl-g to edit)|(do you want to (proceed|make this edit|create|run|continue))|(would you like to proceed)|(yes, and don'?t ask again)|(no, and tell claude)",
        )
        .expect("pure prompt regex")
    })
}

/// claude's optional end-of-session feedback poll. Not a required answer, so it
/// signals a turn end rather than "needs input". Port of `PURE_POLL_RE`.
fn poll_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)1:\s*bad\s+2:\s*fine\s+3:\s*good\s+0:\s*dismiss").expect("pure poll regex")
    })
}

/// Turn-end summary line ("✶ Worked for 3m 9s", "✻ Brewed for 12s", …). A hard
/// "done" signal. Port of `PURE_TURN_END_RE`. Glyph class widened past the
/// original `✱-✽` (see [`working_re`]), plus `※` (U+203B): some builds settle
/// the footer on it ("※ Churned for 4m 38s · done 12:51 · 1 shell still
/// running"), the same mark the recap line uses.
fn turn_end_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[\x{2720}-\x{274F}\x{203B}]\s+\S+\s+for\s+\d+[ms]")
            .expect("pure turn-end regex")
    })
}

/// Live working spinner — same glyph family but a present-participle verb and a
/// trailing ellipsis ("✻ Flummoxing… (8m …)"). Tells us claude is still busy.
/// Port of `PURE_WORKING_RE`.
///
/// The glyph class is the whole `✠`–`❏` Dingbat asterisk/sparkle block
/// (`U+2720`–`U+274F`), not the original narrow `✱-✽` (`U+2731`–`U+273D`):
/// claude's spinner cycles glyphs outside that slice (`✶ U+2736`, `✳ U+2733`,
/// `✦ U+2726`), and a glyph the regex didn't cover read as "not working" → the
/// blue dot never lit. Widening it is the cheap half of the "stuck dot" fix; the
/// load-bearing half is [`interrupt_re`].
fn working_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[\x{2720}-\x{274F}][^\n…]*(…|\.\.\.)").expect("pure working regex")
    })
}

/// The claude TUI prints an "esc to interrupt" (older builds: "ctrl-c to
/// interrupt") hint in the spinner footer for the *entire* time a turn is live,
/// and clears it the instant the turn settles. It does not depend on the
/// rotating spinner glyph, so it survives a glyph change in the CLI — the most
/// stable "a turn is running right now" signal there is. Drives the blue dot via
/// [`PureSniffer::is_busy_now`].
fn interrupt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(esc|ctrl-c|ctrl\+c)\s+to\s+interrupt").expect("pure interrupt regex")
    })
}

/// End-of-conversation recap line — "✻ recap: …" (current TUI) or "※ recap"
/// (older builds). A settled-turn signal that PURE_TURN_END_RE can't catch (no
/// "for <duration>"). Port of `PURE_RECAP_RE`.
fn recap_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)([\x{2720}-\x{274F}]|※)\s*recap").expect("pure recap regex"))
}

/// While a turn is blocked on background work (the Task tool run in parallel,
/// a dynamic workflow), claude paints a static status line — "✻ Waiting for 2
/// background agents to finish", "✻ Waiting for 1 dynamic workflow to finish"
/// — and drops the spinner and the "esc to interrupt" footer until that work
/// reports back. Both of the usual "a turn is running" tells are therefore
/// absent, and the dot used to fall back to idle for what can be several
/// minutes of real work.
///
/// The noun is open ("N <a few words> to finish") so a new kind of background
/// work doesn't need a new regex. Anchored to the start of the line (past the
/// TUI's box/bullet chrome) so the assistant *describing* background agents in
/// prose doesn't read as the status line itself.
fn waiting_agents_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?im)^[\s\x{2502}\x{2514}\x{251C}\x{2570}\x{2500}\x{2720}-\x{274F}*]*waiting\s+for\s+\d+\s+(?:background\s+agents?\b|[\w-]+(?:\s+[\w-]+){0,2}?\s+to\s+finish\b)",
        )
        .expect("pure waiting-agents regex")
    })
}

/// "Interrupted · What should Claude do instead?" — the user hit Esc. Ends a
/// background wait as surely as a turn-end footer does.
fn interrupted_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\binterrupted\b").expect("pure interrupted regex"))
}

/// True if `s` (output painted after the waiting status line) shows the wait
/// is over: claude resumed the turn (spinner / "esc to interrupt"), settled it
/// (turn-end footer), asks for input, or was interrupted.
fn ends_wait(s: &str) -> bool {
    working_re().is_match(s)
        || interrupt_re().is_match(s)
        || turn_end_re().is_match(s)
        || prompt_re().is_match(s)
        || interrupted_re().is_match(s)
}

/// The turn-end footer sometimes reports work that outlived the turn — "✳
/// Worked for 15m 2s · done 10:05 · 1 shell, 1 monitor still running". The turn
/// itself settled, but background shells / monitors are still alive and will
/// wake claude when they finish — so the thread still counts as working.
///
/// Anchored to the turn-end marker ([`turn_end_re`]) so assistant prose saying
/// "still running" can't trip it; the gap between the two halves is `[\s\S]` so
/// a footer wrapped by a narrow PTY still matches.
fn background_running_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)[\x{2720}-\x{274F}\x{203B}]\s+\S+\s+for\s+\d+[ms][\s\S]{0,200}?\b\d+\s+(shells?|monitors?|agents?|tasks?|commands?)\b[\s\S]{0,80}?still\s+running",
        )
        .expect("pure background-running regex")
    })
}

/// claude prints an "API Error:" / "API Error (…)" line when a request to the
/// model API fails mid-turn (e.g. "API Error: Connection closed mid-response",
/// "API Error (Request timed out)"). A hard failure of the turn — surfaced as
/// the red bull + a warning banner. Anchored to the CLI's exact `Error:` / `(`
/// prefix so assistant prose merely *mentioning* an api error can't trip it.
fn api_error_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)api error\s*[:(]").expect("pure api-error regex"))
}

/// claude's rotating one-line TUI hint ("Tip: …"), usually under a tree glyph
/// (└/├/╰). It's chrome, not turn content, and it animates — so it keeps the PTY
/// dripping and can masquerade as the last line. Port of `PURE_TIP_RE`; stripped
/// before detection so a tip can't trip a prompt / working / turn-end match.
fn tip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^[^\S\r\n]*[│└├╰─\s]*Tip:.*$").expect("pure tip regex"))
}

/// Remove claude's TUI tip-hint lines (see [`tip_re`]). Feeds the detectors
/// only — display is untouched. Port of `stripTipLines`.
fn strip_tip_lines(s: &str) -> String {
    tip_re().replace_all(s, "").into_owned()
}

/// Reduce a raw PTY chunk to the form the detectors work on: ANSI escapes
/// stripped, animated tip-hint lines removed. The reader computes this **once**
/// per chunk and hands it to both [`PureSniffer::feed_stripped`] and
/// [`has_content_stripped`] so the hot path strips a chunk a single time instead
/// of once per call.
pub fn strip_for_detection(raw: &str) -> String {
    strip_tip_lines(&strip_ansi(raw))
}

/// True if `data`, once chrome is stripped (ANSI escapes + the animated tip
/// hint), still carries visible text. The idle watchdog uses this so a pure
/// chrome redraw — the rotating "Tip:" hint, a cursor blink, a bare repaint —
/// does NOT push the silence window out and keep a finished turn stuck "busy".
/// A live working spinner ("✻ Drizzling…") DOES count as content, so it keeps
/// the turn alive while it's on screen.
#[cfg(test)]
pub fn has_content(data: &str) -> bool {
    has_content_stripped(&strip_for_detection(data))
}

/// [`has_content`] on an already-[`strip_for_detection`]-ed chunk — lets the
/// reader reuse the single strip it computed for the sniffer.
pub fn has_content_stripped(stripped: &str) -> bool {
    stripped
        .chars()
        .any(|c| !c.is_whitespace() && !c.is_control())
}

/// A signal the sniffer emits when a marker first appears in the PTY stream.
/// Latched per turn so a redraw doesn't re-fire — cleared by [`PureSniffer::reset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PureSignal {
    /// claude is asking the user to approve a tool / answer a question. The red
    /// "wants input" bull, and the auto-pilot's cue to respond.
    NeedsInput,
    /// The turn settled (poll / "Worked for…" / recap). No answer required.
    TurnEnded,
    /// claude is actively working — keeps the busy state alive, suppresses a
    /// premature idle-based turn-end.
    Working,
}

/// Stateful rolling-tail sniffer. Feed it raw PTY chunks; it returns the
/// signals newly triggered by each chunk. One per claude PTY. Mirrors the
/// latching in `pureTurn.ts` (`promptOpenRef`, `pollOpenRef`, `turnEndSeenRef`)
/// so each marker fires once per turn, not once per redraw.
#[derive(Default)]
pub struct PureSniffer {
    tail: String,
    prompt_open: bool,
    poll_open: bool,
    turn_end_seen: bool,
    recap_seen: bool,
    working_active: bool,
    /// Latched while claude is blocked on background work (see
    /// [`waiting_agents_re`]); cleared by [`ends_wait`] output or [`reset`].
    waiting: bool,
    /// Latched once an "API Error" line appears, until [`reset`] (the user's next
    /// submit). Drives the red bull + warning banner in pure mode.
    api_error_seen: bool,
    /// claude's last window title (see [`osc_title`]) and when it last
    /// changed. A level, not a per-turn latch — [`reset`] leaves it alone.
    title: Option<String>,
    title_changed_at: Option<Instant>,
    /// The terminal reported focus-out (`ESC[O`) and no focus-in since. claude
    /// freezes its title spinner while blurred, so [`title_busy`] must not age
    /// a busy title out in this state. `false` (focused) by default — claude's
    /// own default is "unknown", which it treats as focused.
    blurred: bool,
}

impl PureSniffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a raw (un-stripped) PTY output chunk. Returns the signals this chunk
    /// newly triggered, in detection order. Convenience wrapper that strips then
    /// delegates to [`Self::feed_stripped`]; the hot path calls `feed_stripped`
    /// directly with a chunk it already stripped once via [`strip_for_detection`].
    #[cfg(test)]
    pub fn feed(&mut self, data: &str) -> Vec<PureSignal> {
        self.feed_stripped(&strip_for_detection(data))
    }

    /// Feed a chunk already reduced by [`strip_for_detection`]. Returns the
    /// signals it newly triggered, in detection order.
    pub fn feed_stripped(&mut self, stripped: &str) -> Vec<PureSignal> {
        self.tail.push_str(stripped);
        // Keep only the last TAIL_CAP_CHARS chars, on a char boundary.
        let len = self.tail.chars().count();
        if len > TAIL_CAP_CHARS {
            let drop = len - TAIL_CAP_CHARS;
            let start = self
                .tail
                .char_indices()
                .nth(drop)
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            self.tail = self.tail[start..].to_owned();
        }

        let mut out = Vec::new();

        // Live spinner: not latched the same way — it toggles working state.
        // Only report the rising edge so we don't spam Working every chunk.
        let working_now = working_re().is_match(&self.tail);
        if working_now && !self.working_active {
            self.working_active = true;
            out.push(PureSignal::Working);
        } else if !working_now {
            self.working_active = false;
        }

        // Prompt detection is LIVE on the most recent frame, not latched over the
        // whole accumulated tail. The genuine permission/question menu is always
        // the bottom-most interactive element; scoping to the recent window means
        // assistant prose that merely *mentions* a prompt phrase ("do you want to
        // proceed", "yes, and don't ask again") stops reading as a live menu once
        // more output streams below it. Emit NeedsInput only on the rising edge.
        let prompt_now = prompt_re().is_match(recent_window(&self.tail));
        if prompt_now && !self.prompt_open {
            out.push(PureSignal::NeedsInput);
        }
        self.prompt_open = prompt_now;
        if !self.poll_open && poll_re().is_match(&self.tail) {
            self.poll_open = true;
            out.push(PureSignal::TurnEnded);
        }
        if !self.turn_end_seen && turn_end_re().is_match(&self.tail) {
            self.turn_end_seen = true;
            out.push(PureSignal::TurnEnded);
        }
        if !self.recap_seen && recap_re().is_match(&self.tail) {
            self.recap_seen = true;
            out.push(PureSignal::TurnEnded);
        }
        // API error — latched, not surfaced as a signal (it drives the dot via
        // `api_error()`, not the autopilot sink). The whole tail is scanned (not
        // just the recent window): the CLI's error line can scroll up under its
        // own retry chrome and we want it to stay flagged until the user resubmits.
        if !self.api_error_seen && api_error_re().is_match(&self.tail) {
            self.api_error_seen = true;
        }
        // Background wait — a latch, not a recent-window match: during the
        // wait claude keeps repainting a progress row (a workflow's elapsed
        // time and token count tick every second), and under ConPTY each tick
        // is a small screen diff that pushes the status line out of any window
        // long before the work finishes. It holds until something painted
        // after the status line ends it.
        match waiting_agents_re().find_iter(&self.tail).last() {
            Some(m) => self.waiting = !ends_wait(&self.tail[m.end()..]),
            // The status line scrolled out of the capped tail: only this chunk
            // is new, so it alone can end the wait.
            None => self.waiting = self.waiting && !ends_wait(stripped),
        }

        out
    }

    /// Record a window title claude set (from [`osc_title`] on a raw chunk —
    /// the detection strip drops OSC). Only a *change* restarts the staleness
    /// clock, so a spinner frozen on one frame ages out.
    pub fn note_title(&mut self, title: &str) {
        if self.title.as_deref() != Some(title) {
            self.title = Some(title.to_owned());
            self.title_changed_at = Some(Instant::now());
        }
    }

    /// Record a terminal focus report seen on the input stream (see
    /// [`focus_event`]). A focus-in restarts the title staleness clock: claude
    /// resumes flipping the spinner only on its next tick, so without this the
    /// first second after a refocus would read the (legitimately frozen) title
    /// as stale and blip the dot through the screen heuristic.
    pub fn note_focus(&mut self, focused: bool) {
        self.blurred = !focused;
        if focused && self.title.is_some() {
            self.title_changed_at = Some(Instant::now());
        }
    }

    /// Turn state as claude reports it in its window title: `Some(true)` while
    /// the spinner title keeps flipping, `Some(false)` once it settled on `✳`,
    /// `None` when unknown (no title yet, an unrecognised one, or — on a
    /// focused terminal only — a busy title frozen past [`TITLE_STALE`]) —
    /// callers then fall back to the screen heuristics. While the terminal is
    /// blurred the busy glyph is frozen by design and trusted as a level.
    pub fn title_busy(&self) -> Option<bool> {
        let busy = classify_title(self.title.as_deref()?)?;
        if busy && !self.blurred && self.title_changed_at?.elapsed() >= TITLE_STALE {
            return None;
        }
        Some(busy)
    }

    /// Test hook: pretend the title last changed `by` ago.
    #[cfg(test)]
    fn age_title(&mut self, by: Duration) {
        self.title_changed_at = self.title_changed_at.map(|t| t - by);
    }

    /// True once an "API Error" line has appeared this turn (until [`reset`]).
    /// Drives the red bull + the pure-mode warning banner.
    pub fn api_error(&self) -> bool {
        self.api_error_seen
    }

    /// True when the **last** turn-end footer in the tail reported background
    /// work still running (shells, monitors, background agents). Keeps the dot
    /// blue: the turn is done, but the thread still has something alive behind
    /// it that will wake it up.
    ///
    /// Derived as a level, not latched: the tail keeps old footers, so a latch
    /// re-armed on every quiet frame from a *previous* turn's footer ("1 shell,
    /// 1 monitor still running") even after a monitor-woken turn ended clean.
    /// Scoping to the last footer, and discarding it when any turn activity
    /// painted past it, makes the answer depend only on what is currently on
    /// screen.
    pub fn background_running(&self) -> bool {
        let Some(footer) = turn_end_re().find_iter(&self.tail).last() else {
            return false;
        };
        // Anything after the footer that looks like a live turn means a newer
        // turn started — the footer is history, whatever it claimed.
        let after = &self.tail[footer.end()..];
        if working_re().is_match(after) || interrupt_re().is_match(after) {
            return false;
        }
        background_running_re().is_match(&self.tail[footer.start()..])
    }

    /// True while claude is blocked on background agents / workflows (see
    /// [`waiting_agents_re`]). Not a last-line check: the status line sits
    /// *above* the input box, so it is never the bottom-most row. Holds until
    /// claude resumes, settles, prompts, or is interrupted ([`ends_wait`]).
    ///
    /// A busy state with no turn output behind it — the turn is live but
    /// claude itself paints nothing — so callers must treat it as busy
    /// *without* the output-freshness gate that unsticks every other stale
    /// frame.
    pub fn waiting_for_agents(&self) -> bool {
        self.waiting
    }

    /// True while a permission/question menu is on screen. The idle watchdog
    /// must not treat a waiting prompt as a finished turn.
    pub fn prompt_open(&self) -> bool {
        self.prompt_open
    }

    /// True if the **current** TUI frame shows a turn actively running. Unlike a
    /// whole-tail match (the append-only tail keeps every stale spinner frame, so
    /// it would read `true` forever once a spinner ever appeared), this inspects
    /// only the last non-empty line — the most recently painted row. When claude
    /// redraws over the spinner with its turn-end output, the last line is no
    /// longer a spinner/interrupt hint and this goes `false`. That edge is what
    /// drops the blue "busy" dot the moment a turn settles.
    ///
    /// Matches either the rotating spinner ([`working_re`]) **or** the far more
    /// stable "esc to interrupt" footer ([`interrupt_re`]) — whichever claude
    /// paints last — so a spinner-glyph change can't silently freeze the dot.
    pub fn is_busy_now(&self) -> bool {
        self.tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| working_re().is_match(l) || interrupt_re().is_match(l))
            .unwrap_or(false)
    }

    /// Clear all per-turn latches and the rolling tail. Call when the user (or
    /// the auto-pilot) submits a response / starts a new turn, so the next
    /// prompt or turn-end fires fresh.
    pub fn reset(&mut self) {
        self.tail.clear();
        self.prompt_open = false;
        self.poll_open = false;
        self.turn_end_seen = false;
        self.recap_seen = false;
        self.working_active = false;
        self.waiting = false;
        self.api_error_seen = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_and_osc() {
        // CSI color + OSC title + plain text.
        let raw = "\x1b[31mred\x1b[0m\x1b]0;title\x07plain";
        assert_eq!(strip_ansi(raw), "redplain");
    }

    #[test]
    fn strips_cursor_moves() {
        // Erase-line vanishes; an absolute move becomes a line break.
        assert_eq!(strip_ansi("a\x1b[2Kb\x1b[1;1Hc"), "ab\nc");
    }

    /// ConPTY sends a space as cursor-forward and a line break as an absolute
    /// move (bytes from a real Windows capture).
    #[test]
    fn cursor_forward_becomes_spaces() {
        assert_eq!(
            strip_ansi("Blocked.\x1b[1CThe\x1b[3Charness"),
            "Blocked. The   harness"
        );
        assert_eq!(strip_ansi("x\x1b[Cy"), "x y");
        assert_eq!(strip_ansi("a\x1b[13;3Hb\x1b[5Gc"), "a\n  b c");
    }

    /// The turn-end footer as ConPTY really paints it: glyph and verb joined by
    /// a cursor-forward, the row placed by an absolute move.
    #[test]
    fn detects_conpty_footer_with_background_shell() {
        let mut s = PureSniffer::new();
        let signals = s.feed(
            "\x1b[?25l\x1b[10;1H \x1b[38;2;153;153;153m\x1b[1CRan \x1b[1m1\x1b[22m shell command\x1b[K\x1b[m\r\n\r\n   \u{25CF}\x1b[m\x1b[1Cstarted\x1b[38;2;153;153;153m\x1b[14;1H\u{273B}\x1b[1CSaut\u{e9}ed for 5s \u{b7} done 16:30 \u{b7} 1 shell still running\x1b[m\x1b[24;1H\x1b[K\x1b[27;1H\u{276F} \x1b[?25h",
        );
        assert!(signals.contains(&PureSignal::TurnEnded));
        assert!(s.background_running());
    }

    /// Spinner ticks repaint a single cell each — they must not read as a
    /// prompt or a turn end.
    #[test]
    fn conpty_spinner_ticks_are_inert() {
        let mut s = PureSniffer::new();
        for glyph in [
            '\u{2736}', '*', '\u{2722}', '\u{b7}', '\u{273B}', '\u{273D}',
        ] {
            let signals = s.feed(&format!(
                "\x1b[?25l\x1b[38;2;215;119;87m\x1b[23;1H{glyph}\x1b[26;3H\x1b[?25h"
            ));
            assert!(!signals.contains(&PureSignal::TurnEnded));
            assert!(!signals.contains(&PureSignal::NeedsInput));
        }
        assert!(!s.prompt_open());
        assert!(!s.background_running());
    }

    #[test]
    fn conpty_prompt_phrase_matches() {
        let mut s = PureSniffer::new();
        s.feed("\x1b[20;3HDo\x1b[1Cyou\x1b[1Cwant\x1b[1Cto\x1b[1Cproceed?");
        assert!(s.prompt_open());
    }

    /// The numbered-menu branch on a real AskUserQuestion paint: each option
    /// row is placed by an absolute move, its indent only in the column.
    #[test]
    fn conpty_numbered_menu_matches() {
        let mut s = PureSniffer::new();
        s.feed(
            "\x1b[11;1HTea or coffee?\x1b[13;1H\u{276F}\x1b[38;2;153;153;153m\x1b[1C1.\x1b[1CTea\x1b[14;6HPrefer tea.\x1b[15;3H2.\x1b[m\x1b[1CCoffee\x1b[16;6HPrefer coffee.",
        );
        assert!(s.prompt_open());
    }

    #[test]
    fn reads_turn_state_from_window_title() {
        assert_eq!(
            osc_title("x\x1b]0;\u{25D0} Ping localhost test\x07\x1b[?25l"),
            Some("\u{25D0} Ping localhost test")
        );
        assert_eq!(
            osc_title("\x1b]2;\u{2733} Claude Code\x1b\\"),
            Some("\u{2733} Claude Code")
        );
        assert_eq!(osc_title("no title here"), None);

        let mut s = PureSniffer::new();
        assert_eq!(s.title_busy(), None);
        s.note_title("claude");
        assert_eq!(s.title_busy(), None);
        s.note_title("\u{25D1} Ping localhost test");
        assert_eq!(s.title_busy(), Some(true));
        s.note_title("\u{2733} Ping localhost test");
        assert_eq!(s.title_busy(), Some(false));
        // A submit clears the per-turn latches, not the title level.
        s.reset();
        assert_eq!(s.title_busy(), Some(false));
    }

    #[test]
    fn frozen_busy_title_ages_out_only_while_focused() {
        let mut s = PureSniffer::new();
        s.note_title("\u{25D0} cargo build");
        assert_eq!(s.title_busy(), Some(true));
        // Focused terminal, spinner frozen past the stale window → unknown.
        s.age_title(TITLE_STALE);
        assert_eq!(s.title_busy(), None);
        // Blurred terminal: claude freezes the spinner on purpose — trusted.
        s.note_focus(false);
        assert_eq!(s.title_busy(), Some(true));
        s.age_title(Duration::from_secs(600));
        assert_eq!(s.title_busy(), Some(true));
        // The settled glyph still wins while blurred.
        s.note_title("\u{2733} cargo build");
        assert_eq!(s.title_busy(), Some(false));
        // Refocus restarts the clock on a busy title (claude resumes flipping
        // on its next tick), then the stale rule applies again.
        s.note_title("\u{25D1} cargo test");
        s.note_focus(false);
        s.age_title(Duration::from_secs(60));
        s.note_focus(true);
        assert_eq!(s.title_busy(), Some(true));
        s.age_title(TITLE_STALE);
        assert_eq!(s.title_busy(), None);
    }

    #[test]
    fn focus_event_reads_last_report_in_chunk() {
        assert_eq!(focus_event("abc"), None);
        assert_eq!(focus_event("\x1b[I"), Some(true));
        assert_eq!(focus_event("\x1b[O"), Some(false));
        assert_eq!(focus_event("\x1b[I\x1b[O"), Some(false));
        assert_eq!(focus_event("\x1b[Ohello\x1b[I"), Some(true));
    }

    #[test]
    fn detects_numbered_menu_prompt() {
        let mut s = PureSniffer::new();
        let menu = "Do you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude what to do\n";
        let sigs = s.feed(menu);
        assert!(sigs.contains(&PureSignal::NeedsInput));
    }

    #[test]
    fn detects_english_proceed_prompt() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("Do you want to make this edit to lib.rs?");
        assert!(sigs.contains(&PureSignal::NeedsInput));
    }

    #[test]
    fn detects_multi_question_menu_prompt() {
        // The AskUserQuestion multi-question menu uses a `›` chevron (not `❯`),
        // long option descriptions, and a "…to navigate · Esc to cancel" footer.
        let mut s = PureSniffer::new();
        let menu = "\
← ☐ Controle abas   ☐ Escopo   ✓ Submit   →
Como deve ser o controle de cor opaca vs gradient nas abas?
› 1. Pick list (Gradient/Sólido)
    Um seletor 'Tab fill style' com opções Gradient (padrão) / Solid color, \
igual aos picklists existentes em outras telas do app que você já conhece e usa.
  2. Toggle (Solid fill)
    Um toggle simples 'Solid tab fill (no gradient)', off por padrão.
  3. Type something.
Enter to select · Tab/Arrow keys to navigate · Esc to cancel
";
        let sigs = s.feed(menu);
        assert!(sigs.contains(&PureSignal::NeedsInput));
    }

    #[test]
    fn prompt_latches_once_per_turn() {
        let mut s = PureSniffer::new();
        let first = s.feed("Do you want to proceed?");
        assert!(first.contains(&PureSignal::NeedsInput));
        // A redraw of the same prompt must not re-fire.
        let second = s.feed(" still on screen Do you want to proceed?");
        assert!(!second.contains(&PureSignal::NeedsInput));
        // After reset (user answered), it fires again.
        s.reset();
        let third = s.feed("Do you want to proceed?");
        assert!(third.contains(&PureSignal::NeedsInput));
    }

    #[test]
    fn detects_turn_end_marker() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("✶ Worked for 3m 9s");
        assert!(sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn detects_accented_turn_end_verb() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("✻ Sautéed for 11s");
        assert!(sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn detects_feedback_poll_as_turn_end() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("1: Bad   2: Fine   3: Good   0: Dismiss");
        assert!(sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn detects_recap_as_turn_end() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("※ recap of this conversation");
        assert!(sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn detects_star_glyph_recap() {
        let mut s = PureSniffer::new();
        let sigs = s.feed("✻ recap: Building the v0.8 release");
        assert!(sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn tip_line_stripped_before_working_detection() {
        let mut s = PureSniffer::new();
        // A tip line carries the ellipsis but is chrome — must not fire Working.
        let sigs = s.feed("├ Tip: Use /btw to ask a quick side question …\n");
        assert!(!sigs.contains(&PureSignal::Working));
    }

    #[test]
    fn working_spinner_reports_rising_edge_only() {
        let mut s = PureSniffer::new();
        let first = s.feed("✻ Flummoxing… (8m 38s)");
        assert!(first.contains(&PureSignal::Working));
        // Tail still contains the spinner → no second Working.
        let second = s.feed(" more output");
        assert!(!second.contains(&PureSignal::Working));
    }

    #[test]
    fn turn_end_does_not_fire_on_prose_mention() {
        let mut s = PureSniffer::new();
        // No leading glyph — must not match the turn-end marker.
        let sigs = s.feed("I worked for 3 minutes on this");
        assert!(!sigs.contains(&PureSignal::TurnEnded));
    }

    #[test]
    fn has_content_ignores_chrome_only_chunks() {
        // Animated tip hint — chrome, must not count as content.
        assert!(!has_content(
            "├ Tip: Use /btw to ask a quick side question …\n"
        ));
        // ANSI-only redraw (cursor moves + clear), no visible text.
        assert!(!has_content("\x1b[2K\x1b[1;1H\x1b[0m"));
        // Whitespace / control bytes only.
        assert!(!has_content("\r\n  \r"));
    }

    #[test]
    fn has_content_keeps_live_spinner_and_prose() {
        // The working spinner is real output — it should keep a turn alive.
        assert!(has_content("✻ Drizzling… (36s · ↓ 1.8k tokens)"));
        // Ordinary assistant text.
        assert!(has_content("\x1b[32mdeu certo, foi NETWORK SERVICE\x1b[0m"));
    }

    #[test]
    fn is_busy_now_true_while_spinner_is_last_line() {
        let mut s = PureSniffer::new();
        s.feed("some streamed thinking text\n✻ Drizzling… (36s)");
        assert!(s.is_busy_now());
    }

    #[test]
    fn is_busy_now_true_on_esc_to_interrupt_footer() {
        let mut s = PureSniffer::new();
        // No spinner glyph at all — only the stable interrupt hint.
        s.feed("streaming a long answer\n  (12s · esc to interrupt)");
        assert!(s.is_busy_now());
    }

    #[test]
    fn is_busy_now_true_on_widened_spinner_glyph() {
        let mut s = PureSniffer::new();
        // ✦ (U+2726) is outside the old ✱-✽ slice — must still count as busy.
        s.feed("✦ Sketching… (3s)");
        assert!(s.is_busy_now());
    }

    #[test]
    fn is_busy_now_false_after_spinner_redrawn_to_turn_end() {
        let mut s = PureSniffer::new();
        // Stale spinner frame retained earlier in the append-only tail …
        s.feed("✻ Cogitating… (1m)\n");
        // … then claude paints the turn-end summary + recap below it.
        s.feed("✶ Cogitated for 1m 15s\n✶ recap: fixed the upload perms\n› ");
        // Last painted line is the prompt, not a spinner → not busy.
        assert!(!s.is_busy_now());
    }

    #[test]
    fn prose_mentioning_prompt_phrase_does_not_stick_red() {
        let mut s = PureSniffer::new();
        // Assistant prose quoting a menu option — momentarily at the bottom.
        s.feed("explaining the menu: \"2. Yes, and don't ask again for X\"\n");
        assert!(
            s.prompt_open(),
            "phrase at the live bottom reads as a prompt"
        );
        // …then a lot more answer streams below it, pushing the phrase out of the
        // recent window. The red dot must clear — it was never a real menu.
        s.feed(&"more answer text follows here. ".repeat(60));
        assert!(
            !s.prompt_open(),
            "phrase scrolled up under new output must not keep needs-input latched"
        );
    }

    #[test]
    fn real_menu_at_bottom_stays_open_across_redraws() {
        let mut s = PureSniffer::new();
        s.feed("Do you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude\n");
        assert!(s.prompt_open());
        // A cursor-blink redraw of the same frame keeps the menu live.
        s.feed("Do you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude\n");
        assert!(s.prompt_open());
    }

    /// The frame claude paints while `Task` agents run: a status line above the
    /// input box, then nothing at all until an agent reports. Neither the
    /// spinner nor the "esc to interrupt" footer is on screen, so the ordinary
    /// busy tells are absent and the dot needs this one.
    #[test]
    fn detects_waiting_for_background_agents() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed(
            "\u{2726} Waiting for 2 background agents to finish\n\
             \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n\
             \u{276F} \n",
        );
        assert!(sniffer.waiting_for_agents());
        // The status line is not the bottom row (the input box is), so the
        // last-line check can't see it — that is exactly why it has its own.
        assert!(!sniffer.is_busy_now());
    }

    #[test]
    fn waiting_matches_the_singular_form() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed("\u{2726} Waiting for 1 background agent to finish\n");
        assert!(sniffer.waiting_for_agents());
    }

    /// Prose *about* background agents is not the status line. The assistant
    /// describing what it launched must not pin the dot blue.
    #[test]
    fn waiting_ignores_a_mid_line_mention() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed(
            "\u{2022} Dois agentes rodando em paralelo, aviso quando \
             terminarem de waiting for 2 background agents to finish\n",
        );
        assert!(!sniffer.waiting_for_agents());
    }

    /// A dynamic workflow blocks the turn the same way, with its own noun.
    #[test]
    fn waiting_matches_a_dynamic_workflow() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed("\u{273B} Waiting for 1 dynamic workflow to finish\n");
        assert!(sniffer.waiting_for_agents());
    }

    /// The workflow's progress row keeps ticking under the status line (ConPTY
    /// sends just the changed cells). Minutes of those diffs push the status
    /// line out of the tail entirely; the wait must hold regardless.
    #[test]
    fn waiting_survives_progress_row_ticks() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed(
            "\u{273B} Waiting for 1 dynamic workflow to finish\n\
             \u{25CB} release-review  \u{25AC}\u{25AC}\u{25AC}  2/4 \u{b7} 9m34s \u{b7} \u{2193} 590.8k tokens\n",
        );
        for i in 0..400 {
            sniffer.feed(&format!("\x1b[12;44H{i}s\x1b[12;53H59{i}.8k"));
        }
        assert!(sniffer.waiting_for_agents());
    }

    /// Once claude resumes (spinner) or settles (turn-end footer), the wait is
    /// over — otherwise the dot would stay blue forever.
    #[test]
    fn waiting_clears_when_claude_resumes_or_settles() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed("\u{2726} Waiting for 2 background agents to finish\n");
        sniffer.feed(&"agent reported back\n".repeat(40));
        assert!(sniffer.waiting_for_agents());
        sniffer.feed("\u{273B} Reticulating\u{2026} (esc to interrupt)\n");
        assert!(!sniffer.waiting_for_agents());

        let mut sniffer = PureSniffer::new();
        sniffer.feed("\u{273B} Waiting for 1 dynamic workflow to finish\n");
        sniffer.feed(&"x".repeat(3000));
        sniffer.feed("\u{2733} Worked for 12m 3s\n");
        assert!(!sniffer.waiting_for_agents());
    }

    #[test]
    fn reset_clears_waiting_for_agents() {
        let mut sniffer = PureSniffer::new();
        sniffer.feed("\u{2726} Waiting for 3 background agents to finish\n");
        assert!(sniffer.waiting_for_agents());
        sniffer.reset();
        assert!(!sniffer.waiting_for_agents());
    }

    #[test]
    fn detects_background_running_footer() {
        let mut s = PureSniffer::new();
        assert!(!s.background_running());
        s.feed("\u{2733} Worked for 15m 2s \u{b7} done 10:05 \u{b7} 1 shell, 1 monitor still running\n");
        assert!(s.background_running());
        // A plain redraw (no spinner) keeps it latched.
        s.feed("\n> \n");
        assert!(s.background_running());
    }

    #[test]
    fn detects_background_running_single_shell() {
        let mut s = PureSniffer::new();
        s.feed("\u{2736} Worked for 42s \u{b7} 1 shell still running\n");
        assert!(s.background_running());
    }

    /// Footer settled on the reference mark instead of a dingbat glyph — seen
    /// live, and the old dingbat-only class missed both the turn end and the
    /// background half.
    #[test]
    fn detects_background_running_reference_mark_footer() {
        let mut s = PureSniffer::new();
        let signals = s.feed(
            "\u{203B} Churned for 4m 38s \u{b7} done 12:51 \u{b7} 1 shell still running\n\n\
             \u{203B} recap: Estou trazendo os building:part do OSM. (disable recaps in /config)\n> \n",
        );
        assert!(signals.contains(&PureSignal::TurnEnded));
        assert!(s.background_running());
    }

    #[test]
    fn prose_about_still_running_does_not_latch_background() {
        let mut s = PureSniffer::new();
        s.feed("The monitor is still running, so the 2 tasks keep polling.\n");
        assert!(!s.background_running());
    }

    #[test]
    fn background_running_clears_when_a_new_turn_paints() {
        let mut s = PureSniffer::new();
        s.feed("\u{2733} Worked for 15m 2s \u{b7} 1 shell, 1 monitor still running\n");
        assert!(s.background_running());
        // The next turn starts: the spinner footer is the bottom-most line.
        s.feed("\u{2736} Flummoxing\u{2026} (3s \u{b7} esc to interrupt)");
        assert!(!s.background_running());
    }

    /// The monitor wakes claude, the follow-up turn ends with nothing left
    /// running — the *previous* turn's footer is still in the tail, and must not
    /// re-arm the pulse.
    #[test]
    fn background_running_is_not_relatched_from_a_previous_footer() {
        let mut s = PureSniffer::new();
        s.feed("\u{2733} Worked for 15m 2s \u{b7} 1 shell, 1 monitor still running\n");
        assert!(s.background_running());
        s.feed("\u{2736} Flummoxing\u{2026} (3s \u{b7} esc to interrupt)\n");
        s.feed("\u{2736} Worked for 12s\n> \n");
        assert!(!s.background_running());
    }

    /// Same stale footer, but the follow-up turn stops on a permission prompt —
    /// the dot is red and must not pick up a pulse from the old footer either.
    #[test]
    fn background_running_off_while_a_later_turn_prompts() {
        let mut s = PureSniffer::new();
        s.feed("\u{2733} Worked for 15m 2s \u{b7} 1 shell, 1 monitor still running\n");
        s.feed("\u{2736} Flummoxing\u{2026} (3s \u{b7} esc to interrupt)\n");
        s.feed("Do you want to proceed?\n\u{276F} 1. Yes\n  2. No, and tell Claude what to do\n");
        assert!(s.prompt_open());
        assert!(!s.background_running());
    }

    #[test]
    fn reset_clears_background_running() {
        let mut s = PureSniffer::new();
        s.feed("\u{2733} Worked for 15m 2s \u{b7} 1 shell, 1 monitor still running\n");
        assert!(s.background_running());
        s.reset();
        assert!(!s.background_running());
    }

    #[test]
    fn detects_api_error_and_latches_until_reset() {
        let mut s = PureSniffer::new();
        assert!(!s.api_error());
        s.feed("API Error: Connection closed mid-response");
        assert!(s.api_error());
        // A redraw keeps it flagged.
        s.feed(" retrying…");
        assert!(s.api_error());
        // The user's next submit clears it.
        s.reset();
        assert!(!s.api_error());
    }

    #[test]
    fn detects_parenthesized_api_error() {
        let mut s = PureSniffer::new();
        s.feed("API Error (Request timed out)");
        assert!(s.api_error());
    }

    #[test]
    fn api_error_ignores_prose_mention() {
        let mut s = PureSniffer::new();
        // Assistant prose talking about api errors, not the CLI's error line.
        s.feed("you should handle any API error gracefully in your client");
        assert!(!s.api_error());
    }

    #[test]
    fn prompt_split_across_chunks_still_matches() {
        let mut s = PureSniffer::new();
        let a = s.feed("Do you want ");
        assert!(!a.contains(&PureSignal::NeedsInput));
        let b = s.feed("to proceed?");
        assert!(b.contains(&PureSignal::NeedsInput));
    }
}
