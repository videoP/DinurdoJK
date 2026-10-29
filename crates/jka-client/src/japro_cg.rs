//! jaPRO/TaystJK cgame client options that shape how other players are drawn
//! and what the race HUD shows: `cg_stylePlayer`, the `cg_raceTimer` family,
//! spectator helpers and the cosmetic bit table. Everything here is pure data
//! and rules so the presenter, HUD and menus share one source of truth.
//!
//! The rules follow jaPRO `CG_Player` / `CG_DuelCull` (cg_players.c, cg_ents.c)
//! and `DF_RaceTimer` (hud_strafehelper.c) verbatim where the bits are shared.

use std::fmt::Write as _;

/// jaPRO `cp_pluginDisable` bits that TaystJK exposes for jaPRO.
///
/// Keep these names/bit positions in lockstep with jaPRO `bg_public.h` and
/// TaystJK's `japroPluginDisables[]`.  Bits 15-18 exist in the shared jaPRO
/// headers, but TaystJK deliberately does not expose them for jaPRO, so they
/// are not part of [`JAPRO_PLUGIN_DISABLE_OPTIONS`].
pub mod plugin_disable {
    /// TaystJK bg_public.h: on JA+ allow seeing non-opponents during a duel.
    pub const DUEL_SEE_OTHERS: i32 = 1 << 1;
    pub const BLACK_SABERS_DISABLE: i32 = 1 << 3;
    pub const BHOP: i32 = 1 << 19;
    pub const NO_ROLL: i32 = 1 << 20;
    pub const NO_CARTWHEEL: i32 = 1 << 21;
    pub const JAWA_RUN: i32 = 1 << 22;
    pub const NO_DUEL_TELE: i32 = 1 << 23;
    pub const NO_CENTER_CP: i32 = 1 << 24;
    pub const CHATBOX_CP: i32 = 1 << 25;
    pub const NO_DAMAGE_NUMBERS: i32 = 1 << 26;
    pub const CENTER_MUZZLE: i32 = 1 << 27;
    pub const CONSOLE_CP: i32 = 1 << 28;
}

/// One jaPRO plugin-disable preference shown by both `/plugin` and the settings pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginDisableOption {
    pub bit: u8,
    pub label: &'static str,
    pub tooltip: &'static str,
}

impl PluginDisableOption {
    pub const fn mask(self) -> i32 {
        1i32 << self.bit
    }

    pub const fn enabled(self, bits: i32) -> bool {
        bits & self.mask() != 0
    }
}

/// TaystJK `japroPluginDisables[]`: the complete set exposed while connected to
/// jaPRO.  This single table feeds the GUI and console command so they cannot
/// silently drift apart again.
pub const JAPRO_PLUGIN_DISABLE_OPTIONS: &[PluginDisableOption] = &[
    PluginDisableOption {
        bit: 3,
        label: "No black sabers",
        tooltip: "Clamp black saber colours to orange on this client, matching TaystJK's jaPRO plugin-disable behavior.",
    },
    PluginDisableOption {
        bit: 19,
        label: "Disable force jumps",
        tooltip: "jaPRO preference for BHOP-only movement where the server's movement rules allow it.",
    },
    PluginDisableOption {
        bit: 20,
        label: "Disable rolls",
        tooltip: "jaPRO preference that disables rolling where the server permits the client preference.",
    },
    PluginDisableOption {
        bit: 21,
        label: "Disable cartwheels",
        tooltip: "jaPRO preference that disables cartwheel/butterfly-style movement where supported.",
    },
    PluginDisableOption {
        bit: 22,
        label: "New run animation",
        tooltip: "Use jaPRO's alternate Jawa/new run animation when the server advertises support.",
    },
    PluginDisableOption {
        bit: 23,
        label: "Disable duel tele",
        tooltip: "Ask a jaPRO server not to teleport you for private-duel setup when that server supports the preference.",
    },
    PluginDisableOption {
        bit: 24,
        label: "Disable centerprint checkpoints",
        tooltip: "Ask a jaPRO server not to send checkpoint messages as centerprints.",
    },
    PluginDisableOption {
        bit: 25,
        label: "Show chatbox checkpoints",
        tooltip: "Ask a jaPRO server to send checkpoint messages to the chat box.",
    },
    PluginDisableOption {
        bit: 26,
        label: "Disable damage numbers",
        tooltip: "Ask a jaPRO server not to send its damage-number feedback for you.",
    },
    PluginDisableOption {
        bit: 27,
        label: "Centermuzzle",
        tooltip: "Ask a jaPRO server to use the centered weapon muzzle origin instead of weapon-specific lateral/down offsets.",
    },
    PluginDisableOption {
        bit: 28,
        label: "Show checkpoints in console only",
        tooltip: "Ask a jaPRO server to route checkpoint output to the console-only channel.",
    },
];

pub fn plugin_disable_option(bit: u8) -> Option<&'static PluginDisableOption> {
    JAPRO_PLUGIN_DISABLE_OPTIONS.iter().find(|option| option.bit == bit)
}

/// TaystJK `CG_AmRun_f`: keep only the declared cp_pluginDisable range, then
/// XOR the jaPRO Jawa/new-run-animation preference bit.
///
/// TaystJK's plugin-disable range currently occupies bits 0..=28.
const PLUGIN_DISABLE_MASK: i32 = (1i32 << 29) - 1;

pub fn toggle_run_animation(bits: i32) -> i32 {
    plugin_disable::JAWA_RUN ^ (bits & PLUGIN_DISABLE_MASK)
}

/// `cg_stylePlayer` bits (jaPRO cg_local.h `JAPRO_STYLE_*`).
pub mod style {
    /// Draw the private-duel glow shell (brighter the closer your opponent is).
    pub const SHELL: u32 = 1 << 1;
    pub const HIDE_DUELERS: u32 = 1 << 2;
    /// Hide racers while you are in FFA.
    pub const HIDE_RACERS_FFA: u32 = 1 << 3;
    /// Hide FFA players while you are racing.
    pub const HIDE_NONRACERS_IF_RACER: u32 = 1 << 4;
    /// Hide other racers while you are racing.
    pub const HIDE_RACERS_IF_RACER: u32 = 1 << 5;
    /// Draw fellow racers normally instead of as translucent ghosts.
    pub const RACER_VFX_DISABLE: u32 = 1 << 6;
    /// Draw FFA players normally while you are racing.
    pub const NONRACER_VFX_DISABLE: u32 = 1 << 7;
    /// Draw duelers as translucent ghosts instead of dark models with a bubble.
    pub const VFX_DUELERS: u32 = 1 << 8;
    /// Defined upstream; current CG_Player alt-dim uses cp_altDimAlpha instead.
    pub const VFX_ALT_DIM: u32 = 1 << 9;
    /// Base/JA+: hide everyone but your opponent while you duel.
    pub const HIDE_NONDUELERS: u32 = 1 << 10;
    pub const HIDE_COSMETICS: u32 = 1 << 16;
    /// TaystJK JAPRO_STYLE_DISABLEBREATHING (cg_stylePlayer bit 17).
    pub const DISABLE_BREATHING: u32 = 1 << 17;
    pub const SEASONAL_COSMETICS: u32 = 1 << 20;
}

