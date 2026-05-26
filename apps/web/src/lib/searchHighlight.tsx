import type { ReactNode } from "react";

/**
 * Build a global RegExp for highlighting search matches inside a line/label.
 * Mirrors the backend matcher flags so the cosmetic highlight lines up with
 * what was actually matched. Returns `null` for an empty query or an invalid
 * user regex (caller then renders the text un-highlighted).
 */
export function buildHighlightRegex(
  query: string,
  opts: { caseSensitive?: boolean; isRegex?: boolean; wholeWord?: boolean } = {},
): RegExp | null {
  if (!query) return null;
  let src = opts.isRegex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (opts.wholeWord) src = `\\b(?:${src})\\b`;
  try {
    return new RegExp(src, opts.caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

/**
 * Split `text` on `re` and wrap matched spans in a `<mark>`. When `re` is
 * null the text is returned as a single node.
 */
export function highlightMatches(text: string, re: RegExp | null): ReactNode[] {
  if (!re) return [text];
  const out: ReactNode[] = [];
  let last = 0;
  let i = 0;
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(
      <mark
        key={i++}
        className="rounded-[2px] bg-amber-400/25 text-amber-200"
      >
        {m[0]}
      </mark>,
    );
    last = m.index + m[0].length;
    // Guard against zero-width matches looping forever.
    if (m[0].length === 0) re.lastIndex += 1;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
