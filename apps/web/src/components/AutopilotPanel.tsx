import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Power, X } from "lucide-react";
import { useAutopilotStore } from "~/stores/autopilotStore.ts";
import {
  type AutopilotEvent,
  autopilotDisengage,
  autopilotEngage,
} from "~/ipc/autopilot.ts";

interface Props {
  sessionId: string;
  onClose: () => void;
}

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
  const config = useAutopilotStore((s) => s.config[sessionId]);
  const log = useAutopilotStore((s) => s.log[sessionId] ?? []);
  const setMission = useAutopilotStore((s) => s.setMission);
  const setEnabled = useAutopilotStore((s) => s.setEnabled);
  const setConfig = useAutopilotStore((s) => s.setConfig);
  const clearLog = useAutopilotStore((s) => s.clearLog);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const logLine = (e: AutopilotEvent): string => {
    switch (e.kind) {
      case "approved":
        return t("autopilot_log_approved");
      case "rejected":
        return t("autopilot_log_rejected", { reason: e.reason });
      case "replied":
        return t("autopilot_log_replied", { text: e.text });
      case "halted":
        return t("autopilot_log_halted", { reason: e.reason });
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

  const cfg = config ?? {
    supervisor: "multi_model" as const,
    model: "",
    baseUrl: "",
    apiKey: "",
    maxTurns: 30,
  };
  const isMultiModel = cfg.supervisor === "multi_model";
  const canEnable = mission.trim().length > 0 && !busy;

  const engage = async () => {
    setBusy(true);
    setError(null);
    try {
      await autopilotEngage({
        session_id: sessionId,
        mission,
        supervisor: cfg.supervisor,
        ...(cfg.model.trim() ? { model: cfg.model.trim() } : {}),
        ...(cfg.baseUrl.trim() ? { base_url: cfg.baseUrl.trim() } : {}),
        ...(cfg.apiKey.trim() ? { api_key: cfg.apiKey.trim() } : {}),
        ...(cfg.maxTurns != null ? { max_turns: cfg.maxTurns } : {}),
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
            className={enabled ? "size-4 text-emerald-400" : "size-4 text-neutral-400"}
            strokeWidth={1.75}
          />
          <span className="text-[12px] font-medium text-neutral-100">
            {t("autopilot_heading")}
          </span>
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

          <label className="mb-3 block">
            <span className="mb-1 block text-[11px] text-neutral-400">
              {t("autopilot_model_label")}
            </span>
            <input
              value={cfg.model}
              onChange={(e) => setConfig(sessionId, { model: e.target.value })}
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
                  value={cfg.baseUrl}
                  onChange={(e) =>
                    setConfig(sessionId, { baseUrl: e.target.value })
                  }
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
                  value={cfg.apiKey}
                  onChange={(e) =>
                    setConfig(sessionId, { apiKey: e.target.value })
                  }
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
              value={cfg.maxTurns ?? ""}
              onChange={(e) =>
                setConfig(sessionId, {
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
