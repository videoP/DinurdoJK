"""Verify a shipping PK3 contains each original/overridden bundle exactly once.

Checks every nested bundle byte without extracting anything, and verifies the
native shared assets against the input PK3. Original authoring data is read only.
"""
import argparse
import collections
import hashlib
import json
import pathlib
import zipfile


def digest(stream):
    result = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        result.update(block)
    return result.hexdigest()


def verify(original, overrides, native, packed, output):
    files = sorted(original.glob("*.djk_fnemote"))
    assert files, "empty original library"
    variants = contact_tracks = one_shots = 0
    with zipfile.ZipFile(packed) as archive, zipfile.ZipFile(native) as assets:
        names = archive.namelist()
        assert len(names) == len(set(n.lower() for n in names)), "duplicate VFS qpaths"
        assert not any(n.lower().endswith("favorites.json") for n in names)
        bundles = [n for n in names if n.lower().endswith(".djk_fnemote")]
        assert set(bundles) == {"emotes/" + f.name for f in files}
        for source in files:
            replacement = overrides / source.name
            actual = replacement if replacement.is_file() else source
            member = "emotes/" + source.name
            info = archive.getinfo(member)
            assert info.compress_type == zipfile.ZIP_STORED, member
            with archive.open(member) as a, actual.open("rb") as b:
                assert digest(a) == digest(b), member
            with archive.open(member) as stream, zipfile.ZipFile(stream) as bundle:
                manifest = json.loads(bundle.read("manifest.json"))
                for row in manifest["emotes"]:
                    for variant in row.get("variants", [row]):
                        variants += 1
                        contact_tracks += bool(variant.get("contact_track"))
                        one_shots += not variant.get("loop", False)
        for info in assets.infolist():
            other = archive.getinfo(info.filename)
            assert (info.CRC, info.file_size, info.compress_type) == (
                other.CRC, other.file_size, other.compress_type
            ), info.filename
        assert archive.testzip() is None, "corrupt outer package"
    result = {
        "bundles": len(files), "variations": variants,
        "contact_tracks": contact_tracks, "one_shots": one_shots,
        "native_members": len(assets.infolist()),
        "all_bundle_bytes_identical_to_compiler_output": True,
        "outer_crc_valid": True, "favorites_excluded": True,
        "bytes": packed.stat().st_size,
    }
    output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for name in ("original", "overrides", "native", "packed", "output"):
        parser.add_argument(name, type=pathlib.Path)
    verify(**vars(parser.parse_args()))