/// Bit, menu label, tooltip for the `cg_stylePlayer` switches DinurdoJK acts on.
pub const STYLE_TOGGLES: &[(u32, &str, &str)] = &[
    (
        style::DISABLE_BREATHING,
        "Disable breathing effects",
        "Hide TaystJK cold breath and underwater bubbles on player models.",
    ),
    (
        style::SHELL,
        "Duel glow shell",
        "While you duel, wrap you and your opponent in a glow that fades with distance.",
    ),
    (
        style::HIDE_DUELERS,
        "Hide duelers",
        "Do not draw other players who are in a private duel.",
    ),
    (
        style::HIDE_RACERS_FFA,
        "Hide racers in FFA",
        "While you are not racing, do not draw players who are in race mode.",
    ),
    (
        style::HIDE_NONRACERS_IF_RACER,
        "Hide non-racers while racing",
        "While you race, do not draw anyone who is not racing.",
    ),
    (
        style::HIDE_RACERS_IF_RACER,
        "Hide other racers while racing",
        "While you race, do not draw the other racers.",
    ),
];

/// Translucent-ghost switches. `invert` means the bit *disables* the ghost look,
/// so the menu shows it as "on" when the bit is clear.
pub const GHOST_TOGGLES: &[(u32, bool, &str, &str)] = &[
    (
        style::VFX_DUELERS,
        false,
        "Translucent duelers",
        "Draw players in a private duel as blue translucent ghosts instead of dark models inside a bubble.",
    ),
    (
        style::RACER_VFX_DISABLE,
        true,
        "Translucent racers",
        "While you race (or watch from FFA), draw racers as translucent ghosts so they never block your view.",
    ),
    (
        style::NONRACER_VFX_DISABLE,
        true,
        "Translucent FFA players while racing",
        "While you race, draw players who are not racing as translucent ghosts.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JaproCgame {
    /// TaystJK `cg_remaps`: 0 blocks server shader remaps, 1 blocks only
    /// `models/players/*`, every other non-zero value allows all server remaps.
    /// The local `/remapShader` command is intentionally independent of this.
    pub remaps: i32,
    /// `cg_stylePlayer` bit mask (see [`style`]).
    pub style_player: u32,
    /// TaystJK JA+ cp_altDimAlpha: 1..255 alpha byte for other dimensions.
    pub alt_dim_alpha: u8,
    /// `cg_raceTimer`: 0 off, 1 time, 2 time + max/avg/start speed, 3 same with milliseconds.
    pub race_timer: u8,
    pub race_timer_size: f32,
    pub race_timer_x: f32,
    pub race_timer_y: f32,
    /// `cg_raceStart`: show the speed you left the start line with.
    pub race_start: bool,
    pub race_start_x: f32,
    pub race_start_y: f32,
    /// `cg_startGoal`: start speed at which the readout turns green (0 = never).
    pub start_goal: f32,
    /// `cg_specHud`: draw team-coloured through-wall player outlines while spectating or watching demos.
    pub spec_hud: bool,
    /// `cg_specFollowFastest`: while spectating, automatically follow the fastest player.
    pub follow_fastest: bool,
    /// `cg_speedometer` and its position/size cvars, plus `cg_jumpGoal`.
    pub speedometer: crate::speedometer::Settings,
    /// `cg_lagometer`, `cg_lagometerX`, `cg_lagometerY`.
    pub lagometer: crate::lagometer::Settings,
    /// `cg_drainFX`: 0 off, 1 stock `mp/drain(wide).efx`, 2 jaPRO
    /// `mp/drain(wide)_japro.efx`. Only the Force-drain hand effect
    /// (`activeForcePass > FORCE_LEVEL_3`); force lightning is unaffected.
    pub drain_fx: u8,
    /// TaystJK screen-copy effects: refractive cloak and Force push/pull.
    pub render_to_texture_fx: u8,
    /// jaPRO cg_thirdPersonFlagAlpha: opacity of the viewer's carried flag.
    pub third_person_flag_alpha: f32,
}

impl Default for JaproCgame {
    fn default() -> Self {
        Self {
            remaps: 1,
            style_player: 0,
            alt_dim_alpha: 40,
            race_timer: 2,
            race_timer_size: 0.75,
            race_timer_x: 5.0,
            race_timer_y: 280.0,
            race_start: false,
            race_start_x: 300.0,
            race_start_y: 280.0,
            start_goal: 0.0,
            spec_hud: true,
            follow_fastest: false,
            speedometer: crate::speedometer::Settings::default(),
            lagometer: crate::lagometer::Settings::default(),
            drain_fx: 2,
            render_to_texture_fx: 1,
            third_person_flag_alpha: 1.0,
        }
    }
}

impl JaproCgame {
    pub fn style_bit(&self, bit: u32) -> bool {
        self.style_player & bit != 0
    }

    #[cfg(test)]
    pub fn cvar_value(&self, name: &str) -> Option<String> {
        crate::app::japro_cvars::settings_registry().get(self, name).map(|value| value.to_string())
    }
    pub fn set_cvar(&mut self, name: &str, value: &str) -> Option<Result<(), String>> {
        let registry = crate::app::japro_cvars::settings_registry();
        registry.find(name)?;
        Some(registry.set(self, name, value))
    }

    #[cfg(test)]
    pub fn write_cfg(&self, out: &mut String) {
        use std::fmt::Write;
        let mut definitions: Vec<_> = crate::app::japro_cvars::DEFINITIONS.iter().filter(|def| def.config.archive.is_some()).collect();
        definitions.sort_by_key(|def| def.config.archive.unwrap().0);
        let mut presentation = crate::config::ClientPresentationSettings::default();
        presentation.japro = self.clone();
        let video = crate::ui::VideoSettings::default();
        let audio = crate::config::AudioSettings::default();
        let state = crate::config::cvars::ArchiveState { video: &video, presentation: &presentation, audio: &audio };
        for def in definitions { let _ = writeln!(out, "seta {} \"{}\"", def.runtime.entry.name, (def.config.archive.unwrap().1)(&state)); }
    }
}

// ------------------------------------------------------------ appearance --

/// What the viewer's own state means for how everybody else is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleViewer {
    /// The connected server is jaPRO (racemode/duel culling are jaPRO-only).
    pub japro: bool,
    /// JA+ mod detection, independent of whether jp_cinfo is zero.
    pub japlus: bool,
    /// TaystJK cgs.jpDuelAlpha, parsed from JA+ serverinfo.
    pub japlus_duel_alpha: i32,
    /// cp_pluginDisable; bit DUEL_SEE_OTHERS disables JA+ bystander culling.
    pub plugin_disable: i32,
    pub style_player: u32,
    /// `cg.snap->ps.clientNum`.
    pub client_num: i32,
    /// `cg.snap->ps.duelInProgress` and `duelIndex`.
    pub dueling: bool,
    pub duel_index: i32,
    /// `IsRacemode(&cg.predictedPlayerState)`.
    pub racemode: bool,
    /// `stats[STAT_MOVEMENTSTYLE] == MV_COOP_JKA`: partners share a course.
    pub coop_race: bool,
    /// A free-flying spectator (not following anybody).
    pub free_spectator: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ghost {
    None,
    /// `gfx/effects/raceShader`, translucent blue.
    Race,
    /// `gfx/effects/duelShader`, translucent blue-violet.
    Duel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub visible: bool,
    pub ghost: Ghost,
    /// Darken the model to 1/5 brightness (non-participants of your duel).
    pub dim: bool,
    /// Dark 50/255 tint plus the sight bubble on a dueler seen from outside.
    pub duel_bubble: bool,
    /// JA+ RF_FORCE_ENT_ALPHA, byte used only with the normal player skin.
    pub force_alpha: Option<u8>,
}

impl Appearance {
    pub const NORMAL: Self = Self { visible: true, ghost: Ghost::None, dim: false, duel_bubble: false, force_alpha: None };
    const HIDDEN: Self = Self { visible: false, ..Self::NORMAL };
}

pub const MV_COOP_JKA: i32 = 15;

/// Stable lowercase jaPRO movement-style slug. These names are also used by
/// the public race archive (`.../{course}-{style}.json`).
pub fn movement_style_slug(style: i32) -> Option<&'static str> {
    Some(match style {
        0 => "siege",
        1 => "jka",
        2 => "qw",
        3 => "cpm",
        4 => "q3",
        5 => "pjk",
        6 => "wsw",
        7 => "rjq3",
        8 => "rjcpm",
        9 => "swoop",
        10 => "jetpack",
        11 => "speed",
        12 => "sp",
        13 => "slick",
        14 => "botcpm",
        15 => "coop",
        16 => "ocpm",
        17 => "tribes",
        18 => "surf",
        _ => return None,
    })
}

