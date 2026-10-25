import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  onWidgetClicked,
  onWidgetMetrics,
  widgetMetrics,
  widgetPaint,
  type WidgetMetrics,
} from "~/ipc/usageWidget.ts";
import { renderWidget, widgetRows } from "~/lib/usageWidgetPainter.ts";
import { useClaudeUsageStore } from "~/stores/claudeUsageStore.ts";
import { useUsageWidgetSettingsStore } from "~/stores/usageWidgetSettingsStore.ts";

/**
 * Keeps the native taskbar widget painted: re-renders on every usage board,
 * widget geometry/theme change and widget-settings change. Event-driven only
 * (a minimized webview throttles timers but still gets events). `onClick`
 * fires when the widget is clicked — the backend already raised the window.
 */
export function useUsageWidget(onClick: () => void) {
  const { t } = useTranslation("settings");
  const board = useClaudeUsageStore((s) => s.board);
  const initBoard = useClaudeUsageStore((s) => s.init);
  const settings = useUsageWidgetSettingsStore((s) => s.settings);
  const initSettings = useUsageWidgetSettingsStore((s) => s.init);
  const [metrics, setMetrics] = useState<WidgetMetrics | null>(null);

  useEffect(() => {
    initBoard();
    initSettings();
    void widgetMetrics()
      .then((m) => m && setMetrics(m))
      .catch(() => {});
    const unMetrics = onWidgetMetrics(setMetrics);
    const unClick = onWidgetClicked(onClick);
    return () => {
      void unMetrics.then((fn) => fn());
      void unClick.then((fn) => fn());
    };
  }, [initBoard, initSettings, onClick]);

  useEffect(() => {
    if (!metrics || !settings?.enabled) return;
    const rows = widgetRows(board, settings, {
      local: t("accounts.widget.row_local"),
      wsl: t("accounts.widget.row_wsl"),
    });
    const image = renderWidget(rows, metrics);
    if (image) void widgetPaint(image.data, image.width, image.height).catch(() => {});
  }, [board, metrics, settings, t]);
}
