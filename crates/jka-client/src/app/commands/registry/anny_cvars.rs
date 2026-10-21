//! Named Anny controls share the normal config/runtime settings path.
use super::cvar_prelude::*;
app_cvars! {
    APP_R_BODYANNY {
        runtime: CvarDef::new(
            "r_bodyAnny", "", "label=value,...",
            "Anny phenotype, local shape and facial-action values; empty restores defaults.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.video.body_details.encode()),
            |app, value| {
                app.video.body_details = jka_assets::body_shape::anny::Details::parse(&value.to_string())?;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(details) = jka_assets::body_shape::anny::Details::parse(value) {
                    settings.body_details = details;
                }
            }),
            archive: Some((10445, |state| state.video.body_details.encode())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
}
