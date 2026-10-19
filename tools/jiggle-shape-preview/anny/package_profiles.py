"""Package the four validated pilot bundles; optionally stage in the dev base."""
from pathlib import Path
import argparse
import hashlib
import json
import shutil
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", action="store_true", help="Stage the new PK3 in this workspace's target/release/base")
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    root = here.parents[2]
    output = here / "output"
    manifest = dict(controls=["muscle", "weight", "proportions", "age"],
                    neutral=[0.5]*4, age="child / young adult / elder", format="JKBODY2", models={})
    bundles = {}
    for model in ("jedi_hf", "jedi_hm", "jedi_zf", "temp"):
        report = json.loads((output / f"{model}_report.json").read_text())
        validation = json.loads((output / f"{model}_validation.json").read_text())
        bundle = (output / f"{model}.bodyshape").read_bytes()
        assert hashlib.sha256(bundle).hexdigest() == report["bundle"]["sha256"], model
        assert validation["bundle_sha256"] == report["bundle"]["sha256"], model
        assert bundle[:8] == b"JKBODY2\0", model
        assert len(validation["cases"]) == 148, model
        semantics = validation["semantics"]
        assert semantics["max_tissue_height_change"] < .002, model
        assert semantics["max_tissue_pivot_change"] < 2e-5, model
        assert semantics["min_weight_waist_gain"] > 0, model
        assert semantics["high_weight_muscle_arm_gain"] > 0, model
        assert all(c["collapsed_faces"] == 0 and c["reversed_faces"] == 0 and c["seam_error"] < 2e-5 for c in validation["cases"]), model
        manifest["models"][model] = dict(bytes=len(bundle), sha256=hashlib.sha256(bundle).hexdigest(),
                                        recipes_checked=len(validation["cases"]),
                                        transfer=report["transfer"], semantics=semantics)
        bundles[f"models/players/{model}/model.bodyshape"] = bundle
    import csv
    with (output / "animated_link_checks.csv").open() as stream:
        animation = list(csv.DictReader(stream))
    from profile_preview import fingerprint
    assert len(animation) == 48
    for model in manifest["models"]:
        expected = f"{fingerprint(bundles[f'models/players/{model}/model.bodyshape']):016x}"
        rows = [row for row in animation if row["model"] == model]
        assert len(rows) == 12 and all(row["bundle_fingerprint"] == expected for row in rows), model
    assert all(int(row["frames"]) == 32 and float(row["max_link_error"]) < .001 for row in animation)
    manifest["animation"] = dict(cases=48, frames_per_case=32, clips=["stand", "walk", "crouch"],
                                worst_link_error=max(float(row["max_link_error"]) for row in animation))
    destination = output / "zz_anny_profile_v1.pk3"
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, bundle in bundles.items():
            archive.writestr(name, bundle)
        archive.writestr("anny-profile-v1/manifest.json", json.dumps(manifest, indent=2) + "\n")
        archive.writestr("anny-profile-v1/README.md", (here / "README.md").read_text())
        source = root.parent / "anny-main"
        archive.write(source / "LICENSE", "anny-profile-v1/Anny-LICENSE.txt")
        archive.write(source / "src/anny/data/mpfb2/LICENSE.md", "anny-profile-v1/MakeHuman-LICENSE.md")
    if args.install:
        base = root / "target/release/base"
        if not base.is_dir():
            raise FileNotFoundError(base)
        staging = base / (destination.name + ".tmp")
        shutil.copy2(destination, staging)
        staging.replace(base / destination.name)
    print(json.dumps(dict(package=str(destination), bytes=destination.stat().st_size,
                          installed=args.install, models=list(manifest['models'])), indent=2))


if __name__ == "__main__":
    main()
