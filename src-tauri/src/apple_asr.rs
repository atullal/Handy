//! Optional native Apple ASR provider. The same framed worker interface can host
//! a future Core AI implementation; no Apple framework is linked into Rust.
use crate::settings::AppleAsrMode;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
const MAX_SAMPLES: usize = 16_000 * 60 * 60;
pub static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);
pub static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Deserialize)]
struct Manifest {
    repo: String,
    revision: String,
    files: Vec<ModelFile>,
}

#[derive(Deserialize)]
struct ModelFile {
    path: String,
    size: u64,
    sha256: String,
}

fn manifest() -> Manifest {
    serde_json::from_str(include_str!("../resources/native-asr/model-manifest.json"))
        .expect("Checked-in native model manifest must be valid")
}

fn files_for_mode(manifest: &Manifest, mode: AppleAsrMode) -> Vec<&ModelFile> {
    manifest
        .files
        .iter()
        .filter(|file| match mode {
            AppleAsrMode::Off => false,
            AppleAsrMode::Gpu => !file.path.starts_with("parakeet_unified_encoder_int8."),
            AppleAsrMode::Neural => !file.path.starts_with("parakeet_unified_encoder."),
        })
        .collect()
}

pub fn supports_model(model_id: &str) -> bool {
    model_id.starts_with("handy-computer/parakeet-unified-en-0.6b-gguf/")
        && model_id.ends_with(".gguf")
}

pub fn available() -> bool {
    if !cfg!(all(
        feature = "apple-native-asr",
        target_os = "macos",
        target_arch = "aarch64"
    )) {
        return false;
    }
    // The worker targets macOS 14; don't offer it on older supported Handy OSes.
    Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 14)
}

pub fn model_directory(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?
        .join("models/native-parakeet-unified")
        .join(manifest().revision))
}

fn valid_file(path: &Path, expected: &ModelFile) -> Result<bool> {
    if !path.is_file() || path.metadata()?.len() != expected.size {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()) == expected.sha256)
}

pub fn validate_models(directory: &Path, mode: AppleAsrMode) -> Result<()> {
    for file in files_for_mode(&manifest(), mode) {
        if !valid_file(&directory.join(&file.path), file)? {
            bail!("Native model missing or checksum mismatch: {}. Download the Apple model in Advanced settings.", file.path);
        }
    }
    Ok(())
}

/// No downloads occur in the recording path. The setting command prepares and
/// verifies the selected export before saving the opt-in mode.
pub async fn download_models(app: &AppHandle, mode: AppleAsrMode) -> Result<()> {
    let manifest = manifest();
    let directory = model_directory(app)?;
    let files = files_for_mode(&manifest, mode);
    let total: u64 = files.iter().map(|file| file.size).sum();
    let mut completed = 0u64;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(3600))
        .build()?;
    for file in files {
        if CANCEL_DOWNLOAD.load(Ordering::Acquire) {
            bail!("Apple model download cancelled");
        }
        let path = directory.join(&file.path);
        if valid_file(&path, file)? {
            completed += file.size;
            continue;
        }
        std::fs::create_dir_all(path.parent().context("Invalid manifest path")?)?;
        let temporary = path.with_extension("handy-part");
        let result: Result<()> = async {
            let url = format!("https://huggingface.co/{}/resolve/{}/{}", manifest.repo, manifest.revision, file.path);
            let mut response = client.get(url).send().await?.error_for_status()?;
            let mut output = File::create(&temporary)?;
            let mut hash = Sha256::new();
            let mut received = 0u64;
            let mut last_event = std::time::Instant::now();
            while let Some(chunk) = response.chunk().await? {
                if CANCEL_DOWNLOAD.load(Ordering::Acquire) { bail!("Apple model download cancelled"); }
                received += chunk.len() as u64;
                if received > file.size { bail!("Native model download exceeds manifest size"); }
                hash.update(&chunk);
                output.write_all(&chunk)?;
                if last_event.elapsed() >= Duration::from_millis(200) {
                    let _ = app.emit("apple-asr-download-progress", serde_json::json!({"downloaded_bytes": completed + received, "total_bytes": total}));
                    last_event = std::time::Instant::now();
                }
            }
            if received != file.size || format!("{:x}", hash.finalize()) != file.sha256 {
                bail!("Native model download checksum mismatch: {}", file.path);
            }
            output.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            Ok(())
        }.await;
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        completed += file.size;
    }
    let _ = app.emit(
        "apple-asr-download-progress",
        serde_json::json!({"downloaded_bytes": total, "total_bytes": total}),
    );
    Ok(())
}

#[derive(Clone)]
pub struct NativeOverride {
    pub mode: AppleAsrMode,
    pub model_directory: Option<PathBuf>,
    pub prewarm: bool,
}

/// Lives inside LoadedEngine. Unloading/dropping it terminates the worker and
/// releases its models; audio never leaves this machine.
pub struct NativeAsrClient {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Result<serde_json::Value>>,
    pub mode: AppleAsrMode,
    pub load_ms: f64,
    pub warmup_ms: f64,
    pub fallback_model: Option<PathBuf>,
}

fn read_reply(reader: &mut impl Read) -> Result<serde_json::Value> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .context("Native worker closed its output")?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_REPLY_BYTES {
        bail!("Invalid native worker response length");
    }
    let mut data = vec![0u8; length];
    reader.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

