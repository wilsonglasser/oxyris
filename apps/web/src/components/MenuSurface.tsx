import { useEffect, type ReactNode } from "react";

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

/** Hairline divider between groups of `MenuItem`s. */
export function MenuSeparator() {
  return <div className="my-1 border-t border-neutral-800" />;
}

/** One row inside a `MenuSurface`. `danger` tints destructive actions red. */
export function MenuItem({
  icon,
  label,
  onClick,
  danger,
  disabled,
}: {
  icon: ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left disabled:cursor-default disabled:opacity-40 ${
        danger
          ? "text-red-300 enabled:hover:bg-red-900/30"
          : "text-neutral-200 enabled:hover:bg-neutral-900"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

/**
 * Closes an open context menu on an outside click or Escape. `MenuSurface`
 * stops mousedown propagation, so clicks inside the menu don't trigger it.
 */
export function useMenuDismiss(open: boolean, close: () => void) {
  useEffect(() => {
    if (!open) return;
    const onDown = () => close();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
}
