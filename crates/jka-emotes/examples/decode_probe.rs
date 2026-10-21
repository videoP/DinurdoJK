//! Compare actual archive loading and every decoded float bit after codec tuning.
use jka_emotes::{Bundle, PoseClip, MAX_MEMBER_BYTES};
use serde_json::json;
use std::{path::Path, time::Instant};
fn exact(a: &PoseClip, b: &PoseClip) -> bool {
    a.frames == b.frames
        && a.bones == b.bones
        && a.matrices()
            .iter()
            .flatten()
            .flatten()
            .zip(b.matrices().iter().flatten().flatten())
            .all(|(a, b)| a.to_bits() == b.to_bits())
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let old = args.get(1).ok_or("old PK3 required")?;
    let new = args.get(2).ok_or("new PK3 required")?;
    let count: usize = args
        .get(3)
        .ok_or("count required")?
        .parse()
        .map_err(|_| "invalid count")?;
    let output = args.get(4).ok_or("report path required")?;
    let archive = zip::ZipArchive::new(std::fs::File::open(old).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let names: Vec<_> = archive
        .file_names()
        .filter(|n| n.ends_with(".djk_fnemote"))
        .map(str::to_owned)
        .collect();
    let selected = count.min(names.len());
    let mut results = Vec::new();
    for i in 0..selected {
        let name = &names[i * names.len() / selected];
        let before = Bundle::open_packed_index(Path::new(old), name)?;
        let after = Bundle::open_packed_index(Path::new(new), name)?;
        let a = before.read("animation.pose", MAX_MEMBER_BYTES)?;
        let b = after.read("animation.pose", MAX_MEMBER_BYTES)?;
        let reference = PoseClip::parse(&a)?;
        let mut old_us = u64::MAX;
        let mut new_us = u64::MAX;
        let mut old_load = u64::MAX;
        let mut new_load = u64::MAX;
        for _ in 0..3 {
            let start = Instant::now();
            let baseline = PoseClip::parse(&a)?;
            old_us = old_us.min(start.elapsed().as_micros() as u64);
            let start = Instant::now();
            let decoded = PoseClip::parse(&b)?;
            new_us = new_us.min(start.elapsed().as_micros() as u64);
            if !exact(&baseline, &decoded) {
                return Err(format!("decoded float bits differ: {name}"));
            }
            let start = Instant::now();
            let bytes = before.read("animation.pose", MAX_MEMBER_BYTES)?;
            let baseline = PoseClip::parse(&bytes)?;
            old_load = old_load.min(start.elapsed().as_micros() as u64);
            let start = Instant::now();
            let bytes = after.read("animation.pose", MAX_MEMBER_BYTES)?;
            let decoded = PoseClip::parse(&bytes)?;
            new_load = new_load.min(start.elapsed().as_micros() as u64);
            if !exact(&baseline, &decoded) || !exact(&reference, &decoded) {
                return Err(format!("loaded float bits differ: {name}"));
            }
        }
        results.push(json!({"member":name,"frames":reference.frames,"old_parse_us":old_us,"new_parse_us":new_us,
            "old_load_us":old_load,"new_load_us":new_load}));
        if results.len() % 100 == 0 {
            println!("Compared {}/{selected}", results.len());
        }
    }
    let summary:serde_json::Map<String,serde_json::Value>=["old_parse_us","new_parse_us","old_load_us","new_load_us"].into_iter().map(|key|{
        let mut values:Vec<_>=results.iter().map(|r|r[key].as_u64().unwrap()).collect();values.sort();
        let value=if values.is_empty() {json!(null)} else {json!({"median":values[values.len()/2],"p95":values[values.len()*95/100],"sum":values.iter().sum::<u64>(),"max":values[values.len()-1]})};
        (key.to_owned(),value)
    }).collect();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    std::fs::write(output,serde_json::to_vec_pretty(&json!({"exact_float_bits":true,"clips":results,"timings_us":summary,"method":"minimum of three warm runs per clip; parse and decompression+parse measured separately"})).unwrap()).map_err(|e|e.to_string())?;
    Ok(())
}
