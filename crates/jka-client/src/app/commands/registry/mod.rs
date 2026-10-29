//! One source for command metadata/handlers and migrated subsystem cvar bindings.
use crate::{
    app::App,
    console::{cvars::CvarRegistry, Entry, EntryKind, LatchScope},
    ui::VideoSettings,
};
use std::{collections::HashMap, sync::OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandScope {
    Always,
    Connected,
    Japro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandResult {
    Handled,
    Status,
    Unhandled,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GameCommand {
    Generic(u8),
    WeaponNext,
    WeaponPrev,
    WeaponLast,
    WeaponClean,
    ToggleFireMode,
    ForceNext,
    ForcePrev,
    InventoryNext,
    InventoryPrev,
    InventoryFree,
    Weapon,
    Flipkick,
    AmRun,
}

pub(crate) struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
    pub scope: CommandScope,
    pub game_command: Option<GameCommand>,
    /// Preserve early consumption, status printing, and local/server fallback.
    pub handler: fn(&mut App, &str) -> CommandResult,
}
impl CommandDef {
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        std::iter::once(self.name)
            .chain(self.aliases.iter().copied())
            .map(|name| Entry {
                name,
                kind: if self.scope == CommandScope::Connected {
                    EntryKind::ServerCommand
                } else {
                    EntryKind::Command
                },
                default: "",
                range: "",
                description: self.description,
                latch: LatchScope::None,
                usage: self.usage,
            })
    }
}

// Handler bodies and registration are generated together: an executable local
// command cannot be added without also supplying its console metadata.
macro_rules! commands {
    ($($method:ident [$name:literal $(, $alias:literal)* $(,)?]
       ($usage:literal, $description:literal, $scope:expr $(, game = $game:expr)?)
       |$app:ident, $line:ident| $body:block)*) => {
        pub(super) const COMMANDS: &[CommandDef] = &[$(CommandDef {
            name: $name, aliases: &[$($alias),*], description: $description,
            usage: $usage, scope: $scope, game_command: commands!(@game $($game)?), handler: App::$method,
        }),*];
        impl App { $(
            pub(in crate::app) fn $method(&mut $app, $line: &str) -> CommandResult $body
        )* }
    };
    (@game) => { None };
    (@game $game:expr) => { Some($game) };
}

// Each declaration emits its runtime descriptor and its config binding together.
macro_rules! app_cvars {
    ($($id:ident { runtime: $runtime:expr, config: $config:expr,
       implementations: [$($implementation:item)*] })*) => {
        $(pub(super) static $id: crate::console::cvars::CvarDef<App> = $runtime;
          $($implementation)*)*
        pub(crate) static DEFINITIONS: &[super::CvarDefinition] = &[$(super::CvarDefinition { runtime: &$id, config: $config }),*];
    };
}
macro_rules! settings_cvars {
    ($state:ty; $($id:ident { runtime: $runtime:expr, config: $config:expr,
       settings: $settings:ident = $binding:expr, implementations: [$($implementation:item)*] })*) => {
        $(pub(super) static $id: crate::console::cvars::CvarDef<App> = $runtime;
          pub(crate) static $settings: crate::console::cvars::CvarDef<$state> = $binding;
          $($implementation)*)*
        pub(crate) static DEFINITIONS: &[super::CvarDefinition] = &[$(super::CvarDefinition { runtime: &$id, config: $config }),*];
        pub(crate) fn settings_registry() -> &'static crate::console::cvars::CvarRegistry<$state> {
            static REGISTRY: std::sync::OnceLock<crate::console::cvars::CvarRegistry<$state>> = std::sync::OnceLock::new();
            REGISTRY.get_or_init(|| crate::console::cvars::CvarRegistry::with_default_state([$(&$settings),*], &<$state>::default()))
        }
    };
}
#[allow(unused_imports)]
mod cvar_prelude {
    pub(super) use crate::app::*;
    pub(super) use crate::camera::{self, *};
    pub(super) use crate::config::{
        self,
        cvars::{ArchiveState, CvarBinding, VideoLoadState},
        int_in, normalize_fps_cap, normalize_fx_count_scale, normalize_fx_fps,
        normalize_fx_fps_scope, normalize_fx_lod, normalize_fx_physics, normalize_lod_scale,
        parse_bool, parse_ocean_cascade, parse_rgba8, parse_vec3, AudioSettings,
        ClientPresentationSettings, GAMMA_MAX, GAMMA_MIN,
    };
    pub(super) use crate::console::{
        cvars::{CvarDef, CvarFlags, CvarType, CvarValue},
        LatchScope,
    };
    pub(super) use crate::fx::{FX_FPS_DEFAULT, FX_FPS_LEGACY_JKA, FX_FPS_MAX, FX_FPS_MIN};
    pub(super) use crate::japro_cg::JaproCgame;
    pub(super) use crate::net::NetworkSettings;
    pub(super) use crate::ui::{self, *};
    pub(super) use jka_protocol::commands::atoi;
    pub(super) fn bool_value(value: bool) -> String {
        u8::from(value).to_string()
    }
    pub(super) fn network_number(
        name: &str,
        value: &str,
        min: i64,
        max: i64,
    ) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .ok()
            .map(|value| value.clamp(min, max))
            .ok_or_else(|| format!("{name}: expected an integer"))
    }
    pub(super) fn network_text(name: &str, value: &str) -> Result<String, String> {
        if value
            .bytes()
            .any(|byte| matches!(byte, b'\\' | b';' | b'"'))
        {
            Err(format!("{name}: can't use keys or values with \\ ; or \""))
        } else {
            Ok(value.to_owned())
        }
    }
    pub(super) fn japro_integer(name: &str, value: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(|number| number as i64)
            .ok_or_else(|| format!("{name}: expected a number"))
    }
    pub(super) fn japro_float(name: &str, value: &str) -> Result<f32, String> {
        value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("{name}: expected a number"))
    }
}

