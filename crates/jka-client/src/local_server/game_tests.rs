//! End-to-end checks of the solo mover game: the real Pmove player, driven
//! through the same tick hook the client uses, against inline-model doors,
//! plats and buttons in a small synthetic collision BSP.

use std::{collections::HashSet, time::Duration};

use jka_movement::{JoinMode, UserCmd};
use winit::keyboard::KeyCode;

use super::*;
use crate::{
    player::LocalPlayer,
    scene::{render_position, SpawnPoint},
};

#[allow(dead_code)]
mod support {
    include!("../../../jka-movement/tests/support/mod.rs");
}

/// A floor at z=0 plus one inline model per box (model N = `boxes[N - 1]`).
fn world_with(boxes: &[(V3, V3)]) -> CollisionWorld {
    let mut map = support::Map::floor();
    for &(mins, maxs) in boxes {
        map = map.box_brush(mins, maxs, 1);
    }
    let bytes = support::rewrite_lumps(&map.bytes(), |lumps| {
        // The world leaf holds only the floor; the other brushes belong to inline models.
        let end = lumps[4].len();
        lumps[4][end - 4..].copy_from_slice(&1i32.to_le_bytes());
        let mut models = Vec::new();
        let floats = |models: &mut Vec<u8>, values: [f32; 3]| values.iter().for_each(|v| models.extend(v.to_le_bytes()));
        let ints = |models: &mut Vec<u8>, values: [i32; 4]| values.iter().for_each(|v| models.extend(v.to_le_bytes()));
        floats(&mut models, [-4096.0; 3]);
        floats(&mut models, [4096.0; 3]);
        ints(&mut models, [0, 0, 0, 1]);
        for (index, &(mins, maxs)) in boxes.iter().enumerate() {
            floats(&mut models, mins);
            floats(&mut models, maxs);
            ints(&mut models, [0, 0, 1 + index as i32, 1]);
        }
        lumps[7] = models;
    });
    CollisionWorld::from_bsp(&bytes).unwrap()
}

fn brush(model: u32, bounds: (V3, V3), vars: &[(&str, &str)]) -> MapBrushEntity {
    MapBrushEntity {
        model,
        mins: bounds.0,
        maxs: bounds.1,
        train_corners: Vec::new(),
        spawn_vars: vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
    }
}

fn point(vars: &[(&str, &str)]) -> MapBrushEntity {
    brush(0, ([0.0; 3], [0.0; 3]), vars)
}

fn game_of(entities: &[MapBrushEntity]) -> MoverGame {
    let movers = super::super::movers::build_brush_mover_entities(entities, 100, 1024);
    MoverGame::new(movers.entities, movers.sources, movers.trains, entities, None)
}

struct Sim {
    player: LocalPlayer,
    game: MoverGame,
    keys: HashSet<KeyCode>,
}

impl Sim {
    /// A joined player at the origin, facing +X, in a map with `boxes` as models.
    fn new(boxes: &[(V3, V3)], entities: &[MapBrushEntity]) -> Self {
        let spawn = SpawnPoint { position: render_position([0.0, 0.0, 24.125]), yaw: 0.0, ..SpawnPoint::default() };
        let mut player = LocalPlayer::new(Some(support::animations()), Some(world_with(boxes)), spawn).unwrap();
        player.join(JoinMode::Player, spawn).unwrap();
        Self { player, game: game_of(entities), keys: HashSet::new() }
    }

    fn walking(mut self) -> Self {
        self.keys.insert(KeyCode::KeyW);
        self
    }

    fn run(&mut self, milliseconds: u32) {
        for _ in 0..milliseconds / 8 {
            self.player
                .update_with_game(Duration::from_millis(8), &self.keys, (0.0, 0.0), UserCmd::default(), &mut self.game)
                .unwrap();
        }
    }

    fn origin(&self) -> V3 {
        self.player.view().origin
    }

    fn ent(&self, class: Class) -> &GEnt {
        self.game.ents.iter().find(|ent| ent.class == class).unwrap()
    }
}

const WALL: (V3, V3) = ([100.0, -64.0, 0.0], [116.0, 64.0, 128.0]);

#[test]
fn a_closed_door_is_solid_to_the_local_player() {
    let door = brush(1, WALL, &[("classname", "func_door"), ("angle", "-1"), ("targetname", "shut")]);
    let mut sim = Sim::new(&[WALL], &[door]).walking();
    sim.run(2000);
    let x = sim.origin()[0];
    assert!(x > 60.0 && x < 86.0, "player should be stopped by the door at x=100, is at {x}");
}

