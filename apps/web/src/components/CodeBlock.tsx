import { useEffect, useState } from "react";
import {
  type BundledLanguage,
  type Highlighter,
  bundledLanguages,
  createHighlighter,
} from "shiki";

/**
 * Singleton highlighter — one Shiki instance shared across all code blocks.
 * We preload a common set of languages on first use; uncommon ones are
 * loaded lazily via `loadLanguage`.
 */
let highlighterPromise: Promise<Highlighter> | null = null;
const PRELOADED_LANGS: BundledLanguage[] = [
  "typescript",
  "tsx",
  "javascript",
  "jsx",
  "rust",
  "python",
  "bash",
  "shell",
  "json",
  "yaml",
  "toml",
  "sql",
  "markdown",
  "css",
  "html",
  "go",
  "java",
  "c",
  "cpp",
  "diff",
  "powershell",
  "dockerfile",
];
const LOADED = new Set<string>(PRELOADED_LANGS);

function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ["github-dark-default"],
      langs: PRELOADED_LANGS,
    });
  }
  return highlighterPromise;
}

function normalizeLang(lang: string | undefined): string {
  const raw = (lang ?? "").toLowerCase().trim();
  if (!raw) return "text";
  const aliasMap: Record<string, string> = {
    ts: "typescript",
    js: "javascript",
    py: "python",
    sh: "bash",
    shell: "bash",
    rs: "rust",
    yml: "yaml",
    md: "markdown",
  };
  return aliasMap[raw] ?? raw;
}

interface Props {
  code: string;
  lang?: string | undefined;
  inline?: boolean;
}

export function CodeBlock({ code, lang, inline = false }: Props) {
  const normalized = normalizeLang(lang);
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const hl = await getHighlighter();
      if (
        normalized !== "text" &&
        !LOADED.has(normalized) &&
        normalized in bundledLanguages
      ) {
        try {
          await hl.loadLanguage(normalized as BundledLanguage);
          LOADED.add(normalized);
        } catch {
          /* fall through to plain rendering */
        }
      }
      const safeLang = LOADED.has(normalized) ? normalized : "text";
      const rendered = hl.codeToHtml(code, {
        lang: safeLang,
        theme: "github-dark-default",
      });
      if (!cancelled) setHtml(rendered);
    })();
    return () => {
      cancelled = true;
    };
  }, [code, normalized]);

  if (inline) {
    return (
      <code className="rounded bg-neutral-900 px-1 py-0.5 font-mono text-[90%] text-neutral-200">
        {code}
      </code>
    );
  }

  if (!html) {
    // Plain fallback while Shiki boots — keeps the layout stable.
    return (
      <pre className="overflow-x-auto rounded-md border border-neutral-800 bg-neutral-950 p-3 font-mono text-[12px] text-neutral-200">
        <code>{code}</code>
      </pre>
    );
  }

  return (
    <div
      className="overflow-x-auto rounded-md border border-neutral-800 text-[12px] [&>pre]:m-0 [&>pre]:p-3"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
