#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Command,
    Cvar,
    /// A game-module command (OpenJK cgame `gcmds[]`): registered only while
    /// connected, so it completes, and is forwarded to the server.
    ServerCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatchScope {
    None,
    VidRestart,
    MapLoadOrVidRestart,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub kind: EntryKind,
    pub default: &'static str,
    pub range: &'static str,
    pub description: &'static str,
    pub latch: LatchScope,
}

const fn cvar(
    name: &'static str,
    default: &'static str,
    range: &'static str,
    description: &'static str,
) -> Entry {
    Entry {
        name,
        kind: EntryKind::Cvar,
        default,
        range,
        description,
        latch: LatchScope::None,
    }
}

const fn latched_cvar(
    name: &'static str,
    default: &'static str,
    range: &'static str,
    description: &'static str,
    latch: LatchScope,
) -> Entry {
    Entry {
        name,
        kind: EntryKind::Cvar,
        default,
        range,
        description,
        latch,
    }
}

const fn command(name: &'static str, description: &'static str) -> Entry {
    Entry {
        name,
        kind: EntryKind::Command,
        default: "",
        range: "",
        description,
        latch: LatchScope::None,
    }
}

/// Return the cvar's declared discrete integer choices. Pipe-delimited ranges
/// are explicit choice lists (`0|1|2`, `30|60|120|240`). A plain `0..1` is
/// also a two-choice boolean declaration so legacy `toggle <bool-cvar>` binds
/// remain useful. Wider numeric ranges are scalar domains, not option lists.
pub fn declared_integer_options(entry: &Entry) -> Option<Vec<String>> {
    let range = entry.range.trim();
    if range == "0..1" {
        return Some(vec!["0".to_owned(), "1".to_owned()]);
    }
    if !range.contains('|') {
        return None;
    }
    let mut values = Vec::new();
    for part in range.split('|') {
        let token = part.trim().split_whitespace().next().unwrap_or("");
        if token.parse::<i64>().is_err() {
            return None;
        }
        values.push(token.to_owned());
    }
    (values.len() >= 2).then_some(values)
}

