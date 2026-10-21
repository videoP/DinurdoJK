//! Rebase an installed library directly into a fresh PK3 without staging copies.
use crate::{Bundle, BundleSource, PoseClip, MAX_BUNDLE_BYTES, MAX_MEMBER_BYTES};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::Path,
    sync::Arc,
    time::Instant,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug)]
struct Memory(Arc<[u8]>);
impl BundleSource for Memory {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn open(&self) -> io::Result<Box<dyn crate::source::ReadSeek>> {
        Ok(Box::new(Cursor::new(self.0.clone())))
    }
    fn key(&self) -> u64 {
        0
    }
    fn describe(&self) -> String {
        "pose pool verification buffer".into()
    }
}

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

fn rewrite(bytes: Arc<[u8]>) -> Result<(Vec<u8>, Value), String> {
    let bundle = Bundle::from_source(Arc::new(Memory(bytes.clone())), false)?;
    let pose_name = bundle.manifest["emotes"][0]["pose"]
        .as_str()
        .ok_or("bundle lacks body pose")?;
    let gla_name = bundle.manifest["emotes"][0]["gla"]
        .as_str()
        .ok_or("bundle lacks skeleton")?;
    let start = Instant::now();
    let original = bundle.read(pose_name, MAX_MEMBER_BYTES)?;
    let clip = PoseClip::parse(&original)?;
    let old_load_us = start.elapsed().as_micros() as u64;
    let gla = jka_assets::ghoul2::parse_gla(&bundle.read(gla_name, MAX_MEMBER_BYTES)?)?;
    let mut source = ZipArchive::new(Cursor::new(bytes.clone())).map_err(|e| e.to_string())?;
    let old_size = source
        .by_name(pose_name)
        .map_err(|e| e.to_string())?
        .compressed_size();
    let start = Instant::now();
    let (encoded, stats) = match crate::pose::pooled::encode(&clip, &gla) {
        Ok(value) => value,
        Err(error)
            if error == "pooled pose input is not on the exact shipping grid"
                || error == "pooled pose member exceeds size limit" =>
        {
            return Ok((
                bytes.to_vec(),
                json!({"frames":clip.frames,"bones":clip.bones,"selected":false,
                "fallback":error,"pose_before":old_size,"pose_after":old_size,"old_load_us":old_load_us}),
            ));
        }
        Err(error) => return Err(error),
    };
    let encode_us = start.elapsed().as_micros() as u64;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..source.len() {
        let entry = source.by_index(i).map_err(|e| e.to_string())?;
        if entry.name() == pose_name {
            writer
                .start_file(
                    pose_name,
                    SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Zstd)
                        .compression_level(Some(15)),
                )
                .map_err(|e| e.to_string())?;
            writer.write_all(&encoded).map_err(|e| e.to_string())?;
        } else {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
        }
    }
    let candidate = writer.finish().map_err(|e| e.to_string())?.into_inner();
    let mut check = ZipArchive::new(Cursor::new(&candidate)).map_err(|e| e.to_string())?;
    let new_size = check
        .by_name(pose_name)
        .map_err(|e| e.to_string())?
        .compressed_size();
    let start = Instant::now();
    let mut roundtrip = Vec::new();
    check
        .by_name(pose_name)
        .map_err(|e| e.to_string())?
        .read_to_end(&mut roundtrip)
        .map_err(|e| e.to_string())?;
    let restored = PoseClip::parse(&roundtrip)?;
    let new_load_us = start.elapsed().as_micros() as u64;
    if !exact(&clip, &restored) {
        return Err("pooled pose is not bit exact after ZIP roundtrip".into());
    }
    for i in 0..source.len() {
        let old = source.by_index(i).map_err(|e| e.to_string())?;
        if old.name() == pose_name {
            continue;
        }
        let new = check.by_name(old.name()).map_err(|e| e.to_string())?;
        if old.crc32() != new.crc32()
            || old.size() != new.size()
            || old.compressed_size() != new.compressed_size()
        {
            return Err(format!("non-pose member changed: {}", old.name()));
        }
    }
    let selected = candidate.len() < bytes.len();
    let record = json!({"frames":clip.frames,"bones":clip.bones,"selected":selected,
        "pose_before":old_size,"pose_after":if selected {new_size} else {old_size},
        "candidate_pose_bytes":new_size,"exact_float_bits":true,"samples":stats.samples,
        "pool_transforms":stats.transforms,"absolute_predictions":stats.absolute_predictions,
        "residual_bytes":stats.residual_bytes,"old_load_us":old_load_us,"new_load_us":new_load_us,
        "encode_us":encode_us,"decoded_matrix_bytes":clip.frames*clip.bones*48});
    if selected {
        // Exercise the full reader, including ranges, skeleton and audio validation.
        let loaded = Bundle::from_source(Arc::new(Memory(Arc::from(candidate.clone()))), true)?;
        if !exact(
            &clip,
            loaded.pose.as_ref().ok_or("validated bundle lacks pose")?,
        ) {
            return Err("playback reader changed pooled pose".into());
        }
        Ok((candidate, record))
    } else {
        Ok((bytes.to_vec(), record))
    }
}

