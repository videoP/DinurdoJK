// Shared parameters for the Ninja fog port of UE 5.6 volumetric fog
// (VolumetricFog.usf / VolumetricFogVoxelization.usf / HeightFogCommon.ush).
// World units are JKA map units; UE centimetre constants are converted by the host.

struct NinjaVolumeParams {
    unjittered_inv_view_proj: mat4x4<f32>, // UnjitteredClipToTranslatedWorld (world here)
    prev_view_proj: mat4x4<f32>,           // UnjitteredPrevTranslatedWorldToClip
    view_proj: mat4x4<f32>,
    jitter: array<vec4<f32>, 4>,           // FrameJitterOffsets[0..HistoryMissSuperSampleCount)
    grid: vec4<u32>,                       // ViewGridSize xyz, HistoryMissSuperSampleCount
    grid_z: vec4<f32>,                     // GridZParams B, O, S; HistoryWeight (0 without history)
    view: vec4<f32>,                       // camera position, VolumetricFog.MaxDistance
    misc: vec4<f32>,                       // viewport size, VolumetricFogNearFadeInDistanceInv, PhaseG
    frame: vec4<u32>,                       // frame index, HVOL downsample
};

// r.VolumetricFog.GridPixelSize at runtime in the demo (Fog_Mist sets 5).
const NINJA_VOLUME_GRID_PIXEL_SIZE: f32 = 5.0;

fn nv_world_ray(uv: vec2<f32>) -> vec3<f32> {
    // Reversed-Z: unproject on the far plane, as froxel_fog.wgsl world_ray.
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    var far = nv.unjittered_inv_view_proj * ndc;
    far = far / max(abs(far.w), 1.0e-6);
    return normalize(far.xyz - nv.view.xyz);
}

fn nv_forward() -> vec3<f32> {
    return nv_world_ray(vec2<f32>(0.5, 0.5));
}

// Common.ush ComputeDepthFromZSlice.
fn nv_depth_from_zslice(zslice: f32) -> f32 {
    return (exp2(zslice / nv.grid_z.z) - nv.grid_z.y) / nv.grid_z.x;
}

// HeightFogCommon.ush ComputeNormalizedZSliceFromDepth.
fn nv_normalized_zslice(scene_depth: f32) -> f32 {
    return log2(scene_depth * nv.grid_z.x + nv.grid_z.y) * nv.grid_z.z / f32(nv.grid.z);
}

// VolumetricFog.usf ComputeCellTranslatedWorldPosition. SceneDepth is view
// depth; the position lies on the unjittered ray through the cell's NDC.
fn nv_cell_world(grid_coordinate: vec3<f32>, cell_offset: vec3<f32>) -> vec3<f32> {
    let volume_uv = (grid_coordinate.xy + cell_offset.xy) / vec2<f32>(nv.grid.xy);
    let scene_depth = nv_depth_from_zslice(max(grid_coordinate.z + cell_offset.z, 0.0));
    let ray = nv_world_ray(volume_uv);
    return nv.view.xyz + ray * (scene_depth / dot(ray, nv_forward()));
}
