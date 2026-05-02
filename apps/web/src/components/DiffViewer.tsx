import { useEffect, useRef } from "react";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { MergeView } from "@codemirror/merge";
import { LanguageDescription, syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { javascript } from "@codemirror/lang-javascript";
import { rust } from "@codemirror/lang-rust";
import { python } from "@codemirror/lang-python";

/**
 * Map file extensions to CodeMirror language descriptions. We ship a handful
 * of common ones; unknown extensions render as plain text (still with line
 * numbers + the diff gutter).
 */
const LANG_MAP: Record<string, LanguageDescription> = {
  ts: LanguageDescription.of({
    name: "TypeScript",
    extensions: ["ts", "tsx"],
    load: async () => javascript({ typescript: true, jsx: true }),
  }),
  tsx: LanguageDescription.of({
    name: "TSX",
    extensions: ["tsx"],
    load: async () => javascript({ typescript: true, jsx: true }),
  }),
  js: LanguageDescription.of({
    name: "JavaScript",
    extensions: ["js", "jsx", "mjs", "cjs"],
    load: async () => javascript({ jsx: true }),
  }),
  jsx: LanguageDescription.of({
    name: "JSX",
    extensions: ["jsx"],
    load: async () => javascript({ jsx: true }),
  }),
  rs: LanguageDescription.of({
    name: "Rust",
    extensions: ["rs"],
    load: async () => rust(),
  }),
  py: LanguageDescription.of({
    name: "Python",
    extensions: ["py"],
    load: async () => python(),
  }),
};

interface Props {
  /** Arquivo antes do turno (pode ser null se é novo). */
  oldContent: string | null;
  /** Arquivo depois do turno (pode ser null se foi deletado). */
  newContent: string | null;
  /** Extensão ou nome do arquivo pra detectar linguagem. */
  path: string;
  /** Split horizontal vs inline unified diff. */
  orientation?: "a-b" | "b-a";
}

function languageForPath(path: string): LanguageDescription | null {
  const match = path.match(/\.([^./\\]+)$/);
  if (!match) return null;
  return LANG_MAP[match[1]!.toLowerCase()] ?? null;
}

export function DiffViewer({
  oldContent,
  newContent,
  path,
  orientation = "a-b",
}: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    let mv: MergeView | null = null;
    let cancelled = false;

    void (async () => {
      const langDesc = languageForPath(path);
      const langExt = langDesc ? await langDesc.load() : null;
      if (cancelled) return;

      const baseExtensions: Extension[] = [
        lineNumbers(),
        EditorView.editable.of(false),
        syntaxHighlighting(defaultHighlightStyle),
        EditorView.theme(
          {
            "&": {
              backgroundColor: "#0a0a0a",
              color: "#e5e5e5",
              fontSize: "12px",
              fontFamily:
                '"JetBrains Mono", "Cascadia Code", ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace',
            },
            ".cm-gutters": {
              backgroundColor: "#0a0a0a",
              color: "#525252",
              border: "none",
            },
            ".cm-activeLine, .cm-activeLineGutter": {
              backgroundColor: "transparent",
            },
          },
          { dark: true },
        ),
      ];
      if (langExt) baseExtensions.push(langExt);

      const make = (doc: string) =>
        EditorState.create({
          doc,
          extensions: baseExtensions,
        });

      mv = new MergeView({
        a: make(oldContent ?? ""),
        b: make(newContent ?? ""),
        parent: el,
        orientation: orientation === "a-b" ? "a-b" : "b-a",
        highlightChanges: true,
        gutter: true,
      });
    })();

    return () => {
      cancelled = true;
      mv?.destroy();
    };
  }, [oldContent, newContent, path, orientation]);

  return (
    <div
      ref={containerRef}
      className="overflow-hidden rounded-md border border-neutral-800 bg-neutral-950 [&_.cm-editor]:max-h-96 [&_.cm-editor]:overflow-auto"
    />
  );
}
