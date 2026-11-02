//! Replay real Pmove against an installed map; no replacement physics.
//! jump_probe <bsp> <animation.cfg> <output-directory> [legacy|fix|max3] [speed] [hold_ms]
use jka_movement::*;
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: jump_probe <bsp> <animation.cfg> <output-directory> [legacy|fix|max3] [speed] [hold_ms]".into());
    }
    let movement = PmoveContext::new(&fs::read(&args[2])?)?;
    let world = CollisionWorld::from_bsp(&fs::read(&args[1])?)?;
    let out = Path::new(&args[3]);
    let max3 = args.get(4).is_some_and(|arg| arg == "max3");
    let fix = args.get(4).is_some_and(|arg| arg == "fix");
    let speed: f32 = args
        .get(5)
        .map(|arg| arg.parse())
        .transpose()?
        .unwrap_or(0.);
    let hold: i32 = args
        .get(6)
        .map(|arg| arg.parse())
        .transpose()?
        .unwrap_or(1000);
    fs::create_dir_all(out)?;
    let mut summary = BufWriter::new(fs::File::create(out.join("summary.csv"))?);
    writeln!(
        summary,
        "backend,race,level,rate,peak,apex,assignments,contacts"
    )?;
    for backend in [0, 1] {
        for race in [0, 1] {
            if backend == 0 && race == 1 {
                continue;
            }
            for level in [1, 2, 3] {
                for rate in [30, 60, 125, 250, 333] {
                    let settings = PredictSettings {
                        backend,
                        server_mod: backend,
                        ..Default::default()
                    };
                    let mut p =
                        PlayerState::spawn([-14749., 22090., 6485.], 270., JoinMode::Player)?;
                    p.configure(&settings)?;
                    p.set_local_force_jump_slope_fix(fix);
                    p.set_force_jump_level(level)?;
                    if race != 0 {
                        let mut net = p.network();
                        net.stats[11] = 1;
                        net.stats[13] = 1; // MV_JKA
                        p.set_network(&net)?;
                    }
                    let mut map = world.clone();
                    // Identical settled ground state before every run, without a teleport push.
                    for time in (8..=400).step_by(8) {
                        movement.step_command_with_msec(
                            &mut p,
                            UserCmd {
                                server_time: time,
                                angles: [0, 49152, 0],
                                weapon: 3,
                                ..Default::default()
                            },
                            8,
                            &mut map,
                        )?;
                    }
                    if speed != 0. {
                        let mut net = p.network();
                        net.fields[7] = (-speed).to_bits(); // velocity[1], protocol-26 wire order
                        p.set_network(&net)?;
                    }
                    let stem = format!("b{backend}-r{race}-l{level}-hz{rate}");
                    let mut trace =
                        BufWriter::new(fs::File::create(out.join(format!("{stem}.csv")))?);
                    writeln!(trace,"stage,time,msec,ground,walking,ground_plane,flags,entity,bump,x,y,z,inx,iny,inz,outx,outy,outz,nx,ny,nz,jump_start,fraction,overbounce")?;
                    let mut peak = 0_f32;
                    let mut apex = p.view().origin[2];
                    let mut assignments = 0;
                    let mut contacts = 0;
                    let mut previous = 0;
                    let mut n = 1;
                    while previous < 2000 {
                        let command_end = (n * 1000 / rate).min(2000);
                        let mut time = if max3 {
                            command_end.min(previous + 3)
                        } else {
                            command_end
                        };
                        // Give every rate the same input transition time. A command
                        // crossing release is split instead of releasing early.
                        if previous < hold && time > hold {
                            time = hold;
                        }
                        if time == command_end {
                            n += 1;
                        }
                        let msec = time - previous;
                        let cmd = UserCmd {
                            server_time: 400 + time,
                            angles: [0, 49152, 0],
                            weapon: 3,
                            forward_move: 127,
                            up_move: if time <= hold { 127 } else { 0 },
                            ..Default::default()
                        };
                        p.offline_force_tick_with_msec(cmd.server_time, None, msec)?;
                        let (v, events) = movement.trace_command(&mut p, cmd, msec, &mut map)?;
                        peak = peak.max(v.velocity[0].hypot(v.velocity[1]));
                        apex = apex.max(v.origin[2]);
                        for e in events {
                            assignments += i32::from(e.stage == 1);
                            contacts += i32::from(
                                e.stage == 3 && e.normal[2] < -0.01 && e.normal[2] > -0.999,
                            );
                            writeln!(trace,"{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}", e.stage, e.time-400, e.msec,e.ground,e.walking,e.ground_plane,e.flags,e.entity,e.bump,e.origin[0],e.origin[1],e.origin[2],e.before[0],e.before[1],e.before[2],e.after[0],e.after[1],e.after[2],e.normal[0],e.normal[1],e.normal[2],e.jump_start,e.fraction,e.overbounce)?;
                        }
                        previous = time;
                    }
                    writeln!(
                        summary,
                        "{backend},{race},{level},{rate},{peak},{apex},{assignments},{contacts}"
                    )?;
                    println!("{stem}: peak={peak:.3} apex={apex:.3} assignments={assignments} contacts={contacts}");
                }
            }
        }
    }
    Ok(())
}
