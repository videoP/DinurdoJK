"""Vendor byte-identical stock OpenJK sources and their local include closure.

Input: target/openjk-stock.zip downloaded from the pinned codeload URL.
No user-owned game assets are copied.
"""
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
REVISION = "1a6a643427aa347553e9073dac5570b33337c4d9"
DEST = ROOT / "crates/jka-movement/vendor/openjk"
SOURCES = [
    "LICENSE.txt",
    "codemp/game/bg_pmove.c", "codemp/game/bg_slidemove.c",
    "codemp/game/bg_panimate.c", "codemp/game/bg_saber.c",
    "codemp/game/bg_misc.c", "codemp/game/bg_weapons.c",
    "codemp/cgame/cg_local.h",
    "codemp/qcommon/cm_load.cpp", "codemp/qcommon/cm_trace.cpp",
    "codemp/qcommon/cm_test.cpp", "codemp/qcommon/cm_patch.cpp",
    "codemp/qcommon/cm_polylib.cpp",
    "shared/qcommon/q_math.c", "codemp/qcommon/q_shared.c",
    "shared/qcommon/q_string.c",
    "shared/sys/snapvector.cpp",
    "codemp/game/g_xcvar.h", "codemp/game/w_force.c",
    "codemp/game/g_client.c", "codemp/game/g_active.c",
]


def main():
    if "--check" in sys.argv:
        manifest = json.loads((DEST / "manifest.json").read_text())
        for item in manifest["files"]:
            actual = hashlib.sha256((DEST / item["path"]).read_bytes()).hexdigest()
            if actual != item["sha256"]:
                raise ValueError("Modified vendor source: " + item["path"])
        print(f"Verified {len(manifest['files'])} byte-identical OpenJK files at {manifest['revision']}.")
        return
    with zipfile.ZipFile(ROOT / "target/openjk-stock.zip") as archive:
        prefix = "OpenJK-" + REVISION + "/"
        available = {n[len(prefix):]: n for n in archive.namelist() if n.startswith(prefix)}
        pending = SOURCES.copy()
        copied = {}
        while pending:
            relative = pending.pop()
            if relative in copied:
                continue
            if relative not in available:
                raise ValueError("Missing " + relative)
            data = archive.read(available[relative])
            target = DEST / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            copied[relative] = hashlib.sha256(data).hexdigest()
            for include in re.findall(rb'^\s*#\s*include\s*["<]([^">]+)[">]', data, re.M):
                name = include.decode()
                candidates = [str(PurePosixPath(relative).parent / name), "codemp/" + name, "shared/" + name]
                for candidate in candidates:
                    # Normalize ../ without touching the filesystem.
                    parts = []
                    for part in candidate.split("/"):
                        if part == "..":
                            if parts:
                                parts.pop()
                        elif part != ".":
                            parts.append(part)
                    candidate = "/".join(parts)
                    if candidate in available:
                        pending.append(candidate)
                        break
        (DEST / "manifest.json").write_text(json.dumps({
            "repository": "https://github.com/JACoders/OpenJK",
            "revision": REVISION,
            "files": [{"path": p, "sha256": h} for p, h in sorted(copied.items())],
        }, indent=2) + "\n")
        print(f"Vendored {len(copied)} original source/header files.")


if __name__ == "__main__":
    main()
