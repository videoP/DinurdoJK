//! Times `AssetSearchPath::open_game` cold and warm against a real base directory.
//!
//! `cargo run --release -p jka-assets --example vfs_open_bench -- path/to/base`
//! Set `JKA_VFS_CACHE=0` to see the uncached cost of every open.
use std::{path::PathBuf, time::Instant};

use jka_assets::pk3::AssetSearchPath;

fn main() {
    let base = PathBuf::from(std::env::args().nth(1).expect("usage: vfs_open_bench <base dir>"));
    for pass in 0..4 {
        let started = Instant::now();
        let mut assets = AssetSearchPath::open_game(&base, None).expect("open base");
        let opened = started.elapsed();
        let names = assets.names().count();
        let started = Instant::now();
        let read = assets
            .read("gfx/2d/charsgrid_med.tga", 8 << 20)
            .ok()
            .flatten()
            .map(|asset| asset.bytes.len());
        println!(
            "open #{pass}: {:.1} ms ({names} names); first read {:.1} ms ({read:?} bytes)",
            opened.as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }
}
