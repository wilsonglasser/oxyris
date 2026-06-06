import { create } from "zustand";
import type { AutopilotEvent, SupervisorKind } from "~/ipc/autopilot.ts";

/**
 * Per-session auto-pilot config: the mission (a free-text spec / changelog the
 * Supervisor LLM drives the session toward), the supervisor backend choice, and
 * whether the pilot is engaged.
 *
 * Config + mission + enabled are `localStorage`-backed so they survive a
 * WebView reload (WebView2 can suspend + reload on background/resume — same
 * reason `drafts.ts` persists). The decision `log` is ephemeral.
 *
 * The backend Supervisor + AutopilotController (in `oxyris-supervisor` /
 * `infra::autopilot`) read the mission and act on `session:<id>:pure-signal`;
 * see `docs/design/autopilot.md`.
 */
const MISSION_PREFIX = "oxyris.autopilot.mission.";
const ENABLED_PREFIX = "oxyris.autopilot.enabled.";
const CONFIG_PREFIX = "oxyris.autopilot.config.";

export interface AutopilotConfig {
  supervisor: SupervisorKind;
  model: string;
  baseUrl: string;
  apiKey: string;
  maxTurns: number | null;
}

const DEFAULT_CONFIG: AutopilotConfig = {
  supervisor: "multi_model",
  model: "",
  baseUrl: "",
  apiKey: "",
  maxTurns: 30,
};

function readLS(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLS(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    /* localStorage may be disabled in odd contexts */
  }
}

interface AutopilotState {
  enabled: Record<string, boolean>;
  mission: Record<string, string>;
  config: Record<string, AutopilotConfig>;
  /** Ephemeral per-session decision log (most recent last). */
  log: Record<string, AutopilotEvent[]>;
  hydrate: (sessionId: string) => void;
  setEnabled: (sessionId: string, on: boolean) => void;
  setMission: (sessionId: string, text: string) => void;
  setConfig: (sessionId: string, patch: Partial<AutopilotConfig>) => void;
  pushLog: (sessionId: string, event: AutopilotEvent) => void;
  clearLog: (sessionId: string) => void;
}

export const useAutopilotStore = create<AutopilotState>((set, get) => ({
  enabled: {},
  mission: {},
  config: {},
  log: {},
  hydrate: (sessionId) => {
    const s = get();
    if (sessionId in s.config) return;
    const mission = readLS(MISSION_PREFIX + sessionId) ?? "";
    const enabled = readLS(ENABLED_PREFIX + sessionId) === "1";
    let config = DEFAULT_CONFIG;
    const rawCfg = readLS(CONFIG_PREFIX + sessionId);
    if (rawCfg) {
      try {
        config = { ...DEFAULT_CONFIG, ...JSON.parse(rawCfg) };
      } catch {
        /* corrupt — fall back to defaults */
      }
    }
    set((prev) => ({
      mission: { ...prev.mission, [sessionId]: mission },
      enabled: { ...prev.enabled, [sessionId]: enabled },
      config: { ...prev.config, [sessionId]: config },
    }));
  },
  setEnabled: (sessionId, on) => {
    writeLS(ENABLED_PREFIX + sessionId, on ? "1" : null);
    set((prev) => ({ enabled: { ...prev.enabled, [sessionId]: on } }));
  },
  setMission: (sessionId, text) => {
    writeLS(MISSION_PREFIX + sessionId, text.length ? text : null);
    set((prev) => ({ mission: { ...prev.mission, [sessionId]: text } }));
  },
  setConfig: (sessionId, patch) => {
    set((prev) => {
      const next = { ...(prev.config[sessionId] ?? DEFAULT_CONFIG), ...patch };
      writeLS(CONFIG_PREFIX + sessionId, JSON.stringify(next));
      return { config: { ...prev.config, [sessionId]: next } };
    });
  },
  pushLog: (sessionId, event) => {
    set((prev) => {
      const prevLog = prev.log[sessionId] ?? [];
      // Cap the log so a long autonomous run doesn't grow unbounded.
      const next = [...prevLog, event].slice(-50);
      return { log: { ...prev.log, [sessionId]: next } };
    });
  },
  clearLog: (sessionId) =>
    set((prev) => ({ log: { ...prev.log, [sessionId]: [] } })),
}));
