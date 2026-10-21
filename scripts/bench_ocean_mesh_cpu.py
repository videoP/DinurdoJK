#!/usr/bin/env python3
"""Measure native ocean CPU generation/upload with the standard runnable exe.

No binary copies or separate build caches. Restores the user's cfg byte-for-byte.
Each case captures a stationary view and scripted camera translations over Costa.
"""
import argparse
import csv
import datetime
import gzip
import json
import math
import shutil
import statistics
import subprocess
import time
from pathlib import Path

import bench_shader_modes as bench


def percentile(values, fraction):
    values = sorted(values)
    return values[min(len(values) - 1, int((len(values) - 1) * fraction))] if values else None


def summarize(path, label):
    with path.open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    if not rows or "ocean_native_mesh_build_ms" not in rows[0]:
        raise RuntimeError(f"{path}: missing instrumented mesh timings")
    def values(name, subset=rows):
        return [float(row[name]) for row in subset
                if row.get(name) not in (None, "") and math.isfinite(float(row[name]))]
    rebuilt = [row for row in rows if float(row["ocean_native_mesh_rebuilds"]) > 0]
    result = {"label": label, "frames": len(rows), "rebuild_frames": len(rebuilt)}
    for name in ["ocean_native_prepare_ms", "ocean_native_mesh_build_ms", "ocean_native_mesh_upload_ms",
                 "frame_interval_ms", "cpu_encode_ms", "gpu_latest_available_ms"]:
        data = values(name)
        result[name] = {"mean": statistics.fmean(data) if data else None,
                        "p50": percentile(data, .5), "p95": percentile(data, .95),
                        "p99": percentile(data, .99), "max": max(data) if data else None}
        if rebuilt and name.startswith("ocean_native_"):
            data = values(name, rebuilt)
            result[name + "_when_rebuilt"] = {"mean": statistics.fmean(data),
                "p50": percentile(data, .5), "p95": percentile(data, .95),
                "p99": percentile(data, .99), "max": max(data)}
    for name in ["ocean_original_surfaces", "ocean_adaptive_mesh_mode", "blue_water_mesh_segments",
                 "blue_water_mesh_levels", "ocean_triangles"]:
        data = values(name)
        result[name] = {"min": min(data), "max": max(data), "mean": statistics.fmean(data)}
    duration = sum(values("frame_interval_ms")) / 1000
    result["duration_s"] = duration
    result["rebuilds_per_s"] = sum(values("ocean_native_mesh_rebuilds")) / duration
    result["upload_MiB_per_s"] = sum(values("ocean_native_mesh_upload_bytes")) / duration / 2**20
    result["generated_vertices_per_rebuild_frame"] = (sum(values("ocean_native_mesh_vertices")) / len(rebuilt)) if rebuilt else 0
    result["generated_indices_per_rebuild_frame"] = (sum(values("ocean_native_mesh_indices")) / len(rebuilt)) if rebuilt else 0
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--trials", type=int, default=2)
    parser.add_argument("--seconds", type=int, default=8)
    parser.add_argument("--cases", default="off64,rings64,local64,local256")
    parser.add_argument("--wave-height", type=float, default=1.0)
    parser.add_argument("--timeout", type=int, default=1200)
    args = parser.parse_args()
    definitions = {"off64": (0, 64, 6), "rings64": (1, 64, 6),
                   "local64": (2, 64, 6), "local256": (2, 256, 9)}
    cases = args.cases.split(",")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = bench.RESULTS / f"ocean-mesh-cpu-{stamp}"
    out.mkdir(parents=True)
    original = bench.USER_CFG.read_bytes()
    # Text backup only, also available if the Python process is interrupted.
    (out / "user-config-before.cfg").write_bytes(original)
    original_lines = original.decode("utf-8", errors="replace").splitlines()
    overrides = {**bench.COMMON, "r_blueWaterHeight": str(args.wave_height), "r_oceanType": "3",
                 "r_fullscreen": "1", "r_oceanAdaptiveMeshDebug": "0",
                 "r_fragmentShaderMode": "1", "cl_commandRate": "100"}
    (out / "settings.json").write_text(json.dumps({"overrides": overrides,
        "cases": definitions, "args": vars(args), "note": "Other settings retained from user cfg, including local tolerance."}, indent=2))
    results = []
    try:
        for trial in range(args.trials):
            order = cases if trial % 2 == 0 else cases[::-1]
            labels = []
            lines = [f"set {name} {value}" for name, value in overrides.items()]
            lines += ["waitshadermode 1 180"]
            for case in order:
                mode, segments, levels = definitions[case]
                lines += [f"set r_oceanAdaptiveMesh {mode}", f"set r_blueWaterMeshSegments {segments}",
                          f"set r_blueWaterMeshLevels {levels}",
                          "setviewpos_exact 1936 831 826 15 -339", "perfsample 12 meshsettle", "viewpos"]
                label = f"mesh_{case}_t{trial}_static"
                labels.append(label)
                lines += [f"render_perf_capture {args.seconds}s {label}",
                          f"perfsample {args.seconds + 8} {label}"]
                label = f"mesh_{case}_t{trial}_move"
                labels.append(label)
                lines += ["setviewpos_exact 1936 831 826 15 -339",
                          f"render_perf_capture {args.seconds}s {label}"]
                # Explicit camera translations also work without input focus.
                # 40 units per 0.1 s = 10 metres/s. Each 20-metre leg crosses
                # the 16-metre pruning quantum.
                for step in range(math.ceil((args.seconds + 10) / .1)):
                    phase = step % 40
                    offset = 40 * (phase if phase <= 20 else 40 - phase)
                    lines += [f"setviewpos_exact {1936 + offset} 831 826 15 -339",
                              "wait 10"]
                    if step % 20 == 0:
                        lines += ["viewpos"]
                lines += ["setviewpos_exact 1936 831 826 15 -339"]
            lines += ["quit"]
            cfg_name = "bench_ocean_mesh_cpu.cfg"
            cfg_path = bench.BASE / cfg_name
            script_text = "\n".join(lines) + "\n"
            cfg_path.write_text(script_text)
            (out / f"commands-t{trial}.cfg").write_text(script_text)
            bench.write_bench_cfg(original_lines, overrides)
            print(f"[mesh bench] trial {trial}: {order}; output {out}", flush=True)
            started = time.time()
            try:
                proc = subprocess.run([str(bench.REL / "DinurdoJK.exe"), "--base", str(bench.BASE),
                                       "--map", "costadelsol", "+exec", cfg_name],
                                      cwd=bench.REL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                      timeout=args.timeout)
            finally:
                bench.USER_CFG.write_bytes(original)
                cfg_path.unlink(missing_ok=True)
            log = (bench.REL / "latest.log").read_text(encoding="utf-8", errors="replace")
            (out / f"trial-{trial}.log").write_text(log, encoding="utf-8")
            print(f"[mesh bench] exit {proc.returncode}, {time.time() - started:.1f}s", flush=True)
            wrote = {Path(m.group(2)).stem: m for m in bench.WROTE_RE.finditer(log)}
            for label in labels:
                match = next((m for stem, m in wrote.items() if stem.endswith("-" + label)), None)
                if match is None:
                    raise RuntimeError(f"missing capture {label}; see {out / f'trial-{trial}.log'}")
                path = Path(match.group(2))
                result = summarize(path, label)
                meta_path = Path(match.group(3))
                meta = json.loads(meta_path.read_text())
                result["adapter"] = meta.get("adapter")
                result["fingerprint"] = meta.get("settings_fingerprint_at_arm")
                result["backend"] = meta.get("backend")
                results.append(result)
                (out / "summary.json").write_text(json.dumps(results, indent=2))
                with path.open("rb") as source, gzip.open(out / f"{label}.csv.gz", "wb") as target:
                    # Compress only this run's CSV; no build artifacts are touched.
                    shutil.copyfileobj(source, target)
                path.unlink()
                meta_path.replace(out / f"{label}.json")
                build = result["ocean_native_mesh_build_ms"]["mean"]
                conditional = result.get("ocean_native_mesh_build_ms_when_rebuilt", {}).get("mean", 0)
                print(f"{label}: build {build:.4f} ms/frame, {conditional:.4f} ms/rebuild frame; "
                      f"{result['rebuilds_per_s']:.1f} rebuilds/s", flush=True)
            (out / "summary.json").write_text(json.dumps(results, indent=2))
    finally:
        bench.USER_CFG.write_bytes(original)
    print(f"[mesh bench] results: {out}", flush=True)


if __name__ == "__main__":
    main()
