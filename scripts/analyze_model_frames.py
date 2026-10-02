"""Measure draw-anchor discontinuities in a per-present model frame capture."""
import argparse
import csv
import json
import math
import re
from collections import Counter
from pathlib import Path


def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def norm(v):
    return math.sqrt(sum(x * x for x in v))


def point(row, prefix, axes):
    return tuple(float(row[prefix + axis]) for axis in axes)


def animation_names():
    path = Path(__file__).resolve().parents[1] / "crates/jka-movement/vendor/openjk/codemp/game/anims.h"
    text = re.sub(r"/\*.*?\*/", "", path.read_text(), flags=re.S)
    text = re.sub(r"//[^\n]*", "", text)
    return re.findall(r"^\s*([A-Z][A-Z0-9_]+)\s*,", text, re.M)


def analyze(path):
    with path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        rows = list(reader)
    grouped = {}
    for row in rows:
        if None in row or any(value is None for value in row.values()):
            raise ValueError("CSV row does not match header")
        if row["saber"] == "0" and row["blade"] == "0":
            grouped.setdefault(int(row["frame_id"]), {})[row["anchor"]] = row
    frames = list(grouped.items())
    names = animation_names()
    name = lambda value: names[int(value)] if 0 <= int(value) < len(names) else value
    metrics = []
    transitions = []
    mismatches = []
    for i, (frame_id, anchors) in enumerate(frames):
        head = anchors["head"]
        time = float(head["present_ms"])
        previous = frames[i - 1][1] if i else None
        if previous and (head["legs_anim"], head["torso_anim"]) != (previous["head"]["legs_anim"], previous["head"]["torso_anim"]):
            transitions.append({"frame": frame_id, "time_ms": time, "legs": name(head["legs_anim"]), "torso": name(head["torso_anim"])})
        if head["legs_anim"] != head["torso_anim"]:
            if mismatches and mismatches[-1]["last_frame"] == frame_id - 1:
                mismatches[-1].update(last_frame=frame_id, last_time_ms=time)
            else:
                mismatches.append({"first_frame": frame_id, "last_frame": frame_id, "first_time_ms": time, "last_time_ms": time,
                                   "legs": name(head["legs_anim"]), "torso": name(head["torso_anim"])})
        if not previous:
            continue
        metric = {"frame": frame_id, "time_ms": time, "dt_ms": float(head["dt_ms"]),
                  "legs": name(head["legs_anim"]), "torso": name(head["torso_anim"]),
                  "previous_legs": name(previous["head"]["legs_anim"]),
                  "pose_dt_ms": int(head["pose_time"]) - int(previous["head"]["pose_time"]),
                  "camera_step_units": norm(sub(point(head, "camera_", "xyz"), point(previous["head"], "camera_", "xyz")))}
        for anchor in ("head", "hilt", "blade"):
            current, last = anchors[anchor], previous[anchor]
            if current["available"] != "1" or last["available"] != "1":
                continue
            for space, prefix, axes in (("world", "world_", "xyz"), ("screen", "stable_screen_", "xy")):
                delta = sub(point(current, prefix, axes), point(last, prefix, axes))
                metric[f"{anchor}_{space}_step"] = norm(delta)
                if i > 1:
                    older = frames[i - 2][1][anchor]
                    if older["available"] == "1" and float(last["dt_ms"]) > 0:
                        ratio = float(current["dt_ms"]) / float(last["dt_ms"])
                        previous_delta = sub(point(last, prefix, axes), point(older, prefix, axes))
                        metric[f"{anchor}_{space}_residual"] = norm(sub(delta, tuple(v * ratio for v in previous_delta)))
        if all(g[a]["available"] == "1" for g in (anchors, previous) for a in ("head", "hilt", "blade")):
            relative = sub(point(anchors["hilt"], "world_", "xyz"), point(head, "world_", "xyz"))
            old_relative = sub(point(previous["hilt"], "world_", "xyz"), point(previous["head"], "world_", "xyz"))
            metric["hilt_relative_to_head_step_units"] = norm(sub(relative, old_relative))
            metric["hilt_head_distance_units"] = norm(relative)
            metric["hilt_blade_distance_units"] = norm(sub(point(anchors["blade"], "world_", "xyz"), point(anchors["hilt"], "world_", "xyz")))
        metrics.append(metric)
    heads = [group["head"] for _, group in frames]
    largest = sorted((m for m in metrics if "hilt_screen_residual" in m), key=lambda m: m["hilt_screen_residual"], reverse=True)
    summary = {
        "capture": str(path), "frames": len(frames), "rows": len(rows),
        "duration_ms": float(heads[-1]["present_ms"]),
        "frame_id_gaps": sum(b[0] != a[0] + 1 for a, b in zip(frames, frames[1:])),
        "max_present_interval_ms": max(float(h["dt_ms"]) for h in heads),
        "reused_sources": sum(h["source_repeated"] == "1" for h in heads),
        "raster_surface_counts": dict(Counter(h["raster_surfaces"] for h in heads)),
        "availability": {a: dict(Counter(g[a]["available"] for _, g in frames)) for a in ("head", "hilt", "blade")},
        "animation_transitions": transitions, "legs_torso_mismatch_intervals": mismatches,
        "largest_hilt_screen_discontinuities": largest[:20],
        "measurement": "Unjittered projected anchors at present calls. Residual extrapolates the previous step using actual dt; it is a ranking metric, not proof of a particular cause. Relative world motion removes translation but includes body rotation and articulation. No monitor scanout or postprocess pixel readback.",
    }
    return frames, metrics, summary


