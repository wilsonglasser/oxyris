import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight, ExternalLink, Save, X } from "lucide-react";
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  rectangularSelection,
  crosshairCursor,
} from "@codemirror/view";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from "@codemirror/commands";
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
} from "@codemirror/language";
import {
  gotoLine,
  highlightSelectionMatches,
  search,
  searchKeymap,
} from "@codemirror/search";
import { islandDark } from "~/lib/codemirror-theme.ts";
import {
  type LspContext,
  lspExtensions,
  refreshLspDiagnostics,
  runLspFormat,
} from "~/lib/codemirror-lsp.ts";
import { lspDidClose, lspDidSave } from "~/ipc/lsp.ts";
import { languageForPath } from "~/lib/codemirror-language.ts";
import { Eye, FileText } from "lucide-react";
import { fsExternalEditors, fsOpenExternal } from "~/ipc/fs.ts";
import { MenuSurface } from "~/components/MenuSurface.tsx";
import {
  scopeKey,
  useFileEditorStore,
  type Tab,
} from "~/stores/fileEditorStore.ts";
import {
  ImagePreview,
  MarkdownPreview,
  PdfPreview,
} from "~/components/FilePreview.tsx";

interface Props {
  projectId: string;
  worktreeId: string;
}

// Stable empty references — see FileTreePanel for why.
const EMPTY_ORDER: string[] = [];
const EMPTY_TABS: Record<string, Tab> = {};

