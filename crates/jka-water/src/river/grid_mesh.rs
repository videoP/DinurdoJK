//! Native Grid2D_SW_River reset stages 1/2 and ScratchModule_04 allocation.
//! Source evidence: grid-mesh-reference.json. Original algorithms:
//! Copyright Epic Games, Inc. All Rights Reserved.
//!
//! Positions remain flat and local, in centimetres. The native water material
//! supplies displacement; baking height into this mesh would apply it twice.

pub const SHADER: &str = include_str!("grid_mesh.wgsl");
pub const VERTEX_STRIDE: u64 = 32;

/// Interleaved equivalent of the native dynamic mesh's separate attribute
/// streams. Float32 positions/UVs, SNORM8 tangent X/Z, UNORM8 colour, u32 indices.
/// This uses the GPU DI's tangent packing, not its different CPU FPackedNormal
/// setter, nor the WaterBody static-mesh sRGB colour conversion chain.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub tangent_x: u32,
    pub tangent_z: u32,
    pub uv: [f32; 2],
    pub color: u32,
}

impl Vertex {
    pub fn words(self) -> [u32; 8] {
        [
            self.position[0].to_bits(),
            self.position[1].to_bits(),
            self.position[2].to_bits(),
            self.tangent_x,
            self.tangent_z,
            self.uv[0].to_bits(),
            self.uv[1].to_bits(),
            self.color,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GridMesh {
    cells: [u32; 2],
    size: [f32; 2],
    vertex_count: u32,
    index_count: u32,
}

impl GridMesh {
    /// Selected native MeshResMult is 1. The input is the resolved, padded
    /// simulation grid, not the original map bounds or a mapper-authored spline.
    pub fn new(cells: [u32; 2], size: [f32; 2]) -> Result<Self, &'static str> {
        if cells.iter().any(|v| *v == 0 || *v >= (1 << 24))
            || size.iter().any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("invalid native river mesh dimensions or extents");
        }
        let vertices = (u64::from(cells[0]) + 1) * (u64::from(cells[1]) + 1);
        let indices = u64::from(cells[0]) * u64::from(cells[1]) * 6;
        if vertices > i32::MAX as u64 || indices > i32::MAX as u64 {
            return Err("native river mesh signed counts overflow");
        }
        Ok(Self {
            cells,
            size,
            vertex_count: vertices as u32,
            index_count: indices as u32,
        })
    }

    pub fn cells(self) -> [u32; 2] {
        self.cells
    }
    pub fn size(self) -> [f32; 2] {
        self.size
    }
    pub fn vertex_dimensions(self) -> [u32; 2] {
        self.cells.map(|v| v + 1)
    }
    pub fn vertex_count(self) -> u32 {
        self.vertex_count
    }
    pub fn index_count(self) -> u32 {
        self.index_count
    }
    pub fn vertex_bytes(self) -> u64 {
        u64::from(self.vertex_count) * VERTEX_STRIDE
    }
    pub fn index_bytes(self) -> u64 {
        u64::from(self.index_count) * 4
    }

    pub fn vertex(self, index: u32) -> Option<Vertex> {
        if index >= self.vertex_count {
            return None;
        }
        let width = self.vertex_dimensions()[0];
        let uv = [
            (index % width) as f32 / self.cells[0] as f32,
            (index / width) as f32 / self.cells[1] as f32,
        ];
        Some(Vertex {
            position: [
                (uv[0] - 0.5) * self.size[0],
                (uv[1] - 0.5) * self.size[1],
                0.0,
            ],
            // cross(X,Z).dot(Y) < 0 gives 0x80. The native GPU pack casts
            // tangent*127.499 to int, then masks bytes; the SRV is SNORM8.
            tangent_x: 0x8000_007f,
            tangent_z: 0x807f_0000,
            uv,
            color: 0xffff_ffff,
        })
    }

    pub fn quad(self, index: u32) -> Option<[u32; 6]> {
        if index >= self.index_count / 6 {
            return None;
        }
        let x = index % self.cells[0];
        let y = index / self.cells[0];
        let width = self.vertex_dimensions()[0];
        let a = (x + 1) + (y + 1) * width;
        let b = (x + 1) + y * width;
        let c = x + y * width;
        let d = x + (y + 1) * width;
        Some([a, b, c, c, d, a])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_grid_mesh_has_native_corners_topology_and_packing() {
        for cells in [[1, 1], [7, 3], [3, 7], [512, 241]] {
            let mesh = GridMesh::new(cells, [2300.0, 1100.0]).unwrap();
            let first = mesh.vertex(0).unwrap();
            let last = mesh.vertex(mesh.vertex_count() - 1).unwrap();
            assert_eq!(first.position, [-1150.0, -550.0, 0.0]);
            assert_eq!(last.position, [1150.0, 550.0, 0.0]);
            assert_eq!((first.uv, last.uv), ([0.0, 0.0], [1.0, 1.0]));
            assert_eq!(first.words()[3..5], [0x8000_007f, 0x807f_0000]);
            assert_eq!(first.color, u32::MAX);
            assert_eq!(std::mem::size_of::<Vertex>(), VERTEX_STRIDE as usize);
            let mut uses = vec![0u32; mesh.vertex_count() as usize];
            for i in 0..mesh.index_count() / 6 {
                let indices = mesh.quad(i).unwrap();
                for tri in indices.chunks_exact(3) {
                    let [a, b, c] = std::array::from_fn(|j| mesh.vertex(tri[j]).unwrap().position);
                    assert!((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) < 0.0);
                }
                for j in indices {
                    uses[j as usize] += 1;
                }
            }
            assert!(uses.iter().all(|n| *n > 0));
            assert!(mesh.vertex(mesh.vertex_count()).is_none());
            assert!(mesh.quad(mesh.index_count() / 6).is_none());
        }
        assert_eq!(
            GridMesh::new([1, 1], [2.0, 2.0]).unwrap().quad(0),
            Some([3, 1, 0, 0, 2, 3])
        );
    }

    #[test]
    fn river_grid_mesh_rejects_bad_dimensions_before_allocation() {
        for cells in [[0, 1], [1, 0], [u32::MAX, 1], [65536, 65536]] {
            assert!(GridMesh::new(cells, [1.0, 1.0]).is_err());
        }
        for size in [[f32::NAN, 1.0], [f32::INFINITY, 1.0], [1.0, 0.0]] {
            assert!(GridMesh::new([1, 1], size).is_err());
        }
    }
}
