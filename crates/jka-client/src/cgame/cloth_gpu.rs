//! CGame side of GPU cloth (r_clothSolver 1): the static garment the
//! renderer's gpu-cloth-sim port simulates, and the reference's colliders.
//!
//! Topology (welds, attachment pins, attachment falloff) is this system's
//! stitched garment (`cloth::build_gpu_garment_topology`). Constraint building,
//! graph colouring and the normal adjacency are ports of `gpu_cloth_solver.gd`.
use super::cloth::{ClothCapsule, ClothConfig, ClothSurfaceFrame};
use crate::renderer::{ClothGpuCollider, ClothGpuGarment, ClothGpuTuning, Ghoul2GpuVertex};
use glam::Vec3;
use std::collections::HashMap;
use std::sync::Arc;

const METRES_PER_JKA_UNIT: f32 = 0.0254;

/// The GPU solver's settings come from the same r_cloth* / r_physics*
/// settings as the CPU solver; only exports with no shared equivalent keep the
/// `GPUClothSolver` defaults (substeps, gravity, inertia scale, gusts).
pub(crate) fn tuning(config: &ClothConfig) -> ClothGpuTuning {
    ClothGpuTuning {
        hz: config.hz,
        max_ticks: config.max_substeps,
        solver_iterations: super::cloth::solver_iterations(config.stiffness) as u32,
        stiffness: super::cloth::per_iteration_correction(config.stiffness),
        damping: config.damping.clamp(0.0, 30.0),
        max_speed: super::cloth::MAX_LOCAL_PARTICLE_SPEED,
        animation_influence: config.animation_influence.clamp(0.0, 1.0),
        ..REFERENCE_TUNING
    }
}

const REFERENCE_TUNING: ClothGpuTuning = ClothGpuTuning {
    hz: 60,
    max_ticks: 4,
    solver_iterations: 8,
    substeps: 8,
    stiffness: 0.5,
    damping: 0.0,
    max_speed: 5.0,
    animation_influence: 1.0,
    gravity_strength: -9.8,
    inertia_scale: [1.0, 1.0, 1.0],
    wind_turbulence: 0.3,
    wind_frequency: 1.0,
};

/// `_build_constraints`: unique triangle edges in first-seen order, greedily
/// edge-coloured so no two constraints in a group share a particle. Returns
/// (a, b, rest) in group order and each group's (offset, count).
pub(crate) fn build_constraints(
    rest: &[Vec3],
    triangles: &[[u32; 3]],
) -> (Vec<(u32, u32, f32)>, Vec<(u32, u32)>) {
    // Godot Dictionary keys keep insertion order; re-assigning a key keeps it.
    let mut seen = HashMap::<(u32, u32), ()>::new();
    let mut edges = Vec::<(u32, u32)>::new();
    for &[ia, ib, ic] in triangles {
        for (a, b) in [(ia, ib), (ib, ic), (ia, ic)] {
            let key = (a.min(b), a.max(b));
            if seen.insert(key, ()).is_none() {
                edges.push(key);
            }
        }
    }

    let mut vtx_colors = vec![Vec::<u32>::new(); rest.len()];
    let mut color_groups = Vec::<Vec<(u32, u32)>>::new();
    for edge in edges {
        let (a, b) = (edge.0 as usize, edge.1 as usize);
        let free = (0..color_groups.len() as u32)
            .find(|c| !vtx_colors[a].contains(c) && !vtx_colors[b].contains(c));
        let color = match free {
            Some(color) => {
                color_groups[color as usize].push(edge);
                color
            }
            None => {
                color_groups.push(vec![edge]);
                color_groups.len() as u32 - 1
            }
        };
        vtx_colors[a].push(color);
        vtx_colors[b].push(color);
    }

    let mut data = Vec::new();
    let mut groups = Vec::with_capacity(color_groups.len());
    for group in color_groups {
        let start = data.len() as u32;
        for (a, b) in group {
            data.push((a, b, rest[a as usize].distance(rest[b as usize])));
        }
        groups.push((start, data.len() as u32 - start));
    }
    (data, groups)
}

/// `_build_adjacency`: vertex -> touching triangles (counts, offsets, list).
pub(crate) fn build_adjacency(
    particle_count: usize,
    triangles: &[[u32; 3]],
) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut vert_tris = vec![Vec::<u32>::new(); particle_count];
    for (t, triangle) in triangles.iter().enumerate() {
        for &vertex in triangle {
            vert_tris[vertex as usize].push(t as u32);
        }
    }
    let mut counts = Vec::with_capacity(particle_count);
    let mut offsets = Vec::with_capacity(particle_count);
    let mut list = Vec::new();
    for tris in vert_tris {
        counts.push(tris.len() as u32);
        offsets.push(list.len() as u32);
        list.extend(tris);
    }
    (counts, offsets, list)
}

