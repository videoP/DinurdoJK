//! jaPRO crosshair identification: what is under the crosshair, the colour the
//! crosshair takes on it (`CG_DrawCrosshair`) and the name drawn above it
//! (`CG_DrawCrosshairNames`). Pure decisions over cgame state so the rules stay
//! testable; the trace and drawing live in the app and UI layers.
use super::{
    field_i32, info_value, parse_i32_ascii, ClientGameState, PresentedEntity, CLASS_VEHICLE, CS_PLAYERS, ET_MOVER,
    ET_NPC, MAX_CLIENTS, TEAM_SPECTATOR,
};
use jka_protocol::gamestate::EntityState;

const GT_POWERDUEL: i32 = 4;
const GT_TEAM: i32 = 6;
const GT_SIEGE: i32 = 7;
const TEAM_RED: i32 = 1;
const TEAM_BLUE: i32 = 2;
const WP_SABER: i32 = 2;
const PW_CLOAKED: i32 = 11;
const FP_SEE: i32 = 13;
/// `NPCTEAM_PLAYER` (`teams.h`): NPCs on the player side outside siege.
const NPCTEAM_PLAYER: i32 = 2;

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const YELLOW: [f32; 3] = [1.0, 1.0, 0.0];
const WHITE: [f32; 3] = [1.0; 3];
const BLACK: [f32; 3] = [0.0; 3];
const DUEL_GREY: [f32; 3] = [0.4; 3];

/// The viewed player's state from `cg.snap->ps`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrosshairViewer {
    pub client: i32,
    pub team: i32,
    pub duel_in_progress: bool,
    pub duel_index: i32,
    pub weapon: i32,
}

impl CrosshairViewer {
    pub fn spectator(&self) -> bool {
        self.team == TEAM_SPECTATOR
    }
}

/// Client info key read straight from `CS_PLAYERS + client`.
fn client_key(game: &ClientGameState, client: i32, key: &[u8]) -> Option<i32> {
    let index = CS_PLAYERS.checked_add(u16::try_from(client).ok()?)?;
    game.configstring(index).and_then(|info| info_value(info, key)).and_then(parse_i32_ascii)
}

fn client_team(game: &ClientGameState, client: i32) -> i32 {
    client_key(game, client, b"t").unwrap_or(0)
}

fn client_duel_team(game: &ClientGameState, client: i32) -> i32 {
    client_key(game, client, b"dt").unwrap_or(0)
}

/// `CG_IsMindTricked`: the viewer is hidden from by this entity's mind trick.
pub fn is_mind_tricked(entity: &EntityState, viewer: i32, viewer_state: Option<&EntityState>) -> bool {
    if viewer_state.is_some_and(|state| field_i32(state, "forcePowersActive") & (1 << FP_SEE) != 0) {
        return false;
    }
    let (field, sub) = match viewer {
        v if v > 47 => ("trickedentindex4", 48),
        v if v > 31 => ("trickedentindex3", 32),
        v if v > 15 => ("trickedentindex2", 16),
        _ => ("trickedentindex", 0),
    };
    viewer >= 0 && field_i32(entity, field) & (1 << (viewer - sub)) != 0
}

/// The client whose name a crosshair on `entity` shows, and whether it is the
/// pilot of a vehicle. A vehicle under the crosshair stands in for its pilot.
pub fn identify_client(entity: &PresentedEntity) -> Option<(i32, bool)> {
    if entity.entity_type == ET_NPC && field_i32(&entity.state, "NPC_class") == CLASS_VEHICLE {
        let owner = field_i32(&entity.state, "owner");
        if (0..MAX_CLIENTS as i32).contains(&owner) {
            return Some((owner, true));
        }
    }
    let number = i32::from(entity.number);
    (number < MAX_CLIENTS as i32).then_some((number, false))
}

fn cloaked(state: &EntityState) -> bool {
    field_i32(state, "powerups") & (1 << PW_CLOAKED) != 0
}

