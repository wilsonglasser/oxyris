import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  Download,
  Loader2,
  TriangleAlert,
  Trash2,
} from "lucide-react";
import {
  type LanguagePack,
  languagePacksInstall,
  languagePacksInstallInWsl,
  languagePacksList,
  languagePacksUninstall,
  onLanguagePackStatus,
  wslDistros,
} from "~/ipc/languagePacks.ts";

/**
 * Settings → Languages tab. "Plugin"-style UX over a static registry —
 * each language ships with its own LSP install pathway (GitHub release
 * for self-contained binaries; npm/bun global for Node-based servers).
 */
export function LanguagePacksPanel() {
  const { t } = useTranslation("settings");
  const [packs, setPacks] = useState<LanguagePack[]>([]);
  const [distros, setDistros] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const rows = await languagePacksList();
      setPacks(rows);
    } catch (e) {
      console.error("language_packs_list", e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void wslDistros()
      .then(setDistros)
      .catch((e) => {
        console.error("wsl_distros", e);
      });
  }, []);

  // Initial load + subscribe to live progress events to update statuses
  // without full re-fetch. We re-fetch on `done`/`failed` so source/path
  // resolves the new managed binary.
  useEffect(() => {
    void refresh();
    let unlisten: (() => void) | null = null;
    void onLanguagePackStatus((event) => {
      if (event.phase === "started") {
        setPacks((prev) =>
          prev.map((p) =>
            p.id === event.id
              ? { ...p, status: { kind: "installing", progress: 0 } }
              : p,
          ),
        );
      } else if (event.phase === "progress") {
        const pct = event.total
          ? Math.min(100, Math.round((event.bytes / event.total) * 100))
          : 0;
        setPacks((prev) =>
          prev.map((p) =>
            p.id === event.id
              ? { ...p, status: { kind: "installing", progress: pct } }
              : p,
          ),
        );
      } else {
        // done / failed — refetch so we get the canonical detected status
        // (managed vs path) and any updated install location.
        void refresh();
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [refresh]);

  return (
    <section className="space-y-3 px-4 py-4">
      <header>
        <h3 className="text-[12px] font-medium uppercase tracking-wider text-neutral-400">
          {t("languages.heading")}
        </h3>
        <p className="mt-1 text-[11px] text-neutral-500">
          {t("languages.description")}
        </p>
      </header>

      {loading ? (
        <p className="text-[11px] text-neutral-500">{t("languages.loading")}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {packs.map((pack) => (
            <PackRow
              key={pack.id}
              pack={pack}
              distros={distros}
              onChanged={() => void refresh()}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function PackRow({
  pack,
  distros,
  onChanged,
}: {
  pack: LanguagePack;
  distros: string[];
  onChanged: () => void;
}) {
  const { t } = useTranslation("settings");
  const [busy, setBusy] = useState(false);
  const [wslBusy, setWslBusy] = useState<string | null>(null);
  const [wslResult, setWslResult] = useState<
    { distro: string; ok: boolean; message: string } | null
  >(null);

  const onInstallInWsl = async (distro: string) => {
    setWslBusy(distro);
    setWslResult(null);
    try {
      const path = await languagePacksInstallInWsl(pack.id, distro);
      setWslResult({ distro, ok: true, message: path });
    } catch (e) {
      const msg = extractErrorMessage(e);
      setWslResult({ distro, ok: false, message: msg });
    } finally {
      setWslBusy(null);
    }
  };

  const onInstall = async () => {
    setBusy(true);
    try {
      await languagePacksInstall(pack.id);
    } catch (e) {
      console.error("install", e);
    } finally {
      setBusy(false);
    }
  };

  const onUninstall = async () => {
    if (
      !window.confirm(
        t("languages.uninstall_confirm", { name: pack.display_name }),
      )
    )
      return;
    setBusy(true);
    try {
      await languagePacksUninstall(pack.id);
      onChanged();
    } catch (e) {
      console.error("uninstall", e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="flex flex-col gap-2 rounded-lg border border-neutral-800 bg-neutral-900/40 px-3 py-2.5">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium text-neutral-100">
              {pack.display_name}
            </span>
            <StatusBadge pack={pack} />
          </div>
          <p className="mt-0.5 text-[11px] text-neutral-500">
            {pack.description}
          </p>
          <PathLine pack={pack} />
        </div>
        <Action pack={pack} busy={busy} onInstall={onInstall} onUninstall={onUninstall} />
      </div>
      {distros.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 border-t border-neutral-800/60 pt-2">
          <span className="text-[10px] uppercase tracking-wider text-neutral-500">
            {t("languages.wsl_install")}
          </span>
          {distros.map((distro) => {
            const isBusy = wslBusy === distro;
            return (
              <button
                key={distro}
                type="button"
                onClick={() => void onInstallInWsl(distro)}
                disabled={wslBusy !== null}
                className="inline-flex items-center gap-1 rounded-md border border-neutral-800 px-2 py-0.5 text-[10px] text-neutral-300 hover:bg-neutral-800 disabled:cursor-not-allowed disabled:opacity-60"
              >
                {isBusy ? (
                  <Loader2 className="size-3 animate-spin" strokeWidth={2} />
                ) : (
                  <Download className="size-3" strokeWidth={1.75} />
                )}
                {distro}
              </button>
            );
          })}
          {wslResult && (
            <span
              className={`text-[10px] ${
                wslResult.ok ? "text-emerald-300" : "text-red-300"
              }`}
              title={wslResult.message}
            >
              {wslResult.ok
                ? t("languages.wsl_install_ok", { distro: wslResult.distro })
                : t("languages.wsl_install_failed", { distro: wslResult.distro })}
            </span>
          )}
        </div>
      )}
      {wslResult && wslResult.ok && (
        <code className="block break-all rounded border border-emerald-900/40 bg-emerald-950/10 px-2 py-1 font-mono text-[10px] leading-snug text-emerald-200/90">
          {wslResult.message}
        </code>
      )}
      {wslResult && !wslResult.ok && (
        <pre className="whitespace-pre-wrap break-words rounded border border-red-900/40 bg-red-950/20 px-2 py-1.5 font-mono text-[10px] leading-snug text-red-200">
          {wslResult.message}
        </pre>
      )}
    </li>
  );
}

function StatusBadge({ pack }: { pack: LanguagePack }) {
  const { t } = useTranslation("settings");
  const s = pack.status;
  if (s.kind === "installed") {
    const label =
      s.source === "managed"
        ? t("languages.status.managed")
        : t("languages.status.path");
    return (
      <span className="inline-flex items-center gap-1 rounded border border-emerald-900/50 bg-emerald-950/20 px-1.5 py-0.5 text-[9px] uppercase tracking-wider text-emerald-300">
        <Check className="size-2.5" strokeWidth={2} />
        {label}
      </span>
    );
  }
  if (s.kind === "installing") {
    return (
      <span className="inline-flex items-center gap-1 rounded border border-neutral-700 bg-neutral-900 px-1.5 py-0.5 text-[9px] uppercase tracking-wider text-neutral-400">
        <Loader2 className="size-2.5 animate-spin" strokeWidth={1.75} />
        {t("languages.status.installing", { progress: s.progress })}
      </span>
    );
  }
  if (s.kind === "failed") {
    return (
      <span
        title={s.message}
        className="inline-flex items-center gap-1 rounded border border-red-900/50 bg-red-950/20 px-1.5 py-0.5 text-[9px] uppercase tracking-wider text-red-300"
      >
        <TriangleAlert className="size-2.5" strokeWidth={1.75} />
        {t("languages.status.failed")}
      </span>
    );
  }
  return null;
}

function PathLine({ pack }: { pack: LanguagePack }) {
  const s = pack.status;
  const wslLines = (pack.wsl_installs ?? []).map((w) => (
    <p
      key={`wsl:${w.distro}`}
      className="mt-0.5 truncate font-mono text-[10px] text-neutral-600"
    >
      <span className="text-emerald-500/80">[{w.distro}]</span> {w.path}
    </p>
  ));
  if (s.kind === "installed") {
    return (
      <>
        <p className="mt-0.5 truncate font-mono text-[10px] text-neutral-600">
          <span className="text-sky-500/80">[Windows]</span> {s.path}
        </p>
        {wslLines}
      </>
    );
  }
  if (s.kind === "failed") {
    return (
      <>
        <p className="mt-0.5 text-[11px] text-red-300">{s.message}</p>
        {wslLines}
      </>
    );
  }
  if (wslLines.length > 0) {
    return <>{wslLines}</>;
  }
  return null;
}

function Action({
  pack,
  busy,
  onInstall,
  onUninstall,
}: {
  pack: LanguagePack;
  busy: boolean;
  onInstall: () => void;
  onUninstall: () => void;
}) {
  const { t } = useTranslation("settings");
  const s = pack.status;
  const installing = s.kind === "installing" || busy;
  const installed = s.kind === "installed";
  const isManaged = installed && s.source === "managed";
  return (
    <div className="flex shrink-0 items-center gap-1">
      {!installed && (
        <button
          type="button"
          onClick={onInstall}
          disabled={installing}
          className="inline-flex items-center gap-1.5 rounded-md bg-neutral-200 px-3 py-1 text-[11px] font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-500"
        >
          {installing ? (
            <Loader2 className="size-3 animate-spin" strokeWidth={2} />
          ) : (
            <Download className="size-3" strokeWidth={2} />
          )}
          {t("languages.install")}
        </button>
      )}
      {isManaged && (
        <button
          type="button"
          onClick={onUninstall}
          disabled={busy}
          aria-label={t("languages.uninstall")}
          title={t("languages.uninstall")}
          className="flex size-7 items-center justify-center rounded text-neutral-500 hover:bg-red-950/40 hover:text-red-300"
        >
          <Trash2 className="size-3.5" strokeWidth={1.75} />
        </button>
      )}
      {installed && !isManaged && pack.install_method === "github_release" && (
        <button
          type="button"
          onClick={onInstall}
          disabled={installing}
          aria-label={t("languages.install_managed")}
          title={t("languages.install_managed")}
          className="inline-flex items-center gap-1 rounded-md border border-neutral-800 px-2 py-1 text-[10px] text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
        >
          <Download className="size-3" strokeWidth={1.75} />
          {t("languages.install_managed_short")}
        </button>
      )}
    </div>
  );
}

/** Tauri command errors come back as `{code, message}`, plain strings, or
 * Error objects (rejected JS promises). Reach into each shape so the user
 * sees the real reason instead of `[object Object]` or `undefined`. */
function extractErrorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object") {
    const anyE = e as { message?: unknown; code?: unknown };
    if (typeof anyE.message === "string" && anyE.message) return anyE.message;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }
  return String(e);
}
