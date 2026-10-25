import { create } from "zustand";

import { type WidgetSettings, widgetSetSettings, widgetSettings } from "~/ipc/usageWidget.ts";

/** Taskbar widget settings; persisted backend-side so the widget starts at boot. */
interface State {
  settings: WidgetSettings | null;
  init: () => void;
  update: (patch: Partial<WidgetSettings>) => Promise<void>;
}

let loaded = false;

export const useUsageWidgetSettingsStore = create<State>((set, get) => ({
  settings: null,
  init: () => {
    if (loaded) return;
    loaded = true;
    void widgetSettings()
      .then((settings) => set({ settings }))
      .catch(() => {});
  },
  update: async (patch) => {
    const current = get().settings ?? { enabled: true, wsl_distro: null };
    const next = { ...current, ...patch };
    await widgetSetSettings(next);
    set({ settings: next });
  },
}));
