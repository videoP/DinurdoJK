//! Cold-path hydraulic reconstruction. A failed plan leaves authored water.
use super::{Body, Face};
use jka_assets::bsp::river::CollisionSurface;
use jka_river::river_promotion::{Cell, Plan, ID_BIT, MAX_CELLS, MAX_PLANS};
use std::collections::VecDeque;

const MAX_RASTER_WORK: usize = 12_000_000;

struct Raster {
    corner: [f32; 2],
    dx: f32,
    dims: [u32; 2],
}
impl Raster {
    fn len(&self) -> usize {
        self.dims[0] as usize * self.dims[1] as usize
    }
    fn xy(&self, i: usize) -> [f32; 2] {
        let w = self.dims[0] as usize;
        [
            self.corner[0] + (i % w) as f32 * self.dx + self.dx * 0.5,
            self.corner[1] + (i / w) as f32 * self.dx + self.dx * 0.5,
        ]
    }
    fn offset(&self, i: usize, x: i32, y: i32) -> Option<usize> {
        let w = self.dims[0] as i32;
        let (x, y) = (i as i32 % w + x, i as i32 / w + y);
        (x >= 0 && y >= 0 && x < w && y < self.dims[1] as i32).then(|| (y * w + x) as usize)
    }
    fn raster(
        &self,
        t: [[f32; 3]; 3],
        budget: &mut usize,
        mut visit: impl FnMut(usize, [f32; 3]),
    ) -> Result<(), String> {
        let a = t[0];
        let b = t[1];
        let c = t[2];
        let d = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if !d.is_finite() || d.abs() < 1e-5 {
            return Ok(());
        }
        let lo: [i32; 2] = std::array::from_fn(|k| {
            ((a[k].min(b[k]).min(c[k]) - self.corner[k]) / self.dx - 0.5).ceil() as i32
        });
        let hi: [i32; 2] = std::array::from_fn(|k| {
            ((a[k].max(b[k]).max(c[k]) - self.corner[k]) / self.dx - 0.5).floor() as i32
        });
        let lo = lo.map(|v| v.max(0));
        let hi: [i32; 2] = std::array::from_fn(|k| hi[k].min(self.dims[k] as i32 - 1));
        if lo[0] > hi[0] || lo[1] > hi[1] {
            return Ok(());
        }
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if *budget == 0 {
                    return Err("raster budget exceeded".into());
                }
                *budget -= 1;
                let i = (y * self.dims[0] as i32 + x) as usize;
                let p = self.xy(i);
                let u = ((p[0] - a[0]) * (c[1] - a[1]) - (p[1] - a[1]) * (c[0] - a[0])) / d;
                let v = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / d;
                if u >= -1e-5 && v >= -1e-5 && u + v <= 1.00001 {
                    visit(i, [1.0 - u - v, u, v]);
                }
            }
        }
        Ok(())
    }
}
const N8: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];
const N4: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
fn neighbors<'a>(g: &'a Raster, mask: &'a [bool], i: usize) -> impl Iterator<Item = usize> + 'a {
    N8.iter()
        .filter_map(move |&(x, y)| g.offset(i, x, y))
        .filter(move |&j| mask[j])
}
/// Zhang-Suen thinning preserves branches and loops. The footprint, not the
/// skeleton, remains the simulation domain.
fn skeleton(g: &Raster, wet: &[bool]) -> Result<Vec<bool>, String> {
    let mut s = wet.to_vec();
    let mut active: Vec<_> = (0..s.len()).filter(|&i| s[i]).collect();
    for _ in 0..256 {
        let mut changed = false;
        for pass in 0..2 {
            let mut remove = Vec::new();
            for &i in &active {
                if !s[i] {
                    continue;
                }
                let n = N8.map(|(x, y)| g.offset(i, x, y).is_some_and(|j| s[j]));
                let count = n.iter().filter(|&&x| x).count();
                if !(2..=6).contains(&count) {
                    continue;
                }
                if (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count() != 1 {
                    continue;
                }
                let blocked = if pass == 0 {
                    (n[0] && n[2] && n[4]) || (n[2] && n[4] && n[6])
                } else {
                    (n[0] && n[2] && n[6]) || (n[0] && n[4] && n[6])
                };
                if !blocked {
                    remove.push(i)
                }
            }
            changed |= !remove.is_empty();
            for i in remove {
                s[i] = false
            }
            active.retain(|&i| s[i]);
        }
        if !changed {
            return Ok(s);
        }
    }
    Err("channel skeleton did not converge".into())
}
fn connected(g: &Raster, wet: &[bool]) -> bool {
    let Some(start) = wet.iter().position(|&v| v) else {
        return false;
    };
    let mut seen = vec![false; wet.len()];
    seen[start] = true;
    let mut q = VecDeque::from([start]);
    let mut n = 0;
    while let Some(i) = q.pop_front() {
        n += 1;
        for (x, y) in N4 {
            if let Some(j) = g.offset(i, x, y) {
                if wet[j] && !seen[j] {
                    seen[j] = true;
                    q.push_back(j)
                }
            }
        }
    }
    n == wet.iter().filter(|&&v| v).count()
}
fn interp(t: [[f32; 3]; 3], w: [f32; 3]) -> f32 {
    (0..3).map(|k| t[k][2] * w[k]).sum()
}
fn close_fall<'a>(p: [f32; 2], z: f32, mut faces: impl Iterator<Item = &'a Face>, dx: f32) -> bool {
    faces.any(|f| {
        let dz = f.max[2] - f.min[2];
        dz > 96.0
            && f.is_fall()
            && (z - f.min[2]).abs() < dx * 4.0
            && (0..2).all(|k| p[k] >= f.min[k] - dx * 4.0 && p[k] <= f.max[k] + dx * 4.0)
    })
}

