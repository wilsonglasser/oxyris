import type { ReactNode } from "react";

import { useAnchoredMenu, type MenuAlign } from "~/hooks/useAnchoredMenu.ts";

type Props = {
  /** Anchor point in client coordinates (usually `event.clientX/clientY`). */
  x: number;
  y: number;
  /** Which edge of the menu the anchor refers to. */
  align?: MenuAlign;
  /** Extra classes — width constraints belong here. */
  className?: string;
  children: ReactNode;
};

/**
 * Popup surface for context menus: shared chrome plus viewport-aware placement
 * so a menu opened near an edge flips or scrolls instead of getting clipped.
 * Must be rendered only while the menu is open (the hook inside is not
 * conditional).
 */
export function MenuSurface({
  x,
  y,
  align = "left",
  className = "",
  children,
}: Props) {
  const { ref, style } = useAnchoredMenu<HTMLDivElement>(x, y, align);
  return (
    <div
      ref={ref}
      style={style}
      className={`fixed z-50 rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-lg ${className}`}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {children}
    </div>
  );
}
