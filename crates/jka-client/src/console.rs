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

pub const ENTRIES: &[Entry] = &[
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
        "cg_debugEvents",
        "0",
        "0|1|2|3",
        "Event diagnostics: 1 accepted+status, 2 adds suppressed candidates, 3 adds entity/resource detail.",
    ),
    cvar(
        "cg_drawCrosshair",
        "1",
        "0..6",
        "Crosshair shape: 0 off; 1 classic; 2 dot; 3 plus; 4 classic+dot; 5 brackets; 6 box.",
    ),
    cvar(
        "cg_crosshairSize",
        "24",
        "4..96",
        "Crosshair size using the stock JKA cg_crosshairSize convention.",
    ),
    cvar(
        "cg_crosshairColor",
        "255 255 255 230",
        "R G B A (0..255)",
        "TaystJK-style crosshair RGBA color.",
    ),
    cvar("cg_movementKeys", "0", "0..4", "TaystJK movement-key HUD mode: 0 off, 1 original, 2 +attack, 3 compact centered, 4 compact movable."),
    cvar("cg_movementKeysX", "0", "float", "TaystJK movement-key HUD horizontal offset."),
    cvar("cg_movementKeysY", "0", "float", "TaystJK movement-key HUD vertical offset."),
    cvar("cg_movementKeysSize", "1", "0.25..4", "TaystJK movement-key HUD scale."),
    cvar("cg_movementKeysWalk", "0", "0|1", "Show the walk/run key in the movement-key HUD."),
    cvar("cg_strafeHelper", "3008", "bitmask", "TaystJK Strafehelper style/direction bitmask."),
    cvar("cg_strafeHelper_FPS", "0", "0..1000", "Strafehelper physics FPS override; 0 follows com_maxfps, then falls back to 125 when uncapped."),
    cvar("cg_strafeHelperOffset", "75", "float", "TaystJK Strafehelper angular offset in hundredths of a degree."),
    cvar("cg_strafeHelperLineWidth", "1", "0.25..5", "Strafehelper line width."),
    cvar("cg_strafeHelperPrecision", "256", "100..10000", "TaystJK Strafehelper projection precision/sensitivity."),
    cvar("cg_strafeHelperCutoff", "0", "0..480", "Strafehelper line cutoff."),
    cvar("cg_strafeHelperActiveColor", "0 255 0 200", "R G B A", "TaystJK active Strafehelper line color."),
    cvar("cg_strafeHelperInactiveAlpha", "200", "0..255", "TaystJK inactive Strafehelper line alpha."),
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
    cvar("s_volume", "0.5", "0..1", "OpenJK game/effects volume."),
    cvar("s_volumeVoice", "1.0", "0..1", "OpenJK voice-channel volume."),
    cvar("s_musicvolume", "0.25", "0..1", "OpenJK background music volume (playback presenter pending)."),
    cvar("s_separation", "0.5", "0..1", "OpenJK stereo separation used by positional sound."),
    cvar(
        "s_muteWhenUnfocused",
        "1",
        "0..1",
        "Mute game and voice audio while the game window is unfocused.",
    ),
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
        "cg_fxFPS",
        "90",
        "0 or 15..250",
        "Continuous projectile/trail EFX sampling rate. 0 restores legacy JKA presentation-frame-driven density.",
    ),
    cvar(
        "cg_smoothPlayerOrigin",
        "1",
        "0..1",
        "Interpolate the local player model origin between fixed pmove ticks.",
    ),
    cvar(
        "cg_smoothThirdPersonOrigin",
        "1",
        "0..1",
        "Interpolate the third-person camera target origin between fixed pmove ticks.",
    ),
    cvar(
        "cg_smoothPlayerAnimation",
        "1",
        "0..1",
        "Drive local Ghoul2 animation/angle presentation from continuous presentation time.",
    ),
    cvar(
        "cg_subframePlayerAngles",
        "1",
        "0..1",
        "Use cl_input_subframe view angles for the local player's visual pose.",
    ),
    cvar(
        "cg_smoothThirdPersonTime",
        "1",
        "0..1",
        "Drive third-person camera damping from continuous presentation time.",
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
        "0..1",
        "Enable additional developer and renderer debug overlays.",
    ),
    cvar(
        "pmove_msec",
        "8",
        "1..33",
        "Fixed player movement timestep in milliseconds.",
    ),
    cvar(
        "cl_input_subframe",
        "0",
        "0..1",
        "Apply mouse-look on each raw mouse event instead of waiting for the client tick.",
    ),
    cvar("r_physics", "0", "0..1", "Master switch for Rapier client-side visual physics."),
    cvar("r_physicsHz", "60", "30|60|120|240", "Fixed timestep for client-side visual physics."),
    cvar("r_physicsMaxSubsteps", "4", "1|2|4|8", "Maximum visual-physics catch-up steps after a slow frame."),
    cvar("r_physicsCCD", "1", "0..1", "Enable continuous collision detection for eligible visual physics bodies."),
    cvar("r_physicsSleeping", "1", "0..1", "Allow inactive Rapier bodies to sleep."),
    cvar("r_ragdolls", "1", "0..1", "Enable client-side visual ragdolls."),
    cvar("r_ragdollMax", "8", "2|4|8|16|32", "Maximum active client ragdolls."),
    cvar("r_ragdollLifetime", "20", "5|10|20|30|60 seconds", "Lifetime of simulated client ragdolls."),
    cvar("r_ragdollSelfCollision", "0", "0..1", "Allow limbs on one ragdoll to collide with each other."),
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
    command("cmdlist", "List registered console commands, optionally filtered by a wildcard pattern."),
    command("help", "Print help for one registered command."),
    command("echo", "Print message text to the console."),
    command("vstr", "Execute the current value of a cvar as command text."),
    command("wait", "Pause execution of the remaining command buffer for one or more client frames."),
    command("record", "Start recording the current live session to a native demos/*.dm_26 file."),
    command("stoprecord", "Stop the current demo recording and write its end marker."),
    command("demo", "Play demos/<demoname>.dm_26 using the same playback path as the GUI."),
    command("exec", "Execute a .cfg script through the active fs_game/base VFS."),
    command("execq", "Execute a .cfg script without displaying the exec notification."),
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
    command("trace", "Toggle inspection of the world surface under the center crosshair."),
    command("trace_clear", "Clear the pinned surface inspection and triangle highlight."),
    command("toggle", "OpenJK-style cvar toggle: toggle <cvar> [value1 value2 ...]."),
    command("messagemode", "Open global chat input."),
    command("+scores", "Show the live scoreboard while held and request fresh scores."),
    command("-scores", "Hide the live scoreboard."),
    command("messagemode2", "Open team chat input."),
    cvar("r_swapInterval", "0", "0..1", "Vertical synchronization."),
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
    cvar(
        "r_ext_texture_filter_anisotropic",
        "0",
        "0|2|4|8|16",
        "Anisotropic texture filtering level.",
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
    cvar(
        "r_gamma",
        "1.000",
        "0.5..3.0",
        "Display gamma/brightness adjustment.",
    ),
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
        "Short-range screen-space contact shadows.",
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
        "r_fogMode",
        "off",
        "off|legacy|volumetric",
        "Fog rendering mode.",
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
        "r_dofQuality",
        "adaptive",
        "performance|adaptive|high",
        "DOF Gaussian sampling quality. Adaptive spends extra samples only on large blur radii.",
    ),
    cvar("r_colorLut", "off", "preset", "Color grading LUT preset."),
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
        "r_saberMarks",
        "legacy",
        "off|legacy|enhanced",
        "Saber/world contact effects: off, OpenJK-compatible marks, or a smooth heat-accumulating molten relief/sludge effect.",
    ),
    cvar(
        "r_dynamicShadows",
        "off",
        "off|blob_stencil|csm|ray_traced",
        "Dynamic-shadow mode; CSM uses the current cascaded-shadow path.",
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
    cvar("r_showtris", "0", "0..1", "Wireframe triangle overlay."),
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
        "Potentially-visible-set culling mode.",
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
    command("serverinfo", "Print the connected server's serverinfo configstring."),
    command("systeminfo", "Print the connected server's systeminfo configstring."),
    cvar("name", "Padawan", "string", "Player name sent in userinfo."),
    cvar("rate", "25000", "1000..90000", "Maximum bytes/second the server may send."),
    cvar("snaps", "40", "1..125", "Snapshots per second requested from the server."),
    cvar("cl_maxpackets", "60", "15..1000", "Maximum client packets per second."),
    cvar("cl_timeNudge", "0", "-900..900", "Milliseconds of extra (+) or less (-) interpolation delay."),
    cvar("saber1", "single_1", "saber name", "Primary saber sent in userinfo."),
    cvar("saber2", "none", "saber name", "Secondary saber sent in userinfo."),
    cvar("color1", "4", "0..5", "Primary saber colour."),
    cvar("color2", "4", "0..5", "Secondary saber colour."),
    cvar("forcepowers", "7-1-032330000000001333", "rank-side-levels", "Force power configuration."),
    cvar("sex", "male", "male|female", "Sound gender hint sent in userinfo."),
    cvar("password", "", "string", "Server join password."),
    cvar("cg_errorDecay", "100", "0..500", "Milliseconds over which prediction corrections are smoothed."),
    cvar("cg_noPredict", "0", "0..1", "Disable client-side movement prediction (use interpolated server state)."),
    cvar("cg_showMiss", "0", "0..1", "Print prediction misses."),
    cvar(
        "cl_allowMissingMap",
        "0",
        "0..1",
        "Keep a live server connection active without world geometry when the server BSP is not installed locally.",
    ),
    cvar("cl_allowHttpDownload", "1", "0..1", "Allow HTTP PK3 autodownloads advertised by the server. HTTP is preferred when available."),
    cvar("cl_allowDownload", "1", "0..1", "Allow legacy Jedi Academy UDP PK3 autodownloads (svc_download). Used when HTTP is unavailable or fails."),
    cvar("cl_commandRate", "125", "15..1000", "Usercmds sent per second (OpenJK ties this to com_maxfps; 125 is the classic JKA rate)."),
    // cgame weapon selection (cg_weapons.c).
    command("weapon", "Select a weapon slot: weapon <1..13>."),
    command("weapnext", "Select the next weapon."),
    command("weapprev", "Select the previous weapon."),
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
    server_command("tell", "Private message: tell <client number|name> <message>."),
    server_command("voice_cmd", "Send a voice command."),
    server_command("vote", "Vote in a vote: vote <yes|no>."),
    server_command("where", "Print your current origin."),
    server_command("zoom", "Toggle binocular zoom."),
];

