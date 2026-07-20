//! Oxy voice — always-on wake-word ("Oxy") spotting via sherpa-onnx KWS, fed by
//! a cpal microphone stream. Fully offline/on-device. On detection it emits the
//! `oxy:wake` Tauri event; the frontend then starts capturing the command (STT +
//! endpointing) and drives the Oxy session. See `docs/design/oxy-assistant.md`.
//!
//! sherpa-onnx keyword spotting is *open-vocabulary*: the keyword is supplied as
//! tokenized text (`keywords_buf`), so "Oxy" needs no per-user model training.
//!
//! Threading: one dedicated OS thread owns the cpal input `Stream` (which is
//! `!Send` on WASAPI) AND the `KeywordSpotter` + its `OnlineStream` (raw ptr,
//! kept thread-local). The cpal callback runs on cpal's own audio thread and
//! only pushes f32 samples into a channel; the owning thread drains it, feeds
//! the spotter, decodes, and fires the wake event.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::thread::JoinHandle;
use std::time::Instant;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use sherpa_onnx::{
    KeywordSpotter, KeywordSpotterConfig, OfflineRecognizer, OfflineRecognizerConfig,
};
use tauri::{AppHandle, Emitter};

/// The four files a sherpa-onnx streaming (transducer) KWS model needs. Live
/// under `<data_dir>/models/kws/`; downloaded once (see model management, TODO).
#[derive(Clone, Debug)]
pub struct KwsModel {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl KwsModel {
    /// Standard layout under a models root. All four files must exist to arm the
    /// wake word.
    pub fn under(root: &std::path::Path) -> Self {
        let d = root.join("models").join("kws");
        Self {
            encoder: d.join("encoder.onnx"),
            decoder: d.join("decoder.onnx"),
            joiner: d.join("joiner.onnx"),
            tokens: d.join("tokens.txt"),
        }
    }

    pub fn all_present(&self) -> bool {
        self.encoder.is_file()
            && self.decoder.is_file()
            && self.joiner.is_file()
            && self.tokens.is_file()
    }
}

/// Whisper (offline) STT model files for command transcription. Live under
/// `<data_dir>/models/stt/`.
#[derive(Clone, Debug)]
pub struct SttModel {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub tokens: PathBuf,
}

impl SttModel {
    pub fn under(root: &std::path::Path) -> Self {
        let d = root.join("models").join("stt");
        Self {
            encoder: d.join("encoder.onnx"),
            decoder: d.join("decoder.onnx"),
            tokens: d.join("tokens.txt"),
        }
    }

    pub fn all_present(&self) -> bool {
        self.encoder.is_file() && self.decoder.is_file() && self.tokens.is_file()
    }
}

/// Kokoro (offline) TTS model files. Live under `<data_dir>/models/tts/`.
#[derive(Clone, Debug)]
pub struct TtsModel {
    pub model: PathBuf,
    pub voices: PathBuf,
    pub tokens: PathBuf,
    /// espeak-ng data directory (needed by Kokoro for phonemization).
    pub data_dir: PathBuf,
}

impl TtsModel {
    pub fn under(root: &std::path::Path) -> Self {
        let d = root.join("models").join("tts");
        Self {
            model: d.join("model.onnx"),
            voices: d.join("voices.bin"),
            tokens: d.join("tokens.txt"),
            data_dir: d.join("espeak-ng-data"),
        }
    }

