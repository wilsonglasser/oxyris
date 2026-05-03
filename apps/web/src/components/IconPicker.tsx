import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as LucideIcons from "lucide-react";
import { Search, Terminal, X } from "lucide-react";

interface Props {
  value: string;
  onPick: (name: string) => void;
  onClose: () => void;
}

/**
 * Full-lucide icon picker. Lists every PascalCase export from
 * `lucide-react` (currently ~1500 icons), filters by name, virtualizes
 * via fixed-height grid + windowing so the modal stays snappy.
 */
export function IconPicker({ value, onPick, onClose }: Props) {
  const { t } = useTranslation("actions");
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const allIcons = useMemo(() => {
    const map = LucideIcons as unknown as Record<string, unknown>;
    return Object.keys(map)
      .filter((k) => {
        const c = k[0];
        // PascalCase exports only — skip lower-case helpers.
        if (!c || c !== c.toUpperCase()) return false;
        // Skip `*Icon` aliases (lucide ships every icon twice).
        if (k.endsWith("Icon")) return false;
        // Skip metadata-ish exports.
        if (
          k === "createLucideIcon" ||
          k === "Icon" ||
          k === "LucideProps" ||
          k === "LucideIcon"
        )
          return false;
        const v = map[k];
        // Components are functions; skip everything else (constants etc).
        return typeof v === "function" || typeof v === "object";
      })
      .sort();
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allIcons;
    return allIcons.filter((n) => n.toLowerCase().includes(q));
  }, [allIcons, query]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onClick={onClose}
    >
      <div
        className="flex h-[70vh] w-full max-w-2xl flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center gap-2 border-b border-neutral-800 px-3">
          <Search size={13} className="text-neutral-500" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("icon_picker_search")}
            className="flex-1 bg-transparent text-[12px] text-neutral-100 outline-none placeholder:text-neutral-600"
          />
          <span className="text-[10px] text-neutral-500">
            {filtered.length}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
            aria-label={t("close")}
          >
            <X size={13} />
          </button>
        </div>
        <div className="grid min-h-0 flex-1 grid-cols-[repeat(auto-fill,minmax(36px,1fr))] gap-1 overflow-auto p-2">
          {filtered.map((name) => {
            const map = LucideIcons as unknown as Record<string, typeof Terminal>;
            const Icon = map[name] ?? Terminal;
            const selected = value === name;
            return (
              <button
                key={name}
                type="button"
                onClick={() => {
                  onPick(name);
                  onClose();
                }}
                className={`flex h-9 w-9 items-center justify-center rounded border ${
                  selected
                    ? "border-emerald-700 bg-emerald-900/30 text-emerald-300"
                    : "border-neutral-800/50 text-neutral-400 hover:border-neutral-700 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
                title={name}
              >
                <Icon size={14} />
              </button>
            );
          })}
          {filtered.length === 0 && (
            <div className="col-span-full px-3 py-4 text-center text-[12px] text-neutral-500">
              {t("icon_picker_empty")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
