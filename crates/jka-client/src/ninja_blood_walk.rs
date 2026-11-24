//! Surface-walking port from Overgrowth ModelSurfaceWalker::Move and
//! BloodSurface::Update, adapted to static BSP render triangles and Z-up.
//!
//! Copyright 2022 Wolfire Games LLC
//! Licensed under the Apache License, Version 2.0 (the "License");
//! you may not use this file except in compliance with the License.
//! You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
//! Unless required by applicable law or agreed to in writing, software
//! distributed under the License is distributed on an "AS IS" BASIS,
//! WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//! See the License for the specific language governing permissions and limitations.
use glam::Vec3;
use jka_assets::bsp::MarkSurfaces;
use super::{SplatKind, METER, STEP};

pub(crate) struct Droplet {
    pub pos: Vec3,
    pub velocity: Vec3,
    pub remaining: f32,
    pub size_cm: f32,
    pub may_walk: bool,
    pub spurt: Option<std::num::NonZeroU32>,
    pub(super) splat: SplatKind,
}

impl Droplet {
    pub(super) fn new(pos: Vec3, velocity: Vec3, size_cm: f32, may_walk: bool) -> Self {
        Self { pos, velocity, size_cm, may_walk, remaining: 3.0, spurt: None, splat: SplatKind::Ordinary }
    }
}


pub(super) enum Status { Continue, Done, Drip(Vec3, Vec3) }
pub(super) struct Walker {
    tri: usize,
    weights: Vec3,
    // Index of the vertex opposite the edge we entered along.
    on_edge: Option<usize>,
    amount: f32,
}

fn bary(p: Vec3, t: [Vec3; 3]) -> Option<Vec3> {
    let a = t[1]-t[0]; let b = t[2]-t[0]; let d = p-t[0];
    let (aa, ab, bb) = (a.dot(a), a.dot(b), b.dot(b));
    let den = aa * bb - ab * ab;
    if den <= 1.0e-8 || !den.is_finite() { return None; }
    let u = (d.dot(a)*bb-d.dot(b)*ab)/den;
    let v = (d.dot(b)*aa-d.dot(a)*ab)/den;
    Some(Vec3::new(1.0-u-v,u,v))
}
fn point(t: [Vec3;3], w: Vec3) -> Vec3 { t[0]*w.x+t[1]*w.y+t[2]*w.z }
fn tri(s: &MarkSurfaces, i: usize) -> Option<([Vec3;3], Vec3)> {
    let (p, n) = s.blood_triangle(i)?;
    Some((p.map(Vec3::from_array), Vec3::from_array(n).try_normalize()?))
}

impl Walker {
    pub(super) fn at(s: &MarkSurfaces, hit: Vec3, normal: Vec3) -> Option<Self> {
        let mut best: Option<(f32, usize, Vec3)> = None;
        for id in s.blood_triangles_near(hit.to_array()) {
            let Some((p,n)) = tri(s,id) else { continue; };
            if n.dot(normal) < 0.9 { continue; }
            let gap = (hit-p[0]).dot(n).abs();
            if gap > 2.0 { continue; }
            let Some(w) = bary(hit,p) else { continue; };
            if w.min_element() < -0.015 { continue; }
            let w = w.max(Vec3::ZERO); let w = w / w.element_sum();
            if best.as_ref().is_none_or(|(d,_,_)| gap < *d) { best = Some((gap,id,w)); }
        }
        let (_,tri,weights)=best?;
        // aschar.as CreateBloodDrip(...,1,...); amount is spent at fixed
        // timestep per drip update, rather than wall-clock per rendered frame.
        Some(Self {tri, weights, on_edge: None, amount: 1.0})
    }