pub fn run(input: &Path, output: &Path) -> Result<(), String> {
    let temporary = output.with_extension("pk3.partial");
    let report_path = output.with_extension("report.json");
    if output.exists() || temporary.exists() || report_path.exists() {
        return Err("pose pool output already exists".into());
    }
    let metadata = fs::metadata(input).map_err(|e| e.to_string())?;
    let mut source = ZipArchive::new(File::open(input).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let mut writer = ZipWriter::new(file);
    let start = Instant::now();
    let mut records = Vec::new();
    let mut selected = 0;
    let mut pose_before = 0u64;
    let mut pose_after = 0u64;
    for i in 0..source.len() {
        let mut entry = source.by_index(i).map_err(|e| e.to_string())?;
        if !entry.name().ends_with(".djk_fnemote") {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
            continue;
        }
        if entry.compression() != CompressionMethod::Stored || entry.size() > MAX_BUNDLE_BYTES {
            return Err("outer bundle must be stored and bounded".into());
        }
        let name = entry.name().to_owned();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let (bytes, mut record) = rewrite(Arc::from(bytes)).map_err(|e| format!("{name}: {e}"))?;
        record["member"] = json!(name);
        selected += usize::from(record["selected"] == true);
        pose_before += record["pose_before"].as_u64().unwrap();
        pose_after += record["pose_after"].as_u64().unwrap();
        records.push(record);
        writer
            .start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored)
                    .large_file(bytes.len() >= u32::MAX as usize),
            )
            .map_err(|e| e.to_string())?;
        writer.write_all(&bytes).map_err(|e| e.to_string())?;
        if records.len() % 50 == 0 {
            println!(
                "Validated {}/{} bundles selected; pose bytes {pose_before} -> {pose_after}",
                selected,
                records.len()
            );
        }
    }
    if records.is_empty() {
        return Err("no emote bundles found".into());
    }
    writer
        .finish()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    let after = fs::metadata(&temporary).map_err(|e| e.to_string())?.len();
    let frames: u64 = records.iter().filter_map(|r| r["frames"].as_u64()).sum();
    let current = fs::metadata(input).map_err(|e| e.to_string())?;
    if current.len() != metadata.len() || current.modified().ok() != metadata.modified().ok() {
        return Err("source PK3 changed during conversion".into());
    }
    let report = json!({"encoding":"DJKPOSE4 local TRS pool with exact affine residuals", "input":input.to_string_lossy(),
        "output":output.to_string_lossy(),"archive_before":metadata.len(),"archive_after":after,
        "pose_before":pose_before,"pose_after":pose_after,"frames":frames,"selected":selected,
        "bundles":records,"elapsed_seconds":start.elapsed().as_secs_f64(),"exact_float_bits":true,
        "runtime_matrix_storage":"unchanged: full decoded affine matrices; only loading changes"});
    let mut report_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&report_path)
        .map_err(|e| e.to_string())?;
    report_file
        .write_all(&serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    report_file.sync_all().map_err(|e| e.to_string())?;
    fs::hard_link(&temporary, output).map_err(|e| e.to_string())?;
    fs::remove_file(&temporary).map_err(|e| e.to_string())?;
    println!(
        "Validated {} bundles ({selected} selected), archive {} -> {after} bytes; report {}",
        report["bundles"].as_array().unwrap().len(),
        metadata.len(),
        report_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repack_preserves_source_native_assets_and_full_playback() {
        let root = std::env::temp_dir().join(format!("djk-pose-pool-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let directory = root.join("authoring");
        fs::create_dir_all(&directory).unwrap();
        let gla = jka_assets::ghoul2::openjk_default_gla().unwrap();
        let gla_bytes =
            crate::retarget::encode(&gla, 1, &[(glam::Quat::IDENTITY, glam::Vec3::ZERO)]).unwrap();
        let matrices: Vec<_> = (0..120)
            .map(|f| {
                let a = f as f32 * 0.07;
                [
                    [a.cos(), -a.sin(), 0.0, a * 2.0],
                    [a.sin(), a.cos(), 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                ]
            })
            .collect();
        let pose = PoseClip::pack_compact(&PoseClip::encode(120, 1, &matrices).unwrap()).unwrap();
        let original = PoseClip::parse(&pose).unwrap();
        let manifest = json!({"format":"djk_fnemote","version":1,"rig_only":true,
            "emotes":[{"id":"test","name":"Test","gla":"animation.gla","pose":"animation.pose",
            "first_frame":0,"end_frame":120,"fps":30}]});
        let bundle = directory.join("test.djk_fnemote");
        crate::write_shipping_bundle(
            &bundle,
            &manifest,
            &std::collections::BTreeMap::from([
                ("animation.gla".into(), gla_bytes),
                ("animation.pose".into(), pose),
                ("props/preserved.bin".into(), b"keep this exact".to_vec()),
            ]),
        )
        .unwrap();
        let native = root.join("native.pk3");
        let mut z = ZipWriter::new(File::create(&native).unwrap());
        z.start_file("textures/preserved.bin", SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"unchanged native data").unwrap();
        z.finish().unwrap();
        let source = root.join("fnemotes-v12.pk3");
        crate::package::library(&directory, Some(&native), &source).unwrap();
        let source_bytes = fs::read(&source).unwrap();
        let output = root.join("fnemotes-v13.pk3");
        run(&source, &output).unwrap();
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        assert!(run(&source, &output).is_err());
        let loaded = Bundle::open_packed_index(&output, "emotes/test.djk_fnemote")
            .unwrap()
            .validate_for_playback()
            .unwrap();
        assert!(exact(&original, loaded.pose.as_ref().unwrap()));
        assert_eq!(
            loaded.read("props/preserved.bin", 100).unwrap(),
            b"keep this exact"
        );
        let mut z = ZipArchive::new(File::open(&output).unwrap()).unwrap();
        let mut preserved = Vec::new();
        z.by_name("textures/preserved.bin")
            .unwrap()
            .read_to_end(&mut preserved)
            .unwrap();
        assert_eq!(preserved, b"unchanged native data");
        drop(z);
        let mut vfs = jka_assets::pk3::AssetSearchPath::open(&root).unwrap();
        let (provider, _) = vfs
            .stored_location("emotes/test.djk_fnemote")
            .unwrap()
            .unwrap();
        assert_eq!(provider, output);
        assert_eq!(
            vfs.names()
                .filter(|n| n.starts_with("emotes/") && n.ends_with(".djk_fnemote"))
                .count(),
            1
        );
        drop(vfs);
        for file in [
            bundle,
            native,
            source,
            output.clone(),
            output.with_extension("report.json"),
        ] {
            fs::remove_file(file).unwrap();
        }
        fs::remove_dir(directory).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
