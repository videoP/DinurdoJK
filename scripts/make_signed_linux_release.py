#!/usr/bin/env python3
"""Sign Linux/x86-64 runtime files without changing the Windows or macOS assets.

The executable, Steam Audio library and shared PK4 are included in the signed
v2 manifest. The executable is installed as DinurdoJK.x86_64: the client data
folder DinurdoJK/ sits beside it, so a file named DinurdoJK cannot coexist. The shared PK4 is already owned/published by the Windows release.
No privately signed manifest should be published if any package input is absent.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import tarfile
import tomllib
import io

MAX_FILE = 1024 * 1024 * 1024
MAX_MANIFEST = 16 * 1024
# Must match updater.rs GAME_BINARY for Linux.
INSTALLED_BINARY = 'DinurdoJK.x86_64'


def version_tag() -> str:
    with Path('crates/jka-client/Cargo.toml').open('rb') as file:
        package = tomllib.load(file)['package']
    if package['name'] != 'DinurdoJK':
        raise ValueError('unexpected Cargo package')
    version = package['version']
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError('invalid Cargo version')
    return 'v' + version


def read_safe(path: Path) -> bytes:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f'missing or symlinked file: {path}')
    data = path.read_bytes()
    if not 0 < len(data) <= MAX_FILE:
        raise ValueError(f'empty or oversized file: {path}')
    return data


def signing_keys():
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
    seed = base64.b64decode(os.environ['DINURDOJK_UPDATE_PRIVATE_KEY_B64'].strip(), validate=True)
    public = bytes.fromhex(os.environ['DINURDOJK_UPDATE_PUBLIC_KEY_HEX'].strip())
    if len(seed) != 32 or len(public) != 32:
        raise ValueError('Ed25519 seed and public key must be 32 bytes')
    signer = Ed25519PrivateKey.from_private_bytes(seed)
    verifier = Ed25519PublicKey.from_public_bytes(public)
    proof = b'DinurdoJK Linux release signing key check'
    verifier.verify(signer.sign(proof), proof)
    return signer, verifier


def tar_file(output: tarfile.TarFile, name: str, data: bytes, mode: int):
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mtime = 0
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = ''
    info.gname = ''
    output.addfile(info, io.BytesIO(data))


def create(binary: Path, phonon: Path, asset: Path, license_file: Path,
           output: Path, tag: str):
    if tag != version_tag():
        raise ValueError(f'tag {tag!r} does not match Cargo version {version_tag()}')
    if binary.name != 'DinurdoJK' or binary.stat().st_mode & 0o111 == 0:
        raise ValueError('Linux binary must be named DinurdoJK and executable')
    if phonon.name != 'libphonon.so':
        raise ValueError('Steam Audio dependency must be named libphonon.so')
    signer, verifier = signing_keys()
    payload = {
        INSTALLED_BINARY: read_safe(binary),
        'DinurdoJK/dependencies/libphonon.so': read_safe(phonon),
        'DinurdoJK/shared/dinurdo-assets.pk4': read_safe(asset),
    }
    license_bytes = read_safe(license_file)
    if sum(len(data) for data in payload.values()) > MAX_FILE:
        raise ValueError('Linux signed install payload exceeds client aggregate size limit')
    metadata = {
        'format': 2, 'tag': tag,
        'files': {name: {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                  for name, data in sorted(payload.items())},
    }
    raw = json.dumps(metadata, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()
    if len(raw) > MAX_MANIFEST:
        raise ValueError('manifest exceeds client size cap')
    signature = signer.sign(raw)
    verifier.verify(signature, raw)
    if output.exists() and any(output.iterdir()):
        raise ValueError('refusing to overwrite an existing Linux release')
    output.mkdir(parents=True, exist_ok=True)
    asset_names = {
        INSTALLED_BINARY: 'DinurdoJK-linux-x86_64',
        'DinurdoJK/dependencies/libphonon.so': 'libphonon-linux-x86_64.so',
    }
    for name, release_name in asset_names.items():
        (output / release_name).write_bytes(payload[name])
    (output / 'release-manifest-linux-x86_64.json').write_bytes(raw)
    (output / 'release-manifest-linux-x86_64.sig').write_bytes(signature)
    launch = (b'#!/bin/sh\nset -eu\n'
              b'cd -- "$(dirname -- "$0")"\n'
              b'export LD_LIBRARY_PATH="$(pwd)/DinurdoJK/dependencies${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"\n'
              b'exec ./DinurdoJK.x86_64 "$@"\n')
    readme = (b'DinurdoJK Linux x86-64 standalone (Ubuntu 24.04 build)\n'
              b'Extract the whole archive to a writable directory, then run\n'
              b'./Launch-DinurdoJK.sh or ./DinurdoJK.x86_64. Do not run as root.\n\n'
              b'Game assets are not included; configure your legal Jedi Academy / JK2 install\n'
              b'through the Asset Mapping screen.\n\n'
              b'Requires: Vulkan GPU driver/loader, an X11 or Wayland desktop, ALSA runtime,\n'
              b'and system curl (/usr/bin/curl) for HTTPS updates/ghosts.\n'
              b'Optional: ffmpeg for video export; xdg-utils for opening file paths.\n'
              b'The executable has an $ORIGIN runtime library search path for the bundled\n'
              b'Steam Audio library. Updating needs write access to this folder.\n'
              b'Linux packages installed through a system package manager should use that\n'
              b'package manager to update instead of modifying system-owned directories.\n'
              b'Built against Ubuntu 24.04 glibc; older distributions may not run it.\n'
              b'Steam Audio is developed by Valve and distributed under Apache 2.0.\n')
    archive = output / f'DinurdoJK-{tag}-linux-x86_64-standalone.tar.gz'
    with tarfile.open(archive, 'w:gz', format=tarfile.PAX_FORMAT) as archive_out:
        for name, data in payload.items():
            tar_file(archive_out, name, data, 0o755 if name == INSTALLED_BINARY else 0o644)
        tar_file(archive_out, 'Launch-DinurdoJK.sh', launch, 0o755)
        tar_file(archive_out, 'README-LINUX.txt', readme, 0o644)
        tar_file(archive_out, 'licenses/Steam-Audio-Apache-2.0.txt', license_bytes, 0o644)
    with tarfile.open(archive, 'r:gz') as archive_in:
        for name, data in payload.items():
            member = archive_in.getmember(name)
            if member.size != len(data) or archive_in.extractfile(name).read() != data:
                raise ValueError('archive data verification failed')
            if name == INSTALLED_BINARY and member.mode & 0o111 == 0:
                raise ValueError('archive lost executable permissions')
    print(f'Signed Linux x86-64 {tag}: {archive.name}, native binary, phonon and manifest')


if __name__ == '__main__':
    if sys.argv[1:] == ['--print-tag']:
        print(version_tag())
        sys.exit(0)
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--phonon', required=True, type=Path)
    parser.add_argument('--assets', required=True, type=Path)
    parser.add_argument('--license', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--tag', required=True)
    args = parser.parse_args()
    try:
        create(args.binary, args.phonon, args.assets, args.license, args.out, args.tag)
    except Exception as exc:
        print(f'Linux release packaging failed: {exc}', file=sys.stderr)
        sys.exit(1)
