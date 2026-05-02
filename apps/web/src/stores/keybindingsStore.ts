import { create } from "zustand";
import {
  DEFAULT_KEYBINDINGS,
  type Keybindings,
  loadKeybindings,
} from "~/lib/keybindings.ts";

interface State {
  bindings: Keybindings;
  ready: boolean;
  load: () => Promise<void>;
  /** Refresh after the Settings editor writes new values. */
  reload: () => Promise<void>;
}

export const useKeybindingsStore = create<State>((set) => ({
  bindings: DEFAULT_KEYBINDINGS,
  ready: false,
  load: async () => {
    const next = await loadKeybindings();
    set({ bindings: next, ready: true });
  },
  reload: async () => {
    const next = await loadKeybindings();
    set({ bindings: next });
  },
}));
