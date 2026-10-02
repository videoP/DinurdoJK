//! Jedi Academy `animevents.cfg` parsing ported from OpenJK
//! `ParseAnimationEvtBlock` / `BG_ParseAnimationEvtFile` (`bg_panimate.c`).
//!
//! An animation-event file keys sounds, footsteps, effects and pushes to an
//! absolute frame of the skeleton's animation (`firstFrame` of the named anim
//! plus a frame offset). The client compares the legs/torso bone frame between
//! presentation frames and fires whatever lies in between. Only the footstep
//! payload is decoded here; other event types keep their slot, because the
//! stock parser's "same frame and type overwrites" rule and its fixed slot
//! count depend on them, but carry no data.
//!
//! Every stock humanoid player shares `models/players/_humanoid/animevents.cfg`
//! (`CG_G2EvIndexForModel` only hands other skeletons their own file).

use crate::{
    animation::{animation_index, AnimationSet},
    pk3::AssetSearchPath,
};

pub const HUMANOID_ANIMEVENTS_CFG: &str = "models/players/_humanoid/animevents.cfg";
/// `MAX_ANIM_EVENTS`: slots per block. The stock parser drops the game when
/// a block needs more.
pub const MAX_ANIM_EVENTS: usize = 300;
const MAX_ANIMEVENTS_CFG_BYTES: usize = 80_000 - 1;

/// `footstepType_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FootstepType {
    Right,
    Left,
    HeavyRight,
    HeavyLeft,
}

impl FootstepType {
    /// `footstepTypeTable` (`GetIDForString` is case-insensitive). Anything
    /// else is stored as -1, which every consumer treats like `FOOTSTEP_L`.
    fn from_name(name: &str) -> Self {
        match name.to_ascii_uppercase().as_str() {
            "FOOTSTEP_R" => Self::Right,
            "FOOTSTEP_HEAVY_R" => Self::HeavyRight,
            "FOOTSTEP_HEAVY_L" => Self::HeavyLeft,
            _ => Self::Left,
        }
    }

    /// Run-speed (`HEAVY`) steps use the `*_run` sounds and the heavy print.
    pub fn heavy(self) -> bool {
        matches!(self, Self::HeavyRight | Self::HeavyLeft)
    }

    pub fn right_foot(self) -> bool {
        matches!(self, Self::Right | Self::HeavyRight)
    }
}

/// `animEventType_t` without AEV_NONE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimEventType {
    Sound,
    Footstep,
    Effect,
    Fire,
    Move,
    SoundChan,
    SaberSwing,
    SaberSpin,
}

impl AnimEventType {
    /// `animEventTypeTable`.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "AEV_SOUND" => Self::Sound,
            "AEV_FOOTSTEP" => Self::Footstep,
            "AEV_EFFECT" => Self::Effect,
            "AEV_FIRE" => Self::Fire,
            "AEV_MOVE" => Self::Move,
            "AEV_SOUNDCHAN" => Self::SoundChan,
            "AEV_SABER_SWING" => Self::SaberSwing,
            "AEV_SABER_SPIN" => Self::SaberSpin,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimEvent {
    pub event_type: AnimEventType,
    /// Absolute animation frame: the anim's `firstFrame` plus the listed offset.
    pub key_frame: i32,
    /// `AED_FOOTSTEP_TYPE` and `AED_FOOTSTEP_PROBABILITY` of an AEV_FOOTSTEP
    /// (0 = always, otherwise the percentage chance).
    pub footstep: Option<(FootstepType, i32)>,
}

/// The two event lists of one file: `UPPEREVENTS` (torso) and `LOWEREVENTS` (legs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnimEvents {
    pub torso: Vec<AnimEvent>,
    pub legs: Vec<AnimEvent>,
}

impl AnimEvents {
    /// Every footstep on the legs bone, which is where the stock file keeps them.
    pub fn leg_footsteps(&self) -> impl Iterator<Item = &AnimEvent> {
        self.legs.iter().filter(|event| event.event_type == AnimEventType::Footstep)
    }
}

