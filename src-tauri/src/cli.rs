use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, Default)]
#[command(name = "handy", about = "Handy - Speech to Text")]
pub struct CliArgs {
    /// Start with the main window hidden
    #[arg(long)]
    pub start_hidden: bool,

    /// Disable the system tray icon
    #[arg(long)]
    pub no_tray: bool,

    /// Toggle transcription on/off (sent to running instance)
    #[arg(long)]
    pub toggle_transcription: bool,

    /// Toggle transcription with post-processing on/off (sent to running instance)
    #[arg(long)]
    pub toggle_post_process: bool,

    /// Cancel the current operation (sent to running instance)
    #[arg(long)]
    pub cancel: bool,

    /// Enable debug mode with verbose logging
    #[arg(long)]
    pub debug: bool,

    /// Transcribe this WAV (16 kHz mono) headlessly and exit. Runs the same
    /// batch transcription path as the app — no mic, no VAD, no download
    /// (the model must already be installed).
    #[arg(short = 'f', long, value_name = "WAV")]
    pub transcribe_file: Option<PathBuf>,

    /// Model id to load for --transcribe-file (default: the selected model).
    #[arg(long)]
    pub model: Option<String>,

    /// Hard-select the compute device for --transcribe-file by its registry
    /// index (see --list-devices). Omit to use the persisted accelerator
    /// setting. transcribe-cpp (whisper-family) models only.
    #[arg(long, value_name = "N")]
    pub device_index: Option<usize>,

    /// Override ONNX acceleration for --transcribe-file without saving settings.
    #[arg(long, requires = "transcribe_file")]
    pub ort_accelerator: Option<crate::settings::OrtAcceleratorSetting>,

    /// Optional native Apple Parakeet override: off, gpu (FP16), neural (INT8).
    #[arg(long, requires = "transcribe_file", conflicts_with = "ort_accelerator")]
    pub apple_asr: Option<crate::settings::AppleAsrMode>,

    /// Native model directory (pinned files verified before loading). No download.
    #[arg(long, requires = "apple_asr")]
    pub native_model_dir: Option<PathBuf>,

    /// Warm the native worker once during model load; time is reported separately.
    #[arg(long, requires = "apple_asr")]
    pub apple_prewarm: bool,

    /// List the transcribe-cpp compute devices (with indices) and exit.
    #[arg(long)]
    pub list_devices: bool,

    /// List the available models (with ids) and exit. Pass an id to --model.
    /// Honors --json for machine-readable output.
    #[arg(long)]
    pub list_models: bool,

    /// Repeat the transcription N times (best_ms reports the fastest run).
    #[arg(long, value_name = "N")]
    pub repeat: Option<usize>,

    /// Idle between benchmark runs to inspect warm-state retention; excluded from timings.
    #[arg(long, requires_all = ["transcribe_file", "repeat"])]
    pub repeat_delay_ms: Option<u64>,

    /// Emit --transcribe-file results as JSON.
    #[arg(long)]
    pub json: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::OrtAcceleratorSetting;

    #[test]
    fn native_benchmark_flags_require_file_mode_and_reject_mixed_accelerators() {
        assert!(CliArgs::try_parse_from(["handy", "--apple-asr", "gpu"]).is_err());
        assert!(CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "sample.wav",
            "--apple-asr",
            "gpu",
            "--ort-accelerator",
            "cpu"
        ])
        .is_err());
        let args = CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "sample.wav",
            "--apple-asr",
            "neural",
            "--apple-prewarm",
            "--repeat",
            "2",
            "--repeat-delay-ms",
            "10",
        ])
        .unwrap();
        assert_eq!(args.apple_asr, Some(crate::settings::AppleAsrMode::Neural));
        assert_eq!(args.repeat_delay_ms, Some(10));
        assert!(args.apple_prewarm);
    }

    #[test]
    fn ort_override_is_restricted_to_headless_file_transcription() {
        assert!(CliArgs::try_parse_from(["handy", "--ort-accelerator", "coreml"]).is_err());
        let args = CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "sample.wav",
            "--ort-accelerator",
            "coreml_neural_engine",
        ])
        .unwrap();
        assert_eq!(
            args.ort_accelerator,
            Some(OrtAcceleratorSetting::CoreMlNeuralEngine)
        );
    }

    #[test]
    fn unknown_ort_override_is_rejected() {
        assert!(CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "sample.wav",
            "--ort-accelerator",
            "invalid",
        ])
        .is_err());
    }
}
