/**
 * Most-recently-used term lists for the Find in Files dialog (one for the
 * query, one for the replacement), persisted in localStorage so they survive
 * reloads.
 *
 * Deliberately dumb: a capped string array per key, no store, no IPC. Terms
 * are pushed at commit points (opening a hit, running a replace, closing the
 * dialog) rather than on every keystroke, so the list holds searches the user
 * actually ran instead of every prefix they typed.
 */

/** How many terms each list keeps. Older entries fall off the end. */
export const SEARCH_HISTORY_MAX = 10;

export type SearchHistoryKind = "query" | "replace";

const KEYS: Record<SearchHistoryKind, string> = {
  query: "oxyris.find.history.query",
  replace: "oxyris.find.history.replace",
};

export function readSearchHistory(kind: SearchHistoryKind): string[] {
  try {
    const raw = window.localStorage.getItem(KEYS[kind]);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((v): v is string => typeof v === "string" && v.length > 0)
      .slice(0, SEARCH_HISTORY_MAX);
  } catch {
    /* localStorage may be unavailable, or the entry may be corrupt */
    return [];
  }
}

/**
 * Push `term` to the front of the list and return the new list. An empty term
 * is ignored; an existing one moves to the front instead of duplicating.
 */
export function pushSearchHistory(
  kind: SearchHistoryKind,
  term: string,
): string[] {
  const value = term.trim();
  if (!value) return readSearchHistory(kind);
  const next = [
    value,
    ...readSearchHistory(kind).filter((v) => v !== value),
  ].slice(0, SEARCH_HISTORY_MAX);
  try {
    window.localStorage.setItem(KEYS[kind], JSON.stringify(next));
  } catch {
    /* persisting is best-effort; the in-memory list is still returned */
  }
  return next;
}

export function clearSearchHistory(kind: SearchHistoryKind): string[] {
  try {
    window.localStorage.removeItem(KEYS[kind]);
  } catch {
    /* nothing to do */
  }
  return [];
}
