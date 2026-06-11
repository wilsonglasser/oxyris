import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Power, X } from "lucide-react";
import { useAutopilotAlertStore } from "~/stores/autopilotAlertStore.ts";
import { useAutopilotStore } from "~/stores/autopilotStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import { autopilotEngage } from "~/ipc/autopilot.ts";

/**
 * Balloon stack for auto-pilot escalations. When the pilot hits a human-only
 * step it halts (disengages) and raises an alert here. Each balloon shows the
 * supervisor's explanation and a one-click "reactivate" — so once you've done
 * the manual step (created the account, logged in…) you hand control straight
 * back without re-opening the panel and re-typing the mission.
 */
export function AutopilotAlerts() {
  const { t } = useTranslation("chat");
  const alerts = useAutopilotAlertStore((s) => s.alerts);
  const dismiss = useAutopilotAlertStore((s) => s.dismiss);
  const [busy, setBusy] = useState<string | null>(null);

  const list = Object.values(alerts);
  if (list.length === 0) return null;

  const reactivate = async (sessionId: string) => {
    setBusy(sessionId);
    try {
      const store = useAutopilotStore.getState();
      const mission = store.mission[sessionId] ?? "";
      if (!mission.trim()) {
        dismiss(sessionId);
        return;
      }
      const cfg = store.config[sessionId] ?? { supervisor: "multi_model" as const };
      const settings = useAppSettingsStore.getState().autopilot;
      const isMulti = cfg.supervisor === "multi_model";
      const model = (isMulti ? settings.model : settings.claudeModel).trim();
      await autopilotEngage({
        session_id: sessionId,
        mission,
        supervisor: cfg.supervisor,
        ...(model ? { model } : {}),
        ...(settings.baseUrl.trim() ? { base_url: settings.baseUrl.trim() } : {}),
        ...(settings.apiKey.trim() ? { api_key: settings.apiKey.trim() } : {}),
        ...(settings.maxTurns != null ? { max_turns: settings.maxTurns } : {}),
      });
      store.setEnabled(sessionId, true);
      dismiss(sessionId);
    } catch {
      // Leave the balloon up so the user can retry; the panel still works too.
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="fixed bottom-4 right-4 z-50 flex max-w-sm flex-col gap-2">
      {list.map((a) => (
        <div
          key={a.sessionId}
          role="alert"
          className="rounded-lg border border-amber-600/70 bg-amber-950/40 px-4 py-3 text-[12px] text-amber-100 shadow-xl shadow-black/50 backdrop-blur"
        >
          <div className="mb-1 flex items-center gap-2 font-medium text-amber-200">
            <AlertTriangle className="size-4 shrink-0" strokeWidth={2} />
            {t("autopilot_alert_title")}
          </div>
          <div className="text-amber-100/90">{a.why}</div>
          <div className="mt-2.5 flex items-center gap-2">
            <button
              type="button"
              onClick={() => void reactivate(a.sessionId)}
              disabled={busy === a.sessionId}
              className="flex items-center gap-1.5 rounded bg-amber-600 px-2.5 py-1 text-[11px] font-medium text-amber-950 hover:bg-amber-500 disabled:opacity-50"
            >
              <Power className="size-3" strokeWidth={2.25} />
              {t("autopilot_alert_reactivate")}
            </button>
            <button
              type="button"
              onClick={() => dismiss(a.sessionId)}
              className="flex items-center gap-1 rounded px-1.5 py-1 text-[11px] text-amber-300/80 hover:text-amber-100"
            >
              <X className="size-3" strokeWidth={2} />
              {t("autopilot_alert_dismiss")}
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
