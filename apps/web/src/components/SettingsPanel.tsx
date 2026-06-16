import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import {
  BellRing,
  Bot,
  FileCode,
  FileText,
  Globe,
  Languages,
  Plug,
  RefreshCw,
  Save,
  Settings as SettingsIcon,
  TerminalSquare,
} from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import type { Environment } from "~/ipc/commands.ts";
import { LanguageSwitcher } from "~/components/LanguageSwitcher.tsx";
import { localEnvLabel } from "~/lib/host.ts";
import { LanguagePacksPanel } from "~/components/LanguagePacksPanel.tsx";
import { useKeybindingsStore } from "~/stores/keybindingsStore.ts";
import {
  syncAutopilotDefaults,
  useAppSettingsStore,
} from "~/stores/appSettingsStore.ts";
import type { SupervisorKind } from "~/ipc/autopilot.ts";
import {
  CLAUDE_LANGUAGES,
  type ClaudeLanguage,
} from "~/lib/claudeLanguage.ts";
import {
  type UpdateStatus,
  applyUpdate,
  checkForUpdate,
} from "~/lib/updater.ts";
import {
  SOUND_OPTIONS,
  type SoundChannel,
  getChannelSound,
  previewSound,
  setChannelSound,
} from "~/lib/notificationSound.ts";
import { useUpdaterStore } from "~/stores/updaterStore.ts";

type DiscoveredInstall = {
  provider_id: string;
  environment: Environment;
  path: string | null;
  version: string | null;
  error: string | null;
  is_interop_shim: boolean;
};

async function settingsProviderDiscover(): Promise<DiscoveredInstall[]> {
  return invoke<DiscoveredInstall[]>("settings_provider_discover");
}

type Tab = "general" | "languages" | "advanced";

export function SettingsPanel() {
  const { t } = useTranslation("settings");
  const [tab, setTab] = useState<Tab>("general");

  return (
    <section className="rounded-xl border border-neutral-800 bg-neutral-900/50">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-2.5">
        <h2 className="flex items-center gap-2 text-sm font-medium text-neutral-200">
          <SettingsIcon className="size-4" strokeWidth={1.75} />
          {t("heading")}
        </h2>
        <div className="flex gap-1">
          {(["general", "languages", "advanced"] as const).map((k) => (
            <button
              key={k}
              type="button"
              onClick={() => setTab(k)}
              className={`rounded px-2.5 py-1 text-[11px] transition ${
                tab === k
                  ? "bg-neutral-800 text-neutral-100"
                  : "text-neutral-400 hover:bg-neutral-800/60 hover:text-neutral-200"
              }`}
            >
              {t(`tabs.${k}`)}
            </button>
          ))}
        </div>
      </header>

      <div className="px-5 py-5">
        {tab === "general" && <GeneralTab />}
        {tab === "languages" && <LanguagePacksPanel />}
        {tab === "advanced" && <AdvancedTab />}
      </div>
    </section>
  );
}