pub const ENTRIES: &[Entry] = &[
    cvar("cp_pluginDisable", "1536", "0..2147483647", "JAPRO preference bitfield; also available in the Mod menu."),
    cvar(
        "com_maxfps",
        "0",
        "0 or 1..10000",
        "Render FPS cap; 0 resolves to the highest rate the present path can reach.",
    ),
    cvar(
        "cg_drawFPS",
        "1",
        "0|1|2",
        "FPS overlay: 0 off, 1 simple FPS, 2 detailed diagnostics.",
    ),
    cvar(
        "cg_drawTimer",
        "0",
        "0|1",
        "TaystJK/OpenJK match timer: elapsed time since level start as M:SS.",
    ),
    cvar(
        "cg_debugEvents",
        "0",
        "0|1|2|3",
        "Event diagnostics: 1 accepted+status, 2 adds suppressed candidates, 3 adds entity/resource detail.",
    ),
    cvar(
        "cg_asyncAssets",
        "1",
        "0|1",
        "Runtime asset cache misses (player/saber/weapon Ghoul2 models): 1 load on background workers and keep the previous or default model until ready, 0 old synchronous registration. Developer A/B switch.",
    ),
    cvar(
        "cg_eventWorkers",
        "0",
        "0|1",
        "A/B test event semantic preparation: 0 CGame thread, 1 dedicated worker pool; ordered side effects remain on CGame.",
    ),
    cvar(
        "cg_drawCrosshair",
        "1",
        "0..7",
        "Crosshair shape: 0 off; 1 classic; 2 dot; 3 plus; 4 classic+dot; 5 brackets; 6 box; 7 line (jaPRO strafehelper line crosshair). A non-zero cg_crosshairImage draws that image instead of the shape.",
    ),
    cvar(
        "cg_crosshairImage",
        "0",
        "0..10",
        "Image crosshair: 0 draws the cg_drawCrosshair shape; 1..10 draws gfx/2d/crosshaira..crosshairj (j needs japro-assets.pk3). cg_drawCrosshair 0 still hides it.",
    ),
    cvar(
        "cg_crosshairSize",
        "24",
        "4..96",
        "Crosshair size using the stock JKA cg_crosshairSize convention.",
    ),
    cvar(
        "cg_crosshairStrength",
        "1",
        "0..2",
        "Crosshair visibility. 1 is as authored; below fades it, above strengthens the faint stock image crosshairs.",
    ),
    cvar(
        "cg_crosshairColor",
        "255 255 255 230",
        "R G B A (0..255)",
        "TaystJK-style crosshair RGBA color.",
    ),
    cvar("cl_chatBubbleSelf", "1", "0..1", "jaPRO: show your own chat balloon to others while the console, chat or a menu has the keyboard."),
    cvar("cl_chatBubbleUnfocused", "1", "0..1", "jaPRO: also show the chat balloon while the game window is unfocused."),
    cvar(
        "cg_dynamicCrosshair",
        "1",
        "0|1|2",
        "TaystJK dynamic crosshair: 0 static screen-center trace, 1 always trace from the weapon/player muzzle, 2 use the dynamic trace except for the original melee/saber/racemode/strafehelper static overrides.",
    ),
    cvar(
        "cg_crosshairIdentifyTarget",
        "1",
        "0..1",
        "jaPRO: colour the crosshair by what it is on (red enemy, green friend, yellow neutral, grey duelist). With 0 it keeps cg_crosshairColor.",
    ),
    cvar(
        "cg_drawCrosshairNames",
        "1",
        "float",
        "jaPRO: name of the player under the crosshair. 0 off; a positive value is the seconds the name stays after you aim away; a negative value shows it only while aimed at.",
    ),
    cvar(
        "cg_drawCrosshairNamesColours",
        "1",
        "0..1",
        "jaPRO: 1 draws the name with its own colour codes; 0 strips them and colours it red/green by friend or foe.",
    ),
    cvar("cg_drawCrosshairNamesOpacity", "1", "0..1", "jaPRO: opacity of the crosshair name."),
    cvar(
        "cg_drawPlayerNames",
        "0",
        "0..2",
        "TaystJK: draw unobstructed player names above players. 0 off, 1 names, 2 names plus health bars.",
    ),
    cvar(
        "cg_drawPlayerNamesScale",
        "0.5",
        "0.05..4",
        "TaystJK: scale of world-space player-name labels.",
    ),
    cvar("cg_movementKeys", "0", "0..4", "TaystJK movement-key HUD mode: 0 off, 1 original, 2 +attack, 3 compact centered, 4 compact movable."),
    cvar("cg_movementKeysX", "0", "float", "TaystJK movement-key HUD horizontal offset."),
    cvar("cg_movementKeysY", "0", "float", "TaystJK movement-key HUD vertical offset."),
    cvar("cg_movementKeysSize", "1", "0.25..4", "TaystJK movement-key HUD scale."),
    cvar("cg_movementKeysWalk", "0", "0|1", "Show the walk/run key in the movement-key HUD."),
    cvar("cg_strafeHelper", "3008", "bitmask", "TaystJK Strafehelper style/direction bitmask, plus DinurdoJK Cinematic world-space style bit 2097152."),
    cvar("cg_strafeHelper_FPS", "0", "0..1000", "Strafehelper physics FPS override; 0 follows com_maxfps, then falls back to 125 when uncapped."),
    cvar("cg_strafeHelperOffset", "75", "float", "TaystJK Strafehelper angular offset in hundredths of a degree."),
    cvar("cg_strafeHelperLineWidth", "1", "0.25..5", "Strafehelper line width."),
    cvar("cg_strafeHelperPrecision", "256", "100..10000", "TaystJK Strafehelper projection precision/sensitivity."),
    cvar("cg_strafeHelperCutoff", "0", "0..480", "Strafehelper line cutoff."),
    cvar("cg_strafeHelperActiveColor", "0 255 0 200", "R G B A", "TaystJK active Strafehelper line color."),
    cvar("cg_strafeHelperInactiveAlpha", "200", "0..255", "TaystJK inactive Strafehelper line alpha."),
    cvar("cg_strafeTrailRadius", "2", "0.1..100", "jaPRO strafe-trail line diameter."),
    cvar("cg_strafeTrailLife", "5", "0.1..3600 seconds", "jaPRO lifetime for live /strafeTrail player traces."),
    cvar("cg_strafeTrailFPS", "40", "1..1000", "jaPRO SV_FPS metadata for recorded trails (used by legacy plum spacing)."),
    cvar("cg_strafeTrailPlums", "0", "0|1", "jaPRO whole-second number markers on loaded strafe trails."),
    cvar("cg_strafeTrailGhost", "1", "0|1", "Draw strafe trails translucently."),
    cvar("cg_strafeTrailPlayers", "0", "32-bit mask", "jaPRO live-player strafe-trail bitmask; configure with /strafeTrail."),
    cvar("cg_logStrafeTrail", "0", "0 or trail name", "Record your active jaPRO race run to strafetrails/<name>.cfg on a background writer."),
    cvar("cg_strafeTrailDistance", "16384", "256..131072", "DinurdoJK draw-distance cull for loaded/live strafe trails."),
    command("loadTrail", "Load a jaPRO strafetrails/<name>.cfg asynchronously: /loadTrail <name> [slot]."),
    command("clearTrail", "Clear existing trail geometry by client/color slot; -1 clears all."),
    command("strafeTrail", "List/toggle live player trail tracing: /strafeTrail [client|-1]."),
    command("trailMenu", "Open the Strafe Trails GUI."),
    cvar(
        "cg_hudHealth",
        "bl 24 -58 1",
        "anchor x y scale",
        "HUD health panel layout used by HUD Edit Mode.",
    ),
    cvar(
        "cg_hudShield",
        "bl 24 -28 1",
        "anchor x y scale",
        "HUD shield panel layout used by HUD Edit Mode.",
    ),
    cvar(
        "cg_hudAmmo",
        "br -24 -58 1",
        "anchor x y scale",
        "HUD ammo panel layout used by HUD Edit Mode.",
    ),
    cvar(
        "cg_hudForce",
        "br -24 -28 1",
        "anchor x y scale",
        "HUD Force panel layout used by HUD Edit Mode.",
    ),
    cvar("cg_hudMovementKeys", "tl 0 0 1", "anchor x y scale", "Movement keys HUD offset/scale on top of its stock position (HUD Edit Mode)."),
    cvar("cg_hudFps", "tl 0 0 1", "anchor x y scale", "FPS / profiler HUD offset/scale (HUD Edit Mode)."),
    cvar("cg_hudChat", "tl 0 0 1", "anchor x y scale", "Chat history and input box offset/scale (HUD Edit Mode)."),
    cvar("cg_hudCenterPrint", "tl 0 0 1", "anchor x y scale", "Center-print text offset/scale (HUD Edit Mode)."),
    cvar("cg_hudCrosshairName", "tl 0 0 1", "anchor x y scale", "Crosshair target name offset/scale (HUD Edit Mode)."),
    cvar("cg_hudFollow", "tl 0 0 1", "anchor x y scale", "Spectator follow name offset/scale (HUD Edit Mode)."),
    cvar("cg_hudVote", "tl 0 0 1", "anchor x y scale", "Vote text offset/scale (HUD Edit Mode)."),
    cvar("cg_hudRaceTimer", "tl 0 0 1", "anchor x y scale", "Race timer block offset/scale (HUD Edit Mode)."),
    cvar("cg_hudSurfaceInspector", "tl 0 0 1", "anchor x y scale", "Trace/surface inspector panel offset/scale (HUD Edit Mode)."),
    cvar("cg_hudSpeedometer", "tl 0 0 1", "anchor x y scale", "Speedometer main readout offset/scale (HUD Edit Mode)."),
    cvar("cg_hudSpeedometerJumps", "tl 0 0 1", "anchor x y scale", "Speedometer pre-speed jumps array offset/scale (HUD Edit Mode)."),
    cvar("cg_hudSpeedGraph", "tl 0 0 1", "anchor x y scale", "Speedometer speed graph offset/scale (HUD Edit Mode)."),
    cvar("cg_hudRaceStart", "tl 0 0 1", "anchor x y scale", "Race start-speed readout offset/scale (HUD Edit Mode)."),
    cvar("cg_hudLagometer", "tl 0 0 1", "anchor x y scale", "Lagometer graph/numbers offset/scale (HUD Edit Mode)."),
    cvar(
        "cg_hudSnap",
        "1",
        "0..1",
        "Snap HUD Edit Mode dragging to the configured grid.",
    ),
    cvar(
        "cg_hudGridSize",
        "8",
        "1..64 pixels",
        "HUD Edit Mode grid spacing in framebuffer pixels.",
    ),
    command("hudedit", "Open the in-game drag/snap HUD editor."),
    command(
        "cg_eventStats",
        "Summarize accepted demo events; use 'cg_eventStats clear' to reset counters.",
    ),
    cvar("model", "kyle", "model[/skin]", "Local player model and skin."),
    cvar("cg_forceModel", "0", "0 | model[/skin] | allyModel,enemyModel", "Client-side forced player models: off, one model for all other players, or separate ally/enemy models."),
    cvar("sensitivity", "5", "float", "TaystJK mouse sensitivity multiplier."),
    cvar("m_yaw", "0.022", "float", "TaystJK horizontal mouse scale."),
    cvar("m_pitch", "0.022", "float", "TaystJK vertical mouse scale."),
    cvar(
        "cg_fov",
        "90",
        "1..140",
        "TaystJK horizontal FOV on the legacy 4:3 baseline.",
    ),
    cvar(
        "cl_mouseAccel",
        "0",
        "float",
        "TaystJK legacy style-0 mouse acceleration; 0 disables it.",
    ),
    cvar("con_timestamps", "1", "0..1", "Prefix console lines with local HH:MM:SS timestamps."),
    cvar("con_suggest", "1", "0..1", "Live command/cvar filter while typing in the console: Up/Down pick, Tab completes, Esc dismisses."),
    cvar("cg_chatboxCompletion", "1", "0..1", "JAPP: Tab in the chat input completes a player name (colours ignored); repeated Tab cycles ranked matches, preferring exact and prefix matches."),
    cvar("cl_chatLog", "1", "0..1", "Save live server chat sessions as self-contained HTML under the active fs_game/chatlogs directory. File I/O runs on a dedicated worker thread."),
    cvar("ui_vgs", "1", "0=off, nonzero=on", "TaystJK: use the jaPRO VGS canned-voice menu."),
    cvar("cg_screenShake", "1", "0..2", "Camera shake: 1 effect-driven (explosions, creature efx), 2 also the kick of firing rockets, repeater alt, flechette, bryar/demp2 alt and bowcaster. Server-triggered shake events are not affected, as in OpenJK."),
    cvar("r_jumpHeightShade", "1", "0..1", "jaPRO SP physics: tint flat surfaces by jump height while airborne (green = ideal landing, red = deeper, blue = reachable above the jump line)."),
    cvar("s_volume", "0.5", "0..1", "OpenJK game/effects volume."),
    cvar("s_volumeVoice", "1.0", "0..1", "OpenJK voice-channel volume."),
    cvar("s_musicvolume", "0.25", "0..1", "OpenJK background music volume (level music and the duel track)."),
    cvar("cg_raceSounds", "1", "0..255", "jaPRO race mode sound bit mask; bit 1 keeps the race start-trigger sound."),
    cvar("cg_chatSounds", "0", "0..2", "Beep on incoming chat: 0 silent, 1 the legacy talk beep, 2 distinct beeps for private messages and team chat (jaPRO)."),
    cvar("cg_footsteps", "3", "0..4", "Player footsteps (jaPRO): 0 off, 1 sounds, 2 sounds and material effects (sand, mud, snow, gravel dust), 3+ same (footprints are controlled separately by r_footprints), 4 debugging."),
    cvar("cg_ghoul2Marks", "16", "0..64", "Burn marks kept on each player model by blaster/rocket hits (0 disables). Needs GPU Ghoul2 skinning."),
    cvar("cg_hitchrecord", "0", "0..1", "Keep a rolling buffer of per-frame prediction/view state for `hitchmark`, and print ^3HITCH^7 lines to console when one is detected. 0 disables both."),
    cvar("cg_scorePlums", "1", "0..1", "Floating score numbers where you score (kills, captures)."),
    cvar("cg_autoSwitch", "1", "0..2", "Weapon pickup/empty auto switch: 0 never, 1 to a better safe weapon (no rocket/thermal/mines), 2 to any better weapon."),
    cvar("cg_blood", "0", "0..2", "jaPRO gibs: 0 none (death voice instead), 1 skull or brain, 2 full gibs (needs the jaPRO models/gibs assets)."),
    cvar("cg_ambientSounds", "1", "0..1", "Level ambience from the map's sound sets (worldspawn soundSet and local ambient entities)."),
    cvar("cg_duelMusic", "1", "0..1", "jaPRO: play the duel music track while you are in a duel."),
    cvar("s_separation", "0.5", "0..1", "OpenJK stereo separation used by positional sound."),
    cvar(
        "s_muteWhenUnfocused",
        "1",
        "0..1",
        "Mute game and voice audio while the game window is unfocused.",
    ),
    cvar("cg_jumpSounds", "1", "0..3", "jaPRO jump voice: 0 off, 1 everyone, 2 other players only, 3 only you (jaPRO itself defaults to 0)."),
    cvar("cg_rollSounds", "1", "0..3", "jaPRO roll voice: 0 off, 1 everyone, 2 other players only, 3 only you."),
    cvar("cg_noTaunt", "0", "0..1", "jaPRO: silence taunt voice lines."),
    cvar("cg_duelSounds", "1", "0..3", "jaPRO duel start: 0 off, 1 sound and text, 2 sound only, 3 text only."),
    cvar("cg_killSounds", "2", "0..2", "jaPRO frag sound when you kill: 1 always, 2 with a mid-air variant (needs the jaPRO sound/frag assets)."),
    cvar("cg_killMessage", "1", "0..3", "TaystJK kill center-print: 0 off, 1 normal with FFA place/score, 2 kill only, 3 higher on screen."),
    cvar("cg_drawRewards", "1", "0..2", "TaystJK/JKA award medals: 0 off, 1 JKA voice/medals, 2 Quake 3 variants where available."),
    cvar("cg_drawTeamOverlay", "0", "0..6", "TaystJK team overlay: 0 off; 1/2 classic, 3/4 alternate, 5/6 compact scalable; even modes omit your own row."),
    cvar("teamoverlay", "0", "read-only", "Server tinfo request bit mirrored automatically from cg_drawTeamOverlay (CVAR_USERINFO)."),
    cvar("cg_drawTeamOverlayX", "640", "integer", "Horizontal anchor for the TaystJK team overlay."),
    cvar("cg_drawTeamOverlayY", "0", "integer", "Vertical anchor for the TaystJK team overlay."),
    cvar("cg_drawTeamOverlayWeapons", "0", "0..1", "Show teammate weapon icons in cg_drawTeamOverlay."),
    cvar("cg_drawTeamOverlayScale", "1.0", "0.5..2.5", "Scale for cg_drawTeamOverlay modes 5/6."),
    cvar("cg_drawTeamOverlayMaxHP", "150", ">=1", "Health-bar reference maximum for cg_drawTeamOverlay modes 5/6."),
    cvar("cg_drawTeamOverlayForce", "1", "0..1", "Show the jaPRO force-power column/bar in cg_drawTeamOverlay when the server provides it."),
    cvar("cg_scoreDeaths", "1", "0..3", "TaystJK scoreboard deaths: 0 off, 1 server data, 2 local fallback, 3 local count."),
    cvar("cg_drawScores", "1", "0..3", "TaystJK score HUD: 0 off; 1 classic team scores; 2 classic with red/blue score colours; 3 centred team boxes (and duel HUD in GT_DUEL)."),
    cvar("cg_hitsounds", "0", "0..6", "jaPRO hit feedback: 1-4 pick a hit sound (needs the jaPRO sound/effects/hitsound assets), 5 plain saber hit, 6 any saber hit variant."),
    command("soundinfo", "Show active output format, device buffer, voice count and pre-limiter overload diagnostics."),
    cvar("cg_thirdPerson", "0", "0..1", "OpenJK third-person view toggle."),
    cvar(
        "cg_fpls",
        "1",
        "0..1",
        "Allow first-person saber/melee when cg_thirdPerson is disabled.",
    ),
    cvar(
        "cg_saberTrail",
        "1",
        "0|1|2",
        "OpenJK saber swing trail: 0 off, 1 normal authored saberBlur trail, 2 legacy special/high-frequency mode.",
    ),
    cvar(
        "cg_saberTeamColors",
        "1",
        "0..1",
        "TaystJK team saber colors: force red-team sabers red and blue-team sabers blue outside Siege/Jedi-vs-Merc.",
    ),
    cvar(
        "cg_saberStaffMultiColor",
        "0",
        "0..1",
        "TaystJK staff-saber option: use the secondary saber color for primary-saber blades after blade 0.",
    ),
    cvar(
        "cg_fxFPS",
        "90",
        "0 or 15..250",
        "Continuous projectile/trail EFX sampling rate. 0 restores legacy JKA presentation-frame-driven density.",
    ),
    cvar(
        "cg_fxFPSScope",
        "0",
        "0..1",
        "Which render-frame-driven effects cg_fxFPS resamples: 0 continuous projectile/trail EFX only, 1 also eligible stock frame-driven effects such as saber/world sparks and wall marks.",
    ),
    cvar(
        "fx_physics",
        "2",
        "0..3",
        "FX particle physics (TaystJK): 0 off, 1 same as 0 (stock has no non-expensive collision), 2 trace primitives flagged expensivePhysics, 3 force the world trace on every physics primitive.",
    ),
    cvar(
        "fx_lod",
        "2",
        "0..2",
        "EFX level of detail, decided when an effect spawns (live particles are never thinned): 0 stock, 1 honor authored cullRange (stock OpenJK ignores it), 2 also scale Particle/Tail populations down by projected screen size.",
    ),
    cvar(
        "r_fxLodScale",
        "5",
        "0.1..100",
        "EFX distance LOD scale, like r_lodscale: multiplies every authored cullRange (stock OpenJK ignores cullRange) and shifts the adaptive density curve. Larger keeps more detail farther away; 1 is the raw authored range. Needs fx_lod 1 or 2.",
    ),
    cvar(
        "r_lodScale",
        "5",
        "0.1..100",
        "Ghoul2 model LOD scale (OpenJK default 5). Larger keeps higher-detail GLM LODs at greater distances; smaller switches to cheaper LODs sooner. r_lodbias is added afterward.",
    ),
    cvar(
        "fx_countScale",
        "1",
        "0..1",
        "Stock OpenJK EFX count scale: multiplies authored count ranges wider than 1. Never scales upward; every primitive keeps at least one spawn.",
    ),
    cvar("cg_thirdPersonAlpha", "1.0", "float", "OpenJK third-person player alpha."),
    cvar("cg_thirdPersonAngle", "0", "degrees", "OpenJK third-person orbit angle."),
    cvar("cg_thirdPersonCameraDamp", "1", "float", "Third-person camera damping (DinurdoJK default 1)."),
    cvar("cg_thirdPersonHorzOffset", "0", "units", "OpenJK third-person horizontal camera offset."),
    cvar("cg_thirdPersonPitchOffset", "0", "degrees", "OpenJK third-person pitch offset."),
    cvar("cg_thirdPersonRange", "100", "units", "TaystJK third-person camera range."),
    cvar(
        "cg_thirdPersonSpecialCam",
        "0",
        "0..1",
        "TaystJK action camera during saber special moves; Rust action-camera branch is not ported yet.",
    ),
    cvar("cg_thirdPersonTargetDamp", "1", "float", "Third-person target damping (DinurdoJK default 1)."),
    cvar("cg_thirdPersonVertOffset", "16", "units", "OpenJK third-person vertical target offset."),
    cvar(
        "r_skipUi",
        "0",
        "0..1",
        "Diagnostic: skip all UI vertex generation and UI rendering.",
    ),
    cvar(
        "developer",
        "0",
        "0..3",
        "Global diagnostic level: 0 quiet, 1 basic lifecycle, 2 verbose per-entity/job output, 3 trace. Levels above 0 also enable developer-only inspector/overlay tools.",
    ),
    cvar(
        "r_verbose",
        "0",
        "0..3",
        "Renderer-only diagnostic level: 0 quiet, 1 basic renderer lifecycle, 2 verbose renderer internals, 3 trace. A matching developer level also enables renderer output.",
    ),
    cvar(
        "pmove_msec",
        "8",
        "1..33",
        "Fixed player movement timestep in milliseconds.",
    ),
    cvar(
        "cl_timerResolution1ms",
        "0",
        "0|1",
        "Windows-only A/B toggle for timeBeginPeriod(1). Affects timeout/sleep granularity, not raw mouse-event wakeup.",
    ),
    cvar(
        "cl_input_latelatch",
        "0",
        "0..1",
        "Experimental: resample the newest subframe view orientation on the render thread at the latest coherent camera point.",
    ),
    cvar("r_physics", "0", "0..1", "Master switch for client-side visual physics (ragdolls, cloth, jiggle, props)."),
    cvar("r_physicsHz", "60", "30|60|120|240", "Fixed timestep for client-side visual physics."),
    cvar("r_physicsMaxSubsteps", "4", "1|2|4|8", "Maximum visual-physics catch-up steps after a slow frame."),
    cvar("r_physicsCCD", "1", "0..1", "Enable continuous collision detection for eligible visual physics bodies."),
    cvar("r_physicsSleeping", "1", "0..1", "Allow inactive Rapier bodies to sleep."),
    cvar("r_ragdolls", "1", "0..1", "Enable client-side visual ragdolls."),
    cvar("r_ragdollMax", "8", "2|4|8|16|32", "Maximum active client ragdolls."),
    cvar("r_ragdollLifetime", "20", "5|10|20|30|60 seconds", "Lifetime of simulated client ragdolls."),
    cvar("r_ragdollSelfCollision", "0", "0..1", "Allow limbs on one ragdoll to collide with each other."),
    cvar("r_jigglePhysics", "0", "0..1", "Experimental: enable auto-detected _humanoid soft-tissue motion; model.jiggle overrides auto detection."),
    cvar("r_jiggleSolver", "0", "0|1", "Soft-tissue solver: 0 KawaiiPhysics-derived point solve, 1 naelstrof/JigglePhysics Verlet/constraint solve."),
    cvar("r_jiggleStrength", "1", "0..2", "Overall soft-tissue displacement multiplier."),
    cvar("r_jiggleBreastStrength", "1", "0..2", "Chest-region soft-tissue displacement multiplier."),
    cvar("r_jiggleGluteStrength", "1", "0..2", "Glute-region soft-tissue displacement multiplier."),
    cvar("r_jiggleStiffness", "1", "0..3", "Multiplier for KawaiiPhysics-style pose stiffness."),
    cvar("r_jiggleDamping", "1", "0..3", "Multiplier for KawaiiPhysics-style velocity damping."),
    cvar("r_jiggleGluteLift", "0", "-0.4..0.6", "Lower-edge glute trim; positive keeps motion higher, negative includes more upper thigh."),
    cvar("r_jiggleJPStiffness", "0.4", "0..1", "naelstrof/JigglePhysics stiffness/angle elasticity for the virtual soft-region child."),
    cvar("r_jiggleJPDrag", "0.4", "0..1", "naelstrof/JigglePhysics mechanical/local-space drag."),
    cvar("r_jiggleJPAirDrag", "0.1", "0..1", "naelstrof/JigglePhysics world-motion air drag."),
    cvar("r_jiggleJPStretch", "0.8", "0..1", "naelstrof/JigglePhysics squash/stretch control; 0 rigid length, 1 length follows stiffness."),
    cvar("r_jiggleJPSoften", "0", "0..1", "naelstrof/JigglePhysics stiffness softening near the animated rest pose."),
    cvar("r_jiggleJPGravity", "0", "0..2", "naelstrof/JigglePhysics gravity multiplier for soft-region points; 0 is this client's body-tissue preset."),
    cvar("cg_dismember", "0", "0|1|2", "OpenJK dismemberment: 0 off, 1 limbs only, 2 full including head/waist."),
    cvar("r_dismemberMax", "24", "1..128", "Maximum detached limb rigid bodies kept by client physics."),
    cvar("r_dismemberLifetime", "16", "1..300 seconds", "Maximum client Rapier lifetime of a detached limb."),
    cvar("r_clothPhysics", "0", "0..1", "Experimental: add cloth sway and momentum to authored cape/cloak/robe surfaces."),
    cvar("r_clothBodyClearance", "1", "0..4", "Additional cloth/body separation in JKA units. Large values inflate the garment."),
    cvar("r_clothAirResistance", "1", "0..4", "Cloth airflow strength. Zero disables aerodynamic drag; one uses normal fabric pressure."),
    cvar("r_clothTurnResponse", "1.8", "0..4", "Cloth turning inertia multiplier. One uses the same response as translation."),
    cvar("r_clothAnimationInfluence", "0.35", "0..1", "Animation influence on free fabric: zero follows sewn attachments; one follows full skeletal animation."),
    cvar("r_clothWind", "0", "0..1", "Use shared r_weatherWind direction and gusts for cloth."),
    cvar("r_clothBodyCollision", "1", "0..1", "Collide experimental player cloth with animated head and body capsules."),
    cvar("r_physicsProps", "1", "0..1", "Enable client-only dynamic visual props."),
    cvar("r_physicsPropMax", "96", "32|64|96|192|384", "Maximum active client physics props."),
    cvar("r_physicsDebris", "1", "0..1", "Enable rigid-body simulation for client-spawned debris."),
    cvar("r_physicsDebrisMax", "192", "64|128|192|384|768", "Maximum active client debris rigid bodies."),
    cvar("r_physicsDebrisLifetime", "10", "2|5|10|20|30 seconds", "Lifetime of client physics debris."),
    cvar("r_physicsPlayerPush", "1", "0..1", "Allow player capsules to push visual props one-way."),
    cvar("r_physicsWeaponImpulses", "1", "0..1", "Apply observed weapon impacts to client physics bodies."),
    cvar("r_physicsExplosionImpulses", "1", "0..1", "Apply observed explosion impulses to client physics bodies."),
    cvar("r_physicsForceImpulses", "1", "0..1", "Apply observable Force-power impulses to client physics bodies."),
    cvar("r_physicsDebug", "0", "0..1", "Print Rapier initialization and ragdoll candidate diagnostics."),
    cvar("r_physicsStats", "0", "0..1", "Print client physics counters and fixed-step state once per second."),
    latched_cvar(
        "r_fullscreen",
        "0",
        "0|1|2",
        "Fullscreen mode: 0 windowed, 1 borderless, 2 exclusive.",
        LatchScope::VidRestart,
    ),
    latched_cvar(
        "r_backend",
        "vulkan",
        "vulkan|dx12",
        "Graphics backend selected for the next vid_restart.",
        LatchScope::VidRestart,
    ),
    command(
        "version",
        "Print the DinurdoJK version and UTC compile time embedded in this executable.",
    ),
    command(
        "plugin",
        "List or toggle a jaPRO cp_pluginDisable preference by the bit number shown by /plugin.",
    ),
    command(
        "pluginDisable",
        "Alias of /plugin: list or toggle jaPRO cp_pluginDisable preferences.",
    ),
    command("cmdlist", "List registered console commands, optionally filtered by a wildcard pattern."),
    command("help", "Print help for one registered command."),
    command("clear", "Clear the console output."),
    command("echo", "Print message text to the console."),
    command("vstr", "Execute the current value of a cvar as command text."),
    command("wait", "Pause execution of the remaining command buffer for one or more client frames."),
    command("record", "Start recording the current live session to a native demos/*.dm_26 file."),
    command("stoprecord", "Stop the current demo recording and write its end marker."),
    command(
        "predsettings",
        "Print the pmove settings the predictor is running with: race/style, pmove_float/fixed/msec, velocity snapping, command chopping.",
    ),
    command(
        "hitchmark",
        "Write the last ~5 s of per-frame prediction/view state to <game>/hitch/*.csv. Bind it and tap it right after a hitch (needs cg_hitchrecord 1).",
    ),
    command("demo", "Play demos/<demoname>.dm_26 using the same playback path as the GUI."),
    command("rGhost", "Load a demos/<demoname>.dm_26 race run as a visual-only ghost synchronized to your duelTime. Use rGhost clear to remove it."),
    command("exec", "Execute a .cfg script through the active fs_game/base VFS."),
    command("execq", "Execute a .cfg script without displaying the exec notification."),
    command(
        "fs_refresh",
        "Re-scan the active base/fs_game VFS and retry assets that previously failed without restarting CGame.",
    ),
    command("write", "Alias for writeconfig <filename>; writes archived client settings and binds."),
    command("writeconfig", "Write archived client settings and binds to a .cfg under the active fs_game."),
    command(
        "vid_restart",
        "Restart the WGPU renderer and reload GPU resources using current video settings.",
    ),
    command(
        "screenshot",
        "Save the next completed frame under the active fs_game/screenshots directory (base/screenshots when no mod is active).",
    ),
    command("bind", "Bind a key to a command; bind <key> queries the current binding."),
    command("unbind", "Remove a key binding."),
    command("unbindall", "Remove all key bindings."),
    command("bindlist", "List all current key bindings."),
    command("bindspec", "Bind a key only while spectating; unassigned spectator keys inherit normal binds."),
    command("unbindspec", "Remove one spectator-only key override."),
    command("unbindspecall", "Remove all spectator-only key overrides."),
    command("bindspeclist", "List spectator-only key overrides."),
    command("trace", "Toggle inspection of the world surface under the center crosshair."),
    command("trace_clear", "Clear the pinned surface inspection and triangle highlight."),
    command("entities", "Open the 2D entity blueprint: entities [targetname|classname] selects the first match."),
    command("viewpos", "Print the view origin and angles: (x y z) : yaw (pitch p)."),
    command(
        "perfsample",
        "perfsample <seconds> [label]: hold the command buffer, then print the average render FPS.",
    ),
    command("toggle", "Cycle a cvar through its declared discrete options; explicit value lists are also supported."),
    command("messagemode", "Open global chat input."),
    command("voicechat", "Open TaystJK VGS when connected to a jaPRO server and ui_vgs is enabled."),
    command("+scores", "Show the live scoreboard while held and request fresh scores."),
    command("-scores", "Hide the live scoreboard."),
    command("messagemode2", "Open team chat input."),
    cvar(
        "r_swapInterval",
        "0",
        "0|1|2|3",
        "Presentation mode: 0 off, 1 on, 2 fast/mailbox, 3 adaptive.",
    ),
    cvar(
        "r_maxFrameLatency",
        "3",
        "1|2|3",
        "Maximum WGPU surface frames in flight: 1 lowest latency, 2 balanced, 3 maximum throughput.",
    ),
    cvar(
        "r_ext_multisample",
        "0",
        "0|2|4|8...",
        "MSAA sample count; 0 means off.",
    ),
    cvar(
        "r_textureMode",
        "GL_LINEAR_MIPMAP_LINEAR",
        "GL_* filter mode",
        "Base texture filtering mode.",
    ),
    latched_cvar(
        "r_picmip",
        "0",
        "0..16",
        "Classic texture-quality mip bias: omit this many highest map-texture mip levels on upload.",
        LatchScope::MapLoadOrVidRestart,
    ),
    cvar(
        "r_ext_texture_filter_anisotropic",
        "0",
        "0|2|4|8|16",
        "Anisotropic texture filtering level.",
    ),
    cvar(
        "r_detailTextures",
        "off",
        "off|neutral2x|linear2x|dstcolor_one|multiply",
        "Fallback detail-texture blend mode for eligible opaque BSP materials that do not author a classic detail stage.",
    ),
    cvar(
        "r_detailTextureFade",
        "0",
        "0|1",
        "Fade the synthetic detail contribution back to neutral with camera distance.",
    ),
    cvar(
        "r_detailTextureFadeDistance",
        "512",
        "64..8192",
        "JKA-unit distance where the ported power-4 detail fade reaches zero detail contribution.",
    ),
    latched_cvar("r_customwidth", "1280", "320..", "Windowed render width.", LatchScope::VidRestart),
    latched_cvar("r_customheight", "800", "240..", "Windowed render height.", LatchScope::VidRestart),
    cvar(
        "r_windowX",
        "auto",
        "desktop pixels",
        "Last windowed X position; auto means automatic.",
    ),
    cvar(
        "r_windowY",
        "auto",
        "desktop pixels",
        "Last windowed Y position; auto means automatic.",
    ),
    cvar(
        "r_windowMaximized",
        "0",
        "0..1",
        "Restore the window maximized on launch.",
    ),
    cvar("r_gammaMethod", "shader", "shader | baked | hardware", "Brightness method: shader corrects the final image; baked changes textures; hardware uses the display ramp with crash restoration (Windows SDR). Also accepts 0, 1, 2."),
    cvar(
        "r_gamma",
        "1.000",
        "0.5..3.0",
        "Display gamma/brightness adjustment.",
    ),
    cvar(
        "r_modelBrightness",
        "1.000",
        "0.5..3.0",
        "Model lighting brightness on the r_gamma scale (like r_ambientScale). Follows r_gamma while r_modelBrightnessLock is 1.",
    ),
    cvar("r_modelBrightnessLock", "1", "0..1", "1 links r_modelBrightness to r_gamma; 0 lets it be set independently."),
    cvar(
        "r_dynamicLightBrightness",
        "1.000",
        "0.5..3.0",
        "Runtime dynamic light (blasters, sabers, explosions) brightness on the r_gamma scale. Follows r_gamma while r_dynamicLightBrightnessLock is 1.",
    ),
    cvar("r_dynamicLightBrightnessLock", "1", "0..1", "1 links r_dynamicLightBrightness to r_gamma; 0 lets it be set independently."),
    cvar("r_drawTriggers", "0", "0..1", "Draw trigger_* volumes as colored translucent brushes (push green, teleport purple, hurt red, multiple blue, once cyan). Session-only."),
    cvar("r_drawClipBrushes", "0", "0..1", "Draw clip-only brushes (player clip orange, shot clip magenta, monster/bot clip yellow). Session-only."),
    cvar("r_drawEntities", "0", "0..1", "NetRadiant-style 3D overlay: a colored box and classname label above every map entity (same data as the `entities` command), colored by category, with lines to its target/targetname links. Brush movers track their live position when networked. Session-only."),
    cvar("r_drawMapModels", "1", "0..1", "Draw server-placed MD3 map models (misc_model_* props). Brush models and geometry compiled into the BSP are unaffected."),
    cvar("r_hdr", "0", "0..1", "HDR intermediate rendering."),
    latched_cvar("r_floatLightmap", "0", "0..1", "Use FP16 lightmaps while HDR is enabled; prefer Rend2 .hdr companions when present.", LatchScope::MapLoadOrVidRestart),
    cvar("r_autoExposure", "0", "0..1", "Adapt exposure from HDR scene luminance before tone mapping."),
    cvar("r_bloom", "0", "0..1", "Bloom post-processing."),
    cvar("r_halation", "0", "0..1", "Halation post-processing."),
    cvar("r_ssao", "0", "0..1", "Screen-space ambient occlusion."),
    cvar("r_staticBspAo", "0", "0..1", "Cached static BSP ambient occlusion."),
    cvar("r_fxaa", "0", "0..1", "FXAA anti-aliasing."),
    cvar("r_smaa", "0", "0..1", "SMAA anti-aliasing."),
    cvar("r_taa", "0", "0..1", "Temporal anti-aliasing."),
    cvar(
        "r_contactShadows",
        "0",
        "0..1",
        "Short-range screen-space contact shadows toward the map sun.",
    ),
    cvar(
        "r_weatherWind",
        "167 220 0.2 0",
        "speed direction gust direction_variation",
        "Shared atmospheric wind used by clouds, rain, grass and ocean systems.",
    ),
    cvar(
        "r_cloudShapeEvolution",
        "0",
        "0..1",
        "Enable slow relative evolution of cloud detail and billows.",
    ),
    cvar(
        "r_cloudTerrainInteraction",
        "0",
        "0..1",
        "Enable terrain-driven lifting of low cloud shapes.",
    ),
    cvar(
        "r_cloudEmptySkip",
        "0",
        "0..1",
        "A/B adaptive empty-space skipping in the primary volumetric cloud march.",
    ),
    cvar(
        "r_drawfog",
        "0",
        "0|1|2|3",
        "Fog rendering: 0 off, 1 legacy (authored fog redrawn in post), 2 legacy (per-surface EXP2 fog, JKA default), 3 volumetric.",
    ),
    cvar(
        "r_fogStrength",
        "0.000",
        "0..10",
        "Fog density/strength multiplier.",
    ),
    cvar(
        "r_sunOverride",
        "0",
        "0..1",
        "Use a runtime custom q3 shader sun instead of the map-authored sky sun.",
    ),
    cvar(
        "r_entitySunLighting",
        "0",
        "0..1",
        "Remove the baked map sun from the entity lightgrid and re-light entities with the runtime sun (needs r_entityAmbientLighting bsp_lightgrid).",
    ),
    cvar(
        "r_sunYaw",
        "314.256",
        "degrees",
        "Custom q3 sun azimuth: 0 east, 90 north.",
    ),
    cvar(
        "r_sunPitch",
        "57.047",
        "-90..90",
        "Custom q3 sun elevation above the horizon.",
    ),
    cvar(
        "r_sunIntensity",
        "250.000",
        "0..",
        "Custom q3 sun intensity for runtime sun-aware renderer effects.",
    ),
    cvar(
        "r_sunColor",
        "1,1,1",
        "R,G,B (0..1)",
        "Custom q3 sun chromaticity; use comma-separated RGB in the console.",
    ),
    cvar(
        "r_distanceCullScale",
        "0.000",
        "0..10",
        "0 uses the map/default distanceCull; nonzero values scale that distance.",
    ),
    cvar("r_rain", "0", "0..1", "Camera-local GPU rain rendering."),
    cvar(
        "r_rainIntensity",
        "rain",
        "light|rain|heavy",
        "Rain preset used by the shared precipitation renderer.",
    ),
    cvar(
        "r_puddleQuality",
        "high",
        "standard|high",
        "high shades puddles and wet ground with the GodotOcean water response and streaked reflections.",
    ),
    cvar(
        "r_puddleScatter",
        "0.800",
        "0..1",
        "How readily rain collects in scattered puddles on large flat ground; 0 keeps only basin puddles.",
    ),
    cvar(
        "r_rainGrade",
        "0.500",
        "0..1",
        "Wet-weather colour grade (cool shadows, warm highlights, richer neon) while it rains.",
    ),
    cvar(
        "r_footprints",
        "3d",
        "off|2d|3d",
        "Footprints: off, stock JKA 2D mark shader, or 3D snow deformation.",
    ),
    cvar(
        "r_ocean",
        "0",
        "0|1",
        "GPU ocean. Enabling on a world loaded without ocean requires applying video settings.",
    ),
    cvar("r_oceanFogColor", "0.45 0.6 0.75", "R G B (0..1)", "Water scattering tint."),
    cvar("r_oceanFogDistance", "300", "1..65536", "Base water absorption distance in game units."),
    cvar("r_oceanTransparency", "4", "0.1..16", "Water absorption-distance multiplier."),
    cvar("r_oceanDepthDarkening", "1", "0.01..32", "Underwater sunlight penetration multiplier."),
    cvar("r_oceanRefraction", "0.025", "0..0.15", "Wave distortion of transmitted scene color."),
    cvar("r_oceanCaustics", "0", "0..8", "Experimental FFT curvature caustics; not shadow-aware yet."),
    cvar("r_oceanUnderwaterCull", "3", "0..32", "Separate submerged BSP cull distance, as an absorption-distance multiple. Zero disables."),
    cvar(
        "r_grass",
        "1",
        "0..1",
        "Procedural per-blade grass on surfaceSprites vegetation.",
    ),
    cvar(
        "r_grassPrecompute",
        "1",
        "0..1",
        "A/B grass root wind/clump compute prepass (runtime diagnostic; not persisted).",
    ),
    cvar(
        "r_grassMidLod",
        "1",
        "0..1",
        "A/B 5-triangle middle grass blade LOD (runtime diagnostic; not persisted).",
    ),
    cvar(
        "r_contactShadowDebug",
        "0",
        "0..1",
        "Show why the contact-shadow pass returned per pixel: grey=factor, blue=faces away from sun, red=ray sub-pixel/off screen, yellow=origin behind camera, magenta=no depth (runtime diagnostic; not persisted).",
    ),
    cvar(
        "r_grassFrontToBack",
        "1",
        "0..1",
        "A/B front-to-back opaque grass ordering for early-Z (runtime diagnostic; not persisted).",
    ),
    cvar(
        "r_reflectionQuality",
        "high",
        "off|low|medium|high|ultra",
        "Master reflection resolver quality/budget: probes -> SSR -> dynamically allocated planar reflections.",
    ),
    cvar(
        "r_reflectionDebug",
        "0",
        "0..1",
        "Show per-surface reflection resolver result (magenta planar, green SSR, blue probe, gray SSR-eligible/no-hit).",
    ),
    cvar(
        "r_chromaticAberration",
        "0.000",
        "0..1",
        "Photographic purple-fringing strength on bright high-contrast edges.",
    ),
    cvar("r_vignette", "0", "0..1", "Vignette post effect."),
    cvar("r_filmGrain", "0.000", "0..1", "Film grain strength."),
    cvar(
        "r_motionBlur",
        "0.000",
        "0..1",
        "Camera/world motion blur strength.",
    ),
    cvar(
        "r_depthOfField",
        "0.000",
        "0..1",
        "Gaussian depth-of-field strength with crosshair autofocus.",
    ),
    cvar(
        "r_dofAutoFocus",
        "1",
        "0..1",
        "Autofocus depth of field on the surface under the crosshair. Uses the same BSP/entity trace as the crosshair and works in remote play and demos, not only Solo.",
    ),
    cvar(
        "r_dofQuality",
        "adaptive",
        "performance|adaptive|high",
        "DOF Gaussian sampling quality. Adaptive spends extra samples only on large blur radii.",
    ),
    cvar("r_splitToning", "0", "0..1", "Enable separate shadow and highlight color tints."),
    cvar("r_splitToningStrength", "0.500", "0..1", "Overall split-toning strength."),
    cvar("r_splitToningShadowHue", "210.000", "0..360", "Shadow tint hue in degrees."),
    cvar("r_splitToningShadowSaturation", "0.300", "0..1", "Shadow tint saturation; zero leaves shadows untoned."),
    cvar("r_splitToningHighlightHue", "42.000", "0..360", "Highlight tint hue in degrees."),
    cvar("r_splitToningHighlightSaturation", "0.250", "0..1", "Highlight tint saturation; zero leaves highlights untoned."),
    cvar("r_splitToningBalance", "0.000", "-1..1", "Negative favors highlight toning; positive favors shadow toning."),
    cvar("r_colorLut", "off", "preset", "Color grading LUT: built-in film look or a .cube name from base/LUTs."),
    cvar(
        "r_colorLutStrength",
        "1.000",
        "0..1",
        "Color grading LUT strength.",
    ),
    cvar(
        "r_terrainLod",
        "0.000",
        "0..1",
        "Distance-based geometry LOD aggressiveness for q3map indexed terrain; 0 disables it.",
    ),
    cvar("r_gpuDriven", "0", "0..1", "GPU-visible draw compaction."),
    cvar("r_hizOcclusion", "0", "0..1", "Hi-Z occlusion culling."),
    cvar(
        "r_perfTrace",
        "0",
        "0..1",
        "Periodic renderer performance trace in the console.",
    ),
    cvar(
        "r_pom",
        "1",
        "0..1",
        "Parallax occlusion mapping when r_pbr is on; 0 keeps PBR shading without POM height sampling. Session-only.",
    ),
    cvar(
        "r_worldPath",
        "auto",
        "auto|unified",
        "World renderer: auto keeps the FastBaseline path when eligible; unified forces the general renderer (Minimal (Unified) preset).",
    ),
    cvar(
        "r_gpuTimings",
        "0",
        "0..1",
        "GPU timestamp-query profiler; no per-frame query/readback cost when disabled.",
    ),
    cvar(
        "r_ghoul2Skinning",
        "gpu",
        "cpu|workers|gpu",
        "Ghoul2 GLM skinning path: CPU reference, CPU worker pool, or WGPU vertex shader.",
    ),
    cvar(
        "r_ghoul2EarlyCull",
        "1",
        "0..1",
        "Skip full Ghoul2 pose evaluation and render submission for models outside the view frustum while preserving lightweight animation state.",
    ),
    cvar(
        "r_lodbias",
        "0",
        "0..8",
        "JKA-style Ghoul2/model LOD bias; higher values select cheaper authored GLM LODs.",
    ),
    cvar(
        "r_ghoul2BatchDraws",
        "1",
        "0|1|2",
        "GPU Ghoul2 draw batching: 0 off, 1 adaptive, 2 force. Adaptive only groups repeated opaque/masked mesh/material/LOD surfaces when the crowd is likely to benefit.",
    ),
    cvar(
        "r_ghoul2AnimSmooth",
        "0.3",
        "0..1 (exclusive)",
        "jaPRO CBoneCache::SmoothLow renderer bone-history filter (jaPRO default 0.3): blends each player's final composed Ghoul2 pose toward last frame's filtered pose before skinning/attachments. Only active strictly between 0 and 1, matching jaPRO: 0 or >=1 disables it (1 is 'off', not 'maximum'). Does not apply while ragdolled.",
    ),
    cvar(
        "r_entityAmbientLighting",
        "off",
        "off|bsp_lightgrid|bevy_irradiance_volume",
        "Entity ambient-lighting source; bsp_lightgrid uses OpenJK-style classic BSP entity lighting.",
    ),
    cvar(
        "r_fullbright",
        "0",
        "0..1",
        "Classic world-lighting master: 1 replaces baked lightmap contribution with white.",
    ),
    cvar(
        "r_vertexLight",
        "0",
        "0..1",
        "Use BSP vertex lighting instead of baked world lightmaps; as in JKA, runtime dlights are suppressed while enabled.",
    ),
    cvar(
        "r_lightmap",
        "0",
        "0..1",
        "Show baked lightmap or vertex-light data without the diffuse texture where available.",
    ),
    cvar(
        "r_dynamicLights",
        "off",
        "off|legacy|vertex|clustered_lite|forward_plus|ray_traced",
        "Dynamic-lighting technique for runtime-authored lights.",
    ),
    cvar("r_dynamicLightFalloff", "0", "0..1", "Falloff of runtime dynamic lights (sabers, blasters, explosions) in forward_plus/ray_traced. 0 stock JKA 1 - d^2/r^2, 1 physical inverse-square with a smooth window that reaches zero at the radius with no visible edge."),
    cvar("r_rtSamples", "1", "1|2|4", "Samples per soft RT sun or saber light. Higher values reduce noise and cost more GPU time; point lights use one ray."),
    cvar("r_rtResolution", "full", "full|half", "RT sun shadow rate. half = reduced rate: retraces each pixel every 4th frame and carries the rest by motion vectors; shadow edges and disocclusions still trace every frame. Always full screen resolution; local lights are always traced directly."),
    cvar(
        "r_mapLightSimulation",
        "0",
        "0..1",
        "Source .map only: preview authored light entities with the runtime clustered renderer.",
    ),
    cvar(
        "r_modernSabers",
        "0",
        "0..1",
        "Continuous view-facing saber glow ribbon; 0 keeps the OpenJK-compatible sprite-chain presentation.",
    ),
    cvar(
        "r_flares",
        "1",
        "0..1",
        "Screen-space saber flare overlay; does not disable authored saber impact FX.",
    ),
    cvar(
        "r_saberImpactFx",
        "1",
        "0..1",
        "Render authored saber hit/block EFX trees; useful for isolated FX performance A/B testing.",
    ),
    cvar(
        "r_fxGeometry",
        "cpu",
        "cpu|workers|gpu",
        "EFX geometry submission path: reference CPU tessellation, threaded CPU tessellation, or GPU-instanced sprites.",
    ),
    cvar(
        "r_fxZeroAlphaDiscard",
        "0",
        "0..1",
        "GPU EFX sprite A/B: discard exactly-zero-alpha source fragments before fog/blending where alpha controls contribution.",
    ),
    cvar(
        "r_saberMarks",
        "legacy",
        "off|legacy|enhanced",
        "Saber/world contact effects: off, OpenJK-compatible marks, or a smooth heat-accumulating molten relief/sludge effect.",
    ),
    cvar(
        "r_dynamicShadows",
        "off",
        "off|blob|entity|csm|ray_traced",
        "Dynamic-shadow mode; entity = entity-only shadow map aimed by the baked lightgrid; csm = Bevy-style cascaded shadow maps.",
    ),
    cvar(
        "r_emissiveAreaLights",
        "0",
        "0..1",
        "Extract emissive surfaces as area lights.",
    ),
    cvar(
        "r_voxelProbeGI",
        "0",
        "0..1",
        "Voxel/probe indirect lighting; enabling it on a map prepared without GI requires vid_restart.",
    ),
    cvar(
        "r_entityShadowLight",
        "lightgrid",
        "lightgrid|authored",
        "Entity-map shadow light: baked lightgrid direction, or the strongest authored map light's real position.",
    ),
    cvar(
        "r_localLightShadows",
        "0",
        "0..1",
        "Cubemap shadows for selected local lights.",
    ),
    latched_cvar("r_genNormalMaps", "0", "0..1", "Generate fallback normal maps from diffuse textures when authored normals are absent.", LatchScope::MapLoadOrVidRestart),
    cvar("r_deluxeMapping", "1", "0..1", "Use q3map2/Rend2 directional lightmaps when present, with BSP lightgrid fallback."),
    cvar("r_deluxeSpecular", "1", "0..1", "Scale specular response from directional baked lighting."),
    latched_cvar(
        "r_pbr",
        "1",
        "0..1",
        "Master Rend2-style PBR switch; when off, .mtr overrides are not used.",
        LatchScope::MapLoadOrVidRestart,
    ),
    latched_cvar(
        "fs_allowAssetOverrides",
        "1",
        "0..1",
        "Allow higher-priority addon/loose assets to replace qpaths supplied by retail assets0.pk3 through assets3.pk3. Active .mtr dependencies remain source-affine.",
        LatchScope::MapLoadOrVidRestart,
    ),
    cvar(
        "r_cascadedShadows",
        "0",
        "0..1",
        "Legacy boolean alias for Dynamic Shadows: Cascaded Shadow Maps.",
    ),
    cvar(
        "r_showtris",
        "0",
        "0..127",
        "Wireframe category bitmask: 1 map, 2 players, 4 entities, 8 effects, 16 grass, 32 ocean, 64 deformation.",
    ),
    cvar(
        "r_novis",
        "0",
        "0..1",
        "Disable PVS culling; legacy compatibility alias.",
    ),
    cvar(
        "r_pvsMode",
        "auto",
        "off|minimal|full|auto",
        "Potentially-visible-set culling mode; Auto uses map-load portal/cluster draw plans with shared compatible merged batches.",
    ),
    command("devmap", "Load a map for local development: devmap <map>."),
    command("map", "Load a map: map <map>."),
    command(
        "noclip",
        "Toggle local no-clip movement; N is the default in-game shortcut.",
    ),
    command(
        "r_dumpmaterials",
        "Dump material diagnostics; optional filter or 'all'.",
    ),
    command("dumpmaterials", "Alias for r_dumpmaterials."),
    command("say", "Send a global chat message: say <message>."),
    command("say_team", "Send a team chat message: say_team <message>."),
    cvar(
        "r_planardebug",
        "off",
        "off|candidates|selected|texture|applied|binding",
        "Planar reflection debug view.",
    ),
    command("disconnect", "Leave the current game and return to the main menu."),
    command("quit", "Exit the client."),
    command("exit", "Exit the client."),
    // Live connection (cl_main.cpp) and userinfo cvars.
    command("connect", "Connect to a server: connect <host[:port]> (default port 29070)."),
    command("reconnect", "Reconnect to the last server."),
    cvar("sv_master1", "masterjk3.ravensoft.com", "host[:port]", "TaystJK master slot 1 (Raven); empty disables it."),
    cvar("sv_master2", "master.jkhub.org", "host[:port]", "TaystJK master slot 2 (JKHub); empty disables it."),
    cvar("sv_master3", "master.ouned.de", "host[:port]", "TaystJK master slot 3 (Ouned); empty disables it."),
    cvar("sv_master4", "", "host[:port]", "TaystJK custom master slot 4; empty disables it."),
    cvar("sv_master5", "", "host[:port]", "TaystJK custom master slot 5; empty disables it."),
    command("cmd", "Send the rest of the line to the server as a client command."),
    command("userinfo", "Print the userinfo sent to the server."),
    command("configstrings", "Print the active client configstrings list."),
    command("serverinfo", "Print the connected server's serverinfo configstring."),
    command(
        "serverdump",
        "Dump the server's serverinfo + systeminfo with derived numbers (estimated sv_fps from snapshot gaps, ping, command lag) to the console and <game>/serverinfo/<addr>_<time>.txt, and append a one-line summary to <game>/serverinfo/servers.log for comparing servers.",
    ),
    command("systeminfo", "Print the connected server's systeminfo configstring."),
    cvar("name", "Padawan", "string", "Player name sent in userinfo."),
    cvar("rate", "50000", "1000..90000", "Maximum bytes/second the server may send."),
    cvar("snaps", "100", "1..125", "Snapshots per second requested from the server."),
    cvar("cl_maxpackets", "100", "15..1000", "Maximum client packets per second."),
    cvar("cg_stylePlayer", "0", "bit mask", "jaPRO player styling bits: 2 duel shell, 4 hide duelers, 8 hide racers in FFA, 16 hide non-racers while racing, 32 hide racers while racing, 64 solid racers, 128 solid FFA players while racing, 256 ghost duelers, 1024 hide non-duelers while dueling (stock servers), 65536 hide cosmetics, 1048576 seasonal cosmetics."),
    cvar("cg_raceTimer", "2", "0..3", "jaPRO race timer: 0 off, 1 time, 2 time + max/avg/start speed, 3 same with milliseconds."),
    cvar("cg_rGhostAlpha", "0.35", "0.02..1", "Opacity of /rGhost recorded race players."),
    cvar("cg_rGhostDemoBaseUrl", "https://s.playja.pro/races", "URL", "Base URL of the public race-demo archive; the client reads its /index JSON catalog."),
    cvar("cg_raceTimerSize", "0.75", "0.1..3", "Text scale of the race timer."),
    cvar("cg_raceTimerX", "5", "640x480 px", "Left edge of the race timer."),
    cvar("cg_raceTimerY", "280", "640x480 px", "Baseline of the race timer."),
    cvar("cg_raceStart", "0", "0|1", "Show the speed you crossed the race start line with."),
    cvar("cg_raceStartX", "300", "640x480 px", "Left edge of the start speed readout."),
    cvar("cg_raceStartY", "280", "640x480 px", "Baseline of the start speed readout."),
    cvar("cg_startGoal", "0", "speed", "The start speed readout turns green at or above this speed (0 = never)."),
    cvar("cg_drainFX", "2", "0..2", "Force-drain hand effect: 0 off, 1 stock mp/drain(wide).efx, 2 jaPRO mp/drain(wide)_japro.efx. Force lightning itself is unaffected."),
    command("speedometer", "List the cg_speedometer options, or toggle option <num> (see /speedometer)."),
    cvar("cg_speedometer", "0", "bit mask", "jaPRO speedometer bits; configure with /speedometer <num>. 1 enable, 2 pre-speed, 4 jump height, 8 jump distance, 16 vertical speed, 32 yaw speed, 64 accel meter, 128 speed graph, 256 km/h, 512 mph, 1024 pre-speed jumps array, 2048 no colors, 4096/8192 array colors, 16384 old speed graph, 32768 XYZ speed."),
    cvar("cg_speedometerX", "132", "640x480 px", "Left edge of the speedometer."),
    cvar("cg_speedometerY", "459", "640x480 px", "Baseline of the speedometer."),
    cvar("cg_speedometerSize", "0.75", "0.1..3", "Text scale of the speedometer."),
    cvar("cg_speedometerJumps", "10", "0..511", "How many pre-speed jumps the jumps array keeps (bit 1024)."),
    cvar("cg_speedometerJumpsX", "185", "640x480 px", "Left edge of the pre-speed jumps array."),
    cvar("cg_speedometerJumpsY", "300", "640x480 px", "Baseline of the pre-speed jumps array."),
    cvar("cg_jumpGoal", "0", "speed", "First-jump pre-speed at or above which the readout turns green (0 = never)."),
    cvar("cg_lagometer", "0", "0..4", "jaPRO lagometer: 0 off (the connection-interrupted warning still shows), 1 graph, 2 graph with average ping and interpolation numbers, 3 numbers over a frameless graph, 4 (this client only) mode 3 plus packet loss and peak ping under the graph. Not shown against the local server."),
    cvar("cg_lagometerX", "48", "640x480 px", "Distance of the lagometer's right edge from the right of the screen (also positions the old speed graph)."),
    cvar("cl_commandsize", "64", "4..512", "How many usercmds back the connection-interrupted warning looks: it appears once the server has not acknowledged a command that old. (The usercmd history itself is always 512 deep.)"),
    cvar("cg_lagometerY", "144", "640x480 px", "Distance of the lagometer's bottom from the bottom of the screen (also positions the old speed graph)."),
    cvar("cg_specFollowFastest", "0", "0|1", "While spectating, keep following the fastest player (debounced)."),
    cvar("cg_specCamera", "0", "0|1|2", "Spectator follow camera: 0 first person, 1 third person, 2 orbit."),
    cvar("cg_specCameraMotion", "0", "0|1", "Third-person spectator camera faces the current presented direction of motion (TaystJK cg_thirdPersonAngle -1 behavior)."),
    cvar("cg_specOrbitRange", "140", "24..1200", "Spectator orbit-camera distance; the mouse wheel changes it while orbiting."),
    command("cameraedit", "Adjust the third-person camera with the mouse: scroll to zoom, drag to move, hold right mouse to aim."),
    command("followFastest", "Spectate whoever is moving fastest right now."),
    command("followRedFlag", "Spectate the red flag carrier."),
    command("followBlueFlag", "Spectate the blue flag carrier."),
    cvar("cl_packetdup", "1", "0..5", "Repeat the usercmds of this many earlier packets to survive packet loss."),
    cvar("cl_timeNudge", "0", "-900..900", "Milliseconds of extra (+) or less (-) interpolation delay."),
    cvar("net_port", "29070", "0..65535", "Preferred local UDP port for the live game connection. Like TaystJK/OpenJK, the client tries this port and the next 9 if it is busy; 0 lets the OS choose."),
    cvar("saber1", "single_1", "saber name", "Primary saber sent in userinfo."),
    cvar("saber2", "none", "saber name", "Secondary saber sent in userinfo."),
    cvar("color1", "4", "0..6", "Primary saber colour: 0 red, 1 orange, 2 yellow, 3 green, 4 blue, 5 purple, 6 RGB (jaPRO / JA+ servers; see cp_sbRGB1)."),
    cvar("color2", "4", "0..6", "Secondary saber colour (same values as color1; RGB uses cp_sbRGB2)."),
    cvar("cp_sbRGB1", "0", "0..16777215", "jaPRO custom blade colour for color1 6, packed red + green * 256 + blue * 65536."),
    cvar("cp_sbRGB2", "0", "0..16777215", "jaPRO custom blade colour for color2 6, packed like cp_sbRGB1."),
    cvar("forcepowers", "7-1-032330000000001333", "rank-side-levels", "Force power configuration."),
    cvar("sex", "male", "male|female", "Sound gender hint sent in userinfo."),
    cvar("password", "", "string", "Server join password."),
    cvar("rconPassword", "", "string", "Password for remote console access (see rcon). Not saved to the config."),
    cvar("rconAddress", "", "host[:port]", "Alternate server address for rcon when not connected (default port 29070). Not saved to the config."),
    command("rcon", "Run a command on a server remotely: rcon <command>. Needs rconPassword, and a connection or rconAddress."),
    cvar("char_color_red", "255", "0..255", "Player tint (red) sent in userinfo; tints the entity-coloured parts of a player model, e.g. jedi_zf armour."),
    cvar("char_color_green", "255", "0..255", "Player tint (green) sent in userinfo."),
    cvar("char_color_blue", "255", "0..255", "Player tint (blue) sent in userinfo."),
    cvar("handicap", "100", "1..100", "Handicap sent in userinfo; the server uses it as maximum health."),
    cvar("cg_predictItems", "1", "0..1", "Tell the server this client predicts item pickups (userinfo)."),
    cvar("fs_game", "japro", "directory", "Game directory mounted over base for local games (applied when a map loads); empty or base for none. A connected server's own fs_game overrides it."),
    cvar("cp_cosmetics", "0", "bitfield", "jaPRO model cosmetics bitfield, sent to jaPRO servers."),
    cvar("cp_clanPwd", "none", "string", "jaPRO clan password, sent to jaPRO servers. Not saved to the config."),
    cvar("ui_username", "", "string", "jaPRO account name used by the login menu (Setup -> Mod) and the login command."),
    cvar("ui_password", "", "string", "jaPRO account password used by the login menu. Not saved to the config."),
    cvar("cg_displayCameraPosition", "1 80 16", "read-only", "jaPRO userinfo: cg_thirdPerson, range and vertical offset (kept current automatically)."),
    cvar("cg_displayNetSettings", "125 0 125", "read-only", "jaPRO userinfo: cl_maxPackets, cl_timeNudge and com_maxFPS (kept current automatically)."),
    cvar("cg_errorDecay", "100", "0..500", "Milliseconds over which prediction corrections are smoothed."),
    cvar("cg_noPredict", "0", "0..1", "Disable client-side movement prediction (use interpolated server state)."),
    cvar("cg_showMiss", "0", "0..1", "Print prediction misses."),
    cvar("cg_predictionDebug", "0", "0..1", "Show live prediction/ground-trace diagnostics and print rich miss details."),
    cvar("cg_modelFrameDebug", "0", "0..1", "Log head, saber hilt and blade anchors for every presented render frame to <game>/hitch/model-frames-*.csv, with JKA world coordinates and screen pixels. Set 0 to finish and flush."),
    cvar("cg_predictionMissHighlight", "0", "0..1", "Flash a red screen-edge warning when a prediction miss exceeds cg_predictionMissThreshold."),
    cvar("cg_predictionMissThreshold", "8", "0..4096", "Prediction correction distance in JKA units required for the visual miss warning."),
    cvar(
        "cl_allowMissingMap",
        "0",
        "0..1",
        "Keep a live server connection active without world geometry when the server BSP is not installed locally.",
    ),
    cvar("cl_allowHttpDownload", "1", "0..1", "Allow HTTP PK3 autodownloads advertised by the server. HTTP is preferred when available."),
    cvar("cl_allowDownload", "1", "0..1", "Allow legacy Jedi Academy UDP PK3 autodownloads (svc_download). Used when HTTP is unavailable or fails."),
    cvar("cl_commandRate", "125", "15..1000", "Usercmds built per second, i.e. the movement physics rate. Set it yourself; it is not tied to com_maxfps (which only caps rendering here)."),
    cvar("cg_groundTraceDebug", "0", "0..2", "Print this client's ground-trace predictions: 1 = ground changes (takeoff/landing/entity) and mismatches against the server snapshot for the same command, 2 = every new command."),
    cvar("cg_physicsDiag", "0", "0..2", "Replay each snapshot interval from the previous snapshot and compare with the server's state: 1 = print intervals that do not match exactly (origin/velocity/ground/flags deltas), 2 = print every one. Isolates pure physics mismatch (e.g. at cl_commandRate 1000)."),
    cvar("cg_snapMode", "-1", "-1..4", "How prediction rounds velocity after each pmove step (the engine's trap_SnapVector): -1 detect per server by replaying snapshot intervals (default), 0 OpenJK nearest, 1 truncate, 2 floor, 3 nearest-even, 4 none. Retail servers differ from OpenJK here and at cl_commandRate 1000 it decides friction and gravity."),
    cvar("cg_predictBackend", "-1", "-1..1", "Which native pmove predicts: -1 detect (JA+/jaPRO servers use the TaystJK backend; Base/other servers are tried against stock and TaystJK by snapshot replay), 0 stock OpenJK, 1 TaystJK shared BG/Pmove."),
    cvar("cl_commandPacing", "1", "0..1", "1 = stamp usercmds on an exact cl.serverTime cadence (race physics steps by the command gap); 0 = wall-clock pacing (gaps jitter 5-10 ms at 125)."),
    // cgame weapon selection (cg_weapons.c).
    command("weapon", "Select a weapon slot: weapon <1..13>."),
    command("weapnext", "Select the next weapon."),
    command("weapprev", "Select the previous weapon."),
    // cg_consolecmds.c / BG_CycleForce.
    command("forcenext", "Select the next usable Force power."),
    command("forceprev", "Select the previous usable Force power."),
    // TaystJK client-side jaPRO controls.
    command("flipkick", "TaystJK jaPRO flipkick bind sequence."),
    cvar("cg_zoomFov", "30.0", "number", "TaystJK held +zoom target field of view."),
    cvar("cg_fkDuration", "50", "integer", "TaystJK flipkick sequence frame duration."),
    cvar("cg_fkFirstJumpDuration", "0", "integer", "TaystJK flipkick first jump duration."),
    cvar("cg_fkSecondJumpDelay", "0", "integer", "TaystJK flipkick second jump delay."),
    // cl_input.cpp generic commands (usercmd generic_cmd).
    command("sv_saberswitch", "Holster/activate lightsaber."),
    command("engage_duel", "Challenge the player you are looking at to a private duel."),
    command("force_heal", "Use Force heal."),
    command("force_throw", "Use Force push."),
    command("force_pull", "Use Force pull."),
    command("force_distract", "Activate Force mind trick."),
    command("force_protect", "Activate Force protect."),
    command("force_absorb", "Activate Force absorb."),
    command("force_healother", "Use team heal."),
    command("force_forcepowerother", "Use team energize."),
    command("force_seeing", "Activate Force seeing."),
    command("use_seeker", "Use seeker drone."),
    command("use_field", "Use forcefield."),
    command("use_bacta", "Use bacta."),
    command("use_electrobinoculars", "Use electro binoculars."),
    command("use_sentry", "Use sentry gun."),
    command("use_jetpack", "Use jetpack."),
    command("use_bactabig", "Use big bacta."),
    command("use_healthdisp", "Use health dispenser."),
    command("use_ammodisp", "Use ammo dispenser."),
    command("use_eweb", "Use E-Web."),
    command("use_cloak", "Use cloaking device."),
    command("saberAttackCycle", "Cycle lightsaber attack styles."),
    command("taunt", "Taunt."),
    command("bow", "Bow."),
    command("meditate", "Meditate."),
    command("flourish", "Flourish."),
    command("gloat", "Gloat."),
];

