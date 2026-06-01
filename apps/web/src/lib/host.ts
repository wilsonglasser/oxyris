/** Host-OS helpers derived from the Tauri WebView user-agent.
 *
 * The native environment (`Environment::Local`) renders on whatever OS the
 * desktop app runs on. The WebView's user-agent reflects that host (WebView2 →
 * "Windows", WKWebView → "Macintosh", WebKitGTK → "Linux"), so we read it to
 * label the local environment and to decide whether WSL is offered at all. */

/** True when the desktop app is running on a Windows host (the only host where
 * WSL environments exist). */
export const isWindowsHost = navigator.userAgent.includes("Windows");

/** Display name for the native `Local` environment on this host. */
export function localEnvLabel(): string {
  const ua = navigator.userAgent;
  if (ua.includes("Windows")) return "Windows";
  if (ua.includes("Macintosh") || ua.includes("Mac OS")) return "macOS";
  if (ua.includes("Linux")) return "Linux";
  return "Local";
}
