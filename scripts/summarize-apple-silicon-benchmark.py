#!/usr/bin/env python3
"""Summarize a Handy benchmark and optionally plot it with matplotlib."""

import argparse
import csv
import json
from pathlib import Path
import statistics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--plot", action="store_true")
    args = parser.parse_args()
    data = json.loads(args.results.read_text())
    rows = []
    groups = {}
    for run in data["runs"]:
        groups.setdefault((run["case"], run["multiplier"], run["phase"]), []).append(run)
    for (case, multiplier, phase), runs in sorted(groups.items()):
        warm = [ms for run in runs for ms in run["warm_ms"]]
        row = {
            "case": case, "audio_s": runs[0]["result"]["audio_secs"],
            "phase": phase, "trials": len(runs), "warm_samples": len(warm),
            "warm_median_ms": statistics.median(warm),
            "warm_min_ms": min(warm), "warm_max_ms": max(warm),
            "first_median_ms": statistics.median(run["first_ms"] for run in runs),
            "load_median_ms": statistics.median(run["result"]["load_ms"] for run in runs),
            "peak_main_process_gib": max(run["peak_main_process_rss_bytes"] for run in runs) / 2**30,
            "max_word_errors": max(run["word_errors"] for run in runs),
            "max_reference_words": max(run["reference_words"] for run in runs),
        }
        rows.append(row)
    output = args.results.parent
    summary = {"metadata": data["metadata"], "summaries": rows,
               "failures": data.get("failures", [])}
    (output / "summary.json").write_text(json.dumps(summary, indent=2))
    with (output / "summary.csv").open("w") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    for row in rows:
        print(f"{row['case']:27} {row['audio_s']:5.1f}s {row['phase']:11} "
              f"warm {row['warm_median_ms']:7.1f}ms load {row['load_median_ms']:7.1f}ms "
              f"RSS {row['peak_main_process_gib']:.2f}GiB words {row['max_word_errors']}")
    if not args.plot:
        return
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    plt.rcParams.update({"font.family": "sans-serif", "font.size": 11})
    fig, axes = plt.subplots(1, 2, figsize=(12, 5.5), layout="constrained")
    panels = [
        ("Moonshine Base · ONNX", [
            ("moonshine_cpu", "CPU", "#2563eb"),
            ("moonshine_coreml_all", "Core ML · all hardware", "#db7b16"),
            ("moonshine_coreml_neural", "Core ML · Neural Engine + CPU", "#9b51b6"),
        ]),
        ("Parakeet Unified Q8 · GGUF", [
            ("parakeet_cpu", "CPU", "#2563eb"),
            ("parakeet_metal", "Metal · modified build", "#13886b"),
            ("parakeet_metal_original", "Metal · original build", "#64748b"),
        ]),
    ]
    for ax, (title, cases) in zip(axes, panels):
        for case, label, color in cases:
            points = sorted((r for r in rows if r["case"] == case and r["phase"] == "cached"
                             and r["audio_s"] <= 64),
                            key=lambda r: r["audio_s"])
            if not points:
                continue
            ax.plot([p["audio_s"] for p in points], [p["warm_median_ms"] for p in points],
                    marker="o", linewidth=2, color=color, label=label,
                    linestyle="--" if case.endswith("_original") else "-")
            for p in points:
                offset = (0, -16) if case.endswith("_original") else (0, 9)
                if p["audio_s"] < 12:
                    if case == "moonshine_coreml_all":
                        offset = (0, 24)
                    elif case == "moonshine_cpu":
                        offset = (0, -16)
                    elif case.endswith("_original"):
                        offset = (24, -3)
                ax.annotate(f"{p['warm_median_ms']:.0f}", (p["audio_s"], p["warm_median_ms"]),
                            xytext=offset,
                            textcoords="offset points", ha="center", fontsize=9, color=color)
        ax.set_title(title, fontweight="bold", pad=14)
        ax.set_xlabel("Audio duration (seconds)")
        ax.set_ylabel("Warm transcription latency (ms) · lower is faster")
        ax.set_xticks([11, 33, 55])
        ax.set_ylim(bottom=0)
        ax.grid(axis="y", alpha=.2)
        ax.spines[["top", "right"]].set_visible(False)
        ax.legend(loc="upper left", fontsize=9, frameon=False)
    fig.suptitle("Handy on Apple M5 Pro · plugged into power", fontsize=17, fontweight="bold")
    fig.supxlabel("Median of 10 warm runs per point · 2 process trials · repeated JFK speech sample\n"
                  "Moonshine overgenerates at 55s on every backend. File inference excludes microphone, VAD and paste latency.", fontsize=10)
    fig.savefig(output / "comparison.png", dpi=180)


if __name__ == "__main__":
    main()