/// Load the stock humanoid animation events. The file is optional.
pub fn load_humanoid_animation_events(
    search: &mut AssetSearchPath,
    animations: &AnimationSet,
) -> Result<AnimEvents, String> {
    let Some(asset) = search
        .read(HUMANOID_ANIMEVENTS_CFG, MAX_ANIMEVENTS_CFG_BYTES)
        .map_err(|error| format!("failed to read {HUMANOID_ANIMEVENTS_CFG}: {error}"))?
    else {
        return Ok(AnimEvents::default());
    };
    Ok(parse_animation_events(&String::from_utf8_lossy(&asset.bytes), animations))
}

/// Port of the `UPPEREVENTS` / `LOWEREVENTS` block parsing of
/// `BG_ParseAnimationEvtFile`. `include` lines are not followed: no stock
/// humanoid file relies on one.
///
/// Each record is one line, `animName AEV_TYPE frame data...`. A record naming
/// an unknown anim, or one this skeleton does not use (no frames), is skipped.
pub fn parse_animation_events(text: &str, animations: &AnimationSet) -> AnimEvents {
    let mut events = AnimEvents::default();
    let mut pending_torso: Option<bool> = None;
    let mut block: Option<bool> = None;
    for raw_line in text.lines() {
        let line = raw_line.split_once("//").map_or(raw_line, |(before, _)| before);
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let mut index = 0;
        while index < tokens.len() {
            let token = tokens[index];
            match block {
                None => {
                    if token.eq_ignore_ascii_case("UPPEREVENTS") {
                        pending_torso = Some(true);
                    } else if token.eq_ignore_ascii_case("LOWEREVENTS") {
                        pending_torso = Some(false);
                    } else if token == "{" && pending_torso.is_some() {
                        block = pending_torso.take();
                    }
                    index += 1;
                }
                Some(_) if token == "}" => {
                    block = None;
                    index += 1;
                }
                Some(torso) => {
                    let list = if torso { &mut events.torso } else { &mut events.legs };
                    parse_record(&tokens[index..], animations, list);
                    break;
                }
            }
        }
    }
    events
}

/// One record of `ParseAnimationEvtBlock`. `list` is the block's slots, whose
/// length is the parser's `lastAnimEvent`.
fn parse_record(tokens: &[&str], animations: &AnimationSet, list: &mut Vec<AnimEvent>) {
    let [name, kind, frame, data @ ..] = tokens else { return };
    let Some(anim) = animation_index(name) else { return };
    if animations.get(anim as i32).is_none_or(|animation| animation.num_frames == 0) {
        return;
    }
    let Some(event_type) = AnimEventType::from_name(kind) else { return };
    let key_frame = i32::from(animations.animations[anim].first_frame) + atoi(frame);
    // A frame that already has an event of this type is overwritten in place.
    let existing = list.iter().position(|event| event.key_frame == key_frame && event.event_type == event_type);
    if existing.is_none() && list.len() >= MAX_ANIM_EVENTS {
        return;
    }
    let footstep = (event_type == AnimEventType::Footstep).then(|| {
        (
            data.first().map_or(FootstepType::Left, |token| FootstepType::from_name(token)),
            data.get(1).map_or(-1, |token| atoi(token)),
        )
    });
    let event = AnimEvent { event_type, key_frame, footstep };
    match existing {
        Some(slot) => list[slot] = event,
        None => list.push(event),
    }
}

