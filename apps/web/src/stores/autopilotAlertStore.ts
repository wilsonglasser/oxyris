import { create } from "zustand";

/**
 * Ephemeral escalation alerts raised by the auto-pilot. When the pilot hits a
 * human-only step (create an account, log in, pay, solve a CAPTCHA…) it halts
 * and emits an `escalated` event; the sidebar/session listeners capture it here
 * so the App can render an alert balloon from anywhere — including for a
 * background thread the user isn't currently viewing.
 *
 * Keyed by session id so two threads escalating don't clobber each other; the
 * newest `why` for a session wins.
 */
export interface AutopilotAlert {
  sessionId: string;
  why: string;
}

interface AutopilotAlertState {
  alerts: Record<string, AutopilotAlert>;
  raise: (sessionId: string, why: string) => void;
  dismiss: (sessionId: string) => void;
}

export const useAutopilotAlertStore = create<AutopilotAlertState>((set) => ({
  alerts: {},
  raise: (sessionId, why) =>
    set((prev) => ({
      alerts: { ...prev.alerts, [sessionId]: { sessionId, why } },
    })),
  dismiss: (sessionId) =>
    set((prev) => {
      if (!(sessionId in prev.alerts)) return prev;
      const next = { ...prev.alerts };
      delete next[sessionId];
      return { alerts: next };
    }),
}));
