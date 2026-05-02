import { check, type Update } from "@tauri-apps/plugin-updater";

/**
 * Wrapper around the Tauri updater plugin that swallows the "no valid pubkey
 * configured" failure mode — until a real keypair lands via
 * `bun tauri signer generate`, checks should return `null` instead of
 * throwing so the UI doesn't shout about misconfiguration on every boot.
 */

export interface UpdateStatus {
  kind: "up_to_date" | "available" | "disabled" | "error";
  current: string;
  latest?: string;
  body?: string | null;
  date?: string | null;
  message?: string;
}

export async function checkForUpdate(
  currentVersion: string,
): Promise<UpdateStatus> {
  try {
    const update = await check();
    if (!update) {
      return { kind: "up_to_date", current: currentVersion };
    }
    return {
      kind: "available",
      current: currentVersion,
      latest: update.version,
      body: update.body ?? null,
      date: update.date ?? null,
    };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // A placeholder pubkey / missing endpoint / network outage all land here.
    // We treat them as "updater disabled" until the release pipeline is live.
    if (
      /pubkey|signature|endpoint|404|network|failed to fetch/i.test(message)
    ) {
      return { kind: "disabled", current: currentVersion, message };
    }
    return { kind: "error", current: currentVersion, message };
  }
}

/** Download and install the update, restarting the app afterwards. */
export async function applyUpdate(): Promise<void> {
  const update: Update | null = await check();
  if (!update) return;
  await update.downloadAndInstall();
  // The plugin restarts the app automatically on success.
}
