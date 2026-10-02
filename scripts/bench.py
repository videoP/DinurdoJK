#!/usr/bin/env python3
"""Declarative benchmark runner for the DinurdoJK client.

    bench.cmd bench\\plans\\deluxe.bench            (from the repo root)
    python scripts\\bench.py <plan> [--launches N] [--only a,b] [--dry-run]

Reads a plan file (see bench/plans/README.txt), launches the release client once
per case per round through scripts/bench-map.sh (so the user's cfg is backed up
and restored), parses the [JKA PERF ...] log lines, and writes one text report
to bench/results/. The report is meant to be pasted/read by Claude: summary
tables first, then per-launch numbers, then pointers to the full logs.

Rounds are interleaved (A B C, A B C, ...) so thermal drift and background load
hit every case equally. Do not touch the mouse or keyboard while it runs: the
game is windowed and the view would drift.
"""
import argparse
import datetime
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
REL = REPO / "target" / "release"
USER_CFG = REL / "base" / "DinurdoJK.cfg"
WORK = REPO / "target" / "bench-work"
RESULTS = REPO / "bench" / "results"

SAMPLE_RE = re.compile(
    r"\[JKA PERF SAMPLE\] label=(\S+) fps_avg=([\d.]+) fps_min=([\d.]+) fps_max=([\d.]+) "
    r"cpu_frame_avg=([\d.]+)ms gpu_frame_avg=(\S+) windows=(\d+) "
    r"view=\((-?\d+) (-?\d+) (-?\d+)\) yaw=(-?[\d.]+) pitch=(-?[\d.]+)"
)
MS_FIELD_RE = re.compile(r"(\w+)=(-?[\d.]+)ms")


@dataclass
class Spot:
    name: str
    x: str
    y: str
    z: str
    yaw: str
    pitch: str | None


@dataclass
class Case:
    name: str
    sets: list = field(default_factory=list)  # (cvar, value) applied after map load
    cfgs: dict = field(default_factory=dict)  # (cvar -> value) written into the cfg
    cmds: list = field(default_factory=list)  # extra console commands before the sample
    map: str | None = None


@dataclass
class Plan:
    path: Path
    map: str = ""
    timeout: int = 400
    launches: int = 3
    settle: float = 3.0
    sample: float = 5.0
    sets: list = field(default_factory=list)
    cfgs: dict = field(default_factory=dict)
    cmds: list = field(default_factory=list)
    spots: list = field(default_factory=list)
    cases: list = field(default_factory=list)


def die(message):
    sys.exit(f"bench: {message}")


def parse_plan(path):
    plan = Plan(path=path)
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        key, args = parts[0].lower(), parts[1:]
        where = f"{path.name}:{number}"
        try:
            if key == "map":
                plan.map = args[0]
            elif key == "timeout":
                plan.timeout = int(args[0])
            elif key == "launches":
                plan.launches = int(args[0])
            elif key == "settle":
                plan.settle = float(args[0])
            elif key == "sample":
                plan.sample = float(args[0])
            elif key == "set":
                plan.sets.append((args[0], " ".join(args[1:])))
            elif key == "cfg":
                plan.cfgs[args[0]] = " ".join(args[1:])
            elif key == "cmd":
                plan.cmds.append(args)
            elif key == "spot":
                if not re.fullmatch(r"[\w-]+", args[0]) or len(args) not in (5, 6):
                    raise ValueError("spot NAME X Y Z YAW [PITCH]")
                plan.spots.append(Spot(args[0], *args[1:5], args[5] if len(args) == 6 else None))
            elif key == "case":
                case = Case(name=args[0])
                if not re.fullmatch(r"[\w-]+", case.name):
                    raise ValueError("case names are letters/digits/_/- only")
                for token in args[1:]:
                    if token.startswith("cfg:"):
                        name, _, value = token[4:].partition("=")
                        case.cfgs[name] = value
                    elif token.startswith("cmd:"):
                        case.cmds.append(token[4:].split(","))
                    elif token.startswith("map="):
                        case.map = token[4:]
                    elif "=" in token:
                        name, _, value = token.partition("=")
                        case.sets.append((name, value))
                    else:
                        raise ValueError(f"bad case token {token!r}")
                plan.cases.append(case)
            else:
                raise ValueError(f"unknown keyword {key!r}")
        except (IndexError, ValueError) as error:
            die(f"{where}: {error}")
    if not plan.cases:
        die("plan has no `case` lines")
    if not plan.spots:
        die("plan has no `spot` lines")
    if not plan.map and not all(case.map for case in plan.cases):
        die("plan needs a `map` line (or map= on every case)")
    return plan


