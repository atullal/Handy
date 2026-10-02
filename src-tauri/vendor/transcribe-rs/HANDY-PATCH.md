# Handy Core ML patch

Source: crates.io `transcribe-rs` 0.3.8, MIT licensed (see LICENSE).
The source, build script, examples and tests are preserved; upstream sample audio
is omitted. Cargo.toml is the original upstream manifest plus optional SHA-256.

Local changes:

- `src/onnx/coreml.rs`: runtime availability, MLProgram, configurable All or
  CPUAndNeuralEngine compute units, optional compute-plan diagnostics, and a
  content-addressed cache that includes external weights.
- `src/onnx/mod.rs`: exports the optional configuration module.
- `src/onnx/session.rs`: applies configured Core ML providers to explicit CoreMl
  sessions and surfaces provider registration failures. Auto remains CPU on
  macOS: dynamic and quantized speech graphs need benchmarking before opting in.

CPU remains available for operators Core ML cannot execute. Selecting
CPUAndNeuralEngine does not establish that every operator executes on the ANE.
Keep these changes isolated when updating the vendored dependency.
