import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ActionSidebar } from "~/components/ActionSidebar.tsx";
import { OxyDock } from "~/components/OxyDock.tsx";
import { OxyVoice } from "~/components/OxyVoice.tsx";
import { useOxyStore } from "~/stores/oxyStore.ts";
import { voiceEnable, voiceWakeReady } from "~/ipc/voice.ts";
import { ChatPanel } from "~/components/ChatPanel.tsx";
import { PureClaudePanel } from "~/components/PureClaudePanel.tsx";
import { MultiViewPanel } from "~/components/MultiViewPanel.tsx";
import { FilesPanel } from "~/components/FilesPanel.tsx";
import { GitPanel } from "~/components/GitPanel.tsx";
import { Modal } from "~/components/Modal.tsx";
import { QuickFileSearch } from "~/components/QuickFileSearch.tsx";
import {
  SearchEverywhere,
  type SearchTab,
} from "~/components/SearchEverywhere.tsx";
import { FindInFiles } from "~/components/FindInFiles.tsx";
import { PRIMARY_WORKTREE_ID } from "~/ipc/worktree.ts";
import type { ProjectRow } from "~/ipc/commands.ts";
import { NewChatModal } from "~/components/NewChatModal.tsx";
import { ProjectSwitcher } from "~/components/ProjectSwitcher.tsx";
import { ProjectPanel } from "~/components/ProjectPanel.tsx";
import { ProjectSettingsModal } from "~/components/ProjectSettingsModal.tsx";
import { SettingsPanel } from "~/components/SettingsPanel.tsx";
import { Sidebar } from "~/components/Sidebar.tsx";
import { TerminalPanel } from "~/components/TerminalPanel.tsx";
import { TitleBar } from "~/components/TitleBar.tsx";
import { UpdateBanner } from "~/components/UpdateBanner.tsx";
import { AutopilotAlerts } from "~/components/AutopilotAlerts.tsx";
import { WelcomeScreen } from "~/components/WelcomeScreen.tsx";
import { claudeLanguageDirective } from "~/lib/claudeLanguage.ts";
import { useDragResize } from "~/lib/useDragResize.ts";
import { isTypingTarget, matchesKey } from "~/lib/keybindings.ts";
import { clearBadge } from "~/lib/taskbarBadge.ts";
import { sessionStart, type SessionKind } from "~/ipc/session.ts";
import { useIndexingStore } from "~/stores/indexingStore.ts";
import { useKeybindingsStore } from "~/stores/keybindingsStore.ts";
import { useLspStatusStore } from "~/stores/lspStatusStore.ts";
import { useUpdaterStore } from "~/stores/updaterStore.ts";
import {
  type DockerCleanupReport,
  onDockerCleanup,
} from "~/ipc/env.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { useMultiViewStore } from "~/stores/multiViewStore.ts";
import { useProjectStore } from "~/stores/projectStore.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import { useWhipStore } from "~/stores/whipStore.ts";

type Tab = "chat" | "multi" | "files" | "git" | "settings";

