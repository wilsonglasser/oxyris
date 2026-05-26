/**
 * Per-session composer drafts.
 *
 * The chat panel is keyed by session id, so switching conversations remounts
 * the composer and would otherwise discard whatever the user had typed. This
 * module holds draft text keyed by the composer's `sessionKey` so a remount
 * re-hydrates the in-progress message instead of losing it. Cleared when the
 * message is sent.
 *
 * Backed by `localStorage`, not just an in-memory Map: when the WebView is
 * backgrounded (alt-tab to another window) Windows' WebView2 can suspend and
 * reload the document on resume. Everything else recovers because focus
 * re-fetches from the event log, but an unsent draft lives only on the client
 * — so it has to survive a document reload too. The Map is kept as a hot cache
 * so the common keystroke path doesn't hit storage on every read.
 */
const KEY_PREFIX = "oxyris.draft.";
const drafts = new Map<string, string>();

/** Current draft for a session key, or empty string if none. */
export function getDraft(sessionKey: string): string {
  const cached = drafts.get(sessionKey);
  if (cached !== undefined) return cached;
  try {
    const stored = window.localStorage.getItem(KEY_PREFIX + sessionKey);
    if (stored !== null) {
      drafts.set(sessionKey, stored);
      return stored;
    }
  } catch {
    /* localStorage may be disabled in odd contexts */
  }
  return "";
}

/** Store (or clear, when empty) the draft for a session key. */
export function setDraft(sessionKey: string, text: string): void {
  if (text.length === 0) {
    drafts.delete(sessionKey);
    try {
      window.localStorage.removeItem(KEY_PREFIX + sessionKey);
    } catch {
      /* localStorage may be disabled in odd contexts */
    }
    return;
  }
  drafts.set(sessionKey, text);
  try {
    window.localStorage.setItem(KEY_PREFIX + sessionKey, text);
  } catch {
    /* localStorage may be disabled in odd contexts */
  }
}
