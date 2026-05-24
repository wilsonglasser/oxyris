import { useTranslation } from "react-i18next";
import { DownloadCloud } from "lucide-react";
import { useUpdaterStore } from "~/stores/updaterStore.ts";

/**
 * Non-blocking toast shown once the boot check finds a newer version. The
 * user picks the moment to update — clicking the action downloads, installs
 * and relaunches via the Tauri updater plugin. Dismiss just hides the toast
 * for this run (the badge in the sidebar still flags the pending update).
 */
export function UpdateBanner() {
  const { t } = useTranslation("common");
  const status = useUpdaterStore((s) => s.status);
  const applying = useUpdaterStore((s) => s.applying);
  const install = useUpdaterStore((s) => s.install);
  const clearDot = useUpdaterStore((s) => s.clearDot);

  if (status?.kind !== "available") return null;

  return (
    <div
      role="status"
      className="fixed bottom-4 left-4 z-50 max-w-sm rounded-lg border border-emerald-800/60 bg-neutral-900/95 px-4 py-3 text-[12px] text-neutral-200 shadow-xl shadow-black/50 backdrop-blur"
    >
      <div className="mb-1 flex items-center gap-1.5 font-medium text-neutral-100">
        <DownloadCloud className="size-3.5 text-emerald-400" strokeWidth={1.75} />
        {t("update_banner.title")}
      </div>
      <div className="text-neutral-400">
        {t("update_banner.body", { version: status.latest ?? "" })}
      </div>
      <div className="mt-2 flex items-center gap-3">
        <button
          type="button"
          onClick={() => void install()}
          disabled={applying}
          className="rounded bg-emerald-600 px-2 py-1 text-[11px] font-medium text-emerald-50 hover:bg-emerald-500 disabled:opacity-60"
        >
          {applying ? t("update_banner.applying") : t("update_banner.action")}
        </button>
        <button
          type="button"
          onClick={() => clearDot()}
          disabled={applying}
          className="text-[11px] text-neutral-500 hover:text-neutral-200 disabled:opacity-60"
        >
          {t("update_banner.dismiss")}
        </button>
      </div>
    </div>
  );
}
