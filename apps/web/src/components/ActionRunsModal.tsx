import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { CircleDot, Minus, X } from "lucide-react";
import {
  useActionRunsStore,
  type RunInstance,
  type RunStatus,
} from "~/stores/actionRunsStore.ts";

interface Props {
  actionId: string;
  actionName: string;
  onMinimize: () => void;
}

/**
 * Tabbed live-output viewer for every active run of an action. Tabs only
 * appear when there are 2+ instances. Minimize keeps the runs alive (sidebar
 * counter stays); X kills the active tab's run (and auto-closes the modal
 * when the last instance dies).
 */
export function ActionRunsModal({ actionId, actionName, onMinimize }: Props) {
  const { t } = useTranslation("actions");
  const runs = useActionRunsStore((s) => s.runs[actionId] ?? EMPTY);
  const activeTabRun = useActionRunsStore((s) => s.activeTabRun[actionId]);
  const setActiveTab = useActionRunsStore((s) => s.setActiveTab);
  const killRun = useActionRunsStore((s) => s.killRun);

  const active = runs.find((r) => r.runId === activeTabRun) ?? runs[0];

  if (!active) return null;

  const multi = runs.length > 1;

  return (
    <div className="fixed bottom-2 right-12 z-30 flex h-[55vh] w-[60vw] max-w-3xl flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl">
      <div className="flex h-9 shrink-0 items-stretch border-b border-neutral-800 text-[12px]">
        {multi ? (
          <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
            {runs.map((r, i) => {
              const isActive = r.runId === active.runId;
              return (
                <div
                  key={r.runId}
                  className={`group flex shrink-0 items-center gap-1.5 border-r border-neutral-800 pl-3 pr-1 ${
                    isActive
                      ? "bg-neutral-900 text-neutral-100"
                      : "text-neutral-400 hover:bg-neutral-900/50 hover:text-neutral-200"
                  }`}
                >
                  <button
                    type="button"
                    onClick={() => setActiveTab(actionId, r.runId)}
                    className="flex items-center gap-1.5"
                    title={`${actionName} #${i + 1} · ${new Date(r.startedAt).toLocaleString()}`}
                  >
                    <StatusDot status={r.status} />
                    <span>
                      {actionName}
                      <span className="ml-1 text-neutral-500">#{i + 1}</span>
                    </span>
                  </button>
                  <button
                    type="button"
                    onClick={() => killRun(actionId, r.runId)}
                    className="rounded p-0.5 text-neutral-500 hover:bg-red-900/40 hover:text-red-300"
                    title={t("kill")}
                    aria-label={t("kill")}
                  >
                    <X size={11} />
                  </button>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="flex min-w-0 flex-1 items-center gap-1.5 px-3">
            <StatusDot status={active.status} />
            <span className="truncate text-neutral-200">{actionName}</span>
            {active.status.kind !== "running" && (
              <span className="text-[10px] text-neutral-500">
                {labelForStatus(active.status, t)}
              </span>
            )}
          </div>
        )}
        <div className="flex shrink-0 items-center gap-0.5 border-l border-neutral-800 px-1">
          <button
            type="button"
            onClick={onMinimize}
            className="rounded p-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
            title={t("minimize")}
            aria-label={t("minimize")}
          >
            <Minus size={13} />
          </button>
          {!multi && (
            <button
              type="button"
              onClick={() => killRun(actionId, active.runId)}
              className="rounded p-1 text-neutral-400 hover:bg-red-900/40 hover:text-red-300"
              title={t("kill")}
              aria-label={t("kill")}
            >
              <X size={13} />
            </button>
          )}
        </div>
      </div>

      <RunOutput run={active} />
    </div>
  );
}

const EMPTY: RunInstance[] = [];

function RunOutput({ run }: { run: RunInstance }) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickyRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (stickyRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [run.lines.length, run.status.kind]);

  return (
    <div
      ref={scrollRef}
      onScroll={() => {
        const el = scrollRef.current;
        if (!el) return;
        stickyRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 50;
      }}
      className="min-h-0 flex-1 overflow-auto bg-neutral-950 px-3 py-2 font-mono text-[11.5px] text-neutral-200"
    >
      {run.lines.map((l, i) => (
        <div
          key={i}
          className={
            l.kind === "stderr"
              ? "whitespace-pre-wrap text-red-300"
              : "whitespace-pre-wrap"
          }
        >
          {l.text}
        </div>
      ))}
      {run.status.kind === "running" && run.lines.length === 0 && (
        <div className="text-neutral-500">…</div>
      )}
    </div>
  );
}

function StatusDot({ status, small }: { status: RunStatus; small?: boolean }) {
  const size = small ? 9 : 11;
  if (status.kind === "running") {
    return (
      <CircleDot size={size} className="animate-pulse text-amber-400" />
    );
  }
  if (status.kind === "done") {
    return (
      <CircleDot
        size={size}
        className={status.success ? "text-emerald-400" : "text-red-400"}
      />
    );
  }
  return <CircleDot size={size} className="text-red-400" />;
}

function labelForStatus(
  status: RunStatus,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  switch (status.kind) {
    case "running":
      return t("running");
    case "done":
      return t("exit_code", { code: status.code });
    case "error":
      return status.message;
  }
}
