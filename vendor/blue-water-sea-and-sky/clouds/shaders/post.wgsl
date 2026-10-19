// The sky's post pass's uniforms and bindings (`SkyApi.postProcess`): heavy rain's veil, from
// the web game's `sky/weather/haze.ts`, drawn by `haze.wgsl`. (The screen-space sun shafts that also ran here went with the
// old clouds; takram's light shafts take their place in the sky's dome.)
#define_import_path naval_clouds::post

// `PostUniforms` in `src/post.rs`, 192 bytes.
struct PostUniforms {
    inverse_projection: mat4x4<f32>,
    camera_world: mat4x4<f32>,
    // The rain veil: strength (0 dry), camera height, cloud base above the camera, the strike's irradiance at the
    // camera.
    haze_strength: f32,
    haze_height: f32,
    haze_ceiling: f32,
    lightning: f32,
    // How grey the rain-lit air is.
    grey: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
    bolt_direction: vec3<f32>,
    spare: f32,
    forward: vec3<f32>,
    spare_too: f32,
}

@group(0) @binding(0) var<uniform> post: PostUniforms;
@group(2) @binding(0) var scene_color: texture_2d<f32>;
// The opaque scene's reversed depth (0 where there is none: sky). Host fix: the veil must stop at the first surface.
@group(2) @binding(1) var scene_depth: texture_depth_2d;
