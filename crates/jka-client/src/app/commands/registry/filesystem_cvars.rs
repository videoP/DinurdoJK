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
            "fs_basepath", "", "JKA game root",
            "JKA root containing base/ and mod folders. Blank = the executable directory. fs_cdpath/base is an optional stock-asset fallback. Restart required; --base can override at launch.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.basepath.as_ref().map_or_else(|| app.base.parent().unwrap_or(&app.base).display().to_string(), |p| p.display().to_string())),
            |app, value| App::set_fs_basepath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_basepath(app, value), None),
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
    APP_FS_JK2BASEPATH {
        runtime: CvarDef::new(
            "fs_jk2basepath", "", "JK2 GameData root",
            "JK2 installation root containing base/ and unprefixed mod folders; defaults to <executable directory>/JK2 GameData/. Never mounted in JKA. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(jka_assets::pk3::jk2_root_directory(&app.base, &app.fs_roots).display().to_string()),
            |app, value| App::set_fs_jk2basepath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_jk2basepath(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_JK2CDPATH {
        runtime: CvarDef::new(
            "fs_jk2cdpath", "", "path to JK2 GameData",
            "Optional lowest-priority read-only JK2 retail GameData root containing base/. Local fs_jk2basepath takes precedence. Never mounted in JKA. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.jk2cdpath.as_ref().map_or(String::new(), |p| p.display().to_string())),
            |app, value| App::set_fs_jk2cdpath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_jk2cdpath(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_HOMEPATH {
        runtime: CvarDef::new(
            "fs_homepath", "", "writable/readable GameData root",
            "Optional writable overlay root. JKA uses it directly (homepath/base); JK2 uses homepath/JK2 GameData/. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.homepath.as_ref().map_or(String::new(), |p| p.display().to_string())),
            |app, value| App::set_fs_homepath(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_homepath(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_FS_FORCEGAME {
        runtime: CvarDef::new(
            "fs_forcegame", "", "client-owned override folder",
            "Optional highest-priority asset layer DinurdoJK/<name>/, above DinurdoJK/shared. Does not change game directories or write paths. Restart required.",
            CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app| CvarValue::String(app.fs_roots.forcegame.clone().unwrap_or_default()),
            |app, value| App::set_fs_forcegame(app, &value.to_string())
        ).with_text_setter(|app, _, value| App::set_fs_forcegame(app, value), None),
        config: CvarBinding::NONE,
        implementations: []
    }
}

impl App {
    fn set_fs_basepath(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.basepath = crate::filesystem_paths::parse_root(&self.base, value)?;
        self.store_fs_roots(proposed)
    }

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

    fn set_fs_jk2basepath(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.jk2basepath = crate::filesystem_paths::parse_root(&self.base, value)?;
        let changed = proposed.jk2basepath != self.fs_roots.jk2basepath;
        self.store_fs_roots(proposed)?;
        if changed { self.jk2_demo_browser.reset(); }
        Ok(())
    }

    fn set_fs_jk2cdpath(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.jk2cdpath = crate::filesystem_paths::parse_root(&self.base, value)?;
        let changed = proposed.jk2cdpath != self.fs_roots.jk2cdpath;
        self.store_fs_roots(proposed)?;
        // The Play -> Demo JK2 catalog must never keep the old install's
        // recordings after fs_jk2cdpath is changed in the console.
        if changed { self.jk2_demo_browser.reset(); }
        Ok(())
    }

    fn set_fs_forcegame(&mut self, value: &str) -> Result<(), String> {
        let mut proposed = self.fs_roots.clone();
        proposed.forcegame = crate::filesystem_paths::validate_game_name(value)?;
        self.store_fs_roots(proposed)
    }
}
