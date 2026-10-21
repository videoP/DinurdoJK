#!/usr/bin/env python3
"""Capture Hybrid crest modes from one frozen sea; reuse the standard executable.

    python scripts/ocean_breaker_ab.py
    python scripts/ocean_breaker_ab.py --time 117 --view 1936 831 863 15 -339

No binaries or build trees are copied. A private game directory isolates the
capture settings and screenshots without changing the user's config.
"""
import argparse
import datetime
import os
import re
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / "target/release"
BASE = RELEASE / "base"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, default=RELEASE / "DinurdoJK.exe")
    parser.add_argument("--time", type=float, default=100)
    parser.add_argument("--view", nargs=5, type=float, default=[1936, 831, 863, 15, -339],
                        metavar=("X", "Y", "Z", "YAW", "PITCH"))
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--no-wire", action="store_true", help="capture only the three shaded comparisons")
    args = parser.parse_args()
    out = ROOT / "bench/results" / ("ocean-breaker-" + datetime.datetime.now().strftime("%Y%m%d-%H%M%S"))
    out.mkdir(parents=True)
    saved = (BASE / "DinurdoJK.cfg").read_bytes()
    cfg = out / "DinurdoJK.cfg"
    command_file = out / "ocean_breaker_ab_capture.cfg"
    if command_file.exists():
        raise RuntimeError(f"Capture command file already exists: {command_file}")
    settings = {
        "r_ocean": 1, "r_oceanType": 3, "r_blueWaterHeight": 55.8,
        "r_blueWaterChoppiness": 2.2543192, "r_blueWaterWavelength": 512,
        "r_hybridFftBlend": 1, "r_hybridBreaker": 0,
        "r_oceanCostProbe": 0, "r_oceanPixelDetail": 0,
        "r_showtris": 0, "cg_draw2D": 0, "con_notifytime": 0,
        "r_swapInterval": 0, "com_maxfps": 144,
    }
    x, y, z, yaw, pitch = args.view
    commands = [f"set {key} {value}" for key, value in settings.items()]
    commands += [f"setviewpos_exact {x:g} {y:g} {z - 37:g} {yaw:g} {pitch:g}", "perfsample 10 arrive", "viewpos"]
    labels = []
    for wire in ((0,) if args.no_wire else (0, 32)):  # r_showtris OCEAN
        commands += [f"set r_showtris {wire}"]
        for mode in (0, 1, 2):
            labels.append(f"mode{mode}" + ("-wire" if wire else ""))
            # hybridwave writes the camera/mesh classification alongside its PNG.
            commands += [f"set r_hybridBreaker {mode}", "perfsample 2 settle", "hybridwave", "perfsample 2 write"]
    commands += ["quit"]
    (out / "commands.cfg").write_text("\n".join(commands) + "\n", encoding="utf-8")
    command_file.write_text("\n".join(commands) + "\n", encoding="utf-8")
    try:
        # Promotion is applied while loading the map. A runtime-only ocean
        # enable cannot promote a map that was loaded with ocean disabled.
        pending = {key.lower(): (key, value) for key, value in settings.items()}
        lines = []
        for line in saved.decode("utf-8", errors="replace").splitlines():
            match = re.match(r"\s*seta?\s+(\S+)\s", line)
            if match and match[1].lower() in pending:
                key, value = pending.pop(match[1].lower())
                line = f'seta {key} "{value}"'
            lines.append(line)
        lines += [f'seta {key} "{value}"' for key, value in pending.values()]
        cfg.write_text("\n".join(lines) + "\n", encoding="utf-8")
        env = dict(os.environ)
        env["JKA_FIXED_SHADER_TIME"] = str(args.time)
        env["PATH"] = str(RELEASE) + os.pathsep + env.get("PATH", "")
        with (out / "process.log").open("w", encoding="utf-8") as log:
            result = subprocess.run([str(args.exe.resolve()), "--base", str(BASE), "--game", str(out), "--map", "costadelsol",
                                     "+exec", command_file.name], cwd=out, env=env, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=args.timeout)
        if result.returncode:
            raise RuntimeError(f"Game exited {result.returncode}; see {out / 'process.log'}")
    finally:
        command_file.unlink()
    shots = sorted((out / "screenshots").glob("*.png"), key=lambda p: p.stat().st_mtime)
    for label, shot in zip(labels, shots):
        shutil.move(str(shot), out / (label + ".png"))
    print(f"{len(shots)} screenshots: {out}")
    if len(shots) != len(labels):
        raise RuntimeError("Incomplete capture; inspect process.log")


if __name__ == "__main__":
    main()