#[test]
fn walking_up_to_a_door_opens_it_and_lets_the_player_through() {
    let door = brush(1, WALL, &[("classname", "func_door"), ("angle", "-1")]);
    let mut sim = Sim::new(&[WALL], &[door]).walking();
    sim.run(2000);
    assert!(sim.origin()[0] > 140.0, "player should have walked through, is at {:?}", sim.origin());
    // It rises the lip-adjusted height and publishes the motion to clients.
    assert_eq!(sim.ent(Class::Door).pos2, [0.0, 0.0, 120.0]);
    assert_eq!(sim.game.published().count(), 1);
}

#[test]
fn a_door_closes_again_after_its_wait() {
    let door = brush(1, WALL, &[("classname", "func_door"), ("angle", "-1"), ("wait", "1")]);
    let mut sim = Sim::new(&[WALL], &[door]);
    // Standing in the door's trigger opens it...
    sim.run(200);
    assert_ne!(sim.ent(Class::Door).mover_state, MoverState::Pos1);
    // ...and once the player is gone, its wait closes it.
    sim.player.teleport([-400.0, 0.0, 24.0], [0.0; 3]).unwrap();
    sim.run(3000);
    assert_eq!(sim.ent(Class::Door).mover_state, MoverState::Pos1);
    assert_eq!(sim.ent(Class::Door).pos.kind, TR_STATIONARY);
}

#[test]
fn a_plat_lifts_the_player_standing_on_it() {
    // Model bounds are relative to the entity origin (its top position).
    let bounds = ([-64.0, -64.0, -16.0], [64.0, 64.0, 0.0]);
    let plat = brush(1, bounds, &[("classname", "func_plat"), ("origin", "0 0 128"), ("height", "128")]);
    let mut sim = Sim::new(&[bounds], &[plat]);
    let start = sim.origin()[2];
    sim.run(1600);
    assert_eq!(sim.ent(Class::Plat).mover_state, MoverState::Pos2);
    assert!(sim.origin()[2] > start + 120.0, "the player should ride up, is at {:?}", sim.origin());
}

#[test]
fn touching_a_button_uses_its_door() {
    let button_bounds = ([44.0, -8.0, 0.0], [52.0, 8.0, 48.0]);
    let button = brush(1, button_bounds, &[("classname", "func_button"), ("angle", "0"), ("target", "gate")]);
    let gate = brush(2, WALL, &[("classname", "func_door"), ("angle", "-1"), ("targetname", "gate")]);
    let mut sim = Sim::new(&[button_bounds, WALL], &[button, gate]).walking();
    sim.run(1200);
    assert_ne!(sim.ent(Class::Button).mover_state, MoverState::Pos1, "the button should have been pressed");
    assert_ne!(sim.ent(Class::Door).mover_state, MoverState::Pos1, "the button's target should have opened");
}

#[test]
fn a_door_that_would_crush_the_player_reverses_instead() {
    // A ceiling slab 8 units over the player's head that comes down 20.
    let slab = ([-32.0, -32.0, 72.0], [32.0, 32.0, 100.0]);
    let door = brush(1, slab, &[("classname", "func_door"), ("angle", "-2")]);
    let mut sim = Sim::new(&[slab], &[door]);
    let start = sim.origin();
    sim.run(1500);
    let door = sim.ent(Class::Door);
    assert_ne!(door.mover_state, MoverState::Pos2, "it must not close on the player");
    assert!(door.origin[2] > -12.0, "the slab bottoms out on the player's head: {:?}", door.origin);
    assert!((sim.origin()[2] - start[2]).abs() < 0.5, "the player is not pushed into the floor");
}

#[test]
fn the_use_key_presses_player_usable_buttons() {
    let bounds = ([30.0, -8.0, 0.0], [38.0, 8.0, 72.0]);
    let usable = brush(1, bounds, &[("classname", "func_button"), ("angle", "0"), ("spawnflags", "64")]);
    let plain = brush(1, bounds, &[("classname", "func_button"), ("angle", "0")]);
    for (button, pressed) in [(usable, true), (plain, false)] {
        let mut sim = Sim::new(&[bounds], &[button]);
        sim.run(40);
        let view = sim.player.view();
        let mut world = world_with(&[bounds]);
        sim.game.after_pmove(64, Some((&view, BUTTON_USE)), &mut world);
        let state = sim.ent(Class::Button).mover_state;
        assert_eq!(state != MoverState::Pos1, pressed, "state {state:?}");
    }
}

