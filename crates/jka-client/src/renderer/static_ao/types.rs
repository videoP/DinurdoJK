//! Static ao types.
use crate::renderer::{scene, Arc, GpuVertex, Receiver, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::renderer) enum StaticAoBakeMode {
    Lightmap,
}

impl StaticAoBakeMode {
    pub(in crate::renderer) fn label(self) -> &'static str {
        "baked"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::renderer) struct StaticAoJobKey {
    pub(in crate::renderer) map_hash: u64,
    pub(in crate::renderer) mode: StaticAoBakeMode,
    pub(in crate::renderer) samples: u32,
    pub(in crate::renderer) scale: u32,
    pub(in crate::renderer) strength: u32,
    pub(in crate::renderer) range: u32,
    pub(in crate::renderer) current_cell_only: bool,
}

#[derive(Clone)]
pub(in crate::renderer) struct StaticAoLightmapTriangle {
    pub(in crate::renderer) texture: usize,
    pub(in crate::renderer) owner: u32,
    pub(in crate::renderer) positions: [[f32; 3]; 3],
    pub(in crate::renderer) normals: [[f32; 3]; 3],
    pub(in crate::renderer) uvs: [[f32; 2]; 3],
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoRenderTriangle {
    pub(in crate::renderer) positions: [[f32; 3]; 3],
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoTraceTriangle {
    pub(in crate::renderer) origin: Vec3,
    pub(in crate::renderer) edge1: Vec3,
    pub(in crate::renderer) edge2: Vec3,
    pub(in crate::renderer) minimum: Vec3,
    pub(in crate::renderer) maximum: Vec3,
    pub(in crate::renderer) centroid: Vec3,
}

#[derive(Clone)]
pub(in crate::renderer) struct StaticAoBaseLightmap {
    pub(in crate::renderer) label: String,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) rgba: Vec<u8>,
    pub(in crate::renderer) clamp: bool,
    pub(in crate::renderer) srgb: bool,
}

pub(in crate::renderer) struct StaticAoBakedLightmap {
    pub(in crate::renderer) label: String,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) rgba: Vec<u8>,
    pub(in crate::renderer) clamp: bool,
    pub(in crate::renderer) srgb: bool,
}

#[derive(Clone)]
pub(in crate::renderer) struct StaticAoWorldSource {
    pub(in crate::renderer) cache: scene::StaticBspAoCacheInfo,
    pub(in crate::renderer) vertices: Arc<Vec<GpuVertex>>,
    /// Source-vertex mask for true q3map LIGHTMAP_BY_VERTEX baked-light receivers.
    pub(in crate::renderer) vertex_ao_receivers: Arc<Vec<u8>>,
    pub(in crate::renderer) lightmap_triangles: Arc<Vec<StaticAoLightmapTriangle>>,
    pub(in crate::renderer) render_triangles: Arc<Vec<StaticAoRenderTriangle>>,
    pub(in crate::renderer) lightmap_sizes: Arc<Vec<[u32; 2]>>,
    pub(in crate::renderer) lightmap_bases: Arc<Vec<StaticAoBaseLightmap>>,
}

pub(in crate::renderer) enum StaticAoBakeData {
    Lightmap {
        /// HQ per-vertex visibility for LIGHTMAP_BY_VERTEX/static-model surfaces.
        /// Non-receiver vertices remain neutral (255), preventing double AO on lightmaps.
        vertex_values: Vec<u8>,
        images: Vec<StaticAoBakedLightmap>,
    },
}

pub(in crate::renderer) struct StaticAoWorkerResult {
    pub(in crate::renderer) key: StaticAoJobKey,
    pub(in crate::renderer) data: StaticAoBakeData,
    pub(in crate::renderer) cache_hit: bool,
    pub(in crate::renderer) elapsed_ms: f64,
}

pub(in crate::renderer) enum StaticAoWorkerUpdate {
    InitLightmaps {
        key: StaticAoJobKey,
        images: Vec<StaticAoBakedLightmap>,
    },
    VertexAo {
        key: StaticAoJobKey,
        values: Vec<u8>,
    },
    LightmapTile {
        key: StaticAoJobKey,
        texture: usize,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

pub(in crate::renderer) struct StaticAoPendingJob {
    pub(in crate::renderer) key: StaticAoJobKey,
    pub(in crate::renderer) rx: Receiver<Result<StaticAoWorkerResult, String>>,
    pub(in crate::renderer) progress_rx: Receiver<(u32, u32)>,
    pub(in crate::renderer) update_rx: Receiver<StaticAoWorkerUpdate>,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoTexelSample {
    pub(in crate::renderer) position: [f32; 3],
    pub(in crate::renderer) normal: [f32; 3],
}
