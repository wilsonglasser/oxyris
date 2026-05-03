/**
 * Shared CodeMirror theme: JetBrains "Island Dark"-inspired palette.
 *
 * Two pieces:
 *   - `islandDarkTheme`: editor chrome (background, gutter, selection)
 *   - `islandDarkHighlight`: syntax-token colors (keyword, string, etc.)
 *
 * Both are exported individually + together as `islandDark` so callers can
 * mix-and-match (e.g. the diff viewer wants the highlight without the full
 * theme background, since its container already paints the surface).
 *
 * Tag list mirrors `@codemirror/theme-one-dark` so language packs that ship
 * standard `@lezer/highlight` tags get full coverage.
 */

import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { tags as t } from "@lezer/highlight";

// JetBrains Island Dark palette anchors.
const bg = "#1e1f22";
const surface = "#2b2d30";
const text = "#bcbec4";
const muted = "#7f848e";
const cursor = "#e5e7eb";
const selection = "rgba(33, 66, 131, 0.6)";

// Token colors (close to JetBrains "Darcula"/"New UI Dark" defaults).
const orange = "#cf8e6d"; // keywords
const purple = "#b08aff"; // types
const green = "#6aab73"; // strings
const yellow = "#d4a96a"; // numbers / constants
const blue = "#56a8f5"; // function names / properties
const pink = "#c77dbb"; // tag-like, special vars
const cyan = "#2aacb8"; // operators / regex / escapes
const commentGray = "#7a7e85";
const errorRed = "#ff6b6b";

export const islandDarkTheme: Extension = EditorView.theme(
  {
    "&": {
      backgroundColor: bg,
      color: text,
      height: "100%",
    },
    ".cm-content": {
      caretColor: cursor,
      fontFamily:
        '"JetBrains Mono", "Cascadia Code", ui-monospace, SFMono-Regular, Consolas, monospace',
      fontSize: "12.5px",
    },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: cursor },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
      {
        backgroundColor: selection,
      },
    ".cm-activeLine": { backgroundColor: "rgba(255,255,255,0.04)" },
    ".cm-activeLineGutter": { backgroundColor: "rgba(255,255,255,0.05)" },
    ".cm-gutters": {
      backgroundColor: bg,
      color: muted,
      borderRight: `1px solid ${surface}`,
    },
    ".cm-lineNumbers .cm-gutterElement": { color: muted },
    ".cm-foldPlaceholder": {
      backgroundColor: surface,
      border: "none",
      color: text,
    },
    ".cm-tooltip": {
      backgroundColor: surface,
      border: `1px solid ${surface}`,
    },
    ".cm-panels": { backgroundColor: surface, color: text },
    ".cm-searchMatch": { backgroundColor: "rgba(255,255,0,0.20)" },
    ".cm-selectionMatch": { backgroundColor: "rgba(255,255,255,0.07)" },
  },
  { dark: true },
);

export const islandDarkHighlight = HighlightStyle.define([
  // keywords + control flow
  { tag: t.keyword, color: orange },
  { tag: t.controlKeyword, color: orange },
  { tag: t.modifier, color: orange },
  { tag: t.operatorKeyword, color: orange },
  { tag: t.definitionKeyword, color: orange },
  { tag: t.moduleKeyword, color: orange },
  { tag: t.self, color: orange, fontStyle: "italic" },

  // strings + escapes
  { tag: [t.string, t.special(t.string)], color: green },
  { tag: t.escape, color: cyan },
  { tag: t.regexp, color: cyan },

  // numbers / atoms / constants
  { tag: [t.number, t.bool, t.null, t.atom], color: yellow },
  { tag: t.constant(t.name), color: yellow },

  // identifiers
  { tag: t.variableName, color: text },
  { tag: t.definition(t.variableName), color: text },
  { tag: t.function(t.variableName), color: blue },
  { tag: t.function(t.definition(t.variableName)), color: blue },
  { tag: t.propertyName, color: blue },
  { tag: t.function(t.propertyName), color: blue },
  { tag: t.labelName, color: blue },

  // types / classes / namespaces
  { tag: [t.typeName, t.className, t.namespace], color: purple },
  { tag: t.standard(t.typeName), color: purple },

  // tags / attributes (HTML/JSX)
  { tag: t.tagName, color: pink },
  { tag: t.attributeName, color: blue },
  { tag: t.attributeValue, color: green },

  // operators / punctuation
  { tag: t.operator, color: text },
  { tag: t.punctuation, color: text },
  { tag: t.bracket, color: text },
  { tag: t.separator, color: text },

  // comments / meta
  { tag: t.comment, color: commentGray, fontStyle: "italic" },
  { tag: t.lineComment, color: commentGray, fontStyle: "italic" },
  { tag: t.blockComment, color: commentGray, fontStyle: "italic" },
  { tag: t.docComment, color: commentGray, fontStyle: "italic" },
  { tag: t.meta, color: muted },
  { tag: t.annotation, color: yellow },

  // markdown
  { tag: t.heading, color: blue, fontWeight: "bold" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: t.link, color: blue, textDecoration: "underline" },
  { tag: t.url, color: cyan, textDecoration: "underline" },

  // diff status
  { tag: t.deleted, color: errorRed },
  { tag: t.inserted, color: green },
  { tag: t.changed, color: yellow },
  { tag: t.invalid, color: errorRed, textDecoration: "underline" },
]);

/** Convenience bundle when callers want the editor chrome + colors together. */
export const islandDark: Extension[] = [
  islandDarkTheme,
  syntaxHighlighting(islandDarkHighlight, { fallback: true }),
];

/**
 * Highlight-only bundle for surfaces (like the diff viewer container) that
 * already supply their own background/gutter styling.
 */
export const islandDarkHighlightOnly: Extension = syntaxHighlighting(
  islandDarkHighlight,
  { fallback: true },
);
