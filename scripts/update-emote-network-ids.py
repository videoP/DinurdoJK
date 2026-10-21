"""Append permanent EID/variation IDs from loose bundles or stored-bundle PK3s.

The checked-in registry is the authority. Never regenerate/reorder existing rows:
older clients must retain the same meanings even when bundles are missing.
Reads ZIP indexes/manifests in place without extracting or copying asset data.
"""
import argparse
import io
import json
from pathlib import Path
import struct
import zipfile


class Slice(io.RawIOBase):
    def __init__(self, source, start, size):
        self.source, self.start, self.size, self.pos = source, start, size, 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, offset, whence=0):
        self.pos = offset if whence == 0 else self.pos + offset if whence == 1 else self.size + offset
        return self.pos

    def read(self, count=-1):
        self.source.seek(self.start + self.pos)
        data = self.source.read(self.size - self.pos if count < 0 else min(count, self.size - self.pos))
        self.pos += len(data)
        return data


def identities(bundle):
    with zipfile.ZipFile(bundle) as archive:
        for emote in json.loads(archive.read("manifest.json"))["emotes"]:
            for variant in [emote] + emote.get("variants", []):
                key = emote["id"].lower() + "\t" + variant.get("variant_name", "Default").lower()
                if "\n" in key or key.count("\t") != 1:
                    raise ValueError("invalid EID/variation identity")
                yield key


def scan(path):
    if path.is_dir():
        for bundle in sorted(path.glob("*.djk_fnemote")):
            yield from identities(bundle)
    elif path.suffix.lower() == ".pk3":
        with path.open("rb") as source, zipfile.ZipFile(path) as package:
            for entry in package.infolist():
                if not entry.filename.endswith(".djk_fnemote"):
                    continue
                if entry.compress_type != zipfile.ZIP_STORED:
                    raise ValueError("bundles must be stored uncompressed in PK3s")
                source.seek(entry.header_offset + 26)
                name_len, extra_len = struct.unpack("<HH", source.read(4))
                yield from identities(Slice(source, entry.header_offset + 30 + name_len + extra_len, entry.file_size))
    else:
        yield from identities(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources", nargs="+", type=Path)
    args = parser.parse_args()
    registry = Path(__file__).resolve().parents[1] / "crates/jka-client/src/emote_network_ids.tsv"
    old = registry.read_text(encoding="utf-8").splitlines() if registry.exists() else []
    known = {line.split("\t", 1)[1] for line in old if line and not line.startswith("#")}
    ids = [int(line.split("\t", 1)[0]) for line in old if line and not line.startswith("#")]
    additions = sorted(set(key for path in args.sources for key in scan(path)) - known)
    next_id = max(ids, default=-1) + 1
    if next_id + len(additions) > 8191:
        raise ValueError("registry exceeds protocol capacity (8191 is reserved)")
    if not old:
        old = ["# Permanent v2 wire IDs: append only; never reorder, reuse or delete IDs.", "# id<TAB>lowercase source EID<TAB>lowercase source variant_name"]
    old.extend(f"{next_id + i}\t{key}" for i, key in enumerate(additions))
    registry.write_text("\n".join(old) + "\n", encoding="utf-8")
    print(f"Appended {len(additions)} IDs; registry contains {len(known) + len(additions)} performances.")


if __name__ == "__main__":
    main()
