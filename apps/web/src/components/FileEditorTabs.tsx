import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight, ExternalLink, Save, X } from "lucide-react";
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers, keymap } from "@codemirror/view";
import { islandDark } from "~/lib/codemirror-theme.ts";
import { languageForPath } from "~/lib/codemirror-language.ts";
import { Eye, FileText } from "lucide-react";
import { fsExternalEditors, fsOpenExternal } from "~/ipc/fs.ts";
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
                  onClick={() => closeTab(projectId, worktreeId, relPath)}
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
        <div
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-[140px] rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-lg"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            onClick={() => {
              closeTab(projectId, worktreeId, menu.relPath);
              setMenu(null);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close")}
          </button>
          <button
            type="button"
            onClick={() => {
              closeOthers(projectId, worktreeId, menu.relPath);
              setMenu(null);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close_others")}
          </button>
          <button
            type="button"
            onClick={() => {
              closeAll(projectId, worktreeId);
              setMenu(null);
            }}
            className="block w-full px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            {t("ctx_close_all")}
          </button>
        </div>
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

interface EditorPaneProps {
  projectId: string;
  worktreeId: string;
  tab: Tab;
}

export function EditorPane({ projectId, worktreeId, tab }: EditorPaneProps) {
  const { t } = useTranslation("files");
  const setBuffer = useFileEditorStore((s) => s.setBuffer);
  const saveTab = useFileEditorStore((s) => s.saveTab);
  const reloadFromDisk = useFileEditorStore((s) => s.reloadFromDisk);
  const keepLocalChanges = useFileEditorStore((s) => s.keepLocalChanges);
  const reveal = useFileEditorStore(
    (s) => s.reveal[scopeKey(projectId, worktreeId)] ?? null,
  );
  const consumeReveal = useFileEditorStore((s) => s.consumeReveal);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const dirty = tab.buffer !== tab.baseContent;
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
    const extensions: Extension[] = [
      lineNumbers(),
      ...islandDark,
      languageForPath(tab.relPath) ?? [],
      keymap.of([
        {
          key: "Mod-s",
          preventDefault: true,
          run: () => {
            void saveTab(projectId, worktreeId, tab.relPath);
            return true;
          },
        },
      ]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          setBuffer(projectId, worktreeId, tab.relPath, u.state.doc.toString());
        }
      }),
      EditorView.lineWrapping,
    ];
    const view = new EditorView({
      state: EditorState.create({
        doc: tab.buffer,
        extensions,
      }),
      parent: containerRef.current,
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Re-create on (projectId, worktreeId, relPath) change — buffer changes
    // are handled by the updateListener. projectId is in the deps so the
    // keymap/updateListener closures rebind when switching between two
    // projects' same-path file (shared nil-UUID worktreeId sentinel).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, worktreeId, tab.relPath, showEditor]);

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
              {t("truncated")}
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
          <button
            type="button"
            onClick={() => void saveTab(projectId, worktreeId, tab.relPath)}
            disabled={!dirty || tab.saving || isPreviewOnly}
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


