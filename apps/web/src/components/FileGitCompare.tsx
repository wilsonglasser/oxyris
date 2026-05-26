import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { GitBranch, History, Tag as TagIcon, X } from "lucide-react";
import { gitLog, gitTagList, type CommitInfo, type TagInfo } from "~/ipc/git.ts";
import { gitListBranches, type BranchInfo } from "~/ipc/worktree.ts";
import { RevDiffModal } from "~/components/RevDiffModal.tsx";

function basename(p: string): string {
  const idx = p.lastIndexOf("/");
  return idx >= 0 ? p.slice(idx + 1) : p;
}

function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

interface CompareRefProps {
  projectId: string;
  worktreeId: string;
  /** "refs" lists branches + tags; "revs" lists recent commits. */
  kind: "refs" | "revs";
  /** File the comparison targets — shown in the header. */
  relPath: string;
  /** Called with the chosen git ref (branch/tag name or commit oid) and a
   *  human label for the diff title. */
  onPick: (ref: string, label: string) => void;
  onClose: () => void;
}

/**
 * Picker for the file-tree "Compare with Branch or Tag…" / "Compare with
 * Revision…" actions. It only chooses the ref — the caller opens the actual
 * single-file diff (via {@link RevDiffModal} with a `pathFilter`).
 */
