import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";

/**
 * Frameless-window title bar. The central span is a Tauri drag region so
 * the user can move the window by grabbing anywhere that isn't a button.
 * Min/maximize/close use `getCurrentWindow()` APIs directly — no custom
 * Tauri commands needed.
 */
export function TitleBar({
  center,
  actions,
}: {
  center?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    void win.isMaximized().then(setMaximized);
    const unlisten = win.onResized(() => {
      void win.isMaximized().then(setMaximized);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const onMin = () => void getCurrentWindow().minimize();
  const onMax = () => void getCurrentWindow().toggleMaximize();
  const onClose = () => void getCurrentWindow().close();

  return (
    <header
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-center border-b border-neutral-800 bg-neutral-900 pl-3 text-[11px] text-neutral-300"
    >
      <div
        data-tauri-drag-region
        className="flex items-center gap-1.5"
      >
        <img
          data-tauri-drag-region
          src="/appicon.png"
          alt=""
          aria-hidden
          className="size-4 rounded-[3px]"
          draggable={false}
        />
        <span
          data-tauri-drag-region
          className="font-semibold tracking-tight text-neutral-200"
        >
          Oxyris
        </span>
        <span
          data-tauri-drag-region
          className="rounded-sm bg-neutral-800 px-1 py-[1px] text-[9px] font-medium uppercase tracking-wider text-neutral-500"
        >
          alpha
        </span>
      </div>

      <div
        data-tauri-drag-region
        className="flex flex-1 items-center justify-center gap-2 px-4"
      >
        {center}
      </div>

      <div className="flex items-center gap-1 pr-1">{actions}</div>

      <div className="flex shrink-0">
        <WindowButton onClick={onMin} aria-label="minimize" hoverBg="hover:bg-neutral-800">
          <Minus className="size-3.5" strokeWidth={1.5} />
        </WindowButton>
        <WindowButton onClick={onMax} aria-label="maximize" hoverBg="hover:bg-neutral-800">
          {maximized ? (
            <Copy className="size-3.5 -scale-x-100" strokeWidth={1.5} />
          ) : (
            <Square className="size-3.5" strokeWidth={1.5} />
          )}
        </WindowButton>
        <WindowButton onClick={onClose} aria-label="close" hoverBg="hover:bg-red-600">
          <X className="size-3.5" strokeWidth={1.5} />
        </WindowButton>
      </div>
    </header>
  );
}

function WindowButton({
  onClick,
  children,
  hoverBg,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { hoverBg: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex h-9 w-11 items-center justify-center text-neutral-400 transition ${hoverBg} hover:text-neutral-100`}
      {...rest}
    >
      {children}
    </button>
  );
}
