// CombineVolumetricFog for the Ninja fog volume (UE 5.6 HeightFogPixelShader.usf,
// r.VolumetricFog.UpsampleJitterMultiplier 0): a trilinear lookup of the
// integrated volume, composited as scene * transmittance + in-scattering.
// Prefixed by ninja_fog_volume_common.wgsl.

@group(0) @binding(0) var<uniform> nv: NinjaVolumeParams;
@group(0) @binding(1) var nv_integrated: texture_3d<f32>;
@group(0) @binding(2) var nv_integrated_sampler: sampler;

struct NinjaApplyOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn ninja_apply_vs(@builtin(vertex_index) index: u32) -> NinjaApplyOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: NinjaApplyOut;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn ninja_apply_fs(input: NinjaApplyOut) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let dims = textureDimensions(ninja_fog_scene_depth);
    let pixel = vec2<i32>(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.999999)) * vec2<f32>(dims));
    let device_depth = ninja_fog_device_depth(pixel);
    // HeightFogPixelShader evaluates the integrated fog at the opaque
    // SceneDepth. Clear reverse-Z pixels sample the last fog slice.
    var scene_depth = 1.0e7;
    if (device_depth > 0.0) {
        let clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, device_depth, 1.0);
        let world = nv.unjittered_inv_view_proj * clip;
        if (world.w > 0.0) {
            scene_depth = dot(world.xyz / world.w - nv.view.xyz, nv_forward());
        }
    }
    // SVPos * VolumetricFogSVPosToVolumeUV, clamped to VolumetricFogUVMax.
    let viewport = nv.misc.xy;
    let resource_size = vec2<f32>(nv.grid.xy) * NINJA_VOLUME_GRID_PIXEL_SIZE;
    let uv_max = (vec2<f32>(nv.grid.xy) - 0.5) / vec2<f32>(nv.grid.xy);
    let volume_uv = vec3<f32>(
        min(uv * viewport / resource_size, uv_max),
        nv_normalized_zslice(scene_depth),
    );
    let lookup = textureSampleLevel(nv_integrated, nv_integrated_sampler, volume_uv, 0.0);
    // Blend: dst * src.a + src.rgb.
    return vec4<f32>(lookup.rgb, lookup.a);
}
