import notificationUrl from "~/assets/notification.mp3";

/**
 * Plays a short audio cue when a turn completes while the window is not
 * focused. Uses a prerecorded mp3 shipped under `src/assets/` (Vite turns
 * the import into a URL the browser can fetch).
 */

const PREF_KEY = "oxyris.notificationSound";

export function isNotificationSoundEnabled(): boolean {
  const raw = window.localStorage.getItem(PREF_KEY);
  // Default ON — caller opts out explicitly.
  return raw !== "off";
}

export function setNotificationSoundEnabled(enabled: boolean): void {
  window.localStorage.setItem(PREF_KEY, enabled ? "on" : "off");
}

// Reuse one Audio element so we don't keep spawning decoders.
let cached: HTMLAudioElement | null = null;

function getAudio(): HTMLAudioElement {
  if (!cached) {
    cached = new Audio(notificationUrl);
    cached.preload = "auto";
    cached.volume = 0.6;
  }
  return cached;
}

/** Play the chime. No-op if audio isn't available or the pref is off. */
export function playTurnCompleteChime(): void {
  if (!isNotificationSoundEnabled()) return;
  try {
    const audio = getAudio();
    audio.currentTime = 0;
    void audio.play().catch(() => {
      // Chromium requires a user gesture before first playback; silently
      // drop if autoplay policy rejects us.
    });
  } catch {
    /* noop */
  }
}

/** True when the window is **not** focused — cue to actually play the sound. */
export function shouldNotify(): boolean {
  return typeof document !== "undefined" && !document.hasFocus();
}
