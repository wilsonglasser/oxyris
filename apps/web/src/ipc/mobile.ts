import { invoke } from "@tauri-apps/api/core";

/** Pairing info for the mobile-takeover companion server. */
export type MobileInfo = {
  running: boolean;
  /** Full URL (with `#t=<token>`) a phone opens to pair. */
  url: string;
  token: string;
  port: number;
  /** Inline SVG of the pairing QR; empty if generation failed. */
  qr_svg: string;
};

/** Start the LAN server (idempotent — returns existing info if already up). */
export async function mobileTakeoverStart(): Promise<MobileInfo> {
  return invoke("mobile_takeover_start");
}

/** Stop the server, dropping any in-flight phone takeovers. */
export async function mobileTakeoverStop(): Promise<MobileInfo> {
  return invoke("mobile_takeover_stop");
}

/** Current running state + pairing info. */
export async function mobileTakeoverStatus(): Promise<MobileInfo> {
  return invoke("mobile_takeover_status");
}

/**
 * Take control back from a phone that took over this session's pure terminal —
 * used by the desktop "retomar" button when the phone left without releasing.
 */
export async function mobileTakeoverForceRelease(input: {
  session_id: string;
}): Promise<void> {
  await invoke("mobile_takeover_force_release", { input });
}
