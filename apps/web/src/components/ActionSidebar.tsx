import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import * as LucideIcons from "lucide-react";
import { Pencil, Play, Plus, Terminal, Trash2 } from "lucide-react";
import { actionDelete, actionList, type ActionRow } from "~/ipc/actions.ts";
import { ActionEditModal } from "~/components/ActionEditModal.tsx";
import { ActionRunsModal } from "~/components/ActionRunsModal.tsx";
import { useActionRunsStore } from "~/stores/actionRunsStore.ts";

interface Props {
  projectId: string | null;
  worktreeId: string | null;
}

/**
 * Right-side action rail. Each action is one icon button:
 *
 * - Click → if no active runs, start one. If runs exist, toggle their
 *   tabbed modal open / closed (minimize behavior).
 * - Right-click → context menu (Editar / Remover / Nova execução).
 * - Counter badge on the icon shows how many instances are alive.
 *
 * Hidden when no project is selected.
 */
export function ActionSidebar({ projectId, worktreeId }: Props) {
  const { t } = useTranslation("actions");
  const [actions, setActions] = useState<ActionRow[]>([]);
  const [edit, setEdit] = useState<{ row: ActionRow | null } | null>(null);
  const [menu, setMenu] = useState<
    { x: number; y: number; row: ActionRow } | null
  >(null);

  const startRun = useActionRunsStore((s) => s.start);
  const toggleOpen = useActionRunsStore((s) => s.toggleOpen);
  const setOpen = useActionRunsStore((s) => s.setOpen);
  const runs = useActionRunsStore((s) => s.runs);
  const openActionIds = useActionRunsStore((s) => s.openActionIds);

  const refresh = async () => {
    if (!projectId) return;
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

  // Dismiss context menu on outside click / Esc.
  useEffect(() => {
    if (!menu) return;
    const onDown = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  if (!projectId) return null;

  const onIconClick = async (a: ActionRow) => {
    const count = runs[a.id]?.length ?? 0;
    if (count === 0) {
      try {
        await startRun(a.id, a.name, projectId, worktreeId);
      } catch (e) {
        window.alert(e instanceof Error ? e.message : String(e));
      }
    } else {
      toggleOpen(a.id);
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
          const count = runs[a.id]?.length ?? 0;
          const open = openActionIds[a.id] ?? false;
          return (
            <button
              key={a.id}
              type="button"
              onClick={() => void onIconClick(a)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, row: a });
              }}
              className={`relative flex h-8 w-8 items-center justify-center rounded ${
                open
                  ? "bg-neutral-800 text-emerald-300"
                  : "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
              }`}
              title={a.name}
              aria-label={a.name}
            >
              <Icon size={14} />
              {count > 0 && (
                <span className="absolute -right-1 -top-1 flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-emerald-700 px-0.5 text-[9px] font-semibold text-neutral-50">
                  {count}
                </span>
              )}
            </button>
          );
        })}
        <button
          type="button"
          onClick={() => setEdit({ row: null })}
          className="flex h-8 w-8 items-center justify-center rounded text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
          title={t("new_action")}
          aria-label={t("new_action")}
        >
          <Plus size={14} />
        </button>
      </aside>

      {menu && (
        <div
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-[170px] rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-lg"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            onClick={async () => {
              const row = menu.row;
              setMenu(null);
              try {
                await startRun(row.id, row.name, projectId, worktreeId);
              } catch (e) {
                window.alert(e instanceof Error ? e.message : String(e));
              }
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            <Play size={11} />
            {t("ctx_new_run")}
          </button>
          <div className="my-1 border-t border-neutral-800" />
          <button
            type="button"
            onClick={() => {
              const row = menu.row;
              setMenu(null);
              setEdit({ row });
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            <Pencil size={11} />
            {t("ctx_edit")}
          </button>
          <button
            type="button"
            onClick={async () => {
              const row = menu.row;
              setMenu(null);
              if (!window.confirm(t("delete_confirm", { name: row.name }))) return;
              try {
                await actionDelete({ id: row.id });
                await refresh();
              } catch (e) {
                window.alert(e instanceof Error ? e.message : String(e));
              }
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-red-300 hover:bg-red-900/30"
          >
            <Trash2 size={11} />
            {t("ctx_delete")}
          </button>
        </div>
      )}

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

      {actions.map((a) =>
        openActionIds[a.id] ? (
          <ActionRunsModal
            key={a.id}
            actionId={a.id}
            actionName={a.name}
            onMinimize={() => setOpen(a.id, false)}
          />
        ) : null,
      )}
    </>
  );
}

function lookupIcon(name: string): typeof Terminal {
  const map = LucideIcons as unknown as Record<string, typeof Terminal>;
  return map[name] ?? Terminal;
}
