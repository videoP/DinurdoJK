use jka_assets::{
    bsp::{Bsp, MAX_FILE_BYTES},
    pk3::AssetSearchPath,
};
use jka_movement::{CollisionWorld, JoinMode, PlayerState, PmoveContext, UserCmd, TICK_MSEC};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let base = args
        .get(1)
        .ok_or("Usage: offline_check <base folder> [mp/ffa3]")?;
    let map = args.get(2).map(String::as_str).unwrap_or("mp/ffa3");
    let mut assets = AssetSearchPath::open(Path::new(base))?;
    let animations = assets
        .read("models/players/_humanoid/animation.cfg", 60_000)?
        .ok_or("Missing animation.cfg")?;
    let data = assets
        .read(&format!("maps/{map}.bsp"), MAX_FILE_BYTES)?
        .ok_or("Missing map")?;
    let bsp = Bsp::parse(&data.bytes)?;
    eprintln!("Validated BSP; loading collision");
    let mut world = CollisionWorld::from_bsp(&data.bytes)?;
    eprintln!("Loaded collision; loading animations");
    let movement = PmoveContext::new(&animations.bytes)?;
    eprintln!("Loaded animations; simulating");
    for spawn in bsp.deathmatch_spawns() {
        let mut origin = spawn.origin;
        origin[2] += 9.0;
        let mut player = PlayerState::spawn(origin, spawn.yaw, JoinMode::Player)?;
        for tick in 1..=250 {
            let cmd = UserCmd {
                server_time: tick * TICK_MSEC,
                ..UserCmd::default()
            };
            movement.step(&mut player, cmd, &mut world)?;
        }
        let grounded = player.view();
        if !grounded.grounded() {
            return Err(format!("Spawn did not land: {:?}", grounded).into());
        }
        let mut maximum = grounded.origin[2];
        let mut saw_air = false;
        for tick in 251..=750 {
            let cmd = UserCmd {
                server_time: tick * TICK_MSEC,
                up_move: if tick < 400 { 127 } else { 0 },
                ..UserCmd::default()
            };
            let view = movement.step(&mut player, cmd, &mut world)?;
            maximum = maximum.max(view.origin[2]);
            saw_air |= !view.grounded();
        }
        if !saw_air {
            return Err("Force jump did not leave ground".into());
        }
        println!(
            "spawn {:?}: ground {:.3}, jump rise {:.3}, force {}, landed {}",
            spawn.origin,
            grounded.origin[2],
            maximum - grounded.origin[2],
            player.view().force_power,
            player.view().grounded()
        );
    }
    Ok(())
}
