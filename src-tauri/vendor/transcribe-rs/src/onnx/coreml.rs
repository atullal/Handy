//! Core ML configuration shared by the ONNX engines.
//!
//! Explicitly selecting CPU + Neural Engine still permits CPU execution for
//! unsupported operations. It is not a guarantee of exclusive ANE execution.

use ort::ep::{coreml::ComputeUnits, coreml::ModelFormat, CoreML, ExecutionProvider};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Clone, Debug, Default)]
pub struct CoreMlOptions {
    pub neural_engine_only: bool,
    pub cache_directory: Option<PathBuf>,
    pub profile_compute_plan: bool,
}

static OPTIONS: RwLock<CoreMlOptions> = RwLock::new(CoreMlOptions {
    neural_engine_only: false,
    cache_directory: None,
    profile_compute_plan: false,
});

/// Configure before loading an ONNX model, just like the accelerator preference.
pub fn configure(options: CoreMlOptions) {
    *OPTIONS.write().unwrap_or_else(|e| e.into_inner()) = options;
}

/// Check the linked runtime, rather than advertising a compile-time feature alone.
pub fn is_available() -> bool {
    CoreML::default().is_available().unwrap_or(false)
}

pub(super) fn provider(model_path: &Path) -> CoreML {
    let options = OPTIONS.read().unwrap_or_else(|e| e.into_inner()).clone();
    let units = if options.neural_engine_only {
        ComputeUnits::CPUAndNeuralEngine
    } else {
        ComputeUnits::All
    };
    let mut provider = CoreML::default()
        .with_model_format(ModelFormat::MLProgram)
        .with_compute_units(units)
        .with_profile_compute_plan(options.profile_compute_plan);

    // Content-address the entire model directory, including ONNX external
    // weights. ORT's path-based default hash alone misses in-place weight updates.
    if let Some(root) = &options.cache_directory {
        match cache_path(root, model_path, options.neural_engine_only) {
            Ok(cache) => {
                if let Err(error) = std::fs::create_dir_all(&cache) {
                    log::warn!("Core ML cache unavailable: {error}; compiling without cache");
                } else {
                    provider = provider.with_model_cache_dir(cache.to_string_lossy());
                }
            }
            Err(error) => log::warn!("Core ML cache fingerprint failed: {error}"),
        }
    }
    log::info!(
        "Core ML requested for {}: {:?}; unsupported operations may use CPU",
        model_path.display(),
        units
    );
    provider
}

fn cache_path(root: &Path, model: &Path, neural_engine_only: bool) -> std::io::Result<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(model.parent().unwrap_or(Path::new(".")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    let mut hash = Sha256::new();
    hash.update(b"handy-coreml-ort-rc12-mlprogram-v1");
    hash.update([u8::from(neural_engine_only)]);
    hash.update(model.file_name().unwrap_or_default().as_encoded_bytes());
    for path in files {
        hash.update(path.file_name().unwrap_or_default().as_encoded_bytes());
        let mut file = std::fs::File::open(&path)?;
        hash.update(file.metadata()?.len().to_le_bytes());
        let mut buffer = [0; 65536];
        loop {
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            hash.update(&buffer[..size]);
        }
    }
    Ok(root.join(format!("{:x}", hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_invalidates_for_in_place_external_weight_updates_and_compute_units() {
        let dir =
            std::env::temp_dir().join(format!("handy-coreml-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("model.onnx");
        let weights = dir.join("weights.data");
        std::fs::write(&model, b"graph").unwrap();
        std::fs::write(&weights, b"aaaa").unwrap();
        let root = dir.join("cache");
        let first = cache_path(&root, &model, false).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        assert_eq!(first, cache_path(&root, &model, false).unwrap());
        std::fs::write(&weights, b"bbbb").unwrap();
        assert_ne!(first, cache_path(&root, &model, false).unwrap());
        assert_ne!(
            cache_path(&root, &model, false).unwrap(),
            cache_path(&root, &model, true).unwrap()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
