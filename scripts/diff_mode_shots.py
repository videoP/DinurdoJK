#!/usr/bin/env python3
"""Compare bench_shader_modes.py screenshots against one reference mode.

    python scripts/diff_mode_shots.py <results-dir> [--ref 0] [--mask out.png]

Shots are named <launch>_m<mode>_shot*.png. Prints, per launch, the maximum and
mean 8-bit channel difference and the number of pixels differing by more than
1 and by more than 2 levels. Use a frozen clock (--freeze-time) when capturing,
otherwise animated stages dominate the difference.
"""
import argparse
import re
from pathlib import Path

import numpy as np
from PIL import Image


def main():
    p = argparse.ArgumentParser()
    p.add_argument("results")
    p.add_argument("--ref", type=int, default=0)
    p.add_argument("--masks", action="store_true", help="write <shot>.diff.png overlays")
    args = p.parse_args()
    shots = sorted((Path(args.results) / "shots").glob("*_m*_shot*.png"))
    launches = {}
    for shot in shots:
        m = re.match(r"(.+)_m(\d)_shot", shot.name)
        launches.setdefault(m.group(1), {})[int(m.group(2))] = shot
    for launch, by_mode in launches.items():
        if args.ref not in by_mode:
            print(f"{launch}: no reference mode {args.ref}")
            continue
        ref = np.asarray(Image.open(by_mode[args.ref]).convert("RGB")).astype(np.int16)
        for mode, shot in sorted(by_mode.items()):
            img = np.asarray(Image.open(shot).convert("RGB")).astype(np.int16)
            d = np.abs(img - ref).max(axis=2)
            print(f"{launch} m{mode} vs m{args.ref}: max={d.max()} mean={d.mean():.5f} "
                  f">1:{int((d > 1).sum())} >2:{int((d > 2).sum())}")
            if args.masks and d.max() > 1:
                overlay = np.asarray(Image.open(shot).convert("RGB")).copy()
                overlay[d > 1] = [255, 0, 255]
                Image.fromarray(overlay).save(shot.with_suffix(".diff.png"))


if __name__ == "__main__":
    main()