pub(crate) mod audio_cvars;
mod bindings;
mod buttons;
pub(crate) mod client_cvars;
pub(crate) mod cloud_cvars;
pub(crate) mod color_cvars;
pub(crate) mod hdr_output_cvars;
mod connection;
mod core;
pub(crate) mod crosshair_cvars;
pub(crate) mod crt_cvars;
mod diagnostic_cvars;
mod diagnostics;
pub(crate) mod effects_cvars;
mod gameplay;
pub(crate) mod hud;
mod hud_commands;
pub(crate) mod hud_layout_cvars;
pub(crate) mod japro_cvars;
pub(crate) mod jiggle_cvars;
pub(crate) mod jiggle_bounds_cvars;
pub(crate) mod anny_cvars;
pub(crate) mod material_cvars;
mod media;
pub(crate) mod movement_cvars;
pub(crate) mod network_cvars;
pub(crate) mod filesystem_cvars;
pub(crate) mod ocean_cvars;
pub(crate) mod physics_cvars;
pub(crate) mod postprocess_cvars;
pub(crate) mod presentation_cvars;
pub(crate) mod race_ghost_cvars;
pub(crate) mod renderer_cvars;
pub(crate) mod patch_cvars;
pub(crate) mod weather_cvars;

pub(crate) struct CommandRegistry {
    definitions: Vec<&'static CommandDef>,
    by_name: HashMap<String, &'static CommandDef>,
}
impl CommandRegistry {
    fn new(definitions: impl IntoIterator<Item = &'static CommandDef>) -> Self {
        let definitions: Vec<_> = definitions.into_iter().collect();
        let mut by_name = HashMap::new();
        for def in &definitions {
            assert!(
                !def.description.is_empty() && !def.usage.is_empty(),
                "missing command help: {}",
                def.name
            );
            for name in std::iter::once(&def.name).chain(def.aliases.iter()) {
                assert!(!name.is_empty(), "empty command name");
                assert!(
                    by_name.insert(name.to_ascii_lowercase(), *def).is_none(),
                    "duplicate command/alias: {name}"
                );
            }
        }
        Self {
            definitions,
            by_name,
        }
    }
    pub fn find(&self, name: &str) -> Option<&'static CommandDef> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = &'static CommandDef> + '_ {
        self.definitions.iter().copied()
    }
}

