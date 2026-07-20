//! Oxy voice commands — arm/disarm the always-on "Oxy" wake word and enumerate
//! mics for the Settings picker. The wake listener lives in
//! [`crate::infra::voice`]; on detection it emits the `oxy:wake` Tauri event and
//! the frontend takes over (STT + endpointing → drive the Oxy session).

use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::infra::voice::{self, KwsModel, SttModel, TtsEngine, TtsModel, WakeConfig};

/// Input-device names for the mic picker.
#[tauri::command]
pub fn voice_list_devices() -> Vec<String> {
    voice::list_input_devices()
}

/// Whether both voice models (wake + STT) are installed, i.e. voice can be armed.
#[tauri::command]
pub fn voice_wake_ready(state: State<'_, AppState>) -> bool {
    KwsModel::under(&state.data_dir).all_present() && SttModel::under(&state.data_dir).all_present()
}

/// Download + install both voice models (wake + STT). Overwrites if present.
#[tauri::command]
pub async fn voice_download_kws(state: State<'_, AppState>) -> Result<(), String> {
    let root = state.data_dir.clone();
    voice::download_voice_models(root).await
}

#[derive(serde::Deserialize)]
pub struct VoiceEnableInput {
    /// Tokenized keyword line(s) for the KWS model — the "Oxy" trigger.
    pub keywords: String,
    pub threshold: Option<f32>,
    pub score: Option<f32>,
    pub device: Option<String>,
}

/// Arm the wake word. Replaces any previously-armed listener.
#[tauri::command]
pub fn voice_enable(
    app: AppHandle,
    state: State<'_, AppState>,
    input: VoiceEnableInput,
) -> Result<(), String> {
    let cfg = WakeConfig {
        model: KwsModel::under(&state.data_dir),
        stt: SttModel::under(&state.data_dir),
        keywords: input.keywords,
        device: input.device,
        threshold: input.threshold.unwrap_or(0.25),
        score: input.score.unwrap_or(1.0),
    };
    let handle = voice::start_wake(app, cfg)?;
    let mut slot = state
        .voice_wake
        .lock()
        .map_err(|_| "voice lock poisoned".to_string())?;
    if let Some(old) = slot.take() {
        old.stop();
    }
    *slot = Some(handle);
    Ok(())
}

/// Whether the Kokoro TTS model is installed.
#[tauri::command]
pub fn voice_tts_ready(state: State<'_, AppState>) -> bool {
    TtsModel::under(&state.data_dir).all_present()
}

/// Download + install the Kokoro TTS model (~300MB). Separate from the wake/STT
/// pair since it's large and optional.
#[tauri::command]
pub async fn voice_download_tts(state: State<'_, AppState>) -> Result<(), String> {
    let root = state.data_dir.clone();
    voice::download_tts_model(root).await
}

#[derive(serde::Deserialize)]
pub struct VoiceSpeakInput {
    pub text: String,
    /// Voice id (Kokoro speaker index). Default 0.
    pub sid: Option<i32>,
    pub speed: Option<f32>,
    /// espeak-ng language for phonemization (e.g. "pt-br", "en-us"). Default
    /// "pt-br" (Brazilian Portuguese; plain "pt" would be European).
    pub lang: Option<String>,
}

/// Speak `text` with Kokoro. Lazily builds the engine on first use and rebuilds
/// it if the voice id / speed / language changed.
#[tauri::command]
pub fn voice_speak(state: State<'_, AppState>, input: VoiceSpeakInput) -> Result<(), String> {
    let sid = input.sid.unwrap_or(0);
    let speed = input.speed.unwrap_or(1.0);
    let lang = input.lang.unwrap_or_else(|| "pt-br".to_string());
    let mut slot = state
        .voice_tts
        .lock()
        .map_err(|_| "tts lock poisoned".to_string())?;
    let need_new = match slot.as_ref() {
        Some(e) => !e.matches(sid, speed, &lang),
        None => true,
    };
    if need_new {
        *slot = Some(TtsEngine::create(
            &TtsModel::under(&state.data_dir),
            &lang,
            sid,
            speed,
        )?);
    }
    slot.as_ref().expect("engine just set").speak(&input.text)
}

/// Disarm the wake word.
#[tauri::command]
pub fn voice_disable(state: State<'_, AppState>) -> Result<(), String> {
    let mut slot = state
        .voice_wake
        .lock()
        .map_err(|_| "voice lock poisoned".to_string())?;
    if let Some(h) = slot.take() {
        h.stop();
    }
    Ok(())
}
