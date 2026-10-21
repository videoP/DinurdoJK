#!/usr/bin/env python3
"""Unattended r_fragmentShaderMode benchmark (10-second captures, counterbalanced).

    python scripts\\bench_shader_modes.py --scenario isolation --spots costa,ffa3
    python scripts\\bench_shader_modes.py --scenario realistic --modes 1,4 --trials 3
    python scripts\\bench_shader_modes.py --dry-run ...

One launch per (spot, trial). Inside a launch every requested mode is measured
twice in mirrored order (A B C C B A), and odd trials start from the other end,
so drift and warm-up hit every mode equally. For each measurement:

    set r_fragmentShaderMode M
    waitshadermode M        holds until requested == active and nothing compiles
    perfsample <settle>     warm the scene on the new programs
    render_perf_capture Ns  per-frame CSV + JSON (2 s arm delay + 120 frames, then N s)
    perfsample N+4          holds the buffer while the capture runs (cross-check FPS)

The user's target/release/base/DinurdoJK.cfg is backed up and restored after
every launch. Scenario overrides are written into the cfg before launch, so
restart-latched cvars apply. The exe and DLLs are copied to target/bench-work/
so latest.log never collides with a running game.

Outputs go to bench/results/shader-modes-<tag>-<timestamp>/:
    summary.json / summary.csv   one row per 10 s capture
    report.txt                   per-mode aggregates and paired deltas vs the baseline mode
    frames/<label>.csv.gz        reduced per-frame columns (the full CSV is dropped: ~20 MB each)
    logs/<launch>.log            latest.log of each launch
"""
import argparse
import csv
import datetime
import gzip
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
REL = REPO / "target" / "release"
BASE = REL / "base"
USER_CFG = BASE / "DinurdoJK.cfg"
WORK = REPO / "target" / "bench-work" / "shader-modes"
RESULTS = REPO / "bench" / "results"

# Rendered-camera spots as `viewpos` prints them: x y z yaw pitch.
SPOTS = {
    "costa": ("costadelsol", (1936, 831, 863, 15, -339)),
    "ffa3": ("mp/ffa3", (693, -1450, -71, 89, -1)),
}
# setviewpos_exact takes the feet origin (eye = +37) and, unlike setviewpos,
# applies no 400 u/s teleport push, so the camera rests on the requested view.
EYE_HEIGHT = 37


def demo_spot(spot):
    """`demo:<name>@<seconds>`: a paused demo frame (players, models and effects)."""
    if not spot.startswith("demo:"):
        return None
    name, _, seconds = spot[5:].partition("@")
    return name, float(seconds or 0)


def spot_id(spot):
    """Label-safe spot name (render_perf_capture keeps [A-Za-z0-9_-], 32 chars)."""
    demo = demo_spot(spot)
    return f"{demo[0]}{demo[1]:g}".replace(".", "_") if demo else spot


def spot_title(spot):
    demo = demo_spot(spot)
    return f"demo {demo[0]} paused at {demo[1]:g}s" if demo else SPOTS[spot][0]


def setviewpos_args(camera):
    x, y, z, yaw, pitch = camera
    return x, y, z - EYE_HEIGHT, yaw, pitch


# Diagnosis-only neutralisation for the shader-isolation scenario.
SCENARIOS = {
    "isolation": {
        "r_ocean": "0",
        "r_bakedSunRemoval": "0",
        "r_sunLighting": "0",
        "r_bakedIndirectScale": "1",
        "r_bakedIndirectSaturation": "1",
        "r_bakedIndirectPower": "1",
    },
    # The user's own cfg as found (ocean etc.).
    "realistic": {},
}
# Held identical in every scenario: measurement hygiene, not a look change.
COMMON = {
    "r_swapInterval": "0",
    "com_maxfps": "10000",
    "r_gpuTimings": "1",
    "cg_drawFPS": "0",
    "r_frameTimeGraph": "0",
    "cg_thirdPerson": "0",
}

