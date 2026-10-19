//! Classify exported textures by content so material ports never use a
//! normal map or channel-packed mask as visible colour.
use serde_json::{json, Map, Value};
use std::path::Path;

/// Content class of one RGBA image.
pub fn classify(img: &image::RgbaImage) -> Value {
    let (w, h) = img.dimensions();
    let step = ((w as u64 * h as u64) / 65536).max(1) as usize;
    let (mut n, mut normal, mut gray, mut alpha) = (0f64, 0f64, 0f64, 0f64);
    let mut sum = [0f64; 4];
    let mut sq = [0f64; 3];
    // Hue histogram (12 bins) of saturated, visible pixels: several distant
    // hue clusters mean channels hold separate masks, not a coloured image.
    let mut hues = [0f64; 12];
    let (mut signal, mut saturated, mut quantized) = (0f64, 0f64, 0f64);
    for p in img.pixels().step_by(step) {
        let (r, g, b) = (p[0] as f64 / 255., p[1] as f64 / 255., p[2] as f64 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max > 0.3 && p[3] > 32 {
            signal += 1.;
            // ID/region masks use channels that are either off or full on.
            if [r, g, b].iter().all(|c| *c < 0.15 * max || *c > 0.85 * max) {
                quantized += 1.;
            }
            if (max - min) / max > 0.6 {
                saturated += 1.;
                let d = max - min;
                let h = if max == r {
                    ((g - b) / d).rem_euclid(6.)
                } else if max == g {
                    (b - r) / d + 2.
                } else {
                    (r - g) / d + 4.
                };
                hues[((h / 6. * 12.) as usize).min(11)] += 1.;
            }
        }
    }
    let multimask = saturated > 0.3 * signal.max(1.) && {
        let share = hues.map(|v| v / saturated.max(1.));
        (0..12).any(|a| {
            share[a] >= 0.12
                && (0..12).any(|b| {
                    let d = (a as i32 - b as i32)
                        .rem_euclid(12)
                        .min((b as i32 - a as i32).rem_euclid(12));
                    d >= 3 && share[b] >= 0.12
                })
        })
    };
    let pure = quantized / signal.max(1.);
    for p in img.pixels().step_by(step) {
        let c = [
            p[0] as f64 / 255.,
            p[1] as f64 / 255.,
            p[2] as f64 / 255.,
            p[3] as f64 / 255.,
        ];
        n += 1.;
        for i in 0..4 {
            sum[i] += c[i];
        }
        for i in 0..3 {
            sq[i] += c[i] * c[i];
        }
        // Tangent-space normal: unit length after 2x-1, Z (blue) positive and dominant.
        let v = [c[0] * 2. - 1., c[1] * 2. - 1., c[2] * 2. - 1.];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if (len - 1.).abs() < 0.2 && v[2] > 0.35 {
            normal += 1.;
        }
        if (c[0] - c[1]).abs() < 0.04 && (c[1] - c[2]).abs() < 0.04 {
            gray += 1.;
        }
        if c[3] < 0.98 {
            alpha += 1.;
        }
    }
    let n = n.max(1.);
    let mean = sum.map(|s| s / n);
    let spread: Vec<f64> = (0..3)
        .map(|i| (sq[i] / n - mean[i] * mean[i]).max(0.).sqrt())
        .collect();
    let (normal, gray, alpha) = (normal / n, gray / n, alpha / n);
    // Packed masks: channels carry unrelated shapes, so their means/variances diverge
    // and few pixels are gray, yet no hue is consistent.
    let class = if normal > 0.85 && mean[2] > 0.7 {
        "normal"
    } else if gray > 0.9 {
        "grayscale"
    } else if spread.iter().all(|s| *s > 0.12)
        && gray < 0.2
        && (mean[0] - mean[1]).abs() + (mean[1] - mean[2]).abs() > 0.25
    {
        "packed"
    } else if multimask {
        "multimask"
    } else {
        "color"
    };
    // Mean signal on the outer 6% ring, per channel (R,G,B,A,luminance*alpha).
    // Sprite shapes fade out at their edges; tiling noise/erosion maps do not.
    let ring = ((w.min(h) as f32) * 0.06).ceil().max(1.) as u32;
    let mut edge = [0f64; 5];
    let mut edges = 0f64;
    let step = (ring as usize).max(1);
    for y in (0..h).step_by(step.min(h as usize).max(1)) {
        for x in (0..w).step_by(step.min(w as usize).max(1)) {
            if x >= ring && y >= ring && x + ring < w && y + ring < h {
                continue;
            }
            let p = img.get_pixel(x, y);
            let c = [
                p[0] as f64 / 255.,
                p[1] as f64 / 255.,
                p[2] as f64 / 255.,
                p[3] as f64 / 255.,
            ];
            for i in 0..4 {
                edge[i] += c[i];
            }
            edge[4] += c[0].max(c[1]).max(c[2]) * c[3];
            edges += 1.;
        }
    }
    let edge = edge.map(|e| e / edges.max(1.));
    json!({"class":class,"normal":normal,"gray":gray,"alpha":alpha,"mean":mean,"spread":spread,"edge":edge,"pure":pure,"size":[w,h]})
}

/// `texture-audit <Textures dir> <out.json>`: classify every PNG.
pub fn run(directory: &Path, output: &Path) -> Result<(), String> {
    let mut files = std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect::<Vec<_>>();
    files.sort();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Map::new());
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(16)
        {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(file) = files.get(i) else { break };
                let id = file.file_stem().unwrap().to_string_lossy().into_owned();
                let row = match image::open(file) {
                    Ok(img) => classify(&img.to_rgba8()),
                    Err(e) => json!({"class":"unreadable","error":e.to_string()}),
                };
                out.lock().unwrap().insert(id, row);
            });
        }
    });
    let out = out.into_inner().unwrap();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for v in out.values() {
        *counts
            .entry(v["class"].as_str().unwrap_or("").to_owned())
            .or_default() += 1;
    }
    println!("{}", serde_json::to_string(&counts).unwrap());
    std::fs::write(output, serde_json::to_vec(&Value::Object(out)).unwrap())
        .map_err(|e| e.to_string())
}
