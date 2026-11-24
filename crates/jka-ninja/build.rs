//! Selects the Fluid Ninja implementation, like jka-client's NWH module.
//!
//! The real implementation lives in the private DinurdoJK-ninja repository,
//! checked out beside this one (`<parent>/DinurdoJK-ninja`, i.e.
//! `D:\Code\DinurdoJK-ninja` locally; CI clones it there from a secret). When
//! its `src/lib.rs` exists this sets `cfg(djk_ninja_private)` and lib.rs
//! includes it; otherwise lib.rs compiles `src/stub.rs`.
//!
//! The private path is fixed because `#[path]` takes a literal. After cloning
//! the private repository for the first time, touch this file (or run
//! `cargo clean -p jka-ninja`) so Cargo re-runs this script.
use std::path::Path;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(djk_ninja_private)");
    println!("cargo:rustc-check-cfg=cfg(djk_ninja_blood_engine)");
    println!("cargo:rerun-if-changed=build.rs");
    // CI sets DINURDOJK_NINJA=1 after cloning the private repository, so a
    // restored stub-built cache re-runs this script instead of keeping the stub.
    println!("cargo:rerun-if-env-changed=DINURDOJK_NINJA");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // Must match the #[path] in src/lib.rs (relative to src/, one level deeper).
    let private = Path::new(&manifest).join("../../../DinurdoJK-ninja/src");
    let required = std::env::var("DINURDOJK_NINJA").is_ok_and(|v| v == "1");
    if required && !private.join("lib.rs").is_file() {
        panic!("DINURDOJK_NINJA=1 but {} is missing", private.join("lib.rs").display());
    }
    if private.join("lib.rs").is_file() {
        // Presence only: rustc's own dep-info already tracks every included file.
        println!("cargo:rerun-if-changed={}", private.join("lib.rs").display());
        println!("cargo:rustc-cfg=djk_ninja_private");
        // Watch the directory so adding or removing the optional shader
        // reconfigures the facade without watching a missing path.
        let blood_dir = private.join("blood");
        if blood_dir.is_dir() {
            println!("cargo:rerun-if-changed={}", blood_dir.display());
            if blood_dir.join("engine.wgsl").is_file() {
                println!("cargo:rustc-cfg=djk_ninja_blood_engine");
            }
        }
    }
}
