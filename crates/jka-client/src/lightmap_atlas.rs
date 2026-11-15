//! Padded lightmap atlas built once on map load.
use glam::Vec3;
use std::collections::{BTreeMap, BTreeSet};
const PAGE: usize = 128;
const PAD: usize = 2;
const TILE: usize = PAGE + PAD * 2;

#[derive(Debug)]
pub struct Atlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    columns: usize,
    page_count: usize,
    receiver_offsets: BTreeMap<(usize, usize), [i32; 2]>,
}
impl Atlas {
    pub fn new(bytes: &[u8]) -> Self {
        let count = bytes.len() / (PAGE * PAGE * 3);
        assert!(count > 0 && bytes.len().is_multiple_of(PAGE * PAGE * 3));
        let columns = (count as f64).sqrt().ceil() as usize;
        let width = columns * TILE;
        let height = count.div_ceil(columns) * TILE;
        let mut rgba = vec![0; width * height * 4];
        for (page, data) in bytes
            .as_chunks::<{ PAGE * PAGE * 3 }>()
            .0
            .iter()
            .enumerate()
        {
            for y in 0..TILE {
                for x in 0..TILE {
                    let sx = x.saturating_sub(PAD).min(PAGE - 1);
                    let sy = y.saturating_sub(PAD).min(PAGE - 1);
                    let src = (sy * PAGE + sx) * 3;
                    let dst =
                        (((page / columns) * TILE + y) * width + (page % columns) * TILE + x) * 4;
                    rgba[dst..dst + 3].copy_from_slice(&data[src..src + 3]);
                    rgba[dst + 3] = 255;
                }
            }
        }
        Self {
            rgba,
            width: width as u32,
            height: height as u32,
            columns,
            page_count: count,
            receiver_offsets: BTreeMap::new(),
        }
    }

    pub fn uv(&self, page: usize, uv: [f32; 2]) -> [f32; 2] {
        [
            ((page % self.columns * TILE + PAD) as f32 + uv[0] * PAGE as f32) / self.width as f32,
            ((page / self.columns * TILE + PAD) as f32 + uv[1] * PAGE as f32) / self.height as f32,
        ]
    }

    pub fn receiver_uv(&self, page: usize, uv: [f32; 2], surface: usize, slot: usize) -> [f32; 2] {
        let offset = self
            .receiver_offsets
            .get(&(surface, slot))
            .copied()
            .unwrap_or([0; 2]);
        let base = self.uv(page, uv);
        [
            base[0] + offset[0] as f32 / self.width as f32,
            base[1] + offset[1] as f32 / self.height as f32,
        ]
    }

    /// A compiler can reuse baked RGB texels for unrelated surfaces. Their
    /// visibility is not interchangeable: a room and a roof need independent
    /// receivers even when their original illumination happens to match.
    /// Copy only colliding charts, including the original bilinear footprint.
    /// Original RGB and UV derivatives are preserved with relighting disabled.
    pub fn separate_receivers(&mut self, meshes: &[&jka_assets::bsp::Mesh]) -> usize {
        let triangles: Vec<_> = meshes
            .iter()
            .flat_map(|mesh| {
                mesh.batches.iter().flat_map(move |batch| {
                    (0..4)
                        .filter(move |&slot| {
                            batch.lightmaps[slot] >= 0 && batch.lightmap_styles[slot] < 254
                        })
                        .flat_map(move |slot| {
                            mesh.indices[batch.indices.clone()].chunks_exact(3).map(
                                move |indices| {
                                    let vertices =
                                        [0, 1, 2].map(|i| mesh.vertices[indices[i] as usize]);
                                    Receiver {
                                        key: (batch.surface, slot),
                                        page: batch.lightmaps[slot] as usize,
                                        positions: vertices.map(|v| Vec3::from_array(v.position)),
                                        uvs: vertices.map(|v| v.lightmap_uv[slot]),
                                    }
                                },
                            )
                        })
                })
            })
            .collect();
        self.separate_triangles(&triangles)
    }

