//! Voice input for the search boxes: Hebrew speech recognition with the ivrit-ai Whisper model
//! (large-v3-turbo fine-tuned on Hebrew, Apache-2.0), run locally through whisper.cpp.
//!
//! The model is optional data: about 550 MB (q5_0), downloaded from the project's GitLab package
//! registry into `<data>/speech/` and checked against its SHA-256. The webview records the
//! utterance (16 kHz mono PCM) and hands it to `transcribe`. On Android the system recognizer
//! (Google) is used first when online; this model is the offline path there.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::data::{download, Cancel};
use crate::paths::DataPaths;
use crate::types::ProgressFn;

/// Model file (whisper.cpp ggml, quantised q5_0 from ivrit-ai/whisper-large-v3-turbo-ggml).
pub const MODEL_FILE: &str = "ggml-ivrit-large-v3-turbo-q5_0.bin";
/// Where the model is published: GitHub (release "speech-model-1"), else GitLab's registry.
pub const MODEL_URLS: [&str; 2] = [
    "https://github.com/yossi8765/MapsOffline/releases/download/speech-model-1/ggml-ivrit-large-v3-turbo-q5_0.bin",
    "https://gitlab.com/api/v4/projects/RecoveryAdvancedFree%2FMapsOffline/packages/generic/speech-model/1/ggml-ivrit-large-v3-turbo-q5_0.bin",
];
pub const MODEL_SHA256: &str = "6c1da92e8e41dd64b8cc402eee7eb7a433d2152567e1a4d9cf181fefcc67a572";
pub const MODEL_BYTES: u64 = 574_041_195;

/// Samples per second the webview sends (mono, 16-bit).
pub const SAMPLE_RATE: usize = 16_000;
/// At most 30 s of audio (one Whisper window).
const MAX_SAMPLES: usize = 30 * SAMPLE_RATE;

pub fn model_path(paths: &DataPaths) -> PathBuf {
    paths.root.join("speech").join(MODEL_FILE)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechStatus {
    /// The offline model is installed.
    pub installed: bool,
    /// Size of the installed model (bytes).
    pub size_bytes: u64,
    /// Size of the download.
    pub download_bytes: u64,
    /// A download is running.
    pub busy: bool,
    /// The operating system offers its own recognizer (Android: Google).
    pub system_available: bool,
}

static BUSY: AtomicBool = AtomicBool::new(false);
static CANCEL: Mutex<Option<Cancel>> = Mutex::new(None);

pub fn status(paths: &DataPaths) -> SpeechStatus {
    let p = model_path(paths);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    SpeechStatus {
        installed: size > 0,
        size_bytes: size,
        download_bytes: MODEL_BYTES,
        busy: BUSY.load(Ordering::SeqCst),
        system_available: cfg!(target_os = "android"),
    }
}

fn sha256_file(p: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download the model (resumable, through `downloads/`), check it, move it into place.
pub fn install(paths: &DataPaths, progress: ProgressFn) -> anyhow::Result<()> {
    if BUSY.swap(true, Ordering::SeqCst) {
        bail!("SPEECH_BUSY");
    }
    let cancel = Cancel::default();
    *CANCEL.lock() = Some(cancel.clone());
    let r = (|| -> anyhow::Result<()> {
        let dest = model_path(paths);
        let tmp = paths.downloads_dir().join(MODEL_FILE);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
            crate::data::disk::ensure_free(dir, MODEL_BYTES.saturating_mul(2))?;
        }
        let mut got = Err(anyhow::anyhow!("no model source"));
        for url in MODEL_URLS {
            got = download::ensure_fresh(url, &tmp, progress, &cancel);
            if got.is_ok() || cancel.is_cancelled() {
                break;
            }
            log::warn!("speech model from {url}: {:#}", got.as_ref().err().map(|e| e.to_string()).unwrap_or_default());
        }
        got?;
        if cancel.is_cancelled() {
            bail!("בוטל");
        }
        progress(crate::types::ProgressPhase::Finalizing, 0, 0, "מאמת את המודל");
        let got = sha256_file(&tmp)?;
        if !MODEL_SHA256.is_empty() && got != MODEL_SHA256 {
            let _ = std::fs::remove_file(&tmp);
            let _ = std::fs::remove_file(download::meta_path(&tmp));
            bail!("CHECKSUM_MISMATCH: speech model {got}");
        }
        unload();
        std::fs::rename(&tmp, &dest).or_else(|_| std::fs::copy(&tmp, &dest).map(|_| ()).and_then(|_| std::fs::remove_file(&tmp)))?;
        let _ = std::fs::remove_file(download::meta_path(&tmp));
        let size = std::fs::metadata(&dest)?.len();
        progress(crate::types::ProgressPhase::Done, size, size, "הושלם");
        Ok(())
    })();
    *CANCEL.lock() = None;
    BUSY.store(false, Ordering::SeqCst);
    r
}

pub fn cancel() {
    if let Some(c) = CANCEL.lock().as_ref() {
        c.cancel();
    }
}

pub fn delete(paths: &DataPaths) -> anyhow::Result<()> {
    unload();
    let p = model_path(paths);
    if p.exists() {
        std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))?;
    }
    Ok(())
}