#[test]
fn open_areaportals_join_areas_in_the_snapshot_mask() {
    let mut game = game_of(&[]);
    game.area_count = 3;
    game.portals = vec![0; 9];
    // Closed: a view sees only its own area.
    assert_eq!(game.area_mask(Some(0))[0], 0xff & !1);
    game.portals[1] = 1;
    game.portals[3] = 1;
    assert_eq!(game.area_mask(Some(0))[0], 0xff & !0b11);
    assert_eq!(game.area_mask(Some(1))[0], 0xff & !0b11);
    assert_eq!(game.area_mask(Some(2))[0], 0xff & !0b100);
    // A view in solid sees everything.
    assert_eq!(game.area_mask(None), [0; 32]);
}

#[test]
fn relays_and_delays_forward_a_use_to_a_door() {
    let gate = brush(1, WALL, &[("classname", "func_door"), ("angle", "-1"), ("targetname", "gate")]);
    let relay = point(&[("classname", "target_relay"), ("targetname", "r"), ("target", "d")]);
    let delay = point(&[("classname", "target_delay"), ("targetname", "d"), ("target", "gate"), ("wait", "0.5")]);
    let mut game = game_of(&[gate, relay, delay]);
    let mut world = world_with(&[WALL]);
    let relay = game.ents.iter().position(|ent| ent.class == Class::Relay).unwrap();
    let gate = game.ents.iter().position(|ent| ent.class == Class::Door).unwrap();
    game.run_frame(1000, None, &mut world);
    game.global_use(relay, relay, Activator::Player);
    game.run_frame(1400, None, &mut world);
    assert_eq!(game.ents[gate].mover_state, MoverState::Pos1, "the delay has not elapsed");
    game.run_frame(1600, None, &mut world);
    assert_ne!(game.ents[gate].mover_state, MoverState::Pos1);
}

#[test]
fn a_team_of_doors_moves_together_and_shares_one_trigger() {
    let left = brush(1, ([100.0, -64.0, 0.0], [116.0, 0.0, 128.0]), &[("classname", "func_door"), ("angle", "-1"), ("team", "pair")]);
    let right = brush(2, ([100.0, 0.0, 0.0], [116.0, 64.0, 128.0]), &[("classname", "func_door"), ("angle", "-1"), ("team", "pair")]);
    let mut game = game_of(&[left, right]);
    let mut world = world_with(&[
        ([100.0, -64.0, 0.0], [116.0, 0.0, 128.0]),
        ([100.0, 0.0, 0.0], [116.0, 64.0, 128.0]),
    ]);
    assert!(game.ents[1].slave);
    assert_eq!(game.ents[0].team_chain, Some(1));
    game.run_frame(1000, None, &mut world);
    // Door trigger spawned by the first think, around the whole team.
    game.run_frame(1100, None, &mut world);
    assert_eq!(game.ents.iter().filter(|ent| ent.class == Class::TriggerDoor).count(), 1);
    game.use_binary_mover(1, None, Activator::Player);
    assert_eq!(game.ents[0].mover_state, MoverState::OneToTwo);
    assert_eq!(game.ents[1].mover_state, MoverState::OneToTwo);
}