    fn separate_triangles(&mut self, triangles: &[Receiver]) -> usize {
        // Bound temporary raster storage as well as retained atlas growth.
        let size = self.width as usize * self.height as usize;
        if size > 4 * 1024 * 1024 {
            return 0;
        }
        let mut owners = vec![None::<(usize, Vec3)>; size];
        let mut bounds = BTreeMap::<(usize, usize), [i32; 4]>::new();
        let width = self.width as i32;
        let height = self.height as i32;
        let texel_uv = |r: &Receiver| {
            r.uvs.map(|uv| {
                [
                    (r.page % self.columns * TILE + PAD) as f32 + uv[0] * PAGE as f32 - 0.5,
                    (r.page / self.columns * TILE + PAD) as f32 + uv[1] * PAGE as f32 - 0.5,
                ]
            })
        };
        for r in triangles {
            let uv = texel_uv(r);
            let lo = [0, 1].map(|a| {
                uv.iter()
                    .map(|p| p[a])
                    .fold(f32::INFINITY, f32::min)
                    .floor() as i32
                    - 1
            });
            let hi = [0, 1].map(|a| {
                uv.iter()
                    .map(|p| p[a])
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil() as i32
                    + 2
            });
            let b = bounds.entry(r.key).or_insert([width, height, 0, 0]);
            b[0] = b[0].min(lo[0].max(0));
            b[1] = b[1].min(lo[1].max(0));
            b[2] = b[2].max(hi[0].min(width));
            b[3] = b[3].max(hi[1].min(height));
        }
        let mut move_charts = BTreeSet::new();
        let cross = |a: [f32; 2], b: [f32; 2]| a[0] * b[1] - a[1] * b[0];
        let sub = |a: [f32; 2], b: [f32; 2]| [a[0] - b[0], a[1] - b[1]];
        loop {
            owners.fill(None);
            let mut newly_moved = BTreeSet::new();
            for (index, r) in triangles
                .iter()
                .enumerate()
                .filter(|(_, r)| !move_charts.contains(&r.key))
            {
                let uv = texel_uv(r);
                let e1 = sub(uv[1], uv[0]);
                let e2 = sub(uv[2], uv[0]);
                let den = cross(e1, e2);
                if den.abs() < 1e-8 {
                    continue;
                }
                let lo = [0, 1].map(|a| {
                    uv.iter()
                        .map(|p| p[a])
                        .fold(f32::INFINITY, f32::min)
                        .floor()
                        .max(0.0) as i32
                });
                let hi = [0, 1].map(|a| {
                    uv.iter()
                        .map(|p| p[a])
                        .fold(f32::NEG_INFINITY, f32::max)
                        .ceil() as i32
                });
                let b = [
                    lo[0],
                    lo[1],
                    (hi[0] + 1).min(width),
                    (hi[1] + 1).min(height),
                ];
                for y in b[1]..b[3] {
                    for x in b[0]..b[2] {
                        let q = sub([x as f32, y as f32], uv[0]);
                        let u = cross(q, e2) / den;
                        let v = cross(e1, q) / den;
                        if u < -0.001 || v < -0.001 || u + v > 1.001 {
                            continue;
                        }
                        let position = r.positions[0] * (1.0 - u - v)
                            + r.positions[1] * u
                            + r.positions[2] * v;
                        let owner = &mut owners[y as usize * self.width as usize + x as usize];
                        if let Some((other, previous)) = *owner {
                            if triangles[other].key != r.key
                                && position.distance_squared(previous) > 1.0
                            {
                                let other_key = triangles[other].key;
                                let area = |key| {
                                    let b = bounds[&key];
                                    (b[2] - b[0]) * (b[3] - b[1])
                                };
                                newly_moved.insert(if area(r.key) < area(other_key) {
                                    r.key
                                } else {
                                    other_key
                                });
                            }
                        } else {
                            *owner = Some((index, position));
                        }
                    }
                }
            }
            if newly_moved.is_empty() {
                break;
            }
            move_charts.extend(newly_moved);
        }
        if move_charts.is_empty() {
            return 0;
        }
        // Use unallocated tiles in the last row first. Keeping the extent avoids
        // crossing a sun-cache size limit and lowering the entire map's bake
        // resolution for a few small charts. Commit atomically after sizing.
        let mut placements = Vec::new();
        let tail = self.page_count % self.columns;
        let mut row_start = if tail == 0 { 0 } else { (tail * TILE) as i32 };
        let (mut x, mut y, mut row_height) = (
            row_start,
            if tail == 0 {
                height
            } else {
                height - TILE as i32
            },
            0,
        );
        let mut ordered: Vec<_> = move_charts.into_iter().collect();
        ordered.sort_by_key(|key| {
            let b = bounds[key];
            (std::cmp::Reverse(b[3] - b[1]), *key)
        });
        for key in ordered {
            let b = bounds[&key];
            let w = b[2] - b[0];
            let h = b[3] - b[1];
            if w <= 0 || h <= 0 || w > width {
                return 0;
            }
            if w > width - row_start {
                y = height;
                x = 0;
                row_start = 0;
                row_height = 0;
            }
            if x + w > width {
                y += row_height;
                x = row_start;
                row_height = 0;
            }
            if y < height && y + h > height {
                y = height;
                x = 0;
                row_start = 0;
                row_height = 0;
            }
            placements.push((key, b, [x, y]));
            x += w;
            row_height = row_height.max(h);
        }
        let new_height = height.max(y + row_height);
        if new_height > 2048.max(height)
            || width as i64 * (new_height - height) as i64 > 2 * 1024 * 1024
        {
            return 0;
        }
        self.rgba
            .resize(self.width as usize * new_height as usize * 4, 0);
        for &(key, b, dst) in &placements {
            for row in 0..b[3] - b[1] {
                let src = ((b[1] + row) as usize * self.width as usize + b[0] as usize) * 4;
                let at = ((dst[1] + row) as usize * self.width as usize + dst[0] as usize) * 4;
                self.rgba
                    .copy_within(src..src + (b[2] - b[0]) as usize * 4, at);
            }
            self.receiver_offsets
                .insert(key, [dst[0] - b[0], dst[1] - b[1]]);
        }
        self.height = new_height as u32;
        placements.len()
    }
}

