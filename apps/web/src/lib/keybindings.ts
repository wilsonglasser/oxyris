import { invoke } from "@tauri-apps/api/core";

export interface Keybindings {
  new_thread: string;
  interrupt: string;
  toggle_terminal: string;
  focus_search: string;
}

export const DEFAULT_KEYBINDINGS: Keybindings = {
  new_thread: "Ctrl+Shift+N",
  interrupt: "Escape",
  toggle_terminal: "Ctrl+`",
  focus_search: "Ctrl+K",
};

export async function loadKeybindings(): Promise<Keybindings> {
  try {
    const raw = await invoke<string>("settings_keybindings_read");
    const parsed = JSON.parse(raw);
    return {
      ...DEFAULT_KEYBINDINGS,
      ...(typeof parsed === "object" && parsed ? parsed : {}),
    };
  } catch {
    return DEFAULT_KEYBINDINGS;
  }
}

/**
 * Match a keyboard event against a `Ctrl+Shift+B`-style spec. Tokens are
 * case-insensitive; supports ctrl/shift/alt/meta modifiers plus single keys
 * like `Escape`, `F5`, `` ` ``, `/`, etc.
 */
export function matchesKey(e: KeyboardEvent, spec: string): boolean {
  if (!spec) return false;
  const tokens = spec
    .split("+")
    .map((t) => t.trim().toLowerCase())
    .filter(Boolean);
  let needCtrl = false;
  let needShift = false;
  let needAlt = false;
  let needMeta = false;
  let key: string | null = null;
  for (const t of tokens) {
    if (t === "ctrl" || t === "control") needCtrl = true;
    else if (t === "shift") needShift = true;
    else if (t === "alt" || t === "option") needAlt = true;
    else if (t === "meta" || t === "cmd" || t === "win") needMeta = true;
    else key = t;
  }
  if (!key) return false;
  if (needCtrl !== e.ctrlKey) return false;
  if (needShift !== e.shiftKey) return false;
  if (needAlt !== e.altKey) return false;
  if (needMeta !== e.metaKey) return false;
  const evKey = e.key.toLowerCase();
  // Allow both "escape" and "esc", "space" and " ".
  if (key === "esc" && evKey === "escape") return true;
  if (key === "space" && evKey === " ") return true;
  return evKey === key;
}

/** True when the event target is an editable element (input/textarea/etc). */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  return /^(input|textarea|select)$/i.test(target.tagName);
}
