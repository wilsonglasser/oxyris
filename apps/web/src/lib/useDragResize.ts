import { useEffect, useRef, useState } from "react";

export type ResizeAxis = "horizontal" | "vertical";

interface Options {
  storageKey: string;
  defaultSize: number;
  min: number;
  max: number;
  axis: ResizeAxis;
  /**
   * For axis="horizontal": when "right", dragging right grows. When "left",
   * dragging right shrinks (handle is on the panel's left edge).
   * For axis="vertical": when "down", dragging down grows. When "up",
   * dragging up grows (handle on the panel's top edge, e.g. terminal panel).
   */
  direction?: "right" | "left" | "down" | "up";
}

function read(key: string, def: number, min: number, max: number): number {
  try {
    const raw = window.localStorage.getItem(key);
    const n = Number(raw);
    if (Number.isFinite(n) && n >= min && n <= max) return n;
  } catch {
    /* localStorage may be unavailable */
  }
  return def;
}

export function useDragResize({
  storageKey,
  defaultSize,
  min,
  max,
  axis,
  direction,
}: Options) {
  const [size, setSize] = useState<number>(() =>
    read(storageKey, defaultSize, min, max),
  );

  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, String(size));
    } catch {
      /* ignore */
    }
  }, [storageKey, size]);

  const startRef = useRef<{ pos: number; size: number } | null>(null);

  const onResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    const isHorizontal = axis === "horizontal";
    const dir =
      direction ?? (isHorizontal ? "right" : "down");
    const sign = dir === "right" || dir === "down" ? 1 : -1;

    startRef.current = {
      pos: isHorizontal ? e.clientX : e.clientY,
      size,
    };
    const onMove = (ev: MouseEvent) => {
      const start = startRef.current;
      if (!start) return;
      const cur = isHorizontal ? ev.clientX : ev.clientY;
      const delta = (cur - start.pos) * sign;
      const next = Math.max(min, Math.min(max, start.size + delta));
      setSize(next);
    };
    const onUp = () => {
      startRef.current = null;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = isHorizontal ? "col-resize" : "row-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return { size, setSize, onResizeStart };
}
