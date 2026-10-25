import { useCallback, useEffect, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  LogIn,
  RefreshCw,
  Save,
  Trash2,
  UserRound,
} from "lucide-react";

import type { Environment } from "~/ipc/commands.ts";
import {
  type AccountCard,
  accountsActivate,
  accountsErrorMessage,
  accountsLogin,
  accountsRemove,
  accountsSaveLive,
  accountsTrack,
  type EnvBoard,
  type EnvironmentEntry,
  envKey,
  environmentList,
  sessionsRunningCount,
  wslOfflineDistro,
  type UsageError,
  type UsageWindow,
  usageRefresh,
} from "~/ipc/claudeAccounts.ts";
import { localEnvLabel } from "~/lib/host.ts";
import { useClaudeUsageStore } from "~/stores/claudeUsageStore.ts";
import { useUsageWidgetSettingsStore } from "~/stores/usageWidgetSettingsStore.ts";

/**
 * Settings → Accounts. One section per environment (Windows, each WSL
 * distro) because each keeps its own Claude login and its own reserve pool.
 * Data comes from the backend usage board; every action returns the rebuilt
 * board, so the UI never waits for the next poll.
 */
export function ClaudeAccountsPanel() {
  const { t } = useTranslation("settings");
  const board = useClaudeUsageStore((s) => s.board);
  const setBoard = useClaudeUsageStore((s) => s.setBoard);
  const init = useClaudeUsageStore((s) => s.init);
  const [envs, setEnvs] = useState<EnvironmentEntry[]>([]);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    init();
    void environmentList()
      .then(setEnvs)
      .catch(() => setEnvs([{ kind: "local", state: null, version: null, is_default: true }]));
  }, [init]);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      setBoard(await usageRefresh());
    } finally {
      setRefreshing(false);
    }
  }, [setBoard]);

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <p className="text-[11px] leading-relaxed text-neutral-400">
          {t("accounts.intro")}
        </p>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={refreshing}
          className="flex shrink-0 items-center gap-1.5 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
        >
          <RefreshCw className={`size-3 ${refreshing ? "animate-spin" : ""}`} />
          {t("accounts.refresh")}
        </button>
      </div>
      <WidgetSettingsSection envs={envs} />
      {envs.map((entry) => {
        const env: Environment =
          entry.kind === "local" ? { kind: "local" } : { kind: "wsl", distro: entry.distro };
        const key = envKey(env);
        return (
          <EnvSection
            key={key}
            env={env}
            board={board?.environments.find((b) => b.key === key) ?? null}
          />
        );
      })}
    </div>
  );
}

/** Taskbar widget: on/off and which distro fills the WSL row. */
function WidgetSettingsSection({ envs }: { envs: EnvironmentEntry[] }) {
  const { t } = useTranslation("settings");
  const settings = useUsageWidgetSettingsStore((s) => s.settings);
  const init = useUsageWidgetSettingsStore((s) => s.init);
  const update = useUsageWidgetSettingsStore((s) => s.update);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => init(), [init]);
  if (!settings) return null;

  const distros = envs.flatMap((e) => (e.kind === "wsl" ? [e.distro] : []));
  const save = (patch: Parameters<typeof update>[0]) => {
    setError(null);
    void update(patch).catch((e: unknown) => setError(accountsErrorMessage(e)));
  };

  return (
    <section className="rounded-md border border-neutral-800 px-3 py-2.5">
      <h3 className="mb-2 text-[11px] font-medium uppercase tracking-wider text-neutral-500">
        {t("accounts.widget.heading")}
      </h3>
      <label className="flex items-center gap-2 text-xs text-neutral-200">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(e) => save({ enabled: e.target.checked })}
        />
        {t("accounts.widget.enabled")}
      </label>
      {distros.length > 0 && (
        <label className="mt-2 flex items-center gap-2 text-xs text-neutral-300">
          {t("accounts.widget.distro")}
          <select
            value={settings.wsl_distro ?? ""}
            onChange={(e) => save({ wsl_distro: e.target.value || null })}
            className="rounded border border-neutral-700 bg-neutral-900 px-1.5 py-0.5 text-xs text-neutral-200"
          >
            <option value="">{t("accounts.widget.distro_auto")}</option>
            {distros.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
      )}
      <p className="mt-2 text-[11px] text-neutral-500">{t("accounts.widget.hint")}</p>
      {error && (
        <p className="mt-1 text-[11px] text-red-300">{t("error_label", { message: error })}</p>
      )}
    </section>
  );
}