export function CompareRefModal({
  projectId,
  worktreeId,
  kind,
  relPath,
  onPick,
  onClose,
}: CompareRefProps) {
  const { t, i18n } = useTranslation("git");
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [tags, setTags] = useState<TagInfo[]>([]);
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    const load = async () => {
      try {
        if (kind === "refs") {
          const [b, tg] = await Promise.all([
            gitListBranches(projectId),
            gitTagList({ projectId, worktreeId }),
          ]);
          if (cancelled) return;
          setBranches(b);
          setTags(tg);
        } else {
          const log = await gitLog({ projectId, worktreeId, limit: 80 });
          if (cancelled) return;
          setCommits(log);
        }
      } catch (e) {
        if (!cancelled) setError(errMessage(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [kind, projectId, worktreeId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const q = filter.toLowerCase();
  const shownBranches = useMemo(
    () => branches.filter((b) => b.name.toLowerCase().includes(q)),
    [branches, q],
  );
  const shownTags = useMemo(
    () => tags.filter((tg) => tg.name.toLowerCase().includes(q)),
    [tags, q],
  );
  const shownCommits = useMemo(
    () =>
      commits.filter(
        (c) =>
          c.summary.toLowerCase().includes(q) ||
          c.short_oid.toLowerCase().includes(q),
      ),
    [commits, q],
  );

  const title =
    kind === "refs" ? t("compare_pick_ref") : t("compare_pick_rev");

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="flex max-h-[70vh] w-[28rem] flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3 text-[12px] text-neutral-200">
          <span className="truncate">
            {title}
            <span className="ml-2 text-neutral-500">{basename(relPath)}</span>
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
        <div className="border-b border-neutral-800 p-2">
          <input
            type="text"
            autoFocus
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={t("compare_search")}
            className="w-full rounded bg-neutral-900 px-2 py-1 text-[12px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
          />
        </div>
        <div className="min-h-0 flex-1 overflow-auto py-1 text-[12px]">
          {error && (
            <div className="px-3 py-2 text-red-400" role="alert">
              {error}
            </div>
          )}
          {loading && !error && (
            <div className="px-3 py-2 text-neutral-500">{t("loading")}</div>
          )}
          {kind === "refs" && !loading && !error && (
            <>
              {shownBranches.length > 0 && (
                <div className="px-3 py-1 text-[10px] uppercase tracking-wide text-neutral-500">
                  {t("compare_branches")}
                </div>
              )}
              {shownBranches.map((b) => (
                <button
                  key={`${b.is_remote ? "r" : "l"}-${b.name}`}
                  type="button"
                  onClick={() => onPick(b.name, b.name)}
                  className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
                >
                  <GitBranch size={12} className="shrink-0 text-neutral-500" />
                  <span className="truncate">{b.name}</span>
                  {b.is_remote && (
                    <span className="ml-auto text-[9px] text-neutral-500">
                      remote
                    </span>
                  )}
                </button>
              ))}
              {shownTags.length > 0 && (
                <div className="px-3 py-1 text-[10px] uppercase tracking-wide text-neutral-500">
                  {t("compare_tags")}
                </div>
              )}
              {shownTags.map((tg) => (
                <button
                  key={tg.name}
                  type="button"
                  onClick={() => onPick(tg.name, tg.name)}
                  className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
                >
                  <TagIcon size={12} className="shrink-0 text-emerald-400/80" />
                  <span className="truncate">{tg.name}</span>
                </button>
              ))}
              {shownBranches.length === 0 && shownTags.length === 0 && (
                <div className="px-3 py-2 text-neutral-500">
                  {t("compare_no_refs")}
                </div>
              )}
            </>
          )}
          {kind === "revs" && !loading && !error && (
            <>
              {shownCommits.map((c) => (
                <button
                  key={c.oid}
                  type="button"
                  onClick={() => onPick(c.oid, c.short_oid)}
                  className="flex w-full flex-col gap-0.5 px-3 py-1 text-left hover:bg-neutral-900"
                >
                  <span className="flex items-baseline gap-2">
                    <code className="text-neutral-500">{c.short_oid}</code>
                    <span className="truncate text-neutral-200">
                      {c.summary}
                    </span>
                  </span>
                  <span className="text-[10px] text-neutral-500">
                    {c.author_name} ·{" "}
                    {new Date(c.author_time * 1000).toLocaleString(
                      i18n.language,
                    )}
                  </span>
                </button>
              ))}
              {shownCommits.length === 0 && (
                <div className="px-3 py-2 text-neutral-500">
                  {t("no_commits")}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

interface HistoryProps {
  projectId: string;
  worktreeId: string;
  relPath: string;
  onClose: () => void;
}

/**
 * File history: lists the commits that touched `relPath`. Clicking a commit
 * opens that commit's single-file diff (commit^ → commit) for the file.
 */
export function FileHistoryModal({
  projectId,
  worktreeId,
  relPath,
  onClose,
}: HistoryProps) {
  const { t, i18n } = useTranslation("git");
  const [commits, setCommits] = useState<CommitInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [diff, setDiff] = useState<
    { from: string; to: string; title: string } | null
  >(null);

  useEffect(() => {
    let cancelled = false;
    setCommits(null);
    setError(null);
    void gitLog({ projectId, worktreeId, path: relPath, limit: 100 })
      .then((log) => {
        if (!cancelled) setCommits(log);
      })
      .catch((e) => {
        if (!cancelled) setError(errMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, worktreeId, relPath]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="flex max-h-[75vh] w-[32rem] flex-col rounded-lg border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3 text-[12px] text-neutral-200">
          <span className="flex items-center gap-2 truncate">
            <History size={13} className="text-neutral-500" />
            {t("file_history")}
            <span className="text-neutral-500">{basename(relPath)}</span>
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
        <div className="min-h-0 flex-1 overflow-auto py-1 text-[12px]">
          {error && (
            <div className="px-3 py-2 text-red-400" role="alert">
              {error}
            </div>
          )}
          {!commits && !error && (
            <div className="px-3 py-2 text-neutral-500">{t("loading")}</div>
          )}
          {commits && commits.length === 0 && !error && (
            <div className="px-3 py-2 text-neutral-500">{t("no_commits")}</div>
          )}
          {commits?.map((c) => (
            <button
              key={c.oid}
              type="button"
              onClick={() =>
                setDiff({
                  from: `${c.oid}^`,
                  to: c.oid,
                  title: t("rev_show_changes_in", { id: c.short_oid }),
                })
              }
              className="flex w-full flex-col gap-0.5 border-b border-neutral-900/50 px-3 py-1 text-left hover:bg-neutral-900/50"
            >
              <span className="flex items-baseline gap-2">
                <code className="text-neutral-500">{c.short_oid}</code>
                <span className="truncate text-neutral-200">{c.summary}</span>
              </span>
              <span className="text-[10px] text-neutral-500">
                {c.author_name} ·{" "}
                {new Date(c.author_time * 1000).toLocaleString(i18n.language)}
              </span>
            </button>
          ))}
        </div>
      </div>
      {diff && (
        <RevDiffModal
          projectId={projectId}
          worktreeId={worktreeId}
          from={diff.from}
          to={diff.to}
          title={diff.title}
          pathFilter={relPath}
          open
          onClose={() => setDiff(null)}
        />
      )}
    </div>
  );
}
