// Shared helpers for reading turn state out of a pure (PTY claude) session.
// Pure threads have no structured turn-event stream — the claude TUI is opaque
// bytes — so the sidebar bull is driven by sniffing the raw PTY output. Both
// the in-view panel (PureClaudePanel) and the background watchers (App, Sidebar)
// use these so the prompt regex and ANSI stripping stay in one place.

// Strip ANSI escape sequences (CSI + OSC) from raw PTY bytes so prompt text
// matches across redraws. Char-code based to keep raw control bytes out of
// source. ESC = 27, BEL = 7.
export function stripAnsi(s: string): string {
  let out = "";
  for (let i = 0; i < s.length; i += 1) {
    if (s.charCodeAt(i) !== 27) {
      out += s[i];
      continue;
    }
    const next = s[i + 1];
    if (next === "[") {
      // CSI: skip until a final byte in @–~ (0x40–0x7e).
      i += 2;
      while (i < s.length) {
        const c = s.charCodeAt(i);
        if (c >= 0x40 && c <= 0x7e) break;
        i += 1;
      }
    } else if (next === "]") {
      // OSC: skip until BEL or ESC (start of the ST terminator).
      i += 2;
      while (i < s.length && s.charCodeAt(i) !== 7 && s.charCodeAt(i) !== 27) {
        i += 1;
      }
    } else {
      // Lone ESC or a 2-byte escape — drop ESC and the following byte.
      i += 1;
    }
  }
  return out;
}

// The claude TUI renders a numbered menu ("Do you want to proceed?" + "❯ 1.
// Yes" / "…don't ask again") whenever it needs the user to approve a tool or
// answer a question. This is the pure-mode equivalent of a tool-approval
// request: while it's on screen the thread "wants an input" (red bull).
export const PURE_PROMPT_RE =
  /(do you want to (proceed|make this edit|create|run|continue))|(❯\s*\d+\.\s*yes)|(yes, and don'?t ask again)|(no, and tell claude)/i;

// claude's optional end-of-session feedback poll ("How is Claude doing this
// session?" + "1: Bad   2: Fine   3: Good   0: Dismiss"). Distinct from the
// approval menu: the user isn't *required* to answer, so this should not light
// the red "needs input" bull — it just signals that the turn ended (orange
// attention for background threads, green for the active one). The poll's
// blinking cursor keeps the PTY dripping output, defeating the idle-clear, so
// the bull would otherwise stay blue forever. Requires the whole numbered row
// in order so an assistant message that merely mentions the strings in prose
// can't fire it.
export const PURE_POLL_RE =
  /1:\s*bad\s+2:\s*fine\s+3:\s*good\s+0:\s*dismiss/i;

// claude's turn-end summary line: a star-like glyph followed by a past-tense
// verb and "for <duration>" — e.g. "✶ Worked for 3m 9s", "✻ Brewed for 12s",
// "✼ Crunched for 42s". When this appears the turn is settled; we use it as a
// hard "done" signal because the composer's blinking cursor (and the TUI's
// ticking footer) can keep the PTY output dripping past the idle-clear window,
// stranding the blue pulse on. The leading glyph filters out prose mentions.
export const PURE_TURN_END_RE =
  /[✱-✽]\s+\w+\s+for\s+\d+[ms]/i;

/**
 * Rolling-tail sniffer for a pure-mode TUI pattern. Feed it raw PTY output;
 * `onOpen` fires once when the pattern first matches (latched, so a redraw
 * doesn't re-fire). `reset()` clears the latch when the user answers or a new
 * turn starts. A 2000-char tail handles the match being split across output
 * chunks. Defaults to the approval-menu pattern; pass `PURE_POLL_RE` (or any
 * other regex) for a different signal.
 */
export function createPromptSniffer(
  onOpen: () => void,
  re: RegExp = PURE_PROMPT_RE,
) {
  let tail = "";
  let open = false;
  return {
    feed(data: string) {
      tail = (tail + stripAnsi(data)).slice(-2000);
      if (re.test(tail) && !open) {
        open = true;
        onOpen();
      }
    },
    get open() {
      return open;
    },
    reset() {
      open = false;
      tail = "";
    },
  };
}