    pub fn all_present(&self) -> bool {
        self.model.is_file()
            && self.voices.is_file()
            && self.tokens.is_file()
            && self.data_dir.is_dir()
    }
}

/// A ready-to-use Kokoro TTS engine. Created once (model load is expensive),
/// reused for every reply. `sid` selects the voice (pt-BR female = a specific
/// index in the multilingual voices file — tunable from Settings).
pub struct TtsEngine {
    tts: sherpa_onnx::OfflineTts,
    sid: i32,
    speed: f32,
    lang: String,
}

impl TtsEngine {
    pub fn create(model: &TtsModel, lang: &str, sid: i32, speed: f32) -> Result<Self, String> {
        if !model.all_present() {
            return Err("TTS model files missing — download the voice model first".into());
        }
        let mut c = sherpa_onnx::OfflineTtsConfig::default();
        c.model.kokoro.model = Some(path_str(&model.model)?);
        c.model.kokoro.voices = Some(path_str(&model.voices)?);
        c.model.kokoro.tokens = Some(path_str(&model.tokens)?);
        c.model.kokoro.data_dir = Some(path_str(&model.data_dir)?);
        c.model.kokoro.lang = Some(lang.to_owned());
        c.model.num_threads = 2;
        let tts = sherpa_onnx::OfflineTts::create(&c)
            .ok_or_else(|| "failed to create Kokoro TTS engine".to_string())?;
        Ok(Self {
            tts,
            sid,
            speed,
            lang: lang.to_owned(),
        })
    }

    /// Whether this engine already matches the requested config (so the caller
    /// can skip an expensive rebuild).
    pub fn matches(&self, sid: i32, speed: f32, lang: &str) -> bool {
        self.sid == sid && self.speed == speed && self.lang == lang
    }

