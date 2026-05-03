import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/**
 * Monaco-based side-by-side diff. Lazy-loads Monaco on first mount so the
 * 3-MB editor bundle stays out of the initial chunk.
 *
 * Why Monaco (vs the previous CodeMirror MergeView): Monaco's diff editor
 * is the same one VSCode ships, with first-class JetBrains-quality visuals
 * — strict 50/50 panes, word-level highlight, navigation, full syntax.
 */
export function MonacoDiffViewer({
  oldContent,
  newContent,
  path,
}: {
  oldContent: string | null;
  newContent: string | null;
  path: string;
}) {
  const { t } = useTranslation("git");
  const containerRef = useRef<HTMLDivElement | null>(null);
  const editorRef = useRef<unknown>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let editor: { dispose: () => void } | null = null;

    void (async () => {
      try {
        const { monaco, ensureIslandDarkTheme, monacoLanguageFor } =
          await import("~/lib/monaco-setup.ts");
        if (cancelled || !containerRef.current) return;

        ensureIslandDarkTheme();
        const language = monacoLanguageFor(path);

        const created = monaco.editor.createDiffEditor(containerRef.current, {
          theme: "oxyris-island-dark",
          renderSideBySide: true,
          readOnly: true,
          automaticLayout: true,
          fontFamily:
            '"JetBrains Mono", "Cascadia Code", ui-monospace, SFMono-Regular, Consolas, monospace',
          fontSize: 12,
          lineNumbers: "on",
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          renderWhitespace: "none",
          renderIndicators: true,
          // Word-level diff (the "Highlight words" toggle in JetBrains).
          diffWordWrap: "off",
          renderOverviewRuler: true,
          // Enable the inline gutter chevrons.
          renderMarginRevertIcon: false,
          // Smoothes layout when content height changes.
          ignoreTrimWhitespace: false,
          // Splitting ratio — Monaco respects this strictly.
          splitViewDefaultRatio: 0.5,
        });
        editor = created;
        editorRef.current = created;

        const originalModel = monaco.editor.createModel(
          oldContent ?? "",
          language,
        );
        const modifiedModel = monaco.editor.createModel(
          newContent ?? "",
          language,
        );
        created.setModel({
          original: originalModel,
          modified: modifiedModel,
        });

        // Dispose models when the diff editor is torn down to avoid leaks.
        const dispose = created.dispose.bind(created);
        created.dispose = () => {
          originalModel.dispose();
          modifiedModel.dispose();
          dispose();
        };
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    })();

    return () => {
      cancelled = true;
      if (editor) {
        editor.dispose();
      }
      editorRef.current = null;
    };
  }, [oldContent, newContent, path]);

  if (error) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-red-400">
        {error}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 w-full flex-1 flex-col">
      <div ref={containerRef} className="min-h-0 flex-1" />
    </div>
  );
}