struct Receiver {
    key: (usize, usize),
    page: usize,
    positions: [Vec3; 3],
    uvs: [[f32; 2]; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receiver(surface: usize, height: f32, slot: usize) -> Receiver {
        Receiver {
            key: (surface, slot),
            page: 0,
            positions: [
                Vec3::new(0.0, height, 0.0),
                Vec3::new(8.0, height, 0.0),
                Vec3::new(0.0, height, 8.0),
            ],
            uvs: [[0.125, 0.125], [0.1875, 0.125], [0.125, 0.1875]],
        }
    }

    fn sample(atlas: &Atlas, uv: [f32; 2]) -> [f32; 4] {
        let at = [
            uv[0] * atlas.width as f32 - 0.5,
            uv[1] * atlas.height as f32 - 0.5,
        ];
        let p = at.map(f32::floor);
        let f = [at[0] - p[0], at[1] - p[1]];
        let mut color = [0.0; 4];
        for y in 0..2 {
            for x in 0..2 {
                let weight = (if x == 0 { 1.0 - f[0] } else { f[0] })
                    * (if y == 0 { 1.0 - f[1] } else { f[1] });
                let offset = ((p[1] as usize + y) * atlas.width as usize + p[0] as usize + x) * 4;
                for c in 0..4 {
                    color[c] += atlas.rgba[offset + c] as f32 * weight;
                }
            }
        }
        color
    }

    #[test]
    fn aliased_room_roof_and_third_receiver_have_independent_texels_with_identical_rgb() {
        // Three pages leave an unused tile in the last row of a 2x2 atlas.
        let rgb: Vec<u8> = (0..PAGE * PAGE * 3 * 3)
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let original = Atlas::new(&rgb);
        let mut atlas = Atlas::new(&rgb);
        let triangles = [
            receiver(1, 0.0, 0),
            receiver(2, 384.0, 0),
            receiver(3, 704.0, 0),
        ];
        assert_eq!(atlas.separate_triangles(&triangles), 2);
        assert_eq!(
            [atlas.width, atlas.height],
            [original.width, original.height],
            "reuse unused atlas storage without lowering the derived bake resolution"
        );
        // A shared lightmap byte must never choose which world position receives
        // the sun/sky ray. Distinct chart locations make that decision independent.
        let uv = [0.145, 0.15];
        let mapped: Vec<_> = triangles
            .iter()
            .map(|r| atlas.receiver_uv(0, uv, r.key.0, r.key.1))
            .collect();
        for i in 0..3 {
            for j in i + 1..3 {
                assert_ne!(mapped[i], mapped[j]);
            }
        }
        for r in triangles {
            // Include endpoints, sub-luxel sampling and all bilinear neighbours.
            for uv in [[0.125, 0.125], [0.1875, 0.125], [0.125, 0.1875], uv] {
                let before = sample(&original, original.uv(0, uv));
                let after = sample(&atlas, atlas.receiver_uv(0, uv, r.key.0, r.key.1));
                for c in 0..4 {
                    assert!(
                        (before[c] - after[c]).abs() < 0.01,
                        "{before:?} vs {after:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn connected_triangles_keep_their_chart_and_lightstyles_stay_independent() {
        let rgb = vec![127; PAGE * PAGE * 3];
        let mut atlas = Atlas::new(&rgb);
        let room = receiver(1, 0.0, 0);
        let shared = receiver(2, 0.0, 0);
        assert_eq!(
            atlas.separate_triangles(&[room, shared]),
            0,
            "same world position can share"
        );
        let size = [atlas.width, atlas.height];
        assert_eq!(size, [TILE as u32, TILE as u32]);
        let lower = receiver(1, 0.0, 0);
        let upper_style = receiver(1, 384.0, 1);
        assert_eq!(atlas.separate_triangles(&[lower, upper_style]), 1);
        assert_ne!(
            atlas.receiver_uv(0, [0.15; 2], 1, 0),
            atlas.receiver_uv(0, [0.15; 2], 1, 1)
        );
    }
}
