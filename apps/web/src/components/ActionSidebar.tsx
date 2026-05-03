import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import * as LucideIcons from "lucide-react";
import { Plus, Settings, Terminal } from "lucide-react";
import {
  actionList,
  actionRun,
  type ActionRow,
} from "~/ipc/actions.ts";
import { ActionEditModal } from "~/components/ActionEditModal.tsx";
import { ActionRunModal } from "~/components/ActionRunModal.tsx";

interface Props {
  projectId: string | null;
  worktreeId: string | null;
}

/**
 * Right-side action sidebar — shows all per-project user-defined actions
 * as icon buttons. Clicking runs the action; gear icon opens the action
 * manager (CRUD); plus icon opens the create dialog.
 *
 * Hidden when no project is selected.
 */
export function ActionSidebar({ projectId, worktreeId }: Props) {
  const { t } = useTranslation("actions");
  const [actions, setActions] = useState<ActionRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [edit, setEdit] = useState<{ row: ActionRow | null } | null>(null);
  const [running, setRunning] = useState<{
    runId: string;
    name: string;
  } | null>(null);

  const refresh = async () => {
    if (!projectId) return;
    setLoading(true);
    try {
      const list = await actionList({ project_id: projectId });
      setActions(list);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  if (!projectId) return null;

  const launch = async (a: ActionRow) => {
    try {
      const { run_id } = await actionRun({
        action_id: a.id,
        project_id: projectId,
        worktree_id: worktreeId,
      });
      setRunning({ runId: run_id, name: a.name });
    } catch (e) {
      window.alert(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <aside
        className="flex w-10 shrink-0 flex-col items-center gap-1 border-l border-neutral-800 bg-neutral-900/40 py-2"
        aria-label={t("sidebar")}
      >
        {actions.map((a) => {
          const Icon = lookupIcon(a.icon);
          return (
            <button
              key={a.id}
              type="button"
              onClick={() => void launch(a)}
              onContextMenu={(e) => {
                e.preventDefault();
                setEdit({ row: a });
              }}
              className="flex h-8 w-8 items-center justify-center rounded text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
              title={`${a.name} (${a.kind})`}
              aria-label={a.name}
            >
              <Icon size={14} />
            </button>
          );
        })}
        <button
          type="button"
          onClick={() => setEdit({ row: null })}
          className="mt-2 flex h-8 w-8 items-center justify-center rounded text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
          title={t("new_action")}
          aria-label={t("new_action")}
        >
          <Plus size={14} />
        </button>
        {actions.length > 0 && (
          <button
            type="button"
            onClick={() => setEdit({ row: actions[0] ?? null })}
            className="flex h-8 w-8 items-center justify-center rounded text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
            title={t("manage")}
            aria-label={t("manage")}
            disabled={loading}
          >
            <Settings size={12} />
          </button>
        )}
      </aside>

      {edit && (
        <ActionEditModal
          projectId={projectId}
          row={edit.row}
          onClose={() => {
            setEdit(null);
            void refresh();
          }}
        />
      )}

      {running && (
        <ActionRunModal
          runId={running.runId}
          name={running.name}
          onClose={() => setRunning(null)}
        />
      )}
    </>
  );
}

/**
 * Look up a lucide icon by string name. Falls back to `Terminal` when the
 * action's stored icon name doesn't exist (e.g. user typed it manually or
 * the icon was renamed in a lucide upgrade).
 */
function lookupIcon(name: string): typeof Terminal {
  const map = LucideIcons as unknown as Record<string, typeof Terminal>;
  return map[name] ?? Terminal;
}
