// Exercise the vendored cache implementation with the app's locked runtime.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(dead_code)]
#[path = "../vendor/transcribe-rs/src/onnx/coreml.rs"]
mod coreml;