FRAME_COLUMNS = [
    "frame_interval_ms", "frame_ms", "cpu_prepare_ms", "cpu_acquire_ms", "cpu_encode_ms",
    "primary_submit_api_ms", "present_ms", "gpu_latest_available_ms", "gpu_latest_world_ms",
    "world_encoded_batches", "world_multidraw_groups", "frame_inline_set_pipeline_calls",
    "world_general_program_binds", "world_specialized_program_binds", "world_minimal_program_binds",
    "world_shared_program_binds", "world_generated_program_binds", "world_shader_mode_active",
    "world_variant_pending", "world_active_pipelines", "world_active_specialized_pipelines",
    "bsp_primary_vertex_invocations_latest", "bsp_primary_fragment_invocations_latest",
]
SUMMARY_MEANS = [
    "cpu_prepare_ms", "cpu_acquire_ms", "cpu_encode_ms", "encode_scene_ms", "encoder_finish_ms",
    "primary_submit_api_ms", "present_ms", "gpu_latest_available_ms", "gpu_latest_world_ms",
    "gpu_latest_depth_ms", "gpu_latest_post_ms", "gpu_latest_ocean_surface_ms", "gpu_latest_ocean_optics_ms",
    "world_encoded_batches", "world_multidraw_groups", "frame_inline_set_pipeline_calls",
    "frame_inline_set_bind_group_calls", "frame_inline_draw_indexed_calls",
    "world_general_program_binds", "world_specialized_program_binds", "world_minimal_program_binds",
    "world_shared_program_binds", "world_generated_program_binds",
    "world_active_pipelines", "world_active_specialized_pipelines",
    "bsp_primary_vertex_invocations_latest", "bsp_primary_clipped_primitives_latest",
    "bsp_primary_fragment_invocations_latest",
    "ocean_surface_vertex_invocations_latest", "ocean_surface_clipped_primitives_latest",
    "ocean_surface_fragment_invocations_latest", "ocean_samples_passed_latest",
    "loop_top_ms", "loop_command_drain_ms", "loop_mailbox_ms", "loop_pre_render_ms",
    "loop_render_call_ms", "loop_post_render_prev_ms",
    "loop_pre_scene_rebuild_ms", "loop_pre_inline_models_ms", "plan_linear_depth_reasons",
]

SAMPLE_RE = re.compile(r"\[JKA PERF SAMPLE\] label=(\S+) fps_avg=([\d.]+).*?view=\((-?\d+) (-?\d+) (-?\d+)\) yaw=(-?[\d.]+) pitch=(-?[\d.]+)")
READY_RE = re.compile(r"\[JKA SHADER MODE (READY|TIMEOUT)\] mode=(\d) .*?waited_ms=(\d+) binds\[(.*?)\]")
WROTE_RE = re.compile(r"\[RENDER PERF\] wrote (\d+) frames to (.+?\.csv) and (.+?\.json)")


def read_cfg_lines(path):
    return path.read_text(encoding="utf-8", errors="replace").splitlines()


def write_bench_cfg(source_lines, overrides):
    """Replace or append `seta name "value"` lines (case-insensitive names)."""
    pending = {k.lower(): (k, v) for k, v in overrides.items()}
    out = []
    for line in source_lines:
        m = re.match(r'\s*seta?\s+(\S+)\s', line)
        if m and m.group(1).lower() in pending:
            name, value = pending.pop(m.group(1).lower())
            out.append(f'seta {name} "{value}"')
        else:
            out.append(line)
    for name, value in pending.values():
        out.append(f'seta {name} "{value}"')
    USER_CFG.write_text("\n".join(out) + "\n", encoding="utf-8")


def mode_sequence(modes, trial):
    # A B C C B A cancels linear drift; odd trials run C B A A B C so no mode
    # always takes the first (coldest) or middle slot.
    first = modes if trial % 2 == 0 else modes[::-1]
    return first + first[::-1]


