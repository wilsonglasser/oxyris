import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { File, Search } from "lucide-react";
import { fsSearchContent, fsSearchPaths } from "~/ipc/fs.ts";
import {
  CLASS_LIKE_KINDS,
  indexQuerySymbol,
  type SymbolHit,
  type SymbolKind,
} from "~/ipc/indexing.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { buildHighlightRegex, highlightMatches } from "~/lib/searchHighlight.tsx";

export type SearchTab = "all" | "files" | "symbols" | "classes" | "text";

const TAB_ORDER: SearchTab[] = ["all", "files", "symbols", "classes", "text"];

interface Props {
  projectId: string;
  worktreeId: string;
  open: boolean;
  initialTab: SearchTab;
  onClose: () => void;
}

type Item =
  | { type: "file"; relPath: string }
  | { type: "symbol"; hit: SymbolHit }
  | { type: "text"; relPath: string; line: number; text: string };

/** Short, stable per-line key for cursor + react keys. */
function itemKey(it: Item): string {
  if (it.type === "file") return `f:${it.relPath}`;
  if (it.type === "symbol")
    return `s:${it.hit.file}:${it.hit.start_line}:${it.hit.name}`;
  return `t:${it.relPath}:${it.line}`;
}

/** Per-kind colored letter badge — robust against lucide icon-name drift. */
const KIND_COLOR: Record<SymbolKind, string> = {
  function: "bg-purple-500/20 text-purple-300",
  method: "bg-purple-500/20 text-purple-300",
  class: "bg-emerald-500/20 text-emerald-300",
  struct: "bg-emerald-500/20 text-emerald-300",
  enum: "bg-amber-500/20 text-amber-300",
  trait: "bg-sky-500/20 text-sky-300",
  interface: "bg-sky-500/20 text-sky-300",
  type: "bg-teal-500/20 text-teal-300",
  constant: "bg-rose-500/20 text-rose-300",
  module: "bg-neutral-500/20 text-neutral-300",
};

function KindBadge({ kind }: { kind: SymbolKind }) {
  return (
    <span
      className={`flex h-4 w-4 shrink-0 items-center justify-center rounded text-[9px] font-bold uppercase ${KIND_COLOR[kind]}`}
      title={kind}
    >
      {kind.charAt(0)}
    </span>
  );
}

/**
 * JetBrains "Search Everywhere" — one modal, several scopes. Ctrl+N opens on
 * Symbols, Ctrl+Shift+N on Files; Tab cycles scopes. Files come from the
 * fuzzy path index, Symbols/Classes from the tree-sitter symbol index, and
 * Text from the full-text searcher (capped — use Find in Files for the full
 * results + preview).
 */
