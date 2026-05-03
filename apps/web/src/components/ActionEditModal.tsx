import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Box,
  GitBranch,
  Github,
  Hammer,
  Play,
  Rocket,
  Save,
  Search,
  Send,
  Terminal,
  TestTube,
  Trash2,
  Wrench,
  Zap,
} from "lucide-react";
import {
  actionDelete,
  actionUpsert,
  type ActionKind,
  type ActionRow,
} from "~/ipc/actions.ts";

const ICON_CHOICES = [
  { name: "Terminal", icon: Terminal },
  { name: "Play", icon: Play },
  { name: "Hammer", icon: Hammer },
  { name: "Wrench", icon: Wrench },
  { name: "Rocket", icon: Rocket },
  { name: "TestTube", icon: TestTube },
  { name: "Box", icon: Box },
  { name: "Search", icon: Search },
  { name: "Send", icon: Send },
  { name: "Zap", icon: Zap },
  { name: "GitBranch", icon: GitBranch },
  { name: "Github", icon: Github },
] as const;

interface Props {
  projectId: string;
  row: ActionRow | null;
  onClose: () => void;
}

export function ActionEditModal({ projectId, row, onClose }: Props) {
  const { t } = useTranslation("actions");
  const [name, setName] = useState(row?.name ?? "");
  const [command, setCommand] = useState(row?.command ?? "");
  const [icon, setIcon] = useState(row?.icon ?? "Terminal");
  const [kind, setKind] = useState<ActionKind>(row?.kind ?? "terminal_command");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setError(null);
    if (!name.trim() || !command.trim()) {
      setError(t("validation_required"));
      return;
    }
    setSaving(true);
    try {
      await actionUpsert({
        id: row?.id ?? null,
        project_id: projectId,
        name: name.trim(),
        command,
        keybinding: null,
        auto_run_on_worktree_create: false,
        icon,
        kind,
      });
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!row) return;
    if (!window.confirm(t("delete_confirm", { name: row.name }))) return;
    setSaving(true);
    try {
      await actionDelete({ id: row.id });
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-lg border border-neutral-800 bg-neutral-950 p-4 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="mb-3 text-[13px] font-semibold text-neutral-100">
          {row ? t("edit_action") : t("new_action")}
        </h2>

        <label className="block text-[11px] text-neutral-400">
          {t("name")}
        </label>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
          className="mt-1 w-full rounded border border-neutral-800 bg-neutral-900 px-2 py-1 text-[12px] text-neutral-100 outline-none focus:ring-1 focus:ring-neutral-700"
        />

        <label className="mt-3 block text-[11px] text-neutral-400">
          {t("kind")}
        </label>
        <select
          value={kind}
          onChange={(e) => setKind(e.target.value as ActionKind)}
          className="mt-1 w-full rounded border border-neutral-800 bg-neutral-900 px-2 py-1 text-[12px] text-neutral-100 outline-none focus:ring-1 focus:ring-neutral-700"
        >
          <option value="terminal_command">{t("kind_terminal")}</option>
          <option value="one_shot">{t("kind_one_shot")}</option>
          <option value="github_workflow">{t("kind_github")}</option>
        </select>

        <label className="mt-3 block text-[11px] text-neutral-400">
          {kind === "github_workflow" ? t("command_gh") : t("command")}
        </label>
        <textarea
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          rows={3}
          placeholder={
            kind === "github_workflow"
              ? "deploy.yml --ref main"
              : kind === "terminal_command"
                ? "npm run dev"
                : "gulp build"
          }
          className="mt-1 w-full resize-none rounded border border-neutral-800 bg-neutral-900 px-2 py-1 font-mono text-[12px] text-neutral-100 outline-none focus:ring-1 focus:ring-neutral-700"
        />

        <label className="mt-3 block text-[11px] text-neutral-400">
          {t("icon")}
        </label>
        <div className="mt-1 grid grid-cols-6 gap-1">
          {ICON_CHOICES.map(({ name: iconName, icon: Icon }) => (
            <button
              key={iconName}
              type="button"
              onClick={() => setIcon(iconName)}
              className={`flex h-8 items-center justify-center rounded border ${
                icon === iconName
                  ? "border-emerald-700 bg-emerald-900/30 text-emerald-300"
                  : "border-neutral-800 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
              }`}
              title={iconName}
            >
              <Icon size={14} />
            </button>
          ))}
        </div>

        {error && (
          <div className="mt-3 text-[11px] text-red-400" role="alert">
            {error}
          </div>
        )}

        <div className="mt-4 flex items-center justify-between gap-2">
          {row && (
            <button
              type="button"
              onClick={() => void remove()}
              disabled={saving}
              className="flex items-center gap-1 rounded px-2 py-1 text-[11px] text-red-300 enabled:hover:bg-red-900/30 disabled:opacity-40"
            >
              <Trash2 size={11} />
              {t("delete")}
            </button>
          )}
          <div className="ml-auto flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded px-3 py-1 text-[11px] text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
            >
              {t("cancel")}
            </button>
            <button
              type="button"
              onClick={() => void save()}
              disabled={saving}
              className="flex items-center gap-1 rounded bg-emerald-700/80 px-3 py-1 text-[11px] text-neutral-100 enabled:hover:bg-emerald-700 disabled:opacity-40"
            >
              <Save size={11} />
              {saving ? t("saving") : t("save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
