// The post-process uniform block. One definition shared by every shader that
// binds the post buffer (post.wgsl and rain_haze_mask.wgsl); it must match the
// field order of `PostUniform` in renderer.rs, which a unit test checks.
struct PostSettings {
    color: vec4<f32>, // gamma, tone mapping (0..1 SDR ACES / 3 HDR output / 4 native AgX), bloom, ssao
    aa: vec4<f32>, // fxaa, viewport width, viewport height, taa
    taa_params: vec4<f32>, // x HDR scene, y reflection debug, z reflection quality, w reserved
    scene: vec4<f32>, // contact shadows, volumetric fog, ssr, history valid
    film: vec4<f32>, // halation, chromatic aberration, vignette, LUT strength
    grain: vec4<f32>, // strength, size, time, manual exposure
    exposure_controls: vec4<f32>, // x auto exposure strength; y native night readability; zw reserved
    camera_fx: vec4<f32>, // motion shutter scale, DOF strength, focus distance, DOF quality
    legacy_fog: vec4<f32>, // authored display-space RGB, depthForOpaque; taa_params.w = scale/enabled
    clouds: vec4<f32>, // enabled, type, quality, coverage
    cloud_layer: vec4<f32>, // base height, thickness, wind speed, wind direction radians
    cloud_sun_direction: vec4<f32>, // xyz light travel direction, w intensity
    cloud_sun_color: vec4<f32>, // rgb color, w density scale
    cloud_shadow: vec4<f32>, // enabled, projected shadow strength, temporal depth-gate fix, terrain interaction
    cloud_shaping: vec4<f32>, // wind shear, base height variation, empty-space skip gate, aerial perspective
    cloud_sky_ambient: vec4<f32>, // rgb average of the map skybox, w blend amount
    cloud_temporal_tuning: vec4<f32>, // history blend, motion reject, depth reject, shape evolution gate
    cloud_variation: vec4<f32>, // thickness variation, cloud size, weather gust, direction variation radians
    cloud_temporal: vec4<f32>, // enabled, history valid, active 2x2 pattern, grid size
    cloud_wind_dir: vec4<f32>, // xyz unit wind direction now (frame constant, see cloud_wind.rs)
    cloud_wind_offset: vec4<f32>, // xyz accumulated cloud advection now
    cloud_wind_delta: vec4<f32>, // xyz advection since the previous frame, for history reprojection
    cloud_detail_slip: vec4<f32>, // xyz drift of the erosion volume against the weather field
    cloud_detail_billow: vec4<f32>, // xyz unit-scale erosion wander; scaled by tile in the shader
    rain: vec4<f32>, // enabled, intensity, distant haze strength, puddle accumulation
    rain_occlusion: vec4<f32>, // min render X/Z, inverse heightfield extent X/Z
    weather_look: vec4<f32>, // scattered puddle amount, wet grade amount, high-quality water, rain reaching camera (shared with RainyGlass lens exposure)
    legacy_weather: vec4<f32>, // sand haze, spacedust haze, acid-rain haze, 2D-precipitation-haze flag
    snow_haze: vec4<f32>, // x 2D snow haze density/cloud count (zero in froxel mode)
    underwater: vec4<f32>, // water surface Y above a submerged camera (-1e30 when not submerged), absorption distance, promoted ocean plane Y (no-receiver volumetric fog), its square half-extent (0 = off)
    cloud_foreground: vec4<f32>, // lit saber blade count, promoted ocean fog centre X/Z, 1 / sky-portal scale (0 = no portal depth bound, negative = unscaled portal that just hides clouds)
    cloud_blades: array<vec4<f32>, 16>, // per blade: ax, ay, bx, by (pixels); glow radius px, nearest distance
    camera_pos_time: vec4<f32>,
    prev_camera_pos_time: vec4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cloud_inv_view_proj: mat4x4<f32>,
    cloud_prev_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    motion_prev_view_proj: mat4x4<f32>,
};
