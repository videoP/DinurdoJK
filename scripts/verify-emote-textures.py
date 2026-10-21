"""Verify a texture rebase against its source and audit report, including raw ZIP preservation."""
import argparse
import io
import json
import struct
import zipfile
from pathlib import Path

from PIL import Image


def compressed_offset(archive, entry):
    archive.fp.seek(entry.header_offset)
    header = archive.fp.read(30)
    if header[:4] != b"PK\x03\x04":
        raise ValueError("invalid local ZIP header")
    name_length, extra_length = struct.unpack_from("<HH", header, 26)
    return entry.header_offset + 30 + name_length + extra_length


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding="utf-8"))
    replacements = report["replacements"]
    rewritten = set(report["updated_reference_members"])
    rows = {r["member"]: r for r in report["textures"]}
    preserved = 0
    checked_jpegs = 0
    with zipfile.ZipFile(args.source) as source, zipfile.ZipFile(args.output) as output:
        expected = (set(source.namelist()) - set(replacements)) | set(replacements.values())
        if set(output.namelist()) != expected or len(output.infolist()) != len(expected):
            raise ValueError("unexpected output members or duplicates")
        for old in source.infolist():
            if old.filename in replacements or old.filename in rewritten:
                continue
            new = output.getinfo(old.filename)
            if (old.CRC, old.file_size, old.compress_size, old.compress_type) != (new.CRC, new.file_size, new.compress_size, new.compress_type):
                raise ValueError(f"unchanged member metadata differs: {old.filename}")
            source.fp.seek(compressed_offset(source, old))
            output.fp.seek(compressed_offset(output, new))
            remaining = old.compress_size
            while remaining:
                amount = min(remaining, 4 * 1024 * 1024)
                if source.fp.read(amount) != output.fp.read(amount):
                    raise ValueError(f"compressed bytes differ: {old.filename}")
                remaining -= amount
            preserved += 1
        for old, new in replacements.items():
            row = rows[old]
            if not row["opaque"] or row["category"] != "color":
                raise ValueError(f"ineligible JPEG: {old}")
            with Image.open(io.BytesIO(source.read(old))) as original, Image.open(io.BytesIO(output.read(new))) as jpeg:
                if jpeg.format != "JPEG" or original.size != jpeg.size:
                    raise ValueError(f"image format/dimensions differ: {old}")
                if original.convert("RGBA").getchannel("A").tobytes() != jpeg.convert("RGBA").getchannel("A").tobytes():
                    raise ValueError(f"alpha differs: {old}")
            checked_jpegs += 1
        referenced = set()
        for entry in output.infolist():
            # Standalone binary model/prop members must not embed obsolete image paths either.
            if Path(entry.filename).suffix.lower() in (".png", ".jpg", ".tga", ".djk_fnemote"):
                continue
            data = output.read(entry)
            for old, new in replacements.items():
                if old.encode() in data:
                    raise ValueError(f"stale PNG reference in {entry.filename}: {old}")
                if new.encode() in data:
                    referenced.add(new)
        if referenced != set(replacements.values()):
            raise ValueError("some JPEGs have no updated reference")
    report["verification"] = {
        "preserved_members_with_identical_compressed_bytes": preserved,
        "jpegs_decoded_dimensions_and_exact_alpha_checked": checked_jpegs,
        "transparent_pngs_preserved_exactly": sum(not r["opaque"] for r in rows.values()),
        "normal_data_effect_pngs_preserved_exactly": sum(r["category"] != "color" for r in rows.values()),
        "obsolete_standalone_image_references": 0,
        "source_bytes": args.source.stat().st_size,
        "output_bytes": args.output.stat().st_size,
        "saved_bytes": args.source.stat().st_size - args.output.stat().st_size,
    }
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report["verification"], indent=2))


if __name__ == "__main__":
    main()
