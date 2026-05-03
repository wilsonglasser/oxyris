/**
 * Split a unified-diff blob (the kind `git diff` emits) into independent
 * hunks so we can offer per-hunk stage / unstage. Each hunk carries its
 * `@@ ... @@` header + the body lines that follow until the next header.
 *
 * The file header (`diff --git ...`, `--- a/path`, `+++ b/path`) is
 * captured separately so callers can stitch it onto a single hunk to form
 * a self-contained patch suitable for `git apply --cached`.
 */

export type Hunk = {
  /** Verbatim header line, e.g. `@@ -10,3 +12,5 @@`. */
  header: string;
  /** Lines that follow until the next hunk (each starts with ` `, `+` or `-`). */
  body: string;
  /** Source line range (parsed from the header's `-` part). */
  oldStart: number;
  oldCount: number;
  /** Destination line range (parsed from the header's `+` part). */
  newStart: number;
  newCount: number;
  /** Counts of inserted / deleted lines for UI labels. */
  added: number;
  removed: number;
};

export type ParsedDiff = {
  /** File header lines (everything before the first `@@` header). */
  fileHeader: string;
  hunks: Hunk[];
};

const HUNK_HEADER = /^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@/;

export function parseUnifiedDiff(unified: string, path: string): ParsedDiff {
  const lines = unified.split(/\r?\n/);
  const fileHeaderLines: string[] = [];
  const hunks: Hunk[] = [];
  let currentHunk: Hunk | null = null;

  for (const line of lines) {
    if (HUNK_HEADER.test(line)) {
      if (currentHunk) hunks.push(currentHunk);
      const m = HUNK_HEADER.exec(line)!;
      currentHunk = {
        header: line,
        body: "",
        oldStart: parseInt(m[1] ?? "0", 10),
        oldCount: parseInt(m[2] ?? "1", 10),
        newStart: parseInt(m[3] ?? "0", 10),
        newCount: parseInt(m[4] ?? "1", 10),
        added: 0,
        removed: 0,
      };
    } else if (currentHunk) {
      currentHunk.body += line + "\n";
      if (line.startsWith("+") && !line.startsWith("+++")) {
        currentHunk.added++;
      } else if (line.startsWith("-") && !line.startsWith("---")) {
        currentHunk.removed++;
      }
    } else {
      // Pre-hunk content is the file header. Strip empty trailing lines.
      if (line.length > 0 || fileHeaderLines.length > 0) {
        fileHeaderLines.push(line);
      }
    }
  }
  if (currentHunk) hunks.push(currentHunk);

  // Synthesize a minimal file header when git2 didn't include one (it
  // sometimes omits `--- / +++` for plain unified output).
  let fileHeader = fileHeaderLines.join("\n");
  if (!fileHeader.includes("--- ")) {
    fileHeader = [
      `diff --git a/${path} b/${path}`,
      `--- a/${path}`,
      `+++ b/${path}`,
    ].join("\n");
  }
  if (fileHeader.length > 0 && !fileHeader.endsWith("\n")) {
    fileHeader += "\n";
  }

  return { fileHeader, hunks };
}

/** Build a self-contained patch for a single hunk, ready for `git apply`. */
export function buildSingleHunkPatch(diff: ParsedDiff, hunk: Hunk): string {
  return `${diff.fileHeader}${hunk.header}\n${hunk.body}`;
}
