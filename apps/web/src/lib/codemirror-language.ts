/**
 * Map a file path's extension to a CodeMirror `LanguageSupport` (which is
 * itself an `Extension`). Sync — all languages we ship are imported eagerly.
 *
 * Adding a new language: install `@codemirror/lang-<name>`, import it, add a
 * case below + extension(s) to the switch.
 */

import type { Extension } from "@codemirror/state";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { php } from "@codemirror/lang-php";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { sql } from "@codemirror/lang-sql";
import { yaml } from "@codemirror/lang-yaml";

export function languageForPath(path: string): Extension | null {
  const m = path.match(/\.([^./\\]+)$/);
  if (!m || !m[1]) return null;
  const ext = m[1].toLowerCase();
  switch (ext) {
    case "ts":
    case "tsx":
    case "mts":
    case "cts":
      return javascript({ typescript: true, jsx: true });
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
      return javascript({ jsx: true });
    case "rs":
      return rust();
    case "py":
    case "pyi":
      return python();
    case "json":
    case "jsonc":
      return json();
    case "html":
    case "htm":
    case "xhtml":
    case "vue":
    case "svelte":
      return html();
    case "css":
    case "scss":
    case "sass":
    case "less":
      return css();
    case "md":
    case "markdown":
    case "mdx":
      return markdown();
    case "php":
      return php();
    case "sql":
    case "mysql":
    case "pgsql":
      return sql();
    case "yaml":
    case "yml":
      return yaml();
    default:
      return null;
  }
}
