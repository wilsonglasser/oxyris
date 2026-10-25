import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Environment } from "~/ipc/commands.ts";

export type Plan =
  | "pro"
  | "max5x"
  | "max20x"
  | "max"
  | "team"
  | "enterprise"
  | "free"
  | "unknown";

export type AccountIdentity = {
  email: string;
  display_name: string | null;
  account_uuid: string;
  organization_uuid: string | null;
  organization_name: string | null;
};

export type UsageWindow = {
  /** Percent used, 0–100 (can exceed 100). */
  utilization: number;
  /** RFC 3339; null when the window isn't running. */
  resets_at: string | null;
};

export type UsageSnapshot = {
  five_hour: UsageWindow | null;
  seven_day: UsageWindow | null;
  seven_day_opus: UsageWindow | null;
  seven_day_sonnet: UsageWindow | null;
};

export type UsageError =
  | { kind: "auth"; message: string }
  | { kind: "rate_limited"; message: string }
  | { kind: "no_scope" }
  | { kind: "network"; message: string }
  | { kind: "http"; message: string }
  | { kind: "credentials"; message: string };

export type UsageEntry = {
  usage: UsageSnapshot | null;
  error: UsageError | null;
  fetched_at_ms: number | null;
};

export type AccountCard = {
  /** null = live login not registered yet. */
  id: string | null;
  identity: AccountIdentity;
  plan: Plan;
  active: boolean;
  usage: UsageEntry | null;
};

export type EnvBoard = {
  environment: Environment;
  /** `local` | `wsl:<distro>` */
  key: string;
  live_error: string | null;
  /** Distro not running: cards are the last known ones (stale usage). */
  offline: boolean;
  accounts: AccountCard[];
};

export type UsageBoard = {
  environments: EnvBoard[];
  updated_at_ms: number;
};

export type AccountRecord = {
  id: string;
  environment: Environment;
  identity: AccountIdentity;
  plan: Plan;
  added_at_ms: number;
};

export type EnvironmentEntry = Environment & {
  state: string | null;
  version: string | null;
  is_default: boolean;
};

export const USAGE_BOARD_EVENT = "claude-usage://board";

export function envKey(env: Environment): string {
  return env.kind === "local" ? "local" : `wsl:${env.distro}`;
}

export async function environmentList(): Promise<EnvironmentEntry[]> {
  return invoke("environment_list");
}

export async function usageBoard(): Promise<UsageBoard> {
  return invoke("claude_usage_board");
}

export async function usageRefresh(): Promise<UsageBoard> {
  return invoke("claude_usage_refresh");
}

export async function accountsTrack(environment: Environment): Promise<UsageBoard> {
  return invoke("claude_accounts_track", { environment });
}

export async function accountsSaveLive(environment: Environment): Promise<UsageBoard> {
  return invoke("claude_accounts_save_live", { environment });
}

/** Resolves when the login console closes; null if nothing was logged in. */
export async function accountsLogin(
  environment: Environment,
): Promise<AccountRecord | null> {
  return invoke("claude_accounts_login", { environment });
}

/**
 * Rejects with `{ code: "sessions_running", message: <count> }` when Claude
 * sessions are open in that environment; retry with `force` after confirming.
 */
export async function accountsActivate(
  environment: Environment,
  id: string,
  force = false,
): Promise<UsageBoard> {
  return invoke("claude_accounts_activate", { environment, id, force });
}

/** Count from a `sessions_running` rejection, else null. */
export function sessionsRunningCount(e: unknown): number | null {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as { code: string; message?: unknown };
    if (err.code === "sessions_running" && typeof err.message === "number") {
      return err.message;
    }
  }
  return null;
}

export async function accountsRemove(
  environment: Environment,
  id: string,
): Promise<UsageBoard> {
  return invoke("claude_accounts_remove", { environment, id });
}

export function onUsageBoard(cb: (board: UsageBoard) => void): Promise<UnlistenFn> {
  return listen<UsageBoard>(USAGE_BOARD_EVENT, (e) => cb(e.payload));
}

/** Distro name from a `wsl_offline` rejection, else null. */
export function wslOfflineDistro(e: unknown): string | null {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as { code: string; message?: unknown };
    if (err.code === "wsl_offline" && typeof err.message === "string") return err.message;
  }
  return null;
}

/** Tauri command errors arrive as `{ code, message? }`. */
export function accountsErrorMessage(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as { code: string; message?: unknown };
    return typeof err.message === "string" ? err.message : err.code;
  }
  return e instanceof Error ? e.message : String(e);
}