/// jaPRO `IntegerToRaceName`: `movementStyle_e` -> lowercase style name. For
/// `MV_COOP_JKA` the partner (`duelIndex`, only while `duelInProgress`) is
/// appended when their clientinfo is valid and named. `partner_name` is that
/// lookup, already `None` for an invalid/unnamed partner.
pub fn integer_to_race_name(style: i32, dueling: bool, partner_name: Option<&str>) -> String {
    let Some(name) = movement_style_slug(style) else { return "ERROR".to_owned() };
    if style != MV_COOP_JKA {
        return name.to_owned();
    }
    match partner_name.filter(|partner| dueling && !partner.is_empty()) {
        Some(partner) => format!("{name} (co-op: {partner}^7)"),
        None => format!("{name} (co-op)"),
    }
}
/// TaystJK cg_local.h JAPLUS_CINFO_ALTDIM and bg_public.h EF_ALT_DIM.
pub const JAPLUS_CINFO_ALTDIM: i32 = 1 << 17;
pub const EF_ALT_DIM: i32 = 1 << 15;

/// TaystJK CG_JAPlusInOtherDimension uses predicted viewer eFlags XOR target
/// entity eFlags, not bolt1 or bolt2. Capability and other-client gates apply.
pub fn ja_plus_other_dimension(
    japlus_cinfo: i32,
    viewer_client: i32,
    viewer_eflags: i32,
    target_client: i32,
    target_eflags: i32,
) -> bool {
    japlus_cinfo & JAPLUS_CINFO_ALTDIM != 0
        && (0..64).contains(&target_client)
        && target_client != viewer_client
        && (target_eflags ^ viewer_eflags) & EF_ALT_DIM != 0
}

pub const STAT_RACEMODE: usize = 11;
pub const STAT_MOVEMENTSTYLE: usize = 13;

/// `CG_Player` culling and styling for another player. `bolt1` is the target's
/// entity `bolt1`: 0 FFA, 1 dueling, 2 racing (jaPRO).
pub fn classify(viewer: &StyleViewer, target_number: i32, bolt1: i32, dead: bool) -> Appearance {
    if target_number == viewer.client_num {
        return Appearance::NORMAL;
    }
    let bit = |mask: u32| viewer.style_player & mask != 0;
    let mut appearance = Appearance::NORMAL;

    if viewer.dueling {
        if target_number != viewer.duel_index {
            // TaystJK CG_Player: jp_DuelAlpha is a bystander-visibility rule,
            // not the translucency byte. JA+ ignores HIDE_NONDUELERS.
            let hide_non_dueler = (viewer.japlus
                && viewer.japlus_duel_alpha >= 0
                && viewer.plugin_disable & plugin_disable::DUEL_SEE_OTHERS == 0)
                || viewer.japro
                || (!viewer.japlus && bit(style::HIDE_NONDUELERS));
            if !viewer.free_spectator && hide_non_dueler {
                return Appearance::HIDDEN;
            }
            appearance.dim = true;
        }
        return appearance;
    }

    if viewer.japro && viewer.racemode {
        if bit(style::HIDE_DUELERS) && bolt1 == 1 {
            return Appearance::HIDDEN;
        }
        if bit(style::HIDE_RACERS_IF_RACER) && bolt1 == 2 {
            return Appearance::HIDDEN;
        }
        if bit(style::HIDE_NONRACERS_IF_RACER) {
            return Appearance::HIDDEN;
        }
        if !viewer.coop_race {
            if (bolt1 == 0 && !bit(style::NONRACER_VFX_DISABLE))
                || !bit(style::RACER_VFX_DISABLE)
            {
                appearance.ghost = Ghost::Race;
            }
            return appearance;
        }
        // TaystJK skips race presentation in MV_COOP_JKA but then proceeds
        // to normal FFA presentation, while retaining the racer culling gates.
    }

    if bit(style::HIDE_DUELERS) && bolt1 == 1 {
        return Appearance::HIDDEN;
    }
    if bit(style::HIDE_RACERS_FFA) && bolt1 == 2 {
        return Appearance::HIDDEN;
    }
    if bolt1 == 1 && !dead {
        if bit(style::VFX_DUELERS) {
            appearance.ghost = Ghost::Duel;
        } else {
            appearance.duel_bubble = true;
        }
    } else if viewer.japro && bolt1 == 2 && !bit(style::RACER_VFX_DISABLE) {
        appearance.ghost = Ghost::Race;
    }
    appearance
}