    /// Synthesize `text` and play it (fire-and-forget on a playback thread).
    pub fn speak(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let gen_cfg = sherpa_onnx::GenerationConfig {
            sid: self.sid,
            speed: self.speed,
            ..Default::default()
        };
        let audio = self
            .tts
            .generate_with_config(text, &gen_cfg, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| "TTS generation failed".to_string())?;
        play_audio(audio.samples().to_vec(), audio.sample_rate() as u32);
        Ok(())
    }
}

/// Play mono f32 samples on the default output device via rodio. Runs on its own
/// thread so the caller doesn't block; the output stream is kept alive there
/// until playback finishes.
fn play_audio(samples: Vec<f32>, sample_rate: u32) {
    std::thread::spawn(move || match rodio::OutputStream::try_default() {
        Ok((_stream, handle)) => match rodio::Sink::try_new(&handle) {
            Ok(sink) => {
                sink.append(rodio::buffer::SamplesBuffer::new(1, sample_rate, samples));
                sink.sleep_until_end();
            }
            Err(e) => tracing::warn!(error = %e, "oxy_tts: sink failed"),
        },
        Err(e) => tracing::warn!(error = %e, "oxy_tts: no audio output device"),
    });
}

/// Wake-word arming config.
#[derive(Clone, Debug)]
pub struct WakeConfig {
    pub model: KwsModel,
    /// Offline STT model used to transcribe the spoken command after wake.
    pub stt: SttModel,
    /// Tokenized keyword line(s) for `keywords_buf` — e.g. the token pieces that
    /// spell "Oxy" for this model's `tokens.txt`, optionally `:score`.
    pub keywords: String,
    /// Optional input device name (from [`list_input_devices`]); `None` = default.
    pub device: Option<String>,
    /// Detection threshold (sherpa `keywords_threshold`, 0..1). Lower = more eager.
    pub threshold: f32,
    /// Boost score for the keyword (sherpa `keywords_score`).
    pub score: f32,
}

/// Payload of the `oxy:wake` Tauri event.
#[derive(Clone, Debug, Serialize)]
struct WakeEvent {
    keyword: String,
}

/// Payload of the `oxy:command` Tauri event — the transcribed command after a
/// wake. `text` is empty when the user woke Oxy but said nothing (aborted).
#[derive(Clone, Debug, Serialize)]
struct CommandEvent {
    text: String,
}

/// Live wake session. Drop or call [`stop`](WakeHandle::stop) to tear down.
pub struct WakeHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WakeHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for WakeHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Enumerate input device names for the Settings mic picker.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Arm the wake word. Spawns the capture+decode thread and returns a handle.
/// Fails fast when the model files are missing or the mic can't be opened.
pub fn start_wake(app: AppHandle, cfg: WakeConfig) -> Result<WakeHandle, String> {
    if !cfg.model.all_present() {
        return Err("KWS model files missing — download the wake-word model first".into());
    }
    if !cfg.stt.all_present() {
        return Err("STT model files missing — download the voice models first".into());
    }

    let mut kws_config = KeywordSpotterConfig::default();
    kws_config.model_config.transducer.encoder = Some(path_str(&cfg.model.encoder)?);
    kws_config.model_config.transducer.decoder = Some(path_str(&cfg.model.decoder)?);
    kws_config.model_config.transducer.joiner = Some(path_str(&cfg.model.joiner)?);
    kws_config.model_config.tokens = Some(path_str(&cfg.model.tokens)?);
    // sherpa does NOT tokenize keywords at runtime — it looks each space-
    // separated token up in tokens.txt and hard-aborts the process if one is
    // missing. So we pre-tokenize the plain keyword ("OXY") into model token
    // pieces ourselves; producing only pieces that exist guarantees no crash.
    let keyword_line = tokenize_keyword(&cfg.model.tokens, &cfg.keywords)?;
    tracing::info!(keyword = %cfg.keywords, tokens = %keyword_line, "oxy_wake: tokenized keyword");
    kws_config.keywords_buf = Some(keyword_line);
    kws_config.keywords_threshold = cfg.threshold;
    kws_config.keywords_score = cfg.score;

    let spotter = KeywordSpotter::create(&kws_config)
        .ok_or_else(|| "failed to create KWS spotter (bad model or keywords)".to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let device_name = cfg.device.clone();
    let stt = cfg.stt.clone();

    let join = std::thread::Builder::new()
        .name("oxy-wake".into())
        .spawn(move || {
            if let Err(e) = run_wake_loop(app, spotter, stt, device_name, &stop_thread) {
                tracing::warn!(error = %e, "oxy_wake: loop ended with error");
            }
        })
        .map_err(|e| format!("spawn wake thread: {e}"))?;

    Ok(WakeHandle {
        stop,
        join: Some(join),
    })
}

/// End the command capture after this much trailing silence (spoke, then quiet).
/// Generous so slow/paused speech isn't cut off mid-thought.
const CMD_SILENCE_MS: u128 = 1800;
/// Abort the capture if the user woke Oxy but said nothing within this window.
const CMD_NO_SPEECH_MS: u128 = 5000;
/// Hard cap on a single spoken command.
const CMD_MAX_MS: u128 = 20000;
/// RMS energy above which a chunk counts as speech (rough VAD). Low so quiet
/// speech / natural pauses still register as voice.
const RMS_SPEECH: f32 = 0.012;

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

fn run_wake_loop(
    app: AppHandle,
    spotter: KeywordSpotter,
    stt: SttModel,
    device_name: Option<String>,
    stop: &AtomicBool,
) -> Result<(), String> {
    // Build the offline (whisper) recognizer on this thread — its raw ptr is
    // kept thread-local. Created once and reused for every command.
    let mut rc = OfflineRecognizerConfig::default();
    rc.model_config.whisper.encoder = Some(path_str(&stt.encoder)?);
    rc.model_config.whisper.decoder = Some(path_str(&stt.decoder)?);
    // Force Portuguese so multilingual whisper doesn't auto-detect (and mangle
    // into) English on pt-BR speech.
    rc.model_config.whisper.language = Some("pt".into());
    rc.model_config.tokens = Some(path_str(&stt.tokens)?);
    rc.model_config.num_threads = 2;
    let recognizer = OfflineRecognizer::create(&rc)
        .ok_or_else(|| "failed to create STT recognizer (bad model)".to_string())?;

    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| format!("input device not found: {name}"))?,
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input device".to_string())?,
    };

    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = supported.sample_rate().0 as i32;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    // cpal's audio thread pushes downmixed-mono f32 frames here; we drain below.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let err_fn = |e| tracing::warn!(error = %e, "oxy_wake: cpal stream error");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &_| {
                let _ = tx.send(downmix(data, channels));
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &_| {
                let f: Vec<f32> = data.iter().map(|s| *s as f32 / 32768.0).collect();
                let _ = tx.send(downmix(&f, channels));
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &_| {
                let f: Vec<f32> = data
                    .iter()
                    .map(|s| (*s as f32 - 32768.0) / 32768.0)
                    .collect();
                let _ = tx.send(downmix(&f, channels));
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| format!("build input stream: {e}"))?;

    stream.play().map_err(|e| format!("stream play: {e}"))?;
    tracing::info!(sample_rate, channels, "oxy_wake: listening for \"Oxy\"");

    // Two modes on one mic stream: WAKE (feed KWS) and COMMAND (buffer speech,
    // energy-endpoint, then whisper-transcribe). No mic hand-off, no Web Speech.
    let ks = spotter.create_stream();
    let mut commanding = false;
    let mut cmd_buf: Vec<f32> = Vec::new();
    let mut cmd_started = Instant::now();
    let mut last_voice: Option<Instant> = None;

    while !stop.load(Ordering::Relaxed) {
        let samples = match rx.try_recv() {
            Ok(s) => s,
            Err(TryRecvError::Empty) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            Err(TryRecvError::Disconnected) => break,
        };

        if !commanding {
            // ── WAKE ──────────────────────────────────────────────────────────
            ks.accept_waveform(sample_rate, &samples);
            while spotter.is_ready(&ks) {
                spotter.decode(&ks);
            }
            if let Some(result) = spotter.get_result(&ks)
                && !result.keyword.trim().is_empty()
            {
                tracing::info!(keyword = %result.keyword, "oxy_wake: detected");
                let _ = app.emit(
                    "oxy:wake",
                    WakeEvent {
                        keyword: result.keyword,
                    },
                );
                spotter.reset(&ks);
                commanding = true;
                cmd_buf.clear();
                cmd_started = Instant::now();
                last_voice = None;
            }
        } else {
            // ── COMMAND ───────────────────────────────────────────────────────
            if rms(&samples) > RMS_SPEECH {
                last_voice = Some(Instant::now());
            }
            cmd_buf.extend_from_slice(&samples);

            let elapsed = cmd_started.elapsed().as_millis();
            let ended = match last_voice {
                Some(lv) => lv.elapsed().as_millis() >= CMD_SILENCE_MS,
                None => elapsed >= CMD_NO_SPEECH_MS,
            } || elapsed >= CMD_MAX_MS;

            if ended {
                let text = if last_voice.is_some() {
                    let t = recognize(&recognizer, sample_rate, &cmd_buf);
                    if is_noise_transcript(&t) {
                        tracing::info!(raw = %t, "oxy_wake: dropped noise/hallucination transcript");
                        String::new()
                    } else {
                        tracing::info!(text = %t, "oxy_wake: command transcribed");
                        t
                    }
                } else {
                    tracing::info!("oxy_wake: command aborted (no speech)");
                    String::new()
                };
                let _ = app.emit("oxy:command", CommandEvent { text });
                commanding = false;
                cmd_buf.clear();
            }
        }
    }
    drop(stream);
    Ok(())
}

/// Whisper hallucinates on near-silence/noise: sound annotations like
/// `*sad music*`, `[Music]`, `(wind blowing)`, or junk codes like `A-113s.`.
/// Reject anything that looks like that so we never send it as a command.
fn is_noise_transcript(t: &str) -> bool {
    let s = t.trim();
    if s.is_empty() {
        return true;
    }
    // Fully wrapped in *…*, […], (…) → a non-speech annotation.
    let wrapped = (s.starts_with('*') && s.ends_with('*'))
        || (s.starts_with('[') && s.ends_with(']'))
        || (s.starts_with('(') && s.ends_with(')'));
    if wrapped {
        return true;
    }
    // No letters at all (pure punctuation/symbols).
    if !s.chars().any(|c| c.is_alphabetic()) {
        return true;
    }
    // A single token containing digits (e.g. "A-113s.") — real spoken commands
    // are words with spaces, not alphanumeric codes.
    let single_token = !s.contains(char::is_whitespace);
    if single_token && s.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

/// Transcribe buffered samples with the offline whisper recognizer.
fn recognize(rec: &OfflineRecognizer, sample_rate: i32, samples: &[f32]) -> String {
    let stream = rec.create_stream();
    stream.accept_waveform(sample_rate, samples);
    rec.decode(&stream);
    stream
        .get_result()
        .map(|r| r.text.trim().to_owned())
        .unwrap_or_default()
}

/// Average interleaved channels down to mono.
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn path_str(p: &std::path::Path) -> Result<String, String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("non-UTF8 path: {}", p.display()))
}

