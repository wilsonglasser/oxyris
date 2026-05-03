import { useEffect, useRef } from "react";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { MergeView } from "@codemirror/merge";
import { oneDark } from "@codemirror/theme-one-dark";
import { languageForPath } from "~/lib/codemirror-language.ts";

interface Props {
  /** File before (null when newly added). */
  oldContent: string | null;
  /** File after (null when deleted). */
  newContent: string | null;
  /** Path / filename — used only for language detection. */
  path: string;
  /** Pane order; defaults to old-on-left, new-on-right. */
  orientation?: "a-b" | "b-a";
}

/**
 * Side-by-side diff. Two sync `EditorState`s wrapped in a `MergeView`. The
 * 50/50 split + per-pane scroll come from CSS in `index.css` (the merge
 * primitives don't expose a clean way to lock pane widths from JS).
 */
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

    const lang = languageForPath(path);
    const baseExtensions: Extension[] = [
      lineNumbers(),
      EditorView.editable.of(false),
      EditorView.lineWrapping,
      // Step 1: oneDark is a known-good full theme + matching highlight.
      // If syntax tokens DON'T appear with this, MergeView is overriding
      // syntax classes and we need a different strategy.
      oneDark,
    ];
    if (lang) baseExtensions.push(lang);

    const make = (doc: string) =>
      EditorState.create({ doc, extensions: baseExtensions });

    const mv = new MergeView({
      a: make(oldContent ?? ""),
      b: make(newContent ?? ""),
      parent: el,
      orientation: orientation === "a-b" ? "a-b" : "b-a",
      highlightChanges: true,
      gutter: true,
    });

    return () => {
      mv.destroy();
    };
  }, [oldContent, newContent, path, orientation]);

  return (
    <div
      ref={containerRef}
      className="oxyris-diff h-full min-h-0 w-full flex-1 overflow-hidden rounded-md border border-neutral-800 bg-neutral-950"
    />
  );
}