/// Constant colour of the ghost shaders' first stage (`rgbGen const`).
pub fn ghost_color(ghost: Ghost) -> [f32; 4] {
    match ghost {
        // TaystJK CG_Player sets shaderRGBA[3] to 50 and 100, respectively.
        Ghost::Race => [0.1, 0.1, 1.0, 50.0 / 255.0],
        Ghost::Duel => [0.1, 0.1, 0.4, 100.0 / 255.0],
        Ghost::None => [1.0; 4],
    }
}

pub fn ghost_shader(ghost: Ghost) -> Option<&'static str> {
    match ghost {
        Ghost::Race => Some("gfx/effects/raceShader"),
        Ghost::Duel => Some("gfx/effects/duelShader"),
        Ghost::None => None,
    }
}

// ------------------------------------------------------------ race timer --

/// One frame of `DF_RaceTimer` output, ready to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RaceTimerUi {
    /// Multi-line block drawn right-aligned at (`x`, `y`) in 640x480 space.
    pub timer_text: String,
    pub timer_x: f32,
    pub timer_y: f32,
    pub size: f32,
    /// "Start: N" readout, drawn right-aligned at its own position.
    pub start_text: String,
    pub start_x: f32,
    pub start_y: f32,
    pub start_color: [f32; 3],
}

/// Accumulates the per-run speed statistics `cg.startSpeed/maxSpeed/displacement`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RaceStats {
    start_speed: i32,
    max_speed: i32,
    displacement: i64,
    samples: i64,
    last_time: i32,
}

impl RaceStats {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advance one cgame frame. `elapsed_ms` is `cg.time - ps.duelTime`,
    /// `speed` the horizontal speed. Returns the formatted UI, or `None` when
    /// the timer should not be shown.
    pub fn frame(
        &mut self,
        settings: &JaproCgame,
        racemode: bool,
        duel_time: i32,
        now_ms: i32,
        speed: f32,
        move_speed: f32,
    ) -> Option<RaceTimerUi> {
        if !racemode || duel_time == 0 || (settings.race_timer == 0 && !settings.race_start) {
            self.reset();
            return None;
        }
        let elapsed = now_ms - duel_time;
        if elapsed < self.last_time {
            self.reset();
        }
        if settings.race_timer > 1 || settings.race_start {
            if elapsed > 0 {
                if self.start_speed == 0 {
                    self.start_speed = speed as i32;
                }
                if speed > self.max_speed as f32 {
                    self.max_speed = speed as i32;
                }
                self.displacement += speed as i64;
                self.samples += 1;
            }
        }
        self.last_time = elapsed;

        let mut ui = RaceTimerUi {
            timer_x: settings.race_timer_x,
            timer_y: settings.race_timer_y,
            size: settings.race_timer_size,
            start_x: settings.race_start_x,
            start_y: settings.race_start_y,
            start_color: [1.0; 3],
            ..Default::default()
        };
        if settings.race_timer != 0 {
            let minutes = elapsed / 1000 / 60;
            let seconds = elapsed / 1000 % 60;
            let millis = elapsed % 1000;
            ui.timer_text = if settings.race_timer < 3 {
                format!("{minutes}:{seconds:02}.{}", millis / 100)
            } else {
                format!("{minutes}:{seconds:02}.{millis:03}")
            };
            if settings.race_timer > 1 {
                if self.samples > 0 {
                    let _ = write!(
                        ui.timer_text,
                        "\nMax: {}\nAvg: {}",
                        (self.max_speed as f32 + 0.5) as i32,
                        self.displacement / self.samples
                    );
                }
            }
        }
        if settings.race_start {
            if settings.start_goal > 0.0 && settings.start_goal <= self.start_speed as f32 {
                let ratio = self.start_speed as f32 / move_speed.max(1.0);
                let green = 1.0 / (ratio * ratio);
                ui.start_color = [green.min(1.0), 1.0, green.min(1.0)];
            }
            ui.start_text = format!("Start: {}", self.start_speed);
        }
        Some(ui)
    }
}

// ------------------------------------------------------------- spectator --

/// Debounced "follow the fastest player" for spectators. Picking the single
/// fastest player every frame would flip between two similar speeds; instead a
/// challenger must be clearly faster than the current target and stay ahead
/// for a short hold, and switches are rate limited.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FollowFastest {
    challenger: Option<i32>,
    challenger_since_ms: i32,
    last_switch_ms: i32,
}

/// The challenger must beat the followed player's speed by this factor...
const FOLLOW_MARGIN: f32 = 1.25;
/// ...and by at least this many units per second...
const FOLLOW_MIN_LEAD: f32 = 60.0;
/// ...continuously for this long...
const FOLLOW_HOLD_MS: i32 = 800;
/// ...and no more than one switch per this long.
const FOLLOW_MIN_INTERVAL_MS: i32 = 2500;
/// Ignore players below this speed: nobody is worth chasing while standing still.
const FOLLOW_MIN_SPEED: f32 = 150.0;

impl FollowFastest {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `players`: `(client number, speed)` for everyone but the followed player;
    /// `current`: the followed client and its speed (`None` while free-flying).
    /// Returns the client to `follow` when a switch is due.
    pub fn update(
        &mut self,
        now_ms: i32,
        current: Option<(i32, f32)>,
        players: &[(i32, f32)],
    ) -> Option<i32> {
        let best = players
            .iter()
            .copied()
            .filter(|&(_, speed)| speed >= FOLLOW_MIN_SPEED)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let Some((best_client, best_speed)) = best else {
            self.challenger = None;
            return None;
        };
        let beats_current = match current {
            None => true,
            Some((client, _)) if client == best_client => false,
            Some((_, speed)) => best_speed >= speed * FOLLOW_MARGIN && best_speed - speed >= FOLLOW_MIN_LEAD,
        };
        if !beats_current {
            self.challenger = None;
            return None;
        }
        if self.challenger != Some(best_client) {
            self.challenger = Some(best_client);
            self.challenger_since_ms = now_ms;
            return None;
        }
        let held = now_ms - self.challenger_since_ms >= if current.is_none() { 0 } else { FOLLOW_HOLD_MS };
        let cooled = now_ms - self.last_switch_ms >= FOLLOW_MIN_INTERVAL_MS;
        if held && cooled {
            self.last_switch_ms = now_ms;
            self.challenger = None;
            return Some(best_client);
        }
        None
    }
}