/// C `atoi`: the leading integer, else 0.
fn atoi(token: &str) -> i32 {
    let token = token.trim();
    let end = token
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(token.len(), |(i, _)| i);
    token[..end].parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::parse_animation_cfg;

    fn animations() -> AnimationSet {
        parse_animation_cfg(b"BOTH_STAND1 0 10 0 20\nBOTH_WALK1 100 30 0 20\nBOTH_RUN1 200 20 0 25\n").unwrap()
    }

    const CFG: &str = "\
// comment
UPPEREVENTS
{
    BOTH_STAND1 AEV_SOUND 3 sound/x.wav 0 0 0
}
LOWEREVENTS
{
    BOTH_WALK1 AEV_FOOTSTEP 4 footstep_r 0
    BOTH_WALK1 AEV_FOOTSTEP 19 footstep_l 0   // trailing comment
    BOTH_RUN1 AEV_FOOTSTEP 2 FOOTSTEP_HEAVY_R 50
    BOTH_RUN1 AEV_SOUNDCHAN 7 CHAN_BODY sound/y.wav 0 0 0
    BOTH_DEATH1 AEV_FOOTSTEP 1 footstep_l 0
    BOTH_STAND1 AEV_BOGUS 1
}
";

    #[test]
    fn footsteps_are_keyed_to_absolute_frames() {
        let events = parse_animation_events(CFG, &animations());
        assert_eq!(events.torso.len(), 1);
        let steps: Vec<_> = events.leg_footsteps().map(|e| (e.key_frame, e.footstep.unwrap())).collect();
        assert_eq!(
            steps,
            vec![
                (104, (FootstepType::Right, 0)),
                (119, (FootstepType::Left, 0)),
                (202, (FootstepType::HeavyRight, 50)),
            ],
            "anims with no frames in this skeleton, and unknown event names, are skipped"
        );
        assert_eq!(events.legs.len(), 4, "the AEV_SOUNDCHAN keeps its slot");
    }

    /// The stock file: footsteps for the everyday locomotion anims.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with the stock PK3s"]
    fn stock_humanoid_events_hold_walk_and_run_footsteps() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut search = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let animations = crate::animation::load_humanoid_animations(&mut search).unwrap();
        let events = load_humanoid_animation_events(&mut search, &animations).unwrap();
        let per_anim = |name: &str| {
            let anim = animations.get(animation_index(name).unwrap() as i32).unwrap();
            let range = i32::from(anim.first_frame)..i32::from(anim.first_frame) + i32::from(anim.num_frames);
            events.leg_footsteps().filter(|e| range.contains(&e.key_frame)).count()
        };
        println!("legs {} (footsteps {}), torso {}", events.legs.len(), events.leg_footsteps().count(), events.torso.len());
        for name in ["BOTH_WALK1", "BOTH_RUN1", "BOTH_RUN2", "BOTH_RUNBACK1", "BOTH_WALKBACK1", "BOTH_STAND1"] {
            println!("{name}: {} footsteps", per_anim(name));
        }
        assert!(events.leg_footsteps().count() > 100);
        assert!(per_anim("BOTH_RUN1") >= 2, "a run cycle has at least one step per foot");
    }

    #[test]
    fn a_repeat_of_frame_and_type_overwrites_its_slot() {
        let cfg = "LOWEREVENTS\n{\n BOTH_WALK1 AEV_FOOTSTEP 4 footstep_r 0\n BOTH_WALK1 AEV_FOOTSTEP 4 footstep_heavy_l 10\n}\n";
        let events = parse_animation_events(cfg, &animations());
        assert_eq!(events.legs.len(), 1);
        assert_eq!(events.legs[0].footstep, Some((FootstepType::HeavyLeft, 10)));
    }

    #[test]
    fn braces_may_share_a_line_with_the_block_name() {
        let cfg = "LOWEREVENTS {\n BOTH_RUN1 AEV_FOOTSTEP 0 footstep_r 0 }\nUPPEREVENTS\n{\n}\n";
        let events = parse_animation_events(cfg, &animations());
        assert_eq!(events.legs.len(), 1);
        assert!(events.torso.is_empty());
    }

    #[test]
    fn unknown_footstep_types_fall_back_to_left() {
        let cfg = "LOWEREVENTS\n{\n BOTH_RUN1 AEV_FOOTSTEP 0 footstep_sideways 0\n}\n";
        let events = parse_animation_events(cfg, &animations());
        assert_eq!(events.legs[0].footstep, Some((FootstepType::Left, 0)));
    }

    #[test]
    fn a_block_holds_at_most_max_anim_events() {
        let mut cfg = String::from("LOWEREVENTS\n{\n");
        for frame in 0..(MAX_ANIM_EVENTS + 20) {
            cfg.push_str(&format!(" BOTH_WALK1 AEV_FOOTSTEP {frame} footstep_l 0\n"));
        }
        cfg.push_str("}\n");
        let animations = parse_animation_cfg(b"BOTH_WALK1 0 400 0 20\n").unwrap();
        assert_eq!(parse_animation_events(&cfg, &animations).legs.len(), MAX_ANIM_EVENTS);
    }
}