const fn server_command(name: &'static str, description: &'static str) -> Entry {
    Entry {
        name,
        kind: EntryKind::ServerCommand,
        default: "",
        range: "",
        description,
        latch: LatchScope::None,
    }
}

/// OpenJK cg_consolecmds.c `gcmds[]` (vanilla): interpreted by the server's
/// game module. CG_InitConsoleCommands registers them for completion only.
pub const SERVER_COMMANDS: &[Entry] = &[
    // TaystJK gcmds[] jaPRO controls. These are reliable server strings.
    server_command("amTele", "jaPRO: teleport to the saved teleport mark."),
    server_command("amTeleMark", "jaPRO: save the current teleport mark."),
    server_command("engage_fullforceduel", "jaPRO: challenge/accept a full-force duel."),
    server_command("engage_gunduel", "jaPRO: challenge/accept a gun duel."),
    server_command("throwflag", "jaPRO: throw the carried team flag when the server permits it."),
    server_command("addbot", "Add a bot: addbot <name> [skill] [team] [delay] [altname]."),
    server_command("callteamvote", "Call a team vote: callteamvote <leader> <client>."),
    server_command("callvote", "Call a vote: callvote <map|map_restart|g_gametype|kick|clientkick|g_doWarmup|timelimit|fraglimit|nextmap> [value]."),
    server_command("duelteam", "Choose a Power Duel team: duelteam <free|single|double>."),
    server_command("follow", "Spectate a player: follow <name|number>."),
    server_command("follownext", "Spectate the next player."),
    server_command("followprev", "Spectate the previous player."),
    server_command("forcechanged", "Apply a new Force power configuration."),
    server_command("give", "Cheat: give <all|health|armor|ammo|weapons|force|item name>."),
    server_command("god", "Cheat: toggle invulnerability."),
    server_command("kill", "Suicide."),
    server_command("levelshot", "Cheat: move to the intermission point for a levelshot."),
    server_command("loaddefered", "Load deferred player models."),
    server_command("noclip", "Cheat: toggle noclip movement."),
    server_command("notarget", "Cheat: toggle NPC notarget."),
    server_command("NPC", "Cheat: spawn or control NPCs: NPC spawn <type>."),
    server_command("say", "Send a global chat message: say <message>."),
    server_command("say_team", "Send a team chat message: say_team <message>."),
    server_command("setviewpos", "Cheat: teleport: setviewpos <x> <y> <z> <yaw>."),
    server_command("siegeclass", "Choose a Siege class: siegeclass <class name>."),
    server_command("stats", "Show team statistics."),
    server_command("team", "Join a team: team <free|red|blue|spectator|follow1|follow2|scoreboard>."),
    server_command("teamtask", "Choose a team task."),
    server_command("teamvote", "Vote in a team vote: teamvote <yes|no>."),
    server_command("t_use", "Cheat: fire every entity with a targetname: t_use <targetname>."),
    server_command("tell", "Private message: tell <client number|name> <message>."),
    server_command("voice_cmd", "Send a voice command."),
    server_command("vgs_cmd", "Send a jaPRO VGS canned voice command."),
    server_command("vote", "Vote in a vote: vote <yes|no>."),
    server_command("where", "Print your current origin."),
    server_command("zoom", "Toggle binocular zoom."),
];

