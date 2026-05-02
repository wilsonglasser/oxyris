import React, { useEffect, useState } from "react";

/**
 * Tauri invoke errors come back as the serialized Rust error enum (e.g.
 * `{ code: "checkpoint", message: "…" }` or just a string). `String(obj)`
 * collapses to `"[object Object]"` which is useless in the UI — unwrap the
 * common shapes instead.
 */
function formatError(e: unknown): string {
  if (!e) return "unknown";
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (typeof e === "object") {
    const anyObj = e as { code?: unknown; message?: unknown };
    if (typeof anyObj.message === "string" && anyObj.message.length > 0) {
      return typeof anyObj.code === "string"
        ? `${anyObj.code}: ${anyObj.message}`
        : anyObj.message;
    }
    if (typeof anyObj.code === "string") return anyObj.code;
    try {
      return JSON.stringify(e);
    } catch {
      return "unknown";
    }
  }
  return String(e);
}
import { useTranslation } from "react-i18next";
import {
  type FileDiff,
  type TurnDiff,
  sessionTurnDiff,
  sessionTurnRevert,
} from "~/ipc/session.ts";
import { DiffViewer } from "~/components/DiffViewer.tsx";

interface Props {
  sessionId: string;
  turnId: string;
}

export function TurnDiffView({ sessionId, turnId }: Props) {
  const { t } = useTranslation("chat");
  const [diff, setDiff] = useState<TurnDiff | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reverting, setReverting] = useState(false);
  const [revertNotice, setRevertNotice] = useState<string | null>(null);

  // Silently probe for a diff on mount. If the checkpoint doesn't exist or
  // the turn touched nothing, we render nothing — no point showing "Ver diff"
  // buttons with nothing behind them.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void sessionTurnDiff({ session_id: sessionId, turn_id: turnId })
      .then((d) => {
        if (!cancelled) setDiff(d);
      })
      .catch((e) => {
        if (!cancelled) setError(formatError(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, turnId]);

  const hasFiles = !!diff && diff.files.length > 0;
  // Nothing to show, nothing to revert: hide the entire block.
  if (!loading && !hasFiles) return null;

  const onRevert = async (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!window.confirm(t("revert_confirm"))) return;
    setReverting(true);
    setRevertNotice(null);
    try {
      await sessionTurnRevert({ session_id: sessionId, turn_id: turnId });
      setRevertNotice(t("revert_done"));
    } catch (err) {
      setRevertNotice(
        t("revert_failed", { message: formatError(err) }),
      );
    } finally {
      setReverting(false);
    }
  };

  return (
    <details className="rounded-lg border border-neutral-800 bg-neutral-950">
      <summary className="flex cursor-pointer items-center justify-between gap-2 px-3 py-2 text-[10px] uppercase tracking-wide text-neutral-500">
        <span>
          {t("diff_view")}
          {hasFiles && (
            <span className="ml-1.5 normal-case text-neutral-600">
              ({diff.files.length})
            </span>
          )}
        </span>
        <button
          type="button"
          disabled={reverting}
          onClick={onRevert}
          className="rounded border border-amber-900/60 px-2 py-0.5 text-[10px] normal-case text-amber-300 hover:bg-amber-950/40 disabled:opacity-60"
        >
          {t("revert_turn")}
        </button>
      </summary>
      <div className="border-t border-neutral-800 px-3 py-2">
        {revertNotice && (
          <p className="mb-2 text-[11px] text-amber-200">{revertNotice}</p>
        )}
        {loading && <p className="text-xs text-neutral-500">{t("diff_loading")}</p>}
        {error && (
          <p className="text-xs text-red-300">
            {t("diff_error", { message: error })}
          </p>
        )}
        {hasFiles && (
          <ul className="flex flex-col gap-2">
            {diff!.files.map((f, i) => (
              <FileDiffView key={i} file={f} />
            ))}
          </ul>
        )}
      </div>
    </details>
  );
}

function FileDiffView({ file }: { file: FileDiff }) {
  const { t } = useTranslation("chat");
  const [expanded, setExpanded] = useState(false);
  const statusLabel = t(`diff_status_${file.status}` as const);
  const headerColor = {
    added: "text-emerald-300",
    deleted: "text-red-300",
    modified: "text-amber-300",
    renamed: "text-sky-300",
    copied: "text-sky-300",
    typechange: "text-amber-300",
    unchanged: "text-neutral-400",
  }[file.status];

  return (
    <li className="rounded border border-neutral-800 bg-neutral-900 px-2 py-1.5">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center gap-2 text-left font-mono text-[11px]"
      >
        <span className="text-neutral-500">{expanded ? "▾" : "▸"}</span>
        <span className={`uppercase ${headerColor}`}>{statusLabel}</span>
        <span className="truncate text-neutral-200">
          {file.old_path && file.old_path !== file.path ? (
            <>
              {file.old_path} <span className="text-neutral-600">→</span>{" "}
              {file.path}
            </>
          ) : (
            file.path
          )}
        </span>
      </button>
      {expanded && (
        <div className="mt-2">
          <DiffViewer
            oldContent={file.old_content}
            newContent={file.new_content}
            path={file.path}
          />
        </div>
      )}
    </li>
  );
}
