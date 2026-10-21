//! Find shipped emotes whose baked affine matrices change volume substantially.
use jka_emotes::{Bundle, MAX_MEMBER_BYTES};
use std::collections::HashSet;
use std::path::Path;

fn determinant(m: &jka_assets::ghoul2::Matrix3x4) -> f32 {
    let a = m[0][0];
    let b = m[0][1];
    let c = m[0][2];
    let d = m[1][0];
    let e = m[1][1];
    let f = m[1][2];
    let g = m[2][0];
    let h = m[2][1];
    let i = m[2][2];
    a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
}

fn main() -> Result<(), String> {
    let archive = std::env::args().nth(1).ok_or("expected PK3 path")?;
    let file = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
    let zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let names: Vec<String> = zip
        .file_names()
        .filter(|n| n.ends_with(".djk_fnemote"))
        .map(str::to_owned)
        .collect();
    let mut scores = Vec::new();
    let mut total_matrices = 0usize;
    let mut total_unique = 0usize;
    for (n, name) in names.iter().enumerate() {
        let bundle = Bundle::open_packed_index(Path::new(&archive), name)?;
        let pose = bundle.read("animation.pose", MAX_MEMBER_BYTES)?;
        let clip = jka_emotes::PoseClip::parse(&pose)?;
        let unique: HashSet<[u32; 12]> = clip
            .matrices()
            .iter()
            .map(|m| std::array::from_fn(|i| m[i / 4][i % 4].to_bits()))
            .collect();
        total_matrices += clip.matrices().len();
        total_unique += unique.len();
        let gla = jka_assets::ghoul2::parse_gla(&bundle.read("animation.gla", MAX_MEMBER_BYTES)?)?;
        let mut best = (0.0f32, 0usize, 0usize, 0.0f32, 0.0f32);
        for bone in 0..clip.bones {
            let initial = determinant(&clip.matrices()[bone]).abs();
            if initial < 0.000001 {
                continue;
            }
            for frame in (1..clip.frames).step_by(4) {
                let current = determinant(&clip.matrices()[frame * clip.bones + bone]).abs();
                let score = (current / initial).max(initial / current.max(0.000001));
                if score > best.0 {
                    best = (score, bone, frame, initial, current);
                }
            }
        }
        scores.push((
            best,
            name.clone(),
            bundle.manifest["emotes"][0]["name"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            gla.skeleton[best.1].name.clone(),
        ));
        if n % 250 == 0 {
            eprintln!("scanned {n}/{}", names.len());
        }
    }
    scores.sort_by(|a, b| b.0 .0.total_cmp(&a.0 .0));
    for (score, path, title, bone) in scores.iter().take(25) {
        println!(
            "{:.3} {} {} bone={} frame={} det={:.5}->{:.5}",
            score.0, path, title, bone, score.2, score.3, score.4
        );
    }
    println!(
        "exact matrix pool unique={total_unique} total={total_matrices} ratio={:.3}",
        total_unique as f64 / total_matrices as f64
    );
    Ok(())
}
