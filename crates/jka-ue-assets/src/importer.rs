//! Convert exported UE dependencies to native MD3/weighted-mesh/material PK3s.
//! UE executable behavior stays in the authoring report, never silently runs.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
};

const MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024;
fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit as u64 {
        return Err("UE import source exceeds byte limit".into());
    }
    let mut data = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > limit {
        return Err("UE import source exceeds byte limit".into());
    }
    Ok(data)
}

fn index(v: &Value) -> Result<usize, String> {
    v.as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("invalid mesh index".into())
}
fn id(v: &str) -> Result<&str, String> {
    if v.len() != 24 || !v.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid exported asset identifier".into());
    }
    Ok(v)
}
fn load(p: &Path) -> Result<Value, String> {
    let data = read(p, MAX_MEMBER_BYTES)?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
fn integer(bytes: &mut Vec<u8>, value: usize) -> Result<(), String> {
    bytes.extend(
        i32::try_from(value)
            .map_err(|_| "MD3 offset overflow")?
            .to_le_bytes(),
    );
    Ok(())
}
fn name(bytes: &mut Vec<u8>, value: &str, size: usize) -> Result<(), String> {
    if value.len() >= size || !value.is_ascii() {
        return Err("MD3 name too long".into());
    }
    bytes.extend(value.bytes());
    bytes.resize(bytes.len() + size - value.len(), 0);
    Ok(())
}
fn float(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend(value.to_le_bytes());
}
#[derive(Default)]
struct Surface {
    vertices: Vec<[f32; 8]>,
    triangles: Vec<[usize; 3]>,
    shader: String,
}

pub fn md3(mesh: &Value, scale: f32, namespace: &str) -> Result<Vec<u8>, String> {
    if !scale.is_finite() || scale <= 0. {
        return Err("asset scale must be positive and finite".into());
    }
    let vertices = mesh["vertices"].as_array().ok_or("missing mesh vertices")?;
    if vertices.is_empty() || vertices.len() > 2_000_000 {
        return Err("invalid mesh vertex count".into());
    }
    let vertices = vertices
        .iter()
        .map(|v| {
            let fields = v
                .as_array()
                .filter(|v| v.len() == 8)
                .ok_or("vertex requires position, normal and UV")?;
            let mut out = [0.; 8];
            for (i, field) in fields.iter().enumerate() {
                out[i] = field.as_f64().ok_or("invalid vertex component")? as f32;
                if !out[i].is_finite() {
                    return Err("non-finite mesh vertex".to_string());
                }
            }
            // UE left-handed centimetres -> right-handed game space. Reverse
            // triangle winding below alongside the Y reflection.
            out[0] *= scale;
            out[1] *= -scale;
            out[2] *= scale;
            out[4] *= -1.;
            Ok(out)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let indices = mesh["indices"]
        .as_array()
        .ok_or("missing mesh indices")?
        .iter()
        .map(index)
        .collect::<Result<Vec<_>, _>>()?;
    if indices.len() > 12_000_000 || indices.iter().any(|&i| i >= vertices.len()) {
        return Err("mesh indices outside vertices".into());
    }
    let mut surfaces = Vec::new();
    for section in mesh["sections"].as_array().ok_or("missing mesh sections")? {
        let first = index(&section["first_index"])?;
        let count = index(&section["triangles"])?
            .checked_mul(3)
            .ok_or("triangle count overflow")?;
        let end = first
            .checked_add(count)
            .filter(|&e| e <= indices.len())
            .ok_or("section outside index buffer")?;
        if count == 0 {
            continue;
        }
        let shader = match section["material"].as_str() {
            Some(m) => format!("{namespace}/m_{}", id(m)?),
            None => format!("{namespace}/unsupported"),
        };
        let mut surface = Surface {
            shader: shader.clone(),
            ..Default::default()
        };
        let mut remap = BTreeMap::new();
        for tri in indices[first..end].chunks_exact(3) {
            let new = tri.iter().filter(|i| !remap.contains_key(*i)).count();
            if !surface.triangles.is_empty()
                && (surface.vertices.len() + new > 4096 || surface.triangles.len() >= 8192)
            {
                surfaces.push(surface);
                surface = Surface {
                    shader: shader.clone(),
                    ..Default::default()
                };
                remap.clear();
            }
            let mut local = [0; 3];
            for (k, &i) in tri.iter().enumerate() {
                local[k] = *remap.entry(i).or_insert_with(|| {
                    surface.vertices.push(vertices[i]);
                    surface.vertices.len() - 1
                });
            }
            surface.triangles.push([local[0], local[2], local[1]]);
        }
        if !surface.triangles.is_empty() {
            surfaces.push(surface);
        }
    }
    if surfaces.is_empty() || surfaces.len() > 1024 {
        return Err("invalid MD3 surface count".into());
    }
    let mut mins = [f32::MAX; 3];
    let mut maxs = [f32::MIN; 3];
    let mut encoded = Vec::new();
    for (i, s) in surfaces.iter().enumerate() {
        let tris = 108;
        let shaders = tris + s.triangles.len() * 12;
        let uv = shaders + 68;
        let xyz = uv + s.vertices.len() * 8;
        let end = xyz + s.vertices.len() * 8;
        let mut b = Vec::with_capacity(end);
        b.extend(b"IDP3");
        name(&mut b, &format!("prop_{i}"), 64)?;
        for n in [
            0,
            1,
            1,
            s.vertices.len(),
            s.triangles.len(),
            tris,
            shaders,
            uv,
            xyz,
            end,
        ] {
            integer(&mut b, n)?;
        }
        for tri in &s.triangles {
            for &v in tri {
                integer(&mut b, v)?;
            }
        }
        name(&mut b, &s.shader, 64)?;
        integer(&mut b, 0)?;
        for v in &s.vertices {
            float(&mut b, v[6]);
            float(&mut b, v[7]);
        }
        for v in &s.vertices {
            for k in 0..3 {
                let q = (v[k] * 64.).round();
                if !(-32768. ..=32767.).contains(&q) {
                    return Err(
                        "prop exceeds MD3 coordinate range; choose a smaller asset scale".into(),
                    );
                }
                b.extend((q as i16).to_le_bytes());
                mins[k] = mins[k].min(v[k]);
                maxs[k] = maxs[k].max(v[k]);
            }
            let length = (v[3] * v[3] + v[4] * v[4] + v[5] * v[5]).sqrt();
            let (lat, lng) = if length > 1e-6 {
                (v[4].atan2(v[3]), (v[5] / length).clamp(-1., 1.).acos())
            } else {
                (0., 0.)
            };
            let lat = (lat.rem_euclid(std::f32::consts::TAU) * 255. / std::f32::consts::TAU).round()
                as u16;
            let lng = (lng * 255. / std::f32::consts::TAU).round() as u16;
            b.extend(((lat << 8) | lng).to_le_bytes());
        }
        encoded.extend(b);
    }
    let mut b = Vec::new();
    b.extend(b"IDP3");
    integer(&mut b, 15)?;
    name(&mut b, "ue_prop", 64)?;
    for n in [
        0,
        1,
        0,
        surfaces.len(),
        0,
        108,
        164,
        164,
        164 + encoded.len(),
    ] {
        integer(&mut b, n)?;
    }
    for value in mins.into_iter().chain(maxs).chain([0.; 3]) {
        float(&mut b, value);
    }
    float(
        &mut b,
        maxs.iter()
            .zip(mins)
            .map(|(hi, lo)| hi.abs().max(lo.abs()).powi(2))
            .sum::<f32>()
            .sqrt(),
    );
    name(&mut b, "bind", 16)?;
    b.extend(encoded);
    jka_assets::md3::parse(&b)?;
    Ok(b)
}

fn skin_mesh(raw: &Value, scale: f32, namespace: &str) -> Result<Value, String> {
    let bones = raw["bones"]
        .as_array()
        .ok_or("missing skeletal prop bones")?
        .iter()
        .map(|b| b["Name"].clone())
        .collect::<Vec<_>>();
    let vertices=raw["vertices"].as_array().ok_or("missing skeletal prop vertices")?.iter().map(|v|{
        let mut influences=v["Influences"].as_array().into_iter().flatten().filter_map(|i|Some((i["Bone"].as_u64()? as usize,i["Weight"].as_f64()? as f32))).filter(|&(_,w)|w>0.&&w.is_finite()).collect::<Vec<_>>();influences.sort_by(|a,b|b.1.total_cmp(&a.1));influences.truncate(4);
        let sum=influences.iter().map(|i|i.1).sum::<f32>();let mut refs=[0;4];let mut weights=[0.;4];for (i,(b,w)) in influences.into_iter().enumerate(){refs[i]=b;weights[i]=w/sum;}
        json!({"position":[v["Position"]["X"].as_f64().unwrap_or(0.)*scale as f64,-v["Position"]["Y"].as_f64().unwrap_or(0.)*scale as f64,v["Position"]["Z"].as_f64().unwrap_or(0.)*scale as f64],"normal":[v["Normal"]["X"],-v["Normal"]["Y"].as_f64().unwrap_or(0.),v["Normal"]["Z"]],"uv":[v["Uv"]["U"],v["Uv"]["V"]],"bones":refs,"weights":weights})
    }).collect::<Vec<_>>();
    let mut indices = raw["indices"]
        .as_array()
        .ok_or("missing prop indices")?
        .clone();
    for tri in indices.chunks_exact_mut(3) {
        tri.swap(1, 2);
    }
    let sections=raw["sections"].as_array().ok_or("missing prop sections")?.iter().map(|s|json!({"first":s["first_index"],"end":s["first_index"].as_u64().unwrap_or(0)+s["triangles"].as_u64().unwrap_or(0)*3,"shader":s["material"].as_str().map(|m|format!("{namespace}/m_{m}")).unwrap_or(format!("{namespace}/unsupported"))})).collect::<Vec<_>>();
    let native = json!({"version":1,"bones":bones,"vertices":vertices,"indices":indices,"sections":sections});
    crate::mesh::Mesh::parse(&serde_json::to_vec(&native).unwrap())?;
    Ok(native)
}
/// Settings for [`run`].
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// UE centimetres to game units.
    pub scale: f32,
    /// Quake-path folder every generated model, texture and shader lives in,
    /// for example `models/ue`. Lower-case `a-z 0-9 _ /`, no leading slash.
    /// The last component also names `shaders/<leaf>_props.shader`.
    pub namespace: String,
    /// Extra asset-name prefix removed from display names (after `SM_`/`SK_`).
    pub display_prefix: String,
}

impl ImportOptions {
    pub fn new(scale: f32) -> Self {
        Self {
            scale,
            namespace: "models/ue".into(),
            display_prefix: String::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !self.scale.is_finite() || self.scale <= 0. {
            return Err("asset scale must be positive and finite".into());
        }
        let ns = self.namespace.as_str();
        let valid = !ns.is_empty()
            && !ns.starts_with('/')
            && !ns.ends_with('/')
            && !ns.contains("//")
            && ns
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && ns
                .bytes()
                .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'/'));
        valid.then_some(()).ok_or_else(|| {
            "namespace must be lower-case a-z 0-9 _ / without empty or '..' parts".into()
        })
    }
}

pub fn run(source: &Path, output: &Path, options: &ImportOptions) -> Result<(), String> {
    options.validate()?;
    let (scale, ns) = (options.scale, options.namespace.as_str());
    if output.exists() {
        return Err("asset PK3 output already exists; choose a fresh filename".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let report = load(&source.join("report.json"))?;
    let material_index = if source.join("native-materials-v2.json").is_file() {
        source.join("native-materials-v2.json")
    } else {
        source.join("native-materials.json")
    };
    let materials = load(&if material_index.is_file() {
        material_index
    } else {
        source.join("materials.json")
    })?;
    let labels = if source.join("native-labels.json").is_file() {
        load(&source.join("native-labels.json"))?
    } else {
        Value::Null
    };
    let mut members = BTreeMap::<String, Vec<u8>>::new();
    let mut used = BTreeSet::new();
    let mut models = Vec::new();
    let mut failures = Vec::new();
    for row in report["assets"]
        .as_array()
        .ok_or("asset export report has no assets")?
    {
        if row["status"] != "static_mesh" && row["status"] != "skeletal_authoring_only" {
            continue;
        }
        let key = id(row["id"].as_str().ok_or("asset has no ID")?)?;
        let skeletal = row["status"] == "skeletal_authoring_only";
        let raw = load(&source.join("Meshes").join(if skeletal {
            format!("{key}.skeletal.json")
        } else {
            format!("{key}.json")
        }))?;
        let mesh = if skeletal {
            json!({"asset":raw["asset"],"vertices":raw["vertices"].as_array().ok_or("missing prop vertices")?.iter().map(|v|json!([v["Position"]["X"],v["Position"]["Y"],v["Position"]["Z"],v["Normal"]["X"],v["Normal"]["Y"],v["Normal"]["Z"],v["Uv"]["U"],v["Uv"]["V"]])).collect::<Vec<_>>(),"indices":raw["indices"],"sections":raw["sections"]})
        } else {
            raw.clone()
        };
        match md3(&mesh, scale, ns) {
            Ok(bytes) => {
                for s in mesh["sections"].as_array().unwrap() {
                    if let Some(m) = s["material"].as_str() {
                        used.insert(id(m)?.to_owned());
                    }
                }
                let leaf = mesh["asset"]
                    .as_str()
                    .unwrap_or("prop")
                    .rsplit('.')
                    .next()
                    .unwrap_or("prop");
                let leaf = leaf
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() {
                            c.to_ascii_lowercase()
                        } else {
                            '_'
                        }
                    })
                    .take(24)
                    .collect::<String>();
                let qpath = format!("{ns}/{leaf}_{}.md3", &key[..16]);
                if members.insert(qpath.clone(), bytes).is_some() {
                    return Err("native model filename collision".into());
                }
                let display = mesh["asset"]
                    .as_str()
                    .unwrap_or("prop")
                    .rsplit('.')
                    .next()
                    .unwrap_or("Prop")
                    .trim_start_matches("SM_")
                    .trim_start_matches("SK_")
                    .trim_start_matches(options.display_prefix.as_str())
                    .replace('_', " ");
                let display = labels[mesh["asset"].as_str().unwrap_or("").to_lowercase()]
                    .as_str()
                    .map(|title| format!("{title} · {display}"))
                    .unwrap_or(display);
                let runtime_model = if skeletal {
                    let path = qpath.replace(".md3", ".djk_prop");
                    let native = skin_mesh(&raw, scale, ns)?;
                    members.insert(
                        path.clone(),
                        serde_json::to_vec(&native).map_err(|e| e.to_string())?,
                    );
                    Some(path)
                } else {
                    None
                };
                models.push(json!({"asset":row["asset"],"model":qpath,"runtime_model":runtime_model,"material_ids":mesh["sections"].as_array().unwrap().iter().map(|s|s["material"].clone()).collect::<Vec<_>>(),"display_name":display,"scale":scale,"automatic_emote_attachment":true}));
            }
            Err(error) => failures.push(json!({"asset":row["asset"],"error":error})),
        }
    }
    if models.is_empty() {
        return Err("no static assets could be imported".into());
    }
    // Unresolved graphs get a diagnostic checker, distinct from authored white.
    let mut white = vec![0u8; 18];
    white[2] = 2;
    white[12] = 1;
    white[14] = 1;
    white[16] = 24;
    white.extend([255, 255, 255]);
    members.insert(format!("{ns}/neutral.tga"), white);
    let mut checker = vec![0u8; 18];
    checker[2] = 2;
    checker[12] = 2;
    checker[14] = 2;
    checker[16] = 24;
    checker.extend([180, 0, 180, 40, 40, 40, 40, 40, 40, 180, 0, 180]);
    members.insert(format!("{ns}/unsupported.tga"), checker);
    let mut shaders = format!(
        "{ns}/neutral\n{{\n {{ map {ns}/neutral.tga\n rgbGen lightingDiffuse }}\n}}\n{ns}/unsupported\n{{\n {{ map {ns}/unsupported.tga\n rgbGen lightingDiffuse }}\n}}\n"
    );
    let mut material_rows = Vec::new();
    for key in used {
        let material = &materials[&key];
        let mut texture = if material["solid_color"].as_bool().unwrap_or(false) {
            format!("{ns}/neutral.tga")
        } else {
            format!("{ns}/unsupported.tga")
        };
        let mut status = "unsupported_graph";
        if material["solid_color"].as_bool().unwrap_or(false) {
            status = "authored_constant_color";
        }
        if let Some(diffuse) = material["diffuse"].as_str() {
            let diffuse = id(diffuse)?;
            let path = source.join("Textures").join(format!("{diffuse}.png"));
            if path.is_file() {
                texture = format!("{ns}/t_{diffuse}.png");
                members.insert(texture.clone(), read(&path, MAX_MEMBER_BYTES)?);
                status = "base_color_approximation";
            }
        }
        if let Some(name) = material["native_texture"].as_str() {
            if name.contains('/') || name.contains('\\') || name.contains("..") {
                return Err("invalid native material texture name".into());
            }
            let bytes = read(&source.join("NativeTextures").join(name), MAX_MEMBER_BYTES)?;
            texture = format!("{ns}/{name}");
            members.insert(texture.clone(), bytes);
            status = material["status"]
                .as_str()
                .unwrap_or("native_material_port");
        }
        // Graph-free ports keep their own label in the native report.
        if let Some(port) = material["status"].as_str().filter(|s| {
            matches!(
                *s,
                "role_texture_port" | "authored_color_port" | "neutral_port"
            )
        }) {
            if status != "unsupported_graph" || port != "role_texture_port" {
                status = port;
            }
        }
        if !material["program"].is_null() {
            let bytes = serde_json::to_vec(&material["program"]).map_err(|e| e.to_string())?;
            crate::material::Program::parse(&bytes)?;
            members.insert(format!("{ns}/m_{key}.djk_material"), bytes);
        }
        let blend = material["blend"].as_str().unwrap_or("");
        let state = match blend {
            "BLEND_Masked" => "alphaFunc GE128\n depthWrite",
            "BLEND_Translucent" => "blendFunc blend",
            "BLEND_Additive" => "blendFunc add",
            _ => "",
        };
        let color = material["color"]
            .as_array()
            .filter(|a| a.len() == 3)
            .map(|a| {
                [
                    a[0].as_f64().unwrap_or(1.),
                    a[1].as_f64().unwrap_or(1.),
                    a[2].as_f64().unwrap_or(1.),
                ]
            })
            .unwrap_or([1.; 3]);
        let rgb = if color == [1.; 3] && !material["unlit"].as_bool().unwrap_or(false) {
            "rgbGen lightingDiffuse".into()
        } else {
            format!("rgbGen const ( {} {} {} )", color[0], color[1], color[2])
        };
        let mut normal_statement = String::new();
        if let Some(normal) = material["normal"].as_str() {
            let normal = id(normal)?;
            let path = source.join("Textures").join(format!("{normal}.png"));
            if path.is_file() {
                let image = format!("{ns}/n_{normal}.png");
                members.insert(image.clone(), read(&path, MAX_MEMBER_BYTES)?);
                normal_statement = format!("normalmap {image}\n");
            }
        }
        let opacity = material["opacity"].as_f64().unwrap_or(1.);
        if !opacity.is_finite() || !(0. ..=1.).contains(&opacity) {
            return Err("invalid native material opacity".into());
        }
        let alpha = if opacity < 1. {
            format!("alphaGen const {opacity}\n")
        } else {
            String::new()
        };
        let mut stages =
            format!(" {{ map {texture}\n {normal_statement} {state}\n {alpha}{rgb} }}\n");
        if material["program"]["unlit"] == true {
            stages = format!(" {{ map {texture}\n {state}\n glow\n {alpha}{rgb} }}\n");
            if material["status"] == "native_hologram_port" {
                stages = format!(
                    " {{ map {texture}\n {state}\n glow\n tcMod scroll 0 -0.5\n {alpha}{rgb} }}\n"
                );
            }
        }
        if let Some(emissive) = material["emissive"].as_str() {
            let emissive = id(emissive)?;
            let path = source.join("Textures").join(format!("{emissive}.png"));
            if path.is_file() {
                let image = format!("{ns}/t_{emissive}.png");
                members.insert(image.clone(), read(&path, MAX_MEMBER_BYTES)?);
                let color = &material["emissive_color"];
                stages += &format!(
                    " {{ map {image}\n blendFunc add\n glow\n rgbGen const ( {} {} {} ) }}\n",
                    color[0].as_f64().unwrap_or(1.),
                    color[1].as_f64().unwrap_or(1.),
                    color[2].as_f64().unwrap_or(1.)
                );
            }
        }
        let cull = if material["two_sided"].as_bool().unwrap_or(false) {
            "cull none\n"
        } else {
            ""
        };
        shaders += &format!("{ns}/m_{key}\n{{\n {cull}{stages}}}\n");
        material_rows.push(json!({"id":key,"asset":material["asset"],"display_name":material["asset"].as_str().unwrap_or("Material").rsplit('.').next().unwrap_or("Material").trim_start_matches("MI_").trim_start_matches("M_").replace('_'," "),"status":status,"blend":blend,"texture":texture,"color":color,"normal":material["normal"],"emissive":material["emissive"],"program":!material["program"].is_null(),"unsupported":material["unsupported"]}));
    }
    // JKA's material library indexes shaders/, unlike Q3's scripts/ folder.
    members.insert(
        format!(
            "shaders/{}_props.shader",
            ns.rsplit('/').next().unwrap_or(ns)
        ),
        shaders.into_bytes(),
    );
    let dependencies = load(&source.join("dependencies.json"))?;
    let roots=dependencies["seeds"].as_array().ok_or("dependency index has no roots")?.iter().map(|seed| {
        let exported=report["assets"].as_array().unwrap().iter().find(|r|r["asset"].as_str().zip(seed["asset"].as_str()).is_some_and(|(a,b)|a.eq_ignore_ascii_case(b)));
        let model=models.iter().find(|r|r["asset"].as_str().zip(seed["asset"].as_str()).is_some_and(|(a,b)|a.eq_ignore_ascii_case(b)));
        json!({"asset":seed["asset"],"emotes":seed["emotes"],"export_status":exported.map(|r|&r["status"]),"model":model.map(|r|&r["model"])})
    }).collect::<Vec<_>>();
    let index = json!({"version":2,"source_directory":source.to_string_lossy(),"models":models,"materials":material_rows,"conversion_failures":failures,"emote_dependencies":roots,
        "source_status_counts":report["assets"].as_array().unwrap().iter().fold(BTreeMap::<String,usize>::new(),|mut m,r|{*m.entry(r["status"].as_str().unwrap_or("unknown").into()).or_default()+=1;m}),
        "limitations":["Compile-presentation connects meshes to source notify lifetimes and animated socket tracks", "Native ports cover base texture, constant tint, opacity, emissive passes and normal-map/phong approximation; unsupported material graphs remain explicitly marked", "Generic Niagara VM/GPU simulations and Blueprint bytecode are not executed; per-prop Animation Blueprint and dynamic material behavior need native ports"]});
    members.insert(
        format!("{ns}/index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    );
    let temp = output.with_extension("pk3.tmp");
    let f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(f);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    fs::rename(&temp, output).map_err(|e| e.to_string())?;
    fs::write(
        output.with_extension("report.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Imported {} prop models, {} materials, {} rejected models: {}",
        models.len(),
        material_rows.len(),
        failures.len(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mesh() -> Value {
        json!({"vertices":[[0.,0.,0.,0.,0.,1.,0.,0.],[1.,0.,0.,0.,0.,1.,1.,0.],[0.,1.,0.,0.,0.,1.,0.,1.]],"indices":[0,1,2],"sections":[{"first_index":0,"triangles":1}]})
    }
    #[test]
    fn import_reflects_positions_and_winding_and_preserves_uvs() {
        let parsed = jka_assets::md3::parse(&md3(&mesh(), 1., "models/test").unwrap()).unwrap();
        let s = &parsed.surfaces[0];
        assert_eq!(s.indices, vec![0, 2, 1]);
        assert_eq!(s.vertices[2].position, [0., -1., 0.]);
        assert_eq!(s.vertices[2].uv, [0., 1.]);
        assert!(s.vertices[2].normal[2] > 0.999);
    }
    #[test]
    fn invalid_ranges_and_nonfinite_geometry_are_rejected() {
        let mut v = mesh();
        v["indices"][0] = json!(99);
        assert!(md3(&v, 1., "models/test").is_err());
        let mut v = mesh();
        v["vertices"][0][0] = json!(900.);
        assert!(md3(&v, 1., "models/test").is_err());
        assert!(md3(&mesh(), f32::NAN, "models/test").is_err());
        assert!(md3(&mesh(), 0., "models/test").is_err());
        let mut v = mesh();
        v["sections"][0]["triangles"] = json!(2);
        assert!(md3(&v, 1., "models/test").is_err());
    }
    #[test]
    fn large_sections_split_without_losing_triangles() {
        let vertices = (0..4200)
            .map(|i| json!([i % 20, (i / 20) % 20, 0, 0, 0, 1, 0, 0]))
            .collect::<Vec<_>>();
        let v = json!({"vertices":vertices,"indices":(0..4200).collect::<Vec<_>>(),"sections":[{"first_index":0,"triangles":1400}]});
        let parsed = jka_assets::md3::parse(&md3(&v, 1., "models/test").unwrap()).unwrap();
        assert_eq!(parsed.surfaces.len(), 2);
        assert_eq!(
            parsed
                .surfaces
                .iter()
                .map(|s| s.indices.len())
                .sum::<usize>(),
            4200
        );
        assert!(parsed.surfaces.iter().all(|s| s.vertices.len() <= 4096
            && s.indices.iter().all(|&i| (i as usize) < s.vertices.len())));
    }
}
