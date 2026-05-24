import { useEffect, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  type Environment,
  type PathValidation,
  ProjectCommandError,
  projectCreate,
  projectValidatePath,
} from "~/ipc/commands.ts";
import { useProjectStore, workspacesOf } from "~/stores/projectStore.ts";

type EnvKind = Environment["kind"];

interface ProjectPanelProps {
  /** Optional callback fired after a successful project creation. The shell
   * uses it to dismiss the create-project modal. */
  onCreated?: () => void;
}

/** Match `\\wsl.localhost\<distro>\<rest>` or `\\wsl$\<distro>\<rest>` (and
 * forward-slash variants). Returns `[distro, posixPath]` on hit. */
function parseWslUnc(picked: string): [string, string] | null {
  const re = /^[\\/][\\/](?:wsl\.localhost|wsl\$)[\\/]([^\\/]+)(.*)$/;
  const m = picked.match(re);
  if (!m || !m[1]) return null;
  const distro = m[1];
  const rest = (m[2] ?? "").replace(/\\/g, "/");
  const posix = rest.length === 0 ? "/" : rest;
  return [distro, posix];
}

export function ProjectPanel({ onCreated }: ProjectPanelProps = {}) {
  const { t } = useTranslation("project");
  const refresh = useProjectStore((s) => s.refresh);
  const setActive = useProjectStore((s) => s.setActive);
  const projects = useProjectStore((s) => s.projects);
  const knownWorkspaces = workspacesOf(projects);
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [envKind, setEnvKind] = useState<EnvKind>("windows");
  const [distro, setDistro] = useState("Ubuntu");
  const [rootPath, setRootPath] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<PathValidation | null>(null);

  // Debounced path validation — avoid hammering the agent on every keystroke.
  const validateTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const trimmed = rootPath.trim();
    if (!trimmed) {
      setValidation(null);
      return;
    }
    window.clearTimeout(validateTimer.current);
    validateTimer.current = window.setTimeout(() => {
      const environment: Environment =
        envKind === "windows" ? { kind: "windows" } : { kind: "wsl", distro };
      void projectValidatePath({ environment, path: trimmed })
        .then(setValidation)
        .catch(() => setValidation(null));
    }, 250);
    return () => window.clearTimeout(validateTimer.current);
  }, [rootPath, envKind, distro]);

  const onBrowse = async () => {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    const wsl = parseWslUnc(picked);
    if (wsl) {
      const [pickedDistro, posix] = wsl;
      setEnvKind("wsl");
      setDistro(pickedDistro);
      setRootPath(posix);
      if (!name.trim()) {
        const leaf = posix.split("/").filter(Boolean).pop();
        if (leaf) setName(leaf);
      }
    } else {
      setEnvKind("windows");
      setRootPath(picked);
      if (!name.trim()) {
        const leaf = picked.split(/[\\/]/).filter(Boolean).pop();
        if (leaf) setName(leaf);
      }
    }
  };

  const onCreate = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const environment: Environment =
        envKind === "windows" ? { kind: "windows" } : { kind: "wsl", distro };
      const created = await projectCreate({
        name,
        environment,
        root_path: rootPath,
        workspace: workspace.trim() || null,
      });
      setName("");
      setRootPath("");
      await refresh();
      setActive(created.id);
      onCreated?.();
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section className="rounded-xl border border-neutral-800 bg-neutral-900/50 p-5">
      <h2 className="mb-3 text-sm font-medium text-neutral-300">
        {t("create.heading")}
      </h2>

      {error !== null && (
        <p className="mb-3 rounded-md border border-red-900/50 bg-red-950/40 px-3 py-2 text-xs text-red-200">
          {error}
        </p>
      )}
      <form onSubmit={onCreate} className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <label className="flex flex-col gap-1 text-xs text-neutral-400">
          {t("create.name")}
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("create.name_placeholder")}
            required
            className="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-neutral-400">
          {t("create.environment")}
          <select
            value={envKind}
            onChange={(e) => setEnvKind(e.target.value as EnvKind)}
            className="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100"
          >
            <option value="windows">{t("create.environment_windows")}</option>
            <option value="wsl">
              {t("create.environment_wsl", { distro: distro || "Ubuntu" })}
            </option>
          </select>
        </label>

        {envKind === "wsl" && (
          <label className="flex flex-col gap-1 text-xs text-neutral-400">
            {t("create.wsl_distro")}
            <input
              value={distro}
              onChange={(e) => setDistro(e.target.value)}
              placeholder={t("create.wsl_distro_placeholder")}
              required
              className="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100"
            />
          </label>
        )}

        <label className="flex flex-col gap-1 text-xs text-neutral-400 sm:col-span-2">
          {t("create.workspace")}
          <input
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
            list="oxyris-workspaces"
            placeholder={t("create.workspace_placeholder")}
            className="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100"
          />
          <datalist id="oxyris-workspaces">
            {knownWorkspaces.map((ws) => (
              <option key={ws} value={ws} />
            ))}
          </datalist>
        </label>

        <label className="flex flex-col gap-1 text-xs text-neutral-400 sm:col-span-2">
          {t("create.root_path")}
          <div className="flex gap-2">
            <input
              value={rootPath}
              onChange={(e) => setRootPath(e.target.value)}
              placeholder={
                envKind === "windows"
                  ? t("create.root_path_placeholder_windows")
                  : t("create.root_path_placeholder_wsl")
              }
              required
              className="flex-1 rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 font-mono text-sm text-neutral-100"
            />
            <button
              type="button"
              onClick={() => void onBrowse()}
              className="rounded-md border border-neutral-700 bg-neutral-900 px-3 py-2 text-xs text-neutral-200 hover:bg-neutral-800"
            >
              {t("create.browse")}
            </button>
          </div>
          {validation && (
            <ValidationBadge validation={validation} t={t} />
          )}
        </label>

        <div className="sm:col-span-2">
          <button
            type="submit"
            disabled={submitting}
            className="rounded-md bg-neutral-200 px-4 py-2 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:opacity-60"
          >
            {submitting ? t("create.submitting") : t("create.submit")}
          </button>
        </div>
      </form>
    </section>
  );
}

function ValidationBadge({
  validation,
  t,
}: {
  validation: PathValidation;
  t: TFunction<"project">;
}) {
  if (validation.warning) {
    return (
      <span className="mt-1 text-[11px] text-amber-300">
        ⚠ {t("create.validation_warning", { message: validation.warning })}
      </span>
    );
  }
  if (!validation.exists) {
    return (
      <span className="mt-1 text-[11px] text-red-300">
        ✗ {t("create.validation_missing")}
      </span>
    );
  }
  if (!validation.is_dir) {
    return (
      <span className="mt-1 text-[11px] text-red-300">
        ✗ {t("create.validation_not_dir")}
      </span>
    );
  }
  if (!validation.is_git_repo) {
    return (
      <span className="mt-1 text-[11px] text-amber-300">
        ⚠ {t("create.validation_ok_no_git")}
      </span>
    );
  }
  return (
    <span className="mt-1 text-[11px] text-emerald-300">
      ✓ {t("create.validation_ok")}
    </span>
  );
}

function formatError(err: unknown, t: TFunction<"project">): string {
  if (err instanceof ProjectCommandError) {
    switch (err.tauri.code) {
      case "domain":
        return t("error.domain", { message: err.tauri.message });
      case "concurrency":
        return t("error.concurrency");
      case "storage":
        return t("error.storage", { message: err.tauri.message });
      case "projection":
        return t("error.projection", { message: err.tauri.message });
    }
  }
  return t("error.unknown", {
    message: err instanceof Error ? err.message : String(err),
  });
}