#[test]
fn a_door_with_a_sound_set_plays_start_loop_and_end_sounds() {
    let lift = brush(1, WALL, &[("classname", "func_door"), ("angle", "-1"), ("targetname", "lift"), ("soundSet", "piston")]);
    let quiet = brush(2, ([200.0, -64.0, 0.0], [216.0, 64.0, 128.0]), &[("classname", "func_door"), ("angle", "-1"), ("targetname", "hush")]);
    let mut game = game_of(&[lift, quiet]);
    let mut world = world_with(&[WALL, ([200.0, -64.0, 0.0], [216.0, 64.0, 128.0])]);
    assert_eq!(game.sound_sets(), ["piston"]);
    let (lift, quiet) = (0, 1);
    let field = |game: &MoverGame, i: usize, name: &str| game.ents[i].state.as_ref().unwrap().field_i32(name).unwrap();
    assert_eq!(field(&game, lift, "soundSetIndex"), 1, "CS_AMBIENT_SET + 1 names the set");
    assert_eq!(field(&game, quiet, "soundSetIndex"), 0);

    game.run_frame(1000, None, &mut world);
    game.use_binary_mover(lift, None, Activator::Player);
    let start = field(&game, lift, "event");
    assert_eq!(start & !EV_EVENT_BITS, EntityEvent::EV_PLAYDOORSOUND.as_i32());
    assert_eq!(field(&game, lift, "eventParm"), BMS_START);
    assert_eq!((field(&game, lift, "loopIsSoundset"), field(&game, lift, "loopSound")), (1, BMS_MID));

    // The event is withdrawn once it is too old to matter, the loop carries on.
    game.run_frame(1000 + EVENT_VALID_MSEC + 10, None, &mut world);
    assert_eq!(field(&game, lift, "event"), 0);
    assert_eq!(field(&game, lift, "loopSound"), BMS_MID);

    // Arriving at the top stops the loop and posts the end sound with flipped toggle bits.
    game.run_frame(6000, None, &mut world);
    assert_eq!(game.ents[lift].mover_state, MoverState::Pos2);
    assert_eq!(field(&game, lift, "loopSound"), 0);
    assert_eq!(field(&game, lift, "loopIsSoundset"), 0);
    let end = field(&game, lift, "event");
    assert_eq!((end & !EV_EVENT_BITS, field(&game, lift, "eventParm")), (EntityEvent::EV_PLAYDOORSOUND.as_i32(), BMS_END));

    // A mover without a soundSet never posts anything.
    game.use_binary_mover(quiet, None, Activator::Player);
    assert_eq!(field(&game, quiet, "event"), 0);
    assert_eq!(field(&game, quiet, "loopIsSoundset"), 0);
}

/// Every stock deathmatch map with movers: spawn the game from the real BSP,
/// walk a player around its spawns for a few seconds and make sure nothing
/// panics. Prints what each map spawned.
#[test]
#[ignore = "requires JKA_TEST_BASE with the stock PK3s"]
fn stock_maps_spawn_and_run_their_movers() {
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = jka_assets::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let mut ran = 0;
    for map in ["mp/ffa1", "mp/ffa2", "mp/ffa3", "mp/ffa4", "mp/ffa5", "mp/ctf1", "mp/ctf2", "mp/ctf3", "mp/ctf4", "mp/ctf5", "mp/duel1", "mp/duel4", "mp/duel7", "mp/siege_hoth", "mp/siege_desert", "mp/siege_korriban"] {
        let Some(file) = assets.read(&format!("maps/{map}.bsp"), 128 << 20).unwrap() else { continue };
        let bsp = jka_assets::bsp::Bsp::parse(&file.bytes).unwrap();
        let entities = crate::scene::bsp_brush_entities(&bsp);
        let movers = super::super::movers::build_brush_mover_entities(&entities, 100, 1024);
        let areas = bsp.visibility.as_ref().map(jka_assets::bsp::Visibility::area_locator);
        let mut game = MoverGame::new(movers.entities, movers.sources, movers.trains, &entities, areas);
        let mut world = CollisionWorld::from_bsp(&file.bytes).unwrap();
        let spawns = bsp.deathmatch_spawns();
        println!("{map}: {} brush/logic entities -> {}; {} spawn(s)", entities.len(), game.summary(), spawns.len());
        for spawn in spawns.iter().take(4) {
            let origin = [spawn.origin[0], spawn.origin[1], spawn.origin[2] + 24.0];
            let mut state = PlayerState::spawn(origin, 0.0, JoinMode::Player).unwrap();
            let pmove = support::animations();
            let mut solo = crate::player::LocalPlayer::new(Some(pmove), Some(world.clone()), SpawnPoint { position: render_position(origin), yaw: 0.0, ..SpawnPoint::default() }).unwrap();
            solo.join(JoinMode::Player, SpawnPoint { position: render_position(origin), yaw: 0.0, ..SpawnPoint::default() }).unwrap();
            let keys: HashSet<KeyCode> = [KeyCode::KeyW].into();
            for _ in 0..500 {
                solo.update_with_game(Duration::from_millis(8), &keys, (0.0, 0.0), UserCmd::default(), &mut game).unwrap();
            }
            let _ = &mut state;
            ran += 1;
        }
        let _ = &mut world;
        let (door_triggers, movers_moving) = (
            game.ents.iter().filter(|ent| ent.class == Class::TriggerDoor).count(),
            game.ents.iter().filter(|ent| ent.is_moving()).count(),
        );
        println!("   door triggers spawned: {door_triggers}, moving now: {movers_moving}");
    }
    assert!(ran > 0, "no stock map found under JKA_TEST_BASE");
}
