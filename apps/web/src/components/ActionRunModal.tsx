import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDot, X } from "lucide-react";
import { listenActionOutput, type ActionStreamLine } from "~/ipc/actions.ts";

interface Props {
  runId: string;
  name: string;
  onClose: () => void;
}

type Line = { kind: "stdout" | "stderr"; text: string };
type Status =
  | { kind: "running" }
  | { kind: "done"; code: number; success: boolean }
  | { kind: "error"; message: string };

/**
 * Streaming output viewer for a running action. Subscribes to
 * `action:output:<runId>` and renders stdout / stderr lines as they
 * arrive; auto-scrolls when at the bottom; shows exit status when the
 * child terminates.
 */
export function ActionRunModal({ runId, name, onClose }: Props) {
  const { t } = useTranslation("actions");
  const [lines, setLines] = useState<Line[]>([]);
  const [status, setStatus] = useState<Status>({ kind: "running" });
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void listenActionOutput(runId, (line: ActionStreamLine) => {
      if (line.kind === "stdout" || line.kind === "stderr") {
        setLines((prev) => [...prev, line]);
      } else if (line.kind === "exit") {
        setStatus({ kind: "done", code: line.code, success: line.success });
      } else if (line.kind === "error") {
        setStatus({ kind: "error", message: line.message });
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [runId]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 50;
    if (atBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }, [lines.length, status.kind]);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="flex h-[70vh] w-[80vw] max-w-3xl flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3 text-[12px]">
          <span className="flex items-center gap-2 text-neutral-200">
            <StatusDot status={status} />
            {name}
          </span>
          <span className="text-[10px] text-neutral-500">
            {status.kind === "done"
              ? t("exit_code", { code: status.code })
              : status.kind === "error"
                ? status.message
                : t("running")}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-100"
            aria-label={t("close")}
          >
            <X size={13} />
          </button>
        </div>
        <div
          ref={scrollRef}
          className="min-h-0 flex-1 overflow-auto bg-neutral-950 px-3 py-2 font-mono text-[11.5px] text-neutral-200"
        >
          {lines.map((l, i) => (
            <div
              key={i}
              className={
                l.kind === "stderr" ? "whitespace-pre-wrap text-red-300" : "whitespace-pre-wrap"
              }
            >
              {l.text}
            </div>
          ))}
          {status.kind === "running" && lines.length === 0 && (
            <div className="text-neutral-500">{t("waiting")}</div>
          )}
        </div>
      </div>
    </div>
  );
}

function StatusDot({ status }: { status: Status }) {
  if (status.kind === "running") {
    return <CircleDot size={11} className="animate-pulse text-amber-400" />;
  }
  if (status.kind === "done") {
    return (
      <CircleDot
        size={11}
        className={status.success ? "text-emerald-400" : "text-red-400"}
      />
    );
  }
  return <CircleDot size={11} className="text-red-400" />;
}