// ------------------------------------------------------------- cosmetics --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosmeticSlot {
    /// Bolted to `*head_top`.
    Hat,
    /// Bolted to `*back`.
    Back,
}

#[derive(Debug, Clone, Copy)]
pub struct Cosmetic {
    pub bit: u32,
    pub label: &'static str,
    pub model: &'static str,
    pub slot: CosmeticSlot,
    /// Draw-time group: items in the same group are mutually exclusive, jaPRO
    /// draws the first set bit in table order. The goose has its own group here
    /// because `CG_Player` draws it alongside a cape, even though the `cosmetics`
    /// menu command treats it as one of the capes (see [`Cosmetic::menu_group`]).
    pub exclusive_group: u8,
}

impl Cosmetic {
    /// The radio group of `cg_consolecmds.c CG_Cosmetics_JaPRO`: 1 = hats,
    /// 2 = masks, 3 = capes (including the goose). Picking one item clears the
    /// rest of its group.
    pub const fn menu_group(&self) -> u8 {
        if self.exclusive_group == 4 { 3 } else { self.exclusive_group }
    }
}

/// Every cosmetic in `group`, in menu order.
pub fn cosmetics_in_group(group: u8) -> impl Iterator<Item = &'static Cosmetic> {
    COSMETICS.iter().filter(move |item| item.menu_group() == group)
}

/// `cp_cosmetics` after picking `bit` in the menu: like the `cosmetics <n>`
/// command it clears the rest of the item's group and toggles the item. `None`
/// is passed as `bit` to empty a whole group.
pub fn toggle_cosmetic(mask: u32, group: u8, bit: Option<u32>) -> u32 {
    let group_bits = cosmetics_in_group(group).fold(0u32, |bits, item| bits | item.bit);
    match bit {
        Some(bit) if mask & bit == 0 => (mask & !group_bits) | bit,
        _ => mask & !group_bits,
    }
}

const fn hat(bit: u32, label: &'static str, model: &'static str, group: u8) -> Cosmetic {
    Cosmetic { bit, label, model, slot: CosmeticSlot::Hat, exclusive_group: group }
}

const fn back(bit: u32, label: &'static str, model: &'static str, group: u8) -> Cosmetic {
    Cosmetic { bit, label, model, slot: CosmeticSlot::Back, exclusive_group: group }
}

/// jaPRO `cp_cosmetics` bits in the menu order of `cg_consolecmds.c cosmetics[]`.
/// Draw group 1 = head slot (one hat), 2 = face slot (mask/beard/glasses),
/// 3 = back slot (one cape/held item), 4 = the goose (drawn alongside a cape).
pub const COSMETICS: &[Cosmetic] = &[
    hat(1 << 0, "Santa hat", "models/cosmetics/hats/santahat.md3", 1),
    hat(1 << 1, "Jack-o'-lantern", "models/cosmetics/hats/pumpkin.md3", 1),
    hat(1 << 2, "Baseball cap", "models/cosmetics/hats/cap.md3", 1),
    hat(1 << 3, "Indiana Jones", "models/cosmetics/hats/fedora.md3", 1),
    hat(1 << 4, "Kringe Kap", "models/cosmetics/hats/cringe.md3", 1),
    hat(1 << 5, "Sombrero", "models/cosmetics/hats/sombrero.md3", 1),
    hat(1 << 6, "Top hat", "models/cosmetics/hats/tophat.md3", 1),
    hat(1 << 7, "Mask", "models/cosmetics/hats/mask.md3", 2),
    hat(1 << 8, "Graduation cap", "models/cosmetics/hats/gradcap.md3", 1),
    back(1 << 9, "Goose", "models/cosmetics/capes/goose.md3", 4),
    hat(1 << 10, "Black fedora", "models/cosmetics/hats/fedora2.md3", 1),
    hat(1 << 11, "Blue fedora", "models/cosmetics/hats/fedora3.md3", 1),
    hat(1 << 12, "Pimp hat", "models/cosmetics/hats/fedora4.md3", 1),
    hat(1 << 13, "Headcrab", "models/cosmetics/hats/headcrab.md3", 1),
    back(1 << 14, "Vader cape", "models/cosmetics/capes/vadercape.md3", 3),
    back(1 << 15, "Shoulder Yoda", "models/cosmetics/capes/yodacape.md3", 3),
    hat(1 << 16, "Horns", "models/cosmetics/hats/horns.md3", 1),
    hat(1 << 17, "Metal helm", "models/cosmetics/hats/metalhelm.md3", 1),
    hat(1 << 18, "Afro", "models/cosmetics/hats/afro.md3", 1),
    back(1 << 19, "AK-47", "models/cosmetics/capes/ak47.md3", 3),
    hat(1 << 20, "Bucket", "models/cosmetics/hats/bucket.md3", 1),
    back(1 << 21, "Crowbar", "models/cosmetics/capes/crowbar.md3", 3),
    hat(1 << 22, "Crown", "models/cosmetics/hats/crown.md3", 1),
    back(1 << 23, "Royal cape", "models/cosmetics/capes/royalcape.md3", 3),
    hat(1 << 24, "Beard", "models/cosmetics/hats/beard.md3", 2),
    back(1 << 25, "Grogu", "models/cosmetics/capes/grogucape.md3", 3),
    hat(1 << 26, "Plague mask", "models/cosmetics/hats/plaguemask.md3", 2),
    hat(1 << 27, "Glasses", "models/cosmetics/hats/glasses.md3", 2),
    hat(1 << 28, "Mario cap", "models/cosmetics/hats/mario.md3", 1),
    back(1 << 29, "Rocket launcher", "models/cosmetics/capes/rpg.md3", 3),
    hat(1 << 30, "Predator helm", "models/cosmetics/hats/predatorhelm.md3", 1),
    hat(1 << 31, "Super Saiyan", "models/cosmetics/hats/supersaiyan.md3", 1),
];

/// The cosmetics jaPRO would actually draw for a `c5`/`cp_cosmetics` bit mask:
/// the first set bit of each exclusive group, in table order.
pub fn cosmetics_to_draw(mask: u32) -> Vec<&'static Cosmetic> {
    let mut taken = 0u32;
    let mut out = Vec::new();
    for item in COSMETICS {
        if mask & item.bit == 0 || taken & (1 << item.exclusive_group) != 0 {
            continue;
        }
        taken |= 1 << item.exclusive_group;
        out.push(item);
    }
    out
}

/// jaPRO's `JAPRO_STYLE_SEASONALCOSMETICS`: a free santa hat around Christmas and
/// a pumpkin on Halloween for players who wear nothing themselves.
pub fn seasonal_cosmetic(month0: u32, day: u32) -> Option<u32> {
    let christmas = (month0 == 10 && day > 21) || month0 == 11 || (month0 == 0 && day < 8);
    if christmas {
        Some(1 << 0)
    } else if month0 == 9 && day == 31 {
        Some(1 << 1)
    } else {
        None
    }
}