export function App() {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const activeId = useProjectStore((s) => s.activeId);
  const setActiveProject = useProjectStore((s) => s.setActive);
  const refresh = useProjectStore((s) => s.refresh);
  const active = projects.find((p) => p.id === activeId) ?? null;

  const [tab, setTab] = useState<Tab>("chat");
  const [projectModalOpen, setProjectModalOpen] = useState(false);
  const [newChatOpen, setNewChatOpen] = useState(false);
  const [projectSettingsId, setProjectSettingsId] = useState<string | null>(
    null,
  );
  // Terminal dock visibility is per-session, not global: switching threads must
  // reflect *that* thread's own dock state, not carry the previous one's open
  // flag over (which also made the dock auto-spawn a fresh shell into the new
  // session). Keyed by session id; absent = closed.
  const [terminalOpenBySession, setTerminalOpenBySession] = useState<
    Record<string, boolean>
  >({});
  const [quickOpen, setQuickOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchTab, setSearchTab] = useState<SearchTab>("symbols");
  const [findOpen, setFindOpen] = useState(false);
  /** Find in Files opens with the replace row expanded (Ctrl+Shift+R). */
  const [findReplace, setFindReplace] = useState(false);
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const sessionSnapshot = useSessionStore((s) =>
    activeSessionId ? s.snapshots[activeSessionId] : null,
  );
  const setActiveSession = useSessionStore((s) => s.setActive);
  const terminalOpen = activeSessionId
    ? !!terminalOpenBySession[activeSessionId]
    : false;
  const toggleTerminal = useCallback(() => {
    setTerminalOpenBySession((prev) => {
      if (!activeSessionId) return prev;
      return { ...prev, [activeSessionId]: !prev[activeSessionId] };
    });
  }, [activeSessionId]);
  const closeTerminal = useCallback(() => {
    setTerminalOpenBySession((prev) => {
      if (!activeSessionId) return prev;
      return { ...prev, [activeSessionId]: false };
    });
  }, [activeSessionId]);
  const openTerminal = useCallback(() => {
    setTerminalOpenBySession((prev) => {
      if (!activeSessionId) return prev;
      return { ...prev, [activeSessionId]: true };
    });
  }, [activeSessionId]);
  const pureMode = useAppSettingsStore((s) => s.pureMode);
  const toggleOxy = useOxyStore((s) => s.toggle);
  const oxyOpen = useOxyStore((s) => s.open);
  const multiSidebarHidden = useMultiViewStore((s) => s.sidebarHidden);
  const terminalResize = useDragResize({
    storageKey: "oxyris.terminal.height",
    defaultSize: 288,
    min: 120,
    max: 900,
    axis: "vertical",
    direction: "up",
  });

  // Clear the taskbar unread badge whenever the window regains focus — the
  // badge counts turns that completed while the user was away (see
  // `taskbarBadge.ts` / the `bumpBadge` calls on turn completion).
  useEffect(() => {
    const onFocus = () => clearBadge();
    window.addEventListener("focus", onFocus);
    if (document.hasFocus()) clearBadge();
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // Re-arm the wake word on boot if the user left it enabled and the model is
  // installed. The backend listener doesn't survive a restart.
  useEffect(() => {
    const s = useOxyStore.getState();
    if (!s.wakeEnabled) return;
    void (async () => {
      try {
        if (await voiceWakeReady()) {
          await voiceEnable({
            keywords: s.keyword,
            threshold: s.threshold,
            device: s.device || null,
          });
        }
      } catch {
        /* leave it disarmed; Settings surfaces the error on next toggle */
      }
    })();
  }, []);

  // NB: pure-mode dot state (busy / needs-input / done) for the active thread —
  // and every background thread — is owned by the single pure-state bridge in
  // `Sidebar` (always mounted). It used to be driven here too, which raced with
  // the Sidebar and PureClaudePanel listeners over the same stores.
  const bindings = useKeybindingsStore((s) => s.bindings);
  const loadBindings = useKeybindingsStore((s) => s.load);
  const backgroundCheckUpdate = useUpdaterStore((s) => s.backgroundCheck);
  const [cleanupReport, setCleanupReport] = useState<DockerCleanupReport | null>(
    null,
  );

  // Initial load + keep it fresh on window focus (safety net for changes
  // that happen outside the app).
  useEffect(() => {
    void refresh();
    void loadBindings();
    void backgroundCheckUpdate();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh, loadBindings, backgroundCheckUpdate]);

  // Subscribe once to the backend's `indexing:progress`, `lsp:status`, and
  // `fs:changed` streams so the chips + file tree react regardless of which
  // tab is currently visible.
  useEffect(() => {
    let unlistenIndex: (() => void) | null = null;
    let unlistenLsp: (() => void) | null = null;
    let unlistenFs: (() => void) | null = null;
    void useIndexingStore
      .getState()
      .subscribe()
      .then((fn) => {
        unlistenIndex = fn;
      });
    void useLspStatusStore
      .getState()
      .subscribe()
      .then((fn) => {
        unlistenLsp = fn;
      });
    void useFileEditorStore
      .getState()
      .subscribeFsChanged(() => useProjectStore.getState().activeId)
      .then((fn) => {
        unlistenFs = fn;
      });
    return () => {
      if (unlistenIndex) unlistenIndex();
      if (unlistenLsp) unlistenLsp();
      if (unlistenFs) unlistenFs();
    };
  }, []);

  // Subscribe to the boot-time docker cleanup report so the UI can surface
  // a one-time toast about the orphan stacks we pruned.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void onDockerCleanup(setCleanupReport).then((fn) => {
      unlisten = fn;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Auto-dismiss the toast after a short window — keeps the chrome clean.
  useEffect(() => {
    if (!cleanupReport) return;
    const id = window.setTimeout(() => setCleanupReport(null), 8000);
    return () => window.clearTimeout(id);
  }, [cleanupReport]);

  // Global keyboard shortcuts driven by the user-editable keybindings.json.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // WebView2 still honours the browser reload accelerators, and a reload
      // here is a hard app restart: PTYs detach from their panes, modal state
      // and drafts are gone. Swallow them unconditionally (same trick as
      // Ctrl+W below); Ctrl+Shift+R is then free to mean "replace in files".
      if (
        e.key === "F5" ||
        ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "r")
      ) {
        e.preventDefault();
        if (!(e.shiftKey && activeId && tab === "files")) return;
        setFindReplace(true);
        setFindOpen(true);
        return;
      }
      if (matchesKey(e, bindings.new_thread)) {
        // The default "new thread" combo (Ctrl+Shift+N) doubles as "go to
        // file" when not in a conversation view — JetBrains-style. New thread
        // only fires from the chat / multi tabs; on Files/Git/Settings the
        // same combo opens Search Everywhere on the Files scope.
        if (tab === "chat" || tab === "multi") {
          e.preventDefault();
          // "New thread" asks which project to scope the chat to (the app is no
          // longer pinned to a single project). With no projects yet, fall back
          // to creating one — there's nothing to scope to.
          if (useProjectStore.getState().projects.length > 0) {
            setNewChatOpen(true);
          } else {
            setProjectModalOpen(true);
          }
          return;
        }
        if (activeId) {
          e.preventDefault();
          setSearchTab("files");
          setSearchOpen(true);
          return;
        }
      }
      // Ctrl+N / Cmd+N → Search Everywhere on the Symbols scope.
      if (
        (e.ctrlKey || e.metaKey) &&
        !e.shiftKey &&
        !e.altKey &&
        e.key.toLowerCase() === "n" &&
        activeId
      ) {
        e.preventDefault();
        setSearchTab("symbols");
        setSearchOpen(true);
        return;
      }
      // Ctrl+Shift+F → Find in Files (full-text search + preview). Scoped to
      // the Files tab so it doesn't fire while typing in a chat.
      if (
        (e.ctrlKey || e.metaKey) &&
        e.shiftKey &&
        !e.altKey &&
        e.key.toLowerCase() === "f" &&
        activeId &&
        tab === "files"
      ) {
        e.preventDefault();
        setFindReplace(false);
        setFindOpen(true);
        return;
      }
      if (matchesKey(e, bindings.toggle_terminal)) {
        if (!activeSessionId) return;
        e.preventDefault();
        toggleTerminal();
        return;
      }
      if (matchesKey(e, bindings.focus_search) && !isTypingTarget(e.target)) {
        e.preventDefault();
        const input = document.querySelector<HTMLInputElement>(
          "aside input[placeholder]",
        );
        input?.focus();
        input?.select();
      }
      // Ctrl+P / Cmd+P opens the quick file search modal (project-scoped).
      // Fires from any tab, including chat — feels like an editor shortcut.
      if (
        (e.ctrlKey || e.metaKey) &&
        !e.shiftKey &&
        !e.altKey &&
        e.key.toLowerCase() === "p" &&
        activeId
      ) {
        e.preventDefault();
        setQuickOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bindings, activeSessionId, activeId, setActiveSession, tab, toggleTerminal]);

  // Whip mode: Ctrl+W arms the whip cursor, Esc disarms. The body class drives
  // the global cursor swap (see index.css). Ctrl+W must preventDefault or the
  // WebView treats it as "close". Esc only disarms when whip is actually armed,
  // so it doesn't swallow Esc from modals/menus the rest of the time.
  const whipActive = useWhipStore((s) => s.active);
  const whipToggle = useWhipStore((s) => s.toggle);
  const whipSetActive = useWhipStore((s) => s.setActive);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key === "w") {
        e.preventDefault();
        whipToggle();
        return;
      }
      if (e.key === "Escape" && useWhipStore.getState().active) {
        whipSetActive(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [whipToggle, whipSetActive]);
  useEffect(() => {
    document.body.classList.toggle("whip-armed", whipActive);
    return () => document.body.classList.remove("whip-armed");
  }, [whipActive]);

  const quickWorktreeId =
    sessionSnapshot?.worktree_id ?? PRIMARY_WORKTREE_ID;

  // Picking a project for a new chat: switch to it, drop into the empty
  // composer, and make sure we're on the chat tab.
  const handleNewChatPick = (project: ProjectRow) => {
    setActiveProject(project.id);
    setActiveSession(null);
    setTab("chat");
    setNewChatOpen(false);
  };

  // "New thread": immediately spin up a session with the default config
  // (primary worktree → project root, provider's default model, the configured
  // default runtime — supervised unless changed in Settings, auto thinking, no
  // worktree env) and select it. The sidebar is
  // visible from Files/Git too, so jump to the chat tab as well. On failure
  // we fall back to the empty composer so the user can still start manually.
  const startNewSession = (
    project?: ProjectRow | null,
    kindOverride?: SessionKind,
  ) => {
    const p = project ?? active;
    setTab("chat");
    if (!p) {
      setActiveSession(null);
      return;
    }
    void (async () => {
      try {
        const res = await sessionStart({
          project_id: p.id,
          provider_id: "claude",
          environment: p.environment,
          cwd: p.root_path,
          model: "",
          thinking: "auto",
          runtime: useAppSettingsStore.getState().defaultRuntime,
          env_mode: "default",
          // Persist the kind that matches the current display toggle. Pure mode
          // renders the claude PTY, so the session must be stored as `pure` —
          // otherwise Multi View (which reads `session.kind`) embeds the
          // structured ChatPanel for a session that has no event-sourced turns.
          // Oxy is a Structured-shaped session with the cross-thread toolset;
          // an explicit override wins over the pure/structured display toggle.
          kind: kindOverride ?? (pureMode ? "pure" : "structured"),
          system_prompt: claudeLanguageDirective(
            useAppSettingsStore.getState().claudeLanguage,
          ),
        });
        setActiveSession(res.session_id);
      } catch {
        setActiveSession(null);
      }
    })();
  };

  const center =
    projects.length > 0 ? (
      <ProjectSwitcher onNewChat={() => setNewChatOpen(true)} />
    ) : null;

  const titleBarActions = (
    <div className="flex items-center gap-1 pr-2">
      <button
        type="button"
        onClick={toggleOxy}
        title={t("oxy.launch_hint")}
        className={`rounded-md px-2 py-0.5 text-[11px] transition ${
          oxyOpen
            ? "bg-emerald-900/50 text-emerald-300"
            : "text-emerald-400 hover:bg-emerald-950/40 hover:text-emerald-300"
        }`}
      >
        {t("oxy.launch")}
      </button>
      {(["chat", "multi", "files", "git", "settings"] as const).map((id) => (
        <button
          key={id}
          type="button"
          onClick={() => setTab(id)}
          className={`rounded-md px-2 py-0.5 text-[11px] transition ${
            tab === id
              ? "bg-neutral-800 text-neutral-100"
              : "text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
          }`}
        >
          {t(`tabs.${id}`)}
        </button>
      ))}
    </div>
  );

  return (
    <div className="flex h-full flex-col bg-neutral-900 text-neutral-200">
      <TitleBar center={center} actions={titleBarActions} />

      <div className="flex min-h-0 flex-1">
        {tab === "chat" && (
          <>
            <Sidebar
              onNewProject={() => setProjectModalOpen(true)}
              onOpenSettings={() => setTab("settings")}
              onNewSession={startNewSession}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
            <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-neutral-950">
              <main className="flex min-h-0 min-w-0 flex-1 flex-col">
                {projects.length === 0 ? (
                  <div className="min-h-0 flex-1 overflow-y-auto p-4">
                    <WelcomeScreen
                      onNewProject={() => setProjectModalOpen(true)}
                    />
                  </div>
                ) : pureMode ? (
                  // "Claude Code puro": the interactive TUI in a PTY replaces
                  // the structured chat entirely. Keyed by session for the
                  // same isolation reason as ChatPanel below.
                  <PureClaudePanel
                    key={activeSessionId ?? "new"}
                    project={active}
                    onToggleTerminal={activeSessionId ? toggleTerminal : undefined}
                    terminalOpen={terminalOpen}
                  />
                ) : (
                  <ChatPanel
                    // Key by session so each conversation gets its own
                    // composer state (draft text, queue, attachments,
                    // sending flag). Without this the single instance leaks
                    // one thread's draft into the next. Remounting also
                    // re-hydrates via `sessionGet`, which heals a snapshot
                    // left stuck "streaming" because its live TurnCompleted
                    // fired while the user was on a different conversation.
                    key={activeSessionId ?? "new"}
                    project={active}
                    onToggleTerminal={activeSessionId ? toggleTerminal : undefined}
                    terminalOpen={terminalOpen}
                  />
                )}
              </main>
              {terminalOpen && activeSessionId && (
                <div
                  className="relative shrink-0"
                  style={{ height: terminalResize.size }}
                >
                  <div
                    onMouseDown={terminalResize.onResizeStart}
                    role="separator"
                    aria-orientation="horizontal"
                    className="group absolute left-0 right-0 top-0 z-10 h-1 cursor-row-resize"
                  >
                    <div className="h-full w-full bg-transparent transition group-hover:bg-emerald-700/50" />
                  </div>
                  <TerminalPanel
                    key={activeSessionId}
                    sessionId={activeSessionId}
                    onClose={closeTerminal}
                  />
                </div>
              )}
            </div>
          </>
        )}
        {tab === "multi" && (
          <>
            {!multiSidebarHidden && (
              <Sidebar
                onNewProject={() => setProjectModalOpen(true)}
                onOpenSettings={() => setTab("settings")}
                onNewSession={startNewSession}
                onOpenProjectSettings={(id) => setProjectSettingsId(id)}
              />
            )}
            <main className="flex min-h-0 flex-1 flex-col bg-neutral-950">
              <MultiViewPanel />
            </main>
          </>
        )}
        {tab === "files" && (
          <>
            <Sidebar
              onNewProject={() => setProjectModalOpen(true)}
              onOpenSettings={() => setTab("settings")}
              onNewSession={startNewSession}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
            <main className="flex min-h-0 flex-1 flex-col bg-neutral-950">
              <FilesPanel projectId={activeId} />
            </main>
          </>
        )}
        {tab === "git" && (
          <>
            <Sidebar
              onNewProject={() => setProjectModalOpen(true)}
              onOpenSettings={() => setTab("settings")}
              onNewSession={startNewSession}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
            <main className="flex min-h-0 flex-1 flex-col bg-neutral-950">
              <GitPanel
                projectId={activeId}
                onOpenFiles={() => setTab("files")}
              />
            </main>
          </>
        )}
        {tab === "settings" && (
          <main className="min-h-0 flex-1 overflow-y-auto bg-neutral-950 p-4">
            <div className="mx-auto max-w-3xl">
              <SettingsPanel />
            </div>
          </main>
        )}
        {tab !== "settings" && (
          <ActionSidebar
            projectId={activeId}
            worktreeId={sessionSnapshot?.worktree_id ?? null}
            sessionId={activeSessionId}
            onOpenTerminal={openTerminal}
          />
        )}
        {/* Oxy — the app-global assistant dock, present on every tab. */}
        <OxyDock project={active} />
        {/* Headless: wake-word → voice-command capture → drive Oxy. */}
        <OxyVoice project={active} />
      </div>

      <Modal
        open={projectModalOpen}
        onClose={() => setProjectModalOpen(false)}
        closeLabel={t("sidebar.close")}
      >
        <ProjectPanel onCreated={() => setProjectModalOpen(false)} />
      </Modal>

      <NewChatModal
        open={newChatOpen}
        onClose={() => setNewChatOpen(false)}
        onPick={handleNewChatPick}
      />

      <Modal
        open={projectSettingsId !== null}
        onClose={() => setProjectSettingsId(null)}
        closeLabel={t("sidebar.close")}
      >
        {projectSettingsId && (
          <ProjectSettingsModal
            projectId={projectSettingsId}
            onClose={() => setProjectSettingsId(null)}
          />
        )}
      </Modal>

      {activeId && (
        <QuickFileSearch
          projectId={activeId}
          worktreeId={quickWorktreeId}
          open={quickOpen}
          onClose={() => setQuickOpen(false)}
        />
      )}

      {activeId && (
        <SearchEverywhere
          projectId={activeId}
          worktreeId={quickWorktreeId}
          open={searchOpen}
          initialTab={searchTab}
          onClose={() => setSearchOpen(false)}
        />
      )}

      {activeId && (
        <FindInFiles
          projectId={activeId}
          worktreeId={quickWorktreeId}
          open={findOpen}
          replace={findReplace}
          onClose={() => setFindOpen(false)}
        />
      )}

      {cleanupReport && (
        <div
          role="status"
          className="fixed bottom-4 right-4 z-50 max-w-sm rounded-lg border border-neutral-700 bg-neutral-900/95 px-4 py-3 text-[12px] text-neutral-200 shadow-xl shadow-black/50 backdrop-blur"
        >
          <div className="mb-1 font-medium text-neutral-100">
            {t("docker_cleanup.title")}
          </div>
          <div className="text-neutral-400">
            {t("docker_cleanup.body", {
              count: cleanupReport.orphan_projects.length,
              containers: cleanupReport.containers_removed,
              volumes: cleanupReport.volumes_removed,
              networks: cleanupReport.networks_removed,
            })}
          </div>
          <button
            type="button"
            onClick={() => setCleanupReport(null)}
            className="mt-2 text-[11px] text-neutral-500 hover:text-neutral-200"
          >
            {t("docker_cleanup.dismiss")}
          </button>
        </div>
      )}

      <UpdateBanner />
      <AutopilotAlerts />
    </div>
  );
}

