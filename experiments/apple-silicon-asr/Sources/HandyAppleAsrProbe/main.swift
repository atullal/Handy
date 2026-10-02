import CoreML
import Darwin
import FluidAudio
import Foundation

// An isolated file-inference probe; never changes Handy's settings or installed app.
@main
struct NativeAsrProbe {
  static func milliseconds(since start: UInt64) -> Double {
    Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
  }

  static func main() async {
    do {
      try await transcribeFile()
    } catch {
      FileHandle.standardError.write(Data("error: \(error.localizedDescription)\n".utf8))
      exit(1)
    }
  }

  static func transcribeFile() async throws {
    let args = CommandLine.arguments
    guard args.count == 5 else {
      throw NSError(
        domain: "NativeAsrProbe", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey:
            "Usage: probe <model-directory> <audio.wav> <neural|cpu|gpu|all> <int8|fp16>"
        ])
    }
    let configuration = MLModelConfiguration()
    switch args[3] {
    case "neural": configuration.computeUnits = .cpuAndNeuralEngine
    case "cpu": configuration.computeUnits = .cpuOnly
    case "gpu": configuration.computeUnits = .cpuAndGPU
    case "all": configuration.computeUnits = .all
    default:
      throw NSError(
        domain: "NativeAsrProbe", code: 2,
        userInfo: [NSLocalizedDescriptionKey: "Unknown compute units"])
    }
    guard args[4] == "int8" || args[4] == "fp16" else {
      throw NSError(
        domain: "NativeAsrProbe", code: 3,
        userInfo: [NSLocalizedDescriptionKey: "Unknown encoder precision"])
    }
    guard args[4] != "int8" || args[3] == "neural" || args[3] == "cpu" else {
      throw NSError(
        domain: "NativeAsrProbe", code: 4,
        userInfo: [
          NSLocalizedDescriptionKey:
            "FluidAudio documents a GPU crash with this INT8 encoder; use CPU/ANE or FP16"
        ])
    }
    let converter = AudioConverter(sampleRate: 16_000)
    let samples = try converter.resampleAudioFile(URL(fileURLWithPath: args[2]))
    configuration.allowLowPrecisionAccumulationOnGPU = true
    let manager = UnifiedAsrManager(
      configuration: configuration,
      encoderPrecision: args[4] == "int8" ? .int8 : .fp16)
    fflush(nil)
    let savedStdout = dup(STDOUT_FILENO)
    guard savedStdout >= 0, dup2(STDERR_FILENO, STDOUT_FILENO) >= 0 else {
      throw NSError(
        domain: "NativeAsrProbe", code: 5,
        userInfo: [NSLocalizedDescriptionKey: "Cannot capture native diagnostics"])
    }
    defer {
      fflush(nil)
      dup2(savedStdout, STDOUT_FILENO)
      close(savedStdout)
    }
    let start = DispatchTime.now().uptimeNanoseconds
    try await manager.loadModels(from: URL(fileURLWithPath: args[1]))
    let loadMs = milliseconds(since: start)
    var times: [Double] = []
    var transcripts: [String] = []
    for _ in 0..<6 {
      let inference = DispatchTime.now().uptimeNanoseconds
      let text = try await manager.transcribe(samples)
      times.append(milliseconds(since: inference))
      transcripts.append(text)
    }
    let output: [String: Any] = [
      "engine": "FluidAudio native Core ML Parakeet Unified offline",
      "encoder_compute_units": args[3], "encoder_precision": args[4],
      "decoder_compute_units": "cpu", "audio_s": Double(samples.count) / 16_000,
      "load_ms": loadMs, "transcribe_ms": times, "transcripts": transcripts,
      "method":
        "First inference plus five warm repeats; end-to-end model inference including Swift mel features and RNNT decoding, excluding audio read/resampling.",
    ]
    let data = try JSONSerialization.data(
      withJSONObject: output, options: [.prettyPrinted, .sortedKeys])
    fflush(nil)
    dup2(savedStdout, STDOUT_FILENO)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
  }
}
