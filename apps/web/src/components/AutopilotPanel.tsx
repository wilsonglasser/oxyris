import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Power, X } from "lucide-react";
import { useAutopilotStore } from "~/stores/autopilotStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import {
  type AutopilotEvent,
  autopilotDisengage,
  autopilotEngage,
} from "~/ipc/autopilot.ts";

interface Props {
  sessionId: string;
  onClose: () => void;
}

// Stable reference for the empty-log case. Returning a fresh `[]` from the
// zustand selector makes useSyncExternalStore see a new snapshot every render →
// infinite re-render loop ("Maximum update depth exceeded").
const EMPTY_LOG: AutopilotEvent[] = [];

/**
 * Floating mission panel for the auto-pilot. Anchored under the header's
 * auto-pilot button. The user pastes a mission (spec / changelog) the Supervisor
 * LLM drives the session toward, picks a supervisor backend, and engages.
 * Decisions stream into the mini-log (driven by the listener in PureSessionView).
 */
export function AutopilotPanel({ sessionId, onClose }: Props) {
  const { t } = useTranslation("chat");
  const hydrate = useAutopilotStore((s) => s.hydrate);
  const mission = useAutopilotStore((s) => s.mission[sessionId] ?? "");
  const enabled = useAutopilotStore((s) => s.enabled[sessionId] ?? false);
  const thinking = useAutopilotStore((s) => s.thinking[sessionId] ?? false);
  const config = useAutopilotStore((s) => s.config[sessionId]);
  const log = useAutopilotStore((s) => s.log[sessionId] ?? EMPTY_LOG);
  const setMission = useAutopilotStore((s) => s.setMission);
  const setEnabled = useAutopilotStore((s) => s.setEnabled);
  const setConfig = useAutopilotStore((s) => s.setConfig);
  const clearLog = useAutopilotStore((s) => s.clearLog);
  // Endpoint / credentials / turn budget are app-wide, not per-thread.
  const settings = useAppSettingsStore((s) => s.autopilot);
  const setSettings = useAppSettingsStore((s) => s.setAutopilot);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const logLine = (e: AutopilotEvent): string => {
    switch (e.kind) {
      case "thinking":
        return t("autopilot_thinking");
      case "approved":
        return t("autopilot_log_approved");
      case "rejected":
        return t("autopilot_log_rejected", { reason: e.reason });
      case "replied":
        return t("autopilot_log_replied", { text: e.text });
      case "halted":
        return t("autopilot_log_halted", { reason: e.reason });
      case "escalated":
        return t("autopilot_log_escalated", { why: e.why });
      case "error":
        return t("autopilot_log_error", { message: e.message });
    }
  };

  useEffect(() => {
    hydrate(sessionId);
  }, [sessionId, hydrate]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const cfg = config ?? { supervisor: "multi_model" as const };
  const isMultiModel = cfg.supervisor === "multi_model";
  const canEnable = mission.trim().length > 0 && !busy;
  // The active supervisor's model field maps onto the matching global setting.
  const modelValue = isMultiModel ? settings.model : settings.claudeModel;

  const engage = async () => {
    setBusy(true);
    setError(null);
    try {
      const model = (isMultiModel ? settings.model : settings.claudeModel).trim();
      await autopilotEngage({
        session_id: sessionId,
        mission,
        supervisor: cfg.supervisor,
        ...(model ? { model } : {}),
        ...(settings.baseUrl.trim() ? { base_url: settings.baseUrl.trim() } : {}),
        ...(settings.apiKey.trim() ? { api_key: settings.apiKey.trim() } : {}),
        ...(settings.maxTurns != null ? { max_turns: settings.maxTurns } : {}),
      });
      clearLog(sessionId);
      setEnabled(sessionId, true);
    } catch (e) {
      const msg =
        typeof e === "object" && e !== null && "message" in e
          ? String((e as { message: unknown }).message)
          : String(e);
      setError(msg);
    } finally {
      setBusy(false);
    }
  };

  const disengage = async () => {
    setBusy(true);
    try {
      await autopilotDisengage(sessionId);
    } catch {
      /* best-effort — clear UI state regardless */
    } finally {
      setEnabled(sessionId, false);
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-40" onMouseDown={onClose}>
      <div
        className="absolute right-3 top-11 z-50 flex max-h-[80vh] w-[24rem] flex-col rounded-xl border border-neutral-700 bg-neutral-900 p-3 shadow-xl shadow-black/40"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="mb-2 flex items-center gap-2">
          <Bot
            className={`size-4 ${
              thinking
                ? "animate-spin text-emerald-400"
                : enabled
                  ? "text-emerald-400"
                  : "text-neutral-400"
            }`}
            strokeWidth={1.75}
          />
          <span className="text-[12px] font-medium text-neutral-100">
            {t("autopilot_heading")}
          </span>
          {thinking && (
            <span className="text-[10px] text-emerald-400/80">
              {t("autopilot_thinking")}
            </span>
          )}
          <button
            type="button"
            onClick={onClose}
            aria-label={t("actions_close")}
            className="ml-auto text-neutral-500 hover:text-neutral-200"
          >
            <X className="size-3.5" strokeWidth={2} />
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto pr-0.5">
          <p className="mb-3 text-[11px] leading-snug text-neutral-500">
            {t("autopilot_desc")}
          </p>

          <label className="mb-3 block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_mission_label")}
            </span>
            <textarea
              value={mission}
              onChange={(e) => setMission(sessionId, e.target.value)}
              placeholder={t("autopilot_mission_placeholder")}
              rows={5}
              disabled={enabled}
              className="max-h-48 min-h-[6rem] w-full resize-y rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
            />
          </label>

          <label className="mb-3 block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_supervisor_label")}
            </span>
            <select
              value={cfg.supervisor}
              onChange={(e) =>
                setConfig(sessionId, {
                  supervisor: e.target.value as "multi_model" | "claude",
                })
              }
              disabled={enabled}
              className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
            >
              <option value="multi_model" className="bg-neutral-900">
                {t("autopilot_supervisor_multimodel")}
              </option>
              <option value="claude" className="bg-neutral-900">
                {t("autopilot_supervisor_claude")}
              </option>
            </select>
          </label>

          <div className="mb-3 mt-1 flex items-center gap-2">
            <div className="h-px flex-1 bg-neutral-800" />
            <span className="text-[9px] font-medium uppercase tracking-wider text-neutral-600">
              {t("autopilot_global_settings")}
            </span>
            <div className="h-px flex-1 bg-neutral-800" />
          </div>

          <label className="mb-3 block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_model_label")}
            </span>
            <input
              value={modelValue}
              onChange={(e) =>
                setSettings(
                  isMultiModel
                    ? { model: e.target.value }
                    : { claudeModel: e.target.value },
                )
              }
              placeholder={
                isMultiModel
                  ? t("autopilot_model_placeholder_openai")
                  : t("autopilot_model_placeholder_claude")
              }
              disabled={enabled}
              className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
            />
          </label>

          {isMultiModel && (
            <>
              <label className="mb-3 block">
                <span className="mb-1 block text-[11px] text-neutral-400">
                  {t("autopilot_base_url_label")}
                </span>
                <input
                  value={settings.baseUrl}
                  onChange={(e) => setSettings({ baseUrl: e.target.value })}
                  placeholder={t("autopilot_base_url_placeholder")}
                  disabled={enabled}
                  className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
                />
              </label>
              <label className="mb-3 block">
                <span className="mb-1 block text-[11px] text-neutral-400">
                  {t("autopilot_api_key_label")}
                </span>
                <input
                  type="password"
                  value={settings.apiKey}
                  onChange={(e) => setSettings({ apiKey: e.target.value })}
                  placeholder={t("autopilot_api_key_placeholder")}
                  disabled={enabled}
                  className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
                />
              </label>
            </>
          )}

          <label className="mb-3 block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_max_turns_label")}
            </span>
            <input
              type="number"
              min={1}
              value={settings.maxTurns ?? ""}
              onChange={(e) =>
                setSettings({
                  maxTurns: e.target.value ? Number(e.target.value) : null,
                })
              }
              disabled={enabled}
              className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700 disabled:opacity-60"
            />
          </label>

          {log.length > 0 && (
            <div className="mb-3 max-h-32 overflow-y-auto rounded border border-neutral-800 bg-neutral-950 p-2 text-[11px] text-neutral-400">
              {log.map((e, i) => (
                <div key={i} className="truncate" title={logLine(e)}>
                  {logLine(e)}
                </div>
              ))}
            </div>
          )}

          {error && (
            <p className="mb-3 rounded border border-red-900/60 bg-red-950/30 px-2 py-1 text-[11px] text-red-200">
              {error}
            </p>
          )}
        </div>

        {enabled ? (
          <button
            type="button"
            onClick={() => void disengage()}
            disabled={busy}
            className="mt-1 flex w-full items-center justify-center gap-2 rounded bg-red-900/40 px-3 py-1.5 text-[12px] font-medium text-red-200 ring-1 ring-inset ring-red-800/60 hover:bg-red-900/60 disabled:opacity-50"
          >
            <Power className="size-3.5" strokeWidth={2} />
            {t("autopilot_disable")}
          </button>
        ) : (
          <button
            type="button"
            onClick={() => void engage()}
            disabled={!canEnable}
            title={mission.trim() ? undefined : t("autopilot_needs_mission")}
            className="mt-1 flex w-full items-center justify-center gap-2 rounded bg-emerald-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-emerald-500 disabled:cursor-not-allowed disabled:opacity-40"
          >
            <Power className="size-3.5" strokeWidth={2} />
            {busy ? t("autopilot_engaging") : t("autopilot_enable")}
          </button>
        )}
      </div>
    </div>
  );
}
