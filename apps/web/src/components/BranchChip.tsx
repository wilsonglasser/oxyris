import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, GitBranch } from "lucide-react";
import { BranchMenu } from "~/components/BranchMenu.tsx";
import { RevDiffModal } from "~/components/RevDiffModal.tsx";
import { useGitStore } from "~/stores/gitStore.ts";

interface Props {
  projectId: string;
  worktreeId: string;
  /** Shown until the status projection resolves (avoids a label flash). */
  fallback?: string;
}

/**
 * Compact branch widget for panel headers: shows the current branch with its
 * ahead/behind counters and opens the same `BranchMenu` the Git panel uses, so
 * switching branches never requires leaving the conversation.
 */
export function BranchChip({ projectId, worktreeId, fallback }: Props) {
  const { t } = useTranslation("git");
  const status = useGitStore((s) => s.status[worktreeId]);
  const refreshStatus = useGitStore((s) => s.refreshStatus);
  const [menu, setMenu] = useState(false);
  const [revDiff, setRevDiff] = useState<{
    from: string;
    to: string;
    title: string;
  } | null>(null);

  useEffect(() => {
    void refreshStatus(projectId, worktreeId);
  }, [projectId, worktreeId, refreshStatus]);

  const ahead = status?.ahead_behind?.ahead ?? 0;
  const behind = status?.ahead_behind?.behind ?? 0;
  const label = status?.branch ?? fallback ?? t("no_branch");

  return (
    <div className="relative min-w-0">
      <button
        type="button"
        onClick={() => setMenu((v) => !v)}
        title={t("branch_menu_title")}
        className="flex min-w-0 max-w-[220px] items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-neutral-400 transition hover:bg-neutral-800 hover:text-neutral-100"
      >
        <GitBranch className="size-3 shrink-0" strokeWidth={1.75} />
        <span className="truncate">{label}</span>
        {(ahead > 0 || behind > 0) && (
          <span className="shrink-0 text-[9px] text-neutral-500">
            {ahead > 0 && `↑${ahead}`}
            {behind > 0 && `↓${behind}`}
          </span>
        )}
        <ChevronDown className="size-3 shrink-0 text-neutral-500" strokeWidth={1.75} />
      </button>
      {menu && (
        <BranchMenu
          projectId={projectId}
          worktreeId={worktreeId}
          onCompare={(from, to, title) => setRevDiff({ from, to, title })}
          onClose={() => setMenu(false)}
        />
      )}
      {revDiff && (
        <RevDiffModal
          projectId={projectId}
          worktreeId={worktreeId}
          from={revDiff.from}
          to={revDiff.to}
          title={revDiff.title}
          open
          onClose={() => setRevDiff(null)}
        />
      )}
    </div>
  );
}
