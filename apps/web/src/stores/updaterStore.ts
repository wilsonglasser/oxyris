import { create } from "zustand";
import { getVersion } from "@tauri-apps/api/app";
import { type UpdateStatus, checkForUpdate } from "~/lib/updater.ts";

const DISABLED_CACHE_KEY = "oxyris.updater.knownDisabled";

interface State {
  status: UpdateStatus | null;
  checkedAt: number | null;
  /**
   * Boot-time check. Skips the network hop once we've learned the endpoint
   * is misconfigured (placeholder pubkey / unreachable URL) so we don't spam
   * `update endpoint did not respond` on every launch. A manual "Check now"
   * in Settings clears the disabled flag.
   */
  backgroundCheck: () => Promise<void>;
  /** Force a fresh check — used by the Settings button. */
  forceCheck: () => Promise<void>;
  clearDot: () => void;
}

async function runCheck(): Promise<UpdateStatus | null> {
  try {
    const version = await getVersion();
    return await checkForUpdate(version);
  } catch {
    return null;
  }
}

export const useUpdaterStore = create<State>((set) => ({
  status: null,
  checkedAt: null,
  backgroundCheck: async () => {
    if (window.localStorage.getItem(DISABLED_CACHE_KEY) === "1") return;
    const status = await runCheck();
    if (!status) return;
    if (status.kind === "disabled") {
      window.localStorage.setItem(DISABLED_CACHE_KEY, "1");
    }
    set({ status, checkedAt: Date.now() });
  },
  forceCheck: async () => {
    window.localStorage.removeItem(DISABLED_CACHE_KEY);
    const status = await runCheck();
    if (!status) return;
    if (status.kind === "disabled") {
      window.localStorage.setItem(DISABLED_CACHE_KEY, "1");
    }
    set({ status, checkedAt: Date.now() });
  },
  clearDot: () =>
    set((s) =>
      s.status?.kind === "available"
        ? { status: { ...s.status, kind: "up_to_date" } }
        : {},
    ),
}));

/** Convenience selector — true iff a new version is available and unread. */
export function useHasUpdate(): boolean {
  return useUpdaterStore((s) => s.status?.kind === "available");
}