def find_bash():
    git_bash = Path(r"C:\Program Files\Git\bin\bash.exe")
    if git_bash.exists():
        return str(git_bash)
    found = shutil.which("bash")
    if not found:
        die("Git Bash not found (needed to run scripts/bench-map.sh)")
    return found


def posix(path):
    return str(path).replace("\\", "/")


def write_cfg(path, overrides):
    """Copy the user's cfg with `overrides` applied (replace or append)."""
    text = USER_CFG.read_text(encoding="utf-8", errors="replace")
    eol = "\r\n" if "\r\n" in text else "\n"
    lines = text.splitlines()
    pending = dict(overrides)
    for index, line in enumerate(lines):
        match = re.match(r'\s*seta?\s+(\S+)\s', line)
        if match and match.group(1) in pending:
            lines[index] = f'seta {match.group(1)} "{pending.pop(match.group(1))}"'
    for name, value in pending.items():
        lines.append(f'seta {name} "{value}"')
    path.write_text(eol.join(lines) + eol, encoding="utf-8")


def console_args(plan, case):
    args = []
    for name, value in plan.sets + case.sets:
        args += ["+set", name, *value.split()]
    for spot in plan.spots:
        args += ["+setviewpos", spot.x, spot.y, spot.z, spot.yaw]
        if spot.pitch is not None:
            args.append(spot.pitch)
        args += ["+perfsample", f"{plan.settle:g}", f"settle_{spot.name}"]
        for command in plan.cmds + case.cmds:
            args.append("+" + command[0])
            args += command[1:]
        args += ["+perfsample", f"{plan.sample:g}", f"run_{spot.name}"]
    args.append("+quit")
    return args


def number(text):
    try:
        return float(text)
    except ValueError:
        return None


def mean(values):
    values = [v for v in values if v is not None]
    return sum(values) / len(values) if values else None


def parse_log(lines, plan):
    """Returns {spot: metrics dict} for one launch."""
    sample_index = {}
    for index, line in enumerate(lines):
        match = SAMPLE_RE.search(line)
        if match:
            sample_index[match.group(1)] = (index, match)
    out = {}
    for spot in plan.spots:
        run = sample_index.get(f"run_{spot.name}")
        if not run:
            continue
        run_at, m = run
        settle = sample_index.get(f"settle_{spot.name}")
        window = lines[(settle[0] + 1 if settle else 0):run_at]
        metrics = {
            "fps": number(m.group(2)),
            "fps_min": number(m.group(3)),
            "fps_max": number(m.group(4)),
            "cpu_frame": number(m.group(5)),
            "gpu_frame": number(m.group(6).removesuffix("ms")) if m.group(6) != "--" else None,
            "view": tuple(int(m.group(i)) for i in (8, 9, 10)),
            "yaw": number(m.group(11)),
            "pitch": number(m.group(12)),
            "passes": {},
            "variant": None,
        }
        per_second = {"acquire": [], "encode": [], "submit": [], "present": [], "encoded": []}
        passes = {}
        for line in window:
            if "[JKA PERF] fps=" in line:
                for name, value in MS_FIELD_RE.findall(line):
                    if name in per_second:
                        per_second[name].append(number(value))
                found = re.search(r"encoded=(\d+)", line)
                if found:
                    per_second["encoded"].append(float(found.group(1)))
            elif "[JKA PERF GPU]" in line:
                for name, value in MS_FIELD_RE.findall(line):
                    passes.setdefault(name, []).append(number(value))
            elif "[JKA PERF STATE]" in line:
                found = re.search(r"pipeline_variant=(\S+)", line)
                if found:
                    metrics["variant"] = found.group(1)
        for name, values in per_second.items():
            metrics[name] = mean(values)
        metrics["passes"] = {name: mean(values) for name, values in passes.items() if mean(values)}
        out[spot.name] = metrics
    return out


