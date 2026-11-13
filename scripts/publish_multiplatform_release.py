#!/usr/bin/env python3
"""Publish independently signed platform artifacts from one approved workflow.

No private signing credentials are used here. Build jobs behind the protected
release-signing environment produce the signed artifacts this job downloads;
this job re-verifies every manifest's Ed25519 signature with the public key
and every file's size and SHA-256 before anything is uploaded.

Publication is draft-first: a new release is created as a draft, every asset is
uploaded (payload files before each platform's manifest and signature), and
only then is the release published. Clients therefore never see a half-uploaded
release. A rerun reuses the same draft or published release for this commit:
complete platforms are never replaced, while a platform whose upload was
interrupted (some of its assets present, not all) is replaced as a whole so its
manifest always matches its files. Releases from other commits are never touched.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

SHA = re.compile(r"[0-9a-f]{64}")
TAG = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")
SHARED_INSTALL = 'DinurdoJK/shared/dinurdo-assets.pk4'
SHARED_ASSET = 'dinurdo-assets.pk4'
MAX_MANIFEST = 16 * 1024


def run(args: list[str]) -> str:
    return subprocess.check_output(args, text=True).strip()


def version(tag: str) -> tuple[int, int, int] | None:
    match = TAG.fullmatch(tag)
    return tuple(int(part) for part in match.groups()) if match else None


@dataclass
class Platform:
    name: str
    stem: str                       # manifest/signature basename without extension
    # install path in the signed manifest -> GitHub release asset name
    mapping: dict[str, str]
    extra: list[str] = field(default_factory=list)  # unsigned assets (full ZIP/tarball)

    @property
    def manifest(self) -> str:
        return self.stem + '.json'

    @property
    def signature(self) -> str:
        return self.stem + '.sig'

    def payload_assets(self) -> list[str]:
        # The shared PK4 is uploaded once for every platform, not per platform.
        return sorted({a for a in self.mapping.values() if a != SHARED_ASSET} | set(self.extra))

    def all_assets(self) -> list[str]:
        return self.payload_assets() + [self.manifest, self.signature]


def verify_platform(src: Path, platform: Platform, tag: str, shared: Path, key) -> str | None:
    """Return None when the platform is publishable, otherwise the reason."""
    for name in platform.all_assets():
        path = src / name
        if not path.is_file() or path.is_symlink():
            return f'missing {name}'
    raw = (src / platform.manifest).read_bytes()
    sig = (src / platform.signature).read_bytes()
    if len(raw) > MAX_MANIFEST or len(sig) != 64:
        return 'manifest/signature size'
    try:
        key.verify(sig, raw)
    except Exception:  # cryptography raises InvalidSignature
        return 'Ed25519 signature does not verify with the release public key'
    try:
        manifest = json.loads(raw)
    except ValueError:
        return 'manifest is not JSON'
    if manifest.get('format') != 2 or manifest.get('tag') != tag:
        return 'manifest format/tag'
    files = manifest.get('files')
    if not isinstance(files, dict) or set(files) != set(platform.mapping):
        return f'manifest files {sorted(files or {})} != expected {sorted(platform.mapping)}'
    for install_name, info in files.items():
        asset = platform.mapping[install_name]
        path = shared if asset == SHARED_ASSET else src / asset
        if not path.is_file() or path.is_symlink():
            return f'missing {asset}'
        if not isinstance(info, dict) or info.get('size') != path.stat().st_size:
            return f'size mismatch for {install_name}'
        digest = info.get('sha256')
        if not isinstance(digest, str) or not SHA.fullmatch(digest):
            return f'bad digest for {install_name}'
        with path.open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != digest:
                return f'SHA-256 mismatch for {install_name}'
    return None


def windows_platform(src: Path, tag: str) -> Platform | None:
    manifest = src / 'release-manifest.json'
    if not manifest.is_file():
        return None
    try:
        files = json.loads(manifest.read_bytes())['files']
        mapping = {}
        for install_name in files:
            if install_name == SHARED_INSTALL:
                mapping[install_name] = SHARED_ASSET
            elif install_name == 'DinurdoJK.exe':
                mapping[install_name] = install_name
            elif install_name.startswith('DinurdoJK/dependencies/') and install_name.count('/') == 2:
                # make_signed_release.py publishes dependencies by basename.
                mapping[install_name] = install_name.rsplit('/', 1)[1]
            else:
                print(f'::warning::windows: unexpected signed path {install_name!r}', flush=True)
                return None
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as err:
        print(f'::warning::windows: manifest unreadable: {err}', flush=True)
        return None
    return Platform('windows', 'release-manifest', mapping, [f'DinurdoJK-{tag}-full.zip'])


def platforms_for(src: Path, tag: str) -> list[Platform]:
    found = []
    windows = windows_platform(src, tag)
    if windows:
        found.append(windows)
    if (src / 'release-manifest-linux-x86_64.json').is_file():
        found.append(Platform('linux-x86_64', 'release-manifest-linux-x86_64', {
            'DinurdoJK.x86_64': 'DinurdoJK-linux-x86_64',
            'DinurdoJK/dependencies/libphonon.so': 'libphonon-linux-x86_64.so',
            SHARED_INSTALL: SHARED_ASSET,
        }, [f'DinurdoJK-{tag}-linux-x86_64-standalone.tar.gz']))
    for arch in ('arm64', 'x86_64'):
        stem = f'release-manifest-macos-{arch}'
        if (src / f'{stem}.json').is_file():
            found.append(Platform(f'macos-{arch}', stem, {
                'DinurdoJK-macos': f'DinurdoJK-macos-{arch}',
                'DinurdoJK/dependencies/libphonon.dylib': f'libphonon-macos-{arch}.dylib',
                SHARED_INSTALL: SHARED_ASSET,
            }, [f'DinurdoJK-{tag}-macos-{arch}-standalone.zip']))
    return found


def find_release(tag: str) -> dict | None:
    # `gh release view` also resolves draft releases (which have no tag yet).
    proc = subprocess.run(['gh', 'release', 'view', tag, '--json',
                           'tagName,targetCommitish,isDraft,isPrerelease,assets'],
                          capture_output=True, text=True)
    if proc.returncode != 0:
        if 'not found' in (proc.stderr or '').lower():
            return None
        raise RuntimeError(f'could not inspect release {tag}: {proc.stderr.strip()}')
    return json.loads(proc.stdout)


def newest_published() -> tuple[int, int, int] | None:
    releases = json.loads(run(['gh', 'release', 'list', '--limit', '200', '--json',
                               'tagName,isDraft,isPrerelease']))
    versions = [version(r['tagName']) for r in releases
                if not r['isDraft'] and not r['isPrerelease']]
    versions = [v for v in versions if v]
    return max(versions) if versions else None


def upload(tag: str, paths: list[Path], clobber: bool = False):
    if paths:
        run(['gh', 'release', 'upload', tag, *map(str, paths), *(['--clobber'] if clobber else [])])


def publish(tag: str, sha: str, src: Path, shared: Path, platforms: list[Platform]):
    release = find_release(tag)
    newest = newest_published()
    if release is None:
        if newest is not None and version(tag) <= newest:
            raise SystemExit(f'{tag} is not newer than published v{".".join(map(str, newest))}; '
                             'refusing to create an older release')
        if run(['git', 'ls-remote', '--tags', '--refs', 'origin', f'refs/tags/{tag}']):
            raise SystemExit(f'Tag {tag} exists without a Release; refusing to adopt it')
        notes = ('Signed platform builds of ' + sha[:12] + '. Available: '
                 + ', '.join(p.name for p in platforms) + '. Platforms that failed to build '
                 'may be added later by rerunning the workflow for this commit. '
                 'Retail Jedi Academy assets are not included. See packaged notices for licenses.')
        run(['gh', 'release', 'create', tag, '--draft', '--target', sha,
             '--title', f'DinurdoJK {tag}', '--notes', notes])
        release = find_release(tag)
        if release is None:
            raise SystemExit(f'draft release {tag} was not created')
    if release.get('targetCommitish') != sha or release.get('isPrerelease'):
        raise SystemExit(f'Release {tag} exists but does not target {sha}; refusing to modify it')

    present = {a['name']: a for a in release.get('assets', [])}
    shared_size = shared.stat().st_size
    if SHARED_ASSET in present:
        if present[SHARED_ASSET].get('size') != shared_size:
            raise SystemExit(f'{SHARED_ASSET} on {tag} differs from this commit; refusing to mix')
    else:
        upload(tag, [shared])

    added = []
    for platform in platforms:
        names = platform.all_assets()
        existing = [n for n in names if n in present]
        if len(existing) == len(names):
            print(f'{platform.name}: already complete on {tag}; nothing replaced.')
            continue
        if existing:
            print(f'::warning::{platform.name}: interrupted earlier upload '
                  f'({len(existing)}/{len(names)} assets); replacing that platform as a whole.')
        # Payload first, then manifest, then signature last: a platform is only
        # discoverable by clients once its signature exists.
        upload(tag, [src / n for n in platform.payload_assets()], clobber=True)
        upload(tag, [src / platform.manifest], clobber=True)
        upload(tag, [src / platform.signature], clobber=True)
        added.append(platform.name)

    final = find_release(tag)
    names = {a['name'] for a in final.get('assets', [])}
    missing = sorted({n for p in platforms for n in p.all_assets()} - names)
    if missing or SHARED_ASSET not in names:
        raise SystemExit(f'Assets missing after upload: {missing or [SHARED_ASSET]}')

    if final.get('isDraft'):
        args = ['gh', 'release', 'edit', tag, '--draft=false']
        newest = newest_published()
        if newest is None or version(tag) > newest:
            args.append('--latest')
        run(args)
        print(f'Published {tag} with: {", ".join(added)}')
    else:
        print(f'Completed existing release {tag}; added: {", ".join(added) or "(nothing)"}')

    final = find_release(tag)
    if final.get('targetCommitish') != sha or final.get('isDraft'):
        raise SystemExit('Release changed commit or failed to publish')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--tag', required=True)
    ap.add_argument('--commit', required=True)
    ap.add_argument('--input', type=Path, required=True)
    ap.add_argument('--shared-asset', type=Path, required=True)
    ap.add_argument('--public-key-hex', default='')
    args = ap.parse_args()
    if not version(args.tag) or not re.fullmatch('[0-9a-f]{40}', args.commit):
        raise SystemExit('invalid tag or commit SHA')
    if not args.shared_asset.is_file():
        raise SystemExit('missing shared PK4')
    if not args.input.is_dir() or not any(args.input.iterdir()):
        print('::notice::No platform produced signed artifacts; no GitHub Release created.')
        return

    key_hex = args.public_key_hex.strip()
    if not re.fullmatch(r'[0-9a-fA-F]{64}', key_hex):
        raise SystemExit('release public key unavailable from the build jobs; refusing to publish')
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    key = Ed25519PublicKey.from_public_bytes(bytes.fromhex(key_hex))

    selected = []
    for platform in platforms_for(args.input, args.tag):
        problem = verify_platform(args.input, platform, args.tag, args.shared_asset, key)
        if problem:
            print(f'::error::{platform.name}: not published ({problem})', flush=True)
        else:
            selected.append(platform)
    asset_owner: dict[str, str] = {}
    for platform in selected:
        for name in platform.all_assets():
            if name in asset_owner:
                raise SystemExit(f'asset {name} claimed by {asset_owner[name]} and {platform.name}')
            asset_owner[name] = platform.name
    print('Verified signed platforms:', ', '.join(p.name for p in selected) or '(none)')
    if not selected:
        print('::notice::No verified platform; no GitHub Release created.')
        return
    publish(args.tag, args.commit, args.input, args.shared_asset, selected)


if __name__ == '__main__':
    main()