/// jaPRO server game commands (`commands[]` in codemp/game/g_cmds.c) that stock JKA
/// servers do not have. They complete only while connected to a jaPRO server.
pub const JAPRO_COMMANDS: &[Entry] = &[
    server_command("amBan", "jaPRO: Admin: ban a player: amBan <player> [duration]."),
    server_command("amBeg", "jaPRO: Emote: beg."),
    server_command("amBeg2", "jaPRO: Emote: beg (variant)."),
    server_command("amBernie", "jaPRO: Emote: sit with mittens."),
    server_command("amBreakdance", "jaPRO: Emote: breakdance."),
    server_command("amBreakdance2", "jaPRO: Emote: breakdance 2."),
    server_command("amBreakdance3", "jaPRO: Emote: breakdance 3."),
    server_command("amBreakdance4", "jaPRO: Emote: breakdance 4."),
    server_command("amCheer", "jaPRO: Emote: cheer."),
    server_command("amCower", "jaPRO: Emote: cower."),
    server_command("amDance", "jaPRO: Emote: dance."),
    server_command("amFlip", "jaPRO: Emote: saber flip."),
    server_command("amForceTeam", "jaPRO: Admin: move a player to a team: amForceTeam <player> <team>."),
    server_command("amFreeze", "jaPRO: Admin: freeze or unfreeze a player: amFreeze <player>."),
    server_command("amGrantAdmin", "jaPRO: Admin: grant admin rights: amGrantAdmin <player> <level>."),
    server_command("amHug", "jaPRO: Emote: hug."),
    server_command("amInfo", "jaPRO: Show the admin commands you may use."),
    server_command("amKick", "jaPRO: Admin: kick a player: amKick <player> [reason]."),
    server_command("amKillVote", "jaPRO: Admin: cancel the vote in progress."),
    server_command("amListMaps", "jaPRO: List the maps on the server: amListMaps [filter]."),
    server_command("amLockTeam", "jaPRO: Admin: lock or unlock a team: amLockTeam <team>."),
    server_command("amLogin", "jaPRO: Log in as admin: amLogin <user> <password>."),
    server_command("amLogout", "jaPRO: Log out of your admin session."),
    server_command("amLookup", "jaPRO: Admin: look up a player's account: amLookup <player>."),
    server_command("amMap", "jaPRO: Admin: change map immediately: amMap <map>."),
    server_command("amMOTD", "jaPRO: Show the message of the day."),
    server_command("amNoisy", "jaPRO: Emote: noisy."),
    server_command("amPoint", "jaPRO: Emote: point."),
    server_command("amPsay", "jaPRO: Admin: private message: amPsay <player> <message>."),
    server_command("amRage", "jaPRO: Emote: rage."),
    server_command("amRename", "jaPRO: Admin: rename a player: amRename <player> <name>."),
    command("amRun", "jaPRO: Toggle the new run animation preference."),
    server_command("amSay", "jaPRO: Say something to the admins."),
    server_command("amSignal", "jaPRO: Emote: signal."),
    server_command("amSignal2", "jaPRO: Emote: signal 2."),
    server_command("amSignal3", "jaPRO: Emote: signal 3."),
    server_command("amSignal4", "jaPRO: Emote: signal 4."),
    server_command("amSit", "jaPRO: Emote: sit."),
    server_command("amSit2", "jaPRO: Emote: sit 2."),
    server_command("amSit3", "jaPRO: Emote: sit 3."),
    server_command("amSit4", "jaPRO: Emote: sit 4."),
    server_command("amSit5", "jaPRO: Emote: sit 5."),
    server_command("amSlap", "jaPRO: Emote: slap."),
    server_command("amSleep", "jaPRO: Emote: sleep."),
    server_command("amSmack", "jaPRO: Emote: smack."),
    server_command("amSurrender", "jaPRO: Emote: surrender."),
    server_command("amTaunt", "jaPRO: Emote: taunt."),
    server_command("amTaunt2", "jaPRO: Emote: taunt 2."),
    server_command("amTele", "jaPRO: Teleport: amTele [player] or to your saved mark."),
    server_command("amTeleMark", "jaPRO: Save your position as the teleport mark."),
    server_command("amVictory", "jaPRO: Emote: victory pose."),
    server_command("amVstr", "jaPRO: Admin: run a server vstr: amVstr <name>."),
    server_command("blink", "jaPRO: Blink forward a short distance."),
    server_command("changePassword", "jaPRO: Change your account password: changePassword <old> <new>."),
    server_command("clanAdmin", "jaPRO: Clan: administrate your clan."),
    server_command("clanCreate", "jaPRO: Clan: create a clan: clanCreate <name>."),
    server_command("clanInfo", "jaPRO: Clan: show your clan's information."),
    server_command("clanInvite", "jaPRO: Clan: invite a player: clanInvite <player>."),
    server_command("clanJoin", "jaPRO: Clan: join a clan you were invited to."),
    server_command("clanLeave", "jaPRO: Clan: leave your clan."),
    server_command("clanList", "jaPRO: Clan: list the clan members."),
    server_command("clanPass", "jaPRO: Clan: set the clan password."),
    server_command("clanSay", "jaPRO: Clan: message your clan: clanSay <message>."),
    server_command("clanWhois", "jaPRO: Clan: who is in a clan: clanWhois <name>."),
    server_command("coop", "jaPRO: Co-op race: coop <player>."),
    server_command("crouchJump", "jaPRO: Toggle crouch jumping."),
    server_command("flagRecord", "jaPRO: Invalidate your current race record."),
    server_command("gc", "jaPRO: Send a game command to a player: gc <player> <command>."),
    server_command("giveOther", "jaPRO: Cheat: give items to another player."),
    server_command("haste", "jaPRO: Toggle haste."),
    server_command("hide", "jaPRO: Toggle hiding yourself (spectators and racers)."),
    server_command("ignore", "jaPRO: Ignore a player's chat: ignore <player>."),
    server_command("jetpack", "jaPRO: Toggle the jetpack."),
    server_command("jump", "jaPRO: Change your jump level: jump <1-3>."),
    server_command("killOther", "jaPRO: Cheat: kill another player."),
    server_command("launch", "jaPRO: Launch yourself (race practice)."),
    server_command("login", "jaPRO: Log in to your account: login <name> <password>."),
    server_command("logout", "jaPRO: Log out of your account."),
    server_command("mapEnts", "jaPRO: Cheat: list or spawn map entities."),
    server_command("master", "jaPRO: Add a clan master."),
    server_command("masterList", "jaPRO: List the clan masters."),
    server_command("modversion", "jaPRO: Show the server mod version."),
    server_command("move", "jaPRO: Choose your movement style: move <jka|qw|cpm|q3|pjk|wsw|rjq3|rjcpm|swoop|jetpack|sp|...>."),
    server_command("nearby", "jaPRO: List nearby players and their distance."),
    server_command("nudge", "jaPRO: Cheat: nudge your position."),
    server_command("pack", "jaPRO: Tribes: choose your pack."),
    server_command("practice", "jaPRO: Toggle practice mode in race mode."),
    server_command("printStats", "jaPRO: Print your account and race statistics."),
    server_command("race", "jaPRO: Toggle race mode."),
    server_command("rCompare", "jaPRO: Race: compare your records with another player: rCompare <player>."),
    server_command("register", "jaPRO: Register an account: register <name> <password>."),
    server_command("rFind", "jaPRO: Race: find a course by name: rFind <text>."),
    server_command("rHardest", "jaPRO: Race: list the hardest courses."),
    server_command("rLatest", "jaPRO: Race: list the latest records."),
    server_command("rocketChange", "jaPRO: Toggle backwards rocket launching."),
    server_command("rPopular", "jaPRO: Race: list the most popular courses."),
    server_command("rRank", "jaPRO: Race: show your race rank."),
    server_command("rTop", "jaPRO: Race: show the top times for this course."),
    server_command("rWarp", "jaPRO: Race: warp to a course: rWarp <course>."),
    server_command("rWorst", "jaPRO: Race: list the courses you have not beaten."),
    server_command("saber", "jaPRO: Change your saber style: saber <name>."),
    server_command("say_team_mod", "jaPRO: Team message visible to moderators."),
    server_command("score", "jaPRO: Show the scoreboard text."),
    server_command("serverConfig", "jaPRO: Show the server's configuration."),
    server_command("showNet", "jaPRO: Show network information for a player."),
    server_command("spot", "jaPRO: Save your current position as a spot."),
    server_command("thedestroyer", "jaPRO: Cheat: summon the destroyer."),
    server_command("throwNade", "jaPRO: Throw a grenade."),
    server_command("top", "jaPRO: Show the duel ranking table."),
    server_command("trace", "jaPRO: Trace what you are looking at."),
    server_command("warp", "jaPRO: Warp to a saved course spot: warp <name>."),
    server_command("warpList", "jaPRO: List the available warps."),
    server_command("whois", "jaPRO: Look up a player's account and aliases: whois <player>."),
    server_command("ysal", "jaPRO: Toggle the ysalamiri effect."),
];

