//! Typed world-space jiggle/remesh Z tuning, size and solver-shape cvars.
use super::{cvar_prelude::*, CvarDefinition};

// Keep the declaration owned by VideoSettings, then bind that exact typed cvar
// into App.  The cvar metadata/range/parser therefore has one source of truth.
macro_rules! jiggle_float_cvars {
    ($($settings_id:ident, $app_id:ident =>
        ($name:literal, $field:ident, $default:literal, $range:literal, $min:expr, $max:expr,
         $description:literal, $archive:expr);)*) => {
        $(
            pub(crate) static $settings_id: CvarDef<VideoSettings> = CvarDef::new(
                $name,
                $default,
                $range,
                $description,
                CvarType::Float { min: $min, max: $max },
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |settings| CvarValue::Float(settings.$field),
                |settings, value| {
                    let CvarValue::Float(value) = value else {
                        unreachable!("validated float cvar")
                    };
                    settings.$field = value;
                    Ok(())
                },
            );

            pub(super) static $app_id: CvarDef<App> = $settings_id.bind(
                |app| ($settings_id.get)(&app.video),
                |app, value| {
                    ($settings_id.set)(&mut app.video, value)?;
                    app.mark_config_dirty();
                    app.publish_ui();
                    Ok(())
                },
            );
        )*

        pub(crate) static DEFINITIONS: &[CvarDefinition] = &[
            $(CvarDefinition {
                runtime: &$app_id,
                config: CvarBinding {
                    // Clamp saved configs into range (a narrowed range must not
                    // silently reset a stored value to its default).
                    load_video: Some(|settings, _name, value, _context| {
                        if let Some(value) =
                            value.trim().parse::<f32>().ok().filter(|value| value.is_finite())
                        {
                            let _ = ($settings_id.set)(
                                settings,
                                CvarValue::Float(value.clamp($min, $max)),
                            );
                        }
                    }),
                    archive: Some(($archive, |state| format!("{:.3}", state.video.$field))),
                    ..CvarBinding::NONE
                },
            }),*
        ];
    };
}

