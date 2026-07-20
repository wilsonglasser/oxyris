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

/// Strip ANSI escape sequences (CSI + OSC) so prompt text matches across the
/// TUI's redraws. Mirrors `stripAnsi` in `pureTurn.ts` (char-scan, not regex,
/// to handle the OSC `BEL`/`ESC \` terminators cleanly).
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
                // CSI: skip until a final byte in @–~ (0x40–0x7e), inclusive.
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
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
/// original `✱-✽` (see [`working_re`]).
fn turn_end_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[\x{2720}-\x{274F}]\s+\S+\s+for\s+\d+[ms]").expect("pure turn-end regex")
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
    /// Latched once an "API Error" line appears, until [`reset`] (the user's next
    /// submit). Drives the red bull + warning banner in pure mode.
    api_error_seen: bool,
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

        out
    }

    /// True once an "API Error" line has appeared this turn (until [`reset`]).
    /// Drives the red bull + the pure-mode warning banner.
    pub fn api_error(&self) -> bool {
        self.api_error_seen
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
        assert_eq!(strip_ansi("a\x1b[2Kb\x1b[1;1Hc"), "abc");
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
