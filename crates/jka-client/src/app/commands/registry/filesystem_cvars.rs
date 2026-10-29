//! Persistent filesystem root cvars. These are init-time changes, deliberately
//! not live VFS remounts: in-flight map workers and presenters keep consistent
//! indexes until the next process start. Their small bootstrap file lives beside
//! the exe so moving the primary config cannot make its location undiscoverable.
use super::cvar_prelude::*;

// Fix the generic state before the getters access App-specific filesystem fields.
type CvarDef = crate::console::cvars::CvarDef<App>;

app_cvars! {
    APP_FS_BASEPATH {
        runtime: CvarDef::new(
            "fs_basepath", "", "read-only installation root",
            "Resolved directory containing the executable-relative base/ folder. --base overrides detection; not a writable cvar.",
            CvarType::String, CvarFlags::FIXED, LatchScope::None,
            |app| CvarValue::String(app.base.parent().unwrap_or(&app.base).display().to_string()),
            |_, _| Err("fs_basepath is read-only; use --base to override base/ detection".into())
        ).with_text_setter(|_, _, _| Err("fs_basepath is read-only; use --base to override base/ detection".into()), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_CDPATH {
        runtime: CvarDef::new(
            "fs_cdpath", "", "path to shared stock GameData",
            "Lowest-priority read-only asset root; loads cdpath/base PK3s without copying. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.cdpath.as_ref().map_or(String::new(), |p| p.display().to_string())),
            |app, value| App::set_fs_cdpath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_cdpath(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_HOMEPATH {
        runtime: CvarDef::new(
            "fs_homepath", "", "writable/readable GameData root",
            "Optional high-priority writable/readable root, empty uses the executable's installation root. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.homepath.as_ref().map_or(String::new(), |p| p.display().to_string())),
            |app, value| App::set_fs_homepath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_homepath(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_FORCEGAME {
        runtime: CvarDef::new(
            "fs_forcegame", "", "game folder name",
            "Always-on highest-priority asset layer, overriding server fs_game. Also determines config/demo/screenshot output subdirectory. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.forcegame.clone().unwrap_or_default()),
            |app, value| App::set_fs_forcegame(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_forcegame(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
}

impl App {
    fn store_fs_roots(&mut self, proposed: jka_assets::pk3::FilesystemRoots) -> Result<(), String> {
        // Persist first. If the write fails, do not claim the setting changed.
        crate::filesystem_paths::save(&self.base, &proposed)?;
        if self.fs_roots != proposed {
            self.fs_roots = proposed;
            // The filesystem bootstrap INI was just saved; there is no main config value to archive.
            self.push_console_line("^3FILESYSTEM:^7 path changes take effect after restarting DinurdoJK");
        }
        Ok(())
    }

    fn set_fs_homepath(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.homepath = crate::filesystem_paths::parse_root(&self.base, value)?;
        self.store_fs_roots(proposed)
    }

    fn set_fs_cdpath(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.cdpath = crate::filesystem_paths::parse_root(&self.base, value)?;
        self.store_fs_roots(proposed)
    }

    fn set_fs_forcegame(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.forcegame = crate::filesystem_paths::validate_game_name(value)?;
        self.store_fs_roots(proposed)
    }
}
