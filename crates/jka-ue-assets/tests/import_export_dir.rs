//! End-to-end import of a tiny synthetic export directory: the layout here is
//! the documented input contract (see README.md, "Export directory layout").
use jka_ue_assets::importer::{run, ImportOptions};
use serde_json::{json, Value};
use std::{fs, io::Read, path::PathBuf};

const MESH_ID: &str = "00112233445566778899aabb";
const MATERIAL_ID: &str = "ffeeddccbbaa998877665544";
const TEXTURE_ID: &str = "0123456789abcdef01234567";

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("djk-ue-assets-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_json(path: PathBuf, value: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn export_dir(scratch: &Scratch) -> PathBuf {
    let dir = scratch.0.join("export");
    write_json(
        dir.join("report.json"),
        json!({"assets": [{"asset": "/Game/Props/SM_Chair.SM_Chair", "id": MESH_ID, "status": "static_mesh"}]}),
    );
    write_json(
        dir.join("Meshes").join(format!("{MESH_ID}.json")),
        json!({
            "asset": "/Game/Props/SM_Chair.SM_Chair",
            "vertices": [[0, 0, 0, 0, 0, 1, 0, 0], [10, 0, 0, 0, 0, 1, 1, 0], [0, 10, 0, 0, 0, 1, 0, 1]],
            "indices": [0, 1, 2],
            "sections": [{"first_index": 0, "triangles": 1, "material": MATERIAL_ID}],
        }),
    );
    write_json(
        dir.join("materials.json"),
        json!({MATERIAL_ID: {"asset": "/Game/Props/M_Wood.M_Wood", "diffuse": TEXTURE_ID, "blend": "BLEND_Opaque"}}),
    );
    write_json(
        dir.join("dependencies.json"),
        json!({"seeds": [{"asset": "/Game/Props/SM_Chair.SM_Chair"}]}),
    );
    fs::create_dir_all(dir.join("Textures")).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 150, 90, 255]))
        .save(dir.join("Textures").join(format!("{TEXTURE_ID}.png")))
        .unwrap();
    dir
}

fn members(pk3: &PathBuf) -> Vec<String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(pk3).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_owned())
        .collect()
}

#[test]
fn imports_static_mesh_with_texture_under_the_requested_namespace() {
    let scratch = Scratch::new("import");
    let export = export_dir(&scratch);
    let pk3 = scratch.0.join("out.pk3");
    let options = ImportOptions {
        namespace: "models/mygame".into(),
        ..ImportOptions::new(0.5)
    };
    run(&export, &pk3, &options).unwrap();

    let names = members(&pk3);
    for expected in [
        "models/mygame/index.json",
        "models/mygame/neutral.tga",
        "models/mygame/unsupported.tga",
        &format!("models/mygame/t_{TEXTURE_ID}.png"),
        "shaders/mygame_props.shader",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {expected} in {names:?}"
        );
    }
    let model = names
        .iter()
        .find(|n| n.ends_with(".md3"))
        .expect("an MD3 model");
    assert!(
        model.starts_with("models/mygame/sm_chair_") || model.starts_with("models/mygame/chair_"),
        "{model}"
    );
    assert!(
        names.iter().all(|n| !n.contains("fnemote")),
        "nothing may leak the old Fortnite namespace: {names:?}"
    );

    let mut shader = String::new();
    zip::ZipArchive::new(fs::File::open(&pk3).unwrap())
        .unwrap()
        .by_name("shaders/mygame_props.shader")
        .unwrap()
        .read_to_string(&mut shader)
        .unwrap();
    assert!(shader.contains(&format!("models/mygame/m_{MATERIAL_ID}")));

    let report: Value =
        serde_json::from_slice(&fs::read(pk3.with_extension("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["models"][0]["asset"],
        "/Game/Props/SM_Chair.SM_Chair"
    );
    assert_eq!(report["materials"][0]["status"], "base_color_approximation");
}

#[test]
fn refuses_to_overwrite_and_rejects_unsafe_namespaces() {
    let scratch = Scratch::new("reject");
    let export = export_dir(&scratch);
    let pk3 = scratch.0.join("out.pk3");
    fs::write(&pk3, b"existing").unwrap();
    assert!(run(&export, &pk3, &ImportOptions::new(0.4))
        .unwrap_err()
        .contains("already exists"));
    fs::remove_file(&pk3).unwrap();
    for bad in [
        "",
        "/models/x",
        "models/../x",
        "models//x",
        "Models/X",
        "models/x/",
    ] {
        let options = ImportOptions {
            namespace: bad.into(),
            ..ImportOptions::new(0.4)
        };
        assert!(
            run(&export, &pk3, &options).is_err(),
            "namespace {bad:?} should be rejected"
        );
    }
    assert!(!pk3.exists());
}
