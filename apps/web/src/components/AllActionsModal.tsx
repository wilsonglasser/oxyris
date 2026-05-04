import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import * as LucideIcons from "lucide-react";
import { EyeOff, Pencil, Plus, Terminal, Trash2, X, Zap } from "lucide-react";
import { actionDelete, actionList, type ActionRow } from "~/ipc/actions.ts";
import { ActionEditModal } from "~/components/ActionEditModal.tsx";

interface Props {
  projectId: string;
  onClose: () => void;
}

/**
 * "All actions" manager — lists every action for the project (visible +
 * hidden) as cards so the user can edit / delete / create even when an
 * action's icon isn't on the sidebar.
 */
export function AllActionsModal({ projectId, onClose }: Props) {
  const { t } = useTranslation("actions");
  const [actions, setActions] = useState<ActionRow[]>([]);
  const [edit, setEdit] = useState<{ row: ActionRow | null } | null>(null);

  const refresh = async () => {
    try {
      const list = await actionList({ project_id: projectId });
      setActions(list);
    } catch (e) {
      console.warn("actionList failed", e);
    }
  };

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="flex h-[75vh] w-[80vw] max-w-3xl flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3 text-[12px] text-neutral-200">
          <span className="font-semibold">{t("all_actions")}</span>
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={() => setEdit({ row: null })}
              className="flex items-center gap-1 rounded bg-emerald-700/80 px-2 py-0.5 text-[11px] text-neutral-100 hover:bg-emerald-700"
            >
              <Plus size={11} />
              {t("new_action")}
            </button>
            <button
              type="button"
              onClick={onClose}
              className="rounded p-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
              aria-label={t("close")}
            >
              <X size={13} />
            </button>
          </div>
        </div>
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-2 overflow-auto p-3 sm:grid-cols-2">
          {actions.length === 0 && (
            <div className="col-span-full px-3 py-4 text-center text-[12px] text-neutral-500">
              {t("no_actions_yet")}
            </div>
          )}
          {actions.map((a) => {
            const Icon = lookupIcon(a.icon);
            return (
              <div
                key={a.id}
                className="group flex flex-col rounded border border-neutral-800 bg-neutral-900/50 p-2"
              >
                <div className="flex items-start gap-2">
                  <Icon size={16} className="mt-0.5 shrink-0 text-emerald-300" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[12px] font-medium text-neutral-100">
                      {a.name}
                    </div>
                    <div className="mt-0.5 flex items-center gap-1.5 text-[10px] text-neutral-500">
                      <span className="rounded bg-neutral-800 px-1 py-0.5">
                        {a.kind}
                      </span>
                      {a.keybinding && (
                        <span className="flex items-center gap-0.5">
                          <Zap size={9} />
                          {a.keybinding}
                        </span>
                      )}
                      {!a.show_in_sidebar && (
                        <span className="flex items-center gap-0.5">
                          <EyeOff size={9} />
                          {t("hidden")}
                        </span>
                      )}
                      {a.auto_run_on_worktree_create && (
                        <span className="text-amber-400">{t("auto_run_short")}</span>
                      )}
                    </div>
                    <code className="mt-1 line-clamp-2 block text-[10.5px] text-neutral-400">
                      {a.command}
                    </code>
                  </div>
                  <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      type="button"
                      onClick={() => setEdit({ row: a })}
                      className="rounded p-1 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
                      aria-label={t("ctx_edit")}
                      title={t("ctx_edit")}
                    >
                      <Pencil size={11} />
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        if (
                          !window.confirm(t("delete_confirm", { name: a.name }))
                        )
                          return;
                        try {
                          await actionDelete({ id: a.id });
                          await refresh();
                        } catch (e) {
                          window.alert(e instanceof Error ? e.message : String(e));
                        }
                      }}
                      className="rounded p-1 text-red-300 hover:bg-red-900/30"
                      aria-label={t("ctx_delete")}
                      title={t("ctx_delete")}
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

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
    </div>
  );
}

function lookupIcon(name: string): typeof Terminal {
  const map = LucideIcons as unknown as Record<string, typeof Terminal>;
  return map[name] ?? Terminal;
}
