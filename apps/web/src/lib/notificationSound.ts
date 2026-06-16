import neutralUrl from "~/assets/41-neutral_5xNBDJc.mp3";
import aoeAttackUrl from "~/assets/aoe2-attack.mp3";
import aoeFarmUrl from "~/assets/aoe2-farm-exhausted.mp3";
import aoeHousedUrl from "~/assets/aoe2-housed.mp3";
import decompressionUrl from "~/assets/freesound_community-decompression-1-107554.mp3";
import notificationUrl from "~/assets/notification.mp3";
import okayUrl from "~/assets/okay.mp3";
import peasantOkayUrl from "~/assets/peasant-okay-warcraft-ii.mp3";
import prrrohUrl from "~/assets/prrroh.mp3";
import shhhHoUrl from "~/assets/shhh-ho.mp3";
import telegramaUrl from "~/assets/telegrama-goblin-warcraft-3.mp3";
import peonWorkUrl from "~/assets/wc3-peon-says-work-work-only-.mp3";
import workCompleteUrl from "~/assets/work-complete-warcraft-ii.mp3";

/**
 * Two notification channels:
 *   - `completion`: a turn finished naturally (orange attention bull).
 *   - `input`: Claude is waiting on the user (red bull / approval prompt).
 * Each channel picks a sound from `SOUND_OPTIONS` or `"off"` to mute it.
 */

export type SoundChannel = "completion" | "input" | "escalation" | "mission";

export type SoundOption = {
  id: string;
  label: string;
  url: string;
};

export const SOUND_OPTIONS: SoundOption[] = [
  { id: "notification", label: "Notification (default)", url: notificationUrl },
  { id: "decompression", label: "Decompression", url: decompressionUrl },
  { id: "neutral", label: "Neutral chime", url: neutralUrl },
  { id: "okay", label: "Okay", url: okayUrl },
  { id: "prrroh", label: "Prrroh", url: prrrohUrl },
  { id: "shhh-ho", label: "Shhh-ho", url: shhhHoUrl },
  { id: "aoe2-attack", label: "AoE II — Attack!", url: aoeAttackUrl },
  { id: "aoe2-housed", label: "AoE II — Housed", url: aoeHousedUrl },
  { id: "aoe2-farm-exhausted", label: "AoE II — Farm exhausted", url: aoeFarmUrl },
  { id: "wc2-peasant-okay", label: "WC II — Peasant okay", url: peasantOkayUrl },
  { id: "wc2-work-complete", label: "WC II — Work complete", url: workCompleteUrl },
  { id: "wc3-peon-work", label: "WC III — Peon work work", url: peonWorkUrl },
  { id: "wc3-telegrama-goblin", label: "WC III — Goblin telegram", url: telegramaUrl },
];

const DEFAULTS: Record<SoundChannel, string> = {
  completion: "notification",
  input: "wc3-telegrama-goblin",
  // Auto-pilot escalation — the "I'm stuck, a human is needed" alert. Loud and
  // distinct from the other two by default.
  escalation: "aoe2-attack",
  // Auto-pilot mission complete — the pilot finished the whole job and shut off.
  // The triumphant "work complete" by default.
  mission: "wc2-work-complete",
};

const PREF_KEYS: Record<SoundChannel, string> = {
  completion: "oxyris.notificationSound.completion",
  input: "oxyris.notificationSound.input",
  escalation: "oxyris.notificationSound.escalation",
  mission: "oxyris.notificationSound.mission",
};

// Legacy single-channel pref ("on" / "off"). When the user had it set to "off"
// we honor that as a one-time migration to "off" on both new channels.
const LEGACY_KEY = "oxyris.notificationSound";
let migrated = false;

function migrateLegacy(): void {
  if (migrated) return;
  migrated = true;
  const legacy = window.localStorage.getItem(LEGACY_KEY);
  if (legacy === null) return;
  const hasNew =
    window.localStorage.getItem(PREF_KEYS.completion) !== null ||
    window.localStorage.getItem(PREF_KEYS.input) !== null;
  if (!hasNew && legacy === "off") {
    window.localStorage.setItem(PREF_KEYS.completion, "off");
    window.localStorage.setItem(PREF_KEYS.input, "off");
  }
  window.localStorage.removeItem(LEGACY_KEY);
}

export function getChannelSound(ch: SoundChannel): string {
  migrateLegacy();
  const raw = window.localStorage.getItem(PREF_KEYS[ch]);
  if (raw === null) return DEFAULTS[ch];
  if (raw === "off") return "off";
  if (SOUND_OPTIONS.some((s) => s.id === raw)) return raw;
  return DEFAULTS[ch];
}

export function setChannelSound(ch: SoundChannel, id: string): void {
  window.localStorage.setItem(PREF_KEYS[ch], id);
}

// Reuse one Audio element per sound URL so we don't keep spawning decoders.
const cache = new Map<string, HTMLAudioElement>();

function getAudio(url: string): HTMLAudioElement {
  let a = cache.get(url);
  if (!a) {
    a = new Audio(url);
    a.preload = "auto";
    a.volume = 0.6;
    cache.set(url, a);
  }
  return a;
}

function playSoundById(id: string): void {
  if (id === "off") return;
  const opt = SOUND_OPTIONS.find((s) => s.id === id);
  if (!opt) return;
  try {
    const audio = getAudio(opt.url);
    audio.currentTime = 0;
    void audio.play().catch(() => {
      // Chromium requires a user gesture before first playback; silently drop
      // if the autoplay policy rejects us.
    });
  } catch {
    /* noop */
  }
}

/** Chime: a turn finished naturally (orange attention bull). */
export function playCompletionChime(): void {
  playSoundById(getChannelSound("completion"));
}

/** Chime: Claude is waiting for user input (red bull / approval prompt). */
export function playInputChime(): void {
  playSoundById(getChannelSound("input"));
}

/** Chime: the auto-pilot escalated — it can't proceed without you. */
export function playEscalationChime(): void {
  playSoundById(getChannelSound("escalation"));
}

/** Chime: the auto-pilot completed its mission and shut itself off. */
export function playMissionDoneChime(): void {
  playSoundById(getChannelSound("mission"));
}

/** Preview-play a specific sound (used by the settings test button). */
export function previewSound(id: string): void {
  playSoundById(id);
}

/** True when the window is **not** focused — cue to actually play the sound. */
export function shouldNotify(): boolean {
  return typeof document !== "undefined" && !document.hasFocus();
}
