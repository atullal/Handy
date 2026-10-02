// Inspect predicted operation placement; this does not measure hardware occupancy.
// Build: swiftc -O -parse-as-library scripts/inspect-coreml-plan.swift -o /tmp/inspect-coreml-plan
// Run: /tmp/inspect-coreml-plan <model.mlmodelc-or-directory> <all|neural|gpu|cpu>
import CoreML
import Darwin
import Foundation

@main
struct PlanInspector {
  static func deviceName(_ device: MLComputeDevice) -> String {
    switch device {
    case .cpu: return "cpu"
    case .gpu: return "gpu"
    case .neuralEngine: return "neural_engine"
    @unknown default: return "unknown"
    }
  }

  static func main() async {
    do {
      try await inspectPlans()
    } catch {
      FileHandle.standardError.write(Data("error: \(error.localizedDescription)\n".utf8))
      exit(1)
    }
  }

  static func inspectPlans() async throws {
    guard
      CommandLine.arguments.count == 3
        || (CommandLine.arguments.count == 4
          && CommandLine.arguments[3] == "--low-precision-gpu")
    else {
      throw NSError(
        domain: "PlanInspector", code: 1,
        userInfo: [NSLocalizedDescriptionKey: "Expected model/directory and compute-unit mode"])
    }
    let root = URL(fileURLWithPath: CommandLine.arguments[1])
    let mode = CommandLine.arguments[2]
    let configuration = MLModelConfiguration()
    configuration.allowLowPrecisionAccumulationOnGPU = CommandLine.arguments.count == 4
    switch mode {
    case "all": configuration.computeUnits = .all
    case "neural": configuration.computeUnits = .cpuAndNeuralEngine
    case "gpu": configuration.computeUnits = .cpuAndGPU
    case "cpu": configuration.computeUnits = .cpuOnly
    default:
      throw NSError(
        domain: "PlanInspector", code: 2,
        userInfo: [NSLocalizedDescriptionKey: "Unknown compute-unit mode"])
    }
    var urls: [URL] = []
    if root.pathExtension == "mlmodelc" {
      urls = [root]
    } else if let enumerator = FileManager.default.enumerator(
      at: root, includingPropertiesForKeys: nil)
    {
      while let url = enumerator.nextObject() as? URL {
        if url.pathExtension == "mlmodelc" {
          urls.append(url)
          enumerator.skipDescendants()
        }
      }
    }
    guard !urls.isEmpty else {
      throw NSError(
        domain: "PlanInspector", code: 3,
        userInfo: [NSLocalizedDescriptionKey: "No compiled Core ML models found"])
    }
    // Core ML can print native diagnostics to stdout; keep the result valid JSON.
    fflush(nil)
    let savedStdout = dup(STDOUT_FILENO)
    guard savedStdout >= 0, dup2(STDERR_FILENO, STDOUT_FILENO) >= 0 else {
      throw NSError(
        domain: "PlanInspector", code: 4,
        userInfo: [NSLocalizedDescriptionKey: "Cannot redirect native diagnostics"])
    }
    defer {
      fflush(nil)
      dup2(savedStdout, STDOUT_FILENO)
      close(savedStdout)
    }
    var records: [[String: Any]] = []
    var totalPreferred: [String: Int] = [:]
    var totalSupported: [String: Int] = [:]
    for url in urls.sorted(by: { $0.path < $1.path }) {
      do {
        let plan = try await MLComputePlan.load(contentsOf: url, configuration: configuration)
        var preferred: [String: Int] = [:]
        var supported: [String: Int] = [:]
        var operators: [String: Int] = [:]
        var unknown = 0
        func inspect(_ block: MLModelStructure.Program.Block) {
          for operation in block.operations {
            operators[operation.operatorName, default: 0] += 1
            if let usage = plan.deviceUsage(for: operation) {
              preferred[deviceName(usage.preferred), default: 0] += 1
              for device in usage.supported {
                supported[deviceName(device), default: 0] += 1
              }
            } else {
              unknown += 1
            }
            for nested in operation.blocks { inspect(nested) }
          }
        }
        switch plan.modelStructure {
        case .program(let program):
          for function in program.functions.values { inspect(function.block) }
        case .neuralNetwork(let network):
          for layer in network.layers {
            operators[layer.type, default: 0] += 1
            if let usage = plan.deviceUsage(for: layer) {
              preferred[deviceName(usage.preferred), default: 0] += 1
              for device in usage.supported { supported[deviceName(device), default: 0] += 1 }
            } else {
              unknown += 1
            }
          }
        default:
          records.append(["path": url.path, "error": "Unsupported model structure"])
          continue
        }
        for (device, count) in preferred { totalPreferred[device, default: 0] += count }
        for (device, count) in supported { totalSupported[device, default: 0] += count }
        records.append([
          "path": url.path, "preferred_operations": preferred,
          "supported_operations": supported, "operators": operators,
          "operations_without_device_usage": unknown,
        ])
      } catch {
        records.append(["path": url.path, "error": String(describing: error)])
      }
    }
    let output: [String: Any] = [
      "mode": mode, "model_count": urls.count,
      "allow_low_precision_accumulation_on_gpu": configuration.allowLowPrecisionAccumulationOnGPU,
      "preferred_operations": totalPreferred, "supported_operations": totalSupported,
      "models": records,
      "interpretation":
        "Predicted placement from MLComputePlan, not measured occupancy. Operation counts are not time-weighted and exclude nodes outside these Core ML partitions.",
    ]
    let data = try JSONSerialization.data(
      withJSONObject: output, options: [.prettyPrinted, .sortedKeys])
    fflush(nil)
    dup2(savedStdout, STDOUT_FILENO)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
  }
}
