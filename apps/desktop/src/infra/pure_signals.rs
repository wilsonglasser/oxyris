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

/// Strip ANSI escape sequences (CSI + OSC) so prompt text matches across the
/// TUI's redraws. Mirrors `stripAnsi` in `pureTurn.ts` (char-scan, not regex,
/// to handle the OSC `BEL`/`ESC \` terminators cleanly).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != '\u{1b}' {
            out.push(c);
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some('[') => {
                // CSI: skip until a final byte in @–~ (0x40–0x7e).
                i += 2;
                while i < bytes.len() {
                    let f = bytes[i];
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            Some(']') => {
                // OSC: skip until BEL (7) or ESC (start of the ST terminator).
                i += 2;
                while i < bytes.len() && bytes[i] != '\u{7}' && bytes[i] != '\u{1b}' {
                    i += 1;
                }
                // Consume a BEL terminator. An ESC terminator (ST = ESC `\`) is
                // left for the next iteration's lone-ESC branch to drop.
                if i < bytes.len() && bytes[i] == '\u{7}' {
                    i += 1;
                }
            }
            _ => {
                // Lone ESC or a 2-byte escape — drop ESC and the following byte.
                i += 2;
            }
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
            r"(?i)(❯\s*\d+\.\s+\S[\s\S]{0,400}\n\s+\d+\.\s)|(enter to select[\s\S]{0,200}navigate)|(ctrl-g to edit)|(do you want to (proceed|make this edit|create|run|continue))|(would you like to proceed)|(yes, and don'?t ask again)|(no, and tell claude)",
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
/// "done" signal. Port of `PURE_TURN_END_RE`.
fn turn_end_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)[✱-✽]\s+\S+\s+for\s+\d+[ms]").expect("pure turn-end regex"))
}

/// Live working spinner — same glyph family but a present-participle verb and a
/// trailing ellipsis ("✻ Flummoxing… (8m …)"). Tells us claude is still busy.
/// Port of `PURE_WORKING_RE`.
fn working_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[✱-✽][^\n…]*(…|\.\.\.)").expect("pure working regex"))
}

/// End-of-conversation recap line — "✻ recap: …" (current TUI) or "※ recap"
/// (older builds). A settled-turn signal that PURE_TURN_END_RE can't catch (no
/// "for <duration>"). Port of `PURE_RECAP_RE`.
fn recap_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)([✱-✽]|※)\s*recap").expect("pure recap regex"))
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
}

impl PureSniffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a raw (un-stripped) PTY output chunk. Returns the signals this chunk
    /// newly triggered, in detection order.
    pub fn feed(&mut self, data: &str) -> Vec<PureSignal> {
        let stripped = strip_tip_lines(&strip_ansi(data));
        self.tail.push_str(&stripped);
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

        if !self.prompt_open && prompt_re().is_match(&self.tail) {
            self.prompt_open = true;
            out.push(PureSignal::NeedsInput);
        }
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

        out
    }

    /// True while the live "…" working spinner was the last thing seen — the
    /// idle watchdog must not declare a turn done while claude is still thinking
    /// (extended thinking can stall output past the idle window).
    pub fn is_working(&self) -> bool {
        self.working_active
    }

    /// True while a permission/question menu is on screen. The idle watchdog
    /// must not treat a waiting prompt as a finished turn.
    pub fn prompt_open(&self) -> bool {
        self.prompt_open
    }

    /// True once a marker already settled this turn (poll / "Worked for…" /
    /// recap), so the idle watchdog can skip a redundant `TurnEnded`.
    pub fn turn_settled(&self) -> bool {
        self.poll_open || self.turn_end_seen || self.recap_seen
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
    fn prompt_split_across_chunks_still_matches() {
        let mut s = PureSniffer::new();
        let a = s.feed("Do you want ");
        assert!(!a.contains(&PureSignal::NeedsInput));
        let b = s.feed("to proceed?");
        assert!(b.contains(&PureSignal::NeedsInput));
    }
}
