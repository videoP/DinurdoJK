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

/// One jaPRO plugin-disable preference shown by both `/plugin` and the MOD page.
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
    /// Base/JA+: hide everyone but your opponent while you duel.
    pub const HIDE_NONDUELERS: u32 = 1 << 10;
    pub const HIDE_COSMETICS: u32 = 1 << 16;
    pub const SEASONAL_COSMETICS: u32 = 1 << 20;
}

/// Bit, menu label, tooltip for the `cg_stylePlayer` switches DinurdoJK acts on.
pub const STYLE_TOGGLES: &[(u32, &str, &str)] = &[
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
    /// `cg_stylePlayer` bit mask (see [`style`]).
    pub style_player: u32,
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
}

impl Default for JaproCgame {
    fn default() -> Self {
        Self {
            style_player: 0,
            race_timer: 2,
            race_timer_size: 0.75,
            race_timer_x: 5.0,
            race_timer_y: 280.0,
            race_start: false,
            race_start_x: 300.0,
            race_start_y: 280.0,
            start_goal: 0.0,
            follow_fastest: false,
            speedometer: crate::speedometer::Settings::default(),
            lagometer: crate::lagometer::Settings::default(),
            drain_fx: 2,
        }
    }
}

impl JaproCgame {
    pub fn style_bit(&self, bit: u32) -> bool {
        self.style_player & bit != 0
    }

    pub fn cvar_value(&self, name: &str) -> Option<String> {
        Some(match name.to_ascii_lowercase().as_str() {
            "cg_styleplayer" => self.style_player.to_string(),
            "cg_racetimer" => self.race_timer.to_string(),
            "cg_racetimersize" => format!("{}", self.race_timer_size),
            "cg_racetimerx" => format!("{}", self.race_timer_x),
            "cg_racetimery" => format!("{}", self.race_timer_y),
            "cg_racestart" => u8::from(self.race_start).to_string(),
            "cg_racestartx" => format!("{}", self.race_start_x),
            "cg_racestarty" => format!("{}", self.race_start_y),
            "cg_startgoal" => format!("{}", self.start_goal),
            "cg_specfollowfastest" => u8::from(self.follow_fastest).to_string(),
            "cg_speedometer" => self.speedometer.flags.to_string(),
            "cg_speedometerx" => format!("{}", self.speedometer.x),
            "cg_speedometery" => format!("{}", self.speedometer.y),
            "cg_speedometersize" => format!("{}", self.speedometer.size),
            "cg_speedometerjumps" => self.speedometer.jumps.to_string(),
            "cg_speedometerjumpsx" => format!("{}", self.speedometer.jumps_x),
            "cg_speedometerjumpsy" => format!("{}", self.speedometer.jumps_y),
            "cg_jumpgoal" => format!("{}", self.speedometer.jump_goal),
            "cg_lagometer" => self.lagometer.mode.to_string(),
            "cg_lagometerx" => self.lagometer.x.to_string(),
            "cg_lagometery" => self.lagometer.y.to_string(),
            "cl_commandsize" => self.lagometer.command_size.to_string(),
            "cg_drainfx" => self.drain_fx.to_string(),
            _ => return None,
        })
    }