fn reconstruct(
    body: &Body,
    all: &[Body],
    collision: &[CollisionSurface],
    id: u64,
) -> Result<Plan, String> {
    let mut dx = 16.0f32;
    let g = loop {
        let corner = std::array::from_fn(|k| (body.min[k] / dx).floor() * dx - 2.0 * dx);
        let dimensions: [f32; 2] =
            std::array::from_fn(|k| ((body.max[k] - corner[k]) / dx).ceil() + 2.0);
        if dimensions.iter().all(|&d| (1.0..=768.0).contains(&d)) {
            let g = Raster {
                corner,
                dx,
                dims: dimensions.map(|d| d as u32),
            };
            if g.len() <= MAX_CELLS {
                break g;
            }
        }
        dx *= 2.0;
        if dx > 64.0 {
            return Err("domain too large".into());
        }
    };
    let mut budget = MAX_RASTER_WORK;
    let mut cells = vec![Cell::default(); g.len()];
    let mut alpha = vec![255.0f32; g.len()];
    let mut stacked = false;
    for f in &body.faces {
        for t in &f.triangles {
            g.raster(t.points, &mut budget, |i, w| {
                let z = interp(t.points, w);
                let c = &mut cells[i];
                if c.wet && (z - c.level).abs() > 2.0 {
                    stacked = true;
                    return;
                }
                c.wet = true;
                c.level = z;
                c.bed = f32::NEG_INFINITY;
                c.velocity = t.flow.unwrap_or([0.0; 2]);
                alpha[i] = (0..3).map(|k| f32::from(t.alpha[k]) * w[k]).sum();
            })?
        }
    }
    if stacked {
        return Err("overlapping water elevations".into());
    }
    // Only consider a broad standing surface when the authored river itself
    // fades below it. Never clip arbitrary overlapping cave waters.
    let mut ocean = vec![false; g.len()];
    let mut ocean_level = None;
    for b in all.iter().filter(|b| {
        !b.candidate
            && b.area > 1_000_000.0
            && b.elongation < 2.5
            && b.max[2] - b.min[2] < 2.0
            && body.min[2] < b.min[2]
            && body.max[2] > b.max[2]
    }) {
        let z = b.min[2];
        let mut covered = vec![false; g.len()];
        for f in &b.faces {
            for t in &f.triangles {
                g.raster(t.points, &mut budget, |i, _| covered[i] = true)?
            }
        }
        if !(0..cells.len())
            .any(|i| cells[i].wet && covered[i] && cells[i].level < z && alpha[i] < 64.0)
        {
            continue;
        }
        if ocean_level.is_some() {
            return Err("multiple possible receiving reservoirs".into());
        }
        ocean_level = Some(z);
        for i in 0..cells.len() {
            if cells[i].wet && covered[i] && cells[i].level < z {
                ocean[i] = true;
                cells[i].wet = false
            }
        }
    }
    let authored = cells.iter().filter(|c| c.wet).count();
    if authored < 24 {
        return Err("too few cells".into());
    }
    let mut solid = vec![false; g.len()];
    for s in collision {
        if s.wall {
            continue;
        }
        g.raster(s.triangle, &mut budget, |i, w| {
            let c = &mut cells[i];
            if !c.wet {
                return;
            }
            let z = interp(s.triangle, w);
            if z < c.level - 0.25 {
                c.bed = c.bed.max(z)
            } else if s.solid_bottom_z.is_some_and(|b| b < c.level - 0.25) {
                solid[i] = true
            }
        })?
    }
    let mut missing = 0;
    for (i, c) in cells.iter_mut().enumerate() {
        if c.wet && (!c.bed.is_finite() || c.level - c.bed > 512.0 || solid[i]) {
            c.wet = false;
            missing += 1
        }
    }
    if missing * 100 > authored * 5 {
        return Err(format!(
            "collision bed coverage {}/{}",
            authored - missing,
            authored
        ));
    }
    let wet: Vec<_> = cells.iter().map(|c| c.wet).collect();
    if !connected(&g, &wet) {
        return Err("disconnected collision-resolved channel".into());
    }
    let sk = skeleton(&g, &wet)?;
    let ends: Vec<_> = (0..sk.len())
        .filter(|&i| sk[i] && neighbors(&g, &sk, i).count() == 1)
        .collect();
    if !(2..=12).contains(&ends.len()) {
        return Err(format!("ambiguous channel: {} endpoints", ends.len()));
    }
    struct End {
        lanes: Vec<usize>,
        role: i32,
        known: bool,
        head: f32,
        speed: f32,
    }
    let mut terminals = Vec::new();
    let mut assigned = vec![false; cells.len()];
    for end in ends {
        let mut prev = end;
        let Some(mut inner) = neighbors(&g, &sk, end).next() else {
            continue;
        };
        let mut steps = 1;
        while steps < 8 {
            let mut successors = neighbors(&g, &sk, inner).filter(|&i| i != prev);
            let Some(next) = successors.next() else { break };
            if successors.next().is_some() {
                break;
            }
            prev = inner;
            inner = next;
            steps += 1
        }
        let p = g.xy(end);
        let q = g.xy(inner);
        let d = [p[0] - q[0], p[1] - q[1]];
        let len = d[0].hypot(d[1]);
        if len < dx {
            return Err("short terminal spur".into());
        }
        let dir = d.map(|v| v / len);
        let mut edge = end;
        for k in 1..=64 {
            let off = dir.map(|v| (v * k as f32).round() as i32);
            let Some(j) = g.offset(end, off[0], off[1]) else {
                break;
            };
            if !wet[j] {
                break;
            }
            edge = j
        }
        let ep = g.xy(edge);
        let v = cells[edge].velocity;
        let speed = v[0].hypot(v[1]);
        let dot = dir[0] * v[0] + dir[1] * v[1];
        let drop = cells[inner].level - cells[edge].level;
        let mouth = ocean_level.is_some()
            && (0..ocean.len()).any(|i| {
                ocean[i] && {
                    let p = g.xy(i);
                    (p[0] - ep[0]).hypot(p[1] - ep[1]) < dx * 4.0
                }
            });
        let waterfall = close_fall(
            ep,
            cells[edge].level,
            all.iter().flat_map(|b| b.faces.iter()),
            dx,
        );
        let role = if mouth {
            1
        } else if waterfall {
            -1
        } else if speed > 12.0 && dot.abs() > speed * 0.55 {
            if dot > 0.0 {
                1
            } else {
                -1
            }
        } else if drop.abs() > dx * 0.5 {
            if drop > 0.0 {
                1
            } else {
                -1
            }
        } else {
            0
        };
        // Connected terminal strip, extending across the channel only. Flood
        // within a 3-cell-thick cross-section, not into neighbouring branches.
        let mut lanes = Vec::new();
        let mut queue = VecDeque::from([edge]);
        let mut seen = std::collections::BTreeSet::from([edge]);
        while let Some(i) = queue.pop_front() {
            if !wet[i] {
                continue;
            }
            let a = g.xy(i);
            let along = (a[0] - ep[0]) * dir[0] + (a[1] - ep[1]) * dir[1];
            if !(-dx * 2.5..=dx * 1.5).contains(&along)
                || (a[0] - ep[0]).hypot(a[1] - ep[1]) > dx * 32.0
            {
                continue;
            }
            if assigned[i] {
                return Err("overlapping terminal strips".into());
            }
            lanes.push(i);
            for (x, y) in N4 {
                if let Some(j) = g.offset(i, x, y) {
                    if wet[j] && seen.insert(j) {
                        queue.push_back(j)
                    }
                }
            }
        }
        if lanes.is_empty() {
            return Err("empty terminal".into());
        }
        for &i in &lanes {
            assigned[i] = true
        }
        let head = if mouth {
            ocean_level.unwrap()
        } else {
            cells[edge].level
        };
        eprintln!("[RIVER PROMOTION] terminal {:?} role={} phase={:.1} drop={:.1} waterfall={} ocean={} cells={}",ep,role,speed,drop,waterfall,mouth,lanes.len());
        terminals.push(End {
            lanes,
            role,
            known: role != 0,
            head,
            speed,
        });
    }
    // Ambiguous flat terminals can be explored in preview only if a definite
    // inlet exists. In production no guessed outlet is permitted.
    let has_in = terminals.iter().any(|e| e.role < 0);
    if has_in {
        for e in &mut terminals {
            if e.role == 0 {
                e.role = 1
            }
        }
    }
    if !terminals.iter().any(|e| e.role < 0) || !terminals.iter().any(|e| e.role > 0) {
        return Err("no supported inlet/outlet pair".into());
    }
    for e in &terminals {
        if e.role == 0 {
            continue;
        }
        for &i in &e.lanes {
            let c = &mut cells[i];
            let depth = c.level - c.bed;
            let speed = e.speed.clamp(16.0, 128.0);
            c.exchange = -(e.role as f32) * (depth * speed / (dx * 2.0)).clamp(0.5, 64.0);
            c.target = if e.role < 0 {
                c.level + depth * 0.05
            } else {
                (c.level - depth * 0.10).min(e.head).max(c.bed)
            };
        }
    }
    let replace = missing == 0
        && terminals.iter().all(|e| e.known)
        && body.faces.iter().all(|f| f.complete)
        && ocean_level.is_none(); // receiving water transition remains preview-only until coupled
    eprintln!("[RIVER PROMOTION] plan id={id} grid={:?} dx={dx} wet={} missing={missing} replace={replace}",g.dims,authored-missing);
    Ok(Plan {
        id,
        corner: g.corner,
        dx,
        dims: g.dims,
        cells,
        surfaces: body
            .faces
            .iter()
            .map(|f| (f.surface, f.model, f.shader))
            .collect(),
        replace,
        world_ranges: Vec::new(),
    })
}

pub(in crate::scene) fn build(bsp: &jka_assets::bsp::Bsp, bodies: &[Body]) -> Vec<Plan> {
    if !bodies.iter().any(|b| b.candidate) {
        return Vec::new();
    }
    let collision = match jka_assets::bsp::river::collision_surfaces(bsp) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[RIVER PROMOTION] collision unavailable: {e}");
            return Vec::new();
        }
    };
    let mut plans = Vec::new();
    let mut total = 0;
    for (i, b) in bodies
        .iter()
        .enumerate()
        .filter(|(_, b)| b.candidate)
        .take(MAX_PLANS)
    {
        match reconstruct(b, bodies, &collision, ID_BIT | i as u64) {
            Ok(p) if total + p.cells.len() <= MAX_CELLS * 2 => {
                total += p.cells.len();
                plans.push(p)
            }
            Ok(_) => eprintln!("[RIVER PROMOTION] total cell budget exceeded"),
            Err(e) => eprintln!("[RIVER PROMOTION] body={i} retained: {e}"),
        }
    }
    plans
}
