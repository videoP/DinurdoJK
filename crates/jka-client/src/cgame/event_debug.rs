//! Developer diagnostics for OpenJK/TaystJK entity events.
//!
//! OpenJK's `cg_debugEvents` prints the entity/event number and one DEBUGNAME
//! from `CG_EntityEvent`. DinurdoJK keeps that familiar cvar but reports the
//! full lifecycle: whether `CG_CheckEvents` accepted or suppressed an event,
//! the symbolic protocol-26 name, its semantic category, and the current Rust
//! presentation status.

use super::{ClientGameState, EventCheckDisposition, EventCheckTrace, PresentationEvent};

pub(crate) const EVENT_NAMES: &[&str] = &[
    "EV_NONE",
    "EV_CLIENTJOIN",
    "EV_FOOTSTEP",
    "EV_FOOTSTEP_METAL",
    "EV_FOOTSPLASH",
    "EV_FOOTWADE",
    "EV_SWIM",
    "EV_STEP_4",
    "EV_STEP_8",
    "EV_STEP_12",
    "EV_STEP_16",
    "EV_FALL",
    "EV_JUMP_PAD",
    "EV_GHOUL2_MARK",
    "EV_GLOBAL_DUEL",
    "EV_PRIVATE_DUEL",
    "EV_JUMP",
    "EV_ROLL",
    "EV_WATER_TOUCH",
    "EV_WATER_LEAVE",
    "EV_WATER_UNDER",
    "EV_WATER_CLEAR",
    "EV_ITEM_PICKUP",
    "EV_GLOBAL_ITEM_PICKUP",
    "EV_VEH_FIRE",
    "EV_NOAMMO",
    "EV_CHANGE_WEAPON",
    "EV_FIRE_WEAPON",
    "EV_ALT_FIRE",
    "EV_SABER_ATTACK",
    "EV_SABER_HIT",
    "EV_SABER_BLOCK",
    "EV_SABER_CLASHFLARE",
    "EV_SABER_UNHOLSTER",
    "EV_BECOME_JEDIMASTER",
    "EV_DISRUPTOR_MAIN_SHOT",
    "EV_DISRUPTOR_SNIPER_SHOT",
    "EV_DISRUPTOR_SNIPER_MISS",
    "EV_DISRUPTOR_HIT",
    "EV_DISRUPTOR_ZOOMSOUND",
    "EV_PREDEFSOUND",
    "EV_TEAM_POWER",
    "EV_SCREENSHAKE",
    "EV_LOCALTIMER",
    "EV_USE",
    "EV_USE_ITEM0",
    "EV_USE_ITEM1",
    "EV_USE_ITEM2",
    "EV_USE_ITEM3",
    "EV_USE_ITEM4",
    "EV_USE_ITEM5",
    "EV_USE_ITEM6",
    "EV_USE_ITEM7",
    "EV_USE_ITEM8",
    "EV_USE_ITEM9",
    "EV_USE_ITEM10",
    "EV_USE_ITEM11",
    "EV_USE_ITEM12",
    "EV_USE_ITEM13",
    "EV_USE_ITEM14",
    "EV_USE_ITEM15",
    "EV_ITEMUSEFAIL",
    "EV_ITEM_RESPAWN",
    "EV_ITEM_POP",
    "EV_PLAYER_TELEPORT_IN",
    "EV_PLAYER_TELEPORT_OUT",
    "EV_GRENADE_BOUNCE",
    "EV_MISSILE_STICK",
    "EV_PLAY_EFFECT",
    "EV_PLAY_EFFECT_ID",
    "EV_PLAY_PORTAL_EFFECT_ID",
    "EV_PLAYDOORSOUND",
    "EV_PLAYDOORLOOPSOUND",
    "EV_BMODEL_SOUND",
    "EV_MUTE_SOUND",
    "EV_VOICECMD_SOUND",
    "EV_GENERAL_SOUND",
    "EV_GLOBAL_SOUND",
    "EV_GLOBAL_TEAM_SOUND",
    "EV_ENTITY_SOUND",
    "EV_PLAY_ROFF",
    "EV_GLASS_SHATTER",
    "EV_DEBRIS",
    "EV_MISC_MODEL_EXP",
    "EV_CONC_ALT_IMPACT",
    "EV_MISSILE_HIT",
    "EV_MISSILE_MISS",
    "EV_MISSILE_MISS_METAL",
    "EV_BULLET",
    "EV_PAIN",
    "EV_DEATH1",
    "EV_DEATH2",
    "EV_DEATH3",
    "EV_OBITUARY",
    "EV_POWERUP_QUAD",
    "EV_POWERUP_BATTLESUIT",
    "EV_FORCE_DRAINED",
    "EV_GIB_PLAYER",
    "EV_SCOREPLUM",
    "EV_CTFMESSAGE",
    "EV_BODYFADE",
    "EV_SIEGE_ROUNDOVER",
    "EV_SIEGE_OBJECTIVECOMPLETE",
    "EV_DESTROY_GHOUL2_INSTANCE",
    "EV_DESTROY_WEAPON_MODEL",
    "EV_GIVE_NEW_RANK",
    "EV_SET_FREE_SABER",
    "EV_SET_FORCE_DISABLE",
    "EV_WEAPON_CHARGE",
    "EV_WEAPON_CHARGE_ALT",
    "EV_SHIELD_HIT",
    "EV_DEBUG_LINE",
    "EV_TESTLINE",
    "EV_STOPLOOPINGSOUND",
    "EV_STARTLOOPINGSOUND",
    "EV_TAUNT",
    "EV_ANGER1",
    "EV_ANGER2",
    "EV_ANGER3",
    "EV_VICTORY1",
    "EV_VICTORY2",
    "EV_VICTORY3",
    "EV_CONFUSE1",
    "EV_CONFUSE2",
    "EV_CONFUSE3",
    "EV_PUSHED1",
    "EV_PUSHED2",
    "EV_PUSHED3",
    "EV_CHOKE1",
    "EV_CHOKE2",
    "EV_CHOKE3",
    "EV_FFWARN",
    "EV_FFTURN",
    "EV_CHASE1",
    "EV_CHASE2",
    "EV_CHASE3",
    "EV_COVER1",
    "EV_COVER2",
    "EV_COVER3",
    "EV_COVER4",
    "EV_COVER5",
    "EV_DETECTED1",
    "EV_DETECTED2",
    "EV_DETECTED3",
    "EV_DETECTED4",
    "EV_DETECTED5",
    "EV_LOST1",
    "EV_OUTFLANK1",
    "EV_OUTFLANK2",
    "EV_ESCAPING1",
    "EV_ESCAPING2",
    "EV_ESCAPING3",
    "EV_GIVEUP1",
    "EV_GIVEUP2",
    "EV_GIVEUP3",
    "EV_GIVEUP4",
    "EV_LOOK1",
    "EV_LOOK2",
    "EV_SIGHT1",
    "EV_SIGHT2",
    "EV_SIGHT3",
    "EV_SOUND1",
    "EV_SOUND2",
    "EV_SOUND3",
    "EV_SUSPICIOUS1",
    "EV_SUSPICIOUS2",
    "EV_SUSPICIOUS3",
    "EV_SUSPICIOUS4",
    "EV_SUSPICIOUS5",
    "EV_COMBAT1",
    "EV_COMBAT2",
    "EV_COMBAT3",
    "EV_JDETECTED1",
    "EV_JDETECTED2",
    "EV_JDETECTED3",
    "EV_TAUNT1",
    "EV_TAUNT2",
    "EV_TAUNT3",
    "EV_JCHASE1",
    "EV_JCHASE2",
    "EV_JCHASE3",
    "EV_JLOST1",
    "EV_JLOST2",
    "EV_JLOST3",
    "EV_DEFLECT1",
    "EV_DEFLECT2",
    "EV_DEFLECT3",
    "EV_GLOAT1",
    "EV_GLOAT2",
    "EV_GLOAT3",
    "EV_PUSHFAIL",
    "EV_SIEGESPEC"
];

pub fn event_name(event: i32) -> &'static str {
    usize::try_from(event)
        .ok()
        .and_then(|index| EVENT_NAMES.get(index).copied())
        .unwrap_or("EV_UNKNOWN")
}

pub fn event_category(event: i32) -> &'static str {
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
        event_name(trace.event),
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
        "^2CG EVENT^7 t={} ent={} src={} {}({}) raw=0x{:03x} parm={} client={} weapon={} cat={} eventOnly={} -> {}",
        event.server_time,
        event.entity_num,
        event.source_entity_num,
        event_name(event.event),
        event.event,
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
