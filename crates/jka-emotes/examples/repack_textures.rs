//! Merge the texture audit's overlay, preserving every other compressed member.
//! Usage: repack_textures source.pk3 overlay.zip audit.json fresh-output.pk3
use std::{collections::HashSet, fs::File, fs::OpenOptions};
use zip::{ZipArchive, ZipWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err("expected source.pk3 overlay.zip audit.json fresh-output.pk3".into());
    }
    let report: serde_json::Value = serde_json::from_reader(File::open(&args[2])?)?;
    let replacements = report["replacements"]
        .as_object()
        .ok_or("missing replacements")?;
    let mut source = ZipArchive::new(File::open(&args[0])?)?;
    let mut overlay = ZipArchive::new(File::open(&args[1])?)?;
    let changed: HashSet<_> = overlay.file_names().map(str::to_owned).collect();
    for (old, new) in replacements {
        if !source.file_names().any(|n| n == old)
            || !changed.contains(new.as_str().ok_or("invalid replacement")?)
        {
            return Err(format!("missing replacement pair: {old}").into());
        }
    }
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    let mut writer = ZipWriter::new(output);
    let mut preserved = 0;
    for index in 0..source.len() {
        let entry = source.by_index(index)?;
        if replacements.contains_key(entry.name()) || changed.contains(entry.name()) {
            continue;
        }
        writer.raw_copy_file(entry)?;
        preserved += 1;
    }
    for index in 0..overlay.len() {
        writer.raw_copy_file(overlay.by_index(index)?)?;
    }
    writer.finish()?.sync_all()?;
    println!(
        "Preserved {preserved} compressed members; merged {} overlay members",
        overlay.len()
    );
    Ok(())
}
