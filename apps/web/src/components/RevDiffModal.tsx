import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { File, FileDiff as FileDiffIcon, X } from "lucide-react";
import { gitDiffRevs, type FileDiff } from "~/ipc/git.ts";
import { MonacoDiffViewer } from "~/components/MonacoDiffViewer.tsx";

interface Props {
  projectId: string;
  worktreeId: string;
  /** Source revision (commit SHA, branch, tag, or "WORKTREE"). */
  from: string;
  /** Destination revision. */
  to: string;
  /** Title shown in the modal header — caller decides how to render the
   *  comparison ("a → b", "abc1234 → HEAD", etc.). */
  title: string;
  /** When set, the diff is narrowed to this single worktree-relative path
   *  (the file the user right-clicked). Used by the file-tree "Compare with…"
   *  / "Show diff" actions so they don't show the whole-tree diff. */
  pathFilter?: string;
  open: boolean;
  onClose: () => void;
}

/**
 * Full revision-vs-revision diff: file list on the left, Monaco diff on
 * the right. Used from the log right-click menu ("Show changes",
 * "Compare with HEAD", "Compare with working").
 */
export function RevDiffModal({
  projectId,
  worktreeId,
  from,
  to,
  title,
  pathFilter,
  open,
  onClose,
}: Props) {
  const { t } = useTranslation("git");
  const [files, setFiles] = useState<FileDiff[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setFiles(null);
    setError(null);
    setSelected(null);
    void gitDiffRevs({ projectId, worktreeId, from, to })
      .then((res) => {
        if (cancelled) return;
        const filtered = pathFilter
          ? res.filter(
              (f) => f.path === pathFilter || f.old_path === pathFilter,
            )
          : res;
        setFiles(filtered);
        setSelected(filtered[0]?.path ?? null);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [open, projectId, worktreeId, from, to]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const activeFile = useMemo(
    () => files?.find((f) => f.path === selected) ?? null,
    [files, selected],
  );

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="flex h-[85vh] w-[90vw] max-w-6xl flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3 text-[12px] text-neutral-200">
          <span className="flex items-center gap-2">
            <FileDiffIcon size={13} className="text-neutral-500" />
            {title}
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
        <div className="flex min-h-0 flex-1">
          <div className="w-72 shrink-0 overflow-auto border-r border-neutral-800 py-1 text-[11px]">
            {error && (
              <div className="px-3 py-2 text-red-400" role="alert">
                {error}
              </div>
            )}
            {files && files.length === 0 && !error && (
              <div className="px-3 py-2 text-neutral-500">
                {t("rev_diff_empty")}
              </div>
            )}
            {files?.map((f) => (
              <button
                key={`${f.path}-${f.old_path ?? ""}`}
                type="button"
                onClick={() => setSelected(f.path)}
                className={`flex w-full items-center gap-2 px-3 py-1 text-left ${
                  selected === f.path
                    ? "bg-neutral-900 text-neutral-100"
                    : "text-neutral-300 hover:bg-neutral-900/60"
                }`}
              >
                <span
                  className={`shrink-0 rounded px-1 text-[9px] ${statusBadgeClass(f.status)}`}
                >
                  {statusBadgeChar(f.status)}
                </span>
                <span className="truncate" title={f.old_path ? `${f.old_path} → ${f.path}` : f.path}>
                  {f.old_path ? (
                    <>
                      <span className="text-neutral-500 line-through">
                        {basename(f.old_path)}
                      </span>{" "}
                      → {basename(f.path)}
                    </>
                  ) : (
                    basename(f.path)
                  )}
                </span>
              </button>
            ))}
          </div>
          <div className="flex min-h-0 flex-1 flex-col">
            {activeFile ? (
              <>
                <div className="h-7 shrink-0 truncate border-b border-neutral-800/60 bg-neutral-900/40 px-2 py-1 text-[11px] text-neutral-400">
                  {activeFile.old_path ? `${activeFile.old_path} → ` : ""}
                  {activeFile.path}
                </div>
                <MonacoDiffViewer
                  oldContent={activeFile.old_content}
                  newContent={activeFile.new_content}
                  path={activeFile.path}
                />
              </>
            ) : !error && files === null ? (
              <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
                {t("loading")}
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

function basename(p: string): string {
  const idx = p.lastIndexOf("/");
  return idx >= 0 ? p.slice(idx + 1) : p;
}

function statusBadgeChar(s: FileDiff["status"]): string {
  switch (s) {
    case "added":
      return "A";
    case "modified":
      return "M";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "typechange":
      return "T";
    default:
      return "?";
  }
}

function statusBadgeClass(s: FileDiff["status"]): string {
  switch (s) {
    case "added":
      return "bg-emerald-900/40 text-emerald-300";
    case "modified":
      return "bg-amber-900/40 text-amber-300";
    case "deleted":
      return "bg-red-900/40 text-red-300";
    case "renamed":
    case "copied":
      return "bg-blue-900/40 text-blue-300";
    default:
      return "bg-neutral-800 text-neutral-300";
  }
}