export function FileEditorTabs({ projectId, worktreeId }: Props) {
  const { t } = useTranslation("files");
  const key = scopeKey(projectId, worktreeId);
  const order = useFileEditorStore((s) => s.openOrder[key] ?? EMPTY_ORDER);
  const tabs = useFileEditorStore((s) => s.tabs[key] ?? EMPTY_TABS);
  const active = useFileEditorStore((s) => s.active[key] ?? null);
  const setActive = useFileEditorStore((s) => s.setActive);
  const closeTab = useFileEditorStore((s) => s.closeTab);
  const closeOthers = useFileEditorStore((s) => s.closeOthers);
  const closeAll = useFileEditorStore((s) => s.closeAll);
  const openFile = useFileEditorStore((s) => s.openFile);

  const activeTab = active ? tabs[active] : null;

  // After persist rehydrate, `openOrder` + `active` come back but the `tabs`
  // map (which holds buffers) is intentionally dropped to avoid stale data.
  // Re-load whichever tab the user had focused; the rest stay as ghost tabs
  // until clicked.
  useEffect(() => {
    if (active && !tabs[active]) {
      void openFile(projectId, worktreeId, active);
    }
  }, [active, tabs, projectId, worktreeId, openFile]);

  const tabStripRef = useRef<HTMLDivElement | null>(null);
  const [overflow, setOverflow] = useState(false);
  const [menu, setMenu] = useState<
    { x: number; y: number; relPath: string } | null
  >(null);

  // Detect when the tab strip overflows so we know whether to show nav arrows.
  // Re-checks on tab add/remove and window resize.
  useEffect(() => {
    const el = tabStripRef.current;
    if (!el) return;
    const check = () => setOverflow(el.scrollWidth > el.clientWidth);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [order.length]);

  // Keep the active tab visible when the user picks one off-screen via the
  // tree (the tab won't scroll into view on its own).
  useEffect(() => {
    if (!active || !tabStripRef.current) return;
    const el = tabStripRef.current.querySelector<HTMLElement>(
      `[data-tab-rel="${cssEscape(active)}"]`,
    );
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [active]);

  // Dismiss context menu on outside click / escape.
  useEffect(() => {
    if (!menu) return;
    const onDown = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  /** Paths with unsaved edits among `paths`. Ghost tabs (not loaded yet) hold
   *  no buffer, so they can never be dirty. */
  const dirtyAmong = (paths: string[]) =>
    paths.filter((p) => {
      const tab = tabs[p];
      return !!tab && tab.buffer !== tab.baseContent;
    });

  /** Closing drops the buffer, so ask before discarding unsaved edits. Matches
   *  the rest of the app, which uses `window.confirm` for destructive steps. */
  const confirmDiscard = (paths: string[]) => {
    const dirtyPaths = dirtyAmong(paths);
    if (dirtyPaths.length === 0) return true;
    return window.confirm(
      t("confirm_close_dirty", {
        count: dirtyPaths.length,
        files: dirtyPaths.map(basename).join(", "),
      }),
    );
  };

  /** Let the language server drop the closed files' buffers — otherwise it
   *  keeps analysing text nobody is looking at. Fire-and-forget: a file with
   *  no server (or no server running) rejects and there is nothing to undo. */
  const releaseFromLsp = (paths: string[]) => {
    for (const relPath of paths) {
      lspDidClose({ projectId, worktreeId, relPath }).catch(() => {});
    }
  };

  const scrollBy = (delta: number) => {
    tabStripRef.current?.scrollBy({ left: delta, behavior: "smooth" });
  };

  if (order.length === 0) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-neutral-500">
        {t("no_open_files")}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col bg-neutral-950">
      <div className="flex h-8 shrink-0 items-stretch border-b border-neutral-800">
        {overflow && (
          <button
            type="button"
            onClick={() => scrollBy(-200)}
            className="shrink-0 border-r border-neutral-800 px-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
            aria-label={t("scroll_tabs_left")}
          >
            <ChevronLeft size={14} />
          </button>
        )}
        <div
          ref={tabStripRef}
          role="tablist"
          className="flex min-w-0 flex-1 items-stretch overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          {order.map((relPath) => {
            // After a persist-restored boot, `tab` may be undefined for
            // not-yet-clicked entries — render a ghost label so the strip
            // still reflects the saved layout. Clicking promotes it via
            // `openFile`.
            const tab = tabs[relPath];
            const dirty = !!tab && tab.buffer !== tab.baseContent;
            const isActive = active === relPath;
            return (
              <div
                key={relPath}
                role="tab"
                aria-selected={isActive}
                data-tab-rel={relPath}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, relPath });
                }}
                className={`flex shrink-0 items-center gap-1 border-r border-neutral-800 pl-3 pr-1 text-[11px] ${
                  isActive
                    ? "bg-neutral-900 text-neutral-100"
                    : tab
                      ? "text-neutral-400 hover:bg-neutral-900/50 hover:text-neutral-200"
                      : "text-neutral-500 italic hover:bg-neutral-900/50 hover:text-neutral-300"
                }`}
              >
                <button
                  type="button"
                  onClick={() => {
                    setActive(projectId, worktreeId, relPath);
                    if (!tab) {
                      void openFile(projectId, worktreeId, relPath);
                    }
                  }}
                  className="max-w-[180px] truncate text-left"
                  title={relPath}
                >
                  {basename(relPath)}
                  {dirty && (
                    <span className="ml-1 text-neutral-500" aria-hidden>
                      •
                    </span>
                  )}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    if (!confirmDiscard([relPath])) return;
                    closeTab(projectId, worktreeId, relPath);
                    releaseFromLsp([relPath]);
                  }}
                  className="rounded p-0.5 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
                  aria-label={t("close_tab")}
                >
                  <X size={12} />
                </button>
              </div>
            );
          })}
        </div>
        {overflow && (
          <button
            type="button"
            onClick={() => scrollBy(200)}
            className="shrink-0 border-l border-neutral-800 px-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
            aria-label={t("scroll_tabs_right")}
          >
            <ChevronRight size={14} />
          </button>
        )}
      </div>

      {menu && (
        <MenuSurface x={menu.x} y={menu.y} className="min-w-[140px]">
          <button
            type="button"
            onClick={() => {
              setMenu(null);
              if (!confirmDiscard([menu.relPath])) return;
              closeTab(projectId, worktreeId, menu.relPath);
              releaseFromLsp([menu.relPath]);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close")}
          </button>
          <button
            type="button"
            onClick={() => {
              setMenu(null);
              const others = order.filter((p) => p !== menu.relPath);
              if (!confirmDiscard(others)) return;
              closeOthers(projectId, worktreeId, menu.relPath);
              releaseFromLsp(others);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close_others")}
          </button>
          <button
            type="button"
            onClick={() => {
              setMenu(null);
              if (!confirmDiscard(order)) return;
              closeAll(projectId, worktreeId);
              releaseFromLsp(order);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close_all")}
          </button>
        </MenuSurface>
      )}

      {activeTab && (
        <EditorPane
          // Including `loading` in the key forces a remount when the file
          // content arrives — guarantees the empty-doc skeleton is replaced
          // by the real content even if the buffer-sync effect missed it.
          // `projectId` MUST be part of the key: every project's primary
          // checkout shares the same nil-UUID worktreeId sentinel, so two
          // projects' same-relPath file (e.g. ".env") would otherwise collide
          // and React would reuse this instance across a project switch —
          // leaving the editor's keymap/updateListener closures bound to the
          // previous project and routing saves to the wrong file.
          key={`${projectId}::${worktreeId}::${activeTab.relPath}::${
            activeTab.loading ? "load" : "ready"
          }`}
          projectId={projectId}
          worktreeId={worktreeId}
          tab={activeTab}
        />
      )}
    </div>
  );
}

/**
 * Cursor + scroll position per `<scope>::<relPath>`. The editor pane remounts
 * on every tab switch (the key includes the path), so without this the caret
 * and scroll snap back to the top of the file each time. Module-level and
 * deliberately not persisted: an offset saved across a restart would point
 * into a file that may have changed underneath.
 */
const viewStateCache = new Map<
  string,
  { anchor: number; head: number; scrollTop: number }
>();

interface EditorPaneProps {
  projectId: string;
  worktreeId: string;
  tab: Tab;
}

export function EditorPane({ projectId, worktreeId, tab }: EditorPaneProps) {
  const { t } = useTranslation("files");
  const setBuffer = useFileEditorStore((s) => s.setBuffer);
  const saveTab = useFileEditorStore((s) => s.saveTab);
  const loadFullFile = useFileEditorStore((s) => s.loadFullFile);
  const reloadFromDisk = useFileEditorStore((s) => s.reloadFromDisk);
  const keepLocalChanges = useFileEditorStore((s) => s.keepLocalChanges);
  const reveal = useFileEditorStore(
    (s) => s.reveal[scopeKey(projectId, worktreeId)] ?? null,
  );
  const consumeReveal = useFileEditorStore((s) => s.consumeReveal);
  const openFileAt = useFileEditorStore((s) => s.openFileAt);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const diagTimer = useRef<number | null>(null);
  const dirty = tab.buffer !== tab.baseContent;

  /** Save, then tell the language server the file was saved — that is what
   *  triggers its check layer (rust-analyzer runs `cargo check` on didSave),
   *  which the per-keystroke sync deliberately does not. Best-effort: a file
   *  with no server rejects it and nothing is lost. */
  const save = async () => {
    await saveTab(projectId, worktreeId, tab.relPath);
    const text = viewRef.current?.state.doc.toString() ?? tab.buffer;
    lspDidSave({ projectId, worktreeId, relPath: tab.relPath, text }).catch(
      () => {},
    );
  };

  const [editors, setEditors] = useState<{ id: string; label: string; available: boolean }[]>(
    [],
  );
  const [editorPickerOpen, setEditorPickerOpen] = useState(false);
  // Markdown defaults to preview; image/pdf are always preview-only.
  const [mdView, setMdView] = useState<"raw" | "preview">(
    tab.kind === "markdown" ? "preview" : "raw",
  );
  const isPreviewOnly = tab.kind === "image" || tab.kind === "pdf";
  const showEditor = !isPreviewOnly && !(tab.kind === "markdown" && mdView === "preview");

  // Lazily fetch the external-editor list once per tab mount; cheap enough.
  useEffect(() => {
    let cancelled = false;
    void fsExternalEditors().then((list) => {
      if (!cancelled) setEditors(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Editors actually installed (plus the always-available system default).
  // Missing ones are never surfaced.
  const availableEditors = useMemo(
    () => editors.filter((e) => e.available),
    [editors],
  );
  const openWith = (id: string) => {
    setEditorPickerOpen(false);
    void fsOpenExternal({ projectId, worktreeId, relPath: tab.relPath, editor: id });
  };

  // Build the CodeMirror view once per (worktreeId, relPath); buffer updates
  // flow back into the store via the updateListener below.
  useEffect(() => {
    if (!containerRef.current) return;
    if (!showEditor) return;
    // Diagnostics are requested on a debounce: the server needs a beat to
    // re-analyse after an edit, and each request carries the whole buffer.
    const scheduleDiagnostics = () => {
      if (diagTimer.current !== null) window.clearTimeout(diagTimer.current);
      diagTimer.current = window.setTimeout(() => {
        const v = viewRef.current;
        if (v) void refreshLspDiagnostics(v, lspCtx);
      }, 500);
    };
    const lspCtx: LspContext = {
      projectId,
      worktreeId,
      relPath: tab.relPath,
      openLocation: (loc) => {
        if (loc.rel_path) {
          void openFileAt(projectId, worktreeId, loc.rel_path, loc.line + 1);
        }
      },
    };
    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightActiveLine(),
      foldGutter(),
      // Undo/redo. CodeMirror ships no history by default when extensions are
      // composed by hand, and the WebView's native undo can't touch the
      // editor's managed DOM — without this Ctrl+Z simply does nothing.
      history(),
      // Multi-cursor: `drawSelection` renders the extra carets that
      // rectangularSelection (Alt+drag) and Ctrl+click create.
      drawSelection(),
      rectangularSelection(),
      crosshairCursor(),
      bracketMatching(),
      closeBrackets(),
      indentOnInput(),
      autocompletion(),
      indentUnit.of("  "),
      EditorState.tabSize.of(2),
      // A truncated read holds only the first slice of the file; editing it
      // and saving would drop the rest, so the buffer stays read-only until
      // the user loads the whole file (banner button above).
      EditorState.readOnly.of(tab.truncated),
      EditorView.editable.of(!tab.truncated),
      ...islandDark,
      languageForPath(tab.relPath) ?? [],
      // In-editor find/replace over the WHOLE document. Without this the
      // WebView's native Ctrl+F takes over and only matches the visible
      // (virtualized) viewport lines. searchKeymap binds Mod-f → open panel,
      // Mod-Alt-f → replace, Enter/Shift-Enter → next/prev.
      search({ top: true }),
      highlightSelectionMatches(),
      // The goto-line dialog is CodeMirror's own, so its labels come from the
      // phrase table rather than a component's `t()` call.
      EditorState.phrases.of({
        "Go to line": t("goto_line"),
        go: t("goto_line_submit"),
      }),
      keymap.of([
        {
          key: "Mod-s",
          preventDefault: true,
          run: () => {
            void save();
            return true;
          },
        },
        // Ctrl+G → goto line, shadowing searchKeymap's find-next (still on F3
        // and Enter inside the search panel). Listed before ...searchKeymap so
        // it wins; Mod-Alt-g from searchKeymap keeps working too.
        { key: "Mod-g", preventDefault: true, run: gotoLine },
        {
          // Shift+Alt+F — the format shortcut every editor uses.
          key: "Shift-Alt-f",
          preventDefault: true,
          run: (v) => {
            void runLspFormat(v, lspCtx);
            return true;
          },
        },
        ...searchKeymap,
        ...closeBracketsKeymap,
        ...completionKeymap,
        ...foldKeymap,
        ...historyKeymap,
        // Tab indents/dedents inside the editor. Listed last so it never
        // shadows completion's Tab; blurring is still reachable with Escape
        // then Tab, which is what the a11y guidance asks for.
        indentWithTab,
        // Base editing/navigation commands (word-wise motion, line ops,
        // selection). Last so every binding above wins on conflict.
        ...defaultKeymap,
      ]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          setBuffer(projectId, worktreeId, tab.relPath, u.state.doc.toString());
          scheduleDiagnostics();
        }
      }),
      EditorView.lineWrapping,
      // Diagnostics, hover and Ctrl+click go-to-definition, served by the same
      // per-worktree language servers the MCP tools use. Everything fails soft:
      // a file with no server just gets none of it.
      ...lspExtensions(lspCtx),
    ];
    const view = new EditorView({
      state: EditorState.create({
        doc: tab.buffer,
        extensions,
      }),
      parent: containerRef.current,
    });
    viewRef.current = view;
    // Restore where the user was in this file. Offsets are re-clamped: the
    // buffer may have shrunk since (external change, reload from disk).
    const cacheKey = `${scopeKey(projectId, worktreeId)}::${tab.relPath}`;
    const saved = viewStateCache.get(cacheKey);
    if (saved) {
      const max = view.state.doc.length;
      view.dispatch({
        selection: EditorSelection.single(
          Math.min(saved.anchor, max),
          Math.min(saved.head, max),
        ),
      });
      view.scrollDOM.scrollTop = saved.scrollTop;
    }
    // First pass for the file as opened. Skipped while the tab is still
    // loading — that mount holds the empty skeleton, not the file.
    if (!tab.loading) scheduleDiagnostics();
    return () => {
      if (diagTimer.current !== null) {
        window.clearTimeout(diagTimer.current);
        diagTimer.current = null;
      }
      // The pane also mounts once against the empty skeleton while the file
      // loads (the key includes `loading`). Saving that mount's position would
      // overwrite the real one with 0/0 every time a tab is reopened.
      if (!tab.loading) {
        const sel = view.state.selection.main;
        viewStateCache.set(cacheKey, {
          anchor: sel.anchor,
          head: sel.head,
          scrollTop: view.scrollDOM.scrollTop,
        });
      }
      view.destroy();
      viewRef.current = null;
    };
    // Re-create on (projectId, worktreeId, relPath) change — buffer changes
    // are handled by the updateListener. projectId is in the deps so the
    // keymap/updateListener closures rebind when switching between two
    // projects' same-path file (shared nil-UUID worktreeId sentinel).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, worktreeId, tab.relPath, tab.truncated, tab.loading, showEditor]);

  // If the buffer was reset externally (e.g. file reload), sync into the view.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (view.state.doc.toString() !== tab.buffer) {
      view.dispatch({
        changes: {
          from: 0,
          to: view.state.doc.length,
          insert: tab.buffer,
        },
      });
    }
  }, [tab.buffer]);

  // Honor a pending "go to line" request (symbol search / Find in Files):
  // select the target line and center it. Selecting the line gives a visible
  // highlight band without needing a custom decoration field.
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !showEditor) return;
    if (!reveal || reveal.relPath !== tab.relPath) return;
    if (tab.loading) return;
    const lineNo = Math.min(Math.max(reveal.line, 1), view.state.doc.lines);
    const lineObj = view.state.doc.line(lineNo);
    view.dispatch({
      selection: EditorSelection.range(lineObj.from, lineObj.to),
      effects: EditorView.scrollIntoView(lineObj.from, { y: "center" }),
    });
    view.focus();
    consumeReveal(projectId, worktreeId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reveal?.nonce, tab.relPath, tab.loading, showEditor]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 shrink-0 items-center justify-between border-b border-neutral-800/60 bg-neutral-900/40 px-2 text-[11px] text-neutral-400">
        <span className="truncate" title={tab.relPath}>
          {tab.relPath}
          {tab.truncated && (
            <span className="ml-2 rounded bg-amber-900/40 px-1 text-amber-300">
              {t("truncated_readonly")}
            </span>
          )}
        </span>
        <div className="flex items-center gap-1">
          {tab.error && (
            <span className="text-red-400" role="alert">
              {tab.error}
            </span>
          )}
          {tab.kind === "markdown" && (
            <div className="flex items-center overflow-hidden rounded border border-neutral-800">
              <button
                type="button"
                onClick={() => setMdView("raw")}
                className={`flex items-center gap-1 px-1.5 py-0.5 ${
                  mdView === "raw"
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
              >
                <FileText size={10} /> {t("view_raw")}
              </button>
              <button
                type="button"
                onClick={() => setMdView("preview")}
                className={`flex items-center gap-1 px-1.5 py-0.5 ${
                  mdView === "preview"
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
              >
                <Eye size={10} /> {t("view_preview")}
              </button>
            </div>
          )}
          {tab.truncated && (
            <button
              type="button"
              onClick={() => void loadFullFile(projectId, worktreeId, tab.relPath)}
              disabled={tab.loading}
              title={t("load_full_file_hint")}
              className="rounded border border-amber-800/60 px-1.5 py-0.5 text-amber-300 enabled:hover:bg-amber-950/40 disabled:opacity-40"
            >
              {tab.loading ? t("loading") : t("load_full_file")}
            </button>
          )}
          <button
            type="button"
            onClick={() => void save()}
            disabled={!dirty || tab.saving || isPreviewOnly || tab.truncated}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-300 enabled:hover:bg-neutral-800 enabled:hover:text-neutral-100 disabled:opacity-40"
          >
            <Save size={11} />
            {tab.saving ? t("saving") : t("save")}
          </button>
          <div className="relative">
            <button
              type="button"
              onClick={() => {
                // Only the system default is available → no point in a menu,
                // open it straight away. Otherwise toggle the picker.
                if (availableEditors.length <= 1) {
                  openWith(availableEditors[0]?.id ?? "default");
                } else {
                  setEditorPickerOpen((v) => !v);
                }
              }}
              className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-300 hover:bg-neutral-800 hover:text-neutral-100"
            >
              <ExternalLink size={11} />
              {t("open_external")}
            </button>
            {editorPickerOpen && availableEditors.length > 1 && (
              <div className="absolute right-0 top-full z-10 mt-1 min-w-[160px] rounded border border-neutral-800 bg-neutral-950 p-1 shadow-lg">
                {/* Only installed editors are listed (plus the always-present
                    system default) — nothing missing is shown. */}
                {availableEditors.map((ed) => (
                  <button
                    key={ed.id}
                    type="button"
                    onClick={() => openWith(ed.id)}
                    className="block w-full rounded px-2 py-1 text-left text-[11px] hover:bg-neutral-900"
                  >
                    {ed.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
      {tab.externalContent !== null && (
        <div
          role="alert"
          className="flex shrink-0 items-center justify-between gap-2 border-b border-amber-800/60 bg-amber-950/40 px-2 py-1 text-[11px] text-amber-200"
        >
          <span className="truncate">{t("disk_changed")}</span>
          <div className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              onClick={() =>
                void reloadFromDisk(projectId, worktreeId, tab.relPath)
              }
              className="rounded px-1.5 py-0.5 hover:bg-amber-900/60"
            >
              {t("disk_reload")}
            </button>
            <button
              type="button"
              onClick={() => keepLocalChanges(projectId, worktreeId, tab.relPath)}
              className="rounded px-1.5 py-0.5 hover:bg-amber-900/60"
            >
              {t("disk_keep_mine")}
            </button>
          </div>
        </div>
      )}
      {tab.loading ? (
        <div className="flex flex-1 items-center justify-center text-[12px] text-neutral-500">
          {t("loading")}
        </div>
      ) : tab.kind === "image" ? (
        <ImagePreview
          projectId={projectId}
          worktreeId={worktreeId}
          relPath={tab.relPath}
        />
      ) : tab.kind === "pdf" ? (
        <PdfPreview
          projectId={projectId}
          worktreeId={worktreeId}
          relPath={tab.relPath}
        />
      ) : tab.kind === "markdown" && mdView === "preview" ? (
        <MarkdownPreview content={tab.buffer} />
      ) : (
        <div ref={containerRef} className="min-h-0 flex-1 overflow-auto" />
      )}
    </div>
  );
}

function basename(p: string): string {
  const idx = p.lastIndexOf("/");
  return idx >= 0 ? p.slice(idx + 1) : p;
}

/** Polyfill for CSS.escape so we can stuff arbitrary relPaths into selectors. */
function cssEscape(s: string): string {
  if (typeof CSS !== "undefined" && CSS.escape) return CSS.escape(s);
  return s.replace(/["\\]/g, "\\$&");
}