jiggle_float_cvars! {
    SETTINGS_R_JIGGLESTOMACHSTRENGTH, APP_R_JIGGLESTOMACHSTRENGTH => (
        "r_jiggleStomachStrength", jiggle_stomach_strength,
        "0", "0..2", 0.0, 2.0,
        "Optional stomach motion; only fitted abdominal fullness jiggles. Zero disables it. Works with either solver.",
        10435
    );
    SETTINGS_R_JIGGLESTOMACHSTIFFNESS, APP_R_JIGGLESTOMACHSTIFFNESS => (
        "r_jiggleStomachStiffness", jiggle_stomach_stiffness,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics stomach stiffness multiplier (stacks with r_jiggleStiffness).",
        10436
    );
    SETTINGS_R_JIGGLESTOMACHDAMPING, APP_R_JIGGLESTOMACHDAMPING => (
        "r_jiggleStomachDamping", jiggle_stomach_damping,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics stomach damping multiplier (stacks with r_jiggleDamping).",
        10437
    );
    SETTINGS_R_BODYMUSCLE, APP_R_BODYMUSCLE => (
        "r_bodyMuscle", body_muscle, "0.5", "0..1", 0.0, 1.0,
        "Anny muscle phenotype; 0.5 preserves the original body.",
        10440
    );
    SETTINGS_R_BODYWEIGHT, APP_R_BODYWEIGHT => (
        "r_bodyWeight", body_weight, "0.5", "0..1", 0.0, 1.0,
        "Anny weight phenotype; 0.5 preserves the original body.",
        10441
    );
    SETTINGS_R_BODYPROPORTIONS, APP_R_BODYPROPORTIONS => (
        "r_bodyProportions", body_proportions, "0.5", "0..1", 0.0, 1.0,
        "Anny proportions phenotype; 0.5 preserves the original body.",
        10442
    );
    SETTINGS_R_BODYHEIGHT, APP_R_BODYHEIGHT => (
        "r_bodyHeight", body_height, "0.5", "0..1", 0.0, 1.0,
        "Anny height phenotype; 0.5 original. Cosmetic, collision bounds unchanged.",
        10444
    );
    SETTINGS_R_BODYAGE, APP_R_BODYAGE => (
        "r_bodyAge", body_age, "0.5", "0..1", 0.0, 1.0,
        "Anny age phenotype in the demo's 0..1 range; 0.5 preserves the original body. Cosmetic; gameplay bounds unchanged.",
        10443
    );
    SETTINGS_R_JIGGLEBREASTREMESHBOTTOMZ, APP_R_JIGGLEBREASTREMESHBOTTOMZ => (
        "r_jiggleBreastRemeshBottomZ",
        jiggle_breast_remesh_bottom_z,
        "0", "-8..8", -8.0, 8.0,
        "Breast remesh lower-edge Z offset in JKA model units; topology only.",
        10420
    );
    SETTINGS_R_JIGGLEBREASTREMESHTOPZ, APP_R_JIGGLEBREASTREMESHTOPZ => (
        "r_jiggleBreastRemeshTopZ",
        jiggle_breast_remesh_top_z,
        "0", "-8..8", -8.0, 8.0,
        "Breast remesh upper-edge Z offset in JKA model units; topology only.",
        10421
    );
    SETTINGS_R_JIGGLEGLUTEREMESHBOTTOMZ, APP_R_JIGGLEGLUTEREMESHBOTTOMZ => (
        "r_jiggleGluteRemeshBottomZ",
        jiggle_glute_remesh_bottom_z,
        "0", "-8..8", -8.0, 8.0,
        "Glute remesh lower-edge Z offset in JKA model units; topology only.",
        10422
    );
    SETTINGS_R_JIGGLEGLUTEREMESHTOPZ, APP_R_JIGGLEGLUTEREMESHTOPZ => (
        "r_jiggleGluteRemeshTopZ",
        jiggle_glute_remesh_top_z,
        "0", "-8..8", -8.0, 8.0,
        "Glute remesh upper-edge Z offset in JKA model units; topology only.",
        10423
    );
    SETTINGS_R_JIGGLEGLUTEJIGGLEBOTTOMZ, APP_R_JIGGLEGLUTEJIGGLEBOTTOMZ => (
        "r_jiggleGluteJiggleBottomZ",
        jiggle_glute_jiggle_bottom_z,
        "0", "-8..8", -8.0, 8.0,
        "Glute motion lower-edge Z offset in JKA model units; does not change topology.",
        10424
    );
    SETTINGS_R_JIGGLEGLUTEJIGGLETOPZ, APP_R_JIGGLEGLUTEJIGGLETOPZ => (
        "r_jiggleGluteJiggleTopZ",
        jiggle_glute_jiggle_top_z,
        "0", "-8..8", -8.0, 8.0,
        "Glute motion upper-edge Z offset in JKA model units; does not change topology.",
        10425
    );
    SETTINGS_R_JIGGLEBREASTSIZE, APP_R_JIGGLEBREASTSIZE => (
        "r_jiggleBreastSize",
        jiggle_breast_size,
        "0", "-2..3", -2.0, 3.0,
        "Breast mass added at the region center in JKA units (negative reduces); bakes into the promoted GPU-skinned mesh.",
        10426
    );
    SETTINGS_R_JIGGLEGLUTESIZE, APP_R_JIGGLEGLUTESIZE => (
        "r_jiggleGluteSize",
        jiggle_glute_size,
        "0", "-2..4", -2.0, 4.0,
        "Glute mass added at the region center in JKA units (negative reduces); bakes into the promoted GPU-skinned mesh.",
        10427
    );
    SETTINGS_R_JIGGLELOCOMOTION, APP_R_JIGGLELOCOMOTION => (
        "r_jiggleLocomotion",
        jiggle_locomotion,
        "1", "0..2", 0.0, 2.0,
        "Soft-tissue kick from the player's own acceleration (start/stop, jump, landing); steady running never adds lag.",
        10428
    );
    SETTINGS_R_JIGGLEIMPULSECAP, APP_R_JIGGLEIMPULSECAP => (
        "r_jiggleImpulseCap",
        jiggle_impulse_cap,
        "0.5", "0..3", 0.0, 3.0,
        "Maximum locomotion impulse per physics step in JKA units at 60 Hz, scaled for r_physicsHz. Lower tames steps/start-stop/jumps/landings; 0 disables those inputs while animated jiggle continues. Dinurdo input extension for both solvers.",
        10434
    );
    SETTINGS_R_JIGGLESWIRL, APP_R_JIGGLESWIRL => (
        "r_jiggleSwirl",
        jiggle_swirl,
        "0.35", "0..1", 0.0, 1.0,
        "Mirrored orbit coupling: bends straight bounces into elliptical/circular motion without adding energy.",
        10429
    );
    SETTINGS_R_JIGGLEBREASTSTIFFNESS, APP_R_JIGGLEBREASTSTIFFNESS => (
        "r_jiggleBreastStiffness",
        jiggle_breast_stiffness,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics stiffness multiplier for breast regions only (stacks with r_jiggleStiffness).",
        10430
    );
    SETTINGS_R_JIGGLEBREASTDAMPING, APP_R_JIGGLEBREASTDAMPING => (
        "r_jiggleBreastDamping",
        jiggle_breast_damping,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics damping multiplier for breast regions only (stacks with r_jiggleDamping).",
        10431
    );
    SETTINGS_R_JIGGLEGLUTESTIFFNESS, APP_R_JIGGLEGLUTESTIFFNESS => (
        "r_jiggleGluteStiffness",
        jiggle_glute_stiffness,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics stiffness multiplier for glute regions only (stacks with r_jiggleStiffness).",
        10432
    );
    SETTINGS_R_JIGGLEGLUTEDAMPING, APP_R_JIGGLEGLUTEDAMPING => (
        "r_jiggleGluteDamping",
        jiggle_glute_damping,
        "1", "0..3", 0.0, 3.0,
        "KawaiiPhysics damping multiplier for glute regions only (stacks with r_jiggleDamping).",
        10433
    );
}