/// Whether `name` is a vanilla server game command (forwarded while connected).
pub fn is_server_command(name: &str) -> bool {
    SERVER_COMMANDS.iter().any(|entry| entry.name.eq_ignore_ascii_case(name))
}

/// Every name the console knows. Server commands exist only while connected,
/// matching OpenJK's cgame registration lifetime; a server command that
/// shares a name with a local command is listed once (the local entry).
fn registry(connected: bool) -> impl Iterator<Item = &'static Entry> {
    let server: &'static [Entry] = if connected { SERVER_COMMANDS } else { &[] };
    ENTRIES.iter().chain(server.iter().filter(|entry| {
        !ENTRIES.iter().any(|local| local.name.eq_ignore_ascii_case(entry.name))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(names, ["follow", "follownext", "followprev"]);
    }

    #[test]
    fn every_vanilla_gcmd_is_registered() {
        for name in [
            "addbot", "callteamvote", "callvote", "duelteam", "follow", "follownext", "followprev",
            "forcechanged", "give", "god", "kill", "levelshot", "loaddefered", "noclip", "notarget",
            "NPC", "say", "say_team", "setviewpos", "siegeclass", "stats", "team", "teamtask",
            "teamvote", "tell", "voice_cmd", "vote", "where", "zoom",
        ] {
            assert!(is_server_command(name), "{name}");
        }
        assert_eq!(SERVER_COMMANDS.len(), 29);
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
        assert_eq!(common_prefix("r_plan", false).as_deref(), Some("r_planar"));
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
