import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Pencil, Plus, X } from "lucide-react";
import { Terminal, type IDisposable } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import {
  type TerminalInfo,
  onTerminalExit,
  onTerminalOutput,
  terminalAttach,
  terminalKill,
  terminalList,
  terminalRename,
  terminalResize,
  terminalSpawn,
  terminalWrite,
} from "~/ipc/terminal.ts";

interface DockProps {
  sessionId: string;
  onClose?: () => void;
}

/**
 * Multi-terminal dock for the active session. Each tab owns one PTY; PTYs
 * survive tab switches and stay running until the user explicitly kills
 * them or closes the session. Closing the dock (X) does not kill PTYs.
 */
export function TerminalPanel({ sessionId, onClose }: DockProps) {
  const { t } = useTranslation("chat");
  const [tabs, setTabs] = useState<TerminalInfo[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const rows = await terminalList({ session_id: sessionId });
      setTabs(rows);
      setActiveId((cur) => {
        if (cur && rows.some((r) => r.id === cur)) return cur;
        return rows[0]?.id ?? null;
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [sessionId]);

  // Whenever the session changes, refresh from the backend's source of truth.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const spawnNew = useCallback(async () => {
    try {
      const info = await terminalSpawn({
        session_id: sessionId,
        cols: 80,
        rows: 24,
      });
      setTabs((prev) => [...prev, info]);
      setActiveId(info.id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [sessionId]);

  // Auto-spawn the first terminal if the dock opens with none.
  const autoSpawnedRef = useRef<string | null>(null);
  useEffect(() => {
    if (tabs.length === 0 && autoSpawnedRef.current !== sessionId) {
      autoSpawnedRef.current = sessionId;
      void spawnNew();
    }
  }, [tabs.length, sessionId, spawnNew]);

  const closeTab = (id: string) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      setActiveId((cur) => {
        if (cur !== id) return cur;
        return next[0]?.id ?? null;
      });
      return next;
    });
    void terminalKill({ id }).catch(() => {});
  };

  const renameTab = async (id: string, current: string) => {
    const next = window.prompt(t("terminal_rename_prompt"), current);
    if (!next) return;
    const trimmed = next.trim();
    if (!trimmed || trimmed === current) return;
    try {
      await terminalRename({ id, title: trimmed });
      setTabs((prev) =>
        prev.map((tab) => (tab.id === id ? { ...tab, title: trimmed } : tab)),
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section className="flex h-full min-h-0 flex-col border-t border-neutral-800 bg-neutral-950">
      <header className="flex items-stretch border-b border-neutral-800 bg-neutral-900">
        <div className="flex flex-1 items-stretch overflow-x-auto">
          {tabs.map((tab) => {
            const isActive = tab.id === activeId;
            return (
              <div
                key={tab.id}
                className={`group flex shrink-0 items-center gap-1 border-r border-neutral-800 px-2 py-1.5 text-[11px] transition ${
                  isActive
                    ? "bg-neutral-950 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
              >
                <button
                  type="button"
                  onClick={() => setActiveId(tab.id)}
                  onDoubleClick={() => void renameTab(tab.id, tab.title)}
                  className="flex items-center gap-1.5"
                  title={tab.cwd}
                >
                  <span className="size-1.5 rounded-full bg-emerald-500/80" />
                  <span className="max-w-[140px] truncate">{tab.title}</span>
                </button>
                <button
                  type="button"
                  onClick={() => void renameTab(tab.id, tab.title)}
                  aria-label={t("terminal_rename")}
                  title={t("terminal_rename")}
                  className="flex size-3.5 items-center justify-center rounded text-neutral-500 opacity-0 transition hover:bg-neutral-700 hover:text-neutral-200 group-hover:opacity-100"
                >
                  <Pencil className="size-2.5" strokeWidth={1.75} />
                </button>
                <button
                  type="button"
                  onClick={() => closeTab(tab.id)}
                  aria-label={t("terminal_close_tab")}
                  title={t("terminal_close_tab")}
                  className="flex size-3.5 items-center justify-center rounded text-neutral-500 opacity-0 transition hover:bg-red-950/40 hover:text-red-300 group-hover:opacity-100"
                >
                  <X className="size-2.5" strokeWidth={2} />
                </button>
              </div>
            );
          })}
          <button
            type="button"
            onClick={() => void spawnNew()}
            aria-label={t("terminal_new_tab")}
            title={t("terminal_new_tab")}
            className="flex size-7 items-center justify-center self-center text-neutral-500 hover:text-neutral-200"
          >
            <Plus className="size-3.5" strokeWidth={1.75} />
          </button>
        </div>
        {onClose && (
          <button
            type="button"
            onClick={onClose}
            aria-label="hide terminal"
            title={t("terminal_hide")}
            className="flex size-9 items-center justify-center text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
          >
            <X className="size-3.5" strokeWidth={1.75} />
          </button>
        )}
      </header>
      {error && (
        <p className="border-b border-red-900/50 bg-red-950/30 px-3 py-1.5 text-[11px] text-red-200">
          {t("terminal_error", { message: error })}
        </p>
      )}
      <div className="relative min-h-0 flex-1">
        {tabs.map((tab) => (
          <TerminalView
            key={tab.id}
            terminalId={tab.id}
            visible={tab.id === activeId}
            onExit={refresh}
          />
        ))}
        {tabs.length === 0 && !error && (
          <div className="flex h-full items-center justify-center text-[11px] text-neutral-500">
            {t("terminal_empty_dock")}
          </div>
        )}
      </div>
    </section>
  );
}

interface ViewProps {
  terminalId: string;
  visible: boolean;
  onExit?: () => void;
}

/**
 * Renders one xterm bound to an already-spawned PTY. Stays mounted across
 * tab switches (just toggles `visible`) so scrollback is preserved. Exported
 * so the Pure-mode panel can reuse the exact replay/live/resize plumbing.
 */
export function TerminalView({ terminalId, visible, onExit }: ViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    const mount = containerRef.current;
    if (!mount) return;

    const term = new Terminal({
      fontFamily:
        '"JetBrains Mono", "Cascadia Code", ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace',
      fontSize: 12,
      theme: { background: "#19191c", foreground: "#dfe1e5", cursor: "#3574f0" },
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(mount);
    termRef.current = term;
    fitRef.current = fit;

    const safeFit = () => {
      try {
        fit.fit();
      } catch {
        /* noop */
      }
    };
    const fitTimer = window.setTimeout(safeFit, 30);
    window.addEventListener("resize", safeFit);
    // Re-fit when the container itself changes size (not just the window) —
    // e.g. a composer growing with attachment chips above the terminal. Without
    // this the fixed-size canvas overflows and covers sibling content until the
    // next window resize.
    const ro = new ResizeObserver(() => safeFit());
    ro.observe(mount);

    let unlistenOut: (() => void) | null = null;
    let unlistenExit: (() => void) | null = null;
    let cancelled = false;
    const dataHandlers: IDisposable[] = [];

    const safeWrite = (chunk: string) => {
      try {
        term.write(chunk);
      } catch {
        /* noop */
      }
    };

    // Replay-then-live: until `terminalAttach` returns the snapshot of bytes
    // already emitted, queue live events. After replay, drop anything whose
    // `seq <= lastSeq` (already in the snapshot) and write the rest. This
    // closes the race where the backend reader emits before `listen()` has
    // finished registering for newly-spawned tabs.
    let attached = false;
    let lastSeq = 0;
    const pending: { seq: number; data: string }[] = [];

    void onTerminalOutput(terminalId, (seq, data) => {
      if (!attached) {
        pending.push({ seq, data });
        return;
      }
      if (seq <= lastSeq) return;
      lastSeq = seq;
      safeWrite(data);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenOut = fn;
    });

    void terminalAttach({ id: terminalId })
      .then((snap) => {
        if (cancelled) return;
        if (snap.data) safeWrite(snap.data);
        lastSeq = snap.last_seq;
        for (const ev of pending) {
          if (ev.seq <= lastSeq) continue;
          lastSeq = ev.seq;
          safeWrite(ev.data);
        }
        pending.length = 0;
        attached = true;
      })
      .catch(() => {
        // Snapshot failed (terminal already gone, etc) — fall back to live
        // stream only and accept any early bytes were lost.
        attached = true;
      });

    void onTerminalExit(terminalId, () => {
      safeWrite("\r\n\x1b[2m[process exited]\x1b[0m\r\n");
      onExit?.();
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenExit = fn;
    });

    dataHandlers.push(
      term.onData((data) => {
        void terminalWrite({ id: terminalId, data }).catch(() => {});
      }),
    );

    let lastSize = { cols: term.cols || 80, rows: term.rows || 24 };
    let resizeTimer: number | undefined;
    dataHandlers.push(
      term.onResize(({ cols, rows }) => {
        if (cols === lastSize.cols && rows === lastSize.rows) return;
        lastSize = { cols, rows };
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => {
          void terminalResize({ id: terminalId, cols, rows }).catch(() => {});
        }, 80);
      }),
    );

    return () => {
      cancelled = true;
      window.clearTimeout(fitTimer);
      window.clearTimeout(resizeTimer);
      window.removeEventListener("resize", safeFit);
      ro.disconnect();
      try {
        dataHandlers.forEach((d) => d.dispose());
      } catch {
        /* noop */
      }
      if (unlistenOut) unlistenOut();
      if (unlistenExit) unlistenExit();
      try {
        term.dispose();
      } catch {
        /* noop */
      }
      termRef.current = null;
      fitRef.current = null;
    };
  }, [terminalId, onExit]);

  // Re-fit when becoming visible — xterm needs a relayout pass.
  useEffect(() => {
    if (!visible) return;
    const id = window.setTimeout(() => {
      try {
        fitRef.current?.fit();
      } catch {
        /* noop */
      }
    }, 30);
    return () => window.clearTimeout(id);
  }, [visible]);

  return (
    <div
      ref={containerRef}
      className={`absolute inset-0 p-2 ${visible ? "" : "invisible"}`}
    />
  );
}
