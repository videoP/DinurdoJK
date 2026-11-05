//! Studio HUD allowlist shared by project data and the CFG sandbox.

/// Explicit Studio-only HUD allowlist. No gameplay cvar is written or applied.
pub const HUD_ELEMENTS: &[(&str, &str)] = &[
    ("core", "Health / armor / ammo / force"),
    ("crosshair", "Crosshair"),
    ("race_timer", "Race timer"),
    ("game_timer", "Game timer"),
    ("speedometer", "Speedometer"),
    ("strafe_helper", "Strafe helper"),
    ("movement_keys", "Movement keys"),
    ("follow", "Followed player"),
    ("score", "Player score"),
    ("mini_scores", "Team / duel scores (cg_drawScores)"),
    ("enemy_info", "Enemy / duel info"),
    ("crosshair_name", "Crosshair name"),
    ("player_names", "Player names"),
    ("player_names_cinematic", "Cinematic player names"),
];

pub fn studio_hud_key(key: &str) -> bool {
    HUD_ELEMENTS.iter().any(|(id,_)| *id == key)
}

