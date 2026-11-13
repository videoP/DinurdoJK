#!/usr/bin/env python3
"""Package a standalone macOS executable with a signed updater manifest.

The signed v2 JSON uses install paths, while release assets use unique names.
The existing Windows manifest and its assets are never modified. The shared
PK4 is included in the release payload and verified by the common publisher.
This packager does not require Developer ID signing or Apple notarization.
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
import tomllib
import zipfile

MAX_FILE = 1024 * 1024 * 1024
MAX_MANIFEST = 16 * 1024
# Must match updater.rs GAME_BINARY for macOS. The client data folder
# DinurdoJK/ sits beside the executable, and macOS volumes are usually
# case-insensitive, so the executable cannot be named DinurdoJK.
INSTALLED_BINARY = 'DinurdoJK-macos'
PHONON = 'DinurdoJK/dependencies/libphonon.dylib'


def version_tag() -> str:
    with Path('crates/jka-client/Cargo.toml').open('rb') as fp:
        package = tomllib.load(fp)['package']
    assert package['name'] == 'DinurdoJK'
    version = package['version']
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError('invalid Cargo version')
    return 'v' + version


def read_safe(path: Path) -> bytes:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f'missing or symlinked release file: {path}')
    data = path.read_bytes()
    if not 0 < len(data) <= MAX_FILE:
        raise ValueError(f'empty or oversized release file: {path}')
    return data


def key_pair():
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
    key = base64.b64decode(os.environ['DINURDOJK_UPDATE_PRIVATE_KEY_B64'].strip(), validate=True)
    pub = bytes.fromhex(os.environ['DINURDOJK_UPDATE_PUBLIC_KEY_HEX'].strip())
    if len(key) != 32 or len(pub) != 32:
        raise ValueError('Ed25519 seed/public key must each be 32 bytes')
    signer = Ed25519PrivateKey.from_private_bytes(key)
    verifier = Ed25519PublicKey.from_public_bytes(pub)
    proof = b'DinurdoJK macOS release signing key check'
    verifier.verify(signer.sign(proof), proof)
    return signer, verifier


def zip_bytes(archive: zipfile.ZipFile, name: str, data: bytes, mode: int):
    item = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
    item.create_system = 3
    item.external_attr = (stat.S_IFREG | mode) << 16
    item.compress_type = zipfile.ZIP_DEFLATED
    archive.writestr(item, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=6)


def create(binary: Path, phonon: Path, asset: Path, output: Path, tag: str, arch: str):
    if tag != version_tag():
        raise ValueError(f'{tag} does not match Cargo version {version_tag()}')
    if arch not in ('arm64', 'x86_64'):
        raise ValueError('invalid macOS architecture')
    if binary.name != 'DinurdoJK' or binary.stat().st_mode & 0o111 == 0:
        raise ValueError('binary must be named DinurdoJK and executable')
    if phonon.name != 'libphonon.dylib':
        raise ValueError('Steam Audio dependency must be named libphonon.dylib')
    signer, verifier = key_pair()
    payload = {
        INSTALLED_BINARY: read_safe(binary),
        PHONON: read_safe(phonon),
        'DinurdoJK/shared/dinurdo-assets.pk4': read_safe(asset),
    }
    # Must be protected in the signed manifest even though the shared PK4
    # upload already belongs to the Windows publisher.
    metadata = {
        'format': 2,
        'tag': tag,
        'files': {name: {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                  for name, data in sorted(payload.items())},
    }
    raw = json.dumps(metadata, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()
    if len(raw) > MAX_MANIFEST:
        raise ValueError('manifest too large')
    sig = signer.sign(raw)
    verifier.verify(sig, raw)
    if output.exists() and any(output.iterdir()):
        raise ValueError('refusing to overwrite macOS release output')
    output.mkdir(parents=True, exist_ok=True)
    bin_name = f'DinurdoJK-macos-{arch}'
    (output / bin_name).write_bytes(payload[INSTALLED_BINARY])
    os.chmod(output / bin_name, 0o755)
    (output / f'libphonon-macos-{arch}.dylib').write_bytes(payload[PHONON])
    (output / f'release-manifest-macos-{arch}.json').write_bytes(raw)
    (output / f'release-manifest-macos-{arch}.sig').write_bytes(sig)
    archive = output / f'DinurdoJK-{tag}-macos-{arch}-standalone.zip'
    launch = b'#!/bin/sh\ncd "$(dirname "$0")" || exit 1\nexec ./DinurdoJK-macos "$@"\n'
    with zipfile.ZipFile(archive, 'w', allowZip64=False) as zipfile_out:
        zip_bytes(zipfile_out, INSTALLED_BINARY, payload[INSTALLED_BINARY], 0o755)
        zip_bytes(zipfile_out, PHONON, payload[PHONON], 0o755)
        zip_bytes(zipfile_out, 'DinurdoJK/shared/dinurdo-assets.pk4', payload['DinurdoJK/shared/dinurdo-assets.pk4'], 0o644)
        zip_bytes(zipfile_out, 'Launch-DinurdoJK.command', launch, 0o755)
    with zipfile.ZipFile(archive) as z:
        assert z.read(INSTALLED_BINARY) == payload[INSTALLED_BINARY]
        assert z.read(PHONON) == payload[PHONON]
        assert z.read('DinurdoJK/shared/dinurdo-assets.pk4') == payload['DinurdoJK/shared/dinurdo-assets.pk4']
        assert z.getinfo(INSTALLED_BINARY).external_attr >> 16 & 0o111
    print(f'Signed {arch} {tag}: {archive.name}, {bin_name}, release manifest and raw signature')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--phonon', required=True, type=Path)
    parser.add_argument('--assets', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--arch', required=True, choices=['arm64', 'x86_64'])
    args = parser.parse_args()
    try:
        create(args.binary, args.phonon, args.assets, args.out, args.tag, args.arch)
    except Exception as exc:
        print(f'Mac release signing failed: {exc}', file=sys.stderr)
        sys.exit(1)
