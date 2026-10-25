import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type WidgetSettings = {
  enabled: boolean;
  /** Distro on the second row; null = first monitored distro. */
  wsl_distro: string | null;
};

export type WidgetMetrics = {
  width_px: number;
  height_px: number;
  scale: number;
  /** Taskbar (system) theme is light. */
  light: boolean;
  embedded: boolean;
};

export const WIDGET_METRICS_EVENT = "usage-widget://metrics";
export const WIDGET_CLICKED_EVENT = "usage-widget://clicked";

export async function widgetSettings(): Promise<WidgetSettings> {
  return invoke("usage_widget_settings");
}

export async function widgetSetSettings(settings: WidgetSettings): Promise<void> {
  await invoke("usage_widget_set_settings", { settings });
}

export async function widgetMetrics(): Promise<WidgetMetrics | null> {
  return invoke("usage_widget_metrics");
}

export async function widgetPaint(rgba: Uint8ClampedArray, width: number, height: number) {
  await invoke("usage_widget_paint", { rgba: Array.from(rgba), width, height });
}

export function onWidgetMetrics(cb: (m: WidgetMetrics) => void): Promise<UnlistenFn> {
  return listen<WidgetMetrics>(WIDGET_METRICS_EVENT, (e) => cb(e.payload));
}

export function onWidgetClicked(cb: () => void): Promise<UnlistenFn> {
  return listen(WIDGET_CLICKED_EVENT, () => cb());
}
