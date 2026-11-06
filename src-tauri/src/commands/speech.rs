//! Voice input for the search boxes (`crate::speech`): offline model status / download / delete,
//! recognition of a recorded utterance, and the system recognizer where there is one (Android).
use super::prelude::*;
use crate::speech;

/// Offline speech model: installed / size / download running; whether the OS has a recognizer.
#[tauri::command]
pub fn speech_status(state: State<'_, Arc<AppState>>) -> R<speech::SpeechStatus> {
    Ok(speech::status(&state.paths()))
}

/// Download the offline model (~550 MB). Progress: event `speech-progress` {phase, done, total, message}.
#[tauri::command]
pub async fn speech_install(app: AppHandle, state: State<'_, Arc<AppState>>) -> R<()> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || {
        let progress = |phase: ProgressPhase, done: u64, total: u64, message: &str| -> bool {
            let _ = app.emit("speech-progress", serde_json::json!({ "phase": phase, "done": done, "total": total, "message": message }));
            true
        };
        speech::install(&paths, &progress).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn speech_cancel() -> R<()> {
    speech::cancel();
    Ok(())
}

#[tauri::command]
pub fn speech_delete(state: State<'_, Arc<AppState>>) -> R<()> {
    speech::delete(&state.paths()).map_err(err)
}

/// Text of a recorded utterance: `pcm` = Base64 of 16-bit little-endian mono PCM at 16 kHz.
/// (Base64 in JSON, not a raw body: Android's webview cannot hand request bodies to the app, so
/// Tauri sends every call there as JSON.) Errors: SPEECH_NO_MODEL.
#[tauri::command]
pub async fn speech_transcribe(state: State<'_, Arc<AppState>>, pcm: String) -> R<String> {
    let pcm = speech::base64_decode(&pcm).ok_or_else(|| AppError::from("INVALID_ARGS: pcm is not Base64"))?;
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || speech::transcribe(&paths, &pcm).map_err(err)).await.map_err(err)?
}

/// The operating system's own recognizer (Android: Google, online or with its offline pack).
/// Returns the text, or "" when the user cancelled. Elsewhere: SPEECH_UNAVAILABLE.
#[tauri::command]
pub async fn speech_system(lang: String) -> R<String> {
    let _ = lang;
    Err("SPEECH_UNAVAILABLE".into())
}