/// `CG_DrawCrosshair`'s `chEntValid` test: entities the crosshair reacts to.
fn is_identifiable(entity: &PresentedEntity, viewer: &CrosshairViewer) -> bool {
    let state = &entity.state;
    let mover = entity.entity_type == ET_MOVER;
    i32::from(entity.number) < MAX_CLIENTS as i32
        || entity.entity_type == ET_NPC
        || field_i32(state, "shouldtarget") != 0
        || field_i32(state, "health") != 0
        || (mover && field_i32(state, "bolt1") != 0 && viewer.weapon == WP_SABER)
        || (mover && field_i32(state, "teamowner") != 0)
}

/// The colour `CG_DrawCrosshair` gives the crosshair over `entity`, or `None`
/// when the entity is not one it reacts to (the crosshair keeps its own colour).
/// Siege class/vehicle branches follow the original; the force-mover corona is
/// the only piece not drawn.
pub fn crosshair_color(game: &ClientGameState, viewer: &CrosshairViewer, entity: &PresentedEntity) -> Option<[f32; 3]> {
    if !is_identifiable(entity, viewer) {
        return None;
    }
    let state = &entity.state;
    let gametype = game.gametype();
    let team_game = gametype >= GT_TEAM;
    let my_team = client_team(game, viewer.client);
    let teamowner = field_i32(state, "teamowner");
    let number = i32::from(entity.number);
    let is_npc = entity.entity_type == ET_NPC;
    let mut color = BLACK;

    if cloaked(state) {
        color = WHITE;
    } else if number < MAX_CLIENTS as i32 {
        color = if (team_game && client_team(game, number) == my_team)
            || (gametype == GT_POWERDUEL && client_duel_team(game, number) == client_duel_team(game, viewer.client))
        {
            GREEN
        } else {
            RED
        };
        if viewer.duel_in_progress {
            if number != viewer.duel_index {
                color = DUEL_GREY;
            }
        } else if field_i32(state, "bolt1") != 0 {
            color = DUEL_GREY;
        }
    } else if field_i32(state, "shouldtarget") != 0 || is_npc {
        // The original starts from a stored RGBA that is never filled in; black
        // becomes this warm yellow before the team rules below.
        color = [1.0, 0.8, 0.3];
        if is_npc {
            let player_team = if gametype == GT_SIEGE { viewer.team } else { NPCTEAM_PLAYER };
            color = if teamowner == 0 {
                let owner = field_i32(state, "owner");
                if (0..MAX_CLIENTS as i32).contains(&owner) {
                    // Neutral vehicle: coloured by its pilot.
                    if team_game && client_team(game, owner) == viewer.team { GREEN } else { RED }
                } else {
                    YELLOW
                }
            } else if teamowner != player_team {
                RED
            } else {
                GREEN
            };
        } else if teamowner == TEAM_RED || teamowner == TEAM_BLUE {
            color = if !team_game {
                YELLOW
            } else if teamowner != my_team {
                RED
            } else {
                GREEN
            };
        } else if field_i32(state, "owner") == viewer.client || (team_game && teamowner == my_team) {
            color = GREEN;
        } else if teamowner == 16 || (team_game && teamowner != 0 && teamowner != my_team) {
            color = RED;
        }
    } else if entity.entity_type == ET_MOVER && field_i32(state, "bolt1") != 0 && viewer.weapon == WP_SABER {
        // A mover the saber can push or pull.
        color = [0.2, 0.5, 1.0];
    } else if entity.entity_type == ET_MOVER && teamowner != 0 {
        color = if !team_game {
            YELLOW
        } else if viewer.team != teamowner {
            RED
        } else {
            GREEN
        };
    } else if field_i32(state, "health") != 0 {
        color = if teamowner == 0 || !team_game {
            YELLOW
        } else if teamowner == viewer.team {
            GREEN
        } else {
            RED
        };
    }
    Some(color)
}

