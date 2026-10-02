#!/usr/bin/env python3
"""Benchmark integrated Metal/native Core ML routes without saving preferences."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import statistics
import subprocess
import wave

MODEL = "handy-computer/parakeet-unified-en-0.6b-gguf/parakeet-unified-en-0.6b-Q8_0.gguf"


def read_text(*command):
    return subprocess.check_output(command, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--audio", type=Path, default=Path("src-tauri/resources/native-asr/warmup.wav"))
    parser.add_argument("--native-model-dir", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--metal-device", type=int, default=0, help="Confirm with --list-devices")
    parser.add_argument("--multipliers", type=int, nargs="+", default=[1, 3, 5])
    parser.add_argument("--trials", type=int, default=2)
    parser.add_argument("--warm-runs", type=int, default=5)
    parser.add_argument("--extra-route", action="append", default=[], metavar="NAME:MODE:BACKEND",
                        help="Future CLI provider, e.g. core_ai:core_ai:coreai_native; it must already be implemented")
    args = parser.parse_args()
    if args.trials < 1 or args.warm_runs < 1 or any(n < 1 for n in args.multipliers):
        parser.error("Trials, warm runs and multipliers must be positive")
    app = args.app.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with wave.open(str(args.audio), "rb") as source:
        if (source.getnchannels(), source.getsampwidth(), source.getframerate()) != (1, 2, 16000):
            parser.error("Audio must be 16 kHz, mono, 16-bit WAV")
        params = source.getparams()
        frames = source.readframes(source.getnframes())
        source_seconds = source.getnframes() / 16000
    routes = [
        ("metal", "off", False),
        ("coreml_gpu", "gpu", False),
        ("coreml_neural", "neural", False),
        ("coreml_gpu_prewarmed", "gpu", True),
        ("coreml_neural_prewarmed", "neural", True),
    ]
    expected_backends = {"gpu": "coreml_native_gpu", "neural": "coreml_native_neural"}
    for route in args.extra_route:
        parts = route.split(":")
        if len(parts) != 3 or any(not re.fullmatch(r"[a-zA-Z0-9_-]+", part) for part in parts):
            parser.error("Extra route must be NAME:MODE:BACKEND using letters, numbers, underscores or hyphens")
        name, mode, backend = parts
        if mode in expected_backends or mode == "off" or any(name == row[0] for row in routes):
            parser.error("Extra route name and mode must be unique")
        expected_backends[mode] = backend
        routes.extend([(name, mode, False), (name + "_prewarmed", mode, True)])
    results = {"chip": read_text("sysctl", "-n", "machdep.cpu.brand_string"),
               "memory_bytes": int(read_text("sysctl", "-n", "hw.memsize")),
               "macos": read_text("sw_vers", "-productVersion"), "machine": platform.machine(),
               "power": read_text("pmset", "-g", "batt"),
               "audio_sha256": hashlib.sha256(args.audio.read_bytes()).hexdigest(),
               "pcm_sha256": hashlib.sha256(frames).hexdigest(),
               "app_sha256": hashlib.sha256(app.read_bytes()).hexdigest(),
               "method": "New process per trial; one first inference plus warm repeats. Optional worker prewarm is included in load_ms and separately reported. Total app path includes PCM transport/text processing, excludes recording, VAD and paste. Longer inputs repeat the source, not a diverse quality corpus.",
               "runs": []}
    for multiplier in args.multipliers:
        wav = output / f"audio-{multiplier}x.wav"
        with wave.open(str(wav), "wb") as target:
            target.setparams(params)
            target.writeframes(frames * multiplier)
        for trial in range(args.trials):
            # Reverse second-trial order to reduce consistent ordering bias.
            for name, mode, prewarm in routes[::1 if trial % 2 == 0 else -1]:
                command = [str(app), "--transcribe-file", str(wav), "--model", args.model,
                           "--apple-asr", mode, "--repeat", str(args.warm_runs + 1), "--json"]
                if mode == "off":
                    command += ["--device-index", str(args.metal_device)]
                elif args.native_model_dir:
                    command += ["--native-model-dir", str(args.native_model_dir.resolve())]
                if prewarm:
                    command += ["--apple-prewarm"]
                label = f"{name}-{multiplier}x-trial{trial + 1}"
                with (output / f"{label}.stderr.txt").open("w") as stderr:
                    run = subprocess.run(command, stdout=subprocess.PIPE, stderr=stderr, text=True)
                (output / f"{label}.json").write_text(run.stdout)
                if run.returncode:
                    raise SystemExit(f"Failed {label}; inspect its stderr. No failed route is counted as a speedup.")
                data = json.loads(run.stdout)
                expected_backend = expected_backends.get(mode)
                bound = data["bound_backend"]
                if (mode != "off" and bound != expected_backend) or (mode == "off" and "MTL" not in bound):
                    raise SystemExit(f"Unexpected backend for {label}: {bound}")
                if any(value != bound for value in data["backend_by_run"]):
                    raise SystemExit(f"Backend changed during {label}")
                results["runs"].append({"route": name, "audio_s": source_seconds * multiplier,
                                       "trial": trial + 1, "result": data})
                (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
                print(f"{label}: first={data['transcribe_ms'][0]}ms warm={statistics.median(data['transcribe_ms'][1:])}ms", flush=True)
    summaries = []
    normalize = lambda text: re.findall(r"[a-z0-9]+", text.lower())
    for duration in sorted({r["audio_s"] for r in results["runs"]}):
        reference = next(r for r in results["runs"] if r["route"] == "metal" and r["audio_s"] == duration)["result"]["text"]
        for name, _, _ in routes:
            runs = [r["result"] for r in results["runs"] if r["route"] == name and r["audio_s"] == duration]
            warm = [t for run in runs for t in run["transcribe_ms"][1:]]
            summaries.append({"route": name, "audio_s": duration,
                              "warm_median_ms": statistics.median(warm), "warm_min_ms": min(warm), "warm_max_ms": max(warm),
                              "first_ms": [run["transcribe_ms"][0] for run in runs],
                              "load_ms": [run["load_ms"] for run in runs],
                              "native_load": [run.get("native_load") for run in runs],
                              "words_match_metal": all(normalize(text) == normalize(reference) for run in runs for text in run["texts"])})
    (output / "summary.json").write_text(json.dumps({"hardware": {k: v for k, v in results.items() if k != "runs"}, "summaries": summaries}, indent=2) + "\n")


if __name__ == "__main__":
    main()
