import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  History,
  Loader2,
  Regex,
  Replace,
  Save,
  Search,
  Trash2,
  WholeWord,
  X,
} from "lucide-react";
import {
  Compartment,
  EditorSelection,
  EditorState,
  RangeSetBuilder,
  type Extension,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { bracketMatching } from "@codemirror/language";
import { defaultKeymap, history as cmHistory, historyKeymap } from "@codemirror/commands";
import {
  fsReadFile,
  fsSearchContent,
  fsWriteFile,
  type FsSearchContentResult,
} from "~/ipc/fs.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { buildHighlightRegex, highlightMatches } from "~/lib/searchHighlight.tsx";
import { islandDark } from "~/lib/codemirror-theme.ts";
import { languageForPath } from "~/lib/codemirror-language.ts";
import {
  MenuItem,
  MenuSeparator,
  MenuSurface,
  useMenuDismiss,
} from "~/components/MenuSurface.tsx";
import {
  clearSearchHistory,
  pushSearchHistory,
  readSearchHistory,
  type SearchHistoryKind,
} from "~/lib/searchHistory.ts";

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

/**
 * Find in Files (Ctrl+Shift+F). Full-text search across the worktree with a
 * results list (matches highlighted) up top and the selected file below in an
 * editable, syntax-colored editor — the matched line is centered, every
 * occurrence is highlighted, and Ctrl+S writes the file back without leaving
 * the dialog. Case / whole-word / regex toggles + a glob file mask mirror the
 * backend search flags; both inputs keep an MRU list of the last
 * `SEARCH_HISTORY_MAX` terms behind the chevron next to them.
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
  /** MRU term lists, mirrored in state so the dropdowns re-render on push. */
  const [queryHistory, setQueryHistory] = useState<string[]>(() =>
    readSearchHistory("query"),
  );
  const [replaceHistory, setReplaceHistory] = useState<string[]>(() =>
    readSearchHistory("replace"),
  );
  /** Paths with unsaved edits made in the preview editor. */
  const [dirtyPreview, setDirtyPreview] = useState<string[]>([]);
  /** First Escape/X press while the preview has unsaved edits only arms. */
  const [discardArmed, setDiscardArmed] = useState(false);
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

  // Focus AND select: the dialog keeps the last query, and reopening it almost
  // always means searching for something else — typing should replace the term,
  // not append to it.
  useEffect(() => {
    if (!open) return;
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.focus();
      el.select();
    });
  }, [open]);

  // Terms reach the MRU lists at commit points only — opening a hit, running a
  // replace, closing the dialog. Recording from the debounced search effect
  // instead would fill the list with every prefix typed on the way to a term.
  const commitHistory = useCallback(() => {
    if (query.trim()) setQueryHistory(pushSearchHistory("query", query));
    if (replaceOpen && replacement.trim())
      setReplaceHistory(pushSearchHistory("replace", replacement));
  }, [query, replacement, replaceOpen]);

  /** Close after recording history, guarding unsaved preview edits behind the
   *  same two-step confirm the replace button uses. */
  const close = useCallback(() => {
    commitHistory();
    if (dirtyPreview.length > 0 && !discardArmed) {
      setDiscardArmed(true);
      return;
    }
    onClose();
  }, [commitHistory, dirtyPreview.length, discardArmed, onClose]);

  // Escape works from anywhere in the dialog, not only from the two inputs —
  // clicking a result row moves focus off them, and the backdrop no longer
  // closes. A pending "click again to confirm" replace is cancelled first, so
  // Escape never closes the dialog out from under an armed write.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (confirming) setConfirming(false);
      else close();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirming, close]);

  // Each open honours the shortcut it was opened with (Ctrl+Shift+F → find,
  // Ctrl+Shift+R → find + replace) and starts without a stale run summary.
  useEffect(() => {
    if (!open) return;
    setReplaceOpen(replaceProp ?? false);
    setConfirming(false);
    setReport(null);
    setDiscardArmed(false);
    setDirtyPreview([]);
    setQueryHistory(readSearchHistory("query"));
    setReplaceHistory(readSearchHistory("replace"));
  }, [open, replaceProp]);

  // A save (or a discard) in the preview clears the armed close prompt — it was
  // warning about edits that no longer exist.
  useEffect(() => {
    if (dirtyPreview.length === 0) setDiscardArmed(false);
  }, [dirtyPreview.length]);

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
    commitHistory();
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
      // The preview remounts, so its per-path drafts are gone — the parent's
      // dirty list has to go with them or the close guard would warn about
      // edits that no longer exist.
      setDirtyPreview([]);
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
    commitHistory();
    onClose();
    void openFileAt(projectId, worktreeId, current.relPath, current.line);
  };

  const openAt = (idx: number) => {
    const hit = flat[idx];
    if (!hit) return;
    commitHistory();
    setSelected(idx);
    onClose();
    void openFileAt(projectId, worktreeId, hit.relPath, hit.line);
  };

  /**
   * A save from the preview editor leaves the result list stale — lines shift
   * and edited matches disappear. Re-run the same search and re-point the
   * selection at the file the user was editing (nearest surviving line) rather
   * than snapping back to the first hit of the first file. The preview is NOT
   * remounted here: it holds the buffer that was just written.
   */
  const refreshAfterSave = async (relPath: string, line: number) => {
    if (!query) return;
    try {
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
      let best = 0;
      let bestDist = Number.POSITIVE_INFINITY;
      let idx = 0;
      for (const f of refreshed.files)
        for (const m of f.matches) {
          if (f.rel_path === relPath) {
            const dist = Math.abs(m.line - line);
            if (dist < bestDist) {
              bestDist = dist;
              best = idx;
            }
          }
          idx += 1;
        }
      setSelected(best);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-12">
      <div className="flex h-[80vh] w-full max-w-4xl flex-col overflow-hidden rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl">
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
              }
            }}
            placeholder={t("find_in_files_placeholder")}
            className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
          />
          <HistoryMenu
            kind="query"
            entries={queryHistory}
            title={t("search_history")}
            onPick={(term) => {
              setQuery(term);
              inputRef.current?.focus();
            }}
            onCleared={() => setQueryHistory([])}
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
            onClick={close}
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
              placeholder={
                isRegex ? t("replace_placeholder_regex") : t("replace_placeholder")
              }
              className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
            />
            <HistoryMenu
              kind="replace"
              entries={replaceHistory}
              title={t("replace_history")}
              onPick={(term) => setReplacement(term)}
              onCleared={() => setReplaceHistory([])}
            />
            <button
              type="button"
              // A replace pass rewrites files from disk and remounts the
              // preview, which would throw away unsaved edits there.
              disabled={
                !query || replacing || loading || dirtyPreview.length > 0
              }
              title={
                dirtyPreview.length > 0
                  ? t("preview_unsaved_blocks_replace")
                  : undefined
              }
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

        {discardArmed && (
          <div
            role="alert"
            className="border-b border-neutral-800 px-3 py-1.5 text-[11px] text-amber-300"
          >
            {t("preview_discard_confirm", { count: dirtyPreview.length })}
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
                    // Single click only previews — the pane below is a real
                    // editor now, and closing the dialog on the click that
                    // selects a match would make it reachable by arrow keys
                    // only. Double click (or Enter) still opens it in a tab.
                    onClick={() => setSelected(idx)}
                    onDoubleClick={() => openAt(idx)}
                    title={t("find_in_files_open_hint")}
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

        {/* Preview pane — a real editor: colorized, editable, Ctrl+S saves */}
        <PreviewPane
          key={previewEpoch}
          projectId={projectId}
          worktreeId={worktreeId}
          hit={current}
          re={re}
          onDirtyChange={setDirtyPreview}
          onSaved={refreshAfterSave}
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

/**
 * Dropdown of recently used terms for one of the two inputs. Rendered as the
 * chevron sitting inside the input row; picking a row applies the term.
 */
function HistoryMenu({
  kind,
  entries,
  title,
  onPick,
  onCleared,
}: {
  kind: SearchHistoryKind;
  entries: string[];
  title: string;
  onPick: (term: string) => void;
  onCleared: () => void;
}) {
  const { t } = useTranslation("files");
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null);
  // `MenuSurface` holds an unconditional hook, so the dismiss handler has to
  // live out here where it is always mounted.
  useMenuDismiss(anchor !== null, () => setAnchor(null));
  // Escape must close only the menu. The dialog's own Escape handler is on
  // `document` and nothing stops propagation, so without a capture-phase
  // handler one press would close the menu *and* the whole dialog.
  useEffect(() => {
    if (anchor === null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setAnchor(null);
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [anchor]);
  return (
    <>
      <button
        type="button"
        title={title}
        aria-label={title}
        aria-haspopup="menu"
        disabled={entries.length === 0}
        // No toggle: `useMenuDismiss` already closed an open menu on the
        // mousedown that preceded this click, so a second press would flicker.
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setAnchor({ x: r.right, y: r.bottom + 4 });
        }}
        className="shrink-0 rounded p-1 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-300 disabled:cursor-default disabled:opacity-30 disabled:hover:bg-transparent"
      >
        <ChevronDown size={14} />
      </button>
      {anchor && (
        <MenuSurface x={anchor.x} y={anchor.y} align="right" className="w-72">
          {entries.map((term) => (
            <MenuItem
              key={term}
              icon={<History size={12} className="shrink-0 text-neutral-500" />}
              label={term}
              onClick={() => {
                setAnchor(null);
                onPick(term);
              }}
            />
          ))}
          <MenuSeparator />
          <MenuItem
            icon={<Trash2 size={12} className="shrink-0" />}
            label={t("history_clear")}
            danger
            onClick={() => {
              setAnchor(null);
              clearSearchHistory(kind);
              onCleared();
            }}
          />
        </MenuSurface>
      )}
    </>
  );
}

/** Extra chrome for the preview editor: it sizes to its content (up to half
 *  the dialog) instead of scrolling inside a fixed box, so the pane is small
 *  for a short file and the outer wrapper does the scrolling for a long one. */
const previewTheme = EditorView.theme({
  "&": { height: "auto", fontSize: "11px" },
  ".cm-scroller": { overflow: "visible", lineHeight: "1.5" },
  ".cm-content": { paddingBottom: "4px" },
  ".cm-oxyFindMatch": {
    backgroundColor: "rgba(251, 191, 36, 0.25)",
    color: "#fcd34d",
    borderRadius: "2px",
  },
});

const findMatchMark = Decoration.mark({ class: "cm-oxyFindMatch" });

/** Reconfigured (rather than rebuilt) when the query or a flag changes, so
 *  changing a toggle never discards the buffer or the undo history. */
const highlightCompartment = new Compartment();

/**
 * Paints every occurrence of the dialog's query inside the preview buffer.
 *
 * Hand-rolled rather than reusing `@codemirror/search`: its own highlighter
 * only draws while the search *panel* is open, and this pane has no panel —
 * the query lives in the dialog's input.
 */
function findMatchHighlighter(re: RegExp | null): Extension {
  if (!re) return [];
  const decorate = (view: EditorView): DecorationSet => {
    const builder = new RangeSetBuilder<Decoration>();
    // Ranges must reach the builder in order. Two visible ranges can start on
    // the same line (folding), which would hand it a backwards range.
    let painted = -1;
    for (const range of view.visibleRanges) {
      let pos = range.from;
      while (pos <= range.to) {
        const line = view.state.doc.lineAt(pos);
        if (line.from <= painted) {
          pos = line.to + 1;
          continue;
        }
        painted = line.from;
        re.lastIndex = 0;
        let m: RegExpExecArray | null;
        while ((m = re.exec(line.text)) !== null) {
          // A zero-width match would spin here forever and paints nothing.
          if (m[0].length === 0) {
            re.lastIndex += 1;
            continue;
          }
          builder.add(
            line.from + m.index,
            line.from + m.index + m[0].length,
            findMatchMark,
          );
        }
        pos = line.to + 1;
      }
    }
    return builder.finish();
  };
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = decorate(view);
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged) this.decorations = decorate(u.view);
      }
    },
    { decorations: (v) => v.decorations },
  );
}

