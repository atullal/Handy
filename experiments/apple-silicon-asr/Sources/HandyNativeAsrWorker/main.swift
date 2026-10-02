import CoreML
import Darwin
import FluidAudio
import Foundation

// Long-lived, single-engine worker. Binary PCM in; length-prefixed JSON out.
// Core AI can implement this same protocol without changing Handy's audio path.
@main
struct NativeWorker {
  static let maxSamples = 16_000 * 60 * 60

  static func elapsed(_ start: UInt64) -> Double {
    Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
  }

  static func readExactly(_ count: Int) throws -> Data? {
    var data = Data()
    while data.count < count {
      let part = try FileHandle.standardInput.read(upToCount: count - data.count) ?? Data()
      if part.isEmpty {
        if data.isEmpty { return nil }
        throw NSError(
          domain: "IPC", code: 1, userInfo: [NSLocalizedDescriptionKey: "Truncated request"])
      }
      data.append(part)
    }
    return data
  }

  static func respond(_ value: [String: Any], to output: FileHandle) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    var length = UInt32(data.count).littleEndian
    try withUnsafeBytes(of: &length) { try output.write(contentsOf: Data($0)) }
    try output.write(contentsOf: data)
  }

  static func main() async {
    // Keep third-party native diagnostics off the framed protocol stream.
    fflush(nil)
    let descriptor = dup(STDOUT_FILENO)
    guard descriptor >= 0 else { exit(1) }
    let output = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    guard dup2(STDERR_FILENO, STDOUT_FILENO) >= 0 else { exit(1) }
    do {
      try await serve(output)
    } catch {
      try? respond(["error": error.localizedDescription], to: output)
      exit(1)
    }
  }

  static func serve(_ output: FileHandle) async throws {
    let args = CommandLine.arguments
    guard args.count == 5, args[2] == "gpu" || args[2] == "neural" else {
      throw NSError(
        domain: "IPC", code: 2,
        userInfo: [
          NSLocalizedDescriptionKey: "Expected model directory, gpu/neural, warmup WAV, warmup flag"
        ])
    }
    let config = MLModelConfiguration()
    config.computeUnits = args[2] == "gpu" ? .all : .cpuAndNeuralEngine
    config.allowLowPrecisionAccumulationOnGPU = true
    let manager = UnifiedAsrManager(
      configuration: config, encoderPrecision: args[2] == "gpu" ? .fp16 : .int8)
    let loadStart = DispatchTime.now().uptimeNanoseconds
    try await manager.loadModels(from: URL(fileURLWithPath: args[1]))
    let loadMs = elapsed(loadStart)
    var warmMs = 0.0
    if args[4] == "1" {
      let samples = try AudioConverter(sampleRate: 16_000).resampleAudioFile(
        URL(fileURLWithPath: args[3]))
      let start = DispatchTime.now().uptimeNanoseconds
      _ = try await manager.transcribe(samples)
      warmMs = elapsed(start)
    }
    try respond(
      ["ready": true, "protocol": 1, "mode": args[2], "load_ms": loadMs, "warmup_ms": warmMs],
      to: output)
    while let header = try readExactly(4) {
      let count = Int(
        header.withUnsafeBytes { UInt32(littleEndian: $0.loadUnaligned(as: UInt32.self)) })
      guard count > 0, count <= maxSamples, let bytes = try readExactly(count * 4) else {
        throw NSError(
          domain: "IPC", code: 3, userInfo: [NSLocalizedDescriptionKey: "Invalid PCM frame length"])
      }
      let samples: [Float] = bytes.withUnsafeBytes { buffer in
        (0..<count).map { index in
          Float(
            bitPattern: UInt32(
              littleEndian: buffer.loadUnaligned(fromByteOffset: index * 4, as: UInt32.self)))
        }
      }
      guard samples.allSatisfy({ $0.isFinite }) else {
        throw NSError(
          domain: "IPC", code: 4, userInfo: [NSLocalizedDescriptionKey: "Non-finite PCM input"])
      }
      do {
        let start = DispatchTime.now().uptimeNanoseconds
        let text = try await manager.transcribe(samples)
        try respond(["text": text, "inference_ms": elapsed(start)], to: output)
      } catch {
        // Continue after an ordinary inference error; the Rust host may switch to Metal.
        try respond(["error": error.localizedDescription], to: output)
      }
    }
  }
}
