import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES, type SupportedLocale } from "~/i18n.ts";

export function LanguageSwitcher() {
  const { t, i18n } = useTranslation("common");
  const current = (i18n.resolvedLanguage ?? i18n.language) as SupportedLocale;

  return (
    <label className="flex items-center gap-2 text-xs text-neutral-400">
      <span>{t("language.label")}</span>
      <select
        value={current}
        onChange={(e) => void i18n.changeLanguage(e.target.value)}
        className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-neutral-200"
      >
        {SUPPORTED_LOCALES.map((lng) => (
          <option key={lng} value={lng}>
            {t(`language.${lng}`)}
          </option>
        ))}
      </select>
    </label>
  );
}