def run_launch(bash, plan, case, round_index, log_dir, dry):
    map_name = case.map or plan.map
    cfg_path = WORK / f"cfg_{case.name}.cfg"
    write_cfg(cfg_path, {"r_fullscreen": "0", **plan.cfgs, **case.cfgs})
    args = console_args(plan, case)
    command = [bash, posix(REPO / "scripts" / "bench-map.sh"), str(plan.timeout), map_name, *args]
    if dry:
        print("   ", " ".join(command[1:]))
        return None, ""
    env = dict(os.environ, JKA_BENCH_DIR=posix(WORK), JKA_BENCH_CFG=posix(cfg_path))
    started = time.time()
    try:
        subprocess.run(command, cwd=REPO, env=env, capture_output=True, timeout=plan.timeout + 120)
    except subprocess.TimeoutExpired:
        pass
    log_path = WORK / "run" / "latest.log"
    text = log_path.read_text(encoding="utf-8", errors="replace") if log_path.exists() else ""
    lines = text.splitlines()
    log_dir.mkdir(parents=True, exist_ok=True)
    (log_dir / f"{case.name}_r{round_index + 1}.log").write_text(text, encoding="utf-8")
    return parse_log(lines, plan), f"{time.time() - started:.0f}s"


def fmt(value, digits=1, width=0):
    text = "--" if value is None else f"{value:.{digits}f}"
    return text.rjust(width)


def table(rows, headers):
    widths = [max(len(str(row[i])) for row in [headers] + rows) for i in range(len(headers))]
    lines = ["  ".join(str(h).ljust(w) if i == 0 else str(h).rjust(w) for i, (h, w) in enumerate(zip(headers, widths)))]
    lines.append("  ".join("-" * w for w in widths))
    for row in rows:
        lines.append("  ".join(str(c).ljust(w) if i == 0 else str(c).rjust(w) for i, (c, w) in enumerate(zip(row, widths))))
    return lines


def build_report(plan, results, started, exe_info, log_dir):
    """results: {case: [ {spot: metrics} or None per launch ]}"""
    out = []
    out.append(f"BENCH REPORT  plan={plan.path.name}  {started:%Y-%m-%d %H:%M:%S}")
    out.append(f"exe: {exe_info}")
    out.append(
        f"map={plan.map or '(per case)'} launches={plan.launches} settle={plan.settle:g}s "
        f"sample={plan.sample:g}s timeout={plan.timeout}s"
    )
    out.append("plan-wide set: " + (", ".join(f"{n}={v}" for n, v in plan.sets) or "(none)"))
    out.append("plan-wide cfg: " + (", ".join(f"{n}={v}" for n, v in plan.cfgs.items()) or "(none)") + "  (+ r_fullscreen=0 unless overridden)")
    out.append("cases:")
    for case in plan.cases:
        bits = [f"{n}={v}" for n, v in case.sets] + [f"cfg:{n}={v}" for n, v in case.cfgs.items()]
        bits += [f"cmd:{','.join(c)}" for c in case.cmds] + ([f"map={case.map}"] if case.map else [])
        out.append(f"  {case.name}: {' '.join(bits) or '(baseline settings)'}")
    out.append("")
    out.append("Frame time is the honest unit: dms = (this - first case) ms/frame; positive = slower.")
    out.append("noise% = (max-min)/mean of the per-launch FPS. Treat dms smaller than the noise as unproven.")
    out.append("")

    warnings = []
    for spot in plan.spots:
        out.append("=" * 100)
        out.append(f"SPOT {spot.name}: setviewpos {spot.x} {spot.y} {spot.z} {spot.yaw}" + (f" {spot.pitch}" if spot.pitch else ""))
        out.append("=" * 100)
        rows, detail = [], []
        baseline_ms = None
        pass_names = []
        for case in plan.cases:
            for launch in results[case.name]:
                if launch and spot.name in launch:
                    for name in launch[spot.name]["passes"]:
                        if name not in pass_names and name != "frame":
                            pass_names.append(name)
        for case in plan.cases:
            runs = [launch[spot.name] for launch in results[case.name] if launch and spot.name in launch]
            if not runs:
                rows.append([case.name, 0, "FAILED", "", "", "", "", "", "", ""])
                warnings.append(f"{case.name}/{spot.name}: no sample in any launch (see {log_dir.name}/)")
                continue
            views = [(r["yaw"], r["pitch"]) for r in runs]
            median_view = (statistics.median(v[0] for v in views), statistics.median(v[1] for v in views))
            for index, r in enumerate(runs):
                if abs(r["yaw"] - median_view[0]) > 1.5 or abs(r["pitch"] - median_view[1]) > 1.5:
                    warnings.append(f"{case.name}/{spot.name}: launch {index + 1} view drifted (yaw {r['yaw']}, pitch {r['pitch']}) - mouse touched?")
            fps = [r["fps"] for r in runs]
            fps_mean = mean(fps)
            ms = 1000.0 / fps_mean
            if baseline_ms is None:
                baseline_ms = ms
            noise = (max(fps) - min(fps)) / fps_mean * 100
            rows.append([
                case.name, len(runs), fmt(fps_mean), f"{min(fps):.0f}..{max(fps):.0f}", f"{noise:.1f}",
                fmt(ms, 3), f"{ms - baseline_ms:+.3f}", fmt(mean(r["gpu_frame"] for r in runs), 3),
                fmt(mean(r["cpu_frame"] for r in runs), 3), fmt(mean(r["encoded"] for r in runs), 0),
            ])
            detail.append([
                case.name, fmt(mean(r["acquire"] for r in runs), 3), fmt(mean(r["encode"] for r in runs), 3),
                *[fmt(mean(r["passes"].get(p) for r in runs), 3) for p in pass_names],
                next((r["variant"] for r in runs if r["variant"]), "--"),
            ])
        out += table(rows, ["case", "n", "fps", "fps range", "noise%", "ms/frame", "dms", "gpu_ms", "cpu_ms", "draws"])
        out.append("")
        out.append("per-pass GPU ms (needs r_gpuTimings 1; '--' otherwise), CPU acquire/encode, shader variant:")
        out += table(detail, ["case", "acquire", "encode", *pass_names, "variant"])
        out.append("")
        out.append("per-launch fps:")
        for case in plan.cases:
            values = [fmt(launch[spot.name]["fps"]) if launch and spot.name in launch else "FAIL" for launch in results[case.name]]
            out.append(f"  {case.name}: " + "  ".join(values))
        out.append("")
    if warnings:
        out.append("WARNINGS")
        out += [f"  ! {w}" for w in warnings]
        out.append("")
    out.append(f"full logs: {log_dir}")
    return "\n".join(out) + "\n"


