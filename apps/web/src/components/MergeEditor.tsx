import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, GitMerge, RotateCcw } from "lucide-react";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers, keymap } from "@codemirror/view";
import { islandDark } from "~/lib/codemirror-theme.ts";
import { languageForPath } from "~/lib/codemirror-language.ts";
import {
  gitGetConflict,
  gitResolve,
  type ConflictContents,
} from "~/ipc/git.ts";
import { useGitStore } from "~/stores/gitStore.ts";

interface Props {
  projectId: string;
  worktreeId: string;
  path: string;
}

/**
 * Three-pane conflict editor.
 *
 * Layout: [ours read-only] [result editor] [theirs read-only]. "Take ours"
 * / "Take theirs" / "Take both" stamp the result; "Mark resolved" writes
 * the result and stages it via `git_resolve`. Status auto-refreshes after
 * resolve so the conflict drops out of the changes list.
 */
export function MergeEditor({ projectId, worktreeId, path }: Props) {
  const { t } = useTranslation("git");
  const refreshStatus = useGitStore((s) => s.refreshStatus);
  const [conflict, setConflict] = useState<ConflictContents | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [showBase, setShowBase] = useState(false);
  const [result, setResult] = useState<string>("");

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setConflict(null);
    void gitGetConflict({ projectId, worktreeId, path })
      .then((c) => {
        if (cancelled) return;
        setConflict(c);
        setResult(c.workdir ?? c.ours ?? c.theirs ?? "");
      })
      .catch((e) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, worktreeId, path]);

  if (error) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-red-400">
        {error}
      </div>
    );
  }
  if (!conflict) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-neutral-500">
        {t("loading")}
      </div>
    );
  }

  const markResolved = async () => {
    setResolving(true);
    try {
      await gitResolve({ projectId, worktreeId, path, content: result });
      await refreshStatus(projectId, worktreeId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setResolving(false);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-neutral-950">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-neutral-800 px-2 text-[11px] text-neutral-300">
        <span className="flex items-center gap-1 truncate">
          <GitMerge size={12} className="text-amber-400" />
          {path}
        </span>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setResult(conflict.ours ?? "")}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
          >
            <Copy size={10} /> {t("take_ours")}
          </button>
          <button
            type="button"
            onClick={() => setResult(conflict.theirs ?? "")}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
          >
            <Copy size={10} /> {t("take_theirs")}
          </button>
          <button
            type="button"
            onClick={() =>
              setResult(`${conflict.ours ?? ""}\n${conflict.theirs ?? ""}`)
            }
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
          >
            <Copy size={10} /> {t("take_both")}
          </button>
          <button
            type="button"
            onClick={() => setResult(conflict.workdir ?? "")}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
          >
            <RotateCcw size={10} /> {t("reset_to_workdir")}
          </button>
          <button
            type="button"
            onClick={() => setShowBase((v) => !v)}
            className={`rounded px-1.5 py-0.5 ${
              showBase
                ? "bg-neutral-800 text-neutral-100"
                : "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
            }`}
          >
            {t("show_base")}
          </button>
          <button
            type="button"
            onClick={() => void markResolved()}
            disabled={resolving}
            className="flex items-center gap-1 rounded bg-emerald-700/80 px-2 py-0.5 text-neutral-100 enabled:hover:bg-emerald-700 disabled:opacity-40"
          >
            <Check size={11} />
            {resolving ? t("resolving") : t("mark_resolved")}
          </button>
        </div>
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-3 gap-px bg-neutral-800">
        <Pane
          label={t("ours")}
          path={path}
          content={conflict.ours ?? ""}
          editable={false}
        />
        <Pane
          label={t("result")}
          path={path}
          content={result}
          editable
          onChange={setResult}
        />
        <Pane
          label={t("theirs")}
          path={path}
          content={conflict.theirs ?? ""}
          editable={false}
        />
        {showBase && (
          <div className="col-span-3 border-t border-neutral-700">
            <Pane
              label={t("base")}
              path={path}
              content={conflict.base ?? ""}
              editable={false}
            />
          </div>
        )}
      </div>
    </div>
  );
}

interface PaneProps {
  label: string;
  path: string;
  content: string;
  editable: boolean;
  onChange?: (next: string) => void;
}

function Pane({ label, path, content, editable, onChange }: PaneProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);

  const lang = useMemo(() => languageForPath(path), [path]);

  useEffect(() => {
    if (!containerRef.current) return;
    const extensions: Extension[] = [
      lineNumbers(),
      ...islandDark,
      lang ?? [],
      EditorView.lineWrapping,
      EditorView.editable.of(editable),
      EditorState.readOnly.of(!editable),
    ];
    if (editable && onChange) {
      extensions.push(
        EditorView.updateListener.of((u) => {
          if (u.docChanged) {
            onChange(u.state.doc.toString());
          }
        }),
        keymap.of([]),
      );
    }
    const view = new EditorView({
      state: EditorState.create({ doc: content, extensions }),
      parent: containerRef.current,
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, editable]);

  // Sync external content changes (e.g. "Take ours" button on a non-editable
  // pane stays static; on editable pane buttons mutate `content` so we
  // reflect that into the editor).
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (view.state.doc.toString() !== content) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: content },
      });
    }
  }, [content]);

  return (
    <div className="flex min-h-0 flex-col bg-neutral-950">
      <div className="shrink-0 border-b border-neutral-800/60 bg-neutral-900/40 px-2 py-1 text-[10px] uppercase tracking-wide text-neutral-500">
        {label}
      </div>
      <div ref={containerRef} className="min-h-0 flex-1 overflow-auto" />
    </div>
  );
}