static JAPRO_SERVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the console whether the current server is jaPRO (enables its commands).
pub fn set_japro_server(active: bool) {
    JAPRO_SERVER.store(active, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `name` is a vanilla server game command (forwarded while connected).
pub fn is_server_command(name: &str) -> bool {
    SERVER_COMMANDS.iter().any(|entry| entry.name.eq_ignore_ascii_case(name))
}

/// Every name the console knows. Server commands exist only while connected,
/// matching OpenJK's cgame registration lifetime; a server command that
/// shares a name with a local command is listed once (the local entry).
fn registry(connected: bool) -> impl Iterator<Item = &'static Entry> {
    let server: &'static [Entry] = if connected { SERVER_COMMANDS } else { &[] };
    let japro: &'static [Entry] =
        if connected && JAPRO_SERVER.load(std::sync::atomic::Ordering::Relaxed) { JAPRO_COMMANDS } else { &[] };
    ENTRIES
        .iter()
        .chain(server.iter().filter(|entry| {
            !ENTRIES.iter().any(|local| local.name.eq_ignore_ascii_case(entry.name))
        }))
        .chain(japro.iter().filter(|entry| {
            !ENTRIES.iter().chain(SERVER_COMMANDS).any(|known| known.name.eq_ignore_ascii_case(entry.name))
        }))
}

/// Local commands and cvars only (used for cvar get/set).
pub fn find(name: &str) -> Option<&'static Entry> {
    ENTRIES
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
}