def exe_description():
    exe = REL / "DinurdoJK.exe"
    if not exe.exists():
        die(f"{exe} missing - build first (powershell -File build.ps1 -Fast)")
    stamp = datetime.datetime.fromtimestamp(exe.stat().st_mtime)
    try:
        rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True, text=True).stdout.strip()
        rev += " (+uncommitted changes)" if dirty else ""
    except OSError:
        rev = "?"
    return f"built {stamp:%Y-%m-%d %H:%M:%S}, git {rev}"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("plan", type=Path)
    parser.add_argument("--launches", type=int, help="override the plan's launches")
    parser.add_argument("--only", help="comma-separated case names to run")
    parser.add_argument("--dry-run", action="store_true", help="print the launch commands and exit")
    options = parser.parse_args()

    plan = parse_plan(options.plan)
    if options.launches:
        plan.launches = options.launches
    if options.only:
        wanted = set(options.only.split(","))
        unknown = wanted - {c.name for c in plan.cases}
        if unknown:
            die(f"unknown case(s): {', '.join(sorted(unknown))}")
        plan.cases = [c for c in plan.cases if c.name in wanted]

    bash = find_bash()
    exe_info = exe_description()
    WORK.mkdir(parents=True, exist_ok=True)
    started = datetime.datetime.now()
    stem = f"{plan.path.stem}-{started:%Y%m%d-%H%M%S}"
    log_dir = RESULTS / f"{stem}-logs"
    results = {case.name: [] for case in plan.cases}

    total = plan.launches * len(plan.cases)
    print(f"bench: {len(plan.cases)} case(s) x {plan.launches} launch(es) = {total} runs. Do not touch the mouse/keyboard.")
    print(f"bench: {exe_info}")
    done = 0
    try:
        for round_index in range(plan.launches):
            for case in plan.cases:
                done += 1
                print(f"[{done}/{total}] round {round_index + 1} case {case.name} ...", flush=True)
                parsed, elapsed = run_launch(bash, plan, case, round_index, log_dir, options.dry_run)
                if options.dry_run:
                    continue
                results[case.name].append(parsed)
                if parsed:
                    summary = "  ".join(f"{name}={m['fps']:.0f}fps" for name, m in parsed.items())
                    print(f"        {elapsed}: {summary}")
                else:
                    print(f"        {elapsed}: NO SAMPLE (stalled? see log)")
    except KeyboardInterrupt:
        print("bench: interrupted; writing partial report")
    if options.dry_run:
        return
    RESULTS.mkdir(parents=True, exist_ok=True)
    report = build_report(plan, results, started, exe_info, log_dir)
    report_path = RESULTS / f"{stem}.txt"
    report_path.write_text(report, encoding="utf-8")
    print()
    print(report)
    print(f"bench: report written to {report_path}")


if __name__ == "__main__":
    main()