def plots(frames, metrics, output):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    ranked = sorted((m for m in metrics if "hilt_screen_residual" in m), key=lambda m: m["hilt_screen_residual"], reverse=True)
    event = ranked[0]["time_ms"]
    landings = [m["time_ms"] for m in metrics if m["legs"] == "BOTH_LAND1" and m["previous_legs"] != "BOTH_LAND1"]
    comparison = landings[-1] if landings else event
    fig, axes = plt.subplots(3, 2, figsize=(12, 8), constrained_layout=True)
    colors = {"head": "#2074b4", "hilt": "#df7519", "blade": "#39854f"}
    for column, (center, title) in enumerate(((event, "Largest discontinuity"), (comparison, "Final landing"))):
        nearby = [(fid, g) for fid, g in frames if center - 30 <= float(g["head"]["present_ms"]) <= center + 60]
        for anchor, color in colors.items():
            valid = [(fid, g) for fid, g in nearby if g[anchor]["available"] == "1"]
            times = [float(g["head"]["present_ms"]) - center for _, g in valid]
            for row, axis in enumerate("xy"):
                values = [float(g[anchor]["stable_screen_" + axis]) for _, g in valid]
                if values:
                    axes[row, column].plot(times, [v - values[0] for v in values], ".-", markersize=3, color=color, label=anchor)
        local_metrics = [m for m in metrics if center - 30 <= m["time_ms"] <= center + 60]
        axes[2, column].plot([m["time_ms"] - center for m in local_metrics], [m.get("hilt_relative_to_head_step_units", float("nan")) for m in local_metrics], ".-", color="#9a3d90")
        for ax in axes[:, column]:
            ax.axvline(0, color="#555", linestyle="--", linewidth=1)
            ax.grid(alpha=0.2)
        axes[0, column].set_title(f"{title}: {center / 1000:.3f} s")
        axes[0, column].legend(loc="best")
        axes[2, column].set_xlabel("Milliseconds from landing transition")
    axes[0, 0].set_ylabel("Screen X change (pixels)")
    axes[1, 0].set_ylabel("Screen Y change (pixels)")
    axes[2, 0].set_ylabel("Hilt/head relative step (world units)")
    fig.suptitle("Per-present model anchors: landing comparison\nFinal camera projection, TAA jitter removed; dots are rendered frames")
    fig.savefig(output / "landing-comparison.png", dpi=160)
    fig.savefig(output / "landing-comparison.svg")
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    frames, metrics, summary = analyze(args.capture)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    fields = list(dict.fromkeys(key for row in metrics for key in row))
    with (args.output / "frame-metrics.csv").open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader(); writer.writerows(metrics)
    plots(frames, metrics, args.output)
    print(json.dumps({key: summary[key] for key in ("frames", "duration_ms", "max_present_interval_ms", "frame_id_gaps", "reused_sources", "raster_surface_counts")}, indent=2))
    print(json.dumps(summary["largest_hilt_screen_discontinuities"][:1], indent=2))


if __name__ == "__main__":
    main()
