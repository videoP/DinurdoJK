//! Developer diagnostics for OpenJK/TaystJK entity events.
//!
//! OpenJK's `cg_debugEvents` prints the entity/event number and one DEBUGNAME
//! from `CG_EntityEvent`. DinurdoJK keeps that familiar cvar but reports the
//! full lifecycle: whether `CG_CheckEvents` accepted or suppressed an event,
//! the symbolic protocol-26 name, its semantic category, and the current Rust
//! presentation status.

use super::{ClientGameState, EventCheckDisposition, EventCheckTrace, PresentationEvent};
use jka_protocol::entity_event::EntityEvent;

pub fn event_name(event: EntityEvent) -> &'static str {
    event.name()
}

fn event_name_num(event: i32) -> &'static str {
    EntityEvent::from_i32(event).map(EntityEvent::name).unwrap_or("EV_UNKNOWN")
}

pub fn event_category(event: EntityEvent) -> &'static str {
    let name = event_name(event);
    if name.starts_with("EV_SABER") || name.starts_with("EV_DEFLECT") {
        "SABER"
    } else if matches!(
        name,
        "EV_FIRE_WEAPON"
            | "EV_ALT_FIRE"
            | "EV_CHANGE_WEAPON"
            | "EV_NOAMMO"
            | "EV_WEAPON_CHARGE"
            | "EV_WEAPON_CHARGE_ALT"
            | "EV_VEH_FIRE"
    ) {
        "WEAPON"
    } else if name.contains("MISSILE")
        || name == "EV_BULLET"
        || name.starts_with("EV_DISRUPTOR")
        || name == "EV_CONC_ALT_IMPACT"
        || name == "EV_GRENADE_BOUNCE"
    {
        "PROJECTILE"
    } else if name.contains("EFFECT")
        || matches!(name, "EV_GHOUL2_MARK" | "EV_GLASS_SHATTER" | "EV_DEBRIS" | "EV_MISC_MODEL_EXP" | "EV_SHIELD_HIT")
    {
        "FX"
    } else if name.contains("SOUND")
        || name.starts_with("EV_VOICE")
        || matches!(name, "EV_PREDEFSOUND" | "EV_MUTE_SOUND" | "EV_STARTLOOPINGSOUND" | "EV_STOPLOOPINGSOUND")
    {
        "SOUND"
    } else if name.contains("ITEM") || name == "EV_BECOME_JEDIMASTER" {
        "ITEM"
    } else if name.starts_with("EV_FOOT")
        || name.starts_with("EV_STEP_")
        || matches!(name, "EV_SWIM" | "EV_FALL" | "EV_JUMP" | "EV_JUMP_PAD" | "EV_ROLL" | "EV_WATER_TOUCH" | "EV_WATER_LEAVE" | "EV_WATER_UNDER" | "EV_WATER_CLEAR")
    {
        "MOVEMENT"
    } else if name.starts_with("EV_DEATH")
        || matches!(name, "EV_PAIN" | "EV_OBITUARY" | "EV_GIB_PLAYER" | "EV_BODYFADE")
    {
        "DAMAGE"
    } else if name.starts_with("EV_PLAYER_TELEPORT") {
        "TELEPORT"
    } else if name.starts_with("EV_SIEGE") || name == "EV_CTFMESSAGE" {
        "GAME"
    } else if name.starts_with("EV_ANGER")
        || name.starts_with("EV_VICTORY")
        || name.starts_with("EV_CONFUSE")
        || name.starts_with("EV_PUSHED")
        || name.starts_with("EV_CHOKE")
        || name.starts_with("EV_CHASE")
        || name.starts_with("EV_COVER")
        || name.starts_with("EV_DETECTED")
        || name.starts_with("EV_LOST")
        || name.starts_with("EV_OUTFLANK")
        || name.starts_with("EV_ESCAPING")
        || name.starts_with("EV_GIVEUP")
        || name.starts_with("EV_LOOK")
        || name.starts_with("EV_SIGHT")
        || name.starts_with("EV_SUSPICIOUS")
        || name.starts_with("EV_COMBAT")
        || name.starts_with("EV_JDETECTED")
        || name.starts_with("EV_JCHASE")
        || name.starts_with("EV_JLOST")
        || name.starts_with("EV_GLOAT")
        || name.starts_with("EV_TAUNT")
        || name.starts_with("EV_SOUND")
    {
        "VOICE/AI"
    } else {
        "MISC"
    }
}

pub fn trace_line(trace: &EventCheckTrace) -> Option<String> {
    let disposition = match trace.disposition {
        EventCheckDisposition::Accepted => return None,
        EventCheckDisposition::NoEvent => return None,
        EventCheckDisposition::Duplicate => "SUPPRESSED_DUPLICATE",
        EventCheckDisposition::Zero => "SUPPRESSED_ZERO",
    };
    Some(format!(
        "^3CG EVENT CHECK^7 t={} src={} ent={} eType={} {}({}) raw=0x{:03x} prev=0x{:03x} eventOnly={} -> {}",
        trace.server_time,
        trace.source_entity_num,
        trace.entity_num,
        trace.entity_type,
        event_name_num(trace.event),
        trace.event,
        trace.raw_event,
        trace.previous_event,
        u8::from(trace.event_only_entity),
        disposition,
    ))
}

pub fn accepted_line(event: &PresentationEvent, status: &str) -> String {
    let client = event.state.field_i32("clientNum").unwrap_or(-1);
    let weapon = event.state.field_i32("weapon").unwrap_or(0);
    format!(
        "^2CG EVENT^7 q={} t={} ent={} src={} {}({}) raw=0x{:03x} parm={} client={} weapon={} cat={} eventOnly={} -> {}",
        event.receive_sequence,
        event.server_time,
        event.entity_num,
        event.source_entity_num,
        event_name(event.event),
        event.event.as_i32(),
        event.raw_event,
        event.parm,
        client,
        weapon,
        event_category(event.event),
        u8::from(event.event_only_entity),
        status,
    )
}

pub fn verbose_lines(event: &PresentationEvent, game: &ClientGameState) -> Vec<String> {
    let state = &event.state;
    let field = |name| state.field_i32(name).unwrap_or(0);
    let mut lines = vec![format!(
        "^5  STATE^7 eType={} eFlags=0x{:08x} client={} weapon={} other={} other2={} model={} model2={} generic1={} pos={:.1},{:.1},{:.1}",
        field("eType"),
        field("eFlags"),
        field("clientNum"),
        field("weapon"),
        field("otherEntityNum"),
        field("otherEntityNum2"),
        field("modelindex"),
        field("modelindex2"),
        field("generic1"),
        event.position[0], event.position[1], event.position[2],
    )];

    let name = event_name(event.event);
    if matches!(name, "EV_GENERAL_SOUND" | "EV_GLOBAL_SOUND" | "EV_ENTITY_SOUND" | "EV_STARTLOOPINGSOUND" | "EV_GRENADE_BOUNCE" | "EV_MISSILE_STICK") {
        if let Some(qpath) = game.sound_qpath(event.parm) {
            lines.push(format!("^5  RESOURCE^7 sound[{}]={}", event.parm, qpath));
        }
    }
    if matches!(name, "EV_PLAY_EFFECT_ID" | "EV_PLAY_PORTAL_EFFECT_ID") {
        if let Some(qpath) = game.effect_qpath(event.parm) {
            lines.push(format!("^5  RESOURCE^7 effect[{}]={}", event.parm, qpath));
        }
    }
    lines
}
