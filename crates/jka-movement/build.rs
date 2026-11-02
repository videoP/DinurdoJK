use std::env;

fn main() {
    cc::Build::new()
        .include("native")
        .file("native/mod_dispatch.c")
        .compile("jka_mod_dispatch");
    let root = "vendor/openjk";
    let mut movement = cc::Build::new();
    movement
        .include(format!("{root}/codemp"))
        .include(format!("{root}/shared"))
        .include("native")
        .define("_CGAME", None)
        .define("FINAL_BUILD", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .warnings(false)
        .flag_if_supported("/fp:precise")
        .flag_if_supported("/we4013")
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-ffp-contract=off");
    define_linux_arch_string(&mut movement);
    for file in [
        "codemp/game/bg_pmove.c",
        "codemp/game/bg_slidemove.c",
        "codemp/game/bg_panimate.c",
        "codemp/game/bg_saber.c",
        "codemp/game/bg_misc.c",
        "codemp/game/bg_weapons.c",
        "shared/qcommon/q_math.c",
        "codemp/qcommon/q_shared.c",
        "shared/qcommon/q_string.c",
    ] {
        movement.file(format!("{root}/{file}"));
    }
    if movement.get_compiler().is_like_msvc() {
        movement.flag("/FIstock_namespace.h");
    } else {
        movement.flag("-include").flag("native/stock_namespace.h");
    }
    movement.file("native/movement.c").compile("jka_pmove");
    let japro_root = "vendor/japro";
    let mut japro = cc::Build::new();
    japro
        .include(format!("{japro_root}/codemp"))
        .include(format!("{japro_root}/shared"))
        .include("native/japro")
        .include("native")
        .define("_CGAME", None)
        .define("FINAL_BUILD", None)
        .define("JKA_JAPRO", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .warnings(false)
        .flag_if_supported("/fp:precise")
        .flag_if_supported("/we4013")
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-fcommon")
        .flag_if_supported("-ffp-contract=off");
    define_linux_arch_string(&mut japro);
    for file in [
        "codemp/game/bg_pmove.c",
        "codemp/game/bg_slidemove.c",
        "codemp/game/bg_panimate.c",
        "codemp/game/bg_saber.c",
        "codemp/game/bg_misc.c",
        "codemp/game/bg_weapons.c",
        "shared/qcommon/q_math.c",
        "codemp/qcommon/q_shared.c",
        "shared/qcommon/q_string.c",
    ] {
        japro.file(format!("{japro_root}/{file}"));
    }
    if japro.get_compiler().is_like_msvc() {
        japro.flag("/FIjapro_namespace.h");
    } else {
        japro.flag("-include").flag("native/japro_namespace.h");
    }
    japro.file("native/japro_movement.c").compile("jka_japro_pmove");
    // Separate JK2 native layouts and symbols. Never add this to JKA's backend
    // switch or feed normalized presentation state to its snapshot importer.
    let mut jk2 = cc::Build::new();
    jk2.include("vendor/mvsdk/code").include("vendor/mvsdk/code/game")
        .include("vendor/mvsdk/code/cgame").include("vendor/mvsdk/code/renderer").include("native")
        .define("JK2_CGAME", None).define("_JK2", None).define("FINAL_BUILD", None)
        .define("_CRT_SECURE_NO_WARNINGS", None).warnings(false)
        .flag_if_supported("/fp:precise").flag_if_supported("/we4013")
        .flag_if_supported("-fno-strict-aliasing").flag_if_supported("-ffp-contract=off");
    match env::var("CARGO_CFG_TARGET_ARCH").ok().as_deref() {
        Some("x86_64") => { jk2.define("ARCH_X86_64", None); }
        Some("x86") => { jk2.define("ARCH_X86", None); }
        Some("aarch64") => { jk2.define("ARCH_ARM64", None); }
        _ => {}
    }
    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") { jk2.define("WIN32", None); }
    if jk2.get_compiler().is_like_msvc() { jk2.flag("/FIjk2_namespace.h"); }
    else { jk2.flag("-include").flag("native/jk2_namespace.h"); }
    for file in ["bg_pmove", "bg_slidemove", "bg_panimate", "bg_saber", "bg_weapons", "bg_misc", "bg_multiversion", "animMappingTable", "q_math", "q_shared"] {
        jk2.file(format!("vendor/mvsdk/code/game/{file}.c"));
    }
    jk2.file("native/jk2_movement.c").file("native/jk2_oracle.c").compile("jk2_mvsdk_pmove");
    let mut collision = cc::Build::new();
    collision
        .cpp(true)
        .include(format!("{root}/codemp"))
        .include(format!("{root}/shared"))
        .include("native")
        .define("FINAL_BUILD", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .define("Com_Error", "JKA_CM_Error")
        .define("Com_Printf", "JKA_CM_Printf")
        // The STL since VS 2022 17.10 gives std::mutex a constexpr constructor that
        // skips _Mtx_init_in_situ. Against a msvcp140.dll older than 14.40 (any PC
        // without a recent VC++ redistributable) the first lock() then dereferences
        // an uninitialised mutex and crashes in jka_lock. This keeps the old ABI.
        .define("_DISABLE_CONSTEXPR_MUTEX_CONSTRUCTOR", None)
        .warnings(false)
        .flag_if_supported("/fp:precise")
        .flag_if_supported("/EHsc")
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-ffp-contract=off");
    define_linux_arch_string(&mut collision);
    for file in [
        "cm_load.cpp",
        "cm_trace.cpp",
        "cm_test.cpp",
        "cm_patch.cpp",
        "cm_polylib.cpp",
    ] {
        collision.file(format!("{root}/codemp/qcommon/{file}"));
    }
    collision.file(format!("{root}/shared/sys/snapvector.cpp"));
    collision
        .file("native/collision.cpp")
        .compile("jka_collision");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=native");
}

fn define_linux_arch_string(build: &mut cc::Build) {
    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("linux") {
        return;
    }

    let Some(arch) = env::var("CARGO_CFG_TARGET_ARCH").ok() else {
        return;
    };
    let arch = match arch.as_str() {
        "aarch64" => "arm64",
        other => other,
    };
    let value = format!("\"{arch}\"");
    build.define("ARCH_STRING", Some(value.as_str()));
}
