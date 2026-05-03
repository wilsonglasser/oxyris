/**
 * Monaco runtime setup. Runs once on first import of any Monaco-using
 * component. Wires the editor web-worker via Vite's `?worker` import so
 * Monaco can spawn it without trying to fetch a CDN URL (we ship offline
 * inside Tauri).
 *
 * We don't load language workers (json/ts/css/html) — they only enable
 * IntelliSense, which we don't need for a read-only diff. Syntax highlight
 * works synchronously in the main thread via Monaco's Monarch grammars.
 */

import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";

declare global {
  interface Window {
    MonacoEnvironment?: monaco.Environment;
  }
}

if (typeof window !== "undefined" && !window.MonacoEnvironment) {
  window.MonacoEnvironment = {
    getWorker: () => new EditorWorker(),
  };
}

/**
 * JetBrains "Island Dark"-inspired Monaco theme. Token colors mirror the
 * palette in `lib/codemirror-theme.ts` so the diff and the editor feel
 * consistent.
 */
const ISLAND_DARK_THEME: monaco.editor.IStandaloneThemeData = {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "", foreground: "bcbec4" },
    { token: "comment", foreground: "7a7e85", fontStyle: "italic" },
    { token: "keyword", foreground: "cf8e6d" },
    { token: "keyword.control", foreground: "cf8e6d" },
    { token: "keyword.modifier", foreground: "cf8e6d" },
    { token: "keyword.flow", foreground: "cf8e6d" },
    { token: "keyword.json", foreground: "cf8e6d" },
    { token: "string", foreground: "6aab73" },
    { token: "string.key.json", foreground: "56a8f5" },
    { token: "string.value.json", foreground: "6aab73" },
    { token: "number", foreground: "d4a96a" },
    { token: "number.float", foreground: "d4a96a" },
    { token: "number.hex", foreground: "d4a96a" },
    { token: "constant", foreground: "d4a96a" },
    { token: "constant.language", foreground: "d4a96a" },
    { token: "boolean", foreground: "d4a96a" },
    { token: "type", foreground: "b08aff" },
    { token: "type.identifier", foreground: "b08aff" },
    { token: "identifier", foreground: "bcbec4" },
    { token: "function", foreground: "56a8f5" },
    { token: "method", foreground: "56a8f5" },
    { token: "variable", foreground: "bcbec4" },
    { token: "variable.predefined", foreground: "cf8e6d" },
    { token: "tag", foreground: "c77dbb" },
    { token: "attribute.name", foreground: "56a8f5" },
    { token: "attribute.value", foreground: "6aab73" },
    { token: "operator", foreground: "bcbec4" },
    { token: "delimiter", foreground: "bcbec4" },
    { token: "regexp", foreground: "2aacb8" },
    { token: "annotation", foreground: "d4a96a" },
    { token: "macro", foreground: "c77dbb" },
    { token: "namespace", foreground: "b08aff" },
  ],
  colors: {
    "editor.background": "#1e1f22",
    "editor.foreground": "#bcbec4",
    "editorLineNumber.foreground": "#7f848e",
    "editorLineNumber.activeForeground": "#bcbec4",
    "editor.selectionBackground": "#214283",
    "editor.inactiveSelectionBackground": "#214283",
    "editor.selectionHighlightBackground": "#3b475a",
    "editor.lineHighlightBackground": "#26282d",
    "editorCursor.foreground": "#e5e7eb",
    "editorIndentGuide.background": "#2b2d30",
    "editorIndentGuide.activeBackground": "#393b40",
    "editorWhitespace.foreground": "#393b40",
    // Diff-specific
    "diffEditor.insertedTextBackground": "#2e573a55",
    "diffEditor.removedTextBackground": "#5a323255",
    "diffEditor.insertedLineBackground": "#2c4a3220",
    "diffEditor.removedLineBackground": "#4a2e2e20",
    "diffEditor.diagonalFill": "#26282d",
    "diffEditorOverview.insertedForeground": "#3a8a4a",
    "diffEditorOverview.removedForeground": "#a04a4a",
    "diffEditorGutter.insertedLineBackground": "#3a8a4a30",
    "diffEditorGutter.removedLineBackground": "#a04a4a30",
    "scrollbar.shadow": "#00000000",
    "scrollbarSlider.background": "#393b4080",
    "scrollbarSlider.hoverBackground": "#4e5157aa",
    "scrollbarSlider.activeBackground": "#6f737aaa",
    "editorWidget.background": "#2b2d30",
    "editorWidget.border": "#393b40",
    "minimap.background": "#1e1f22",
  },
};

let themeInstalled = false;
export function ensureIslandDarkTheme(): void {
  if (themeInstalled) return;
  monaco.editor.defineTheme("oxyris-island-dark", ISLAND_DARK_THEME);
  themeInstalled = true;
}

/** Map a file path's extension to Monaco's language id. */
export function monacoLanguageFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  switch (ext) {
    case "ts":
    case "tsx":
    case "mts":
    case "cts":
      return "typescript";
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
      return "javascript";
    case "rs":
      return "rust";
    case "py":
    case "pyi":
      return "python";
    case "json":
    case "jsonc":
      return "json";
    case "html":
    case "htm":
    case "xhtml":
      return "html";
    case "css":
      return "css";
    case "scss":
      return "scss";
    case "less":
      return "less";
    case "md":
    case "markdown":
    case "mdx":
      return "markdown";
    case "yaml":
    case "yml":
      return "yaml";
    case "php":
      return "php";
    case "go":
      return "go";
    case "java":
      return "java";
    case "kt":
    case "kts":
      return "kotlin";
    case "swift":
      return "swift";
    case "rb":
      return "ruby";
    case "sh":
    case "bash":
    case "zsh":
      return "shell";
    case "ps1":
      return "powershell";
    case "sql":
      return "sql";
    case "toml":
      return "ini";
    case "xml":
    case "svg":
      return "xml";
    case "dockerfile":
      return "dockerfile";
    case "c":
    case "h":
      return "c";
    case "cpp":
    case "cc":
    case "cxx":
    case "hpp":
      return "cpp";
    default:
      return "plaintext";
  }
}

export { monaco };
