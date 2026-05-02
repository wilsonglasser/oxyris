import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { diffLines } from "diff";
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  FileEdit,
  FilePlus2,
  FileSearch,
  Globe,
  Loader2,
  Pencil,
  Search,
  Square,
  SquareCheckBig,
  Terminal,
  type LucideIcon,
} from "lucide-react";
import type { AssistantBlock } from "~/ipc/session.ts";

type ToolUseBlock = Extract<AssistantBlock, { kind: "tool_use" }>;
type ToolResultBlock = Extract<AssistantBlock, { kind: "tool_result" }>;

interface Props {
  use: ToolUseBlock;
  result: ToolResultBlock | undefined;
}

/**
 * Dispatcher that picks a specialized renderer for each known tool. Unknown
 * tools fall back to the generic JSON expander.
 */
export function ToolCallView({ use, result }: Props) {
  switch (use.name) {
    case "Edit":
    case "Update":
      return <EditToolView use={use} result={result} />;
    case "Write":
    case "Create":
      return <WriteToolView use={use} result={result} />;
    case "Read":
      return <ReadToolView use={use} result={result} />;
    case "Bash":
      return <BashToolView use={use} result={result} />;
    case "Grep":
      return <GrepToolView use={use} result={result} />;
    case "Glob":
      return <GlobToolView use={use} result={result} />;
    case "TodoWrite":
      return <TodoWriteToolView use={use} />;
    case "Task":
      return <TaskToolView use={use} result={result} />;
    case "WebFetch":
    case "WebSearch":
      return <WebToolView use={use} result={result} />;
    default:
      return <GenericToolView use={use} result={result} />;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// Layout primitives

interface RowProps {
  status: "running" | "ok" | "error";
  icon: LucideIcon;
  title: React.ReactNode;
  subline?: React.ReactNode;
  children?: React.ReactNode;
  initiallyOpen?: boolean;
}

function ToolRow({
  status,
  icon: Icon,
  title,
  subline,
  children,
  initiallyOpen = true,
}: RowProps) {
  const [open, setOpen] = useState(initiallyOpen);
  const dotColor =
    status === "running"
      ? "text-amber-400"
      : status === "error"
        ? "text-red-400"
        : "text-emerald-500";
  const hasBody = !!children;
  return (
    <div className="text-[12px] leading-5">
      <div className="flex items-start gap-2">
        <span className={`mt-0.5 ${dotColor}`}>
          {status === "running" ? (
            <Loader2 className="size-3 animate-spin" strokeWidth={2} />
          ) : status === "error" ? (
            <AlertCircle className="size-3" strokeWidth={2} />
          ) : (
            <CheckCircle2 className="size-3" strokeWidth={2} />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <div
            className={`flex min-w-0 items-center gap-1 ${
              hasBody ? "cursor-pointer" : ""
            }`}
            onClick={hasBody ? () => setOpen((v) => !v) : undefined}
          >
            <Icon className="size-3 shrink-0 text-neutral-400" strokeWidth={1.75} />
            <span className="min-w-0 flex-1 truncate font-medium text-neutral-100">
              {title}
            </span>
            {hasBody && (
              <span className="shrink-0 text-neutral-600">
                {open ? (
                  <ChevronDown className="size-3" strokeWidth={2} />
                ) : (
                  <ChevronRight className="size-3" strokeWidth={2} />
                )}
              </span>
            )}
          </div>
          {subline && (
            <div className="truncate pl-[18px] text-[11px] text-neutral-500">
              ↳ {subline}
            </div>
          )}
          {hasBody && open && (
            <div className="mt-1 border-l border-neutral-800 pl-3">
              {children}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Specialized renderers

function EditToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const path = asString(input.file_path);
  const oldStr = asString(input.old_string);
  const newStr = asString(input.new_string);
  const status = !result ? "running" : result.is_error ? "error" : "ok";

  const { added, removed, hunks } = useMemo(
    () => diffHunks(oldStr, newStr),
    [oldStr, newStr],
  );

  return (
    <ToolRow
      status={status}
      icon={FileEdit}
      title={
        <>
          Edit(<span className="text-neutral-400">{shortenPath(path)}</span>)
        </>
      }
      subline={
        result?.is_error ? (
          <span className="text-red-300">{t("tool_edit_failed")}</span>
        ) : (
          t("tool_edit_summary", { added, removed })
        )
      }
    >
      {hunks.length > 0 && <MiniDiff hunks={hunks} />}
    </ToolRow>
  );
}

function WriteToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const path = asString(input.file_path);
  const content = asString(input.content);
  const lines = content.split("\n").length;
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={FilePlus2}
      title={
        <>
          Write(<span className="text-neutral-400">{shortenPath(path)}</span>)
        </>
      }
      subline={t("tool_write_summary", { count: lines })}
    />
  );
}

function ReadToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const path = asString(input.file_path);
  const offset = asNumber(input.offset);
  const limit = asNumber(input.limit);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  const range =
    offset && limit
      ? `L${offset}-${offset + limit - 1}`
      : offset
        ? `L${offset}+`
        : limit
          ? `L1-${limit}`
          : null;
  return (
    <ToolRow
      status={status}
      icon={FileSearch}
      title={
        <>
          Read(<span className="text-neutral-400">{shortenPath(path)}</span>)
        </>
      }
      subline={
        result?.is_error
          ? t("tool_read_failed")
          : range
            ? t("tool_read_range", { range })
            : t("tool_read_full")
      }
    />
  );
}

function BashToolView({ use, result }: Props) {
  const input = (use.input as Record<string, unknown>) ?? {};
  const command = asString(input.command);
  const desc = asString(input.description);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={Terminal}
      title={
        <span className="font-mono text-[11px] text-neutral-200">
          $ {command}
        </span>
      }
      subline={desc || null}
    >
      {result && (
        <pre className="max-h-60 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

function GrepToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const pattern = asString(input.pattern);
  const path = asString(input.path);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={Search}
      title={
        <>
          {t("tool_grep_title", { pattern })}
          {path && (
            <span className="ml-1.5 text-neutral-500">
              in {shortenPath(path)}
            </span>
          )}
        </>
      }
      subline={result && !result.is_error ? countMatches(result) : null}
      initiallyOpen={false}
    >
      {result && (
        <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

function GlobToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const pattern = asString(input.pattern);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={Search}
      title={t("tool_glob_title", { pattern })}
      subline={result && !result.is_error ? countMatches(result) : null}
      initiallyOpen={false}
    >
      {result && (
        <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

interface TodoItem {
  content: string;
  status: "pending" | "in_progress" | "completed";
}

function TodoWriteToolView({ use }: { use: ToolUseBlock }) {
  const input = (use.input as Record<string, unknown>) ?? {};
  const todos = Array.isArray(input.todos)
    ? (input.todos as TodoItem[])
    : [];
  const { t } = useTranslation("chat");
  const completedCount = todos.filter((t) => t.status === "completed").length;
  return (
    <ToolRow
      status="ok"
      icon={SquareCheckBig}
      title={t("tool_todo_title")}
      subline={t("tool_todo_progress", {
        done: completedCount,
        total: todos.length,
      })}
    >
      <ul className="flex flex-col gap-0.5">
        {todos.map((todo, i) => (
          <li key={i} className="flex items-start gap-1.5 text-[11px]">
            {todo.status === "completed" ? (
              <CheckCircle2
                className="mt-0.5 size-3 shrink-0 text-emerald-500"
                strokeWidth={2}
              />
            ) : todo.status === "in_progress" ? (
              <Square
                className="mt-0.5 size-3 shrink-0 fill-amber-400/40 text-amber-400"
                strokeWidth={2}
              />
            ) : (
              <Circle
                className="mt-0.5 size-3 shrink-0 text-neutral-600"
                strokeWidth={2}
              />
            )}
            <span
              className={
                todo.status === "completed"
                  ? "text-neutral-500 line-through"
                  : todo.status === "in_progress"
                    ? "text-amber-200"
                    : "text-neutral-300"
              }
            >
              {todo.content}
            </span>
          </li>
        ))}
      </ul>
    </ToolRow>
  );
}

function TaskToolView({ use, result }: Props) {
  const { t } = useTranslation("chat");
  const input = (use.input as Record<string, unknown>) ?? {};
  const description = asString(input.description);
  const subagent = asString(input.subagent_type);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (status !== "running") return;
    const start = Date.now();
    const id = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - start) / 1000));
    }, 1000);
    return () => window.clearInterval(id);
  }, [status]);
  return (
    <ToolRow
      status={status}
      icon={Pencil}
      title={
        <>
          {t("tool_task_title", {
            agent: subagent || t("tool_task_default_agent"),
          })}
          {description && (
            <span className="ml-1.5 text-neutral-400">— {description}</span>
          )}
        </>
      }
      subline={
        status === "running" ? t("tool_task_running", { seconds: elapsed }) : null
      }
      initiallyOpen={false}
    >
      {result && (
        <pre className="max-h-60 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

function WebToolView({ use, result }: Props) {
  const input = (use.input as Record<string, unknown>) ?? {};
  const query = asString(input.query ?? input.url);
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={Globe}
      title={
        <span className="truncate">
          {use.name}(<span className="text-neutral-400">{query}</span>)
        </span>
      }
      initiallyOpen={false}
    >
      {result && (
        <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

function GenericToolView({ use, result }: Props) {
  const status = !result ? "running" : result.is_error ? "error" : "ok";
  return (
    <ToolRow
      status={status}
      icon={Square}
      title={
        <span>
          {use.name}
          <span className="ml-1 text-neutral-500">(generic)</span>
        </span>
      }
      initiallyOpen={false}
    >
      <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
        {JSON.stringify(use.input, null, 2)}
      </pre>
      {result && (
        <pre className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap rounded bg-neutral-950 p-2 font-mono text-[11px] text-neutral-300">
          {renderResultText(result)}
        </pre>
      )}
    </ToolRow>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Mini diff

interface Hunk {
  kind: "add" | "del" | "ctx";
  text: string;
}

function diffHunks(
  oldStr: string,
  newStr: string,
): { added: number; removed: number; hunks: Hunk[] } {
  if (!oldStr && !newStr) return { added: 0, removed: 0, hunks: [] };
  const parts = diffLines(oldStr, newStr);
  let added = 0;
  let removed = 0;
  const hunks: Hunk[] = [];
  for (const part of parts) {
    const lines = part.value.replace(/\n$/, "").split("\n");
    if (part.added) {
      added += lines.length;
      for (const line of lines) hunks.push({ kind: "add", text: line });
    } else if (part.removed) {
      removed += lines.length;
      for (const line of lines) hunks.push({ kind: "del", text: line });
    } else {
      // Trim long context runs so the mini-diff stays mini.
      const trimmed = lines.length > 6 ? [...lines.slice(0, 3), "…", ...lines.slice(-3)] : lines;
      for (const line of trimmed) hunks.push({ kind: "ctx", text: line });
    }
  }
  return { added, removed, hunks };
}

function MiniDiff({ hunks }: { hunks: Hunk[] }) {
  return (
    <pre className="max-h-64 overflow-y-auto rounded bg-neutral-950 p-1 font-mono text-[11px] leading-5">
      {hunks.map((h, i) => {
        if (h.kind === "add") {
          return (
            <div key={i} className="bg-emerald-950/40 text-emerald-200">
              <span className="select-none text-emerald-500">+ </span>
              {h.text}
            </div>
          );
        }
        if (h.kind === "del") {
          return (
            <div key={i} className="bg-red-950/40 text-red-200">
              <span className="select-none text-red-500">- </span>
              {h.text}
            </div>
          );
        }
        return (
          <div key={i} className="text-neutral-500">
            <span className="select-none">&nbsp;&nbsp;</span>
            {h.text}
          </div>
        );
      })}
    </pre>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Helpers

function asString(v: unknown): string {
  if (typeof v === "string") return v;
  if (v == null) return "";
  return String(v);
}

function asNumber(v: unknown): number | null {
  if (typeof v === "number") return v;
  if (typeof v === "string") {
    const n = Number(v);
    return Number.isFinite(n) ? n : null;
  }
  return null;
}

function shortenPath(full: string): string {
  if (!full) return "";
  // Keep last two segments so the view stays compact but you still know
  // which file is affected in monorepos.
  const parts = full.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return full;
  return `…/${parts.slice(-2).join("/")}`;
}

function renderResultText(result: ToolResultBlock): string {
  if (typeof result.output === "string") return result.output;
  return JSON.stringify(result.output, null, 2);
}

function countMatches(result: ToolResultBlock): string | null {
  const raw = renderResultText(result);
  const lines = raw.split("\n").filter((l) => l.trim().length > 0);
  if (lines.length === 0) return null;
  return `${lines.length} match${lines.length === 1 ? "" : "es"}`;
}
