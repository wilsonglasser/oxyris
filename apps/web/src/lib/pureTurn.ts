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

/**
 * Rolling-tail sniffer for the pure-mode permission/question menu. Feed it raw
 * PTY output; `onOpen` fires once when the menu first appears (latched, so a
 * redraw doesn't re-fire). `reset()` clears the latch when the user answers or
 * a new turn starts. A 2000-char tail handles the menu being split across
 * output chunks.
 */
export function createPromptSniffer(onOpen: () => void) {
  let tail = "";
  let open = false;
  return {
    feed(data: string) {
      tail = (tail + stripAnsi(data)).slice(-2000);
      if (PURE_PROMPT_RE.test(tail) && !open) {
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
