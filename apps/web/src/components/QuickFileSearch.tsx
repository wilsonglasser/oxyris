import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { File, Search } from "lucide-react";
import { fsSearchPaths, type FsSearchHit } from "~/ipc/fs.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";

interface Props {
  projectId: string;
  worktreeId: string;
  open: boolean;
  onClose: () => void;
}

/**
 * Ctrl+P-style quick file open. Debounced fuzzy search against the worktree
 * via `fs.search_paths`; arrow keys navigate, Enter opens the highlighted
 * file as a new tab. The modal is portal-free — it renders fixed-positioned
 * over everything via z-index.
 */
export function QuickFileSearch({ projectId, worktreeId, open, onClose }: Props) {
  const { t } = useTranslation("files");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<FsSearchHit[]>([]);
  const [cursor, setCursor] = useState(0);
  const [loading, setLoading] = useState(false);
  const openFile = useFileEditorStore((s) => s.openFile);

  // Reset on open + autofocus the input.
  useEffect(() => {
    if (open) {
      setQuery("");
      setHits([]);
      setCursor(0);
      // Defer to after the input mounts.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Debounced search.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    const t = window.setTimeout(() => {
      void fsSearchPaths({ projectId, worktreeId, query, limit: 50 })
        .then((res) => {
          if (cancelled) return;
          setHits(res.hits);
          setCursor(0);
        })
        .catch(() => {
          if (!cancelled) setHits([]);
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 80);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [query, projectId, worktreeId, open]);

  // Global Escape to close.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const visible = useMemo(() => hits.slice(0, 50), [hits]);

  if (!open) return null;

  const pick = async (hit: FsSearchHit) => {
    onClose();
    await openFile(projectId, worktreeId, hit.rel_path);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-24"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-2">
          <Search size={14} className="text-neutral-500" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setCursor((c) => Math.min(c + 1, visible.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setCursor((c) => Math.max(c - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                const hit = visible[cursor];
                if (hit) void pick(hit);
              }
            }}
            placeholder={t("quick_open_placeholder")}
            className="flex-1 bg-transparent text-[13px] text-neutral-100 outline-none placeholder:text-neutral-600"
          />
          {loading && (
            <span className="text-[10px] text-neutral-500">{t("loading")}</span>
          )}
        </div>
        <div className="max-h-80 overflow-auto">
          {visible.length === 0 && !loading && (
            <div className="px-3 py-4 text-center text-[12px] text-neutral-500">
              {query ? t("quick_open_no_results") : t("quick_open_hint")}
            </div>
          )}
          {visible.map((hit, i) => {
            const idx = hit.rel_path.lastIndexOf("/");
            const name = idx >= 0 ? hit.rel_path.slice(idx + 1) : hit.rel_path;
            const dir = idx >= 0 ? hit.rel_path.slice(0, idx) : "";
            return (
              <button
                key={hit.rel_path}
                type="button"
                onClick={() => void pick(hit)}
                onMouseEnter={() => setCursor(i)}
                className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] ${
                  i === cursor
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-300 hover:bg-neutral-900"
                }`}
              >
                <File size={12} className="shrink-0 text-neutral-500" />
                <span className="truncate">{name}</span>
                {dir && (
                  <span className="ml-auto truncate text-[10px] text-neutral-500">
                    {dir}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