/// (0-based month, day of month) of a Unix timestamp in UTC.
pub fn month_day_utc(unix_secs: i64) -> (u32, u32) {
    // Howard Hinnant's civil_from_days.
    let z = unix_secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let _ = era;
    (month - 1, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer() -> StyleViewer {
        StyleViewer { japro: true, client_num: 0, ..StyleViewer::default() }
    }

    #[test]
    fn japro_plugin_disable_table_matches_taystjk_bits() {
        let bits: Vec<u8> = JAPRO_PLUGIN_DISABLE_OPTIONS.iter().map(|option| option.bit).collect();
        assert_eq!(bits, vec![3, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28]);
        let mut seen = 0i32;
        for option in JAPRO_PLUGIN_DISABLE_OPTIONS {
            assert_eq!(seen & option.mask(), 0, "duplicate plugin-disable bit {}", option.bit);
            seen |= option.mask();
            assert_eq!(plugin_disable_option(option.bit), Some(option));
        }
    }

    #[test]
    fn amrun_matches_taystjk_xor_and_mask_behavior() {
        assert_eq!(toggle_run_animation(0), plugin_disable::JAWA_RUN);
        assert_eq!(toggle_run_animation(plugin_disable::JAWA_RUN), 0);

        let other = plugin_disable::NO_ROLL | plugin_disable::CHATBOX_CP;
        assert_eq!(toggle_run_animation(other), other | plugin_disable::JAWA_RUN);
        assert_eq!(
            toggle_run_animation(other | plugin_disable::JAWA_RUN),
            other
        );

        // CG_AmRun_f masks cp_pluginDisable before XORing the Jawa-run bit.
        assert_eq!(toggle_run_animation(1i32 << 30), plugin_disable::JAWA_RUN);
    }

    #[test]
    fn default_ffa_dueler_is_dark_with_a_bubble_and_racer_is_a_ghost() {
        let v = viewer();
        let dueler = classify(&v, 3, 1, false);
        assert!(dueler.visible && dueler.duel_bubble && dueler.ghost == Ghost::None);
        let racer = classify(&v, 4, 2, false);
        assert_eq!(racer.ghost, Ghost::Race);
    }

    #[test]
    fn vfx_duelers_switches_the_bubble_for_a_ghost() {
        let v = StyleViewer { style_player: style::VFX_DUELERS, ..viewer() };
        let dueler = classify(&v, 3, 1, false);
        assert_eq!(dueler.ghost, Ghost::Duel);
        assert!(!dueler.duel_bubble);
    }

    #[test]
    fn hide_bits_cull_duelers_and_racers() {
        let v = StyleViewer { style_player: style::HIDE_DUELERS | style::HIDE_RACERS_FFA, ..viewer() };
        assert!(!classify(&v, 3, 1, false).visible);
        assert!(!classify(&v, 4, 2, false).visible);
        assert!(classify(&v, 5, 0, false).visible);
    }

    #[test]
    fn racing_viewer_ghosts_everyone_unless_disabled() {
        let v = StyleViewer { racemode: true, ..viewer() };
        assert_eq!(classify(&v, 3, 0, false).ghost, Ghost::Race);
        assert_eq!(classify(&v, 4, 2, false).ghost, Ghost::Race);
        let plain = StyleViewer {
            racemode: true,
            style_player: style::RACER_VFX_DISABLE | style::NONRACER_VFX_DISABLE,
            ..viewer()
        };
        assert_eq!(classify(&plain, 3, 0, false).ghost, Ghost::None);
        assert_eq!(classify(&plain, 4, 2, false).ghost, Ghost::None);
    }

    #[test]
    fn racing_viewer_hide_bits() {
        let v = StyleViewer { racemode: true, style_player: style::HIDE_RACERS_IF_RACER, ..viewer() };
        assert!(!classify(&v, 4, 2, false).visible);
        assert!(classify(&v, 3, 0, false).visible);
        let all = StyleViewer { racemode: true, style_player: style::HIDE_NONRACERS_IF_RACER, ..viewer() };
        assert!(!classify(&all, 3, 0, false).visible);
    }

    #[test]
    fn coop_uses_ordinary_ffa_presentation() {
        let v = StyleViewer { racemode: true, coop_race: true, ..viewer() };
        assert_eq!(classify(&v, 3, 2, false).ghost, Ghost::Race);
        assert_eq!(classify(&v, 4, 0, false).ghost, Ghost::None);
    }

    #[test]
    fn dueling_viewer_hides_bystanders_on_japro_and_dims_elsewhere() {
        let v = StyleViewer { dueling: true, duel_index: 7, ..viewer() };
        assert!(!classify(&v, 3, 0, false).visible);
        assert_eq!(classify(&v, 7, 1, false), Appearance::NORMAL);
        let stock = StyleViewer { japro: false, dueling: true, duel_index: 7, ..viewer() };
        let bystander = classify(&stock, 3, 0, false);
        assert!(bystander.visible && bystander.dim);
        let stock_hide = StyleViewer { style_player: style::HIDE_NONDUELERS, ..stock };
        assert!(!classify(&stock_hide, 3, 0, false).visible);
        let spectator = StyleViewer { free_spectator: true, ..v };
        assert!(classify(&spectator, 3, 0, false).visible);
    }

    #[test]
    fn dead_duelers_get_no_bubble() {
        assert!(!classify(&viewer(), 3, 1, true).duel_bubble);
    }

    #[test]
    fn the_viewer_is_never_restyled() {
        let v = StyleViewer { racemode: true, style_player: style::HIDE_NONRACERS_IF_RACER, ..viewer() };
        assert_eq!(classify(&v, 0, 0, false), Appearance::NORMAL);
    }

    #[test]
    fn taystjk_ja_plus_alt_dimension_gate() {
        let bit = JAPLUS_CINFO_ALTDIM;
        assert!(ja_plus_other_dimension(bit, 2, 0, 3, EF_ALT_DIM));
        assert!(ja_plus_other_dimension(bit, 2, EF_ALT_DIM, 3, 0));
        assert!(!ja_plus_other_dimension(bit, 2, EF_ALT_DIM, 3, EF_ALT_DIM));
        assert!(!ja_plus_other_dimension(0, 2, 0, 3, EF_ALT_DIM));
        assert!(!ja_plus_other_dimension(bit, 2, 0, 2, EF_ALT_DIM));
        assert!(!ja_plus_other_dimension(bit, 2, 0, 64, EF_ALT_DIM));
    }

    #[test]
    fn taystjk_duel_alpha_and_plugin_visibility() {
        let ja = StyleViewer {
            japro: false, japlus: true, dueling: true, duel_index: 7,
            client_num: 2, japlus_duel_alpha: 0,
            ..StyleViewer::default()
        };
        assert!(!classify(&ja, 8, 0, false).visible);
        assert!(classify(&ja, 7, 1, false).visible);
        let neg = StyleViewer { japlus_duel_alpha: -1, ..ja };
        let look = classify(&neg, 8, 0, false);
        assert!(look.visible && look.dim);
        let allow = StyleViewer { plugin_disable: plugin_disable::DUEL_SEE_OTHERS, ..ja };
        assert!(classify(&allow, 8, 0, false).visible);
        let style = StyleViewer { style_player: style::HIDE_NONDUELERS, ..neg };
        assert!(classify(&style, 8, 0, false).visible);
        assert!(classify(&StyleViewer { free_spectator: true, ..ja }, 8, 0, false).visible);
        assert!(!classify(&StyleViewer { japro: true, japlus: false, ..ja }, 8, 0, false).visible);
    }

    #[test]
    fn taystjk_ghost_alpha_bytes() {
        assert_eq!(ghost_color(Ghost::Race)[3], 50.0 / 255.0);
        assert_eq!(ghost_color(Ghost::Duel)[3], 100.0 / 255.0);
        assert_eq!(JaproCgame::default().alt_dim_alpha, 40);
    }

    #[test]
    fn cvars_round_trip() {
        let mut settings = JaproCgame::default();
        settings.set_cvar("cg_stylePlayer", "260").unwrap().unwrap();
        settings.set_cvar("cg_raceTimer", "9").unwrap().unwrap();
        settings.set_cvar("cg_raceTimerY", "300.5").unwrap().unwrap();
        assert!(settings.style_bit(style::VFX_DUELERS) && settings.style_bit(style::HIDE_DUELERS));
        assert_eq!(settings.race_timer, 3);
        assert_eq!(settings.cvar_value("CG_RACETIMERY").as_deref(), Some("300.5"));
        assert!(settings.set_cvar("cg_stylePlayer", "abc").unwrap().is_err());
        assert!(settings.set_cvar("nope", "1").is_none());
        settings.set_cvar("cg_lagometer", "2").unwrap().unwrap();
        settings.set_cvar("cg_lagometerX", "60").unwrap().unwrap();
        assert_eq!(settings.lagometer.mode, 2);
        assert_eq!(settings.cvar_value("cg_lagometerX").as_deref(), Some("60"));
        assert_eq!(settings.cvar_value("cg_lagometerY").as_deref(), Some("144"));
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("auto"));
        settings.set_cvar("cl_commandsize", "128").unwrap().unwrap();
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("128"));
        settings.set_cvar("cl_commandsize", "100").unwrap().unwrap();
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("128"));
        settings.set_cvar("cl_commandsize", "300").unwrap().unwrap();
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("512"));
        settings.set_cvar("cl_commandsize", "12").unwrap().unwrap();
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("64"));
        settings.set_cvar("cl_commandsize", "AUTO").unwrap().unwrap();
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("auto"));
        settings.set_cvar("cg_lagometer", "4").unwrap().unwrap();
        assert_eq!(settings.lagometer.mode, 4);
        assert_eq!(settings.drain_fx, 2, "jaPRO's default is the JaPRO drain effect, not stock");
        settings.set_cvar("cg_drainFX", "1").unwrap().unwrap();
        assert_eq!(settings.drain_fx, 1);
        assert_eq!(settings.cvar_value("cg_drainfx").as_deref(), Some("1"));
        settings.set_cvar("cg_drainFX", "9").unwrap().unwrap();
        assert_eq!(settings.drain_fx, 2, "clamps to the documented 0..2 range");
        let mut text = String::new();
        settings.write_cfg(&mut text);
        assert!(text.contains("seta cg_stylePlayer \"260\""));
        assert!(text.contains("seta cg_lagometer \"4\"") && text.contains("seta cg_lagometerX \"60\""));
        assert!(text.contains("seta cg_drainFX \"2\""));
    }

    #[test]
    fn render_to_texture_fx_preserves_taystjk_modes_and_new_predator_mode() {
        let mut settings = JaproCgame::default();
        assert_eq!(settings.render_to_texture_fx, 1);
        settings.set_cvar("cg_renderToTextureFX", "0").unwrap().unwrap();
        assert_eq!(settings.render_to_texture_fx, 0);
        assert_eq!(settings.cvar_value("CG_RENDERTOTEXTUREFX").as_deref(), Some("0"));
        let mut text = String::new();
        settings.write_cfg(&mut text);
        assert!(text.contains("seta cg_renderToTextureFX \"0\""));
        settings.set_cvar("cg_renderToTextureFX", "2").unwrap().unwrap();
        assert_eq!(settings.render_to_texture_fx, 2);
        assert_eq!(settings.cvar_value("cg_renderToTextureFX").as_deref(), Some("2"));
        settings.set_cvar("cg_renderToTextureFX", "-1").unwrap().unwrap();
        assert_eq!(settings.render_to_texture_fx, 1);
        for (value, expected) in [("3", 3u8), ("4", 4), ("5", 5)] {
            settings.set_cvar("cg_renderToTextureFX", value).unwrap().unwrap();
            assert_eq!(settings.render_to_texture_fx, expected);
            assert_eq!(settings.cvar_value("cg_renderToTextureFX").as_deref(), Some(value));
        }
        assert!(settings.set_cvar("cg_renderToTextureFX", "6").unwrap().is_err());
        assert!(settings.set_cvar("cg_renderToTextureFX", "oops").unwrap().is_err());
        assert_eq!(settings.render_to_texture_fx, 5);
    }

    #[test]
    fn race_timer_formats_like_japro() {
        let mut settings = JaproCgame::default();
        settings.race_timer = 3;
        let mut stats = RaceStats::default();
        let ui = stats.frame(&settings, true, 1000, 66_061, 640.0, 250.0).unwrap();
        assert!(ui.timer_text.starts_with("1:05.061"), "{}", ui.timer_text);
        assert!(ui.timer_text.contains("Max: 640") && ui.timer_text.contains("Avg: 640"));
        settings.race_timer = 1;
        let ui = stats.frame(&settings, true, 1000, 66_061, 640.0, 250.0).unwrap();
        assert_eq!(ui.timer_text, "1:05.0");
        assert!(stats.frame(&settings, false, 1000, 66_061, 640.0, 250.0).is_none());
        assert!(stats.frame(&settings, true, 0, 66_061, 640.0, 250.0).is_none());
    }

    #[test]
    fn race_timer_respects_the_separate_start_speed_selection() {
        let mut settings = JaproCgame::default();
        let mut stats = RaceStats::default();
        let early = stats.frame(&settings, true, 10_000, 11_000, 480.0, 250.0).unwrap();
        assert!(!early.timer_text.contains("Start"), "{}", early.timer_text);
        assert!(early.start_text.is_empty());
        settings.race_start = true;
        let enabled = stats.frame(&settings, true, 10_000, 11_100, 500.0, 250.0).unwrap();
        assert!(!enabled.timer_text.contains("Start"));
        assert_eq!(enabled.start_text, "Start: 480");
        let late = stats.frame(&settings, true, 10_000, 14_000, 700.0, 250.0).unwrap();
        assert!(!late.timer_text.contains("Start"));
        assert!(late.timer_text.contains("Max: 700"));
    }

    #[test]
    fn race_stats_reset_when_a_new_run_begins() {
        let settings = JaproCgame::default();
        let mut stats = RaceStats::default();
        stats.frame(&settings, true, 10_000, 15_000, 900.0, 250.0);
        let fresh = stats.frame(&settings, true, 20_000, 20_500, 300.0, 250.0).unwrap();
        assert!(fresh.timer_text.contains("Max: 300"), "{}", fresh.timer_text);
    }

    #[test]
    fn start_readout_turns_green_at_the_goal() {
        let mut settings = JaproCgame::default();
        settings.race_start = true;
        settings.start_goal = 500.0;
        let mut stats = RaceStats::default();
        let ui = stats.frame(&settings, true, 1000, 2000, 600.0, 250.0).unwrap();
        assert_eq!(ui.start_text, "Start: 600");
        assert!(ui.start_color[0] < 1.0 && ui.start_color[1] == 1.0);
    }

    #[test]
    fn follow_fastest_waits_and_then_switches_once() {
        let mut follow = FollowFastest::default();
        let players = [(2, 900.0), (3, 300.0)];
        assert_eq!(follow.update(10_000, Some((3, 300.0)), &players), None);
        assert_eq!(follow.update(10_400, Some((3, 300.0)), &players), None);
        assert_eq!(follow.update(10_900, Some((3, 300.0)), &players), Some(2));
        // Just switched: nothing more until the interval passes.
        assert_eq!(follow.update(11_000, Some((2, 900.0)), &[(3, 1400.0)]), None);
        assert_eq!(follow.update(11_900, Some((2, 900.0)), &[(3, 1400.0)]), None);
    }

    #[test]
    fn follow_fastest_ignores_marginal_and_flickering_leads() {
        let mut follow = FollowFastest::default();
        // 10% faster is not worth a camera cut.
        assert_eq!(follow.update(0, Some((3, 500.0)), &[(2, 550.0)]), None);
        assert_eq!(follow.update(5_000, Some((3, 500.0)), &[(2, 550.0)]), None);
        // A burst that ends before the hold time never switches.
        assert_eq!(follow.update(6_000, Some((3, 300.0)), &[(2, 900.0)]), None);
        assert_eq!(follow.update(6_300, Some((3, 900.0)), &[(2, 900.0)]), None);
        assert_eq!(follow.update(7_500, Some((3, 300.0)), &[(2, 900.0)]), None);
    }

    #[test]
    fn follow_fastest_picks_a_target_immediately_when_free_flying() {
        let mut follow = FollowFastest::default();
        assert_eq!(follow.update(5_000, None, &[(4, 700.0), (2, 200.0)]), None);
        assert_eq!(follow.update(5_050, None, &[(4, 700.0), (2, 200.0)]), Some(4));
        // Standing players are never worth chasing.
        assert_eq!(FollowFastest::default().update(9_000, None, &[(1, 10.0)]), None);
    }

    #[test]
    fn cosmetics_keep_one_hat_but_combine_slots() {
        let mask = (1 << 0) | (1 << 3) | (1 << 7) | (1 << 9) | (1 << 14) | (1 << 19);
        let drawn: Vec<_> = cosmetics_to_draw(mask).iter().map(|c| c.label).collect();
        assert_eq!(drawn, ["Santa hat", "Mask", "Goose", "Vader cape"]);
    }

    #[test]
    fn menu_groups_match_the_native_cosmetics_command() {
        // Bit masks from cg_consolecmds.c CG_Cosmetics_JaPRO.
        let bits = |group| cosmetics_in_group(group).fold(0u32, |all, item| all | item.bit);
        let hats = [0, 1, 2, 3, 4, 5, 6, 8, 10, 11, 12, 13, 16, 17, 18, 20, 22, 28, 30, 31];
        let masks = [7, 24, 26, 27];
        let capes = [9, 14, 15, 19, 21, 23, 25, 29];
        let native = |list: &[u32]| list.iter().fold(0u32, |all, bit| all | (1 << bit));
        assert_eq!(bits(1), native(&hats));
        assert_eq!(bits(2), native(&masks));
        assert_eq!(bits(3), native(&capes));
    }

    #[test]
    fn picking_a_cosmetic_clears_its_group_and_toggles_off() {
        let goose = 1 << 9;
        let vader = 1 << 14;
        let santa = 1 << 0;
        let mask = santa | goose;
        // A cape replaces the goose (native radio group) and leaves the hat alone.
        assert_eq!(toggle_cosmetic(mask, 3, Some(vader)), santa | vader);
        // Clicking the worn item takes it off again.
        assert_eq!(toggle_cosmetic(mask, 3, Some(goose)), santa);
        assert_eq!(toggle_cosmetic(mask, 3, None), santa);
    }

    #[test]
    fn seasonal_cosmetics_follow_the_calendar() {
        assert_eq!(seasonal_cosmetic(11, 25), Some(1));
        assert_eq!(seasonal_cosmetic(0, 3), Some(1));
        assert_eq!(seasonal_cosmetic(9, 31), Some(2));
        assert_eq!(seasonal_cosmetic(5, 10), None);
    }

    #[test]
    fn month_day_matches_known_dates() {
        // 2024-02-29 12:00 UTC, 2025-12-25 00:00 UTC, 2026-10-31 23:59 UTC.
        assert_eq!(month_day_utc(1_709_208_000), (1, 29));
        assert_eq!(month_day_utc(1_766_620_800), (11, 25));
        assert_eq!(month_day_utc(1_793_491_140), (9, 31));
        assert_eq!(month_day_utc(0), (0, 1));
    }

    #[test]
    fn every_cosmetic_has_a_unique_bit() {
        let mut seen = 0u32;
        for item in COSMETICS {
            assert_eq!(seen & item.bit, 0, "{} repeats a bit", item.label);
            seen |= item.bit;
        }
        assert_eq!(COSMETICS.len(), 32);
    }
}