    /// `None` when `name` is not one of ours; otherwise whether parsing succeeded.
    pub fn set_cvar(&mut self, name: &str, value: &str) -> Option<Result<(), String>> {
        let integer = || {
            value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .map(|number| number as i64)
                .ok_or_else(|| format!("{name}: expected a number"))
        };
        let float = || {
            value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| format!("{name}: expected a number"))
        };
        let boolean = || integer().map(|number| number != 0);
        let result = match name.to_ascii_lowercase().as_str() {
            "cg_styleplayer" => integer().map(|v| self.style_player = v.clamp(0, i64::from(i32::MAX)) as u32),
            "cg_racetimer" => integer().map(|v| self.race_timer = v.clamp(0, 3) as u8),
            "cg_racetimersize" => float().map(|v| self.race_timer_size = v.clamp(0.1, 3.0)),
            "cg_racetimerx" => float().map(|v| self.race_timer_x = v.clamp(-640.0, 1280.0)),
            "cg_racetimery" => float().map(|v| self.race_timer_y = v.clamp(-480.0, 960.0)),
            "cg_racestart" => boolean().map(|v| self.race_start = v),
            "cg_racestartx" => float().map(|v| self.race_start_x = v.clamp(-640.0, 1280.0)),
            "cg_racestarty" => float().map(|v| self.race_start_y = v.clamp(-480.0, 960.0)),
            "cg_startgoal" => float().map(|v| self.start_goal = v.max(0.0)),
            "cg_specfollowfastest" => boolean().map(|v| self.follow_fastest = v),
            "cg_speedometer" => integer().map(|v| self.speedometer.flags = v.clamp(0, i64::from(i32::MAX)) as u32),
            "cg_speedometerx" => float().map(|v| self.speedometer.x = v.clamp(-640.0, 1280.0)),
            "cg_speedometery" => float().map(|v| self.speedometer.y = v.clamp(-480.0, 960.0)),
            "cg_speedometersize" => float().map(|v| self.speedometer.size = v.clamp(0.1, 3.0)),
            "cg_speedometerjumps" => integer().map(|v| self.speedometer.jumps = v.clamp(0, 511) as i32),
            "cg_speedometerjumpsx" => float().map(|v| self.speedometer.jumps_x = v.clamp(-640.0, 1280.0)),
            "cg_speedometerjumpsy" => float().map(|v| self.speedometer.jumps_y = v.clamp(-480.0, 960.0)),
            "cg_jumpgoal" => float().map(|v| self.speedometer.jump_goal = v.max(0.0)),
            // Plain integer cvars in jaPRO: any value is accepted.
            "cg_lagometer" => integer().map(|v| self.lagometer.mode = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32),
            "cg_lagometerx" => integer().map(|v| self.lagometer.x = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32),
            "cg_lagometery" => integer().map(|v| self.lagometer.y = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32),
            "cl_commandsize" => integer().map(|v| self.lagometer.command_size = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32),
            "cg_drainfx" => integer().map(|v| self.drain_fx = v.clamp(0, 2) as u8),
            _ => return None,
        };
        Some(result)
    }

    pub fn write_cfg(&self, out: &mut String) {
        let _ = writeln!(out, "seta cg_stylePlayer \"{}\"", self.style_player);
        let _ = writeln!(out, "seta cg_raceTimer \"{}\"", self.race_timer);
        let _ = writeln!(out, "seta cg_raceTimerSize \"{}\"", self.race_timer_size);
        let _ = writeln!(out, "seta cg_raceTimerX \"{}\"", self.race_timer_x);
        let _ = writeln!(out, "seta cg_raceTimerY \"{}\"", self.race_timer_y);
        let _ = writeln!(out, "seta cg_raceStart \"{}\"", u8::from(self.race_start));
        let _ = writeln!(out, "seta cg_raceStartX \"{}\"", self.race_start_x);
        let _ = writeln!(out, "seta cg_raceStartY \"{}\"", self.race_start_y);
        let _ = writeln!(out, "seta cg_startGoal \"{}\"", self.start_goal);
        let _ = writeln!(out, "seta cg_specFollowFastest \"{}\"", u8::from(self.follow_fastest));
        let speedometer = &self.speedometer;
        let _ = writeln!(out, "seta cg_speedometer \"{}\"", speedometer.flags);
        let _ = writeln!(out, "seta cg_speedometerX \"{}\"", speedometer.x);
        let _ = writeln!(out, "seta cg_speedometerY \"{}\"", speedometer.y);
        let _ = writeln!(out, "seta cg_speedometerSize \"{}\"", speedometer.size);
        let _ = writeln!(out, "seta cg_speedometerJumps \"{}\"", speedometer.jumps);
        let _ = writeln!(out, "seta cg_speedometerJumpsX \"{}\"", speedometer.jumps_x);
        let _ = writeln!(out, "seta cg_speedometerJumpsY \"{}\"", speedometer.jumps_y);
        let _ = writeln!(out, "seta cg_jumpGoal \"{}\"", speedometer.jump_goal);
        let _ = writeln!(out, "seta cg_lagometer \"{}\"", self.lagometer.mode);
        let _ = writeln!(out, "seta cg_lagometerX \"{}\"", self.lagometer.x);
        let _ = writeln!(out, "seta cg_lagometerY \"{}\"", self.lagometer.y);
        let _ = writeln!(out, "seta cl_commandsize \"{}\"", self.lagometer.command_size);
        let _ = writeln!(out, "seta cg_drainFX \"{}\"", self.drain_fx);
    }
}

// ------------------------------------------------------------ appearance --

/// What the viewer's own state means for how everybody else is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleViewer {
    /// The connected server is jaPRO (racemode/duel culling are jaPRO-only).
    pub japro: bool,
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
}

impl Appearance {
    pub const NORMAL: Self = Self { visible: true, ghost: Ghost::None, dim: false, duel_bubble: false };
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
            if !viewer.free_spectator && (viewer.japro || bit(style::HIDE_NONDUELERS)) {
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
        if !viewer.coop_race
            && ((bolt1 == 0 && !bit(style::NONRACER_VFX_DISABLE)) || !bit(style::RACER_VFX_DISABLE))
        {
            appearance.ghost = Ghost::Race;
        }
        return appearance;
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
        Ghost::Race => [0.1, 0.1, 1.0, 1.0],
        Ghost::Duel => [0.1, 0.1, 0.4, 1.0],
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
                if elapsed < 3000 && !settings.race_start {
                    let _ = write!(ui.timer_text, "\nStart: {}", self.start_speed);
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

/// The three radio groups of the jaPRO `cosmetics` command: (group, title, tip).
pub const COSMETIC_GROUPS: [(u8, &str, &str); 3] = [
    (1, "HATS", "One hat at a time."),
    (2, "MASKS", "One mask, beard or pair of glasses. Combines with a hat and a cape."),
    (3, "CAPES & CARRIED", "One cape, goose or carried item. Combines with a hat and a mask."),
];

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
    fn coop_racers_are_not_ghosted() {
        let v = StyleViewer { racemode: true, coop_race: true, ..viewer() };
        assert_eq!(classify(&v, 3, 2, false).ghost, Ghost::None);
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
        assert_eq!(settings.cvar_value("cl_commandsize").as_deref(), Some("64"));
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
        assert!(text.contains("seta cg_lagometer \"2\"") && text.contains("seta cg_lagometerX \"60\""));
        assert!(text.contains("seta cg_drainFX \"2\""));
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
    fn race_timer_shows_the_start_speed_for_the_first_three_seconds() {
        let settings = JaproCgame::default();
        let mut stats = RaceStats::default();
        let early = stats.frame(&settings, true, 10_000, 11_000, 480.0, 250.0).unwrap();
        assert!(early.timer_text.ends_with("Start: 480"), "{}", early.timer_text);
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