function envLabel(env: Environment, t: TFunction<"settings">): string {
  return env.kind === "local" ? localEnvLabel() : t("env_wsl", { distro: env.distro });
}

/** User-facing text for a failed account action. */
function actionError(e: unknown, t: TFunction<"settings">): string {
  const distro = wslOfflineDistro(e);
  return distro ? t("accounts.wsl_offline", { distro }) : accountsErrorMessage(e);
}

function EnvSection({ env, board }: { env: Environment; board: EnvBoard | null }) {
  const { t } = useTranslation("settings");
  const setBoard = useClaudeUsageStore((s) => s.setBoard);
  const [busy, setBusy] = useState<"track" | "save" | "login" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const run = useCallback(
    async (kind: "track" | "save" | "login", fn: () => Promise<void>) => {
      setBusy(kind);
      setError(null);
      setNotice(null);
      try {
        await fn();
      } catch (e) {
        setError(actionError(e, t));
      } finally {
        setBusy(null);
      }
    },
    [t],
  );

  const unregisteredLive = board?.accounts.find((a) => a.active && a.id === null);
  const label = envLabel(env, t);

  return (
    <section>
      <header className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h3 className="flex items-center gap-2 text-[11px] font-medium uppercase tracking-wider text-neutral-500">
          {label}
          {board?.offline && (
            <span
              title={t("accounts.offline_hint")}
              className="rounded-full border border-neutral-700 px-1.5 py-px text-[10px] normal-case tracking-normal text-neutral-400"
            >
              {t("accounts.offline")}
            </span>
          )}
        </h3>
        {board && (
          <div className="flex gap-1.5">
            {unregisteredLive && (
              <button
                type="button"
                disabled={busy !== null}
                onClick={() =>
                  void run("save", async () => setBoard(await accountsSaveLive(env)))
                }
                className="flex items-center gap-1.5 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
              >
                <Save className="size-3" />
                {t("accounts.save_current")}
              </button>
            )}
            <button
              type="button"
              disabled={busy !== null}
              onClick={() =>
                void run("login", async () => {
                  const record = await accountsLogin(env);
                  setNotice(
                    record
                      ? t("accounts.login_added", { email: record.identity.email })
                      : t("accounts.login_cancelled"),
                  );
                })
              }
              className="flex items-center gap-1.5 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
            >
              <LogIn className="size-3" />
              {busy === "login" ? t("accounts.login_waiting") : t("accounts.add")}
            </button>
          </div>
        )}
      </header>

      {!board ? (
        <div className="flex items-center justify-between rounded-md border border-neutral-800 px-3 py-2 text-[11px] text-neutral-400">
          <span>{t("accounts.not_tracked")}</span>
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void run("track", async () => setBoard(await accountsTrack(env)))}
            className="rounded border border-neutral-700 px-2 py-1 text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
          >
            {busy === "track" ? t("accounts.loading") : t("accounts.track")}
          </button>
        </div>
      ) : (
        <>
          {board.live_error && (
            <p className="mb-2 text-[11px] text-red-300">
              {t("error_label", { message: board.live_error })}
            </p>
          )}
          {board.accounts.length === 0 ? (
            <p className="rounded-md border border-dashed border-neutral-800 px-3 py-3 text-[11px] text-neutral-500">
              {t("accounts.empty")}
            </p>
          ) : (
            <ul className="space-y-2">
              {board.accounts.map((card) => (
                <AccountCardView
                  key={card.id ?? `live:${card.identity.email}`}
                  env={env}
                  card={card}
                  onError={setError}
                  onSwitched={() => setNotice(t("accounts.switched", { env: label }))}
                />
              ))}
            </ul>
          )}
        </>
      )}
      {busy === "login" && (
        <p className="mt-2 text-[11px] text-neutral-400">{t("accounts.login_hint")}</p>
      )}
      {notice && <p className="mt-2 text-[11px] text-amber-200">{notice}</p>}
      {error && (
        <p className="mt-2 text-[11px] text-red-300">{t("error_label", { message: error })}</p>
      )}
    </section>
  );
}

