import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ActionSidebar } from "~/components/ActionSidebar.tsx";
import { ChatPanel } from "~/components/ChatPanel.tsx";
import { PureClaudePanel } from "~/components/PureClaudePanel.tsx";
import { MultiViewPanel } from "~/components/MultiViewPanel.tsx";
import { FilesPanel } from "~/components/FilesPanel.tsx";
import { GitPanel } from "~/components/GitPanel.tsx";
import { Modal } from "~/components/Modal.tsx";
import { QuickFileSearch } from "~/components/QuickFileSearch.tsx";
import { PRIMARY_WORKTREE_ID } from "~/ipc/worktree.ts";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";
import { ProjectPanel } from "~/components/ProjectPanel.tsx";
import { ProjectSettingsModal } from "~/components/ProjectSettingsModal.tsx";
import { SettingsPanel } from "~/components/SettingsPanel.tsx";
import { Sidebar } from "~/components/Sidebar.tsx";
import { TerminalPanel } from "~/components/TerminalPanel.tsx";
import { TitleBar } from "~/components/TitleBar.tsx";
import { UpdateBanner } from "~/components/UpdateBanner.tsx";
import { WelcomeScreen } from "~/components/WelcomeScreen.tsx";
import { isTypingTarget, matchesKey } from "~/lib/keybindings.ts";
import { useIndexingStore } from "~/stores/indexingStore.ts";
import { useKeybindingsStore } from "~/stores/keybindingsStore.ts";
import { useLspStatusStore } from "~/stores/lspStatusStore.ts";
import { useUpdaterStore } from "~/stores/updaterStore.ts";
import {
  type DockerCleanupReport,
  onDockerCleanup,
} from "~/ipc/env.ts";
import { useFileEditorStore } from "~/stores/fileEditorStore.ts";
import { useProjectStore } from "~/stores/projectStore.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";

type Tab = "chat" | "multi" | "files" | "git" | "settings";

export function App() {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const activeId = useProjectStore((s) => s.activeId);
  const refresh = useProjectStore((s) => s.refresh);
  const active = projects.find((p) => p.id === activeId) ?? null;

  const [tab, setTab] = useState<Tab>("chat");
  const [projectModalOpen, setProjectModalOpen] = useState(false);
  const [projectSettingsId, setProjectSettingsId] = useState<string | null>(
    null,
  );
  const [terminalOpen, setTerminalOpen] = useState(false);
  const [quickOpen, setQuickOpen] = useState(false);
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const sessionSnapshot = useSessionStore((s) =>
    activeSessionId ? s.snapshots[activeSessionId] : null,
  );
  const setActiveSession = useSessionStore((s) => s.setActive);
  const pureMode = useAppSettingsStore((s) => s.pureMode);
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
      if (matchesKey(e, bindings.new_thread)) {
        e.preventDefault();
        // "New thread" now means "leave the active session and land on the
        // empty state for the active project". If there's no project yet,
        // fall back to creating one — the user has nothing to scope to.
        if (activeId) {
          setActiveSession(null);
        } else {
          setProjectModalOpen(true);
        }
        return;
      }
      if (matchesKey(e, bindings.toggle_terminal)) {
        if (!activeSessionId) return;
        e.preventDefault();
        setTerminalOpen((v) => !v);
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
  }, [bindings, activeSessionId, activeId, setActiveSession]);

  const quickWorktreeId =
    sessionSnapshot?.worktree_id ?? PRIMARY_WORKTREE_ID;

  const center = active ? (
    <div className="flex items-center gap-2">
      <ProjectBadge
        name={active.name}
        projectId={active.id}
        logoPath={active.logo_path}
        size={16}
      />
      <span className="text-neutral-200">{active.name}</span>
    </div>
  ) : null;

  const titleBarActions = (
    <div className="flex items-center gap-1 pr-2">
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
              onNewSession={() => setActiveSession(null)}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
            <div className="flex min-h-0 flex-1 flex-col bg-neutral-950">
              <main className="flex min-h-0 flex-1 flex-col">
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
                    onToggleTerminal={
                      activeSessionId
                        ? () => setTerminalOpen((v) => !v)
                        : undefined
                    }
                    terminalOpen={terminalOpen}
                  />
                )}
              </main>
              {terminalOpen && activeSessionId && (
                <div className="h-72 shrink-0">
                  <TerminalPanel
                    sessionId={activeSessionId}
                    onClose={() => setTerminalOpen(false)}
                  />
                </div>
              )}
            </div>
          </>
        )}
        {tab === "multi" && (
          <>
            <Sidebar
              onNewProject={() => setProjectModalOpen(true)}
              onOpenSettings={() => setTab("settings")}
              onNewSession={() => setActiveSession(null)}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
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
              onNewSession={() => setActiveSession(null)}
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
              onNewSession={() => setActiveSession(null)}
              onOpenProjectSettings={(id) => setProjectSettingsId(id)}
            />
            <main className="flex min-h-0 flex-1 flex-col bg-neutral-950">
              <GitPanel projectId={activeId} />
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
          />
        )}
      </div>

      <Modal
        open={projectModalOpen}
        onClose={() => setProjectModalOpen(false)}
        closeLabel={t("sidebar.close")}
      >
        <ProjectPanel onCreated={() => setProjectModalOpen(false)} />
      </Modal>

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
    </div>
  );
}