/**
 * Preview of the selected match, as a real editor: syntax-colored, every
 * occurrence of the query highlighted, and editable in place — Ctrl+S writes
 * the file through the same environment-routed `fsWriteFile` the replace pass
 * uses, then asks the parent to refresh the (now stale) result list.
 *
 * Edits survive arrow-key browsing: buffers are kept per path in `draftsRef`
 * so stepping away from a file and back restores what was typed.
 */
function PreviewPane({
  projectId,
  worktreeId,
  hit,
  re,
  onDirtyChange,
  onSaved,
}: {
  projectId: string;
  worktreeId: string;
  hit: FlatHit | null;
  /** Same matcher the results list uses, so both highlight identically. */
  re: RegExp | null;
  onDirtyChange: (paths: string[]) => void;
  onSaved: (relPath: string, line: number) => void;
}) {
  const { t } = useTranslation("files");
  const [doc, setDoc] = useState<
    { relPath: string; text: string; truncated: boolean } | null
  >(null);
  const [readError, setReadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  /** relPath → edited buffer, kept while the dialog is open. */
  const draftsRef = useRef(new Map<string, string>());
  /** relPath → content as last read/written, to tell dirty from clean. */
  const baseRef = useRef(new Map<string, string>());
  /** Current matcher, read when a freshly built view is configured. */
  const reRef = useRef(re);
  reRef.current = re;
  /** Last `relPath:line` the caret was moved to, so a result refresh that
   *  produces a new `hit` object doesn't re-yank a caret already there. */
  const lastRevealRef = useRef("");
  /** Mirrors `dirty` for the update listener, which must not re-publish (and
   *  re-render the parent) on every keystroke — only when the flag flips. */
  const dirtyRef = useRef(false);

  const relPath = hit?.relPath ?? null;

  const publishDirty = useCallback(() => {
    const paths = [...draftsRef.current.keys()].filter(
      (p) => draftsRef.current.get(p) !== baseRef.current.get(p),
    );
    onDirtyChange(paths);
  }, [onDirtyChange]);

  const save = useCallback(async () => {
    const path = relPath;
    const view = viewRef.current;
    if (!path || !view || view.state.readOnly) return;
    const text = view.state.doc.toString();
    // Where the caret actually is, not where the (pre-edit) match was — the
    // parent re-points the selection at the nearest match to this line.
    const caretLine = view.state.doc.lineAt(view.state.selection.main.head).number;
    setSaving(true);
    setSaveError(null);
    try {
      await fsWriteFile({ projectId, worktreeId, relPath: path, content: text });
      baseRef.current.set(path, text);
      draftsRef.current.set(path, text);
      dirtyRef.current = false;
      setDirty(false);
      publishDirty();
      onSaved(path, caretLine);
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }, [projectId, worktreeId, relPath, publishDirty, onSaved]);
  const saveRef = useRef(save);
  saveRef.current = save;

  // Load the file when the selected file changes (not on every line). A draft
  // for the path wins over disk — it is what the user typed here.
  useEffect(() => {
    if (!relPath) {
      setDoc(null);
      setReadError(null);
      return;
    }
    const draft = draftsRef.current.get(relPath);
    if (draft !== undefined) {
      setReadError(null);
      setDoc({ relPath, text: draft, truncated: false });
      dirtyRef.current = draft !== baseRef.current.get(relPath);
      setDirty(dirtyRef.current);
      return;
    }
    let cancelled = false;
    void fsReadFile({ projectId, worktreeId, relPath })
      .then((r) => {
        if (cancelled) return;
        setReadError(null);
        baseRef.current.set(relPath, r.content);
        setDoc({ relPath, text: r.content, truncated: r.truncated });
        dirtyRef.current = false;
        setDirty(false);
      })
      // Surfaced rather than swallowed: a read that fails on a path the search
      // just matched means the (project, worktree) pair this dialog is scoped
      // to no longer resolves, and a blank pane hides that.
      .catch((e) => {
        if (cancelled) return;
        setDoc(null);
        setReadError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [relPath, projectId, worktreeId]);

  // Build the view once per file. Line changes within the same file are handled
  // by the effect below — rebuilding there would throw away in-flight edits.
  useEffect(() => {
    const host = containerRef.current;
    if (!host || !doc) return;
    // A truncated read holds only the head of the file; saving it back would
    // drop the rest, so the buffer stays read-only (same rule as the tabs).
    const readOnly = doc.truncated;
    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightActiveLine(),
      cmHistory(),
      drawSelection(),
      bracketMatching(),
      ...islandDark,
      previewTheme,
      languageForPath(doc.relPath) ?? [],
      highlightCompartment.of(findMatchHighlighter(reRef.current)),
      EditorState.readOnly.of(readOnly),
      EditorView.editable.of(!readOnly),
      EditorView.lineWrapping,
      keymap.of([
        {
          key: "Mod-s",
          preventDefault: true,
          run: () => {
            void saveRef.current();
            return true;
          },
        },
        ...historyKeymap,
        ...defaultKeymap,
      ]),
      EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        const text = u.state.doc.toString();
        draftsRef.current.set(doc.relPath, text);
        const nowDirty = text !== baseRef.current.get(doc.relPath);
        setDirty(nowDirty);
        // Publishing on every keystroke would re-render the parent (and its
        // whole result list) for each character typed.
        if (nowDirty !== dirtyRef.current) {
          dirtyRef.current = nowDirty;
          publishDirty();
        }
      }),
    ];
    const view = new EditorView({
      state: EditorState.create({ doc: doc.text, extensions }),
      parent: host,
    });
    viewRef.current = view;
    // A rebuilt view starts with the caret at 0, so the next reveal must run
    // even if it targets the same match this pane showed before.
    lastRevealRef.current = "";
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // `doc.text` is intentionally out of the deps: re-reading the same path
    // yields the same object, and an edit must not rebuild the view.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc?.relPath, doc?.truncated, publishDirty]);

  // Repaint the highlight when the query or a flag changes. Reconfiguring the
  // compartment leaves the buffer and the undo history alone.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: highlightCompartment.reconfigure(findMatchHighlighter(re)),
    });
  }, [re, doc?.relPath]);

  // Put the caret on the matched line and center it. Keyed on the target
  // itself, not on the `hit` object: every search refresh mints a new object
  // for the same match, and re-running this would yank the caret out from
  // under someone typing.
  const hitPath = hit?.relPath ?? null;
  const hitLine = hit?.line ?? 0;
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !hitPath || doc?.relPath !== hitPath) return;
    const target = `${hitPath}:${hitLine}`;
    // Never move the caret out from under someone editing. A save re-points
    // the selection at the nearest surviving match, which is rarely the line
    // being typed on; the editor holding focus means the reveal isn't wanted.
    if (view.hasFocus) {
      lastRevealRef.current = target;
      return;
    }
    if (lastRevealRef.current === target) return;
    lastRevealRef.current = target;
    const lineNo = Math.min(Math.max(hitLine, 1), view.state.doc.lines);
    const line = view.state.doc.line(lineNo);
    view.dispatch({
      selection: EditorSelection.cursor(line.from),
      effects: EditorView.scrollIntoView(line.from, { y: "center" }),
    });
  }, [hitPath, hitLine, doc?.relPath]);

  if (!hit || (!doc && !readError)) {
    return (
      <div className="flex h-32 shrink-0 items-center justify-center border-t border-neutral-800 px-4 text-center text-[12px] text-neutral-600">
        {t("find_in_files_preview_hint")}
      </div>
    );
  }

  if (readError) {
    return (
      <div
        role="alert"
        className="flex h-32 shrink-0 items-center justify-center border-t border-neutral-800 px-4 text-center text-[12px] text-red-400"
      >
        {readError}
      </div>
    );
  }

  return (
    <div className="flex max-h-[50%] shrink-0 flex-col border-t border-neutral-800">
      <div className="flex shrink-0 items-center gap-2 border-b border-neutral-800/60 bg-neutral-900/60 px-3 py-1 text-[11px]">
        <span className="truncate text-neutral-400">{doc?.relPath}</span>
        {dirty && <span className="shrink-0 text-amber-400">●</span>}
        {doc?.truncated && (
          <span className="shrink-0 text-neutral-500">{t("truncated_readonly")}</span>
        )}
        {saveError && (
          <span className="truncate text-red-400" role="alert">
            {saveError}
          </span>
        )}
        <button
          type="button"
          onClick={() => void save()}
          disabled={!dirty || saving || doc?.truncated}
          title={t("preview_save")}
          className="ml-auto flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100 disabled:cursor-default disabled:opacity-30 disabled:hover:bg-transparent"
        >
          {saving ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <Save size={12} />
          )}
          {t("preview_save")}
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto" ref={containerRef} />
    </div>
  );
}