function AccountCardView({
  env,
  card,
  onError,
  onSwitched,
}: {
  env: Environment;
  card: AccountCard;
  onError: (msg: string | null) => void;
  onSwitched: () => void;
}) {
  const { t } = useTranslation("settings");
  const setBoard = useClaudeUsageStore((s) => s.setBoard);
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  /** Running-session count awaiting a "switch anyway" confirmation. */
  const [confirmSwitch, setConfirmSwitch] = useState<number | null>(null);
  const usage = card.usage?.usage ?? null;
  const usageError = card.usage?.error ?? null;
  const org = card.identity.organization_name;

  const act = async (fn: () => Promise<void>) => {
    setBusy(true);
    onError(null);
    try {
      await fn();
    } catch (e) {
      onError(actionError(e, t));
    } finally {
      setBusy(false);
    }
  };

  const activate = (force: boolean) =>
    act(async () => {
      try {
        setBoard(await accountsActivate(env, card.id as string, force));
        setConfirmSwitch(null);
        onSwitched();
      } catch (e) {
        const running = sessionsRunningCount(e);
        if (running === null) throw e;
        setConfirmSwitch(running);
      }
    });

  return (
    <li
      className={`rounded-md border px-3 py-2.5 ${
        card.active ? "border-emerald-900/60 bg-emerald-950/15" : "border-neutral-800 bg-neutral-950/40"
      }`}
    >
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <UserRound className="size-3.5 text-neutral-500" />
        <span className="font-medium text-neutral-100">{card.identity.email}</span>
        <span className="rounded-full border border-neutral-700 px-2 py-0.5 text-[10px] text-neutral-300">
          {t(`accounts.plan.${card.plan}`)}
        </span>
        {org && <span className="truncate text-[11px] text-neutral-500">{org}</span>}
        <div className="ml-auto flex items-center gap-1.5">
          {card.active ? (
            <span className="flex items-center gap-1 text-[11px] text-emerald-300">
              <CheckCircle2 className="size-3.5" />
              {t("accounts.active")}
            </span>
          ) : (
            card.id && (
              <button
                type="button"
                disabled={busy}
                onClick={() => void activate(false)}
                className="rounded border border-emerald-800/70 bg-emerald-900/30 px-2 py-0.5 text-[11px] text-emerald-200 hover:bg-emerald-900/50 disabled:opacity-50"
              >
                {busy ? t("accounts.switching") : t("accounts.activate")}
              </button>
            )
          )}
          {card.id &&
            (confirmRemove ? (
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void act(async () => setBoard(await accountsRemove(env, card.id as string)))
                }
                onBlur={() => setConfirmRemove(false)}
                className="rounded border border-red-900/70 bg-red-950/40 px-2 py-0.5 text-[11px] text-red-200 hover:bg-red-900/40"
              >
                {t("accounts.remove_confirm")}
              </button>
            ) : (
              <button
                type="button"
                title={t("accounts.remove")}
                aria-label={t("accounts.remove")}
                onClick={() => setConfirmRemove(true)}
                className="rounded p-1 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-300"
              >
                <Trash2 className="size-3.5" />
              </button>
            ))}
        </div>
      </div>

      {confirmSwitch !== null && (
        <div className="mt-2 flex flex-wrap items-center gap-2 rounded border border-amber-900/60 bg-amber-950/30 px-2.5 py-1.5 text-[11px] text-amber-200">
          <span className="flex-1">
            {t("accounts.sessions_running", { count: confirmSwitch })}
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={() => void activate(true)}
            className="rounded border border-amber-800/70 px-2 py-0.5 hover:bg-amber-900/40 disabled:opacity-50"
          >
            {t("accounts.switch_anyway")}
          </button>
          <button
            type="button"
            onClick={() => setConfirmSwitch(null)}
            className="rounded px-2 py-0.5 text-amber-300/80 hover:bg-amber-900/30"
          >
            {t("accounts.cancel")}
          </button>
        </div>
      )}
      {usage && (
        <div className="mt-2.5 grid gap-1.5">
          <UsageBar label={t("accounts.window_5h")} window={usage.five_hour} />
          <UsageBar label={t("accounts.window_7d")} window={usage.seven_day} />
          {usage.seven_day_opus && (
            <UsageBar label={t("accounts.window_7d_opus")} window={usage.seven_day_opus} />
          )}
          {usage.seven_day_sonnet && (
            <UsageBar label={t("accounts.window_7d_sonnet")} window={usage.seven_day_sonnet} />
          )}
        </div>
      )}
      {usageError && (
        <p className="mt-1.5 text-[11px] text-amber-300">{usageErrorText(usageError, t)}</p>
      )}
      {!usage && !usageError && (
        <p className="mt-1.5 text-[11px] text-neutral-500">{t("accounts.loading")}</p>
      )}
    </li>
  );
}

