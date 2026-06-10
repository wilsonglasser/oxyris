import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Pencil, Plus, X } from "lucide-react";
import { Terminal, type IDisposable, type ILink } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { openUrl } from "@tauri-apps/plugin-opener";
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
import {
  TERM_FONT_DEFAULT,
  useAppSettingsStore,
} from "~/stores/appSettingsStore.ts";
import { useTerminalDockStore } from "~/stores/terminalDockStore.ts";

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

  // Set once we've made the first-load auto-spawn decision for a session.
  // Crucially armed inside `refresh` (not only when *this* panel auto-spawns)
  // so that if tabs already exist — surviving shells, or ones spawned out of
  // band by an action — closing every tab later does NOT trigger a respawn.
  const autoSpawnedRef = useRef<string | null>(null);

  const spawnNew = useCallback(
    async (command?: string) => {
      try {
        const info = await terminalSpawn({
          session_id: sessionId,
          cols: 80,
          rows: 24,
        });
        setTabs((prev) => [...prev, info]);
        setActiveId(info.id);
        setError(null);
        if (command) {
          await terminalWrite({ id: info.id, data: `${command}\r` });
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [sessionId],
  );

  const refresh = useCallback(async () => {
    try {
      // The dock only hosts auxiliary shells. The pure-mode claude TUI PTY is
      // owned by its own pane (PureClaudePanel) — attaching a second xterm to
      // it here would fight over resize and garble both renders.
      const rows = (await terminalList({ session_id: sessionId })).filter(
        (r) => r.kind !== "claude",
      );
      setTabs(rows);
      setActiveId((cur) => {
        if (cur && rows.some((r) => r.id === cur)) return cur;
        return rows[0]?.id ?? null;
      });
      // First load for this session decides whether to seed a starter shell.
      // Made exactly once (the ref then stays armed), so closing every tab
      // afterwards won't respawn. Skipped when a command request is already
      // queued — that request will create the tab, so an empty one would be a
      // spurious extra.
      if (autoSpawnedRef.current !== sessionId) {
        autoSpawnedRef.current = sessionId;
        const hasPending = useTerminalDockStore
          .getState()
          .requests.some((r) => r.sessionId === sessionId);
        if (rows.length === 0 && !hasPending) {
          void spawnNew();
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [sessionId, spawnNew]);

  // Whenever the session changes, refresh from the backend's source of truth.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Drain queued command requests (PTY actions / auto-run) into real tabs that
  // this dock owns, so they show up immediately instead of only after an
  // unrelated refresh. Arming `autoSpawnedRef` here too keeps the starter-shell
  // logic from also adding an empty tab.
  const dockRequests = useTerminalDockStore((s) => s.requests);
  const takeRequests = useTerminalDockStore((s) => s.take);
  useEffect(() => {
    if (!dockRequests.some((r) => r.sessionId === sessionId)) return;
    autoSpawnedRef.current = sessionId;
    for (const req of takeRequests(sessionId)) {
      void spawnNew(req.command);
    }
  }, [dockRequests, sessionId, takeRequests, spawnNew]);

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
                className={`group relative flex shrink-0 items-center border-r border-neutral-800 text-[11px] transition ${
                  isActive
                    ? "bg-neutral-950 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
              >
                <button
                  type="button"
                  onClick={() => setActiveId(tab.id)}
                  onDoubleClick={() => void renameTab(tab.id, tab.title)}
                  className="flex items-center gap-1.5 py-1.5 pl-2 pr-12"
                  title={tab.cwd}
                >
                  <span className="size-1.5 rounded-full bg-emerald-500/80" />
                  <span className="max-w-[140px] truncate">{tab.title}</span>
                </button>
                <div className="pointer-events-none absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5 opacity-0 transition group-hover:pointer-events-auto group-hover:opacity-100">
                  <button
                    type="button"
                    onClick={() => void renameTab(tab.id, tab.title)}
                    aria-label={t("terminal_rename")}
                    title={t("terminal_rename")}
                    className="flex size-4 items-center justify-center rounded text-neutral-500 transition hover:bg-neutral-700 hover:text-neutral-200"
                  >
                    <Pencil className="size-2.5" strokeWidth={1.75} />
                  </button>
                  <button
                    type="button"
                    onClick={() => closeTab(tab.id)}
                    aria-label={t("terminal_close_tab")}
                    title={t("terminal_close_tab")}
                    className="flex size-4 items-center justify-center rounded text-neutral-500 transition hover:bg-red-950/40 hover:text-red-300"
                  >
                    <X className="size-2.5" strokeWidth={2} />
                  </button>
                </div>
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
  /**
   * Called when an image is pasted (Ctrl/Cmd+V) while the terminal is focused.
   * xterm has no native image-paste path — claude CLI can't ingest clipboard
   * bitmaps directly — so the host saves the blob to a file and injects the
   * resulting `@path` ref into the PTY. Text paste is left to xterm.
   */
  onImagePaste?: (file: File) => void | Promise<void>;
  /** Raw keystroke bytes the user typed into the terminal (mirrors PTY input). */
  onInput?: (data: string) => void;
  /** Fired on each *live* PTY output chunk (not during the attach replay). */
  onOutput?: (data: string) => void;
  /**
   * Ctrl/Cmd+click on a detected file path in the terminal. Receives the raw
   * matched token (may carry a trailing `:line[:col]`); the host resolves it
   * against the PTY's cwd and opens it. When omitted, paths are not linkified.
   */
  onOpenPath?: (rawPath: string) => void;
}

// Path-like tokens: require at least one separator and a trailing extension so
// version strings ("v2.1.150") and prose ("Opus 4.7") aren't linkified. Allows
// an optional drive ("C:\"), leading "./" / "../" / "/", and a ":line[:col]"
// suffix. `g` flag → reset `lastIndex` is implicit since we re-run per line.
const PATH_RE =
  /(?:[A-Za-z]:[\\/])?(?:\.{0,2}[\\/])?(?:[\w.@~+-]+[\\/])+[\w.@+-]+\.[A-Za-z0-9]{1,12}(?::\d+(?::\d+)?)?/g;

// http(s) URLs. Stops at whitespace and quotes/brackets so a URL wrapped in
// parens or quotes in claude's output doesn't drag the delimiter in. A
// trailing run of sentence punctuation (".,;:!?" and closing brackets) is
// trimmed off the match below — those almost always belong to the prose, not
// the URL. Unlike file paths, URLs are linkified in every terminal (dock
// shells included), so this provider is always registered.
const URL_RE = /https?:\/\/[^\s'"<>`]+/g;
const URL_TRAILING_RE = /[.,;:!?)\]}>'"]+$/;

/**
 * Renders one xterm bound to an already-spawned PTY. Stays mounted across
 * tab switches (just toggles `visible`) so scrollback is preserved. Exported
 * so the Pure-mode panel can reuse the exact replay/live/resize plumbing.
 */
export function TerminalView({
  terminalId,
  visible,
  onExit,
  onImagePaste,
  onInput,
  onOutput,
  onOpenPath,
}: ViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // Global terminal zoom. Kept in a ref too so the keydown/wheel handlers wired
  // once at mount read the latest size without rebuilding the terminal.
  const fontSize = useAppSettingsStore((s) => s.terminalFontSize);
  const fontSizeRef = useRef(fontSize);
  // Transient zoom badge (bottom-left). `null` = hidden; set to the new px on
  // change, then cleared after a beat.
  const [zoomBadge, setZoomBadge] = useState<number | null>(null);
  const zoomBadgeTimer = useRef<number | undefined>(undefined);
  // Held in refs so the listeners always see the latest callbacks without
  // rebuilding the whole terminal when their identity changes.
  const onImagePasteRef = useRef(onImagePaste);
  const onInputRef = useRef(onInput);
  const onOutputRef = useRef(onOutput);
  const onOpenPathRef = useRef(onOpenPath);
  useEffect(() => {
    onImagePasteRef.current = onImagePaste;
    onInputRef.current = onInput;
    onOutputRef.current = onOutput;
    onOpenPathRef.current = onOpenPath;
  }, [onImagePaste, onInput, onOutput, onOpenPath]);

  useEffect(() => {
    const mount = containerRef.current;
    if (!mount) return;

    const term = new Terminal({
      fontFamily:
        '"JetBrains Mono", "Cascadia Code", ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace',
      fontSize: fontSizeRef.current,
      theme: {
        background: "#19191c",
        foreground: "#dfe1e5",
        // Muted, thin, non-blinking cursor. The claude TUI repaints its spinner
        // line ~10×/sec and the old blinking blue *block* (#3574f0) got smeared
        // along it as a flickering square — and showed as a fat block at the
        // input tail. A dim bar reads as a caret, not a square, and
        // `cursorInactiveStyle: "none"` removes it entirely while focus is in
        // the composer (the common case during a "thinking" turn).
        cursor: "#5b6270",
        cursorAccent: "#19191c",
      },
      cursorBlink: false,
      cursorStyle: "bar",
      cursorInactiveStyle: "none",
      // Deep scrollback: a `cargo run`/build flood blows past a few thousand
      // lines, and pure-claude renders inline (normal buffer) so this cap is
      // exactly how far back the user can scroll. Kept in step with the backend
      // replay cap (REPLAY_CAP_BYTES) so a tab-switch re-attach doesn't truncate.
      scrollback: 50000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(mount);
    termRef.current = term;
    fitRef.current = fit;

    // GPU renderer. The claude TUI repaints its spinner/status block ~10×/sec;
    // xterm's default DOM renderer tears colored spans on those rapid redraws
    // (visible flicker on the blue "thinking" text). WebGL repaints atomically,
    // killing the flicker. Guarded: if WebGL is unavailable (or the GL context
    // is later lost) we dispose the addon and fall back to the DOM renderer.
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      /* no WebGL — DOM renderer stays in place */
    }

    // Linkify file paths so Ctrl/Cmd+click opens them (the host resolves the
    // token against the PTY's cwd). Decorations are only emitted when a handler
    // is wired, so plain dock terminals show no spurious underlines.
    const linkProvider = term.registerLinkProvider({
      provideLinks(bufferLineNumber, callback) {
        if (!onOpenPathRef.current) return callback(undefined);
        const line = term.buffer.active.getLine(bufferLineNumber - 1);
        if (!line) return callback(undefined);
        const text = line.translateToString(true);
        const links: ILink[] = [];
        PATH_RE.lastIndex = 0;
        for (let m = PATH_RE.exec(text); m; m = PATH_RE.exec(text)) {
          const start = m.index + 1;
          const raw = m[0];
          links.push({
            text: raw,
            range: {
              start: { x: start, y: bufferLineNumber },
              end: { x: start + raw.length - 1, y: bufferLineNumber },
            },
            decorations: { pointerCursor: true, underline: true },
            activate: (event: MouseEvent, token: string) => {
              if (!(event.ctrlKey || event.metaKey)) return;
              onOpenPathRef.current?.(token);
            },
          });
        }
        callback(links.length ? links : undefined);
      },
    });

    // Linkify http(s) URLs so Ctrl/Cmd+click opens them in the system browser
    // (via the opener plugin — it escapes the URL, so query strings with `&`
    // are safe). Always on, independent of the file-path provider above.
    const urlLinkProvider = term.registerLinkProvider({
      provideLinks(bufferLineNumber, callback) {
        const line = term.buffer.active.getLine(bufferLineNumber - 1);
        if (!line) return callback(undefined);
        const text = line.translateToString(true);
        const links: ILink[] = [];
        URL_RE.lastIndex = 0;
        for (let m = URL_RE.exec(text); m; m = URL_RE.exec(text)) {
          const url = m[0].replace(URL_TRAILING_RE, "");
          if (!url) continue;
          const start = m.index + 1;
          links.push({
            text: url,
            range: {
              start: { x: start, y: bufferLineNumber },
              end: { x: start + url.length - 1, y: bufferLineNumber },
            },
            decorations: { pointerCursor: true, underline: true },
            activate: (event: MouseEvent, token: string) => {
              if (!(event.ctrlKey || event.metaKey)) return;
              void openUrl(token).catch(() => {});
            },
          });
        }
        callback(links.length ? links : undefined);
      },
    });

    // Route a pasted image to the host exactly once. Both the Ctrl+Shift+V
    // keydown (async clipboard read, below) and `onPasteCapture` (native paste)
    // can fire for the same paste, so debounce: the second call within the
    // window is dropped.
    let lastImageRoute = 0;
    // Ctrl+C burst tracking. A lone Ctrl+C copies the selection (so it can't be
    // fat-fingered into interrupting claude); pressing it again within the
    // window forwards SIGINT (\x03) to the PTY, and every further press in the
    // burst keeps forwarding. Gaps longer than the window reset to "copy".
    const CTRL_C_BURST_MS = 500;
    let lastCtrlC = 0;
    // Last non-empty selection. The claude TUI repaints its spinner/status line
    // ~10×/sec; those in-place rewrites make xterm drop the active selection, so
    // by the time the user reaches for Ctrl+C / Ctrl+Shift+C `getSelection()` is
    // often already "" and the copy was a silent no-op. Cache the selection the
    // instant it's made (mouse-up) and copy current-or-cached so a repaint
    // between select and keypress can't eat it.
    let lastSelection = "";
    const selDisposable = term.onSelectionChange(() => {
      const s = term.getSelection();
      if (s) lastSelection = s;
    });
    const routeImage = (file: File) => {
      const now = Date.now();
      if (now - lastImageRoute < 500) return;
      lastImageRoute = now;
      void onImagePasteRef.current?.(file);
    };

    // Ctrl+C in a terminal is SIGINT (interrupts claude), so it can't be copy.
    // Bind the conventional terminal shortcuts instead: Ctrl+Shift+C copies the
    // selection; Ctrl+V / Ctrl+Shift+V pastes (image → @path ref, else text).
    // Returning false stops xterm from forwarding the keystroke to the PTY.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      // Zoom: Ctrl +/- steps the shared font size, Ctrl+0 resets. Shift is
      // ignored on Equal/Minus so the "+" key (Shift+Equal on most layouts)
      // also zooms in. Swallow the keystroke so it never reaches the PTY.
      if (e.ctrlKey && !e.altKey && !e.metaKey) {
        const store = useAppSettingsStore.getState();
        if (e.code === "Equal" || e.code === "NumpadAdd") {
          store.bumpTerminalFontSize(1);
          return false;
        }
        if (e.code === "Minus" || e.code === "NumpadSubtract") {
          store.bumpTerminalFontSize(-1);
          return false;
        }
        if (e.code === "Digit0" || e.code === "Numpad0") {
          store.resetTerminalFontSize();
          return false;
        }
        // Paste on Ctrl+V *and* Ctrl+Shift+V. Plain Ctrl+V is what users
        // expect, but if it reaches the PTY it's forwarded as \x16 and the
        // browser's native paste never fires — so it was previously a no-op
        // and only the Shift variant worked. Handle it here, before the
        // ctrl+shift `combo` gate below.
        if (e.code === "KeyV") {
          // Images only. A screenshot lives on the clipboard as a bitmap,
          // which WebView2's native `paste` event does NOT expose as a file
          // item — only the async Clipboard API reconstitutes it as image/png
          // — so read it here and route to the host (deduped via `routeImage`
          // against any native paste that also catches a real image *file*).
          // We deliberately do NOT write text here: returning false stops
          // xterm forwarding the keystroke to the PTY but leaves the native
          // paste untouched, so xterm pastes the text once. The old code
          // wrote text too and the keydown preventDefault does NOT reliably
          // suppress the native paste in WebView2, so text landed twice.
          void (async () => {
            try {
              if (!navigator.clipboard.read || !onImagePasteRef.current) return;
              for (const ci of await navigator.clipboard.read()) {
                const imgType = ci.types.find((tp) => tp.startsWith("image/"));
                if (!imgType) continue;
                const blob = await ci.getType(imgType);
                const ext = imgType.split("/")[1] || "png";
                routeImage(new File([blob], `pasted.${ext}`, { type: imgType }));
                return;
              }
            } catch {
              /* no image / no permission — native paste handles text */
            }
          })();
          return false;
        }
        // Plain Ctrl+C: first press copies, consecutive presses interrupt.
        // (Ctrl+Shift+C below always copies, regardless of burst state.)
        if (e.code === "KeyC" && !e.shiftKey) {
          const now = Date.now();
          const consecutive = now - lastCtrlC < CTRL_C_BURST_MS;
          lastCtrlC = now;
          if (consecutive) {
            // Forward SIGINT to the PTY ourselves and swallow the keystroke so
            // xterm doesn't also send a second \x03.
            void terminalWrite({ id: terminalId, data: "\x03" });
            return false;
          }
          // First press in a burst: copy the selection, don't interrupt.
          const sel = term.getSelection() || lastSelection;
          if (sel) void navigator.clipboard.writeText(sel).catch(() => {});
          return false;
        }
      }
      const combo = e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
      if (!combo) return true;
      if (e.code === "KeyC") {
        const sel = term.getSelection() || lastSelection;
        if (sel) void navigator.clipboard.writeText(sel).catch(() => {});
        return false;
      }
      return true;
    });

    // Intercept image paste on xterm's hidden helper textarea. Capture phase +
    // stopPropagation so xterm's own (text-only) paste handler never sees it.
    // Non-image clipboards fall through to xterm untouched.
    const textarea = term.textarea;
    const onPasteCapture = (e: ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      for (let i = 0; i < items.length; i += 1) {
        const item = items[i];
        if (item && item.kind === "file" && item.type.startsWith("image/")) {
          const file = item.getAsFile();
          if (!file) continue;
          e.preventDefault();
          e.stopPropagation();
          routeImage(file);
          return;
        }
      }
    };
    textarea?.addEventListener("paste", onPasteCapture, { capture: true });

    // Ctrl+scroll zooms instead of scrolling the buffer. Capture phase +
    // preventDefault so xterm's wheel-scroll never sees it. Non-passive so
    // preventDefault is honored. One notch = one px step (sign-only — wheel
    // deltas vary wildly across devices, so we ignore magnitude).
    const onWheelZoom = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      useAppSettingsStore.getState().bumpTerminalFontSize(e.deltaY < 0 ? 1 : -1);
    };
    mount.addEventListener("wheel", onWheelZoom, {
      capture: true,
      passive: false,
    });

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
      onOutputRef.current?.(data);
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
        onInputRef.current?.(data);
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
      textarea?.removeEventListener("paste", onPasteCapture, { capture: true });
      mount.removeEventListener("wheel", onWheelZoom, { capture: true });
      ro.disconnect();
      try {
        dataHandlers.forEach((d) => d.dispose());
      } catch {
        /* noop */
      }
      if (unlistenOut) unlistenOut();
      if (unlistenExit) unlistenExit();
      try {
        linkProvider.dispose();
        urlLinkProvider.dispose();
        selDisposable.dispose();
      } catch {
        /* noop */
      }
      try {
        term.dispose();
      } catch {
        /* noop */
      }
      termRef.current = null;
      fitRef.current = null;
    };
  }, [terminalId, onExit]);

  // Apply zoom changes to the live terminal: resize the font, reflow the PTY to
  // the new cols/rows, and flash the badge. Skips the initial mount (the badge
  // shouldn't pop just from opening a terminal). Re-fit is deferred a frame so
  // xterm picks up the new char metrics before measuring.
  const fontMountedRef = useRef(false);
  useEffect(() => {
    fontSizeRef.current = fontSize;
    const term = termRef.current;
    if (!term) return;
    term.options.fontSize = fontSize;
    const id = window.setTimeout(() => {
      try {
        fitRef.current?.fit();
      } catch {
        /* noop */
      }
    }, 0);
    if (!fontMountedRef.current) {
      fontMountedRef.current = true;
      return () => window.clearTimeout(id);
    }
    setZoomBadge(fontSize);
    window.clearTimeout(zoomBadgeTimer.current);
    zoomBadgeTimer.current = window.setTimeout(() => setZoomBadge(null), 1200);
    return () => window.clearTimeout(id);
  }, [fontSize]);

  useEffect(() => () => window.clearTimeout(zoomBadgeTimer.current), []);

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
    <div className={`absolute inset-0 ${visible ? "" : "invisible"}`}>
      <div ref={containerRef} className="absolute inset-0 overflow-hidden p-2" />
      {zoomBadge !== null && (
        <div
          className="pointer-events-none absolute bottom-2 left-2 z-10 rounded bg-neutral-900/90 px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-neutral-300 ring-1 ring-neutral-700"
          aria-live="polite"
        >
          {Math.round((zoomBadge / TERM_FONT_DEFAULT) * 100)}% · {zoomBadge}px
        </div>
      )}
    </div>
  );
}
