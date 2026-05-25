/**
 * The language Claude is told to respond in — a separate axis from the UI
 * locale (see {@link import("~/components/LanguageSwitcher").LanguageSwitcher}).
 * The values are plain codes that only ever feed a natural-language
 * instruction, never i18n lookups, so they don't have to be valid BCP-47.
 * `"auto"` means "mirror whatever language the user writes in".
 */
export type ClaudeLanguage =
  | "auto"
  | "en"
  | "pt-BR"
  | "es"
  | "fr"
  | "de"
  | "it"
  | "zh"
  | "ja"
  | "ko"
  | "ru";

/**
 * Options for the settings dropdown. `label` is the language's own endonym so
 * the list reads the same in every UI locale; `"auto"` is the one entry whose
 * label is translated at render time via the `settings` namespace.
 */
export const CLAUDE_LANGUAGES: { code: ClaudeLanguage; label: string }[] = [
  { code: "auto", label: "" },
  { code: "en", label: "English" },
  { code: "pt-BR", label: "Português (Brasil)" },
  { code: "es", label: "Español" },
  { code: "fr", label: "Français" },
  { code: "de", label: "Deutsch" },
  { code: "it", label: "Italiano" },
  { code: "zh", label: "中文" },
  { code: "ja", label: "日本語" },
  { code: "ko", label: "한국어" },
  { code: "ru", label: "Русский" },
];

/** English names used to build the model-facing directive. */
const NAMES: Record<Exclude<ClaudeLanguage, "auto">, string> = {
  en: "English",
  "pt-BR": "Brazilian Portuguese",
  es: "Spanish",
  fr: "French",
  de: "German",
  it: "Italian",
  zh: "Chinese",
  ja: "Japanese",
  ko: "Korean",
  ru: "Russian",
};

export function isClaudeLanguage(v: unknown): v is ClaudeLanguage {
  return typeof v === "string" && CLAUDE_LANGUAGES.some((l) => l.code === v);
}

/**
 * Build the system-prompt fragment that pins Claude's response language. It is
 * appended to whatever else we feed the session (e.g. the MCP tool nudge).
 *
 * Our system prompt, this repo's code, and its docs are overwhelmingly English,
 * which drags responses toward English even when the user writes in another
 * language. So even `"auto"` emits an explicit instruction to counteract that
 * pull rather than leaving the choice to context inertia.
 *
 * This is model-facing prompt text, never shown in the UI — it stays in
 * English and is intentionally not run through i18n.
 */
export function claudeLanguageDirective(lang: ClaudeLanguage): string {
  if (lang === "auto") {
    return (
      "Always respond in the same language the user writes their messages in. " +
      "Decide the reply language from the user's own words only — ignore the " +
      "language of the surrounding code, file contents, documentation, and " +
      "tool output. Match the user, not the context."
    );
  }
  return (
    `Always respond in ${NAMES[lang]}, regardless of the language of the code, ` +
    "file contents, documentation, or surrounding context. Keep code " +
    "identifiers and technical terms that have no natural translation as-is."
  );
}
