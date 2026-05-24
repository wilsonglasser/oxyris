import { invoke } from "@tauri-apps/api/core";

/**
 * Taskbar unread badge — a WhatsApp-style counter on the app's taskbar icon.
 *
 * Incremented when a turn completes (or needs attention) while the window is
 * unfocused; cleared when the window regains focus (see App.tsx). Windows has
 * no numeric taskbar badge, so we render the number into a small overlay icon
 * here on a canvas and hand the raw RGBA to the backend, which sets it via
 * `set_overlay_icon`. macOS/Linux ignore the pixels and get a native count.
 */

const SIZE = 32;

let count = 0;

function renderBadge(n: number): { rgba: number[]; width: number; height: number } {
  const label = n > 99 ? "99+" : String(n);
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const ctx = canvas.getContext("2d");
  if (!ctx) return { rgba: [], width: 0, height: 0 };

  // Filled red disc, leaving a 1px transparent margin so it doesn't clip.
  ctx.fillStyle = "#ef4444";
  ctx.beginPath();
  ctx.arc(SIZE / 2, SIZE / 2, SIZE / 2 - 1, 0, Math.PI * 2);
  ctx.fill();

  // Centered white count. Shrink the font for 3-glyph labels ("99+").
  ctx.fillStyle = "#ffffff";
  const fontPx = label.length >= 3 ? 13 : 19;
  ctx.font = `bold ${fontPx}px "Segoe UI", system-ui, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(label, SIZE / 2, SIZE / 2 + 1);

  const data = ctx.getImageData(0, 0, SIZE, SIZE).data;
  return { rgba: Array.from(data), width: SIZE, height: SIZE };
}

async function push(): Promise<void> {
  try {
    if (count <= 0) {
      await invoke("set_taskbar_badge", { count: 0, rgba: [], width: 0, height: 0 });
      return;
    }
    const { rgba, width, height } = renderBadge(count);
    await invoke("set_taskbar_badge", { count, rgba, width, height });
  } catch {
    /* not running under Tauri, or platform doesn't support it — ignore */
  }
}

/** Add one to the unread count and refresh the taskbar icon. */
export function bumpBadge(): void {
  count += 1;
  void push();
}

/** Reset the unread count (e.g. on window focus). No-op when already zero. */
export function clearBadge(): void {
  if (count === 0) return;
  count = 0;
  void push();
}