pub(crate) fn command_registry() -> &'static CommandRegistry {
    static REGISTRY: OnceLock<CommandRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        CommandRegistry::new(
            core::COMMANDS
                .iter()
                .chain(buttons::COMMANDS)
                .chain(media::COMMANDS)
                .chain(hud_commands::COMMANDS)
                .chain(diagnostics::COMMANDS)
                .chain(gameplay::COMMANDS)
                .chain(connection::COMMANDS)
                .chain(bindings::COMMANDS),
        )
    })
}
pub(crate) struct CvarDefinition {
    pub runtime: &'static crate::console::cvars::CvarDef<App>,
    pub config: crate::config::cvars::CvarBinding,
}
pub(crate) fn cvar_definitions() -> &'static [&'static CvarDefinition] {
    static DEFINITIONS: OnceLock<Vec<&'static CvarDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        let mut definitions = Vec::new();
        definitions.extend(audio_cvars::DEFINITIONS.iter());
        definitions.extend(client_cvars::DEFINITIONS.iter());
        definitions.extend(crosshair_cvars::DEFINITIONS.iter());
        definitions.extend(presentation_cvars::DEFINITIONS.iter());
        definitions.extend(hud_layout_cvars::DEFINITIONS.iter());
        definitions.extend(movement_cvars::DEFINITIONS.iter());
        definitions.extend(race_ghost_cvars::DEFINITIONS.iter());
        definitions.extend(network_cvars::DEFINITIONS.iter());
        definitions.extend(filesystem_cvars::DEFINITIONS.iter());
        definitions.extend(japro_cvars::DEFINITIONS.iter());
        definitions.extend(postprocess_cvars::DEFINITIONS.iter());
        definitions.extend(color_cvars::DEFINITIONS.iter());
        definitions.extend(hdr_output_cvars::DEFINITIONS.iter());
        definitions.extend(crt_cvars::DEFINITIONS.iter());
        definitions.extend(effects_cvars::DEFINITIONS.iter());
        definitions.extend(cloud_cvars::DEFINITIONS.iter());
        definitions.extend(weather_cvars::DEFINITIONS.iter());
        definitions.extend(ocean_cvars::DEFINITIONS.iter());
        definitions.extend(renderer_cvars::DEFINITIONS.iter());
        definitions.extend(patch_cvars::DEFINITIONS.iter());
        definitions.extend(physics_cvars::DEFINITIONS.iter());
        definitions.extend(material_cvars::DEFINITIONS.iter());
        definitions.extend(jiggle_cvars::DEFINITIONS.iter());
        definitions.extend(jiggle_bounds_cvars::DEFINITIONS.iter());
        definitions.extend(anny_cvars::DEFINITIONS.iter());
        definitions
    })
}
pub(crate) fn cvar_definition(name: &str) -> Option<&'static CvarDefinition> {
    static LOOKUP: OnceLock<HashMap<String, &'static CvarDefinition>> = OnceLock::new();
    LOOKUP
        .get_or_init(|| {
            cvar_definitions()
                .iter()
                .map(|def| (def.runtime.entry.name.to_ascii_lowercase(), *def))
                .collect()
        })
        .get(&name.to_ascii_lowercase())
        .copied()
}
pub(crate) fn cvar_registry() -> &'static CvarRegistry<App> {
    static REGISTRY: OnceLock<CvarRegistry<App>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let video = VideoSettings::default();
        let presentation = crate::config::ClientPresentationSettings::default();
        let audio = crate::config::AudioSettings::default();
        let state = crate::config::cvars::ArchiveState {
            video: &video,
            presentation: &presentation,
            audio: &audio,
        };
        CvarRegistry::new(
            cvar_definitions()
                .iter()
                .map(|def| {
                    if let Some(owner) =
                        network_cvars::settings_registry().find(def.runtime.entry.name)
                    {
                        def.runtime.with_default(owner.entry.default.to_owned())
                    } else if let Some(owner) =
                        japro_cvars::settings_registry().find(def.runtime.entry.name)
                    {
                        def.runtime.with_default(owner.entry.default.to_owned())
                    } else {
                        match (def.config.default, def.config.archive) {
                            (Some(get), _) | (None, Some((_, get))) => {
                                def.runtime.with_default(get(&state))
                            }
                            _ => def.runtime,
                        }
                    }
                })
                .chain(diagnostic_cvars::CVARS.iter()),
        )
    })
}
pub(crate) fn video_cvar_registry() -> &'static CvarRegistry<VideoSettings> {
    static REGISTRY: OnceLock<CvarRegistry<VideoSettings>> = OnceLock::new();
    REGISTRY.get_or_init(|| CvarRegistry::new(hud::VIDEO_CVARS.iter().copied()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_and_aliases_have_unique_names_and_help() {
        let registry = command_registry();
        let mut names = std::collections::HashSet::new();
        for def in registry.iter() {
            assert!(!def.usage.is_empty() && !def.description.is_empty());
            for entry in def.entries() {
                assert!(
                    names.insert(entry.name.to_ascii_lowercase()),
                    "{}",
                    entry.name
                );
                assert!(std::ptr::eq(registry.find(entry.name).unwrap(), def));
            }
        }
        assert!(std::ptr::eq(
            registry.find("quit").unwrap(),
            registry.find("EXIT").unwrap()
        ));
        assert!(std::ptr::eq(
            registry.find("plugin").unwrap(),
            registry.find("pluginDisable").unwrap()
        ));
    }
    #[test]
    fn cvar_names_and_defaults_are_valid() {
        let mut names = std::collections::HashSet::new();
        for def in cvar_registry().iter() {
            assert!(names.insert(def.entry.name.to_ascii_lowercase()));
            assert!(def.parse(def.entry.default).is_ok());
            assert_eq!(
                crate::console::find(def.entry.name).unwrap().default,
                def.entry.default
            );
        }
        let video = VideoSettings::default();
        for def in video_cvar_registry().iter() {
            assert_eq!((def.get)(&video), def.default_value());
        }
    }
    #[test]
    fn cg_remaps_is_live_session_only_and_defaults_to_one() {
        use crate::console::cvars::CvarFlags;
        let definition = cvar_registry().find("cg_remaps").expect("cg_remaps registered");
        assert_eq!(definition.entry.default, "1");
        assert_eq!(definition.entry.latch, LatchScope::None);
        assert!(!definition.flags.contains(CvarFlags::ARCHIVE));
        let config = cvar_definition("cg_remaps").expect("cg_remaps config binding");
        assert!(config.config.archive.is_none());
        assert!(config.config.load_presentation.is_none());
        assert_eq!(crate::japro_cg::JaproCgame::default().remaps, 1);
    }

    #[test]
    fn archived_cvars_have_complete_bindings_and_stable_order() {
        let mut order = std::collections::HashSet::new();
        for def in cvar_definitions() {
            let archived = def
                .runtime
                .flags
                .contains(crate::console::cvars::CvarFlags::ARCHIVE);
            assert_eq!(
                archived,
                def.config.archive.is_some(),
                "{}",
                def.runtime.entry.name
            );
            if let Some((rank, _)) = def.config.archive {
                assert!(
                    order.insert(rank),
                    "duplicate archive position: {}",
                    def.runtime.entry.name
                );
                assert!(
                    def.runtime
                        .flags
                        .contains(crate::console::cvars::CvarFlags::FIXED)
                        || def.config.load_video.is_some()
                        || def.config.load_audio.is_some()
                        || def.config.load_presentation.is_some(),
                    "missing config setter: {}",
                    def.runtime.entry.name
                );
            }
            if def.runtime.entry.latch != LatchScope::None {
                assert!(
                    def.runtime.normalize.is_some() && def.config.apply_latched.is_some(),
                    "missing latch binding: {}",
                    def.runtime.entry.name
                );
            }
        }
    }

    #[test]
    fn local_team_command_wins_over_teamoverlay_even_without_a_live_netchan() {
        let def = command_registry().find("team").expect("team command");
        assert_eq!(def.scope, CommandScope::Always);
        assert_eq!(
            crate::console::find_command("team", false).unwrap().name,
            "team"
        );
        assert!(crate::console::prefix("team", false).any(|entry| entry.name == "teamoverlay"));
    }

    #[test]
    fn wait_is_registered_and_console_discoverable() {
        let def = command_registry().find("wait").expect("wait command");
        assert_eq!(def.name, "wait");
        assert_eq!(def.scope, CommandScope::Always);
        assert_eq!(
            crate::console::find("wait").unwrap().kind,
            EntryKind::Command
        );
        assert!(crate::console::commands(false).any(|entry| entry.name == "wait"));
        assert!(crate::console::prefix("wai", false).any(|entry| entry.name == "wait"));
    }

    #[test]
    fn in_restart_is_a_local_discoverable_command() {
        let def = command_registry().find("IN_RESTART").expect("in_restart registered");
        assert_eq!(def.scope, CommandScope::Always);
        assert_eq!(crate::console::find_command("in_restart", false).unwrap().name, "in_restart");
        assert!(crate::console::commands(false).any(|entry| entry.name == "in_restart"));
    }

    #[test]
    fn input_buttons_register_both_press_and_release_commands() {
        for name in [
            "+movedown",
            "-movedown",
            "+speed",
            "-speed",
            "+attack",
            "-attack",
            "+button15",
            "-button15",
            "+zoom",
            "-zoom",
        ] {
            assert!(command_registry().find(name).is_some(), "{name}");
            assert_eq!(crate::console::find(name).unwrap().kind, EntryKind::Command);
        }
    }

    #[test]
    fn screenshot_formats_share_registry_discovery() {
        for name in ["screenshot", "screenshot_png"] {
            assert!(command_registry().find(name).is_some());
            assert_eq!(crate::console::find(name).unwrap().kind, EntryKind::Command);
            assert!(crate::console::commands(false).any(|entry| entry.name == name));
            assert!(crate::console::prefix(name, false).any(|entry| entry.name == name));
        }
        assert_eq!(
            crate::console::unique_prefix("screenshot_p", false)
                .unwrap()
                .name,
            "screenshot_png"
        );
    }
    #[test]
    #[should_panic(expected = "duplicate command/alias")]
    fn command_alias_collisions_are_rejected() {
        let def = command_registry().find("quit").unwrap();
        CommandRegistry::new([def, def]);
    }
    #[test]
    fn game_input_ids_match_jka_protocol() {
        for (name, expected) in [
            ("sv_saberswitch", 1),
            ("force_speed", 4),
            ("force_rage", 8),
            ("zoom", 18),
            ("saberAttackCycle", 26),
            ("gloat", 31),
        ] {
            assert!(
                matches!(command_registry().find(name).unwrap().game_command,
                Some(GameCommand::Generic(value)) if value == expected),
                "{name}"
            );
        }
        assert_eq!(cvar_registry().archived().count(), 424);
        for name in ["cg_debugEvents", "cg_eventWorkers", "cg_asyncAssets"] {
            assert!(!cvar_registry()
                .find(name)
                .unwrap()
                .flags
                .contains(crate::console::cvars::CvarFlags::ARCHIVE));
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod runtime_cvar_tests;
