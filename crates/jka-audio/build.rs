use std::env;

fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=../../vendor/ealman");
    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    cc::Build::new()
        .cpp(true).std("c++17").define("EAXMANVERSION", "3")
        .include("../../vendor/ealman")
        .files(["native/eal_scene.cpp", "../../vendor/ealman/EalMan.cpp", "../../vendor/ealman/Chunk.cpp"])
        .flag_if_supported("/EHsc")
        .compile("djk_eal_scene");
}
