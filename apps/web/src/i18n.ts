import i18n from "i18next";
import LanguageDetector from "i18next-browser-languagedetector";
import { initReactI18next } from "react-i18next";

import enActions from "~/locales/en/actions.json";
import enChat from "~/locales/en/chat.json";
import enCommon from "~/locales/en/common.json";
import enFiles from "~/locales/en/files.json";
import enGit from "~/locales/en/git.json";
import enProject from "~/locales/en/project.json";
import enSettings from "~/locales/en/settings.json";
import ptBRActions from "~/locales/pt-BR/actions.json";
import ptBRChat from "~/locales/pt-BR/chat.json";
import ptBRCommon from "~/locales/pt-BR/common.json";
import ptBRFiles from "~/locales/pt-BR/files.json";
import ptBRGit from "~/locales/pt-BR/git.json";
import ptBRProject from "~/locales/pt-BR/project.json";
import ptBRSettings from "~/locales/pt-BR/settings.json";

// English is the canonical/base locale. New locales are added by dropping a
// folder under `~/locales/<bcp47>/<namespace>.json` and registering it below.
//
// We use namespaces (e.g. `common`, `project`, future `chat`, `settings`) so
// each feature owns its strings without one giant JSON.
export const SUPPORTED_LOCALES = ["en", "pt-BR"] as const;
export type SupportedLocale = (typeof SUPPORTED_LOCALES)[number];
export const DEFAULT_LOCALE: SupportedLocale = "en";

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: DEFAULT_LOCALE,
    supportedLngs: SUPPORTED_LOCALES as unknown as string[],
    defaultNS: "common",
    ns: ["common", "project", "chat", "settings", "files", "git", "actions"],
    resources: {
      en: {
        common: enCommon,
        project: enProject,
        chat: enChat,
        settings: enSettings,
        files: enFiles,
        git: enGit,
        actions: enActions,
      },
      "pt-BR": {
        common: ptBRCommon,
        project: ptBRProject,
        chat: ptBRChat,
        settings: ptBRSettings,
        files: ptBRFiles,
        git: ptBRGit,
        actions: ptBRActions,
      },
    },
    interpolation: { escapeValue: false },
    detection: {
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
    },
  });

export default i18n;