    /// Upstream matches welded vertex pairs. Query the existing BSP spatial
    /// index at the edge instead of building a second full map index. Accept
    /// small coordinate roundoff, not disconnected/crossing surfaces.
    fn neighbor(s: &MarkSurfaces, current: usize, a: Vec3, b: Vec3, at: Vec3) -> Option<(usize,usize,Vec3)> {
        const EPS: f32 = 0.05;
        let mut found = None;
        for id in s.blood_triangles_near(at.to_array()) {
            if id == current { continue; }
            let Some((p,_)) = tri(s,id) else { continue; };
            for opposite in 0..3 {
                let c=p[(opposite+1)%3]; let d=p[(opposite+2)%3];
                let same = (a.distance(c)<=EPS && b.distance(d)<=EPS) || (a.distance(d)<=EPS && b.distance(c)<=EPS);
                if !same { continue; }
                // Overgrowth connects exactly two faces sharing an edge.
                if found.is_some() { return None; }
                let mut w=bary(at,p)?.max(Vec3::ZERO);
                w[opposite]=0.0; w /= w.element_sum();
                if w.is_finite() { found=Some((id,opposite,w)); }
            }
        }
        found
    }

    pub(super) fn advance(&mut self, s: &MarkSurfaces, lines: &mut Vec<(Vec3,Vec3,Vec3)>) -> Status {
        if self.amount < 0.0 { return Status::Done; }
        self.amount -= STEP;
        // Upstream .005 metre step, 10 triangle budget, gravity in surface plane.
        let mut remaining = 0.005 * METER;
        for _ in 0..10 {
            if remaining <= 1.0e-5 { return Status::Continue; }
            let Some((p,n))=tri(s,self.tri) else { return Status::Done; };
            let old=point(p,self.weights);
            if n.z >= 0.999 { return Status::Done; }
            let Some(mut dir)=(-Vec3::Z+n*n.z).try_normalize() else {
                // Host adaptation for a horizontal ceiling: gravity pulls
                // directly away, so the drop cannot surface-walk.
                return if n.z < 0.0 { Status::Drip(old,n) } else { Status::Done };
            };
            let mut along_edge=false;
            if let Some(edge)=self.on_edge {
                let Some(test)=bary(old+dir,p) else { return Status::Done; };
                if test[edge] <= self.weights[edge] + 1.0e-6 {
                    let Some(axis)=(p[(edge+2)%3]-p[(edge+1)%3]).try_normalize() else { return Status::Done; };
                    dir=axis*axis.dot(dir);
                    along_edge=true;
                    if dir.length_squared()<1.0e-10 { return Status::Drip(old,n); }
                }
            }
            let Some(target)=bary(old+dir*remaining,p) else { return Status::Done; };
            // Equivalent to the upstream 2D line/edge intersection: the
            // barycentric opposite weight reaches zero at the crossed edge.
            let mut edge=None; let mut t=1.0f32;
            for i in 0..3 {
                let delta=target[i]-self.weights[i];
                if delta >= -1.0e-8 { continue; }
                let crossing=-self.weights[i]/delta;
                if crossing >= -1.0e-5 && crossing <= t && target[i] < -1.0e-6 {
                    t=crossing.max(0.0); edge=Some(i);
                }
            }
            let mut w=self.weights.lerp(target,t).max(Vec3::ZERO);
            if let Some(e)=edge { w[e]=0.0; }
            w /= w.element_sum();
            if !w.is_finite() { return Status::Done; }
            let at=point(p,w);
            if old.distance_squared(at)>1.0e-10 { lines.push((old,at,n)); }
            self.weights=w;
            let Some(e)=edge else {
                if !along_edge { self.on_edge=None; }
                return Status::Continue;
            };
            if along_edge { return Status::Drip(at,n); }
            let a=p[(e+1)%3]; let b=p[(e+2)%3];
            let Some((id,reverse,next_w))=Self::neighbor(s,self.tri,a,b,at) else {
                // BSP boundary / unmatched T-junction: release, never paint
                // across empty space. OG's mesh walker stops on open edges.
                return Status::Drip(at,n);
            };
            self.tri=id; self.on_edge=Some(reverse); self.weights=next_w;
            remaining *= 1.0-t;
        }
        Status::Continue
    }
}
