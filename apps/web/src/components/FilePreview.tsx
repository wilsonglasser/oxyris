import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { fsReadFileBytes } from "~/ipc/fs.ts";

/**
 * Markdown preview — react-markdown + GFM + Tailwind typography (`prose`).
 * Styling tokens are tuned to match the JetBrains Markdown preview: tight
 * heading sizes, monospace inline code with subtle background, fenced blocks
 * with darker surface.
 */
export function MarkdownPreview({ content }: { content: string }) {
  return (
    <div
      className="
        prose prose-invert max-w-none
        min-h-0 flex-1 overflow-auto px-6 py-4 text-[13px] leading-relaxed
        prose-headings:scroll-mt-4 prose-headings:font-semibold prose-headings:text-neutral-100
        prose-h1:mt-2 prose-h1:mb-4 prose-h1:text-2xl prose-h1:border-b prose-h1:border-neutral-800 prose-h1:pb-2
        prose-h2:mt-6 prose-h2:mb-3 prose-h2:text-xl prose-h2:border-b prose-h2:border-neutral-800/60 prose-h2:pb-1
        prose-h3:mt-5 prose-h3:mb-2 prose-h3:text-lg
        prose-h4:mt-4 prose-h4:mb-2 prose-h4:text-base
        prose-p:my-2 prose-p:text-neutral-300
        prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline
        prose-strong:text-neutral-100 prose-strong:font-semibold
        prose-em:text-neutral-200
        prose-code:rounded prose-code:bg-neutral-800/70 prose-code:px-1 prose-code:py-0.5
        prose-code:text-[0.92em] prose-code:font-mono prose-code:text-amber-300
        prose-code:before:content-none prose-code:after:content-none
        prose-pre:bg-neutral-950 prose-pre:border prose-pre:border-neutral-800 prose-pre:rounded
        prose-pre:p-3 prose-pre:text-[12px]
        prose-blockquote:border-l-2 prose-blockquote:border-neutral-700
        prose-blockquote:text-neutral-400 prose-blockquote:not-italic prose-blockquote:pl-3
        prose-li:my-0.5 prose-ul:my-2 prose-ol:my-2
        prose-table:text-[12px] prose-th:bg-neutral-900 prose-th:text-neutral-100
        prose-td:border prose-td:border-neutral-800 prose-th:border prose-th:border-neutral-800
        prose-hr:border-neutral-800
        prose-img:rounded
      "
    >
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
    </div>
  );
}

/** Image preview — fetches bytes once via IPC and renders as <img>. */
export function ImagePreview({
  projectId,
  worktreeId,
  relPath,
}: {
  projectId: string;
  worktreeId: string;
  relPath: string;
}) {
  const { t } = useTranslation("files");
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setError(null);
    void fsReadFileBytes({ projectId, worktreeId, relPath })
      .then((res) => {
        if (cancelled) return;
        setSrc(`data:${res.mime};base64,${res.bytes_b64}`);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, worktreeId, relPath]);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-red-400">
        {error}
      </div>
    );
  }
  if (!src) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
        {t("loading")}
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto bg-neutral-950 p-4">
      <img
        src={src}
        alt={relPath}
        className="max-h-full max-w-full object-contain"
      />
    </div>
  );
}

/** PDF preview — base64 data URL into an iframe. */
export function PdfPreview({
  projectId,
  worktreeId,
  relPath,
}: {
  projectId: string;
  worktreeId: string;
  relPath: string;
}) {
  const { t } = useTranslation("files");
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setError(null);
    void fsReadFileBytes({ projectId, worktreeId, relPath })
      .then((res) => {
        if (cancelled) return;
        setSrc(`data:${res.mime};base64,${res.bytes_b64}`);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, worktreeId, relPath]);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-red-400">
        {error}
      </div>
    );
  }
  if (!src) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
        {t("loading")}
      </div>
    );
  }
  return <iframe title={relPath} src={src} className="h-full w-full border-0" />;
}
