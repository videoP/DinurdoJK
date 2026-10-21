"""Audit transparency, material roles, duplicates and JPEG candidates in a PK3.

JPEG candidates are decoded back and compared with the original pixels.
Optionally writes a replacement overlay for repack_textures.rs. Requires Pillow.
"""
import argparse
import collections
import hashlib
import io
import json
import math
import re
import time
import zipfile
from pathlib import Path

from PIL import Image, ImageChops, ImageStat


def add_lossless(pk3, report):
    """Measure RGB PNG alternatives while retaining exact decoded RGBA pixels."""
    rows = [r for r in report["textures"] if r["opaque"] and
            r["mode"] in ("RGB", "RGBA") and r["bit_depth"] == 8]
    with zipfile.ZipFile(pk3) as archive:
        for number, row in enumerate(rows, 1):
            with Image.open(io.BytesIO(archive.read(row["member"]))) as image:
                out = io.BytesIO()
                image.convert("RGB").save(out, "PNG", optimize=True, compress_level=9)
                payload = out.getvalue()
                with Image.open(io.BytesIO(payload)) as restored:
                    if restored.convert("RGBA").tobytes() != image.convert("RGBA").tobytes():
                        raise ValueError(f"Lossless RGBA roundtrip mismatch: {row['member']}")
                row["rgb_png"] = {"bytes": len(payload), "saved_bytes": row["archive_bytes"]-len(payload),
                                  "exact_rgba_pixels": True}
            if number % 200 == 0 or number == len(rows):
                print(f"Measured lossless RGB PNG {number}/{len(rows)}", flush=True)
    report["lossless_rgb_png"] = {
        "measured": len(rows),
        "smaller_count": sum(r["rgb_png"]["saved_bytes"] > 0 for r in rows),
        "saved_bytes": sum(max(0, r["rgb_png"]["saved_bytes"]) for r in rows),
        "by_category": {category: sum(max(0, r["rgb_png"]["saved_bytes"]) for r in rows if r["category"] == category)
                        for category in sorted({r["category"] for r in rows})},
        "exact_rgba_pixels": True}
    report["lossless_plus_optional_jpeg_saved_bytes"] = sum(
        max(0, r.get("rgb_png", {}).get("saved_bytes", 0), r.get("jpeg", {}).get("saved_bytes", 0))
        for r in report["textures"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pk3", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--quality", type=int, default=85)
    parser.add_argument("--overlay", type=Path, help="create a fresh ZIP of smaller JPEGs and updated text references")
    parser.add_argument("--add-lossless", action="store_true", help="extend an existing audit with exact RGB PNG size measurements")
    args = parser.parse_args()
    if not 1 <= args.quality <= 100:
        parser.error("quality must be between 1 and 100")
    if args.overlay and (args.overlay.exists() or args.add_lossless):
        parser.error("overlay must be a fresh path and cannot accompany --add-lossless")
    if args.add_lossless:
        report = json.loads(args.report.read_text(encoding="utf-8"))
        if Path(report["source"]).resolve() != args.pk3.resolve():
            parser.error("existing audit is for a different archive")
        add_lossless(args.pk3, report)
        args.report.write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")
        print(json.dumps({k: report[k] for k in ("lossless_rgb_png", "lossless_plus_optional_jpeg_saved_bytes")}, indent=2))
        return
    started = time.monotonic()
    roles = collections.defaultdict(set)
    records = []
    replacements = {}
    updated_references = []
    overlay = zipfile.ZipFile(args.overlay, "x", compression=zipfile.ZIP_STORED) if args.overlay else None
    duplicate_groups = collections.defaultdict(list)
    with zipfile.ZipFile(args.pk3) as archive:
        for entry in archive.infolist():
            if not entry.filename.lower().endswith(".shader"):
                continue
            text = archive.read(entry).decode("utf-8", errors="replace")
            for line in text.splitlines():
                line = line.split("//", 1)[0]
                for match in re.finditer(r"([\w/$.-]+\.png)\b", line, re.I):
                    path = match.group(1).lower()
                    prefix = line[:match.start()].lower()
                    role = "normal" if "normalmap" in prefix else (
                        "data" if any(x in prefix for x in ("specularmap", "roughnessmap", "heightmap", "metallicmap")) else "color")
                    roles[path].add(role)
        pngs = [i for i in archive.infolist() if i.filename.lower().endswith(".png")]
        for number, entry in enumerate(pngs, 1):
            data = archive.read(entry)
            with Image.open(io.BytesIO(data)) as image:
                original_mode = image.mode
                rgba = image.convert("RGBA")
                alpha = rgba.getchannel("A")
                low, high = alpha.getextrema()
                category = "normal" if "normal" in roles[entry.filename.lower()] or Path(entry.filename).name.startswith("n_") else (
                    "data" if "data" in roles[entry.filename.lower()] else (
                        "effect" if "/fx/" in entry.filename.lower() else "color"))
                record = {"member": entry.filename, "width": image.width, "height": image.height,
                          "mode": original_mode, "png_color_type": data[25], "bit_depth": data[24],
                          "png_bytes": len(data), "archive_bytes": entry.compress_size,
                          "alpha_min": low, "alpha_max": high, "opaque": low == 255,
                          "alpha_channel_present": "A" in image.getbands() or "transparency" in image.info,
                          "category": category, "shader_roles": sorted(roles[entry.filename.lower()])}
                digest = hashlib.sha256(rgba.tobytes()).hexdigest()
                duplicate_groups[(image.size, digest)].append(entry.filename)
                if low == 255 and category == "color" and original_mode in ("RGB", "RGBA") and data[24] == 8:
                    rgb = rgba.convert("RGB")
                    output = io.BytesIO()
                    rgb.save(output, "JPEG", quality=args.quality, subsampling=0, optimize=True)
                    jpeg = output.getvalue()
                    with Image.open(io.BytesIO(jpeg)) as restored:
                        if restored.size != rgba.size or restored.convert("RGBA").getchannel("A").tobytes() != alpha.tobytes():
                            raise ValueError(f"JPEG alpha or dimensions changed: {entry.filename}")
                        difference = ImageChops.difference(rgb, restored.convert("RGB"))
                        stats = ImageStat.Stat(difference)
                        mse = sum(x*x for x in stats.rms)/3
                        record["jpeg"] = {"bytes": len(jpeg), "saved_bytes": entry.compress_size-len(jpeg),
                                          "quality": args.quality, "subsampling": "4:4:4",
                                          "psnr_db": 10*math.log10(255*255/mse) if mse else None,
                                          "max_channel_error": max(hi for _, hi in difference.getextrema()),
                                          "mean_absolute_channel_error": sum(stats.mean)/3}
                    if overlay and len(jpeg) < entry.compress_size:
                        name = str(Path(entry.filename).with_suffix("")).replace("\\", "/") + f"_q{args.quality}.jpg"
                        if name in archive.namelist():
                            raise ValueError(f"Replacement already exists: {name}")
                        replacements[entry.filename] = name
                        overlay.writestr(name, jpeg)
                records.append(record)
            if number % 100 == 0 or number == len(pngs):
                print(f"Audited {number}/{len(pngs)} PNGs", flush=True)
        if overlay:
            pattern = re.compile("|".join(re.escape(n) for n in sorted(replacements, key=len, reverse=True))) if replacements else None
            referenced = set()
            for entry in archive.infolist():
                if Path(entry.filename).suffix.lower() not in (".shader", ".json", ".djk_material", ".md"):
                    continue
                before = archive.read(entry)
                text = before.decode("utf-8")
                def replace(match):
                    referenced.add(match.group(0))
                    return replacements[match.group(0)]
                after = pattern.sub(replace, text).encode("utf-8") if pattern else before
                if before != after:
                    overlay.writestr(entry.filename, after, compress_type=zipfile.ZIP_DEFLATED)
                    updated_references.append(entry.filename)
            if referenced != set(replacements):
                raise ValueError(f"Unreferenced replacements: {set(replacements)-referenced}")
            overlay.close()
    summary = {}
    for category in sorted({r["category"] for r in records}):
        rows = [r for r in records if r["category"] == category]
        opaque = [r for r in rows if r["opaque"]]
        summary[category] = {"count": len(rows), "archive_bytes": sum(r["archive_bytes"] for r in rows),
                             "opaque_count": len(opaque), "opaque_archive_bytes": sum(r["archive_bytes"] for r in opaque),
                             "has_transparency_count": len(rows)-len(opaque)}
    candidates = [r for r in records if "jpeg" in r and r["jpeg"]["saved_bytes"] > 0]
    groups = [names for names in duplicate_groups.values() if len(names) > 1]
    by_name = {r["member"]: r for r in records}
    duplicates_saved = sum(sum(by_name[n]["archive_bytes"] for n in names)-min(by_name[n]["archive_bytes"] for n in names) for names in groups)
    report = {"source": str(args.pk3), "png_count": len(records),
              "png_archive_bytes": sum(r["archive_bytes"] for r in records),
              "opaque_count": sum(r["opaque"] for r in records),
              "opaque_with_alpha_channel_count": sum(r["opaque"] and r["alpha_channel_present"] for r in records),
              "categories": summary, "jpeg_candidates_that_save_space": len(candidates),
              "candidate_png_archive_bytes": sum(r["archive_bytes"] for r in candidates),
              "candidate_jpeg_bytes": sum(r["jpeg"]["bytes"] for r in candidates),
              "candidate_saved_bytes": sum(r["jpeg"]["saved_bytes"] for r in candidates),
              "jpeg_settings": {"quality": args.quality, "subsampling": "4:4:4", "optimize": True},
              "duplicate_pixel_groups": groups, "duplicate_pixel_potential_saved_bytes": duplicates_saved,
              "elapsed_seconds": time.monotonic()-started, "textures": records,
              "replacements": replacements, "updated_reference_members": updated_references,
              "notes": ["JPEG is lossy even at quality 100; PSNR alone does not prove visual equivalence.",
                        "Normal/data/effect maps and every non-opaque PNG excluded from JPEG candidates.",
                        "Converted JPEGs restore exactly the original all-255 alpha pixels; RGB is lossy.",
                        "JPEG byte estimates use ZIP Store, as JPEG data is already compressed.",
                        "Duplicate savings require updating all references; they overlap other proposed savings."]}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")
    print(json.dumps({k: v for k, v in report.items() if k not in ("textures", "duplicate_pixel_groups", "replacements")}, indent=2))


if __name__ == "__main__":
    main()
