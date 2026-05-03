import { useTranslation } from "react-i18next";
import { Check, Loader2, TriangleAlert, ZapOff } from "lucide-react";
import {
  type LanguageState,
  useLspStatusStore,
} from "~/stores/lspStatusStore.ts";

interface Props {
  worktreeId: string | null;
}

/**
 * Compact chip showing the LSP warm-up state for the active worktree. We
 * pick the "loudest" language to display — anything spawning/failed wins
 * over ready, so the chip reflects what the user most needs to know.
 * Hidden when nothing is happening.
 */
export function LspChip({ worktreeId }: Props) {
  const { t } = useTranslation("common");
  const langs = useLspStatusStore((s) =>
    worktreeId ? s.byWorktree[worktreeId] : null,
  );
  if (!worktreeId || !langs) return null;
  const states = Object.values(langs);
  if (states.length === 0) return null;

  // Priority: failed/not_installed > spawning > ready.
  const priority = (s: LanguageState): number => {
    switch (s.phase) {
      case "failed":
      case "not_installed":
        return 3;
      case "spawning":
        return 2;
      case "ready":
        return 1;
    }
  };
  const top = states.reduce((a, b) => (priority(b) > priority(a) ? b : a));

  if (top.phase === "ready") {
    return (
      <span
        title={top.language}
        className="inline-flex items-center gap-1 rounded border border-emerald-900/50 bg-emerald-950/20 px-1.5 py-0.5 text-[10px] text-emerald-300"
      >
        <Check className="size-3" strokeWidth={2} />
        {t("lsp.ready", { language: prettyLang(top.language) })}
      </span>
    );
  }
  if (top.phase === "spawning") {
    return (
      <span
        title={top.language}
        className="inline-flex items-center gap-1 rounded border border-neutral-700 bg-neutral-900 px-1.5 py-0.5 text-[10px] text-neutral-400"
      >
        <Loader2 className="size-3 animate-spin" strokeWidth={1.75} />
        {t("lsp.warming", { language: prettyLang(top.language) })}
      </span>
    );
  }
  if (top.phase === "not_installed") {
    return (
      <span
        title={top.message ?? ""}
        className="inline-flex items-center gap-1 rounded border border-amber-900/50 bg-amber-950/20 px-1.5 py-0.5 text-[10px] text-amber-300"
      >
        <ZapOff className="size-3" strokeWidth={1.75} />
        {t("lsp.not_installed", { language: prettyLang(top.language) })}
      </span>
    );
  }
  return (
    <span
      title={top.message ?? ""}
      className="inline-flex items-center gap-1 rounded border border-red-900/50 bg-red-950/20 px-1.5 py-0.5 text-[10px] text-red-300"
    >
      <TriangleAlert className="size-3" strokeWidth={1.75} />
      {t("lsp.failed", { language: prettyLang(top.language) })}
    </span>
  );
}

function prettyLang(id: string): string {
  switch (id) {
    case "rust":
      return "rust-analyzer";
    case "typescript-javascript":
      return "tsserver";
    case "php":
      return "PHP";
    default:
      return id;
  }
}
