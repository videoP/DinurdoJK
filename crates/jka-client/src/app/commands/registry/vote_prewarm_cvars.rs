//! Passed-map-vote CPU prewarm A/B switch; session-local by design.
use super::cvar_prelude::*;

app_cvars! {
    APP_CL_VOTEMAPPRELOAD {
        runtime: CvarDef::new(
            "cl_voteMapPreload", "1", "0..3",
            "CPU-only passed-map-vote prewarm: 0 off, 1 conservative single worker, 2 frame-adaptive single worker, 3 unthrottled A/B. Never uploads GPU data before map change.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.vote_map_preload_mode.to_string()),
            |app, value| App::cvar_set_cl_votemappreload(app, APP_CL_VOTEMAPPRELOAD.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cl_votemappreload, None),
        config: CvarBinding { default: Some(|_| "1".to_string()), ..CvarBinding::NONE },
        implementations: [
            impl App {
                fn cvar_set_cl_votemappreload(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .map_err(|_| format!("{name}: expected integer 0..3"))?;
                    if mode > 3 { return Err(format!("{name}: expected integer 0..3")); }
                    if self.vote_map_preload_mode != mode {
                        self.cancel_vote_map_prewarm();
                        self.vote_map_preload_mode = mode;
                        self.push_console_line(format!(
                            "[VOTE PREWARM] cl_voteMapPreload={mode} ({}). Mode 3 enables one-shot vote network diagnostics.",
                            match mode { 0 => "off", 1 => "conservative", 2 => "adaptive", _ => "aggressive/debug" },
                        ));
                    }
                    Ok(())
                }
            }
        ]
    }
}
