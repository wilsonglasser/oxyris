import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";

type DiscoveredInstall = {
  provider_id: string;
  environment: { kind: string; distro?: string };
  path: string | null;
  version: string | null;
  error: string | null;
  is_interop_shim: boolean;
};

interface Props {
  onNewProject: () => void;
}

export function WelcomeScreen({ onNewProject }: Props) {
  const { t } = useTranslation("common");
  const [installs, setInstalls] = useState<DiscoveredInstall[] | null>(null);

  useEffect(() => {
    void invoke<DiscoveredInstall[]>("settings_provider_discover")
      .then(setInstalls)
      .catch(() => setInstalls([]));
  }, []);

  const hasAnyClaude = installs?.some(
    (i) => i.provider_id === "claude" && !i.error,
  );

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-6 py-12">
      <div>
        <h1 className="text-3xl font-semibold tracking-tight">
          {t("welcome.title")}
        </h1>
        <p className="mt-2 text-sm text-neutral-400">
          {t("welcome.tagline")}
        </p>
      </div>

      <div className="rounded-xl border border-neutral-800 bg-neutral-900/50 p-5">
        <h2 className="mb-3 text-sm font-medium text-neutral-300">
          {t("welcome.checks_heading")}
        </h2>
        <ul className="flex flex-col gap-2 text-[12px]">
          <CheckRow
            label={t("welcome.check_claude")}
            state={
              installs === null
                ? "loading"
                : hasAnyClaude
                  ? "ok"
                  : "missing"
            }
            detail={
              installs === null
                ? undefined
                : hasAnyClaude
                  ? installs
                      .filter((i) => !i.error)
                      .map((i) => envLabel(i.environment))
                      .join(", ")
                  : t("welcome.check_claude_missing_detail")
            }
          />
        </ul>
      </div>

      <div className="rounded-xl border border-neutral-800 bg-neutral-900/50 p-5">
        <h2 className="mb-2 text-sm font-medium text-neutral-300">
          {t("welcome.next_heading")}
        </h2>
        <p className="text-[12px] text-neutral-400">
          {t("welcome.next_body")}
        </p>
        <button
          type="button"
          onClick={onNewProject}
          className="mt-4 rounded-md bg-neutral-100 px-4 py-2 text-sm font-medium text-neutral-900 hover:bg-white"
        >
          {t("welcome.cta_new_project")}
        </button>
      </div>

      <p className="text-[11px] text-neutral-600">
        {t("welcome.shortcut_hint")}
      </p>
    </div>
  );
}

function envLabel(env: { kind: string; distro?: string }): string {
  if (env.kind === "windows") return "Windows";
  if (env.kind === "wsl") return `WSL · ${env.distro ?? ""}`;
  return env.kind;
}

function CheckRow({
  label,
  state,
  detail,
}: {
  label: string;
  state: "loading" | "ok" | "missing";
  detail?: string | undefined;
}) {
  const color =
    state === "ok"
      ? "text-emerald-300"
      : state === "missing"
        ? "text-amber-300"
        : "text-neutral-500";
  const mark = state === "ok" ? "✓" : state === "missing" ? "⚠" : "…";
  return (
    <li className="flex items-start gap-2">
      <span className={`${color} font-mono`}>{mark}</span>
      <div className="min-w-0 flex-1">
        <div className="text-neutral-200">{label}</div>
        {detail && (
          <div className="text-[11px] text-neutral-500">{detail}</div>
        )}
      </div>
    </li>
  );
}