/// Turn a plain keyword ("Oxy") into the space-separated token-piece line the
/// KWS model expects, by greedy longest-match against its `tokens.txt`. Every
/// emitted piece is guaranteed to exist in the vocab (so sherpa won't abort).
/// The BPE models use `▁` (U+2581) as the word-boundary prefix and are
/// uppercase. Returns `Err` (never a crash) when a character can't be matched.
fn tokenize_keyword(tokens_path: &std::path::Path, text: &str) -> Result<String, String> {
    let content =
        std::fs::read_to_string(tokens_path).map_err(|e| format!("read tokens.txt: {e}"))?;
    let pieces: std::collections::HashSet<&str> = content
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();

    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let w = format!("\u{2581}{}", word.to_uppercase());
        let chars: Vec<char> = w.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let mut end = chars.len();
            let matched = loop {
                if end <= i {
                    break None;
                }
                let cand: String = chars[i..end].iter().collect();
                if pieces.contains(cand.as_str()) {
                    break Some((cand, end));
                }
                end -= 1;
            };
            match matched {
                Some((piece, next)) => {
                    out.push(piece);
                    i = next;
                }
                None => {
                    return Err(format!(
                        "keyword '{text}' can't be spelled with this model's tokens \
                         (stuck at '{}') — try a different word",
                        chars[i]
                    ));
                }
            }
        }
    }
    if out.is_empty() {
        return Err("empty keyword".into());
    }
    // `@name` sets the label returned on detection.
    Ok(format!("{} @{}", out.join(" "), text.trim().to_lowercase()))
}

