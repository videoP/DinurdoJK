#!/usr/bin/env python3
"""Build GitHub Release assets for the signed DinurdoJK per-file updater.

Produces a full ZIP for manual installation and individually addressable files
for auto-update. Both key material and the signature stay out of the game ZIP.
The private key is read only from DINURDOJK_UPDATE_PRIVATE_KEY_B64.
Requires Python 3.11+ and `cryptography` (CI installs it).
"""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tomllib
import zipfile

# Binary dependencies now live under DinurdoJK/dependencies, not at the EXE root.
DEPENDENCY_FILES = (
    "phonon.dll",
    "OpenALSoft.x86_64.dll",
    "nvngx_dlss.dll",
    "dxcompiler.dll",
    "dxil.dll",
    "msvcp140.dll",
    "msvcp140_atomic_wait.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "ffmpeg.exe",  # optional: only published when supplied by the build
)
DEPENDENCY_PREFIX = "DinurdoJK/dependencies/"
CLIENT_ASSET = "DinurdoJK/shared/dinurdo-assets.pk4"

ALLOWED_FILES = (
    "DinurdoJK.exe",
    *(DEPENDENCY_PREFIX + name for name in DEPENDENCY_FILES),
    CLIENT_ASSET,
    "OpenAL-Soft-LGPL-2.0.txt",
    "EalMan-LGPL-2.1.txt",
    "Bevy-Solari-MIT.txt",
    "NVIDIA-DLSS-SDK-LICENSE.txt",
    "DXC-LICENSE.txt",
    "DXC-ThirdPartyNotices.txt",
)
LICENSE_FILES = {
    "OpenAL-Soft-LGPL-2.0.txt",
    "EalMan-LGPL-2.1.txt",
    "Bevy-Solari-MIT.txt",
    "NVIDIA-DLSS-SDK-LICENSE.txt",
    "DXC-LICENSE.txt",
    "DXC-ThirdPartyNotices.txt",
}
MAX_BYTES = 1024 * 1024 * 1024
MAX_MANIFEST = 16 * 1024


def version_tag():
    with Path("crates/jka-client/Cargo.toml").open("rb") as file:
        package = tomllib.loads(file.read().decode("utf-8-sig"))["package"]
    if package["name"] != "DinurdoJK":
        raise ValueError("expected the DinurdoJK Cargo package")
    value = package["version"]
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", value):
        raise ValueError("release version must be MAJOR.MINOR.PATCH")
    return f"v{value}"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def asset_name(name):
    # Release assets are flat; destinations in the signed manifest are not.
    if name == CLIENT_ASSET:
        return "dinurdo-assets.pk4"
    if name.startswith(DEPENDENCY_PREFIX):
        return name[len(DEPENDENCY_PREFIX):]
    return name


def read_file(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"missing or unsafe release file: {path}")
    data = path.read_bytes()
    if not data or len(data) > MAX_BYTES:
        raise ValueError(f"empty or oversized release file: {path}")
    return data


def signing_key():
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

    private_b64 = os.environ.get("DINURDOJK_UPDATE_PRIVATE_KEY_B64", "").strip()
    public_hex = os.environ.get("DINURDOJK_UPDATE_PUBLIC_KEY_HEX", "").strip()
    if not re.fullmatch("[0-9a-fA-F]{64}", public_hex):
        raise ValueError("missing DINURDOJK_UPDATE_PUBLIC_KEY_HEX environment variable")
    try:
        seed = base64.b64decode(private_b64, validate=True)
        if len(seed) != 32:
            raise ValueError("private key must be 32 raw Ed25519 bytes encoded in Base64")
        private = Ed25519PrivateKey.from_private_bytes(seed)
        public = Ed25519PublicKey.from_public_bytes(bytes.fromhex(public_hex))
        probe = b"DinurdoJK key pair validation"
        public.verify(private.sign(probe), probe)
    except Exception as exc:
        raise ValueError("signing key does not match public verification key") from exc
    return private, public


def create(runtime, output, tag):
    if tag != version_tag():
        raise ValueError(f"{tag!r} does not match Cargo version {version_tag()!r}")
    private, public = signing_key()
    payload = {}
    for name in ALLOWED_FILES:
        if name == CLIENT_ASSET:
            source = Path("assets/dinurdo-assets.pk4")
        elif name in LICENSE_FILES:
            source = runtime / "licenses" / name
        elif name.startswith(DEPENDENCY_PREFIX):
            # Support both updated build.ps1 outputs and legacy root-level
            # Cargo staging. The *published* layout is always the new one.
            source = runtime / name
            if not source.is_file():
                source = runtime / asset_name(name)
        else:
            source = runtime / name
        if source.is_file():
            payload[name] = read_file(source)
    for required in ("DinurdoJK.exe", *(DEPENDENCY_PREFIX + dll for dll in
                     ("OpenALSoft.x86_64.dll", "nvngx_dlss.dll", "dxcompiler.dll", "dxil.dll")),
                     "DXC-LICENSE.txt", "DXC-ThirdPartyNotices.txt",
                     "NVIDIA-DLSS-SDK-LICENSE.txt", CLIENT_ASSET):
        if required not in payload:
            raise ValueError(f"mandatory release file is missing: {required}")
    if len(payload) > len(ALLOWED_FILES) or sum(map(len, payload.values())) > MAX_BYTES:
        raise ValueError("signed release exceeds file/count/size limits")

    if output.exists() and any(output.iterdir()):
        raise ValueError("release output directory must be empty")
    output.mkdir(parents=True, exist_ok=True)

    # A complete, standalone ZIP for manual downloads; individual files for
    # hash-based updater downloads (the updater never downloads the ZIP).
    full_name = f"DinurdoJK-{tag}-full.zip"
    full_path = output / full_name
    with zipfile.ZipFile(full_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6, allowZip64=False) as z:
        for name, data in sorted(payload.items()):
            item = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
            item.compress_type = zipfile.ZIP_DEFLATED
            item.external_attr = 0o100644 << 16
            z.writestr(item, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=6)
            (output / asset_name(name)).write_bytes(data)

    manifest = {
        "format": 2,
        "tag": tag,
        "files": {name: {"size": len(data), "sha256": sha256(data)}
                  for name, data in sorted(payload.items())},
    }
    raw = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    if len(raw) > MAX_MANIFEST:
        raise ValueError("signed manifest exceeds 16KiB verifier limit")
    signature = private.sign(raw)  # Exactly 64 raw bytes (not Base64/hex).
    public.verify(signature, raw)
    if len(signature) != 64:
        raise AssertionError("incorrect Ed25519 signature length")
    with zipfile.ZipFile(full_path, "r") as z:
        if set(z.namelist()) != set(payload):
            raise AssertionError("full ZIP layout does not match manifest")
        for name, data in payload.items():
            if z.read(name) != data or read_file(output / asset_name(name)) != data:
                raise AssertionError(f"ZIP or individual asset mismatch: {name}")
    (output / "release-manifest.json").write_bytes(raw)
    (output / "release-manifest.sig").write_bytes(signature)
    print(f"Signed {tag}: {len(payload)} individual files and {full_name}")
    print("These release assets will be published:")
    for path in sorted(output.iterdir()):
        print(f"  {path.name}: {path.stat().st_size:,} bytes")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--print-tag", action="store_true")
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--tag")
    args = parser.parse_args()
    if args.print_tag:
        print(version_tag())
    else:
        if args.runtime is None or args.out is None or args.tag is None:
            parser.error("--runtime, --out and --tag are required")
        create(args.runtime, args.out, args.tag)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Signed release packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