/// The loaded model (≈ 0.6–1 GB in memory): kept after the first use, dropped on delete / update.
static CTX: Mutex<Option<(PathBuf, whisper_rs::WhisperContext)>> = Mutex::new(None);

fn unload() {
    *CTX.lock() = None;
}

/// Hebrew text of a recorded utterance: 16-bit little-endian mono PCM at 16 kHz.
pub fn transcribe(paths: &DataPaths, pcm_le16: &[u8]) -> anyhow::Result<String> {
    let model = model_path(paths);
    if !model.exists() {
        bail!("SPEECH_NO_MODEL");
    }
    let mut samples: Vec<f32> = pcm_le16.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
    samples.truncate(MAX_SAMPLES);
    if samples.len() < SAMPLE_RATE / 4 {
        return Ok(String::new());
    }
    // Whisper wants at least 1 s: pad short utterances with silence
    if samples.len() < SAMPLE_RATE + SAMPLE_RATE / 10 {
        samples.resize(SAMPLE_RATE + SAMPLE_RATE / 10, 0.0);
    }

    let mut guard = CTX.lock();
    if guard.as_ref().map(|(p, _)| p != &model).unwrap_or(true) {
        // whisper.cpp prints every decoded token to stderr: route its log to nothing
        whisper_rs::install_logging_hooks();
        let t0 = std::time::Instant::now();
        let ctx = whisper_rs::WhisperContext::new_with_params(&model, whisper_rs::WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("SPEECH_MODEL_LOAD: {e}"))?;
        log::info!("speech model loaded in {:.1} s", t0.elapsed().as_secs_f32());
        *guard = Some((model.clone(), ctx));
    }
    let (_, ctx) = guard.as_ref().expect("model loaded above");
    let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("speech state: {e}"))?;
    let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("he"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(true);
    params.set_suppress_blank(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    // The encoder works on a fixed 30 s window (1500 frames) whatever the length: a short query
    // padded to 30 s takes minutes on a phone. A window sized to the audio (+ margin) costs a
    // fraction and keeps the accuracy on short phrases.
    params.set_audio_ctx(audio_ctx(samples.len()));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    params.set_n_threads(threads as i32);
    let t0 = std::time::Instant::now();
    state.full(params, &samples).map_err(|e| anyhow::anyhow!("speech recognition: {e}"))?;
    let mut text = String::new();
    for seg in state.as_iter() {
        text.push_str(&seg.to_string());
    }
    let text = clean(&text);
    log::info!("speech: {:.1} s audio -> {} chars in {:.1} s", samples.len() as f32 / SAMPLE_RATE as f32, text.chars().count(), t0.elapsed().as_secs_f32());
    Ok(text)
}

/// Encoder frames for `n` samples: 50 per second of audio, plus a margin, at most the full 1500.
fn audio_ctx(n: usize) -> i32 {
    let frames = n * 1500 / (30 * SAMPLE_RATE);
    (frames + 128).clamp(256, 1500) as i32
}

/// Standard Base64 (with or without padding) → bytes; None on any other character.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let bytes = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
        if chunk.len() == 1 {
            return None;
        }
    }
    Some(out)
}

/// Whisper's habits on short queries: a trailing full stop, bracketed noise tags.
fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(['.', '。', '!']).trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_roundtrip() {
        assert_eq!(super::base64_decode("SGVsbG8="), Some(b"Hello".to_vec()));
        assert_eq!(super::base64_decode("SGVsbG8"), Some(b"Hello".to_vec()));
        assert_eq!(super::base64_decode("AAEC/w=="), Some(vec![0, 1, 2, 255]));
        assert_eq!(super::base64_decode("**"), None);
    }

    #[test]
    fn cleans_whisper_output() {
        assert_eq!(super::clean(" דיזנגוף 50 תל אביב."), "דיזנגוף 50 תל אביב");
        assert_eq!(super::clean("[מוזיקה] בית כנסת  "), "בית כנסת");
    }
}