impl NativeAsrClient {
    pub fn load(
        app: &AppHandle,
        directory: &Path,
        mode: AppleAsrMode,
        prewarm: bool,
    ) -> Result<Self> {
        if mode == AppleAsrMode::Off || !available() {
            bail!("Native Apple ASR is unavailable; build with --features apple-native-asr on Apple Silicon macOS 14+");
        }
        validate_models(directory, mode)?;
        let worker = app.path().resolve(
            "resources/native-asr/HandyNativeAsrWorker",
            tauri::path::BaseDirectory::Resource,
        )?;
        let warmup = app.path().resolve(
            "resources/native-asr/warmup.wav",
            tauri::path::BaseDirectory::Resource,
        )?;
        Self::spawn(&worker, directory, mode, &warmup, prewarm)
    }

    fn spawn(
        worker: &Path,
        directory: &Path,
        mode: AppleAsrMode,
        warmup: &Path,
        prewarm: bool,
    ) -> Result<Self> {
        let mode_arg = match mode {
            AppleAsrMode::Gpu => "gpu",
            AppleAsrMode::Neural => "neural",
            AppleAsrMode::Off => bail!("Native mode is off"),
        };
        let mut child = Command::new(worker)
            .arg(directory)
            .arg(mode_arg)
            .arg(warmup)
            .arg(if prewarm { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Cannot launch bundled native ASR worker")?;
        let input = child.stdin.take().context("Native worker has no input")?;
        let mut output = child.stdout.take().context("Native worker has no output")?;
        let (tx, replies) = mpsc::sync_channel(1);
        std::thread::spawn(move || loop {
            let reply = read_reply(&mut output);
            let failed = reply.is_err();
            if tx.send(reply).is_err() || failed {
                break;
            }
        });
        let mut client = Self {
            child,
            input,
            replies,
            mode,
            load_ms: 0.0,
            warmup_ms: 0.0,
            fallback_model: None,
        };
        let ready = client.receive(Duration::from_secs(180))?;
        if ready["ready"] != true || ready["protocol"] != 1 || ready["mode"] != mode_arg {
            bail!("Incompatible native worker handshake");
        }
        client.load_ms = ready["load_ms"].as_f64().unwrap_or_default();
        client.warmup_ms = ready["warmup_ms"].as_f64().unwrap_or_default();
        Ok(client)
    }

    fn receive(&mut self, timeout: Duration) -> Result<serde_json::Value> {
        let result = (|| -> Result<serde_json::Value> {
            let reply = self
                .replies
                .recv_timeout(timeout)
                .context("Native ASR worker timed out or exited")??;
            if let Some(error) = reply.get("error").and_then(|e| e.as_str()) {
                bail!("Native ASR: {error}");
            }
            Ok(reply)
        })();
        if result.is_err() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        result
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        if samples.len() > MAX_SAMPLES || samples.iter().any(|value| !value.is_finite()) {
            bail!("Native audio must be finite 16 kHz PCM, at most one hour");
        }
        self.input
            .write_all(&(samples.len() as u32).to_le_bytes())?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.input.write_all(&bytes)?;
        self.input.flush()?;
        let reply = self.receive(Duration::from_secs(300))?;
        reply["text"]
            .as_str()
            .map(str::to_string)
            .context("Native worker omitted transcript")
    }
}

impl Drop for NativeAsrClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn export_selection_cannot_route_int8_to_gpu() {
        let data = manifest();
        for (mode, encoder) in [
            (AppleAsrMode::Gpu, "parakeet_unified_encoder.mlmodelc/"),
            (
                AppleAsrMode::Neural,
                "parakeet_unified_encoder_int8.mlmodelc/",
            ),
        ] {
            let files = files_for_mode(&data, mode);
            assert_eq!(files.len(), 14);
            assert!(files.iter().any(|file| file.path.starts_with(encoder)));
            for file in files {
                assert!(!Path::new(&file.path).is_absolute());
                assert!(Path::new(&file.path)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))));
                assert_eq!(file.sha256.len(), 64);
            }
        }
        assert!(files_for_mode(&data, AppleAsrMode::Off).is_empty());
    }

    #[test]
    fn protocol_rejects_truncated_and_oversized_replies() {
        assert!(read_reply(&mut Cursor::new([2u8, 0, 0, 0, b'{'])).is_err());
        assert!(read_reply(&mut Cursor::new(u32::MAX.to_le_bytes())).is_err());
        let json = br#"{"text":"hello"}"#;
        let mut frame = (json.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(json);
        assert_eq!(
            read_reply(&mut Cursor::new(frame)).unwrap()["text"],
            "hello"
        );
    }

    #[test]
    fn only_the_benchmarked_model_family_is_substituted() {
        assert!(supports_model(
            "handy-computer/parakeet-unified-en-0.6b-gguf/parakeet-unified-en-0.6b-Q8_0.gguf"
        ));
        assert!(!supports_model("whisper-small"));
        assert!(!supports_model("parakeet-tdt-0.6b-v3"));
    }

    #[test]
    fn integrity_check_detects_same_size_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("weight.bin");
        std::fs::write(&path, b"good").unwrap();
        let expected = ModelFile {
            path: "weight.bin".into(),
            size: 4,
            sha256: format!("{:x}", Sha256::digest(b"good")),
        };
        assert!(valid_file(&path, &expected).unwrap());
        std::fs::write(&path, b"evil").unwrap();
        assert!(!valid_file(&path, &expected).unwrap());
    }
}
