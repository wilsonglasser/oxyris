import { create } from "zustand";
import { getVersion } from "@tauri-apps/api/app";
import { type UpdateStatus, applyUpdate, checkForUpdate } from "~/lib/updater.ts";

// Timestamp (ms) until which boot checks stay suppressed after the endpoint
// reported "disabled" (placeholder pubkey / unreachable URL / 404). Stored as
// an expiry rather than a permanent flag so a transient outage — or a release
// pipeline that only just started publishing `latest.json` — heals itself on
// the next launch past the TTL instead of disabling updates forever.
const DISABLED_UNTIL_KEY = "oxyris.updater.disabledUntil";
const DISABLED_RETRY_MS = 6 * 60 * 60 * 1000; // 6h
// Legacy permanent flag from before the TTL scheme. Migrated away on load so
// users poisoned by the broken-endpoint era start re-checking immediately.
const LEGACY_DISABLED_KEY = "oxyris.updater.knownDisabled";

function isCheckSuppressed(): boolean {
  // One-time migration: a leftover permanent "1" must not keep updates off.
  if (window.localStorage.getItem(LEGACY_DISABLED_KEY) !== null) {
    window.localStorage.removeItem(LEGACY_DISABLED_KEY);
  }
  const raw = window.localStorage.getItem(DISABLED_UNTIL_KEY);
  if (!raw) return false;
  const until = Number(raw);
  if (!Number.isFinite(until) || Date.now() >= until) {
    window.localStorage.removeItem(DISABLED_UNTIL_KEY);
    return false;
  }
  return true;
}

function suppressChecks(): void {
  window.localStorage.setItem(
    DISABLED_UNTIL_KEY,
    String(Date.now() + DISABLED_RETRY_MS),
  );
}

interface State {
  status: UpdateStatus | null;
  checkedAt: number | null;
  /** True while an update download/install is in flight. */
  applying: boolean;
  /**
   * Boot-time check. Skips the network hop only while a recent "disabled"
   * result is still within its retry TTL, so we don't spam
   * `update endpoint did not respond` on every launch — but a previously
   * broken endpoint is re-checked once the TTL lapses. A manual "Check now"
   * in Settings clears the suppression immediately.
   */
  backgroundCheck: () => Promise<void>;
  /** Force a fresh check — used by the Settings button. */
  forceCheck: () => Promise<void>;
  /** Download + install the available update, then relaunch. */
  install: () => Promise<void>;
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
  applying: false,
  backgroundCheck: async () => {
    if (isCheckSuppressed()) return;
    const status = await runCheck();
    if (!status) return;
    if (status.kind === "disabled") suppressChecks();
    set({ status, checkedAt: Date.now() });
  },
  forceCheck: async () => {
    window.localStorage.removeItem(DISABLED_UNTIL_KEY);
    const status = await runCheck();
    if (!status) return;
    if (status.kind === "disabled") suppressChecks();
    set({ status, checkedAt: Date.now() });
  },
  install: async () => {
    set({ applying: true });
    try {
      await applyUpdate();
      // applyUpdate relaunches on success; this line is reached only if the
      // user cancels a pending download or it returns without restarting.
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      set((s) => ({
        status: s.status
          ? { ...s.status, kind: "error", message }
          : { kind: "error", current: "0.0.0", message },
      }));
    } finally {
      set({ applying: false });
    }
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
