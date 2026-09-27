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
        .flag_if_supported("-ffp-contract=off");
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
        .warnings(false)
        .flag_if_supported("/fp:precise")
        .flag_if_supported("/EHsc")
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-ffp-contract=off");
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
