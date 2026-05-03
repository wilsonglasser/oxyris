import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Play, Plus, Settings2, Trash2, X } from "lucide-react";
import { type ActionRow } from "~/ipc/actions.ts";
import {
  useActionsStore,
  useProjectActions,
} from "~/stores/actionsStore.ts";
import { Modal } from "~/components/Modal.tsx";
import { terminalSpawn, terminalWrite } from "~/ipc/terminal.ts";
import { isTypingTarget, matchesKey } from "~/lib/keybindings.ts";

interface Props {
  projectId: string;
  /** Active session id; required to know which PTY to spawn against. */
  sessionId: string | null;
  /** Tells the chat shell to surface the terminal dock so the user sees output. */
  onShowTerminal?: (() => void) | undefined;
}

export function ProjectActionsBar({
  projectId,
  sessionId,
  onShowTerminal,
}: Props) {
  const { t } = useTranslation("chat");
  const list = useProjectActions(projectId);
  const refresh = useActionsStore((s) => s.refresh);
  const [open, setOpen] = useState(false);
  const [running, setRunning] = useState<string | null>(null);

  useEffect(() => {
    void refresh(projectId);
  }, [projectId, refresh]);

  const runAction = async (action: ActionRow) => {
    if (!sessionId) return;
    setRunning(action.id);
    try {
      onShowTerminal?.();
      const term = await terminalSpawn({
        session_id: sessionId,
        cols: 80,
        rows: 24,
      });
      await terminalWrite({ id: term.id, data: `${action.command}\r` });
    } catch {
      /* errors surface in the dock */
    } finally {
      setRunning(null);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target)) return;
      for (const action of list) {
        if (action.keybinding && matchesKey(e, action.keybinding)) {
          e.preventDefault();
          void runAction(action);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [list, sessionId]);

  return (
    <>
      <div className="flex items-center gap-1">
        {list.map((action) => (
          <button
            key={action.id}
            type="button"
            onClick={() => void runAction(action)}
            disabled={!sessionId || running === action.id}
            title={
              action.keybinding
                ? `${action.command} · ${action.keybinding}`
                : action.command
            }
            className="inline-flex items-center gap-1 rounded border border-neutral-800 px-2 py-0.5 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
          >
            <Play className="size-2.5" strokeWidth={2} />
            {action.name}
          </button>
        ))}
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-label={t("actions_manage")}
          title={t("actions_manage")}
          className="flex size-6 items-center justify-center rounded text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
        >
          <Settings2 className="size-3" strokeWidth={1.75} />
        </button>
      </div>

      <Modal
        open={open}
        onClose={() => setOpen(false)}
        closeLabel={t("actions_close")}
      >
        <ActionsManager projectId={projectId} />
      </Modal>
    </>
  );
}

type DraftAction = {
  id: string | null;
  name: string;
  command: string;
  keybinding: string;
  autoRun: boolean;
};

function ActionsManager({ projectId }: { projectId: string }) {
  const { t } = useTranslation("chat");
  const actions = useProjectActions(projectId);
  const upsert = useActionsStore((s) => s.upsert);
  const remove = useActionsStore((s) => s.remove);
  const [editing, setEditing] = useState<DraftAction | null>(null);
  const [error, setError] = useState<string | null>(null);

  return (
    <section className="rounded-xl border border-neutral-800 bg-neutral-900/60 p-5">
      <header className="mb-3 flex items-center justify-between">
        <h2 className="text-sm font-medium text-neutral-200">
          {t("actions_heading")}
        </h2>
        <button
          type="button"
          onClick={() =>
            setEditing({
              id: null,
              name: "",
              command: "",
              keybinding: "",
              autoRun: false,
            })
          }
          className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-xs text-neutral-200 hover:bg-neutral-800"
        >
          <Plus className="size-3" strokeWidth={1.75} />
          {t("actions_new")}
        </button>
      </header>

      {error && (
        <p className="mb-3 rounded border border-red-900/60 bg-red-950/30 px-3 py-2 text-[11px] text-red-200">
          {error}
        </p>
      )}

      {actions.length === 0 ? (
        <p className="text-[11px] text-neutral-500">{t("actions_empty")}</p>
      ) : (
        <ul className="mb-4 flex flex-col gap-2">
          {actions.map((a) => (
            <li
              key={a.id}
              className="flex items-center justify-between rounded border border-neutral-800 bg-neutral-950 px-3 py-2"
            >
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-neutral-100">
                  {a.name}
                </div>
                <div className="truncate font-mono text-[11px] text-neutral-500">
                  {a.command}
                </div>
                <div className="mt-0.5 text-[10px] text-neutral-600">
                  {a.keybinding ? a.keybinding : t("actions_no_keybinding")}
                  {a.auto_run_on_worktree_create
                    ? ` · ${t("actions_runs_on_worktree")}`
                    : ""}
                </div>
              </div>
              <div className="ml-3 flex items-center gap-1">
                <button
                  type="button"
                  onClick={() =>
                    setEditing({
                      id: a.id,
                      name: a.name,
                      command: a.command,
                      keybinding: a.keybinding ?? "",
                      autoRun: a.auto_run_on_worktree_create,
                    })
                  }
                  className="rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-200 hover:bg-neutral-800"
                >
                  {t("actions_edit")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setError(null);
                    void remove(projectId, a.id).catch((e) =>
                      setError(e instanceof Error ? e.message : String(e)),
                    );
                  }}
                  aria-label={t("actions_delete")}
                  title={t("actions_delete")}
                  className="flex size-7 items-center justify-center rounded border border-red-900/50 text-red-300 hover:bg-red-950/40"
                >
                  <Trash2 className="size-3" strokeWidth={1.75} />
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {editing && (
        <ActionEditor
          initial={editing}
          onCancel={() => setEditing(null)}
          onSave={async (draft) => {
            setError(null);
            try {
              await upsert({
                id: draft.id,
                project_id: projectId,
                name: draft.name,
                command: draft.command,
                keybinding: draft.keybinding || null,
                auto_run_on_worktree_create: draft.autoRun,
                icon: "Terminal",
                kind: "terminal_command",
              });
              setEditing(null);
            } catch (e) {
              setError(e instanceof Error ? e.message : String(e));
            }
          }}
        />
      )}
    </section>
  );
}

function ActionEditor({
  initial,
  onCancel,
  onSave,
}: {
  initial: DraftAction;
  onCancel: () => void;
  onSave: (a: DraftAction) => void;
}) {
  const { t } = useTranslation("chat");
  const [name, setName] = useState(initial.name);
  const [command, setCommand] = useState(initial.command);
  const [keybinding, setKeybinding] = useState(initial.keybinding);
  const [autoRun, setAutoRun] = useState(initial.autoRun);

  const valid = useMemo(
    () => name.trim().length > 0 && command.trim().length > 0,
    [name, command],
  );

  const onSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!valid) return;
    onSave({
      id: initial.id,
      name: name.trim(),
      command: command.trim(),
      keybinding: keybinding.trim(),
      autoRun,
    });
  };

  return (
    <form
      onSubmit={onSubmit}
      className="rounded-md border border-neutral-800 bg-neutral-950 p-3"
    >
      <div className="mb-2 flex items-center justify-between">
        <h3 className="text-xs uppercase tracking-wide text-neutral-500">
          {initial.id ? t("actions_edit_heading") : t("actions_new_heading")}
        </h3>
        <button
          type="button"
          onClick={onCancel}
          aria-label="cancel"
          className="text-neutral-500 hover:text-neutral-200"
        >
          <X className="size-3.5" strokeWidth={1.75} />
        </button>
      </div>

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <label className="flex flex-col gap-1 text-[11px] text-neutral-400">
          {t("actions_field_name")}
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("actions_field_name_placeholder")}
            className="rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-sm text-neutral-100 outline-none focus:border-neutral-500"
            required
          />
        </label>
        <label className="flex flex-col gap-1 text-[11px] text-neutral-400">
          {t("actions_field_keybinding")}
          <input
            value={keybinding}
            onChange={(e) => setKeybinding(e.target.value)}
            placeholder="Ctrl+Shift+B"
            className="rounded border border-neutral-700 bg-neutral-900 px-2 py-1 font-mono text-sm text-neutral-100 outline-none focus:border-neutral-500"
          />
        </label>
        <label className="col-span-full flex flex-col gap-1 text-[11px] text-neutral-400">
          {t("actions_field_command")}
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder="bun run dev"
            className="rounded border border-neutral-700 bg-neutral-900 px-2 py-1 font-mono text-sm text-neutral-100 outline-none focus:border-neutral-500"
            required
          />
        </label>
        <label className="col-span-full flex items-center gap-2 text-[11px] text-neutral-400">
          <input
            type="checkbox"
            checked={autoRun}
            onChange={(e) => setAutoRun(e.target.checked)}
            className="size-3"
          />
          {t("actions_field_auto_run")}
        </label>
      </div>

      <div className="mt-3 flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          className="rounded border border-neutral-700 px-3 py-1 text-xs text-neutral-300 hover:bg-neutral-800"
        >
          {t("actions_cancel")}
        </button>
        <button
          type="submit"
          disabled={!valid}
          className="rounded bg-neutral-200 px-3 py-1 text-xs font-medium text-neutral-900 hover:bg-white disabled:opacity-50"
        >
          {t("actions_save")}
        </button>
      </div>
    </form>
  );
}