def build_script(args, spot, tag, trial, seq, sets):
    lines = [f"set {k} {v}" for k, v in sets.items()]
    demo = demo_spot(spot)
    if demo:
        # Wait for playback, seek, freeze the frame (demo_speed 0 = paused).
        lines += ["perfsample 6 demoload", f"demo_seek {demo[1]}", "perfsample 2 seek",
                  "demo_speed 0", "perfsample 2 arrive", "viewpos"]
    else:
        x, y, z, yaw, pitch = setviewpos_args(SPOTS[spot][1])
        lines += [f"setviewpos_exact {x} {y} {z} {yaw} {pitch}", "perfsample 2 arrive", "viewpos"]
    labels = []
    if args.shots_only:
        # Image comparison: frozen clock (--freeze-time), no HUD/console, one shot per mode.
        lines += ["set cg_draw2D 0", "set con_notifytime 0"]
        for i, mode in enumerate(seq[: len(seq) // 2]):
            lines += [f"set r_fragmentShaderMode {mode}", f"waitshadermode {mode} {args.compile_timeout}",
                      "perfsample 2 shotsettle", "screenshot_png", "perfsample 2 shotwrite"]
        lines += ["set cg_draw2D 1", "set r_fragmentShaderMode 1", "quit"]
        return lines, []
    for i, mode in enumerate(seq):
        label = f"{tag}_{spot_id(spot)}_t{trial}_s{i}_m{mode}"
        assert len(label) <= 32, f"capture label too long: {label}"
        labels.append((label, mode, i))
        lines += [
            f"set r_fragmentShaderMode {mode}",
            f"waitshadermode {mode} {args.compile_timeout}",
            f"perfsample {args.settle} settle_{label}",
            f"render_perf_capture {args.seconds}s {label}",
            f"perfsample {args.seconds + 4} {label}",
        ]
        if args.screenshots and trial == 0 and i < len(seq) // 2:
            lines += ["screenshot_png", "perfsample 1 shot"]
    lines += ["set r_fragmentShaderMode 1", "quit"]
    return lines, labels


def summarize_capture(csv_path):
    with open(csv_path, newline="") as f:
        rows = list(csv.DictReader(f))
    def col(name):
        values = []
        for row in rows:
            v = row.get(name, "")
            if v not in ("", None):
                try:
                    x = float(v)
                except ValueError:
                    continue
                if math.isfinite(x):
                    values.append(x)
        return values
    def pct(values, p):
        if not values:
            return None
        s = sorted(values)
        pos = p * (len(s) - 1)
        lo, hi = math.floor(pos), math.ceil(pos)
        return s[lo] + (s[hi] - s[lo]) * (pos - lo)
    intervals = col("frame_interval_ms")
    out = {"frames": len(rows)}
    if intervals:
        mean = statistics.fmean(intervals)
        out.update({
            "fps": 1000.0 / mean, "frame_mean_ms": mean, "frame_p50_ms": pct(intervals, 0.5),
            "frame_p95_ms": pct(intervals, 0.95), "frame_p99_ms": pct(intervals, 0.99),
        })
    for name in SUMMARY_MEANS:
        values = col(name)
        out[name] = statistics.fmean(values) if values else None
    gw = col("gpu_latest_world_ms")
    out["gpu_world_p50_ms"] = pct(gw, 0.5)
    ga = col("gpu_latest_available_ms")
    out["gpu_frame_p50_ms"] = pct(ga, 0.5)
    modes = set(col("world_shader_mode_active"))
    out["active_modes_seen"] = sorted(int(m) for m in modes)
    out["pending_frames"] = int(sum(col("world_variant_pending")))
    # Every GpuPass (gpu_pass_<label>_ms): medians, for the per-pass breakdown.
    if rows:
        for name in rows[0]:
            if name.startswith("gpu_pass_"):
                out[name + "_p50"] = pct(col(name), 0.5)
    # Reduced per-frame export.
    return out, rows


def run_launch(args, spot, trial, seq, tag, outdir, sets, cfg_lines, exe, exe_name, case_name):
    map_name = spot_title(spot)
    script, labels = build_script(args, spot, tag, trial, seq, sets)
    run_dir = WORK / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    script_name = f"bench_shader_modes_{tag}.cfg"
    script_path = BASE / script_name
    launch_name = f"{tag}_{spot_id(spot)}_t{trial}"
    if args.dry_run:
        print(f"--- {launch_name}: {map_name}\n" + "\n".join(script))
        return []
    shutil.copy2(exe, run_dir / "DinurdoJK.exe")
    for dll in REL.glob("*.dll"):
        target = run_dir / dll.name
        if not target.exists() or target.stat().st_size != dll.stat().st_size:
            shutil.copy2(dll, target)
    backup = WORK / "DinurdoJK.cfg.backup"
    shutil.copy2(USER_CFG, backup)
    renderperf = BASE / "renderperf"
    results = []
    try:
        write_bench_cfg(cfg_lines, {**sets, **args.cfg_overrides})
        script_path.write_text("\n".join(script) + "\n", encoding="utf-8")
        log = run_dir / "latest.log"
        if log.exists():
            log.unlink()
        started = time.time()
        demo = demo_spot(spot)
        target = ["+demo", demo[0]] if demo else ["--map", SPOTS[spot][0]]
        cmd = [str(run_dir / "DinurdoJK.exe"), "--base", str(BASE), *target, "+exec", script_name]
        print(f"[bench] launch {launch_name} map={map_name} seq={seq}", flush=True)
        try:
            env = dict(os.environ)
            if args.freeze_time is not None:
                env["JKA_FIXED_SHADER_TIME"] = str(args.freeze_time)
            proc = subprocess.run(cmd, cwd=run_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                  timeout=args.timeout, env=env)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            code = "timeout"
        print(f"[bench] exit={code} after {time.time() - started:.0f}s", flush=True)
    finally:
        shutil.copy2(backup, USER_CFG)
        if script_path.exists():
            script_path.unlink()
    # Only screenshots written during this launch; the user's own share the folder.
    # Demos can switch fs_game (e.g. japlus/screenshots), so look in every game dir.
    shot_dirs = [d for d in REL.glob("*/screenshots") if d.is_dir()]
    if shot_dirs:
        new = sorted((p for d in shot_dirs for p in d.glob("shot*.png") if p.stat().st_mtime >= started),
                     key=lambda p: p.stat().st_mtime)
        shot_modes = seq[: len(seq) // 2] if args.shots_only or (args.screenshots and trial == 0) else []
        for index, shot in enumerate(new):
            (outdir / "shots").mkdir(exist_ok=True)
            mode = f"_m{shot_modes[index]}" if index < len(shot_modes) else ""
            shutil.move(str(shot), outdir / "shots" / f"{launch_name}{mode}_{shot.name}")
    (outdir / "logs").mkdir(exist_ok=True)
    text = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
    (outdir / "logs" / f"{launch_name}.log").write_text(text, encoding="utf-8")
    samples = {m.group(1): m for m in SAMPLE_RE.finditer(text)}
    readies = list(READY_RE.finditer(text))
    wrote = {Path(m.group(2)).stem: m for m in WROTE_RE.finditer(text)}
    (outdir / "frames").mkdir(exist_ok=True)
    for index, (label, mode, seq_index) in enumerate(labels):
        row = {"label": label, "spot": spot, "map": map_name, "trial": trial, "seq": seq_index, "mode": mode,
               "exe": exe_name, "case": case_name, "launch": launch_name, "launch_exit": str(code)}
        ready = readies[index] if index < len(readies) else None
        if ready:
            row["ready"] = ready.group(1)
            row["ready_wait_ms"] = int(ready.group(3))
            row["first_use_of_mode"] = seq.index(mode) == seq_index
        sample = samples.get(label)
        if sample:
            row["perfsample_fps"] = float(sample.group(2))
            row["view"] = f"{sample.group(3)} {sample.group(4)} {sample.group(5)} {sample.group(6)} {sample.group(7)}"
        stem = next((s for s in wrote if s.endswith("-" + label)), None)
        if stem:
            csv_path = Path(wrote[stem].group(2))
            json_path = Path(wrote[stem].group(3))
            stats, rows = summarize_capture(csv_path)
            row.update(stats)
            try:
                meta = json.loads(json_path.read_text(encoding="utf-8"))
                row["wall_fps"] = meta.get("wall_fps")
                row["adapter"] = meta.get("adapter")
                row["backend"] = meta.get("backend")
                row["fingerprint"] = meta.get("settings_fingerprint_at_arm")
            except (OSError, ValueError):
                pass
            extra = [c for c in (rows[0] if rows else {})
                     if c.startswith(("gpu_pass_", "loop_", "ocean_surface_", "ocean_samples_", "plan_"))
                     and c not in FRAME_COLUMNS]
            with gzip.open(outdir / "frames" / f"{label}.csv.gz", "wt", newline="") as f:
                w = csv.writer(f)
                w.writerow(FRAME_COLUMNS + extra)
                for r in rows:
                    w.writerow([r.get(c, "") for c in FRAME_COLUMNS + extra])
            # Only files this run produced (unique label); never anything else.
            for p in (csv_path, json_path):
                if p.exists() and p.parent.resolve() == renderperf.resolve() and label in p.name:
                    p.unlink()
        else:
            row["error"] = "no capture written"
        results.append(row)
    return results


def mean_ci(values):
    if not values:
        return None, None
    m = statistics.fmean(values)
    if len(values) < 2:
        return m, None
    return m, 1.96 * statistics.stdev(values) / math.sqrt(len(values))


def expected_view(spot, rows=()):
    if demo_spot(spot):
        # The paused demo camera: the view of this launch's first capture.
        views = [r.get("view") for r in rows if r.get("view")]
        return views[0] if views else None
    x, y, z, yaw, pitch = SPOTS[spot][1]
    pitch = pitch % 360
    pitch = pitch - 360 if pitch > 180 else pitch
    return f"{x} {y} {z} {float(yaw % 360):.1f} {float(pitch):.1f}"


def usable(row, results=()):
    """A capture is kept only on the requested camera with no capture error.
    Mouse/keyboard input during a run moves the view, which perfsample reports."""
    launch = [r for r in results if r.get("launch") == row.get("launch")] if demo_spot(row["spot"]) else ()
    return row.get("fps") and not row.get("error") and row.get("view") == expected_view(row["spot"], launch)


def report(results, args, outdir):
    everything = results
    dropped = [r for r in everything if r.get("fps") and not usable(r, everything)]
    results = [r for r in everything if usable(r, everything)]
    lines = [f"shader-mode benchmark  tag={args.tag}  scenario={args.scenario}  seconds={args.seconds}",
             f"exes={ {k: str(v) for k, v in args.exe_map.items()} }  cases={args.case_map}  trials={args.trials}  modes={args.modes}", ""]
    adapters = {r.get("adapter") for r in results if r.get("adapter")}
    fps_ok = [r for r in results if r.get("fps")]
    lines.append(f"adapter={', '.join(sorted(adapters))}")
    fingerprints = sorted({re.sub(r"shader_mode=\d;|active_world=[^;]*;", "", r.get("fingerprint") or "") for r in fps_ok})
    lines.append(f"distinct settings fingerprints (mode/variant removed): {len(fingerprints)}")
    for f in fingerprints:
        lines.append("  " + f)
    variants = sorted({(r.get("mode"), (r.get("fingerprint") or "").split("active_world=")[-1].split(";")[0]) for r in fps_ok})
    lines.append("active world variants: " + ", ".join(f"m{m}:{v}" for m, v in variants))
    lines.append("")
    base_mode = args.modes[0] if 1 not in args.modes else 1
    base_exe, base_case = next(iter(args.exe_map)), next(iter(args.case_map))
    groups = [(e, c, m) for e in args.exe_map for c in args.case_map for m in args.modes]
    for spot in dict.fromkeys(r["spot"] for r in results):
        lines.append(f"== {spot} ({spot_title(spot)})   delta = paired per trial vs exe={base_exe} case={base_case} mode={base_mode}")
        lines.append(
            f"{'exe':>8} {'case':>8} {'mode':>4} {'n':>2} {'fps':>8} {'±95%':>6} {'p50ms':>6} {'p95ms':>6} {'p99ms':>6} "
            f"{'gpuFr':>6} {'gpuWld':>6} {'prep':>5} {'acq':>5} {'enc':>5} {'subm':>5} {'pres':>5} {'draws':>5} "
            f"{'pso':>4} {'frag(M)':>7}  binds g/s/m/sh/gen   delta")
        rows_of = {}
        for r in results:
            if r["spot"] == spot and r.get("fps"):
                rows_of.setdefault((r["exe"], r["case"], r["mode"]), []).append(r)
        base_by_trial = {}
        for r in rows_of.get((base_exe, base_case, base_mode), []):
            base_by_trial.setdefault(r["trial"], []).append(r["fps"])
        for group in groups:
            rows = rows_of.get(group, [])
            exe_name, case_name, mode = group
            if not rows:
                lines.append(f"{exe_name:>8} {case_name:>8} {mode:>4}  -- no data")
                continue
            fps, ci = mean_ci([r["fps"] for r in rows])

            def avg(k):
                values = [r[k] for r in rows if r.get(k) is not None]
                return statistics.fmean(values) if values else float("nan")
            paired = []
            for trial, base_values in base_by_trial.items():
                mine = [r["fps"] for r in rows if r["trial"] == trial]
                if mine and base_values:
                    paired.append(100.0 * (statistics.fmean(mine) / statistics.fmean(base_values) - 1.0))
            pm, pci = mean_ci(paired)
            is_base = group == (base_exe, base_case, base_mode)
            delta = "" if is_base or pm is None else f"{pm:+.2f}%" + (f" ±{pci:.2f}" if pci else "")
            binds = "/".join(f"{avg(k):.0f}" for k in ("world_general_program_binds", "world_specialized_program_binds",
                                                      "world_minimal_program_binds", "world_shared_program_binds",
                                                      "world_generated_program_binds"))
            lines.append(
                f"{exe_name:>8} {case_name:>8} {mode:>4} {len(rows):>2} {fps:8.1f} {ci or 0:6.1f} {avg('frame_p50_ms'):6.3f} "
                f"{avg('frame_p95_ms'):6.3f} {avg('frame_p99_ms'):6.3f} {avg('gpu_frame_p50_ms'):6.3f} {avg('gpu_world_p50_ms'):6.3f} "
                f"{avg('cpu_prepare_ms'):5.3f} {avg('cpu_acquire_ms'):5.3f} {avg('cpu_encode_ms'):5.3f} "
                f"{avg('primary_submit_api_ms'):5.3f} {avg('present_ms'):5.3f} {avg('world_encoded_batches'):5.0f} "
                f"{avg('world_active_pipelines') + avg('world_active_specialized_pipelines'):4.0f} "
                f"{avg('bsp_primary_fragment_invocations_latest') / 1e6:7.3f}  {binds:<18} {delta}")
            passes = sorted(((avg(k), k[len("gpu_pass_"):-len("_ms_p50")]) for k in rows[0]
                             if k.startswith("gpu_pass_") and k.endswith("_p50")), reverse=True)
            passes = [(v, n) for v, n in passes if v == v and v >= 0.005]
            if passes:
                lines.append(f"{'':>22}gpu p50 ms: " + "  ".join(f"{n} {v:.3f}" for v, n in passes[:12]))
            reasons = avg('plan_linear_depth_reasons')
            if reasons == reasons:
                names = ["ssao", "rt_reduced", "temporal", "contact", "fog", "legacy_global_post", "clouds",
                         "snow", "atmosphere", "ssr", "reflection_debug", "motion_blur", "dof", "rain"]
                bits = int(round(reasons))
                lines.append(f"{'':>22}linear-depth prepass forced by: "
                             + (", ".join(n for i, n in enumerate(names) if bits >> i & 1) or "nothing"))
            loop_render = avg('loop_render_call_ms')
            if loop_render == loop_render:
                parts = [("top", "loop_top_ms"), ("drain", "loop_command_drain_ms"), ("mailbox", "loop_mailbox_ms"),
                         ("pre", "loop_pre_render_ms"), ("pre.rebuild", "loop_pre_scene_rebuild_ms"),
                         ("pre.inline", "loop_pre_inline_models_ms"), ("render()", "loop_render_call_ms"),
                         ("post", "loop_post_render_prev_ms")]
                lines.append(f"{'':>22}render loop ms: " + "  ".join(f"{name} {avg(key):.3f}" for name, key in parts)
                             + f"  (interval {avg('frame_mean_ms'):.3f})")
            ocean_fs = avg('ocean_surface_fragment_invocations_latest')
            ocean_passed = avg('ocean_samples_passed_latest')
            if ocean_fs == ocean_fs and ocean_fs > 0:
                ratio = ocean_fs / ocean_passed if ocean_passed and ocean_passed == ocean_passed else float("nan")
                lines.append(
                    f"{'':>22}ocean surface: VS {avg('ocean_surface_vertex_invocations_latest') / 1e6:.3f}M  "
                    f"tris {avg('ocean_surface_clipped_primitives_latest') / 1e6:.3f}M  FS {ocean_fs / 1e6:.3f}M  "
                    f"samples passed {ocean_passed / 1e6:.3f}M  FS/passed {ratio:.2f}  "
                    f"gpu {avg('gpu_latest_ocean_surface_ms'):.3f} ms")
        first = {}
        for r in results:
            if r["spot"] == spot and r.get("ready_wait_ms") is not None and r.get("first_use_of_mode"):
                first.setdefault((r["exe"], r["mode"]), []).append((r["ready_wait_ms"], r.get("ready")))
        lines.append("  mode-switch wait, first use per launch (1 s resolution; includes compile): " + ", ".join(
            f"{e}/m{m}={statistics.fmean([w for w, _ in v]):.0f}ms" + ("" if all(x == "READY" for _, x in v) else "(TIMEOUT)")
            for (e, m), v in sorted(first.items())))
        if not demo_spot(spot):
            lines.append(f"  expected view: {expected_view(spot)}")
        gone = [r["label"] for r in dropped if r["spot"] == spot]
        if gone:
            lines.append(f"  DROPPED (view drift / input during capture): {gone}")
        errors = [r["label"] for r in results if r["spot"] == spot and r.get("error")]
        if errors:
            lines.append(f"  MISSING captures: {errors}")
        lines.append("")
    (outdir / "report.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--scenario", choices=sorted(SCENARIOS), default="isolation")
    p.add_argument("--spots", default="costa,ffa3")
    p.add_argument("--spot-def", action="append", default=[],
                   help="name=map:x:y:z:yaw:pitch (repeatable) adds a camera spot usable in --spots")
    p.add_argument("--modes", default="0,1,2,3,4")
    p.add_argument("--trials", type=int, default=3)
    p.add_argument("--seconds", type=float, default=10.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--compile-timeout", type=int, default=180)
    p.add_argument("--timeout", type=int, default=1800)
    p.add_argument("--exe", action="append", default=[],
                   help="name=path (repeatable); launches alternate between exes. Default cur=target/release")
    p.add_argument("--case", action="append", default=[],
                   help="name=cvar:value;cvar:value (repeatable) console sets per case; launches alternate")
    p.add_argument("--tag", default="run")
    p.add_argument("--set", action="append", default=[], help="cvar=value console set after load")
    p.add_argument("--cfg", action="append", default=[], help="cvar=value written into the cfg before launch")
    p.add_argument("--screenshots", action="store_true")
    p.add_argument("--shots-only", action="store_true", help="one PNG per mode, no perf captures")
    p.add_argument("--freeze-time", type=float, default=None, help="JKA_FIXED_SHADER_TIME for the client")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--report", nargs="+", default=None,
                   help="only re-report (and merge) existing result dirs; --exclude-launch drops launches")
    p.add_argument("--exclude-launch", action="append", default=[],
                   help="label prefix of a launch to drop, e.g. abR-new-std_ffa3_t1")
    args = p.parse_args()
    args.modes = [int(m) for m in args.modes.split(",")]
    args.seconds = int(args.seconds) if args.seconds == int(args.seconds) else args.seconds
    args.cfg_overrides = dict(kv.split("=", 1) for kv in args.cfg)
    args.exe_map = {}
    for item in args.exe or [f"cur={REL / 'DinurdoJK.exe'}"]:
        name, _, path = item.partition("=") if "=" in item else ("cur", "", item)
        args.exe_map[name] = Path(path)
    args.case_map = {}
    for item in args.case or ["std="]:
        name, _, body = item.partition("=")
        args.case_map[name] = dict(kv.split(":", 1) for kv in body.split(";") if kv)
    if args.report:
        results = []
        for directory in args.report:
            results += json.loads((Path(directory) / "summary.json").read_text(encoding="utf-8"))
        results = [r for r in results if not any(r["label"].startswith(x) for x in args.exclude_launch)]
        args.exe_map = {r["exe"]: r["exe"] for r in results}
        args.case_map = {r["case"]: {} for r in results}
        args.modes = sorted({r["mode"] for r in results})
        outdir = Path(args.report[-1])
        report(results, args, outdir)
        return
    for item in args.spot_def:
        name, _, body = item.partition("=")
        map_name, *view = body.split(":")
        assert name and len(view) == 5, f"--spot-def {item!r}: expected name=map:x:y:z:yaw:pitch"
        SPOTS[name] = (map_name, tuple(float(v) for v in view))
    spots = args.spots.split(",")
    sets = {**COMMON, **SCENARIOS[args.scenario], **dict(kv.split("=", 1) for kv in args.set)}
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    outdir = RESULTS / f"shader-modes-{args.tag}-{args.scenario}-{stamp}"
    if not args.dry_run:
        outdir.mkdir(parents=True, exist_ok=True)
    WORK.mkdir(parents=True, exist_ok=True)
    cfg_lines = read_cfg_lines(USER_CFG)
    results = []
    combos = [(e, c) for e in args.exe_map for c in args.case_map]
    for trial in range(args.trials):
        for spot in (spots if trial % 2 == 0 else spots[::-1]):
            seq = mode_sequence(args.modes, trial)
            for exe_name, case_name in (combos if trial % 2 == 0 else combos[::-1]):
                tag = f"{args.tag}-{exe_name}-{case_name}"
                results += run_launch(args, spot, trial, seq, tag, outdir,
                                      {**sets, **args.case_map[case_name]}, cfg_lines,
                                      args.exe_map[exe_name], exe_name, case_name)
                if not args.dry_run:
                    (outdir / "summary.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    if args.dry_run:
        return
    keys = sorted({k for r in results for k in r})
    with open(outdir / "summary.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=keys)
        w.writeheader()
        for r in results:
            w.writerow({k: (json.dumps(v) if isinstance(v, list) else v) for k, v in r.items()})
    if not args.shots_only:
        report(results, args, outdir)
    print(f"[bench] results: {outdir}")


if __name__ == "__main__":
    sys.exit(main())
