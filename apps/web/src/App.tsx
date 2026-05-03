import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChatPanel } from "~/components/ChatPanel.tsx";
import { FilesPanel } from "~/components/FilesPanel.tsx";
import { GitPanel } from "~/components/GitPanel.tsx";
import { Modal } from "~/components/Modal.tsx";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";
import { ProjectPanel } from "~/components/ProjectPanel.tsx";
import { ProjectSettingsModal } from "~/components/ProjectSettingsModal.tsx";
import { SettingsPanel } from "~/components/SettingsPanel.tsx";
import { Sidebar } from "~/components/Sidebar.tsx";
import { TerminalPanel } from "~/components/TerminalPanel.tsx";
import { TitleBar } from "~/components/TitleBar.tsx";
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
import { useProjectStore } from "~/stores/projectStore.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";

type Tab = "chat" | "files" | "git" | "settings";

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
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const setActiveSession = useSessionStore((s) => s.setActive);
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

  // Subscribe once to the backend's `indexing:progress` and `lsp:status`
  // streams so the chips live in any panel that wants them.
  useEffect(() => {
    let unlistenIndex: (() => void) | null = null;
    let unlistenLsp: (() => void) | null = null;
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
    return () => {
      if (unlistenIndex) unlistenIndex();
      if (unlistenLsp) unlistenLsp();
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
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bindings, activeSessionId, activeId, setActiveSession]);

  const center = active ? (
    <div className="flex items-center gap-2">
      <ProjectBadge name={active.name} size={16} />
      <span className="text-neutral-200">{active.name}</span>
    </div>
  ) : null;

  const titleBarActions = (
    <div className="flex items-center gap-1 pr-2">
      {(["chat", "files", "git", "settings"] as const).map((id) => (
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
                ) : (
                  <ChatPanel
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
    </div>
  );
}

