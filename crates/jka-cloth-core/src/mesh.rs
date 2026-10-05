use crate::{ClothError, Real, Vec3, constraints::bend::angle_and_gradients};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Edge {
    pub vertices: [u32; 2],
    pub rest_length: Real,
}

#[derive(Debug, Clone)]
pub struct Hinge {
    pub vertices: [u32; 4],
    pub rest_angle: Real,
}

/// Validated, immutable simulation topology. No implicit vertex welding.
#[derive(Debug, Clone)]
pub struct ClothMesh {
    positions: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    edges: Vec<Edge>,
    hinges: Vec<Hinge>,
    vertex_areas: Vec<Real>,
    area: Real,
}

impl ClothMesh {
    pub fn new(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Result<Self, ClothError> {
        let invalid = |s: &str| ClothError::InvalidMesh(s.to_owned());
        if positions.len() < 3 || positions.len() > u32::MAX as usize || triangles.is_empty() {
            return Err(invalid(
                "at least three vertices and one triangle are required",
            ));
        }
        if positions.iter().any(|p| !p.is_finite()) {
            return Err(invalid("non-finite vertex"));
        }
        let mut face_keys = BTreeSet::new();
        let mut adjacency: BTreeMap<[u32; 2], Vec<[u32; 3]>> = BTreeMap::new();
        let mut vertex_areas = vec![0.0; positions.len()];
        let mut area = 0.0;
        for &tri in &triangles {
            if tri.iter().any(|&i| i as usize >= positions.len()) {
                return Err(invalid("triangle index out of bounds"));
            }
            let mut key = tri;
            key.sort_unstable();
            if key[0] == key[1] || key[1] == key[2] {
                return Err(invalid("repeated triangle vertex"));
            }
            if !face_keys.insert(key) {
                return Err(invalid("duplicate triangle"));
            }
            let a = positions[tri[0] as usize];
            let b = positions[tri[1] as usize];
            let c = positions[tri[2] as usize];
            let face_area = (b - a).cross(c - a).length() * 0.5;
            if !face_area.is_finite() || face_area <= Real::MIN_POSITIVE {
                return Err(invalid("zero or non-finite triangle area"));
            }
            area += face_area;
            for i in tri {
                vertex_areas[i as usize] += face_area / 3.0;
            }
            for [i, j, k] in [
                [tri[0], tri[1], tri[2]],
                [tri[1], tri[2], tri[0]],
                [tri[2], tri[0], tri[1]],
            ] {
                let pair = [i.min(j), i.max(j)];
                let faces = adjacency.entry(pair).or_default();
                if faces.len() == 2 {
                    return Err(invalid("non-manifold edge"));
                }
                if faces.first().is_some_and(|f| f[0] == i) {
                    return Err(invalid("inconsistent triangle winding"));
                }
                faces.push([i, j, k]);
            }
        }
        if !area.is_finite()
            || vertex_areas
                .iter()
                .any(|a| !a.is_finite() || *a <= Real::MIN_POSITIVE)
        {
            return Err(invalid("isolated vertex or invalid accumulated area"));
        }
        let mut edges = Vec::with_capacity(adjacency.len());
        let mut hinges = Vec::new();
        for (vertices, faces) in adjacency {
            let rest_length =
                positions[vertices[0] as usize].distance(positions[vertices[1] as usize]);
            if !rest_length.is_finite() || rest_length <= Real::MIN_POSITIVE {
                return Err(invalid("invalid edge length"));
            }
            edges.push(Edge {
                vertices,
                rest_length,
            });
            if faces.len() == 2 {
                let ids = [faces[0][0], faces[0][1], faces[0][2], faces[1][2]];
                let (rest_angle, _) = angle_and_gradients(ids.map(|i| positions[i as usize]))
                    .ok_or_else(|| invalid("degenerate rest hinge"))?;
                hinges.push(Hinge {
                    vertices: ids,
                    rest_angle,
                });
            }
        }
        Ok(Self {
            positions,
            triangles,
            edges,
            hinges,
            vertex_areas,
            area,
        })
    }

    pub fn rest_positions(&self) -> &[Vec3] {
        &self.positions
    }
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }
    pub fn hinges(&self) -> &[Hinge] {
        &self.hinges
    }
    pub fn vertex_areas(&self) -> &[Real] {
        &self.vertex_areas
    }
    pub fn area(&self) -> Real {
        self.area
    }
}

/// Rectangular grid in an explicitly chosen plane; defaults to X/Z.
#[derive(Debug, Clone)]
pub struct GridBuilder {
    nx: usize,
    ny: usize,
    width: Real,
    height: Real,
    origin: Vec3,
    u: Vec3,
    v: Vec3,
}

impl GridBuilder {
    pub fn new(nx: usize, ny: usize) -> Self {
        Self {
            nx,
            ny,
            width: 1.0,
            height: 1.0,
            origin: Vec3::ZERO,
            u: Vec3::X,
            v: Vec3::Z,
        }
    }
    pub fn size(mut self, width: Real, height: Real) -> Self {
        self.width = width;
        self.height = height;
        self
    }
    pub fn origin(mut self, origin: Vec3) -> Self {
        self.origin = origin;
        self
    }
    /// Axes must be finite orthonormal unit vectors.
    pub fn axes(mut self, u: Vec3, v: Vec3) -> Self {
        self.u = u;
        self.v = v;
        self
    }
    pub fn build(self) -> Result<ClothMesh, ClothError> {
        if self.nx < 2
            || self.ny < 2
            || self
                .nx
                .checked_mul(self.ny)
                .is_none_or(|n| n > u32::MAX as usize)
        {
            return Err(ClothError::InvalidParameter("grid dimensions"));
        }
        if !self.width.is_finite()
            || self.width <= 0.0
            || !self.height.is_finite()
            || self.height <= 0.0
        {
            return Err(ClothError::InvalidParameter("grid size"));
        }
        if !self.u.is_finite()
            || !self.v.is_finite()
            || (self.u.length_squared() - 1.0).abs() > 1.0e-5
            || (self.v.length_squared() - 1.0).abs() > 1.0e-5
            || self.u.dot(self.v).abs() > 1.0e-5
        {
            return Err(ClothError::InvalidParameter(
                "grid axes must be orthonormal",
            ));
        }
        let mut p = Vec::with_capacity(self.nx * self.ny);
        for y in 0..self.ny {
            for x in 0..self.nx {
                p.push(
                    self.origin
                        + self.u * (self.width * x as Real / (self.nx - 1) as Real)
                        + self.v * (self.height * y as Real / (self.ny - 1) as Real),
                );
            }
        }
        let mut t = Vec::with_capacity((self.nx - 1) * (self.ny - 1) * 2);
        for y in 0..self.ny - 1 {
            for x in 0..self.nx - 1 {
                let a = (y * self.nx + x) as u32;
                let b = a + 1;
                let c = a + self.nx as u32;
                let d = c + 1;
                t.push([a, c, b]);
                t.push([b, c, d]);
            }
        }
        ClothMesh::new(p, t)
    }
}
