"""Vendor byte-identical TaystJK sources and their local include closure.

TaystJK (an OpenJK fork) is the only JKA/Base movement and collision source.
Input: the pinned codeload archive, default target/taystjk-client.zip, or the
path given with --archive. No user-owned game assets are copied.

    python scripts/vendor-movement.py [--archive PATH]   # import
    python scripts/vendor-movement.py --check            # verify hashes

Import overwrites files, including the local jaPRO sailing hunks in
bg_pmove.c, so run it only to refresh the pin deliberately.
"""
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
REVISION = "b35ed06fec41c53644352743c6b199a5d5d500f3"
DEST = ROOT / "crates/jka-movement/vendor/japro"
REPOSITORY = "https://github.com/taysta/TaystJK"
ARCHIVE = ROOT / "target/taystjk-client.zip"
if "--archive" in sys.argv:
    ARCHIVE = Path(sys.argv[sys.argv.index("--archive") + 1])
PREFIX = "TaystJK-" + REVISION + "/"
SOURCES = [
    "LICENSE.txt", "codemp/cgame/cg_local.h", "codemp/cgame/animtable.h",
    "codemp/game/bg_pmove.c", "codemp/game/bg_slidemove.c",
    "codemp/game/bg_panimate.c", "codemp/game/bg_saber.c",
    "codemp/game/bg_misc.c", "codemp/game/bg_weapons.c",
    "shared/qcommon/q_math.c", "codemp/qcommon/q_shared.c",
    "shared/qcommon/q_string.c",
    # Clip-map collision (JKA maps) and the engine's Sys_SnapVector.
    "codemp/qcommon/cm_load.cpp", "codemp/qcommon/cm_trace.cpp",
    "codemp/qcommon/cm_test.cpp", "codemp/qcommon/cm_patch.cpp",
    "codemp/qcommon/cm_polylib.cpp", "shared/sys/snapvector.cpp",
]


def main():
    if "--check" in sys.argv:
        manifest = json.loads((DEST / "manifest.json").read_text())
        for item in manifest["files"]:
            actual = hashlib.sha256((DEST / item["path"]).read_bytes()).hexdigest()
            if actual != item["sha256"]:
                raise ValueError("Modified vendor source: " + item["path"])
        print(f"Verified {len(manifest['files'])} byte-identical files at {manifest['revision']}.")
        return
    with zipfile.ZipFile(ARCHIVE) as archive:
        prefix = PREFIX
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
            "repository": REPOSITORY,
            "revision": REVISION,
            "files": [{"path": p, "sha256": h} for p, h in sorted(copied.items())],
        }, indent=2) + "\n")
        print(f"Vendored {len(copied)} original source/header files.")


if __name__ == "__main__":
    main()