/// Prebuilt English streaming KWS model (k2-fsa release). Small (~3.3M params),
/// open-vocabulary, ships `bpe.vocab` so keywords can be tokenized.
pub const KWS_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2";

/// Download + extract the KWS model into `<root>/models/kws/`, normalizing the
/// versioned onnx filenames to `encoder/decoder/joiner.onnx`. Idempotent —
/// re-running overwrites. Also keeps `tokens.txt`, `bpe.vocab`, `keywords.txt`.
pub async fn download_kws_model(root: PathBuf) -> Result<(), String> {
    let dest = root.join("models").join("kws");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    tracing::info!("oxy_wake: downloading KWS model…");
    let bytes = reqwest::get(KWS_MODEL_URL)
        .await
        .map_err(|e| format!("download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    tokio::task::spawn_blocking(move || extract_kws(&bytes, &dest))
        .await
        .map_err(|e| format!("extract task: {e}"))?
}

/// Unpack the `.tar.bz2`, keeping only the files the detector needs and giving
/// the onnx files canonical names. Top-level archive dir is stripped.
fn extract_kws(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    let decoder = bzip2::read::BzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let target = if name.starts_with("encoder") && name.ends_with(".onnx") {
            "encoder.onnx"
        } else if name.starts_with("decoder") && name.ends_with(".onnx") {
            "decoder.onnx"
        } else if name.starts_with("joiner") && name.ends_with(".onnx") {
            "joiner.onnx"
        } else if name == "tokens.txt" || name == "bpe.vocab" || name == "keywords.txt" {
            name
        } else {
            continue;
        };
        entry
            .unpack(dest.join(target))
            .map_err(|e| format!("unpack {name}: {e}"))?;
    }
    let model = KwsModel::under(dest.parent().and_then(|p| p.parent()).unwrap_or(dest));
    if model.all_present() {
        tracing::info!("oxy_wake: KWS model ready");
        Ok(())
    } else {
        Err("extraction finished but expected model files are missing".into())
    }
}

/// Prebuilt multilingual whisper-base (k2-fsa release) — noticeably better than
/// tiny for pt-BR. int8 variants are skipped in favour of the fp32 onnx files.
pub const STT_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-base.tar.bz2";

/// Download both voice models (wake + STT). Used by the Settings "download"
/// button so the user gets a ready-to-arm voice stack in one click.
pub async fn download_voice_models(root: PathBuf) -> Result<(), String> {
    download_kws_model(root.clone()).await?;
    download_stt_model(root).await
}

/// Download + extract the whisper STT model into `<root>/models/stt/`.
pub async fn download_stt_model(root: PathBuf) -> Result<(), String> {
    let dest = root.join("models").join("stt");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    tracing::info!("oxy_wake: downloading STT model…");
    let bytes = reqwest::get(STT_MODEL_URL)
        .await
        .map_err(|e| format!("download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    tokio::task::spawn_blocking(move || extract_stt(&bytes, &dest))
        .await
        .map_err(|e| format!("extract task: {e}"))?
}

/// Unpack the whisper `.tar.bz2`, normalizing `*-encoder.onnx` → `encoder.onnx`
/// etc. (fp32 only; int8 variants end in `.int8.onnx` and are skipped).
fn extract_stt(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    let decoder = bzip2::read::BzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let target = if name.ends_with("-encoder.onnx") {
            "encoder.onnx"
        } else if name.ends_with("-decoder.onnx") {
            "decoder.onnx"
        } else if name.ends_with("-tokens.txt") {
            "tokens.txt"
        } else {
            continue;
        };
        entry
            .unpack(dest.join(target))
            .map_err(|e| format!("unpack {name}: {e}"))?;
    }
    let model = SttModel::under(dest.parent().and_then(|p| p.parent()).unwrap_or(dest));
    if model.all_present() {
        tracing::info!("oxy_wake: STT model ready");
        Ok(())
    } else {
        Err("STT extraction finished but expected files are missing".into())
    }
}

/// Kokoro multilingual v1.0 (k2-fsa release) — includes pt-BR voices. Large
/// (~300MB) so it's a separate, opt-in download from the wake/STT pair.
pub const TTS_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2";

/// Download + extract the Kokoro TTS model into `<root>/models/tts/`, preserving
/// the full tree (model.onnx, voices.bin, tokens.txt, espeak-ng-data/, …).
pub async fn download_tts_model(root: PathBuf) -> Result<(), String> {
    let dest = root.join("models").join("tts");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    tracing::info!("oxy_tts: downloading Kokoro model (~300MB)…");
    let bytes = reqwest::get(TTS_MODEL_URL)
        .await
        .map_err(|e| format!("download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    tokio::task::spawn_blocking(move || extract_tts(&bytes, &dest))
        .await
        .map_err(|e| format!("extract task: {e}"))?
}

/// Unpack the Kokoro `.tar.bz2` whole (stripping the leading archive dir) so the
/// espeak-ng-data directory and lexicons land intact.
fn extract_tts(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    let decoder = bzip2::read::BzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        // Strip the top-level archive directory component.
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry
            .unpack(&out)
            .map_err(|e| format!("unpack {}: {e}", rel.display()))?;
    }
    let model = TtsModel::under(dest.parent().and_then(|p| p.parent()).unwrap_or(dest));
    if model.all_present() {
        tracing::info!("oxy_tts: Kokoro model ready");
        Ok(())
    } else {
        Err("TTS extraction finished but expected files are missing".into())
    }
}
