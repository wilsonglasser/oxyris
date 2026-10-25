import type { EnvBoard, UsageBoard, UsageWindow } from "~/ipc/claudeAccounts.ts";
import type { WidgetMetrics, WidgetSettings } from "~/ipc/usageWidget.ts";

/**
 * Pixels for the native taskbar widget (`infra::usage_widget`). Drawn in
 * device pixels straight from the backend's metrics; one row per environment
 * (Windows, then the chosen WSL distro), each with the active account's
 * 5-hour and weekly bars.
 */

export type WidgetRow = {
  label: string;
  fiveHour: UsageWindow | null;
  sevenDay: UsageWindow | null;
};

export function widgetRows(
  board: UsageBoard | null,
  settings: WidgetSettings,
  labels: { local: string; wsl: string },
): WidgetRow[] {
  if (!board) return [];
  const row = (env: EnvBoard | undefined, label: string): WidgetRow | null => {
    if (!env) return null;
    const active = env.accounts.find((a) => a.active);
    if (!active) return null;
    const usage = active.usage?.usage ?? null;
    return { label, fiveHour: usage?.five_hour ?? null, sevenDay: usage?.seven_day ?? null };
  };
  const local = board.environments.find((e) => e.environment.kind === "local");
  const wslEnvs = board.environments.filter((e) => e.environment.kind === "wsl");
  const wsl =
    wslEnvs.find(
      (e) => e.environment.kind === "wsl" && e.environment.distro === settings.wsl_distro,
    ) ?? wslEnvs[0];
  return [row(local, labels.local), row(wsl, labels.wsl)].filter(
    (r): r is WidgetRow => r !== null,
  );
}

function tone(pct: number): string {
  if (pct >= 90) return "#ef4444";
  if (pct >= 70) return "#f59e0b";
  return "#22c55e";
}

export function renderWidget(rows: WidgetRow[], m: WidgetMetrics): ImageData | null {
  const { width_px: w, height_px: h, scale: s } = m;
  if (w <= 0 || h <= 0) return null;
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;

  // Nothing to show: stay fully transparent, which a layered window also
  // makes click-through, so the taskbar underneath keeps working.
  if (rows.length === 0) return ctx.getImageData(0, 0, w, h);
  // Otherwise keep the whole strip clickable (alpha-0 pixels would pass
  // clicks through) with an invisible alpha-1/255 wash.
  ctx.fillStyle = "rgba(128,128,128,0.004)";
  ctx.fillRect(0, 0, w, h);

  const fg = m.light ? "rgba(0,0,0,0.88)" : "rgba(255,255,255,0.92)";
  const track = m.light ? "rgba(0,0,0,0.16)" : "rgba(255,255,255,0.22)";
  const pad = 4 * s;
  const labelW = 26 * s;
  const pctW = 25 * s;
  const gap = 4 * s;
  const barW = (w - pad * 2 - labelW - gap * 2 - pctW * 2) / 2;
  const barH = Math.max(2, Math.round(3 * s));
  // Two rows split the height; a lone row is centered.
  const rowH = h / Math.max(2, rows.length);
  const top = rows.length === 1 ? (h - rowH) / 2 : 0;
  const font = `${Math.round(9.5 * s)}px "Segoe UI Variable Text", "Segoe UI", sans-serif`;

  rows.forEach((row, i) => {
    const cy = top + rowH * i + rowH / 2;
    ctx.textBaseline = "middle";
    ctx.fillStyle = fg;
    ctx.font = `600 ${font}`;
    ctx.textAlign = "left";
    ctx.fillText(row.label, pad, cy);

    let x = pad + labelW;
    for (const win of [row.fiveHour, row.sevenDay]) {
      const pct = win ? Math.max(0, Math.min(100, win.utilization)) : 0;
      const barY = Math.round(cy - barH / 2);
      ctx.fillStyle = track;
      ctx.fillRect(Math.round(x), barY, Math.round(barW), barH);
      if (win) {
        ctx.fillStyle = tone(pct);
        ctx.fillRect(Math.round(x), barY, Math.round((barW * pct) / 100), barH);
      }
      ctx.fillStyle = fg;
      ctx.font = font;
      ctx.textAlign = "right";
      ctx.fillText(win ? `${Math.round(win.utilization)}%` : "—", x + barW + pctW - 1 * s, cy);
      x += barW + pctW + gap;
    }
  });
  return ctx.getImageData(0, 0, w, h);
}
