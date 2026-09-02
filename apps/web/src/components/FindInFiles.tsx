import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  Loader2,
  Regex,
  Replace,
  Search,
  WholeWord,
  X,
} from "lucide-react";
import {
  fsReadFile,
  fsSearchContent,
  fsWriteFile,
  type FsSearchContentResult,
} from "~/ipc/fs.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { buildHighlightRegex, highlightMatches } from "~/lib/searchHighlight.tsx";

interface Props {
  projectId: string;
  worktreeId: string;
  open: boolean;
  /** Open with the replace row expanded (Ctrl+Shift+R vs Ctrl+Shift+F). */
  replace?: boolean;
  onClose: () => void;
}

/** Outcome of a Replace All run, rendered as a one-line summary. */
type ReplaceReport = {
  files: number;
  matches: number;
  /** relPath → why it was left alone (too large, undecodable, write failed). */
  skipped: { relPath: string; reason: string }[];
};

/** Read cap for the replace pass. Files above it are skipped rather than
 *  written back truncated — the read returns only the first `maxBytes`. */
const REPLACE_READ_CAP = 8 * 1024 * 1024;

/** Search cap for the replace pass: the on-screen list is capped at 1000
 *  matches, which is fine for browsing but would silently replace a subset. */
const REPLACE_SEARCH_CAP = 20000;

type FlatHit = { relPath: string; line: number; text: string };

/** Lines of context rendered around the selected match in the preview pane. */
const PREVIEW_CONTEXT = 80;

/**
 * Find in Files (Ctrl+Shift+F). Full-text search across the worktree with a
 * results list (matches highlighted) up top and a live preview of the
 * selected file below — the matched line is highlighted and scrolled into
 * view. Case / whole-word / regex toggles + a glob file mask mirror the
 * backend search flags.
 */