/// Exact lookup for completion, including server commands while connected.
pub fn find_command(name: &str, connected: bool) -> Option<&'static Entry> {
    registry(connected).find(|entry| entry.name.eq_ignore_ascii_case(name))
}

pub fn prefix(prefix: &str, connected: bool) -> impl Iterator<Item = &'static Entry> {
    let prefix = prefix.to_ascii_lowercase();
    registry(connected).filter(move |entry| entry.name.to_ascii_lowercase().starts_with(&prefix))
}

/// Registered executable commands (including cgame/server commands only while
/// connected), excluding cvars just like OpenJK's `cmdlist`.
pub fn commands(connected: bool) -> impl Iterator<Item = &'static Entry> {
    registry(connected).filter(|entry| entry.kind != EntryKind::Cvar)
}

/// Small case-insensitive `Com_Filter` subset used by `cmdlist`: `*` matches
/// any run and `?` matches one character. Ordinary patterns are exact matches.
pub fn filter_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase().into_bytes();
    let text = text.to_ascii_lowercase().into_bytes();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star = None;
    let mut star_text = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            star_text = t;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            star_text += 1;
            t = star_text;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// How a registered name matched the live console query. Lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    Exact,
    Prefix,
    Substring,
    /// Query letters appear in order (`cgfov` finds `cg_fov`).
    Subsequence,
    /// Only the description mentions the query.
    Description,
}

