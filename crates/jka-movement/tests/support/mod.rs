use jka_movement::{
    CollisionWorld, JoinMode, PlayerState, PlayerView, PmoveContext, UserCmd, TICK_MSEC,
};

pub fn animations() -> PmoveContext {
    // Synthetic timings exercise transitions without redistributing game animation data.
    let table = include_str!("../../vendor/openjk/codemp/cgame/animtable.h");
    let mut cfg = String::new();
    for line in table.lines() {
        if let Some(rest) = line.trim().strip_prefix("ENUM2STRING(") {
            if let Some((name, _)) = rest.split_once(')') {
                let name = name.trim();
                if name.starts_with("BOTH_")
                    || name.starts_with("TORSO_")
                    || name.starts_with("LEGS_")
                {
                    cfg.push_str(&format!("{name} 1 20 -1 20\n"));
                }
            }
        }
    }
    assert!(!cfg.is_empty());
    PmoveContext::new(cfg.as_bytes()).unwrap()
}

#[derive(Default)]
pub struct Map {
    pub boxes: Vec<([f32; 3], [f32; 3], i32)>,
}
impl Map {
    pub fn floor() -> Self {
        Self {
            boxes: vec![([-4096.0, -4096.0, -64.0], [4096.0, 4096.0, 0.0], 1)],
        }
    }
    pub fn box_brush(mut self, mins: [f32; 3], maxs: [f32; 3], contents: i32) -> Self {
        self.boxes.push((mins, maxs, contents));
        self
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut lumps = vec![Vec::new(); 18];
        lumps[0] = b"{\n\"classname\" \"worldspawn\"\n}\0".to_vec();
        for (i, &(mins, maxs, contents)) in self.boxes.iter().enumerate() {
            let mut shader = vec![0u8; 72];
            shader[..4].copy_from_slice(b"test");
            shader[68..72].copy_from_slice(&contents.to_le_bytes());
            lumps[1].extend(shader);
            for side in 0..6 {
                let axis = side / 2;
                for n in 0..3 {
                    float(
                        &mut lumps[2],
                        if n != axis {
                            0.0
                        } else if side % 2 == 0 {
                            -1.0
                        } else {
                            1.0
                        },
                    );
                }
                float(
                    &mut lumps[2],
                    if side % 2 == 0 {
                        -mins[axis]
                    } else {
                        maxs[axis]
                    },
                );
                ints(&mut lumps[9], &[i as i32 * 6 + side as i32, i as i32, -1]);
            }
            ints(&mut lumps[8], &[i as i32 * 6, 6, i as i32]);
            ints(&mut lumps[6], &[i as i32]);
        }
        ints(
            &mut lumps[3],
            &[0, -1, -1, -4096, -4096, -4096, 4096, 4096, 4096],
        );
        ints(
            &mut lumps[4],
            &[
                0,
                0,
                -4096,
                -4096,
                -4096,
                4096,
                4096,
                4096,
                0,
                0,
                0,
                self.boxes.len() as i32,
            ],
        );
        for value in [-4096.0, -4096.0, -4096.0, 4096.0, 4096.0, 4096.0] {
            float(&mut lumps[7], value);
        }
        ints(&mut lumps[7], &[0, 0, 0, self.boxes.len() as i32]);
        let mut bytes = vec![0u8; 152];
        bytes[..4].copy_from_slice(b"RBSP");
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        for (i, lump) in lumps.into_iter().enumerate() {
            let offset = bytes.len() as i32;
            bytes[8 + i * 8..12 + i * 8].copy_from_slice(&offset.to_le_bytes());
            bytes[12 + i * 8..16 + i * 8].copy_from_slice(&(lump.len() as i32).to_le_bytes());
            bytes.extend(lump);
            while !bytes.len().is_multiple_of(4) {
                bytes.push(0);
            }
        }
        bytes
    }
    pub fn world(&self) -> CollisionWorld {
        CollisionWorld::from_bsp(&self.bytes()).unwrap()
    }
}
fn ints(out: &mut Vec<u8>, values: &[i32]) {
    for value in values {
        out.extend(value.to_le_bytes());
    }
}
fn float(out: &mut Vec<u8>, value: f32) {
    out.extend(value.to_le_bytes());
}
pub fn rewrite_lumps(bytes: &[u8], edit: impl FnOnce(&mut [Vec<u8>])) -> Vec<u8> {
    let mut lumps: Vec<_> = (0..18)
        .map(|i| {
            let start =
                i32::from_le_bytes(bytes[8 + i * 8..12 + i * 8].try_into().unwrap()) as usize;
            let len =
                i32::from_le_bytes(bytes[12 + i * 8..16 + i * 8].try_into().unwrap()) as usize;
            bytes[start..start + len].to_vec()
        })
        .collect();
    edit(&mut lumps);
    let mut result = bytes[..152].to_vec();
    for (i, lump) in lumps.into_iter().enumerate() {
        let start = result.len() as i32;
        result[8 + i * 8..12 + i * 8].copy_from_slice(&start.to_le_bytes());
        result[12 + i * 8..16 + i * 8].copy_from_slice(&(lump.len() as i32).to_le_bytes());
        result.extend(lump);
        while !result.len().is_multiple_of(4) {
            result.push(0);
        }
    }
    result
}
pub fn ramp(angle: f32) -> CollisionWorld {
    let map = Map::floor()
        .box_brush([-512.0, -512.0, -512.0], [512.0, 512.0, 1024.0], 1)
        .bytes();
    let bytes = rewrite_lumps(&map, |lumps| {
        let plane = lumps[2].len() as i32 / 16;
        for v in [
            -angle.to_radians().sin(),
            0.0,
            angle.to_radians().cos(),
            0.0,
        ] {
            float(&mut lumps[2], v);
        }
        ints(&mut lumps[9], &[plane, 1, -1]);
        lumps[8][16..20].copy_from_slice(&7i32.to_le_bytes());
    });
    CollisionWorld::from_bsp(&bytes).unwrap()
}
pub fn patch() -> CollisionWorld {
    let bytes = rewrite_lumps(&Map::floor().bytes(), |lumps| {
        for y in [-64.0f32, 0.0, 64.0] {
            for x in [-64.0f32, 0.0, 64.0] {
                let mut row = vec![0u8; 80];
                row[0..4].copy_from_slice(&x.to_le_bytes());
                row[4..8].copy_from_slice(&y.to_le_bytes());
                row[8..12]
                    .copy_from_slice(&(if x == 0.0 { 80.0f32 } else { 48.0f32 }).to_le_bytes());
                row[60..64].copy_from_slice(&1.0f32.to_le_bytes());
                lumps[10].extend(row);
            }
        }
        let mut surface = vec![0u8; 148];
        surface[8..12].copy_from_slice(&2i32.to_le_bytes());
        surface[16..20].copy_from_slice(&9i32.to_le_bytes());
        surface[28..36].fill(255);
        for i in 0..4 {
            surface[36 + i * 4..40 + i * 4].copy_from_slice(&(-1i32).to_le_bytes());
        }
        surface[140..144].copy_from_slice(&3i32.to_le_bytes());
        surface[144..148].copy_from_slice(&3i32.to_le_bytes());
        lumps[13] = surface;
        ints(&mut lumps[5], &[0]);
        lumps[4][36..40].copy_from_slice(&1i32.to_le_bytes());
        lumps[7][28..32].copy_from_slice(&1i32.to_le_bytes());
    });
    CollisionWorld::from_bsp(&bytes).unwrap()
}
pub fn player(z: f32) -> PlayerState {
    PlayerState::spawn([0.0, 0.0, z], 0.0, JoinMode::Player).unwrap()
}
pub fn tick(
    movement: &PmoveContext,
    player: &mut PlayerState,
    world: &mut CollisionWorld,
    forward: i8,
    right: i8,
    up: i8,
) -> PlayerView {
    let cmd = UserCmd {
        server_time: player.view().command_time + TICK_MSEC,
        forward_move: forward,
        right_move: right,
        up_move: up,
        ..UserCmd::default()
    };
    movement.step(player, cmd, world).unwrap()
}
