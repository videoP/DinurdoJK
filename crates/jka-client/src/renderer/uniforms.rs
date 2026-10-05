//! Uniforms.
use crate::renderer::{
    cloud_wind, CloudRenderResolution, CloudType, Pod, PostEffects, SunVisibilityMode,
    TransientLight, Vec3, Zeroable, MAX_CLOUD_FOREGROUND_BLADES, PLANAR_REFLECTION_SLOTS,
    SHADOW_CASCADES,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct CameraUniform {
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) camera_pos_time: [f32; 4],
    // xyz + d plane equation. A zero normal disables reflection-pass clipping.
    pub(in crate::renderer) clip_plane: [f32; 4],
    // x != 0 marks the lightweight reflected view. The BSP shader uses this
    // to skip main-camera clustered/cascade data that is invalid for that view.
    pub(in crate::renderer) render_flags: [u32; 4],
    // xyz: forward vector for OpenJK-compatible eye-depth fog.
    pub(in crate::renderer) camera_forward: [f32; 4],
    // Bevy TAA motion-vector prepass uses unjittered current/previous clip matrices.
    // Keep these at the end so shaders that only consume the historical prefix remain valid.
    pub(in crate::renderer) unjittered_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) previous_unjittered_view_proj: [[f32; 4]; 4],
    // Jump-height helper (jump_shade.rs): jump-line floor Y, reachable apex Y, gradient
    // range, strength. Zero strength disables the tint; only the main world camera sets it.
    pub(in crate::renderer) jump_shade: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct MaterialUniform {
    pub(in crate::renderer) header: [u32; 4],
    pub(in crate::renderer) vector_s: [f32; 4],
    pub(in crate::renderer) vector_t: [f32; 4],
    pub(in crate::renderer) mods: [[f32; 4]; 8],
    pub(in crate::renderer) color: [f32; 4],
    pub(in crate::renderer) params: [f32; 4],
    // xy normalScale, z fixed roughness (-1 = texture/default), w cached roughness hint.
    pub(in crate::renderer) pbr_params0: [f32; 4],
    // xyz fixed dielectric reflectance, w != 0 when authored.
    pub(in crate::renderer) pbr_params1: [f32; 4],
    pub(in crate::renderer) reflection_probe: [f32; 4],
    pub(in crate::renderer) planar_plane: [f32; 4],
    // `rgbGen wave` / `alphaGen wave`: base, amplitude, phase, frequency.
    pub(in crate::renderer) wave_rgb: [f32; 4],
    pub(in crate::renderer) wave_alpha: [f32; 4],
    // x: rgb waveform id, y: alpha waveform id (see jka_assets::shader::WaveFunc::id).
    pub(in crate::renderer) wave_funcs: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct StaticLightGridUniform {
    // xyz: JKA-space origin, w: enabled
    pub(in crate::renderer) origin_enabled: [f32; 4],
    // xyz: inverse JKA grid spacing
    pub(in crate::renderer) inv_size: [f32; 4],
    // xyz: grid dimensions
    pub(in crate::renderer) bounds: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct IrradianceVolumeUniform {
    // Bevy LightProbe transform: renderer world space -> a 1x1x1 cube centered
    // on the origin. The JKA adapter constructs this from the BSP lightgrid.
    pub(in crate::renderer) light_from_world: [[f32; 4]; 4],
    // xyz: Bevy LightProbe falloff ratio per axis, w: IrradianceVolume intensity.
    pub(in crate::renderer) falloff_intensity: [f32; 4],
    // x: volume present/enabled. Remaining components are padding.
    pub(in crate::renderer) volume_enabled_pad: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct VoxelProbeGiUniform {
    // xyz: first probe center in renderer coordinates, w: data present
    pub(in crate::renderer) origin_enabled: [f32; 4],
    // xyz: reciprocal cell size, w: cell size
    pub(in crate::renderer) inv_cell: [f32; 4],
    // xyz: probe dimensions, w: final indirect intensity
    pub(in crate::renderer) bounds_strength: [f32; 4],
}

impl Default for MaterialUniform {
    fn default() -> Self {
        Self {
            header: [0; 4],
            vector_s: [0.0; 4],
            vector_t: [0.0; 4],
            mods: [[0.0; 4]; 8],
            color: [1.0; 4],
            params: [0.0; 4],
            pbr_params0: [1.0, 1.0, -1.0, 0.0],
            pbr_params1: [0.04, 0.04, 0.04, 0.0],
            reflection_probe: [0.0; 4],
            planar_plane: [0.0; 4],
            wave_rgb: [0.0; 4],
            wave_alpha: [0.0; 4],
            wave_funcs: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PlanarReflectionUniform {
    // Up to four renderer-space mirror planes, xyz normal + d.
    pub(in crate::renderer) planes: [[f32; 4]; PLANAR_REFLECTION_SLOTS],
    // xy main-frame viewport size, z active slot count,
    // w permits planar promotion of tcGen environment stages.
    pub(in crate::renderer) viewport: [f32; 4],
    // x: PlanarReflectionDebugMode shader value. Remaining lanes reserved.
    pub(in crate::renderer) debug: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostColorSettings {
    pub(in crate::renderer) gamma: f32,
    pub(in crate::renderer) tone_mapping: f32,
    pub(in crate::renderer) bloom: f32,
    pub(in crate::renderer) ssao: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostAaSettings {
    pub(in crate::renderer) fxaa: f32,
    pub(in crate::renderer) viewport_width: f32,
    pub(in crate::renderer) viewport_height: f32,
    pub(in crate::renderer) taa: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostSceneSettings {
    pub(in crate::renderer) contact_shadows: f32,
    pub(in crate::renderer) volumetric_fog: f32,
    pub(in crate::renderer) ssr: f32,
    pub(in crate::renderer) history_valid: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostFilmSettings {
    pub(in crate::renderer) halation: f32,
    pub(in crate::renderer) chromatic_aberration: f32,
    pub(in crate::renderer) vignette: f32,
    pub(in crate::renderer) lut_strength: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostGrainSettings {
    pub(in crate::renderer) strength: f32,
    pub(in crate::renderer) grain_size: f32,
    pub(in crate::renderer) time: f32,
    pub(in crate::renderer) exposure: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCameraFxSettings {
    // Camera-motion shutter scale, DOF strength, smoothed autofocus distance,
    // and DOF quality (0 performance, 1 adaptive, 2 high).
    pub(in crate::renderer) motion_blur_scale: f32,
    pub(in crate::renderer) depth_of_field: f32,
    pub(in crate::renderer) dof_focus_distance: f32,
    pub(in crate::renderer) dof_quality: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct DofUniform {
    pub(in crate::renderer) focus_strength: [f32; 4], // focus distance, strength, viewport width, height
    pub(in crate::renderer) quality: [f32; 4],        // x: 0 performance, 1 adaptive, 2 high
}

/// Everything that invalidates the temporal cloud history when it changes: the
/// sun, the cloud shape controls, the wind, and the temporal tuning itself.
#[derive(Clone, PartialEq)]
pub(in crate::renderer) struct CloudHistoryKey {
    pub(in crate::renderer) sun_override: bool,
    pub(in crate::renderer) sun_visibility: SunVisibilityMode,
    pub(in crate::renderer) sun_yaw: f32,
    pub(in crate::renderer) sun_pitch: f32,
    pub(in crate::renderer) sun_intensity: f32,
    pub(in crate::renderer) sun_color: [f32; 3],
    pub(in crate::renderer) clouds: bool,
    pub(in crate::renderer) cloud_type: CloudType,
    pub(in crate::renderer) quality: f32,
    pub(in crate::renderer) coverage: f32,
    pub(in crate::renderer) height: f32,
    pub(in crate::renderer) thickness: f32,
    pub(in crate::renderer) weather_wind: crate::ocean::OceanWind,
    pub(in crate::renderer) resolution: CloudRenderResolution,
    pub(in crate::renderer) temporal: bool,
    pub(in crate::renderer) temporal_depth_fix: bool,
    pub(in crate::renderer) shear: f32,
    pub(in crate::renderer) base_variation: f32,
    pub(in crate::renderer) shape_evolution: bool,
    pub(in crate::renderer) terrain_interaction: bool,
    pub(in crate::renderer) empty_skip: bool,
    pub(in crate::renderer) aerial: f32,
    pub(in crate::renderer) sky_ambient: bool,
    pub(in crate::renderer) history_blend: f32,
    pub(in crate::renderer) motion_reject: f32,
    pub(in crate::renderer) history_depth_reject: bool,
    pub(in crate::renderer) thickness_variation: f32,
    pub(in crate::renderer) size: f32,
}

impl CloudHistoryKey {
    pub(in crate::renderer) fn of(effects: &PostEffects) -> Self {
        Self {
            sun_override: effects.sun_override,
            sun_visibility: effects.sun_visibility,
            sun_yaw: effects.sun_yaw.rem_euclid(360.0),
            sun_pitch: effects.sun_pitch,
            sun_intensity: effects.sun_intensity,
            sun_color: effects.sun_color,
            clouds: effects.clouds,
            cloud_type: effects.cloud_type,
            quality: effects.cloud_quality,
            coverage: effects.cloud_coverage,
            height: effects.cloud_height,
            thickness: effects.cloud_thickness,
            weather_wind: effects.weather_wind.sanitize(),
            resolution: effects.cloud_render_resolution,
            temporal: effects.cloud_temporal,
            temporal_depth_fix: effects.cloud_temporal_depth_fix,
            shear: effects.cloud_shear,
            base_variation: effects.cloud_base_variation,
            shape_evolution: effects.cloud_shape_evolution,
            terrain_interaction: effects.cloud_terrain_interaction,
            empty_skip: effects.cloud_empty_skip,
            aerial: effects.cloud_aerial,
            sky_ambient: effects.cloud_sky_ambient,
            history_blend: effects.cloud_history_blend,
            motion_reject: effects.cloud_motion_reject,
            history_depth_reject: effects.cloud_history_depth_reject,
            thickness_variation: effects.cloud_thickness_variation,
            size: effects.cloud_size,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct CloudRenderSettings {
    pub(in crate::renderer) values: [f32; 4], // enabled, type, quality, coverage
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct CloudLayerSettings {
    pub(in crate::renderer) values: [f32; 4], // base height, thickness, wind speed, wind direction radians
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct CloudSunSettings {
    pub(in crate::renderer) direction_intensity: [f32; 4],
    pub(in crate::renderer) color_density: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudShadowSettings {
    pub(in crate::renderer) values: [f32; 4], // enabled, projected shadow strength, temporal depth-gate fix, terrain interaction
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudShapingSettings {
    // wind shear, base height variation, empty-space skip gate, aerial perspective
    pub(in crate::renderer) values: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudSkyAmbientSettings {
    pub(in crate::renderer) values: [f32; 4], // rgb average of the map skybox, w blend amount
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudTemporalTuningSettings {
    pub(in crate::renderer) values: [f32; 4], // history blend, motion reject, depth reject, reserved
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudVariationSettings {
    pub(in crate::renderer) values: [f32; 4], // thickness variation, cloud size, wind variation gate, shape evolution gate
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudTemporalSettings {
    pub(in crate::renderer) values: [f32; 4], // enabled, history valid, active 2x2 pattern, grid size
}

/// Frame-constant wind terms the cloud shader used to recompute per sample.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostCloudWindSettings {
    pub(in crate::renderer) direction: [f32; 4],
    pub(in crate::renderer) offset: [f32; 4],
    pub(in crate::renderer) delta: [f32; 4],
    pub(in crate::renderer) detail_slip: [f32; 4],
    pub(in crate::renderer) detail_billow: [f32; 4],
}

impl From<cloud_wind::CloudWindTerms> for PostCloudWindSettings {
    fn from(terms: cloud_wind::CloudWindTerms) -> Self {
        Self {
            direction: terms.direction,
            offset: terms.offset,
            delta: terms.delta,
            detail_slip: terms.detail_slip,
            detail_billow: terms.detail_billow,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PostUniform {
    pub(in crate::renderer) color: PostColorSettings,
    pub(in crate::renderer) aa: PostAaSettings,
    // x: scene is HDR (Bevy tonemaps before temporal resolve and reverses after).
    pub(in crate::renderer) taa_params: [f32; 4],
    pub(in crate::renderer) scene: PostSceneSettings,
    pub(in crate::renderer) film: PostFilmSettings,
    pub(in crate::renderer) grain: PostGrainSettings,
    pub(in crate::renderer) camera_fx: PostCameraFxSettings,
    // rgb: authored OpenJK global fog color in legacy framebuffer/display space;
    // w: depthForOpaque. taa_params.w carries the authored strength scale.
    pub(in crate::renderer) legacy_fog: [f32; 4],
    pub(in crate::renderer) clouds: CloudRenderSettings,
    pub(in crate::renderer) cloud_layer: CloudLayerSettings,
    pub(in crate::renderer) cloud_sun: CloudSunSettings,
    pub(in crate::renderer) cloud_shadow: PostCloudShadowSettings,
    pub(in crate::renderer) cloud_shaping: PostCloudShapingSettings,
    pub(in crate::renderer) cloud_sky_ambient: PostCloudSkyAmbientSettings,
    pub(in crate::renderer) cloud_temporal_tuning: PostCloudTemporalTuningSettings,
    pub(in crate::renderer) cloud_variation: PostCloudVariationSettings,
    pub(in crate::renderer) cloud_temporal: PostCloudTemporalSettings,
    pub(in crate::renderer) cloud_wind: PostCloudWindSettings,
    pub(in crate::renderer) rain: [f32; 4], // enabled, intensity, distant haze strength, puddle accumulation
    pub(in crate::renderer) weather_occlusion: [f32; 4], // min render X/Z, inverse heightfield extent X/Z
    pub(in crate::renderer) weather_look: [f32; 4], // scattered puddle amount, wet grade amount, high-quality water, reserved
    pub(in crate::renderer) underwater: [f32; 4], // surface Y over a submerged camera (NO_WATER_SURFACE if dry), absorption distance, reserved
    pub(in crate::renderer) cloud_foreground: [f32; 4], // blade count, reserved x3
    // Two vec4 per blade: [ax, ay, bx, by] pixels, then [glow radius px, nearest distance].
    pub(in crate::renderer) cloud_blades: [[f32; 4]; MAX_CLOUD_FOREGROUND_BLADES * 2],
    pub(in crate::renderer) camera_pos_time: [f32; 4],
    pub(in crate::renderer) prev_camera_pos_time: [f32; 4],
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) inv_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) cloud_inv_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) cloud_prev_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) prev_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) motion_prev_view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct SsaoTemporalUniform {
    // x/y: full-resolution viewport, z: temporal history valid.
    pub(in crate::renderer) viewport_history: [f32; 4],
    pub(in crate::renderer) camera_pos_time: [f32; 4],
    pub(in crate::renderer) previous_camera_pos: [f32; 4],
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) inv_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) prev_view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct SsrTemporalUniform {
    // x/y: full-resolution viewport, z: temporal history valid, w: frame index.
    pub(in crate::renderer) viewport_history: [f32; 4],
    pub(in crate::renderer) camera_pos_time: [f32; 4],
    pub(in crate::renderer) previous_camera_pos: [f32; 4],
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) inv_view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) prev_view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct GammaPostUniform {
    pub(in crate::renderer) values: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct AutoExposureUniform {
    // enabled, delta seconds, min EV, max EV
    pub(in crate::renderer) params: [f32; 4],
    // middle gray, brighten speed, darken speed, reserved
    pub(in crate::renderer) adaptation: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct AutoExposureState {
    pub(in crate::renderer) exposure_ev: f32,
    pub(in crate::renderer) average_luminance: f32,
    pub(in crate::renderer) target_ev: f32,
    pub(in crate::renderer) reserved: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct GpuCullRecord {
    pub(in crate::renderer) minimum: [f32; 4],
    pub(in crate::renderer) maximum: [f32; 4],
    pub(in crate::renderer) draw: [u32; 4], // index_count, first_index, sky flag, reserved
    pub(in crate::renderer) compact: [u32; 4], // group index, output base, compactable, reserved
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct GpuCullSettings {
    pub(in crate::renderer) viewport_mips_flags: [u32; 4], // width, height, mip count, hi-z enabled
    pub(in crate::renderer) active_compaction: [u32; 4], // active records, groups, compaction enabled, diagnostics enabled
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct DrawIndexedIndirectArgs {
    pub(in crate::renderer) index_count: u32,
    pub(in crate::renderer) instance_count: u32,
    pub(in crate::renderer) first_index: u32,
    pub(in crate::renderer) base_vertex: i32,
    pub(in crate::renderer) first_instance: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct GpuPointLight {
    pub(in crate::renderer) position_radius: [f32; 4],
    pub(in crate::renderer) color_intensity: [f32; 4],
    // xyz: area normal, or RT segment half-vector when w=-1.
    // w: -1=segment, 0=point, 1=one-sided area, 2=two-sided area.
    pub(in crate::renderer) emitter: [f32; 4],
    pub(in crate::renderer) shadow: [f32; 4], // x: cubemap slot + 1, 0 means unshadowed
}

pub(in crate::renderer) fn transient_light_gpu(
    light: &TransientLight,
    intensity_scale: f32,
    segment_enabled: bool,
) -> GpuPointLight {
    let mut result = GpuPointLight {
        position_radius: [
            light.position[0],
            light.position[1],
            light.position[2],
            light.radius.max(0.0),
        ],
        color_intensity: [
            light.color[0],
            light.color[1],
            light.color[2],
            light.intensity.max(0.0) * intensity_scale,
        ],
        emitter: [0.0; 4],
        shadow: [0.0, 0.0, 0.0, 1.0],
    };
    if segment_enabled {
        if let Some([start, end]) = light
            .segment
            .filter(|ends| ends.iter().flatten().all(|v| v.is_finite()))
        {
            let half = (Vec3::from_array(end) - Vec3::from_array(start)) * 0.5;
            let midpoint = Vec3::from_array(start) + half;
            if half.is_finite() && midpoint.is_finite() && half.length_squared() > 1e-8 {
                result.position_radius[..3].copy_from_slice(&midpoint.to_array());
                result.emitter = [half.x, half.y, half.z, -1.0];
            }
        }
    }
    result
}

pub(in crate::renderer) fn transient_light_samples(
    light: &TransientLight,
    rt: bool,
) -> impl Iterator<Item = TransientLight> + '_ {
    let blades = light
        .blade_segments
        .as_deref()
        .filter(|blades| rt && !blades.is_empty());
    let count = blades.map_or(1, |blades| blades.len());
    (0..count).map(move |index| {
        let mut sample = light.clone();
        if let Some(blades) = blades {
            let blade = blades[index];
            sample.segment = Some(blade.endpoints);
            sample.color = blade.rgb;
            sample.intensity *= blade.weight;
            sample.blade_segments = None;
        }
        sample
    })
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct LightingSettings {
    pub(in crate::renderer) values: [u32; 4], // enabled, light count, viewport width, viewport height
    pub(in crate::renderer) local_shadows: [u32; 4], // enabled, shadowed count, cubemap size, transient-light start
    pub(in crate::renderer) feature_flags: [u32; 4], // area lights, voxel/probe GI, point-light mode (0/1/2), reflection quality
    pub(in crate::renderer) map_ambient: [f32; 4], // xyz q3map2 ambient RGB; w RT sample count (1, 2, 4)
    pub(in crate::renderer) map_minlight: [f32; 4], // source-.map q3map2 minlight RGB, normalized; w runtime-dlight falloff mode (0 stock, 1 physical)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct PbrSettings {
    pub(in crate::renderer) values: [u32; 4], // PBR enabled, parallax enabled, height scale * 1000, reserved
    // x deluxe enabled, y deluxe specular, z detail distance fade enabled,
    // w detail fade distance in JKA map units.
    pub(in crate::renderer) deluxe: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct ShadowReceiverUniform {
    pub(in crate::renderer) view_proj: [[[f32; 4]; 4]; SHADOW_CASCADES],
    pub(in crate::renderer) split_depths: [f32; 4],
    pub(in crate::renderer) light_direction_enabled: [f32; 4],
    pub(in crate::renderer) params: [f32; 4], // map size, legacy bias, shadow strength, 1 = Bevy CSM
    pub(in crate::renderer) camera_forward: [f32; 4],
    pub(in crate::renderer) cascade_texel_sizes: [f32; 4],
    pub(in crate::renderer) bevy_params: [f32; 4], // depth bias, normal bias, overlap proportion, cascade count
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct ShadowCasterUniform {
    pub(in crate::renderer) view_proj: [[f32; 4]; 4],
    pub(in crate::renderer) camera_pos_time: [f32; 4],
}

/// Entity map shadow window: radius of the orthographic map, in world units.
/// 2048 texels across 2 * 768 units is ~0.75 unit per texel, finer than the
/// nearest BSP cascade, so one map is enough for the characters in view.
pub(in crate::renderer) const ENTITY_SHADOW_RADIUS: f32 = 768.0;

/// Receiver darkening at full contrast (the shared receiver multiplies the lit
/// colour by `1 - strength * facing * occlusion`).
pub(in crate::renderer) const ENTITY_SHADOW_MAX_STRENGTH: f32 = 0.55;

/// Lowest allowed sin(elevation) of the shadow-casting light. Near-horizontal
/// baked light would throw shadows hundreds of units; JKA's own shadow code
/// likewise forced its light mostly downward.
pub(in crate::renderer) const ENTITY_SHADOW_MIN_ELEVATION: f32 = 0.3;

/// Time constant for smoothing the lightgrid-derived direction/strength, so
/// crossing probe cells or a dark doorway never snaps the shadow.
pub(in crate::renderer) const ENTITY_SHADOW_SMOOTHING_SECONDS: f32 = 0.2;

/// Light cameras kept for entity casters: the Entity map uses [0]; the CSM modes
/// draw entities into cascades 0 and 1 and use [0] and [1].
pub(in crate::renderer) const ENTITY_SHADOW_CAMERAS: usize = 2;

/// Authored lights farther than this from the player are ignored as shadow sources.
pub(in crate::renderer) const ENTITY_SHADOW_LIGHT_RANGE: f32 = 1536.0;
