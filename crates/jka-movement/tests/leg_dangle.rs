#[allow(dead_code)]
mod support;
use jka_movement::{JoinMode, PlayerState, PredictSettings, UserCmd};
use support::{animations, ramp};

const SLOPE_DEGREES: f32 = 40.0;

/// `*l_leg_foot` / `*r_leg_foot` in model space (x = forward, y = left), 20
/// units apart along the facing direction, which climbs the ramp.
const FEET: [[f32; 3]; 2] = [[10.0, 6.0, -24.0], [-10.0, -6.0, -24.0]];

/// Stand still halfway up the ramp for about a second and report the legs anim.
fn settle(settings: PredictSettings, bolts: Option<[[f32; 3]; 2]>) -> String {
    let movement = animations();
    let mut world = ramp(SLOPE_DEGREES);
    let x = 100.0;
    let z = x * SLOPE_DEGREES.to_radians().tan() + 24.125;
    let mut p = PlayerState::spawn([x, 0.0, z], 0.0, JoinMode::Player).unwrap();
    p.configure(&settings).unwrap();
    p.set_foot_bolts(bolts).unwrap();
    for i in 1..=120 {
        let cmd = UserCmd { server_time: i * 8, ..Default::default() };
        movement.predict(&mut p, cmd, &settings, &mut world).unwrap();
    }
    p.view().legs_animation_name()
}

/// LEGS_LEFTUPn plus the per-stance LEGS_Sn_LUPn variants.
fn left_up(name: &str) -> bool {
    name.starts_with("LEGS_LEFTUP") || name.contains("_LUP")
}

fn right_up(name: &str) -> bool {
    name.starts_with("LEGS_RIGHTUP") || name.contains("_RUP")
}

fn japro() -> PredictSettings {
    PredictSettings { server_mod: 1, ..PredictSettings::default() }
}

#[test]
fn standing_on_a_slope_predicts_the_left_up_legs_anim() {
    for settings in [PredictSettings::default(), japro()] {
        let name = settle(settings, Some(FEET));
        assert!(left_up(&name), "{name}");
    }
}

#[test]
fn without_foot_bolts_pmove_skips_the_slope_anims_like_a_missing_ghoul2() {
    for settings in [PredictSettings::default(), japro()] {
        let name = settle(settings, None);
        assert!(!left_up(&name) && !right_up(&name), "{name}");
    }
}

#[test]
fn feet_the_other_way_round_predict_the_right_up_anim() {
    let swapped = [FEET[1], FEET[0]];
    for settings in [PredictSettings::default(), japro()] {
        let name = settle(settings, Some(swapped));
        assert!(right_up(&name), "{name}");
    }
}
