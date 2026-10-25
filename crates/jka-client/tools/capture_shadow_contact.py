"""Bounded background visual repro; uses existing runtime/assets and no build cache.

Requires the updated DJK_HEADLESS safeguards. Produces screenshots and a small
manifest. --perf records paired GPU timings; hidden runs do not establish
interactive FPS. --previous reuses the standard build script's preserved exe.
"""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import sys


def main():
    root = Path(__file__).resolve().parents[3]
    runtime = root / "target/release"
    base = runtime / "base"
    config = base / "DinurdoJK.cfg"
    screenshots = base / "screenshots"
    geometry = "--geometry" in sys.argv
    perf = "--perf" in sys.argv
    previous = "--previous" in sys.argv
    label = ("perf-previous" if previous else "perf-current") if perf else ("geometry" if geometry else "capture")
    before = set(screenshots.glob("*"))
    perf_before = set((base / "renderperf").glob("*"))
    config_hash = hashlib.sha256(config.read_bytes()).hexdigest()
    commands = [
        "r_dynamicShadows csm", "r_bakedLocalLights 0", "r_lampShadowAB 3", "r_fragmentShaderMode 1",
        "r_bakedLocalGlow 1", "r_bakedLocalAdditive 1", "r_bakedLocalEmissive 1", "r_bakedLocalSurfacelight 1", "r_bakedLocalFx 0",
        "r_playerContactShadows 0", "s_volume 0", "cg_draw2D 0",
        "cg_thirdPerson 1", "team free", "wait 1500",
        # Approximate actors reconstructed from the reported camera positions.
        "setviewpos_exact 1576.27 892.05 -82.15 182 49", "wait 300", "viewpos", "waitshadermode 1 60",
        "screenshot_png", "wait 100",
        "r_playerContactShadows 1", "wait 100", "screenshot_png", "wait 100",
        "setviewpos_exact 1623.91 1069.90 6.33 178 12", "wait 300", "viewpos",
        "screenshot_png", "wait 100",
        "setviewpos_exact 1206.59 1672.05 229.36 344 40", "wait 300", "viewpos",
        "screenshot_png", "wait 100", "quit",
    ]
    cases = ["weak_without_contact", "weak_with_contact", "strong_with_contact", "cafe_with_contact"]
    if perf:
        commands.insert(0, "r_gpuTimings 1")
        capture_at = commands.index("screenshot_png")
        commands[capture_at:capture_at] = [
            f"render_perf_capture 600 attenuation_{label}_1", "wait 1000",
            f"render_perf_capture 600 attenuation_{label}_2", "wait 1000",
        ]
    if geometry:
        cases = ["csm_contact_on", "csm_contact_off", "csm_lamps_off", "classic_blob"]
        commands = [
            "r_dynamicShadows csm", "r_bakedLocalLights 0", "r_lampShadowAB 3", "r_fragmentShaderMode 1",
            "r_bakedLocalGlow 1", "r_bakedLocalAdditive 1", "r_bakedLocalEmissive 1", "r_bakedLocalSurfacelight 1", "r_bakedLocalFx 0",
            "r_playerContactShadows 1", "s_volume 0", "cg_draw2D 0", "cg_thirdPerson 1",
            "team free", "wait 1500", "setviewpos_exact 1576.27 892.05 -82.15 182 49", "wait 300",
            "viewpos", "waitshadermode 1 60", "render_perf_capture 30 contact_on", "wait 800", "screenshot_png", "wait 100",
            "r_playerContactShadows 0", "wait 100", "render_perf_capture 30 contact_off", "wait 800",
            "screenshot_png", "wait 100",
            "r_bakedLocalGlow 0", "r_bakedLocalAdditive 0", "r_bakedLocalEmissive 0", "r_bakedLocalSurfacelight 0", "r_bakedLocalFx 0",
            "waitshadermode 1 60", "wait 100", "screenshot_png", "wait 100", "r_dynamicShadows blob", "wait 500", "waitshadermode 1 60",
            "render_perf_capture 30 classic_blob", "wait 800", "screenshot_png", "wait 100", "quit",
        ]
    env = dict(os.environ, DJK_HEADLESS="1")
    log_path = root / f".codex-shadow-contact-{label}.log"
    with log_path.open("wb") as output:
        process = subprocess.Popen(
            [str(runtime / ("DinurdoJK.exe.prev" if previous else "DinurdoJK.exe")), "--base", str(base), "--map", "mp/ffa3",
             "+" + "; ".join(commands)], cwd=runtime, env=env,
            stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        try:
            result = process.wait(timeout=150)
        except subprocess.TimeoutExpired:
            process.terminate()  # Only the process created above.
            process.wait(timeout=15)
            result = "timeout"
    files = sorted(set(screenshots.glob("*")) - before, key=lambda p: p.stat().st_mtime_ns)
    manifest = {
        "exit": result,
        "config_unchanged": hashlib.sha256(config.read_bytes()).hexdigest() == config_hash,
        "cases": cases,
        "screenshots": [str(p) for p in files],
        "geometry_captures": [str(p) for p in set((base / "renderperf").glob("*")) - perf_before],
        "log": str(log_path),
        "note": "Inspect logged viewpos and images; actor positions are approximate. GPU timestamp comparisons require matching settings and no competing game. No interactive FPS acceptance.",
    }
    (root / f".codex-shadow-contact-{label}.json").write_text(json.dumps(manifest, indent=2))
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