function GeneralTab() {
  const { t } = useTranslation("settings");
  const [installs, setInstalls] = useState<DiscoveredInstall[]>([]);
  const [loading, setLoading] = useState(false);
  const [version, setVersion] = useState<string>("0.0.0");
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [completionSound, setCompletionSound] = useState<string>(() =>
    getChannelSound("completion"),
  );
  const [inputSound, setInputSound] = useState<string>(() =>
    getChannelSound("input"),
  );
  const [escalationSound, setEscalationSound] = useState<string>(() =>
    getChannelSound("escalation"),
  );
  const [missionSound, setMissionSound] = useState<string>(() =>
    getChannelSound("mission"),
  );
  const updateChannel = useCallback(
    (ch: SoundChannel, id: string) => {
      setChannelSound(ch, id);
      if (ch === "completion") setCompletionSound(id);
      else if (ch === "input") setInputSound(id);
      else if (ch === "escalation") setEscalationSound(id);
      else setMissionSound(id);
      previewSound(id);
    },
    [],
  );
  const pureMode = useAppSettingsStore((s) => s.pureMode);
  const setPureMode = useAppSettingsStore((s) => s.setPureMode);
  const openFilesExternally = useAppSettingsStore((s) => s.openFilesExternally);
  const setOpenFilesExternally = useAppSettingsStore(
    (s) => s.setOpenFilesExternally,
  );
  const claudeLanguage = useAppSettingsStore((s) => s.claudeLanguage);
  const setClaudeLanguage = useAppSettingsStore((s) => s.setClaudeLanguage);
  const autopilot = useAppSettingsStore((s) => s.autopilot);
  const setAutopilot = useAppSettingsStore((s) => s.setAutopilot);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => {});
    // Push the current config to the backend once on open, so the MCP engage
    // tool has it even if the user hasn't touched the settings since upgrading.
    syncAutopilotDefaults(useAppSettingsStore.getState().autopilot);
  }, []);

  const forceCheck = useUpdaterStore((s) => s.forceCheck);
  const runCheck = useCallback(async () => {
    setChecking(true);
    try {
      // Clear the "known disabled" cache so a re-check actually hits the wire.
      await forceCheck();
      setUpdateStatus(await checkForUpdate(version || "0.0.0"));
    } finally {
      setChecking(false);
    }
  }, [version, forceCheck]);

  const runInstall = useCallback(async () => {
    setInstalling(true);
    try {
      await applyUpdate();
    } catch (e) {
      setUpdateStatus({
        kind: "error",
        current: version,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setInstalling(false);
    }
  }, [version]);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setInstalls(await settingsProviderDiscover());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="flex flex-col gap-6">
      <Section
        icon={<Languages className="size-3.5" strokeWidth={1.75} />}
        title={t("section_language")}
      >
        <LanguageSwitcher />
      </Section>

      <Section
        icon={<Globe className="size-3.5" strokeWidth={1.75} />}
        title={t("section_claude_language")}
      >
        <label className="flex items-start gap-3 text-[11px] text-neutral-400">
          <span className="flex-1">
            <span className="font-medium text-neutral-200">
              {t("claude_language_title")}
            </span>
            <span className="block text-neutral-500">
              {t("claude_language_desc")}
            </span>
          </span>
          <select
            value={claudeLanguage}
            onChange={(e) =>
              setClaudeLanguage(e.target.value as ClaudeLanguage)
            }
            className="mt-0.5 shrink-0 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-neutral-200"
          >
            {CLAUDE_LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.code === "auto" ? t("claude_language_auto") : l.label}
              </option>
            ))}
          </select>
        </label>
      </Section>

      <Section
        icon={<RefreshCw className="size-3.5" strokeWidth={1.75} />}
        title={t("section_updates")}
        action={
          <button
            type="button"
            onClick={() => void runCheck()}
            disabled={checking}
            className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-60"
          >
            <RefreshCw
              className={`size-3 ${checking ? "animate-spin" : ""}`}
              strokeWidth={1.75}
            />
            {checking ? t("checking") : t("check_now")}
          </button>
        }
      >
        <p className="mb-2 text-[11px] text-neutral-500">
          {t("current_version_label")}:{" "}
          <span className="font-mono text-neutral-300">v{version}</span>
        </p>

        {updateStatus?.kind === "available" && (
          <div className="mb-3 rounded border border-emerald-900/60 bg-emerald-950/20 px-3 py-2 text-[11px] text-emerald-200">
            <div className="mb-1 font-medium text-emerald-100">
              {t("update_available", { version: updateStatus.latest })}
            </div>
            {updateStatus.body && (
              <pre className="mb-2 max-h-32 overflow-y-auto whitespace-pre-wrap font-sans text-[10px] text-emerald-200/90">
                {updateStatus.body}
              </pre>
            )}
            <button
              type="button"
              onClick={() => void runInstall()}
              disabled={installing}
              className="rounded bg-emerald-600 px-2 py-1 text-[11px] font-medium text-emerald-50 hover:bg-emerald-500 disabled:opacity-60"
            >
              {installing ? t("installing") : t("install_restart")}
            </button>
          </div>
        )}
        {updateStatus?.kind === "up_to_date" && (
          <p className="mb-3 text-[11px] text-neutral-500">
            {t("update_up_to_date")}
          </p>
        )}
        {updateStatus?.kind === "disabled" && (
          <p className="mb-3 rounded border border-amber-900/50 bg-amber-950/20 px-2.5 py-1.5 text-[10px] text-amber-200">
            {t("update_disabled")}
          </p>
        )}
        {updateStatus?.kind === "error" && (
          <p className="mb-3 rounded border border-red-900/50 bg-red-950/20 px-2.5 py-1.5 text-[10px] text-red-200">
            {t("update_error", { message: updateStatus.message ?? "" })}
          </p>
        )}
      </Section>

      <Section
        icon={<BellRing className="size-3.5" strokeWidth={1.75} />}
        title={t("section_notifications")}
      >
        <p className="mb-3 text-[11px] text-neutral-500">
          {t("notifications_sound_desc")}
        </p>
        <div className="flex flex-col gap-3">
          <ChannelPicker
            label={t("notifications_channel_completion_title")}
            description={t("notifications_channel_completion_desc")}
            value={completionSound}
            onChange={(id) => updateChannel("completion", id)}
            offLabel={t("notifications_sound_off")}
            testLabel={t("notifications_test")}
          />
          <ChannelPicker
            label={t("notifications_channel_input_title")}
            description={t("notifications_channel_input_desc")}
            value={inputSound}
            onChange={(id) => updateChannel("input", id)}
            offLabel={t("notifications_sound_off")}
            testLabel={t("notifications_test")}
          />
          <ChannelPicker
            label={t("notifications_channel_escalation_title")}
            description={t("notifications_channel_escalation_desc")}
            value={escalationSound}
            onChange={(id) => updateChannel("escalation", id)}
            offLabel={t("notifications_sound_off")}
            testLabel={t("notifications_test")}
          />
          <ChannelPicker
            label={t("notifications_channel_mission_title")}
            description={t("notifications_channel_mission_desc")}
            value={missionSound}
            onChange={(id) => updateChannel("mission", id)}
            offLabel={t("notifications_sound_off")}
            testLabel={t("notifications_test")}
          />
        </div>
      </Section>

      <Section
        icon={<TerminalSquare className="size-3.5" strokeWidth={1.75} />}
        title={t("section_pure_mode")}
      >
        <label className="flex items-start gap-2 text-[11px] text-neutral-400">
          <input
            type="checkbox"
            checked={pureMode}
            onChange={(e) => setPureMode(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="font-medium text-neutral-200">
              {t("pure_mode_title")}
            </span>
            <span className="block text-neutral-500">
              {t("pure_mode_desc")}
            </span>
          </span>
        </label>
      </Section>

      <Section
        icon={<Bot className="size-3.5" strokeWidth={1.75} />}
        title={t("section_autopilot")}
      >
        <p className="mb-3 text-[11px] text-neutral-500">
          {t("autopilot_desc")}
        </p>
        <div className="flex flex-col gap-3">
          <label className="block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_supervisor_label")}
            </span>
            <select
              value={autopilot.supervisor}
              onChange={(e) =>
                setAutopilot({
                  supervisor: e.target.value as SupervisorKind,
                })
              }
              className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200"
            >
              <option value="multi_model">
                {t("autopilot_supervisor_multimodel")}
              </option>
              <option value="claude">{t("autopilot_supervisor_claude")}</option>
            </select>
          </label>

          <label className="block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_model_label")}
            </span>
            <input
              value={
                autopilot.supervisor === "multi_model"
                  ? autopilot.model
                  : autopilot.claudeModel
              }
              onChange={(e) =>
                setAutopilot(
                  autopilot.supervisor === "multi_model"
                    ? { model: e.target.value }
                    : { claudeModel: e.target.value },
                )
              }
              placeholder={
                autopilot.supervisor === "multi_model"
                  ? t("autopilot_model_ph_openai")
                  : t("autopilot_model_ph_claude")
              }
              className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-600"
            />
          </label>

          {autopilot.supervisor === "multi_model" && (
            <>
              <label className="block">
                <span className="mb-1 block text-[11px] text-neutral-400">
                  {t("autopilot_base_url_label")}
                </span>
                <input
                  value={autopilot.baseUrl}
                  onChange={(e) => setAutopilot({ baseUrl: e.target.value })}
                  placeholder={t("autopilot_base_url_ph")}
                  className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-600"
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[11px] text-neutral-400">
                  {t("autopilot_api_key_label")}
                </span>
                <input
                  type="password"
                  value={autopilot.apiKey}
                  onChange={(e) => setAutopilot({ apiKey: e.target.value })}
                  placeholder={t("autopilot_api_key_ph")}
                  className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-600"
                />
              </label>
            </>
          )}

          <label className="block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_max_turns_label")}
            </span>
            <input
              type="number"
              min={1}
              value={autopilot.maxTurns ?? ""}
              onChange={(e) =>
                setAutopilot({
                  maxTurns: e.target.value ? Number(e.target.value) : null,
                })
              }
              className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-600"
            />
          </label>
        </div>
      </Section>

      <Section
        icon={<FileText className="size-3.5" strokeWidth={1.75} />}
        title={t("section_files")}
      >
        <label className="flex items-start gap-2 text-[11px] text-neutral-400">
          <input
            type="checkbox"
            checked={openFilesExternally}
            onChange={(e) => setOpenFilesExternally(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="font-medium text-neutral-200">
              {t("open_external_title")}
            </span>
            <span className="block text-neutral-500">
              {t("open_external_desc")}
            </span>
          </span>
        </label>
      </Section>

      <Section
        icon={<Plug className="size-3.5" strokeWidth={1.75} />}
        title={t("providers")}
        action={
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={loading}
            className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-60"
          >
            <RefreshCw
              className={`size-3 ${loading ? "animate-spin" : ""}`}
              strokeWidth={1.75}
            />
            {loading ? t("refreshing") : t("refresh")}
          </button>
        }
      >
        {installs.length === 0 && !loading ? (
          <p className="text-[11px] text-neutral-500">{t("no_providers")}</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {installs.map((inst, i) => (
              <InstallCard key={i} install={inst} />
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}

function AdvancedTab() {
  const { t } = useTranslation("settings");
  const reloadBindings = useKeybindingsStore((s) => s.reload);
  const [logsDir, setLogsDir] = useState<string | null>(null);
  const [keybindingsPath, setKeybindingsPath] = useState<string | null>(null);
  const [keybindings, setKeybindings] = useState<string>("");
  const [savingKb, setSavingKb] = useState(false);
  const [kbError, setKbError] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  useEffect(() => {
    void invoke<string>("settings_logs_dir").then(setLogsDir).catch(() => {});
    void invoke<string>("settings_keybindings_path")
      .then(setKeybindingsPath)
      .catch(() => {});
    void invoke<string>("settings_keybindings_read")
      .then(setKeybindings)
      .catch((e) => setKbError(String(e)));
  }, []);

  const onSaveKeybindings = async () => {
    setSavingKb(true);
    setKbError(null);
    try {
      await invoke("settings_keybindings_write", { contents: keybindings });
      await reloadBindings();
      setSavedAt(Date.now());
    } catch (e) {
      setKbError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavingKb(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <Section
        icon={<FileCode className="size-3.5" strokeWidth={1.75} />}
        title={t("section_keybindings")}
        action={
          <button
            type="button"
            onClick={() => void onSaveKeybindings()}
            disabled={savingKb}
            className="inline-flex items-center gap-1 rounded bg-neutral-200 px-2 py-1 text-[11px] font-medium text-neutral-900 hover:bg-white disabled:opacity-50"
          >
            <Save className="size-3" strokeWidth={1.75} />
            {savingKb ? t("saving") : t("save")}
          </button>
        }
      >
        <p className="mb-2 text-[10px] text-neutral-500">
          {t("section_keybindings_desc")}
        </p>
        {keybindingsPath && (
          <p className="mb-2 break-all font-mono text-[10px] text-neutral-600">
            {keybindingsPath}
          </p>
        )}
        <textarea
          value={keybindings}
          onChange={(e) => setKeybindings(e.target.value)}
          spellCheck={false}
          className="h-48 w-full resize-y rounded border border-neutral-800 bg-neutral-950 p-2 font-mono text-[11px] text-neutral-200 outline-none focus:border-neutral-700"
        />
        {kbError && (
          <p className="mt-2 rounded border border-red-900/60 bg-red-950/30 px-2 py-1 text-[11px] text-red-200">
            {kbError}
          </p>
        )}
        {!kbError && savedAt && (
          <p className="mt-2 text-[10px] text-emerald-300">{t("saved_ok")}</p>
        )}
      </Section>

      <Section
        icon={<FileText className="size-3.5" strokeWidth={1.75} />}
        title={t("logs_heading")}
      >
        <p className="text-[11px] text-neutral-400">{t("logs_path_label")}</p>
        {logsDir && (
          <p className="mt-1 break-all font-mono text-[11px] text-neutral-300">
            {logsDir}
          </p>
        )}
      </Section>

      <Section
        icon={<TerminalSquare className="size-3.5" strokeWidth={1.75} />}
        title={t("section_devtools")}
      >
        <p className="text-[11px] text-neutral-500">
          {t("section_devtools_desc")}
        </p>
      </Section>
    </div>
  );
}

function ChannelPicker({
  label,
  description,
  value,
  onChange,
  offLabel,
  testLabel,
}: {
  label: string;
  description: string;
  value: string;
  onChange: (id: string) => void;
  offLabel: string;
  testLabel: string;
}) {
  return (
    <div className="rounded-md border border-neutral-800 bg-neutral-950/40 px-3 py-2">
      <div className="mb-1.5 flex items-baseline justify-between gap-3">
        <span className="text-[11px] font-medium text-neutral-200">{label}</span>
        <button
          type="button"
          onClick={() => previewSound(value)}
          disabled={value === "off"}
          className="rounded border border-neutral-700 px-2 py-0.5 text-[10px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-40"
        >
          {testLabel}
        </button>
      </div>
      <p className="mb-2 text-[10px] text-neutral-500">{description}</p>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-[11px] text-neutral-200"
      >
        <option value="off">{offLabel}</option>
        {SOUND_OPTIONS.map((s) => (
          <option key={s.id} value={s.id}>
            {s.label}
          </option>
        ))}
      </select>
    </div>
  );
}

function Section({
  icon,
  title,
  action,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <header className="mb-2 flex items-center justify-between">
        <h3 className="flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wider text-neutral-500">
          {icon}
          {title}
        </h3>
        {action}
      </header>
      {children}
    </section>
  );
}

function InstallCard({ install }: { install: DiscoveredInstall }) {
  const { t } = useTranslation("settings");
  const envLabel =
    install.environment.kind === "local"
      ? localEnvLabel()
      : t("env_wsl", { distro: install.environment.distro });
  const providerLabel =
    install.provider_id === "claude" ? t("provider_claude") : install.provider_id;

  const statusColor = install.error
    ? "border-red-900/60 bg-red-950/30"
    : install.is_interop_shim
      ? "border-amber-900/60 bg-amber-950/30"
      : "border-emerald-900/60 bg-emerald-950/20";

  const statusLabel = install.error
    ? t("status_missing")
    : install.is_interop_shim
      ? t("status_interop_warning")
      : t("status_ok");

  return (
    <li className={`rounded-md border px-3 py-2 ${statusColor}`}>
      <div className="flex flex-wrap items-center gap-3 text-xs">
        <span className="font-medium text-neutral-100">
          {providerLabel} · {envLabel}
        </span>
        <span className="rounded-full border border-neutral-700 bg-neutral-950/50 px-2 py-0.5 text-[10px] text-neutral-200">
          {statusLabel}
        </span>
        {install.version && (
          <span className="text-neutral-300">
            {t("version")}: <span className="font-mono">{install.version}</span>
          </span>
        )}
      </div>
      {install.path && (
        <div className="mt-1 font-mono text-[11px] text-neutral-400">
          {t("path")}: {install.path}
        </div>
      )}
      {install.is_interop_shim && (
        <p className="mt-2 text-[11px] text-amber-200">{t("interop_hint")}</p>
      )}
      {install.error && (
        <p className="mt-1 text-[11px] text-red-200">
          {t("error_label", { message: install.error })}
        </p>
      )}
    </li>
  );
}
