#!/usr/bin/env python3
"""Compare Handy backends without changing the selected model or accelerator."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time
import wave


def run_text(*args):
    return subprocess.check_output(args, text=True).strip()


def preferences():
    path = Path.home() / "Library/Application Support/com.pais.handy/settings_store.json"
    store = json.loads(path.read_text())
    settings = store.get("settings", store)
    return {key: settings.get(key) for key in (
        "selected_model", "ort_accelerator", "transcribe_accelerator", "transcribe_gpu_device"
    )}


def words(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def word_errors(expected, actual):
    previous = list(range(len(actual) + 1))
    for i, left in enumerate(expected, 1):
        current = [i]
        for j, right in enumerate(actual, 1):
            current.append(min(current[-1] + 1, previous[j] + 1,
                               previous[j - 1] + (left != right)))
        previous = current
    return previous[-1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--app", type=Path,
                        default=Path("/Applications/Handy.app/Contents/MacOS/handy"))
    parser.add_argument("--old-app", type=Path,
                        help="Optional original binary for a paired Metal comparison")
    parser.add_argument("--multipliers", type=int, nargs="+", default=[1, 3, 5],
                        help="Repeat source audio; default stays below Moonshine's 64s limit")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--skip-fresh-cache", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    previous = None
    if output.exists() and args.resume:
        previous = json.loads((output / "results.json").read_text())
    elif output.exists():
        raise SystemExit(f"Use a new output directory: {output}")
    output.mkdir(parents=True, exist_ok=bool(previous))
    raw = output / "raw"
    raw.mkdir(exist_ok=bool(previous))
    with wave.open(str(args.audio), "rb") as source:
        spec = source.getparams()
        assert (spec.nchannels, spec.sampwidth, spec.framerate) == (1, 2, 16000)
        frames = source.readframes(spec.nframes)
    clips = {}
    for multiplier in args.multipliers:
        path = output / f"jfk-{multiplier}x.wav"
        with wave.open(str(path), "wb") as target:
            target.setparams(spec)
            target.writeframes(frames * multiplier)
        clips[multiplier] = path

    before = preferences()
    devices = run_text(str(args.app), "--list-devices")
    gpu = re.search(r"index=(\d+) kind=metal", devices)
    cpu = re.search(r"index=(\d+) kind=cpu", devices)
    if not gpu or not cpu:
        raise SystemExit("Both Metal and CPU devices are required")
    selected = before["selected_model"]
    if not selected or not selected.endswith(".gguf"):
        raise SystemExit("Select an installed GGUF model before this benchmark")
    cases = [
        ("moonshine_cpu", "moonshine-base", ["--ort-accelerator", "cpu"]),
        ("moonshine_coreml_all", "moonshine-base", ["--ort-accelerator", "coreml"]),
        ("moonshine_coreml_neural", "moonshine-base", ["--ort-accelerator", "coreml_neural_engine"]),
        ("parakeet_cpu", selected, ["--device-index", cpu.group(1)]),
        ("parakeet_metal", selected, ["--device-index", gpu.group(1)]),
    ]
    if args.old_app:
        cases.append(("parakeet_metal_original", selected, ["--device-index", gpu.group(1)]))
    metadata = {
        "chip": run_text("sysctl", "-n", "machdep.cpu.brand_string"),
        "memory_bytes": int(run_text("sysctl", "-n", "hw.memsize")),
        "macos": run_text("sw_vers", "-productVersion"),
        "battery_start": run_text("pmset", "-g", "batt"),
        "thermal_start": run_text("pmset", "-g", "therm"),
        "binary_sha256": hashlib.sha256(args.app.read_bytes()).hexdigest(),
        "original_binary_sha256": hashlib.sha256(args.old_app.read_bytes()).hexdigest() if args.old_app else None,
        "source_audio_sha256": hashlib.sha256(args.audio.read_bytes()).hexdigest(),
        "source_audio_url": "https://github.com/ggml-org/whisper.cpp/blob/master/samples/jfk.wav",
        "preferences_before": before,
        "devices": devices,
        "method": "2 process trials per case; 1 first inference + 5 warm repeats per trial. Sequential, rotated case order. Repeated 11s JFK clip. Existing Core ML cache retained for standard trials.",
        "limitations": [
            "Longer clips repeat the same speech; this is a latency smoke benchmark, not a diverse accuracy dataset.",
            "Memory is maximum resident size of the main process, excluding WebKit child processes.",
            "File transcription excludes microphone, VAD, clipboard/paste and UI latency.",
            "Core ML compute-unit selection does not prove every operation runs on the Neural Engine.",
            "No energy or hardware-occupancy measurement; ordinary background desktop activity remains.",
        ],
    }
    if previous:
        if previous["metadata"]["binary_sha256"] != metadata["binary_sha256"]:
            raise SystemExit("Cannot resume with a different binary")
        metadata = previous["metadata"]
        metadata.setdefault("resumes", []).append({
            "battery": run_text("pmset", "-g", "batt"), "multipliers": args.multipliers})
    results = previous["runs"] if previous else []
    failures = previous.get("failures", []) if previous else []
    reference = words("And so my fellow Americans ask not what your country can do for you ask what you can do for your country")

    def save():
        (output / "results.json").write_text(json.dumps({
            "metadata": metadata, "runs": results, "failures": failures}, indent=2))

    def execute(case, multiplier, trial, phase="cached", attempt=1):
        name, model, options = case
        stem = f"{phase}-{name}-{multiplier}x-trial{trial}-attempt{attempt}"
        binary = args.old_app if name.endswith("_original") else args.app
        command = ["/usr/bin/time", "-l", str(binary), "--transcribe-file",
                   str(clips[multiplier]), "--model", model, *options,
                   "--repeat", "6", "--json"]
        battery = run_text("pmset", "-g", "batt")
        start = time.monotonic()
        environment = os.environ.copy()
        environment.pop("HANDY_COREML_PROFILE", None)
        environment.pop("GGML_METAL_TENSOR_DISABLE", None)
        environment.pop("GGML_METAL_TENSOR_ENABLE", None)
        try:
            completed = subprocess.run(command, capture_output=True, text=True,
                                       env=environment, timeout=240)
        except subprocess.TimeoutExpired as error:
            completed = subprocess.CompletedProcess(command, -1,
                (error.stdout or b"").decode() if isinstance(error.stdout, bytes) else error.stdout or "",
                (error.stderr or b"").decode() if isinstance(error.stderr, bytes) else error.stderr or "")
        elapsed = time.monotonic() - start
        (raw / f"{stem}.json").write_text(completed.stdout)
        (raw / f"{stem}.stderr.txt").write_text(completed.stderr)
        try:
            if completed.returncode:
                raise ValueError(f"Exit code {completed.returncode}")
            result = json.loads(completed.stdout)
            timings = result["transcribe_ms"]
            if len(timings) != 6:
                raise ValueError("Expected six timings")
        except (ValueError, KeyError) as error:
            diagnostic = next((line for line in completed.stderr.splitlines()
                               if line.startswith("error:")), "")
            failures.append({"case": name, "phase": phase, "trial": trial,
                "multiplier": multiplier, "attempt": attempt, "error": str(error),
                "diagnostic": diagnostic[:1000],
                "returncode": completed.returncode, "wall_s": elapsed,
                "battery_start": battery, "raw_stem": stem})
            save()
            print(f"{stem}: FAILED: {error}", flush=True)
            if attempt == 1 and "Audio duration must be between" not in diagnostic:
                execute(case, multiplier, trial, phase, 2)
            return
        rss = re.search(r"(\d+)\s+maximum resident set size", completed.stderr)
        expected = reference * multiplier
        errors = word_errors(expected, words(result["text"]))
        entry = {
            "case": name, "phase": phase, "trial": trial,
            "attempt": attempt, "battery_start": battery,
            "multiplier": multiplier, "result": result,
            "first_ms": timings[0], "warm_ms": timings[1:],
            "wall_s": elapsed, "load_average": os.getloadavg(),
            "peak_main_process_rss_bytes": int(rss.group(1)) if rss else None,
            "word_errors": errors, "reference_words": len(expected),
            "unbounded_shape_warnings": completed.stderr.count("unbounded dimension"),
        }
        results.append(entry)
        save()
        print(f"{stem}: load={result['load_ms']}ms first={timings[0]}ms "
              f"warm-median={statistics.median(timings[1:])}ms word-errors={errors}", flush=True)

    for multiplier in args.multipliers:
        for trial in (1, 2):
            rotation = (trial + multiplier) % len(cases)
            order = cases[rotation:] + cases[:rotation]
            for case in order:
                if any(r["case"] == case[0] and r["multiplier"] == multiplier
                       and r["trial"] == trial and r["phase"] == "cached" for r in results):
                    continue
                execute(case, multiplier, trial)

    # Isolate fresh compilation without discarding the user's existing cache.
    # The selected GUI model is GGUF, so it does not consume this ONNX cache.
    cache = Path.home() / "Library/Caches/com.pais.handy/coreml"
    saved = output / "original-coreml-cache"
    if not args.skip_fresh_cache and preferences() == before:
        had_cache = cache.exists()
        if had_cache:
            cache.rename(saved)
        try:
            for case in cases[1:3]:
                execute(case, 1, 1, "fresh_cache")
        finally:
            if cache.exists():
                cache.rename(output / "fresh-coreml-cache-artifacts")
            if had_cache:
                saved.rename(cache)
    metadata["preferences_after"] = preferences()
    metadata["preferences_unchanged"] = metadata["preferences_after"] == before
    metadata["battery_end"] = run_text("pmset", "-g", "batt")
    metadata["thermal_end"] = run_text("pmset", "-g", "therm")
    metadata["complete"] = True
    save()
    print(f"Complete: {output / 'results.json'}", flush=True)


if __name__ == "__main__":
    main()
