fn main() {
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
    movement.file("native/movement.c").compile("jka_pmove");
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