export function FindInFiles({
  projectId,
  worktreeId,
  open,
  replace: replaceProp,
  onClose,
}: Props) {
  const { t } = useTranslation("files");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);
  const [isRegex, setIsRegex] = useState(false);
  const [mask, setMask] = useState("");
  const [result, setResult] = useState<FsSearchContentResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState(0);
  const [replaceOpen, setReplaceOpen] = useState(replaceProp ?? false);
  const [replacement, setReplacement] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [report, setReport] = useState<ReplaceReport | null>(null);
  /** Bumped after a replace run so the preview pane (which caches by path)
   *  re-reads the file instead of showing the pre-replace text. */
  const [previewEpoch, setPreviewEpoch] = useState(0);
  const openFileAt = useFileEditorStore((s) => s.openFileAt);

  // Flatten file→matches into a single ordered list for keyboard nav.
  const flat = useMemo<FlatHit[]>(() => {
    if (!result) return [];
    const out: FlatHit[] = [];
    for (const f of result.files)
      for (const m of f.matches)
        out.push({ relPath: f.rel_path, line: m.line, text: m.text });
    return out;
  }, [result]);

  useEffect(() => {
    if (open) requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  // Each open honours the shortcut it was opened with (Ctrl+Shift+F → find,
  // Ctrl+Shift+R → find + replace) and starts without a stale run summary.
  useEffect(() => {
    if (!open) return;
    setReplaceOpen(replaceProp ?? false);
    setConfirming(false);
    setReport(null);
  }, [open, replaceProp]);

  // Any change to the query, the flags or the replacement text invalidates a
  // pending "click again to confirm" — the count it was showing no longer holds.
  useEffect(() => {
    setConfirming(false);
  }, [query, replacement, caseSensitive, wholeWord, isRegex, mask]);

  // Debounced search whenever the query or any flag changes.
  useEffect(() => {
    if (!open) return;
    const q = query;
    let cancelled = false;
    if (!q) {
      setResult(null);
      setError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    const handle = window.setTimeout(() => {
      void fsSearchContent({
        projectId,
        worktreeId,
        query: q,
        caseSensitive,
        isRegex,
        wholeWord,
        includeGlob: mask.trim() || null,
      })
        .then((r) => {
          if (cancelled) return;
          setResult(r);
          setSelected(0);
          setError(null);
          setLoading(false);
        })
        .catch((e) => {
          if (cancelled) return;
          setResult(null);
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [
    query,
    caseSensitive,
    wholeWord,
    isRegex,
    mask,
    open,
    projectId,
    worktreeId,
  ]);

  const re = useMemo(
    () => buildHighlightRegex(query, { caseSensitive, isRegex, wholeWord }),
    [query, caseSensitive, isRegex, wholeWord],
  );

  const current = flat[selected] ?? null;

  /**
   * Replace every match across the whole worktree, file by file.
   *
   * Frontend-driven on purpose: `fsSearchContent` / `fsReadFile` /
   * `fsWriteFile` already route by project environment (native on Windows,
   * agent ops inside the distro on WSL), so this needs no new backend op and
   * can't accidentally bypass that routing.
   *
   * The writes are plain disk writes — there is no undo. Files are skipped
   * rather than mangled when reading them can't round-trip: over the read cap
   * (the read would come back truncated) or not valid UTF-8 (the read is
   * lossy, so writing it back would corrupt bytes).
   */
  const replaceAll = async () => {
    if (!query || replacing) return;
    setReplacing(true);
    setError(null);
    setReport(null);
    try {
      // The on-screen result is capped for rendering; re-run wide so a replace
      // never silently covers only the first page of matches.
      const full = await fsSearchContent({
        projectId,
        worktreeId,
        query,
        caseSensitive,
        isRegex,
        wholeWord,
        includeGlob: mask.trim() || null,
        maxResults: REPLACE_SEARCH_CAP,
      });
      if (full.truncated) {
        setError(t("replace_too_many", { max: REPLACE_SEARCH_CAP }));
        return;
      }
      const skipped: ReplaceReport["skipped"] = [];
      let changedFiles = 0;
      let changedMatches = 0;
      for (const f of full.files) {
        try {
          const read = await fsReadFile({
            projectId,
            worktreeId,
            relPath: f.rel_path,
            maxBytes: REPLACE_READ_CAP,
          });
          if (read.truncated) {
            skipped.push({ relPath: f.rel_path, reason: t("replace_skip_large") });
            continue;
          }
          // U+FFFD in the read means the backend decoded lossily — writing the
          // string back would replace those bytes with the marker for real.
          if (read.content.includes("�")) {
            skipped.push({ relPath: f.rel_path, reason: t("replace_skip_binary") });
            continue;
          }
          // Fresh regex per file: the shared one carries `lastIndex` state.
          const fileRe = buildHighlightRegex(query, {
            caseSensitive,
            isRegex,
            wholeWord,
          });
          if (!fileRe) {
            skipped.push({ relPath: f.rel_path, reason: t("replace_skip_regex") });
            continue;
          }
          const hits = read.content.match(fileRe)?.length ?? 0;
          if (hits === 0) continue;
          fileRe.lastIndex = 0;
          // Regex mode passes the replacement through so `$1` &co. expand;
          // literal mode goes through a function so a `$` stays a `$`.
          const next = isRegex
            ? read.content.replace(fileRe, replacement)
            : read.content.replace(fileRe, () => replacement);
          if (next === read.content) continue;
          await fsWriteFile({
            projectId,
            worktreeId,
            relPath: f.rel_path,
            content: next,
          });
          changedFiles += 1;
          changedMatches += hits;
        } catch (e) {
          skipped.push({
            relPath: f.rel_path,
            reason: e instanceof Error ? e.message : String(e),
          });
        }
      }
      setReport({ files: changedFiles, matches: changedMatches, skipped });
      // Re-run the visible search so the list reflects what's left on disk.
      const refreshed = await fsSearchContent({
        projectId,
        worktreeId,
        query,
        caseSensitive,
        isRegex,
        wholeWord,
        includeGlob: mask.trim() || null,
      });
      setResult(refreshed);
      setSelected(0);
      setPreviewEpoch((n) => n + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setReplacing(false);
      setConfirming(false);
    }
  };

  if (!open) return null;

  const openCurrent = () => {
    if (!current) return;
    onClose();
    void openFileAt(projectId, worktreeId, current.relPath, current.line);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-12"
      onClick={onClose}
    >
      <div
        className="flex h-[80vh] w-full max-w-4xl flex-col overflow-hidden rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Query row + flag toggles */}
        <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-2">
          <Search size={15} className="shrink-0 text-neutral-500" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setSelected((c) => Math.min(c + 1, flat.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setSelected((c) => Math.max(c - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                openCurrent();
              } else if (e.key === "Escape") {
                e.preventDefault();
                onClose();
              }
            }}
            placeholder={t("find_in_files_placeholder")}
            className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
          />
          <FlagToggle
            active={replaceOpen}
            onClick={() => setReplaceOpen((v) => !v)}
            title={t("replace_toggle")}
          >
            {replaceOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </FlagToggle>
          <FlagToggle
            active={caseSensitive}
            onClick={() => setCaseSensitive((v) => !v)}
            title={t("match_case")}
          >
            <CaseSensitive size={14} />
          </FlagToggle>
          <FlagToggle
            active={wholeWord}
            onClick={() => setWholeWord((v) => !v)}
            title={t("whole_word")}
          >
            <WholeWord size={14} />
          </FlagToggle>
          <FlagToggle
            active={isRegex}
            onClick={() => setIsRegex((v) => !v)}
            title={t("use_regex")}
          >
            <Regex size={14} />
          </FlagToggle>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
            aria-label={t("close_tab")}
          >
            <X size={14} />
          </button>
        </div>

        {/* Replace row — hidden until toggled (or opened via Ctrl+Shift+R) */}
        {replaceOpen && (
          <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-2">
            <Replace size={15} className="shrink-0 text-neutral-500" />
            <input
              type="text"
              value={replacement}
              onChange={(e) => setReplacement(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  onClose();
                }
              }}
              placeholder={
                isRegex ? t("replace_placeholder_regex") : t("replace_placeholder")
              }
              className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
            />
            <button
              type="button"
              disabled={!query || replacing || loading}
              onClick={() => {
                // Two-step: the first click turns the button into the count it
                // is about to write. Disk writes here have no undo.
                if (!confirming) {
                  setConfirming(true);
                  return;
                }
                void replaceAll();
              }}
              className={`shrink-0 rounded px-2.5 py-1 text-[11px] font-medium disabled:cursor-not-allowed disabled:opacity-40 ${
                confirming
                  ? "bg-amber-500/90 text-amber-950 hover:bg-amber-400"
                  : "bg-neutral-200 text-neutral-900 hover:bg-white"
              }`}
            >
              {replacing ? (
                <span className="flex items-center gap-1.5">
                  <Loader2 size={12} className="animate-spin" />
                  {t("replace_running")}
                </span>
              ) : confirming ? (
                t("replace_confirm", {
                  // The visible result is capped; say "1000+" rather than
                  // promising a number the wide re-search will exceed.
                  matches: result
                    ? `${result.total_matches}${result.truncated ? "+" : ""}`
                    : "0",
                  files: `${result?.files.length ?? 0}${result?.truncated ? "+" : ""}`,
                })
              ) : (
                t("replace_all")
              )}
            </button>
          </div>
        )}

        {/* File mask + summary */}
        <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-1.5">
          <span className="text-[11px] text-neutral-500">{t("file_mask")}</span>
          <input
            type="text"
            value={mask}
            onChange={(e) => setMask(e.target.value)}
            placeholder="*.ts, *.rs"
            className="w-48 rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
          />
          <span className="ml-auto text-[11px] text-neutral-500">
            {loading
              ? t("loading")
              : result
                ? t("matches_in_files", {
                    matches: result.total_matches,
                    files: result.files.length,
                  })
                : ""}
            {result?.truncated ? ` · ${t("truncated")}` : ""}
          </span>
        </div>

        {error && (
          <div className="border-b border-neutral-800 px-3 py-1.5 text-[11px] text-red-400" role="alert">
            {error}
          </div>
        )}

        {report && (
          <div
            role="status"
            className="border-b border-neutral-800 px-3 py-1.5 text-[11px] text-emerald-300"
          >
            {t("replace_done", { matches: report.matches, files: report.files })}
            {report.skipped.length > 0 && (
              <span className="ml-2 text-amber-300">
                {t("replace_skipped", { count: report.skipped.length })}:{" "}
                {report.skipped
                  .slice(0, 3)
                  .map((s) => `${s.relPath} (${s.reason})`)
                  .join(", ")}
                {report.skipped.length > 3 ? "…" : ""}
              </span>
            )}
          </div>
        )}

        {/* Results list */}
        <div className="min-h-0 flex-1 overflow-auto">
          {flat.length === 0 && !loading && (
            <div className="px-3 py-6 text-center text-[12px] text-neutral-500">
              {query ? t("search_no_results") : t("find_in_files_hint")}
            </div>
          )}
          {result?.files.map((f) => (
            <div key={f.rel_path}>
              <div className="sticky top-0 z-10 bg-neutral-900/95 px-3 py-1 text-[11px] text-neutral-400 backdrop-blur">
                {f.rel_path}
                <span className="ml-2 text-neutral-600">{f.matches.length}</span>
              </div>
              {f.matches.map((m) => {
                const idx = flat.findIndex(
                  (h) => h.relPath === f.rel_path && h.line === m.line,
                );
                const activeRow = idx === selected;
                return (
                  <button
                    key={`${f.rel_path}:${m.line}`}
                    type="button"
                    onClick={() => setSelected(idx)}
                    onDoubleClick={openCurrent}
                    className={`flex w-full items-baseline gap-2 px-3 py-0.5 text-left font-mono text-[11px] ${
                      activeRow
                        ? "bg-neutral-800 text-neutral-100"
                        : "text-neutral-400 hover:bg-neutral-900"
                    }`}
                  >
                    <span className="w-10 shrink-0 text-right text-neutral-600">
                      {m.line}
                    </span>
                    <span className="truncate whitespace-pre">
                      {highlightMatches(m.text, re)}
                    </span>
                  </button>
                );
              })}
            </div>
          ))}
        </div>

        {/* Preview pane */}
        <PreviewPane
          key={previewEpoch}
          projectId={projectId}
          worktreeId={worktreeId}
          hit={current}
          re={re}
        />
      </div>
    </div>
  );
}

function FlagToggle({
  active,
  onClick,
  title,
  children,
}: {
  active: boolean;
  onClick: () => void;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-pressed={active}
      className={`rounded p-1 ${
        active
          ? "bg-sky-500/20 text-sky-300"
          : "text-neutral-500 hover:bg-neutral-800 hover:text-neutral-300"
      }`}
    >
      {children}
    </button>
  );
}

function PreviewPane({
  projectId,
  worktreeId,
  hit,
  re,
}: {
  projectId: string;
  worktreeId: string;
  hit: FlatHit | null;
  re: RegExp | null;
}) {
  const { t } = useTranslation("files");
  const [content, setContent] = useState<{ relPath: string; lines: string[] } | null>(
    null,
  );
  const selectedRef = useRef<HTMLDivElement | null>(null);

  // Load the file content when the selected file changes (not on every line).
  useEffect(() => {
    if (!hit) {
      setContent(null);
      return;
    }
    if (content?.relPath === hit.relPath) return;
    let cancelled = false;
    void fsReadFile({ projectId, worktreeId, relPath: hit.relPath })
      .then((r) => {
        if (!cancelled)
          setContent({ relPath: hit.relPath, lines: r.content.split(/\r?\n/) });
      })
      .catch(() => {
        if (!cancelled) setContent(null);
      });
    return () => {
      cancelled = true;
    };
  }, [hit, projectId, worktreeId, content?.relPath]);

  // Keep the selected line centered in the preview.
  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "center" });
  }, [hit?.relPath, hit?.line, content]);

  if (!hit || !content) {
    return (
      <div className="flex h-1/2 shrink-0 items-center justify-center border-t border-neutral-800 text-[12px] text-neutral-600">
        {t("find_in_files_preview_hint")}
      </div>
    );
  }

  const start = Math.max(0, hit.line - 1 - PREVIEW_CONTEXT);
  const end = Math.min(content.lines.length, hit.line + PREVIEW_CONTEXT);
  const slice = content.lines.slice(start, end);

  return (
    <div className="h-1/2 shrink-0 overflow-auto border-t border-neutral-800 bg-neutral-900/30 font-mono text-[11px] leading-[1.5]">
      {slice.map((line, i) => {
        const lineNo = start + i + 1;
        const isMatch = lineNo === hit.line;
        return (
          <div
            key={lineNo}
            ref={isMatch ? selectedRef : null}
            className={`flex ${isMatch ? "bg-amber-400/10" : ""}`}
          >
            <span className="w-12 shrink-0 select-none border-r border-neutral-800/60 px-2 text-right text-neutral-600">
              {lineNo}
            </span>
            <span className="whitespace-pre px-2 text-neutral-300">
              {isMatch ? highlightMatches(line, re) : line}
            </span>
          </div>
        );
      })}
    </div>
  );
}
