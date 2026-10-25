import { create } from "zustand";

export type SettingsTab = "general" | "accounts" | "voice" | "languages" | "advanced";

/** Settings sub-tab, lifted so other surfaces (taskbar widget click) can deep-link. */
interface State {
  tab: SettingsTab;
  setTab: (tab: SettingsTab) => void;
}

export const useSettingsNavStore = create<State>((set) => ({
  tab: "general",
  setTab: (tab) => set({ tab }),
}));
