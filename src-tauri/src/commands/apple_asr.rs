use crate::apple_asr::{self, CANCEL_DOWNLOAD, DOWNLOAD_LOCK};
use crate::managers::{audio::AudioRecordingManager, transcription::TranscriptionManager};
use crate::settings::{get_settings, write_settings, AppleAsrMode};
use std::sync::{atomic::Ordering, Arc};
use tauri::{AppHandle, Manager};

#[tauri::command]
#[specta::specta]
pub async fn change_apple_asr_options(
    app: AppHandle,
    mode: AppleAsrMode,
    prewarm: bool,
    keep_loaded: bool,
) -> Result<(), String> {
    let _download_guard = DOWNLOAD_LOCK.lock().await;
    if mode != AppleAsrMode::Off && !apple_asr::available() {
        return Err("Native Apple ASR requires an Apple Silicon build with the apple-native-asr feature and macOS 14+".into());
    }
    if app.state::<Arc<AudioRecordingManager>>().is_recording() {
        return Err("Finish recording before changing Apple acceleration".into());
    }
    if mode != AppleAsrMode::Off {
        CANCEL_DOWNLOAD.store(false, Ordering::Release);
        apple_asr::download_models(&app, mode)
            .await
            .map_err(|error| error.to_string())?;
    }
    let mut settings = get_settings(&app);
    settings.apple_asr_mode = mode;
    settings.apple_asr_prewarm = prewarm;
    settings.apple_asr_keep_loaded = keep_loaded;
    write_settings(&app, settings);
    let manager = app.state::<Arc<TranscriptionManager>>();
    manager.reload_model_on_next_use();
    // Apply at next dictation rather than replace an engine in an active batch.
    // A retained native profile is also preloaded at the next application launch.
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn cancel_apple_asr_download() {
    CANCEL_DOWNLOAD.store(true, Ordering::Release);
}