/// Builds the static garment (welds + attachments + reference constraints).
/// Runs off the render thread once per model/LOD/surface set.
pub(crate) fn build_garment(
    key: Arc<str>,
    model_label: &str,
    lod_index: usize,
    frames: &[ClothSurfaceFrame],
    render_vertices: Vec<Vec<Ghoul2GpuVertex>>,
    capsules: &[ClothCapsule],
    config: ClothConfig,
) -> Result<ClothGpuGarment, String> {
    if render_vertices.len() != frames.len()
        || render_vertices
            .iter()
            .zip(frames)
            .any(|(vertices, frame)| vertices.len() != frame.bind_positions.len())
    {
        return Err("GPU cloth vertices disagree with the garment surfaces".to_owned());
    }
    let topology =
        super::cloth::build_gpu_garment_topology(model_label, lod_index, frames, capsules, config)?;
    let (constraints, constraint_groups) =
        build_constraints(&topology.bind_particles, &topology.triangles);
    let particle_count = topology.cloth_weights.len();
    let (vert_tri_counts, vert_tri_offsets, vert_tri_list) =
        build_adjacency(particle_count, &topology.triangles);
    let mut member_offsets = Vec::with_capacity(particle_count + 1);
    let mut member_list = Vec::new();
    for members in &topology.particle_members {
        member_offsets.push(member_list.len() as u32);
        member_list.extend(members.iter().map(|&m| m as u32));
    }
    member_offsets.push(member_list.len() as u32);
    let mut surface_ranges = Vec::with_capacity(render_vertices.len());
    let mut flat = Vec::new();
    for vertices in render_vertices {
        surface_ranges.push((flat.len() as u32, vertices.len() as u32));
        flat.extend(vertices);
    }
    if topology.render_to_particle.len() != flat.len() {
        return Err("GPU cloth weld map disagrees with the render vertices".to_owned());
    }
    devprintln!(
        1,
        "CLOTH GPU: {model_label} lod{lod_index} {} particles / {} constraints in {} colour groups / {} triangles",
        particle_count,
        constraints.len(),
        constraint_groups.len(),
        topology.triangles.len(),
    );
    Ok(ClothGpuGarment {
        key,
        render_vertices: flat,
        surface_ranges,
        render_to_particle: topology.render_to_particle,
        member_offsets,
        member_list,
        cloth_weights: topology.cloth_weights,
        bind_particles: topology.bind_particles.iter().map(|p| p.to_array()).collect(),
        driver_particles: topology.driver_particles,
        attachment_drivers: topology
            .attachment_drivers
            .iter()
            .map(|drivers| drivers.iter().map(|&(d, w)| (d as u32, w)).collect())
            .collect(),
        max_travel: topology.runaway_limit,
        constraints,
        constraint_groups,
        triangles: topology.triangles,
        vert_tri_counts,
        vert_tri_offsets,
        vert_tri_list,
    })
}

/// `GPUClothCollider.pack_collider_data` for this system's body proxies, in
/// model-local metres. Round capsules are the reference capsule (a sphere
/// when a == b). Fitted ellipsoid proxies have no reference shape; they use
/// the reference's oriented box with the ellipsoid's axes. The configured
/// body clearance pads both.
pub(crate) fn pack_colliders(capsules: &[ClothCapsule], clearance_jka: f32) -> Vec<ClothGpuCollider> {
    let m = |v: [f32; 3]| Vec3::from_array(v) * METRES_PER_JKA_UNIT;
    capsules
        .iter()
        .filter_map(|capsule| {
            let mut data = [[0.0f32; 4]; 4];
            match capsule.basis {
                None => {
                    let a = m(capsule.a);
                    let b = m(capsule.b);
                    let radius = (capsule.radius + clearance_jka) * METRES_PER_JKA_UNIT;
                    data[0] = [a.x, a.y, a.z, radius];
                    data[1] = [b.x, b.y, b.z, 0.0];
                }
                Some(axes) => {
                    let center = m(capsule.a);
                    let column = |i: usize| Vec3::from_array(axes[i]);
                    let half = |i: usize| (column(i).length() + clearance_jka) * METRES_PER_JKA_UNIT;
                    let right = column(0).try_normalize()?;
                    let up = column(1).try_normalize()?;
                    data[0] = [center.x, center.y, center.z, 0.0];
                    data[1] = [half(0), half(1), half(2), 1.0];
                    data[2] = [right.x, right.y, right.z, 0.0];
                    data[3] = [up.x, up.y, up.z, 0.0];
                }
            }
            data.iter().flatten().all(|v| v.is_finite()).then_some(data)
        })
        .collect()
}
