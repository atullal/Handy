// swift-tools-version: 6.2
import Foundation
import PackageDescription

let source = ProcessInfo.processInfo.environment["HANDY_RESEARCH_FLUIDAUDIO_SOURCE"]
let dependency: Package.Dependency =
  source.map {
    .package(name: "FluidAudio", path: $0, traits: [])
  }
  ?? .package(
    url: "https://github.com/FluidInference/FluidAudio.git",
    revision: "0b1f46289fe27d95b5e66ad8be46e64f5ee02ae7",
    traits: []
  )

let package = Package(
  name: "HandyAppleAsrProbe",
  platforms: [.macOS(.v14)],
  dependencies: [dependency],
  targets: [
    .executableTarget(
      name: "HandyNativeAsrWorker",
      dependencies: [.product(name: "FluidAudio", package: "FluidAudio")]),
    .executableTarget(
      name: "HandyAppleAsrProbe",
      dependencies: [
        .product(name: "FluidAudio", package: "FluidAudio")
      ]),
  ]
)
