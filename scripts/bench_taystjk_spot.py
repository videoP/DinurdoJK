#!/usr/bin/env python3
"""External baseline: TaystJK (Vulkan renderer) FPS at a fixed camera.

    python scripts/bench_taystjk_spot.py --spot ffa3 [--frames 12000] [--trials 3]

TaystJK has no per-spot frame logger, so each trial records a static-camera demo
on a local devmap (spectator, `setviewpos`, timescale 10) and plays it back with
`timedemo 2`, which prints average FPS and frame-time percentiles. The camera
TaystJK ends up at (its setviewpos has no pitch and applies the 400 u/s push) is
printed by `viewpos`; measure DinurdoJK at that same camera with
`setviewpos_exact x y z-37 yaw 0`.

Assets come from DinurdoJK's target/release/base (fs_basepath); TaystJK's
config and logs live in an isolated fs_homepath under target/bench-work, so no
user configuration is touched. Results: bench/results/taystjk-<spot>-<time>.txt
"""
import argparse
import datetime
import re
import shutil
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# Scratch copy of D:\Code\TaystJK\build\msvc16_x64\Release (its folder lacks an x64
# SDL2.dll); cgame/jampgame/ui x86_64 modules go in HOME/base.
TAYST = REPO / "target" / "bench-work" / "tayst-run" / "taystjk.x86_64.exe"
BASEPATH = REPO / "target" / "release"
HOME = REPO / "target" / "bench-work" / "taystjk-home"
RESULTS = REPO / "bench" / "results"
SPOTS = {  # TaystJK setviewpos arguments (feet origin, yaw)
    "costa": ("costadelsol", (1936, 831, 826, 15)),
    "ffa3": ("mp/ffa3", (693, -1450, -108, 89)),
}
# Matched to DinurdoJK's isolation scenario as far as vanilla cvars allow.
SETTINGS = {
    "cl_renderer": "rd-vulkant", "r_mode": "-1", "r_customwidth": "2560",
    "r_customheight": "1440", "r_fullscreen": "0", "r_swapInterval": "0",
    "com_maxfps": "0", "r_picmip": "0", "r_texturemode": "GL_LINEAR_MIPMAP_NEAREST",
    "r_ext_texture_filter_anisotropic": "0", "r_ext_multisample": "0",
    "r_dynamiclight": "0", "r_fastsky": "0", "cg_drawFPS": "0", "cg_thirdPerson": "0",
    "logfile": "2", "sv_pure": "0", "com_hunkmegs": "512",
}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--spot", choices=sorted(SPOTS), default="ffa3")
    p.add_argument("--frames", type=int, default=12000, help="wait frames while recording")
    p.add_argument("--trials", type=int, default=3)
    p.add_argument("--timeout", type=int, default=900)
    args = p.parse_args()
    map_name, (x, y, z, yaw) = SPOTS[args.spot]
    base_home = HOME / "base"
    base_home.mkdir(parents=True, exist_ok=True)
    lines = []
    for trial in range(args.trials):
        log = base_home / "qconsole.log"
        if log.exists():
            log.unlink()
        demo = f"tayst_{args.spot}_{trial}"
        script = [
            "team spectator", "wait 300",
            f"setviewpos {x} {y} {z} {yaw}", "wait 2000", "viewpos",
            "timescale 10", f"record {demo}", f"wait {args.frames}", "stoprecord",
            "timescale 1", "wait 100", "disconnect", "wait 100",
            "timedemo 2", f"demo {demo}", f"wait {args.frames * 2}", "quit",
        ]
        (base_home / "bench_spot.cfg").write_text("\n".join(script) + "\n", encoding="utf-8")
        cmd = [str(TAYST), "+set", "fs_basepath", str(BASEPATH), "+set", "fs_homepath", str(HOME)]
        for name, value in SETTINGS.items():
            cmd += ["+set", name, value]
        cmd += ["+devmap", map_name, "+exec", "bench_spot.cfg"]
        try:
            subprocess.run(cmd, cwd=TAYST.parent, timeout=args.timeout,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            status = "exit"
        except subprocess.TimeoutExpired:
            status = "timeout"
        text = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
        view = re.findall(r"\((-?\d+) (-?\d+) (-?\d+)\) : (-?\d+)", text)
        stats = [l for l in text.splitlines() if re.search(r"frames, .* fps|Timedemo|%|msec|low", l)]
        lines.append(f"trial {trial} ({status}) viewpos={view[-1] if view else None}")
        lines += ["  " + l for l in stats]
        print("\n".join(lines[-(len(stats) + 1):]), flush=True)
        shutil.copy2(log, HOME / f"qconsole_{args.spot}_{trial}.log") if log.exists() else None
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    RESULTS.mkdir(parents=True, exist_ok=True)
    out = RESULTS / f"taystjk-{args.spot}-{stamp}.txt"
    out.write_text(f"TaystJK {TAYST} rd-vulkant {args.spot} ({map_name})\n" + "\n".join(lines) + "\n",
                   encoding="utf-8")
    print(f"[tayst] results: {out}")


if __name__ == "__main__":
    main()
