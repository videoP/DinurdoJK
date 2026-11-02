//! Deterministic world inputs for the native bridge diagnostic. This supplies
//! collision geometry; movement, animation, weapons and Force remain in MVSDK.
use super::{Player, Settings, State, Version, SOURCE_REVISION};
use crate::{TraceQuery, TraceResult, TraceWorld, UserCmd, ENTITY_WORLD};

#[derive(Clone, Copy, Default)]
pub(super) struct FixtureWorld {
    pub water: bool,
    pub wall: bool,
}
impl TraceWorld for FixtureWorld {
    fn trace(&mut self, q: TraceQuery) -> TraceResult {
        let mut t = TraceResult::clear(q.end);
        if q.mask & 1 == 0 {
            return t;
        }
        for (axis, sign, limit) in [(2, 1.0, 0.0), (0, -1.0, -256.0)] {
            if axis == 0 && !self.wall {
                continue;
            }
            let extent = if sign > 0.0 {
                q.mins[axis]
            } else {
                q.maxs[axis]
            };
            let a = (q.start[axis] + extent) * sign - limit;
            let b = (q.end[axis] + extent) * sign - limit;
            if a < -0.001 {
                t.start_solid = 1;
                if b < 0.0 {
                    t.all_solid = 1;
                    t.fraction = 0.0;
                    t.end = q.start;
                }
            } else if b < 0.0 {
                let f = (a / (a - b)).clamp(0.0, 1.0);
                if f < t.fraction {
                    t.fraction = f;
                    t.end = std::array::from_fn(|i| q.start[i] + f * (q.end[i] - q.start[i]));
                    t.normal = [0.0; 3];
                    t.normal[axis] = sign;
                    t.distance = limit;
                    t.entity = ENTITY_WORLD;
                    t.contents = 1;
                }
            }
        }
        t
    }
    fn point_contents(&mut self, point: [f32; 3], _: i32) -> i32 {
        if point[2] < 0.0 {
            1
        } else if self.water && point[2] < 120.0 {
            32
        } else {
            0
        }
    }
}

pub(super) fn seed(version: Version, weapon: u8) -> State {
    let mut state = State::zero(version);
    for (name, bits) in [
        ("commandTime", 1000),
        ("clientNum", 1),
        ("gravity", 800),
        ("speed", 320),
        ("basespeed", 320),
        ("groundEntityNum", 1022),
        ("weapon", weapon.into()),
        ("saberEntityNum", 32),
        ("fd.saberAnimLevel", 1),
        ("fd.forcePower", 100),
        ("fd.forcePowerLevel[FP_LEVITATION]", 3),
        ("fd.forcePowerLevel[FP_SEE]", 3),
        ("origin[2]", 24.0f32.to_bits()),
    ] {
        state.set_field_bits(name, bits).expect("pinned JK2 schema");
    }
    state.stats[0] = 100;
    state.stats[4] = 0xffff;
    state.stats[8] = 100;
    state.ammo.fill(999);
    state
}

pub(super) fn commands(weapon: u8) -> Vec<UserCmd> {
    let mut time = 1000;
    (0..192)
        .map(|i| {
            time += if i == 170 {
                1700
            } else if i % 13 == 0 {
                17
            } else {
                16
            };
            UserCmd {
                server_time: if i == 171 {
                    time - 16
                } else if i == 172 {
                    1000
                } else {
                    time
                },
                weapon,
                force_selection: 1,
                inventory_selection: 0,
                forward_move: if i < 150 { 127 } else { 0 },
                right_move: if (32..64).contains(&i) { 127 } else { 0 },
                up_move: if (64..84).contains(&i) {
                    127
                } else if (100..124).contains(&i) {
                    -127
                } else {
                    0
                },
                buttons: if (128..140).contains(&i) {
                    1
                } else if (145..155).contains(&i) {
                    128
                } else {
                    0
                },
                ..UserCmd::default()
            }
        })
        .collect()
}

/// Runs the same compiled native SDK through two independently scheduled call
/// paths. It checks the host boundary, not agreement with a separate server.
pub fn run(animations: &[u8]) -> Result<String, String> {
    let mut cases = 0;
    let mut total = 0;
    for wire in [Version::V102, Version::V104] {
        for gameplay in [Version::V102, Version::V104] {
            for weapon in [2, 4] {
                // JK2 saber and blaster identifiers
                let seed = Player::new(wire, animations, &seed(wire, weapon))?;
                let commands = commands(weapon);
                for (fixed, msec) in [(0, 8), (1, 8), (1, 16), (1, 33)] {
                    let settings = Settings {
                        pmove_fixed: fixed,
                        pmove_msec: msec,
                        ..Settings::new(gameplay)
                    };
                    for world in [
                        FixtureWorld::default(),
                        FixtureWorld {
                            wall: true,
                            water: false,
                        },
                        FixtureWorld {
                            wall: false,
                            water: true,
                        },
                    ] {
                        let label = format!("wire={wire:?} gameplay={gameplay:?} weapon={weapon} fixed={fixed}/{msec} water={} wall={}",world.water,world.wall);
                        total += super::verify_replay(&seed, &commands, &settings, || world)
                            .map_err(|e| format!("{label}: {e}"))?;
                        let mut player = seed.try_clone()?;
                        let mut moving_world = world;
                        for &cmd in &commands {
                            player.predict(cmd, &settings, &mut moving_world)?;
                        }
                        let state = player.network();
                        let x = f32::from_bits(state.field_bits("origin[0]").unwrap());
                        if !x.is_finite() || x < 1.0 {
                            return Err(format!("{label}: native replay did not move: x={x}"));
                        }
                        if world.wall && x > 241.0 {
                            return Err(format!("{label}: native player crossed wall: x={x}"));
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    Ok(format!("Pinned MVSDK {SOURCE_REVISION}\nJK2 native bridge verified: {cases} cases, {total} commands, complete native state compared after every command.\n1.02/1.04 wire and gameplay selection; saber/blaster; floor, wall, water; jump, crouch, attacks; variable/fixed 8/16/33 ms; stale command backlog.\nThis compares the host with direct calls to the same native SDK. This diagnostic does not validate live prediction."))
}
