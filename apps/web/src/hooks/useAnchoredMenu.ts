import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";

/** Gap kept between a popup and the viewport edges. */
const EDGE_GAP = 6;

export type MenuAlign = "left" | "right";

/**
 * Positions a `fixed` popup at a pointer anchor without letting it spill out of
 * the viewport: it flips to the other side of the anchor when the preferred
 * side has no room, clamps to the edges, and caps the height (turning the popup
 * into a scroller) when neither side fits.
 *
 * `align` says which edge of the popup the anchor refers to — `"left"` for the
 * usual right-and-down context menu, `"right"` for menus opened off a control
 * near the right edge.
 */
export function useAnchoredMenu<T extends HTMLElement = HTMLDivElement>(
  x: number,
  y: number,
  align: MenuAlign = "left",
) {
  const ref = useRef<T | null>(null);
  const [style, setStyle] = useState<CSSProperties>({ left: x, top: y });

  // Layout effect: the corrected position must land before the browser paints,
  // otherwise the menu visibly jumps from the raw pointer position.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const place = () => measure(el, x, y, align, setStyle);
    place();
    // Menus that fill in after mount (async branch lists, lazily loaded rows)
    // change height once the data lands — re-place so a grown menu still
    // flips or scrolls instead of running off the bottom. Re-placing is
    // idempotent, so the observer settles after one extra pass.
    const ro = new ResizeObserver(place);
    ro.observe(el);
    return () => ro.disconnect();
  }, [x, y, align]);

  return { ref, style };
}

function measure(
  el: HTMLElement,
  x: number,
  y: number,
  align: MenuAlign,
  setStyle: (s: CSSProperties) => void,
) {
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const w = el.offsetWidth;
  // `scrollHeight` is the natural content height, so re-running this after a
  // `maxHeight` was applied still measures what the menu *wants* to be.
  // (+2 covers the border, which `scrollHeight` excludes.)
  const h = el.scrollHeight + 2;

  let left = align === "right" ? x - w : x;
  if (left + w > vw - EDGE_GAP) {
    // Flip to the other side of the anchor.
    left = align === "right" ? x : x - w;
  }
  left = Math.min(
    Math.max(EDGE_GAP, left),
    Math.max(EDGE_GAP, vw - EDGE_GAP - w),
  );

  const below = vh - EDGE_GAP - y;
  const above = y - EDGE_GAP;
  let top = y;
  let maxHeight: number | undefined;
  if (h > below) {
    if (h <= above) {
      top = y - h;
    } else if (above > below) {
      top = EDGE_GAP;
      maxHeight = above;
    } else {
      maxHeight = below;
    }
  }

  setStyle({
    left,
    top,
    maxHeight,
    overflowY: maxHeight ? "auto" : undefined,
  });
}
