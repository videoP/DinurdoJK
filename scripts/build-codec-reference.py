"""Build an isolated oracle from pinned OpenJK functions; run under WSL with g++.

Fetch sources with fetch-openjk.ps1 first. Extracted GPL source stays in ignored
.references alongside its license. No original C++ is linked into the Rust app.
"""
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REVISION = "1a6a643427aa347553e9073dac5570b33337c4d9"
SOURCE = ROOT / ".references" / "OpenJK" / REVISION
BUILD = ROOT / ".references" / "codec-harness"


def extract_function(source, name):
    match = re.search(r"^(?:void|int)\s+" + name + r"\([^;]*?\)\s*\{", source, re.M)
    if not match:
        raise ValueError(f"Function not found: {name}")
    depth = 1
    end = match.end()
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[match.start():end]


def main():
    manifest = json.loads((SOURCE / "manifest.json").read_text(encoding="utf-8-sig"))
    assert manifest["revision"] == REVISION
    for item in manifest["files"]:
        assert hashlib.sha256((SOURCE / item["path"]).read_bytes()).hexdigest() == item["sha256"]
    common = (SOURCE / "codemp/qcommon/qcommon.h").read_text()
    msg = (SOURCE / "codemp/qcommon/msg.cpp").read_text()
    stripped = re.sub(r"/\*.*?\*/|//[^\n]*", "", msg, flags=re.S)
    frequency = re.search(r"int msg_hData\[256\]\s*=\s*\{.*?\};", stripped, re.S).group()
    msg_struct = re.search(r"typedef struct msg_s\s*\{.*?\} msg_t;", common, re.S).group()
    huff_start = common.index("#define NYT")
    huff_end = common.index("extern huffman_t clientHuffTables", huff_start)
    include = BUILD / "qcommon"
    include.mkdir(parents=True, exist_ok=True)
    shim = """#pragma once
#include <cstring>
#include <stdexcept>
using byte = unsigned char;
using qboolean = int;
constexpr int qfalse = 0, qtrue = 1, ERR_DROP = 1;
#define Com_Memset std::memset
#define Com_Memcpy std::memcpy
inline void CopyLittleShort(void* dst, const void* src) { std::memcpy(dst, src, 2); }
inline void CopyLittleLong(void* dst, const void* src) { std::memcpy(dst, src, 4); }
[[noreturn]] inline void Com_Error(int, const char* message, ...) { throw std::runtime_error(message); }
"""
    (include / "qcommon.h").write_text(shim + msg_struct + "\n" + common[huff_start:huff_end])
    functions = "\n".join(extract_function(msg, name) for name in ["MSG_WriteBits", "MSG_ReadBits", "MSG_ReadByte", "MSG_ReadShort", "MSG_ReadLong", "MSG_initHuffman"])
    (BUILD / "message.inc").write_text("static huffman_t msgHuff;\nstatic int msgInit, oldsize, overflows;\n" + frequency + "\n" + functions)
    entity_block = re.search(r"netField_t\s+entityStateFields\[\]\s*=\s*\{(.*?)\};", stripped, re.S).group(1)
    entries = re.findall(r"\{\s*NETF\((.*?)\),\s*(\w+|-?\d+)\s*\}", entity_block)
    constants = {"GENTITYNUM_BITS": 10, "MAX_POWERUPS": 16}
    fields = [(name, constants[width] if width in constants else int(width)) for name, width in entries]
    entity_shim = """
#include <cstddef>
#include <cstdint>
#define ARRAY_LEN(a) (sizeof(a) / sizeof((a)[0]))
constexpr int MAX_GENTITIES = 1024, GENTITYNUM_BITS = 10, FLOAT_INT_BITS = 13, FLOAT_INT_BIAS = 4096;
struct netField_t { const char* name; int offset; int bits; };
struct cvar_t { int integer; };
static cvar_t* cl_shownet = nullptr;
static struct { int state; } sv{};
struct gentity_t { const char* classname; };
inline gentity_t* SV_GentityNum(int) { static gentity_t value{}; return &value; }
inline void Com_Printf(const char*, ...) {}
"""
    entity_shim += f"struct entityState_t {{ int number; uint32_t fields[{len(fields)}]; }};\nnetField_t entityStateFields[] = {{\n"
    entity_shim += "".join(f'{{"{name}", int(offsetof(entityState_t, fields) + {index} * 4), {width}}},\n' for index, (name, width) in enumerate(fields)) + "};\n"
    (BUILD / "entity.inc").write_text(entity_shim + extract_function(msg, "MSG_ReadDeltaEntity"))
    compiler = subprocess.check_output(["g++", "--version"], text=True).splitlines()[0]
    executable = BUILD / "codec-oracle"
    subprocess.run(["g++", "-std=c++17", "-O0", "-fwrapv", "-fno-strict-aliasing", "-DFINAL_BUILD", "-I", str(BUILD),
                    str(ROOT / "scripts/codec-oracle.cpp"), str(SOURCE / "codemp/qcommon/huffman.cpp"),
                    "-o", str(executable)], check=True)
    output = subprocess.check_output([str(executable)], text=True)
    codes, vectors, gamestate = [], [], None
    for line in output.splitlines():
        kind, *values = line.split("\t")
        if kind == "code":
            symbol, bits, width = map(int, values)
            assert symbol == len(codes)
            codes.append((bits, width))
        elif kind == "vector":
            vectors.append("\t".join(values))
        elif kind == "gamestate":
            gamestate = bytes.fromhex(values[0])
        else:
            raise ValueError(line)
    assert len(codes) == 256
    generated = ROOT / "crates/jka-protocol/src/huffman_codes.rs"
    generated.write_text(
        f"// Generated by scripts/build-codec-reference.py from OpenJK {REVISION}.\n"
        "// (LSB-first codeword, bit length), indexed by byte value.\n"
        "pub(crate) const CODES: [(u16, u8); 256] = [\n"
        + "".join(f"    ({bits}, {width}), // {symbol}\n" for symbol, (bits, width) in enumerate(codes)) + "];\n"
    )
    fixture_dir = ROOT / "crates/jka-protocol/tests/fixtures"
    fixture_dir.mkdir(parents=True, exist_ok=True)
    (fixture_dir / "codec-vectors.tsv").write_text(
        "# prefix_bits\tread_width\tinput_u32\texpected_i32\tbits\tbytes\thex\n" + "\n".join(vectors) + "\n")
    assert gamestate
    (fixture_dir / "gamestate.bin").write_bytes(gamestate)
    (ROOT / "crates/jka-protocol/src/entity_fields.rs").write_text(
        f"// Stock protocol schema from OpenJK {REVISION}; see reference manifest.\n"
        "pub const ENTITY_FIELDS: &[(&str, i8)] = &[\n"
        + "".join(f'    ("{name}", {width}),\n' for name, width in fields) + "];\n")
    metadata = dict(manifest, compiler=compiler, compiler_flags=["-std=c++17", "-O0", "-fwrapv", "-fno-strict-aliasing", "-DFINAL_BUILD"],
                    scope="MSG bit codec, trained Huffman table and delta entity routine; not full engine conformance", vector_count=len(vectors),
                    source_functions=["MSG_WriteBits", "MSG_ReadBits", "MSG_ReadByte", "MSG_ReadShort", "MSG_ReadLong", "MSG_initHuffman", "MSG_ReadDeltaEntity"],
                    entity_layout="Surrogate slots with mapped field offsets; original C++ delta routine unchanged; not an ABI-layout test.",
                    note="Writer uses positive width to avoid the source's negative-shift diagnostic; reader uses recorded signed width.")
    (fixture_dir / "reference.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Generated {len(codes)} codewords, {len(vectors)} oracle vectors and {len(fields)} entity fields.")


if __name__ == "__main__":
    main()
