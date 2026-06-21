import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Smartphone, X } from "lucide-react";
import {
  type MobileInfo,
  mobileTakeoverStart,
  mobileTakeoverStatus,
  mobileTakeoverStop,
} from "~/ipc/mobile.ts";

/**
 * Pairing dialog for the mobile-takeover server. Shows the on/off state, a QR
 * the phone scans, and the raw URL as a fallback. The server is global (it
 * serves every pure session), so this is just the on-ramp — the actual takeover
 * happens on the phone, and the desktop pure panel freezes via its own
 * `session:<id>:takeover` listener.
 */
export function MobileTakeoverModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation("chat");
  const [info, setInfo] = useState<MobileInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void mobileTakeoverStatus()
      .then(setInfo)
      .catch(() => {});
  }, []);

  const start = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setInfo(await mobileTakeoverStart());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setBusy(true);
    try {
      setInfo(await mobileTakeoverStop());
    } finally {
      setBusy(false);
    }
  }, []);

  const running = info?.running ?? false;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-sm rounded-xl border border-neutral-800 bg-neutral-900 p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-1 flex items-center gap-2">
          <Smartphone className="size-4 text-sky-400" strokeWidth={1.75} />
          <h2 className="text-sm font-medium text-neutral-100">
            {t("mobile_title")}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("mobile_close")}
            className="ml-auto text-neutral-500 hover:text-neutral-200"
          >
            <X className="size-4" strokeWidth={2} />
          </button>
        </div>
        <p className="mb-4 text-[11px] text-neutral-500">{t("mobile_desc")}</p>

        {error && (
          <p className="mb-3 rounded border border-red-900/60 bg-red-950/30 px-2 py-1 text-[11px] text-red-200">
            {t("mobile_error", { message: error })}
          </p>
        )}

        {running && info ? (
          <div className="flex flex-col items-center gap-3">
            <span className="inline-flex items-center gap-1.5 rounded-full bg-emerald-500/15 px-2.5 py-0.5 text-[11px] text-emerald-300">
              <span className="size-1.5 rounded-full bg-emerald-400" />
              {t("mobile_running")}
            </span>
            {info.qr_svg && (
              <div
                className="rounded-lg bg-white p-3"
                // The QR SVG comes from our own backend (qrcode crate) — safe to
                // inline. It encodes the pairing URL + token.
                dangerouslySetInnerHTML={{ __html: info.qr_svg }}
              />
            )}
            <p className="text-center text-[11px] text-neutral-400">
              {t("mobile_scan")}
            </p>
            <p className="w-full text-center text-[10px] text-neutral-500">
              {t("mobile_url_hint")}
            </p>
            <code className="w-full break-all rounded bg-neutral-950 px-2 py-1 text-center text-[10px] text-neutral-300">
              {info.url}
            </code>
            <button
              type="button"
              onClick={() => void stop()}
              disabled={busy}
              className="mt-1 w-full rounded border border-neutral-700 px-3 py-1.5 text-[12px] text-neutral-200 hover:bg-neutral-800 disabled:opacity-50"
            >
              {t("mobile_stop")}
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={() => void start()}
            disabled={busy}
            className="flex w-full items-center justify-center gap-2 rounded bg-neutral-200 px-3 py-1.5 text-[12px] font-medium text-neutral-900 hover:bg-white disabled:opacity-50"
          >
            {busy && <Loader2 className="size-3.5 animate-spin" />}
            {busy ? t("mobile_starting") : t("mobile_start")}
          </button>
        )}
      </div>
    </div>
  );
}
