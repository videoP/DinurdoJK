"""Vendor pinned shared JK2 gameplay, include closure, and isolated bridge tables.

Usage: python crates/jka-movement/reference/vendor_mvsdk.py D:/Code/mvsdk
Source bytes are copied verbatim from Git, never from a dirty working tree.
"""
from pathlib import Path, PurePosixPath
import hashlib
import json
import posixpath
import re
import subprocess
import sys

crate = Path(__file__).resolve().parents[1]
sdk = Path(sys.argv[1]).resolve()
revision = "4c7c463638192aead1ff3de8a88cc5de266bf3a2"
git = ["git", "-c", f"safe.directory={sdk.as_posix()}", "-C", str(sdk)]
assert subprocess.check_output(git + ["rev-parse", "HEAD"], text=True).strip() == revision
files = set(subprocess.check_output(git + ["ls-tree", "-r", "--name-only", revision], text=True).splitlines())
sources = [f"code/game/{name}.c" for name in ["bg_pmove", "bg_slidemove", "bg_panimate", "bg_saber", "bg_weapons", "bg_misc", "bg_multiversion", "animMappingTable", "q_math", "q_shared"]]
include_dirs = ["code/game", "code/cgame", "code/renderer", "code/qcommon", "code/ui", "code"]
destination = crate / "vendor/mvsdk"
pending = sources + ["LICENSE.txt"]
copied = {}
while pending:
    path = pending.pop()
    if path in copied:
        continue
    data = subprocess.check_output(git + ["show", f"{revision}:{path}"])
    copied[path] = data
    target = destination / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)
    for include in re.findall(rb'^\s*#\s*include\s*"([^"]+)"', data, re.M):
        name = include.decode()
        # These branches are inactive in the JK2_CGAME shared-code build.
        if path in sources and name in ("g_local.h", "../game/g_local.h", "../ui/ui_local.h"):
            continue
        candidates = [posixpath.normpath(str(PurePosixPath(path).parent / name))]
        candidates += [posixpath.normpath(f"{root}/{name}") for root in include_dirs]
        matches = [p for p in candidates if p in files]
        if not matches:
            raise RuntimeError(f"Unresolved include {path}: {name}")
        pending.append(matches[0])

def cleaned(text):
    return re.sub(r'/\*.*?\*/|//[^\n]*|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'', lambda m: ' ' * len(m[0]), text, flags=re.S)

# Existing isolated OpenJK symbols are a superset of most shared math helpers.
# Add every non-static file-scope definition in the pinned SDK compilation set.
symbols = set(re.findall(r'^#define (\w+) ', (crate / "native/japro_namespace.h").read_text(), re.M))
symbols = {s for s in symbols if not s.startswith(("jka_", "japro_"))}
for source in sources:
    text = cleaned(copied[source].decode())
    depth = [0]
    for ch in text:
        depth.append(depth[-1] + (ch == '{') - (ch == '}'))
    for match in re.finditer(r'^([\w \t*]+?)\b(\w+)\s*\([^;{}]*\)\s*\{', text, re.M):
        if depth[match.start()] == 0 and "static" not in match[1].split():
            symbols.add(match[2])
    for match in re.finditer(r'^([\w \t*]+?)\b(\w+)\s*(?:\[[^;{}]*?\])?\s*(?:=|;)', text, re.M):
        if depth[match.start()] == 0 and not set(match[1].split()) & {"static", "typedef", "extern", "return"}:
            symbols.add(match[2])
symbols.update(["trap_FS_FOpenFile", "trap_FS_Read", "trap_FS_FCloseFile", "trap_Error", "trap_Print", "trap_SnapVector", "trap_Cvar_VariableStringBuffer", "trap_Cvar_VariableIntegerValue", "animTable", "animTable_1_02", "BGPAFtext", "bgGlobalAnimations", "jk2version", "jk2gameplay", "jk2startversion", "animMappingTable_1_02_to_1_04", "animMappingTable_1_04_to_1_02"])
header = "/* Isolate pinned MVSDK globals from both JKA implementations. */\n#pragma once\n"
header += "".join(f"#define {name} mvsdk_{name}\n" for name in sorted(symbols))
(crate / "native/jk2_namespace.h").write_text(header, newline="\n")

wire = (crate.parent / "jka-protocol/src/jk2/fields.rs").read_text()
tables = "/* Native offsets, not a foreign struct cast. Generated from pinned JK2 wire descriptors. */\n"
for version in [102, 104]:
    fields = re.findall(r'\("([^"]+)", (-?\d+)\)', re.search(rf'pub const PLAYER_FIELDS_{version}:.*?= &\[(.*?)\];', wire, re.S)[1])
    tables += f"static const jk2_field fields_{version}[] = {{\n"
    tables += "".join(f'    {{"{name}", offsetof(playerState_t, {name}), {bits}}},\n' for name, bits in fields)
    tables += "};\n"
(crate / "native/jk2_fields.h").write_text(tables, newline="\n")

# Name every native member, including timers and the complete Force struct.
# Offsets and array sizes are evaluated by C, never assumed by Rust.
native = copied["code/game/q_shared.h"].decode()
members = []
for tag, prefix in [("playerState_s", ""), ("forcedata_s", "fd.")]:
    body = re.search(rf'typedef struct {tag}\s*\{{(.*?)\}}', native, re.S)[1]
    body = re.sub(r'/\*.*?\*/|//[^\n]*', '', body, flags=re.S)
    for declaration in body.split(';'):
        declaration = declaration.strip()
        if not declaration:
            continue
        match = re.fullmatch(r'(\w+)\s+(\w+)(\s*\[[^]]+\])?', declaration)
        if not match:
            raise RuntimeError(f"Unrecognized native member: {declaration!r}")
        if match[1] != "forcedata_t":
            members.append(prefix + match[2])
names = '/* Generated native member ranges; no Rust layout assumptions. */\n'
names += 'static const jk2_member native_members[] = {\n'
names += ''.join(f'    {{"{name}", offsetof(playerState_t, {name}), sizeof(((playerState_t *)0)->{name})}},\n' for name in members)
names += '};\n'
(crate / "native/jk2_state_members.h").write_text(names, newline="\n")
manifest = {"repository": "https://github.com/mvdevs/mvsdk", "revision": revision, "sources": sources,
            "sha256": {p: hashlib.sha256(data).hexdigest() for p, data in sorted(copied.items())}}
(destination / "PROVENANCE.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(f"Vendored {len(copied)} pinned files; isolated {len(symbols)} symbols")
