import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CaseSensitive, Regex, Search, WholeWord, X } from "lucide-react";
import {
  fsReadFile,
  fsSearchContent,
  type FsSearchContentResult,
} from "~/ipc/fs.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { buildHighlightRegex, highlightMatches } from "~/lib/searchHighlight.tsx";

interface Props {
  projectId: string;
  worktreeId: string;
  open: boolean;
  onClose: () => void;
}

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
export function FindInFiles({ projectId, worktreeId, open, onClose }: Props) {
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