/// One row of the live console filter.
#[derive(Debug, Clone, Copy)]
pub struct Suggestion {
    pub entry: &'static Entry,
    pub tier: MatchTier,
    /// Bit `i` set = byte `i` of `entry.name` matched the query (first 64 bytes).
    pub mask: u64,
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let (hay, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || hay.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

fn name_mask(start: usize, len: usize) -> u64 {
    (start..start + len).filter(|bit| *bit < 64).fold(0, |mask, bit| mask | 1u64 << bit)
}

fn classify(entry: &'static Entry, query: &str) -> Option<Suggestion> {
    let name = entry.name.as_bytes();
    let q = query.as_bytes();
    let hit = |tier, mask| Some(Suggestion { entry, tier, mask });
    if name.eq_ignore_ascii_case(q) {
        return hit(MatchTier::Exact, name_mask(0, name.len()));
    }
    if name.len() >= q.len() && name[..q.len()].eq_ignore_ascii_case(q) {
        return hit(MatchTier::Prefix, name_mask(0, q.len()));
    }
    if let Some(start) = name.windows(q.len()).position(|w| w.eq_ignore_ascii_case(q)) {
        return hit(MatchTier::Substring, name_mask(start, q.len()));
    }
    let mut mask = 0u64;
    let mut wanted = q.iter().map(u8::to_ascii_lowercase);
    let mut next = wanted.next();
    for (index, byte) in name.iter().enumerate() {
        if next == Some(byte.to_ascii_lowercase()) {
            mask |= name_mask(index, 1);
            next = wanted.next();
        }
    }
    if next.is_none() && q.len() >= 2 {
        return hit(MatchTier::Subsequence, mask);
    }
    (q.len() >= 3 && contains_ignore_case(entry.description, query))
        .then(|| Suggestion { entry, tier: MatchTier::Description, mask: 0 })
}

/// Live-filter the registry for the console popup: best matches first (exact,
/// prefix, substring, in-order letters, then description hits), shorter names
/// ahead of longer ones inside a tier. Returns at most `limit` rows plus the
/// total number of matches.
pub fn suggest(query: &str, connected: bool, limit: usize) -> (Vec<Suggestion>, usize) {
    let query = query.trim();
    if query.is_empty() {
        return (Vec::new(), 0);
    }
    let mut hits: Vec<Suggestion> = registry(connected).filter_map(|entry| classify(entry, query)).collect();
    let total = hits.len();
    hits.sort_by(|a, b| {
        (a.tier, a.entry.name.len(), a.entry.name.to_ascii_lowercase())
            .cmp(&(b.tier, b.entry.name.len(), b.entry.name.to_ascii_lowercase()))
    });
    hits.truncate(limit);
    (hits, total)
}

/// Return the only registered command/cvar matching `prefix`, if exactly one exists.
/// Exact names should normally be checked with [`find_command`] first so an exact
/// command wins even when it is also a prefix of another registered name.
pub fn unique_prefix(prefix_text: &str, connected: bool) -> Option<&'static Entry> {
    let mut matches = prefix(prefix_text, connected);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Longest common command/cvar prefix for every registered name matching `prefix_text`.
/// Command names are ASCII, so byte indexing is intentional here. The spelling/case of
/// the first registered entry is preserved in the returned completion.
pub fn common_prefix(prefix_text: &str, connected: bool) -> Option<String> {
    let matches: Vec<_> = prefix(prefix_text, connected).collect();
    let first = matches.first()?.name;
    let first_lower = first.to_ascii_lowercase();
    let mut common_len = first_lower.len();

    for entry in matches.iter().skip(1) {
        let other = entry.name.to_ascii_lowercase();
        common_len = first_lower
            .as_bytes()
            .iter()
            .zip(other.as_bytes())
            .take(common_len)
            .take_while(|(a, b)| a == b)
            .count();
    }

    Some(first[..common_len].to_owned())
}

/// Result of completing a word against the connected players' names.
#[derive(Debug, PartialEq, Eq)]
pub enum PlayerNameCompletion {
    None,
    /// The full name with its colour codes, as in the player's `n` userinfo.
    One(String),
    /// Multiple matches, already ordered from closest to loosest match.
    Many(Vec<String>),
}

/// JAPP `CG_ChatboxTabComplete`: `word` is matched against connected player
/// names with colour codes stripped and case ignored. Ambiguous hits are ranked
/// so repeated Tab can cycle in a useful order:
/// exact -> prefix -> word/separator boundary -> interior substring.
pub fn complete_player_name(word: &str, names: &[String]) -> PlayerNameCompletion {
    let needle = crate::logging::strip_jka_colors(word).to_lowercase();
    if needle.is_empty() {
        return PlayerNameCompletion::None;
    }

    let mut hits: Vec<(usize, usize, usize, usize, String)> = names
        .iter()
        .enumerate()
        .filter_map(|(slot_order, name)| {
            let clean = crate::logging::strip_jka_colors(name).to_lowercase();
            let position = clean.find(&needle)?;
            let tier = if clean == needle {
                0
            } else if position == 0 {
                1
            } else {
                let at_boundary = clean[..position]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| !ch.is_alphanumeric());
                if at_boundary { 2 } else { 3 }
            };
            // Within a tier, earlier occurrences and shorter names are closer.
            // Preserve server/client slot order as the final deterministic tie-break.
            Some((tier, position, clean.chars().count(), slot_order, name.clone()))
        })
        .collect();
    hits.sort_by_key(|(tier, position, clean_len, slot_order, _)| {
        (*tier, *position, *clean_len, *slot_order)
    });
    let mut hits: Vec<String> = hits.into_iter().map(|(_, _, _, _, name)| name).collect();

    match hits.len() {
        0 => PlayerNameCompletion::None,
        1 => PlayerNameCompletion::One(hits.remove(0)),
        _ => PlayerNameCompletion::Many(hits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_names_match_anywhere_ignoring_colour_and_case() {
        let names = vec!["^1Red^7Jawa".to_owned(), "Padawan".to_owned(), "".to_owned()];
        assert_eq!(complete_player_name("^2jaw", &names), PlayerNameCompletion::One("^1Red^7Jawa".to_owned()));
        assert_eq!(complete_player_name("zzz", &names), PlayerNameCompletion::None);
        assert_eq!(complete_player_name("", &names), PlayerNameCompletion::None);
        assert_eq!(complete_player_name("^7", &names), PlayerNameCompletion::None);
        // "a" is in both "RedJawa" and "Padawan".
        assert!(matches!(complete_player_name("A", &names), PlayerNameCompletion::Many(hits) if hits.len() == 2));
    }

    #[test]
    fn player_name_completion_ranks_exact_prefix_boundary_then_substring() {
        let names = vec![
            "SuperJawaGuy".to_owned(),
            "RedJawa".to_owned(),
            "JawaBob".to_owned(),
            "Foo_Jawa".to_owned(),
            "^2Jawa".to_owned(),
        ];
        assert_eq!(
            complete_player_name("^7jaw", &names),
            PlayerNameCompletion::Many(vec![
                "^2Jawa".to_owned(),
                "JawaBob".to_owned(),
                "Foo_Jawa".to_owned(),
                "RedJawa".to_owned(),
                "SuperJawaGuy".to_owned(),
            ])
        );
    }

    #[test]
    fn server_commands_complete_only_while_connected() {
        assert!(find_command("team", false).is_none());
        assert_eq!(find_command("TEAM", true).map(|entry| entry.kind), Some(EntryKind::ServerCommand));
        // "tea" also matches teamtask/teamvote: Tab extends to the common "team".
        assert_eq!(common_prefix("tea", true).as_deref(), Some("team"));
        assert_eq!(unique_prefix("callv", true).map(|entry| entry.name), Some("callvote"));
        assert!(unique_prefix("callv", false).is_none());
        // Local commands keep priority and are not listed twice.
        assert_eq!(prefix("say", true).filter(|entry| entry.name == "say").count(), 1);
        let names: Vec<_> = prefix("follow", true).map(|entry| entry.name).collect();
        assert_eq!(
            names,
            ["followFastest", "followRedFlag", "followBlueFlag", "follow", "follownext", "followprev"]
        );
    }

    #[test]
    fn japro_commands_complete_only_on_japro_servers() {
        set_japro_server(false);
        assert!(find_command("whois", true).is_none());
        set_japro_server(true);
        assert_eq!(find_command("whois", true).map(|entry| entry.kind), Some(EntryKind::ServerCommand));
        assert!(find_command("whois", false).is_none(), "not connected: nothing server-side completes");
        assert_eq!(unique_prefix("whoi", true).map(|entry| entry.name), Some("whois"));
        // Vanilla names are listed once even though jaPRO has them too.
        assert_eq!(prefix("callvote", true).count(), 1);
        assert_eq!(prefix("amtele", true).count(), 2, "amTele and amTeleMark");
        set_japro_server(false);
        assert!(find_command("whois", true).is_none());
    }

    #[test]
    fn suggestions_rank_exact_prefix_substring_then_fuzzy() {
        let (hits, total) = suggest("cg_fov", false, 8);
        assert_eq!(hits[0].entry.name, "cg_fov");
        assert_eq!(hits[0].tier, MatchTier::Exact);
        assert!(total >= 1);

        let (hits, _) = suggest("fps", false, 24);
        assert!(hits.iter().any(|hit| hit.entry.name == "com_maxfps" && hit.tier == MatchTier::Substring));
        let first_substring = hits.iter().position(|hit| hit.tier == MatchTier::Substring).unwrap();
        assert!(hits[..first_substring].iter().all(|hit| hit.tier < MatchTier::Substring));

        let (hits, _) = suggest("cgfov", false, 8);
        let fuzzy = hits.iter().find(|hit| hit.entry.name == "cg_fov").expect("cg_fov");
        assert_eq!(fuzzy.tier, MatchTier::Subsequence);
        assert_eq!(fuzzy.mask.count_ones(), 5);
    }

    #[test]
    fn suggestions_ignore_blank_queries_and_hide_server_commands_offline() {
        assert!(suggest("   ", false, 8).0.is_empty());
        assert!(suggest("callvot", false, 8).0.is_empty());
        assert_eq!(suggest("callvot", true, 8).0[0].entry.name, "callvote");
    }

    #[test]
    fn every_vanilla_gcmd_is_registered() {
        for name in [
            "addbot", "callteamvote", "callvote", "duelteam", "follow", "follownext", "followprev",
            "forcechanged", "give", "god", "kill", "levelshot", "loaddefered", "noclip", "notarget",
            "NPC", "say", "say_team", "setviewpos", "siegeclass", "stats", "t_use", "team", "teamtask",
            "teamvote", "tell", "voice_cmd", "vote", "where", "zoom",
        ] {
            assert!(is_server_command(name), "{name}");
        }
        assert!(is_server_command("vgs_cmd"));
        assert_eq!(SERVER_COMMANDS.len(), 36);
    }

    #[test]
    fn unique_prefix_resolves_a_single_entry() {
        assert_eq!(
            unique_prefix("pmove_", false).map(|entry| entry.name),
            Some("pmove_msec")
        );
    }

    #[test]
    fn common_prefix_extends_ambiguous_names() {
        // The current registry has many r_p* entries. Completion should extend as far
        // as every matching name allows, but no farther.
        assert_eq!(common_prefix("r_p", false).as_deref(), Some("r_p"));
        assert_eq!(common_prefix("r_plan", false).as_deref(), Some("r_planardebug"));
    }

    #[test]
    fn fx_ab_cvars_are_registered_for_direct_console_use() {
        for name in [
            "r_flares",
            "r_saberImpactFx",
            "r_fxGeometry",
            "r_fxZeroAlphaDiscard",
        ] {
            let entry = find(name).unwrap_or_else(|| panic!("missing console cvar {name}"));
            assert_eq!(entry.kind, EntryKind::Cvar, "{name}");
        }
    }

    #[test]
    fn command_filter_matches_openjk_style_wildcards() {
        assert!(filter_match("demo*", "demo_speed"));
        assert!(filter_match("?elp", "help"));
        assert!(filter_match("VID_RESTART", "vid_restart"));
        assert!(!filter_match("demo", "demo_speed"));
    }

    #[test]
    fn restart_sensitive_cvars_are_registered_as_latched() {
        assert_eq!(find("r_backend").unwrap().latch, LatchScope::VidRestart);
        assert_eq!(find("r_pbr").unwrap().latch, LatchScope::MapLoadOrVidRestart);
        assert_eq!(
            find("fs_allowAssetOverrides").unwrap().latch,
            LatchScope::MapLoadOrVidRestart
        );
        assert_eq!(find("r_swapInterval").unwrap().latch, LatchScope::None);
    }
}

#[cfg(test)]
mod discrete_option_tests {
    use super::*;

    #[test]
    fn declared_integer_options_are_explicit_only() {
        let discrete = cvar("x", "0", "0|1|2", "");
        assert_eq!(declared_integer_options(&discrete), Some(vec!["0".into(), "1".into(), "2".into()]));
        let annotated = cvar("x", "20", "5|10|20|30|60 seconds", "");
        assert_eq!(declared_integer_options(&annotated), Some(vec!["5".into(), "10".into(), "20".into(), "30".into(), "60".into()]));
        let boolean = cvar("x", "0", "0..1", "");
        assert_eq!(declared_integer_options(&boolean), Some(vec!["0".into(), "1".into()]));
        let slider = cvar("x", "0", "0..10000", "");
        assert_eq!(declared_integer_options(&slider), None);
    }
}