function usageErrorText(err: UsageError, t: TFunction<"settings">): string {
  switch (err.kind) {
    case "no_scope":
      return t("accounts.error.no_scope");
    case "auth":
      return t("accounts.error.auth");
    case "rate_limited":
      return t("accounts.error.rate_limited");
    case "network":
      return t("accounts.error.network", { message: err.message });
    case "http":
      return t("accounts.error.http", { message: err.message });
    case "credentials":
      return t("accounts.error.credentials", { message: err.message });
  }
}

/** Re-render every 30s so reset countdowns stay current. */
function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}

export function formatResetIn(resetsAt: string | null, now: number): string | null {
  if (!resetsAt) return null;
  const ms = Date.parse(resetsAt) - now;
  if (!Number.isFinite(ms)) return null;
  const mins = Math.max(0, Math.round(ms / 60_000));
  const d = Math.floor(mins / 1440);
  const h = Math.floor((mins % 1440) / 60);
  const m = mins % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

export function usageTone(pct: number): string {
  if (pct >= 90) return "bg-red-500";
  if (pct >= 70) return "bg-amber-500";
  return "bg-emerald-500";
}

function UsageBar({ label, window: w }: { label: string; window: UsageWindow | null }) {
  const { t } = useTranslation("settings");
  const now = useNow();
  if (!w) {
    return (
      <div className="flex items-center gap-2 text-[11px] text-neutral-500">
        <span className="w-24 shrink-0">{label}</span>
        <span>—</span>
      </div>
    );
  }
  const pct = Math.max(0, Math.min(100, w.utilization));
  const reset = formatResetIn(w.resets_at, now);
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className="w-24 shrink-0 text-neutral-400">{label}</span>
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-neutral-800">
        <div className={`h-full rounded-full ${usageTone(pct)}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="w-10 shrink-0 text-right font-mono text-neutral-200">
        {Math.round(w.utilization)}%
      </span>
      <span className="w-28 shrink-0 text-right text-neutral-500">
        {reset ? t("accounts.resets_in", { time: reset }) : ""}
      </span>
    </div>
  );
}
