/**
 * Per-session composer drafts.
 *
 * The chat panel is keyed by session id, so switching conversations remounts
 * the composer and would otherwise discard whatever the user had typed. This
 * module holds draft text outside the React tree, keyed by the composer's
 * `sessionKey`, so a remount re-hydrates the in-progress message instead of
 * losing it. Cleared when the message is sent.
 */
const drafts = new Map<string, string>();

/** Current draft for a session key, or empty string if none. */
export function getDraft(sessionKey: string): string {
  return drafts.get(sessionKey) ?? "";
}

/** Store (or clear, when empty) the draft for a session key. */
export function setDraft(sessionKey: string, text: string): void {
  if (text.length === 0) drafts.delete(sessionKey);
  else drafts.set(sessionKey, text);
}