export function SearchEverywhere({
  projectId,
  worktreeId,
  open,
  initialTab,
  onClose,
}: Props) {
  const { t } = useTranslation("files");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<SearchTab>(initialTab);
  const [items, setItems] = useState<Item[]>([]);
  const [cursor, setCursor] = useState(0);
  const [loading, setLoading] = useState(false);
  const openFile = useFileEditorStore((s) => s.openFile);
  const openFileAt = useFileEditorStore((s) => s.openFileAt);

  // Reset + focus when the modal opens; adopt the requested scope.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setItems([]);
    setCursor(0);
    setTab(initialTab);
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [open, initialTab]);

  // Debounced fetch driven by (query, scope).
  useEffect(() => {
    if (!open) return;
    const q = query.trim();
    let cancelled = false;
    if (!q) {
      setItems([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    const handle = window.setTimeout(() => {
      const perCat = tab === "all" ? 6 : 60;
      const wantFiles = tab === "all" || tab === "files";
      const wantSymbols = tab === "all" || tab === "symbols";
      const wantClasses = tab === "classes";
      // In the merged "all" scope, full-text search runs a grep over the
      // worktree on each keystroke — gate it to ≥3 chars so single letters
      // don't trigger a heavy walk. The dedicated Text scope has no gate.
      const wantText =
        tab === "text" || (tab === "all" && q.length >= 3);

      const filesP = wantFiles
        ? fsSearchPaths({ projectId, worktreeId, query: q, limit: perCat }).then(
            (r) => r.hits.map((h) => h.rel_path),
          )
        : Promise.resolve<string[]>([]);
      const symbolsP =
        wantSymbols || wantClasses
          ? indexQuerySymbol({ worktreeId, projectId, name: q, limit: perCat * 2 })
          : Promise.resolve<SymbolHit[]>([]);
      const textP = wantText
        ? fsSearchContent({
            projectId,
            worktreeId,
            query: q,
            maxResults: tab === "all" ? 30 : 200,
          })
        : Promise.resolve(null);

      void Promise.all([filesP, symbolsP, textP])
        .then(([files, symbols, text]) => {
          if (cancelled) return;
          const merged: Item[] = [];
          for (const relPath of files.slice(0, perCat))
            merged.push({ type: "file", relPath });
          const syms = wantClasses
            ? symbols.filter((s) => CLASS_LIKE_KINDS.includes(s.kind))
            : symbols;
          for (const hit of syms.slice(0, perCat))
            merged.push({ type: "symbol", hit });
          if (text) {
            let n = 0;
            outer: for (const f of text.files) {
              for (const m of f.matches) {
                merged.push({
                  type: "text",
                  relPath: f.rel_path,
                  line: m.line,
                  text: m.text,
                });
                if (++n >= perCat) break outer;
              }
            }
          }
          setItems(merged);
          setCursor(0);
          setLoading(false);
        })
        .catch(() => {
          if (cancelled) return;
          setItems([]);
          setLoading(false);
        });
    }, 120);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [query, tab, open, projectId, worktreeId]);

  const re = useMemo(() => buildHighlightRegex(query.trim()), [query]);

  const pick = async (it: Item) => {
    onClose();
    if (it.type === "file") {
      await openFile(projectId, worktreeId, it.relPath);
    } else if (it.type === "symbol") {
      await openFileAt(projectId, worktreeId, it.hit.file, it.hit.start_line);
    } else {
      await openFileAt(projectId, worktreeId, it.relPath, it.line);
    }
  };

  if (!open) return null;

  const cycleTab = (dir: 1 | -1) => {
    const idx = TAB_ORDER.indexOf(tab);
    setTab(TAB_ORDER[(idx + dir + TAB_ORDER.length) % TAB_ORDER.length]!);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-20"
      onClick={onClose}
    >
      <div
        className="flex max-h-[70vh] w-full max-w-2xl flex-col overflow-hidden rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-2.5">
          <Search size={15} className="text-neutral-500" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setCursor((c) => Math.min(c + 1, items.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setCursor((c) => Math.max(c - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                const it = items[cursor];
                if (it) void pick(it);
              } else if (e.key === "Tab") {
                e.preventDefault();
                cycleTab(e.shiftKey ? -1 : 1);
              } else if (e.key === "Escape") {
                e.preventDefault();
                onClose();
              }
            }}
            placeholder={t("search_everywhere_placeholder")}
            className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
          />
          {loading && (
            <span className="text-[10px] text-neutral-500">{t("loading")}</span>
          )}
        </div>

        <div className="flex items-center gap-1 border-b border-neutral-800 px-2 py-1">
          {TAB_ORDER.map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={`rounded px-2 py-0.5 text-[11px] transition ${
                tab === id
                  ? "bg-neutral-800 text-neutral-100"
                  : "text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
              }`}
            >
              {t(`search_tabs.${id}`)}
            </button>
          ))}
        </div>

        <div className="min-h-0 flex-1 overflow-auto py-1">
          {items.length === 0 && !loading && (
            <div className="px-3 py-6 text-center text-[12px] text-neutral-500">
              {query.trim()
                ? t("search_no_results")
                : t("search_everywhere_hint")}
            </div>
          )}
          {items.map((it, i) => {
            const k = itemKey(it);
            const activeRow = i === cursor;
            return (
              <button
                key={k}
                type="button"
                onClick={() => void pick(it)}
                onMouseEnter={() => setCursor(i)}
                className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] ${
                  activeRow
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-300 hover:bg-neutral-900"
                }`}
              >
                {it.type === "file" && (
                  <Row
                    icon={<File size={12} className="shrink-0 text-neutral-500" />}
                    primary={highlightMatches(baseName(it.relPath), re)}
                    secondary={dirName(it.relPath)}
                  />
                )}
                {it.type === "symbol" && (
                  <Row
                    icon={<KindBadge kind={it.hit.kind} />}
                    primary={highlightMatches(it.hit.name, re)}
                    secondary={`${it.hit.file}:${it.hit.start_line}`}
                  />
                )}
                {it.type === "text" && (
                  <Row
                    icon={
                      <span className="shrink-0 text-[10px] text-neutral-600">
                        {it.line}
                      </span>
                    }
                    primary={highlightMatches(it.text.trim(), re)}
                    primaryMono
                    secondary={it.relPath}
                  />
                )}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function Row({
  icon,
  primary,
  secondary,
  primaryMono,
}: {
  icon: React.ReactNode;
  primary: React.ReactNode;
  secondary: string;
  primaryMono?: boolean;
}) {
  return (
    <>
      {icon}
      <span
        className={`truncate ${primaryMono ? "font-mono text-[11px]" : ""}`}
      >
        {primary}
      </span>
      <span className="ml-auto max-w-[45%] truncate pl-2 text-[10px] text-neutral-500">
        {secondary}
      </span>
    </>
  );
}

function baseName(p: string): string {
  const idx = p.lastIndexOf("/");
  return idx >= 0 ? p.slice(idx + 1) : p;
}

function dirName(p: string): string {
  const idx = p.lastIndexOf("/");
  return idx >= 0 ? p.slice(0, idx) : "";
}