/// `cg_drawCrosshairNames` fade: `CG_FadeColor` over `total_ms`, in 0..=1, or
/// `None` once expired. Negative cvar values show the name only while aimed at.
pub fn name_alpha(names: f32, age_ms: i32, on_target_now: bool) -> Option<f32> {
    if names == 0.0 || (names < 0.0 && !on_target_now) {
        return None;
    }
    let total = if names < 0.0 { 1000 } else { (names * 1000.0) as i32 };
    if total == 0 || age_ms >= total {
        return None;
    }
    const FADE_TIME: f32 = 200.0;
    let remaining = (total - age_ms.max(0)) as f32;
    Some(if remaining < FADE_TIME { remaining / FADE_TIME } else { 1.0 })
}

/// Everything `CG_DrawCrosshairNames` decides once a client is under the crosshair.
#[derive(Debug, Clone, PartialEq)]
pub struct CrosshairName {
    pub text: String,
    /// RGB base colour; embedded `^` colour codes still apply on top.
    pub color: [f32; 3],
    pub alpha: f32,
}

/// `CG_SanitizeString`: strips `^` colour codes and control characters.
pub fn sanitize_name(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() && index < 127 {
        let byte = bytes[index];
        if byte == b'^' {
            index += if bytes.get(index + 1).is_some_and(u8::is_ascii_digit) { 2 } else { 1 };
        } else if byte < 32 {
            index += 1;
        } else {
            out.push(byte as char);
            index += 1;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn crosshair_name(
    game: &ClientGameState,
    viewer: &CrosshairViewer,
    target: i32,
    is_pilot: bool,
    target_state: Option<&EntityState>,
    alpha: f32,
    coloured: bool,
    opacity: f32,
) -> Option<CrosshairName> {
    if !(0..MAX_CLIENTS as i32).contains(&target) || target_state.is_some_and(cloaked) {
        return None;
    }
    let index = CS_PLAYERS.checked_add(u16::try_from(target).ok()?)?;
    let name = game.configstring(index).and_then(|info| info_value(info, b"n")).map(super::bytes_to_lossless_ascii)?;

    let gametype = game.gametype();
    let mut color = if coloured {
        WHITE
    } else if gametype >= GT_TEAM {
        if client_team(game, target) == viewer.team { GREEN } else { RED }
    } else if gametype == GT_POWERDUEL
        && !viewer.spectator()
        && client_duel_team(game, target) == client_duel_team(game, viewer.client)
    {
        GREEN
    } else {
        RED
    };
    if viewer.duel_in_progress {
        if target != viewer.duel_index {
            color = BLACK;
        }
    } else if target_state.is_some_and(|state| field_i32(state, "bolt1") != 0) {
        color = BLACK;
    }

    let mut text = if coloured { name } else { sanitize_name(&name) };
    if is_pilot {
        text.push_str(" ^7(pilot)");
    }
    Some(CrosshairName { text, color, alpha: alpha * opacity.clamp(0.0, 1.0) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgame::CS_SERVERINFO;
    use jka_protocol::gamestate::ENTITY_FIELDS;

    fn state(values: &[(&str, u32)]) -> EntityState {
        let mut state = EntityState { number: 0, fields: [0; ENTITY_FIELDS.len()] };
        for (name, value) in values {
            let index = ENTITY_FIELDS.iter().position(|(candidate, _)| candidate == name).unwrap();
            state.fields[index] = *value;
        }
        state
    }

    fn game(gametype: i32, players: &[(i32, i32)]) -> ClientGameState {
        let mut game = ClientGameState::new();
        game.configstrings.insert(CS_SERVERINFO, format!(r"\g_gametype\{gametype}").into_bytes());
        for &(client, team) in players {
            game.configstrings.insert(
                CS_PLAYERS + client as u16,
                format!(r"\n\P{client}\t\{team}").into_bytes(),
            );
        }
        game
    }

    fn player(number: u16) -> PresentedEntity {
        PresentedEntity {
            number,
            entity_type: 1,
            origin: [0.0; 3],
            angles: [0.0; 3],
            state: state(&[]),
        }
    }

    fn viewer(client: i32, team: i32) -> CrosshairViewer {
        CrosshairViewer { client, team, ..CrosshairViewer::default() }
    }

    #[test]
    fn players_are_red_in_ffa_and_green_only_for_teammates() {
        let ffa = game(0, &[(0, 0), (3, 0)]);
        assert_eq!(crosshair_color(&ffa, &viewer(0, 0), &player(3)), Some(RED));
        let team = game(6, &[(0, 1), (3, 1), (4, 2)]);
        assert_eq!(crosshair_color(&team, &viewer(0, 1), &player(3)), Some(GREEN));
        assert_eq!(crosshair_color(&team, &viewer(0, 1), &player(4)), Some(RED));
    }

    #[test]
    fn powerduel_teammates_are_friends() {
        let mut duel = game(4, &[]);
        duel.configstrings.insert(CS_PLAYERS, br"\n\A\t\0\dt\1".to_vec());
        duel.configstrings.insert(CS_PLAYERS + 2, br"\n\B\t\0\dt\1".to_vec());
        duel.configstrings.insert(CS_PLAYERS + 3, br"\n\C\t\0\dt\2".to_vec());
        assert_eq!(crosshair_color(&duel, &viewer(0, 0), &player(2)), Some(GREEN));
        assert_eq!(crosshair_color(&duel, &viewer(0, 0), &player(3)), Some(RED));
    }

    #[test]
    fn duels_grey_the_crosshair_for_everyone_but_the_foe() {
        let ffa = game(0, &[(0, 0), (2, 0), (3, 0)]);
        let mut in_duel = viewer(0, 0);
        in_duel.duel_in_progress = true;
        in_duel.duel_index = 2;
        assert_eq!(crosshair_color(&ffa, &in_duel, &player(2)), Some(RED));
        assert_eq!(crosshair_color(&ffa, &in_duel, &player(3)), Some(DUEL_GREY));
    }

    #[test]
    fn unidentifiable_entities_leave_the_crosshair_alone() {
        let ffa = game(0, &[]);
        let mut crate_entity = player(200);
        crate_entity.entity_type = 4;
        assert_eq!(crosshair_color(&ffa, &viewer(0, 0), &crate_entity), None);
    }

    #[test]
    fn name_fade_follows_the_multi_value_cvar() {
        assert_eq!(name_alpha(0.0, 0, true), None);
        // Positive: seconds since last aimed at, fading over the last 200 ms.
        assert_eq!(name_alpha(1.0, 500, false), Some(1.0));
        assert!((name_alpha(1.0, 900, false).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(name_alpha(1.0, 1000, false), None);
        assert_eq!(name_alpha(3.0, 2500, false), Some((500.0f32 / 200.0).min(1.0)));
        // Negative: only while on target.
        assert_eq!(name_alpha(-1.0, 0, true), Some(1.0));
        assert_eq!(name_alpha(-1.0, 0, false), None);
    }

    #[test]
    fn name_colour_and_text_follow_the_colours_cvar() {
        let team = game(6, &[(0, 1), (3, 1), (4, 2)]);
        let me = viewer(0, 1);
        let plain = crosshair_name(&team, &me, 3, false, None, 1.0, false, 1.0).unwrap();
        assert_eq!((plain.text.as_str(), plain.color), ("P3", GREEN));
        assert_eq!(crosshair_name(&team, &me, 4, false, None, 1.0, false, 1.0).unwrap().color, RED);
        let coloured = crosshair_name(&team, &me, 4, false, None, 0.5, true, 0.5).unwrap();
        assert_eq!((coloured.color, coloured.alpha), (WHITE, 0.25));
        assert!(crosshair_name(&team, &me, 40, false, None, 1.0, true, 1.0).is_none());
        assert_eq!(sanitize_name("^1Red^7 ^xName"), "Red xName");
    }

    #[test]
    fn mind_trick_hides_the_viewer_unless_force_seeing() {
        let tricker = state(&[("trickedentindex2", 1 << 3)]);
        assert!(is_mind_tricked(&tricker, 19, None));
        assert!(!is_mind_tricked(&tricker, 18, None));
    }
}
