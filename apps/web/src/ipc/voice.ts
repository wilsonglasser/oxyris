import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Input-device names for the mic picker. */
export function voiceListDevices(): Promise<string[]> {
  return invoke("voice_list_devices");
}

/** Whether the KWS wake-word model files are installed. */
export function voiceWakeReady(): Promise<boolean> {
  return invoke("voice_wake_ready");
}

/** Download + install the wake-word model (resolves when on disk). */
export function voiceDownloadKws(): Promise<void> {
  return invoke("voice_download_kws");
}

export interface VoiceEnableInput {
  /** Wake keyword — plain text (e.g. "OXY") when the model ships a BPE vocab. */
  keywords: string;
  threshold?: number;
  score?: number;
  device?: string | null;
}

/** Arm the always-on wake word. */
export function voiceEnable(input: VoiceEnableInput): Promise<void> {
  return invoke("voice_enable", { input });
}

/** Disarm the wake word. */
export function voiceDisable(): Promise<void> {
  return invoke("voice_disable");
}

/** Whether the Kokoro TTS model is installed. */
export function voiceTtsReady(): Promise<boolean> {
  return invoke("voice_tts_ready");
}

/** Download + install the Kokoro TTS model (~300MB). */
export function voiceDownloadTts(): Promise<void> {
  return invoke("voice_download_tts");
}

/** Speak text with Kokoro (pt-BR). `sid` = voice id, `speed` = rate. */
export function voiceSpeak(input: {
  text: string;
  sid?: number;
  speed?: number;
  lang?: string;
}): Promise<void> {
  return invoke("voice_speak", { input });
}

/** Subscribe to wake-word detections (`oxy:wake`). */
export function onOxyWake(cb: (keyword: string) => void): Promise<UnlistenFn> {
  return listen<{ keyword: string }>("oxy:wake", (e) => cb(e.payload.keyword));
}

/**
 * Subscribe to transcribed spoken commands (`oxy:command`). `text` is empty when
 * the user woke Oxy but said nothing (capture aborted).
 */
export function onOxyCommand(cb: (text: string) => void): Promise<UnlistenFn> {
  return listen<{ text: string }>("oxy:command", (e) => cb(e.payload.text));
}
