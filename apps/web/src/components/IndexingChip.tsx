import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2, TriangleAlert } from "lucide-react";
import { useIndexingStore } from "~/stores/indexingStore.ts";

const AUTO_HIDE_AFTER_DONE_MS = 4000;

interface Props {
  worktreeId: string | null;
}

/**
 * Compact chip showing the current state of the worktree's symbol-index
 * walk. Hidden when nothing is happening; auto-fades after a short window
 * once "done" arrives so the chrome stays clean.
 */
export function IndexingChip({ worktreeId }: Props) {
  const { t } = useTranslation("common");
  const state = useIndexingStore((s) =>
    worktreeId ? s.byWorktree[worktreeId] : null,
  );
  const clear = useIndexingStore((s) => s.clear);
  const [now, setNow] = useState(() => Date.now());

  // Force a re-render when the auto-hide window crosses, so the chip
  // disappears without needing another store update.
  useEffect(() => {
    if (!state) return;
    if (state.phase !== "done") return;
    const elapsed = Date.now() - state.updatedAt;
    const remaining = AUTO_HIDE_AFTER_DONE_MS - elapsed;
    if (remaining <= 0) {
      if (worktreeId) clear(worktreeId);
      return;
    }
    const id = window.setTimeout(() => setNow(Date.now()), remaining);
    return () => window.clearTimeout(id);
  }, [state, worktreeId, clear]);

  if (!state || !worktreeId) return null;

  if (state.phase === "done") {
    if (now - state.updatedAt > AUTO_HIDE_AFTER_DONE_MS) return null;
    return (
      <span className="inline-flex items-center gap-1 rounded border border-emerald-900/50 bg-emerald-950/20 px-1.5 py-0.5 text-[10px] text-emerald-300">
        <Check className="size-3" strokeWidth={2} />
        {t("indexing.done", {
          symbols: state.symbols ?? 0,
        })}
      </span>
    );
  }

  if (state.phase === "failed") {
    return (
      <span className="inline-flex max-w-[36rem] items-start gap-1 rounded border border-red-900/50 bg-red-950/20 px-1.5 py-0.5 text-[10px] text-red-300">
        <TriangleAlert className="mt-0.5 size-3 shrink-0" strokeWidth={1.75} />
        <span className="flex flex-col gap-0.5">
          <span>{t("indexing.failed")}</span>
          {state.error && (
            <span className="font-mono text-[10px] text-red-200/90 break-words whitespace-pre-wrap">
              {state.error}
            </span>
          )}
        </span>
      </span>
    );
  }

  // started + progress share the spinner UI; the bar fills as we walk.
  const total = Math.max(state.totalFiles, 1);
  const indexed = state.filesIndexed;
  const pct = Math.min(100, Math.round((indexed / total) * 100));
  return (
    <span className="inline-flex items-center gap-1.5 rounded border border-neutral-700 bg-neutral-900 px-1.5 py-0.5 text-[10px] text-neutral-400">
      <Loader2 className="size-3 animate-spin" strokeWidth={1.75} />
      {t("indexing.in_progress", {
        indexed,
        total: state.totalFiles,
      })}
      <span
        aria-hidden
        className="ml-0.5 inline-block h-1 w-12 overflow-hidden rounded bg-neutral-800"
      >
        <span
          className="block h-full bg-emerald-500/80 transition-all"
          style={{ width: `${pct}%` }}
        />
      </span>
    </span>
  );
}
