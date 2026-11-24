// Fluid Ninja Fire_Smoke_Explosive GPU sprite particles, shared by the
// simulation (embers.wgsl) and the sprite draw (ninja_fire.wgsl).
// Kind 0 = turret VisualizedParticles1, kind 1 = turret VisualizedParticles2
// (NS_WriteDataCh_GPU_Turret), kind 2 = Burning Man decoration sparks
// (NS_Sparks_DecorationOnly2 / NS_PASSIVE_Sparks). Constants come from the
// assets' LastHlslTranslationGPU; spawn rates and the longest lifetimes for
// buffer sizing are also on the host (embers.rs PoolKind).

struct EmberParticle {
    // Authored centimetres (JKA/UE axes, Z up) relative to `anchor`; age.
    position_age: vec4<f32>,
    velocity_lifetime: vec4<f32>,
    // Particles.Initial.Color.
    color: vec4<f32>,
    // SpriteSize.x, Mass, Drag, kind.
    size_mass_drag_kind: vec4<f32>,
    // Spawn anchor in the pool frame (the sparks keep theirs as the player
    // moves on; zero for the embers, whose positions are frame-relative).
    anchor: vec4<f32>,
}

struct EmberRecipe {
    lifetime: vec2<f32>,
    size: vec2<f32>,
    color_low: vec4<f32>,
    color_high: vec4<f32>,
    // ENiagaraChannelCorrelation: 1 = Link RGB / Link A, 0 = Link RGBA.
    separate_alpha: f32,
    launch_speed: vec2<f32>,
    gravity: f32,
    drag: vec2<f32>,
    curl_strength: f32,
    // VelocityAligned renderer; ScaleSpriteSizeBySpeed length at 1000 cm/s.
    aligned: f32,
    stretch_max: f32,
    // ScaleColorBySpeed (100..1500 cm/s).
    speed_color: f32,
    // SphereMask 1 / (1 - Hardness); back-face emissive scale.
    inv_hardness: f32,
    back_face: f32,
}

fn ember_recipe(kind: u32) -> EmberRecipe {
    if (kind == 0u) {
        // VP1: DefaultSpriteMaterial (Hardness 0.5, one-sided).
        return EmberRecipe(
            vec2<f32>(1.4, 2.0), vec2<f32>(2.0, 8.0),
            vec4<f32>(10.0, 0.0, 0.0, 0.0), vec4<f32>(80.0, 0.3, 0.0, 1.0),
            1.0, vec2<f32>(550.0, 850.0), 400.0, vec2<f32>(0.4, 0.6), 800.0,
            1.0, 2.0, 1.0, 2.0, 0.15,
        );
    }
    if (kind == 1u) {
        // VP2: MI_SpriteMaterialSoft1 (Hardness -4, additive, two-sided), Unaligned.
        return EmberRecipe(
            vec2<f32>(2.0, 3.0), vec2<f32>(8.0, 16.0),
            vec4<f32>(2.0, 0.05, 0.0, 0.0), vec4<f32>(8.0, 0.2, 0.0, 0.5),
            0.0, vec2<f32>(320.0, 650.0), 900.0, vec2<f32>(0.4, 0.6), 1600.0,
            0.0, 1.0, 0.0, 0.2, 1.0,
        );
    }
    // Burning Man sparks: DefaultSpriteMaterial, VelocityAligned, constant colour,
    // ScaleSpriteSizeBySpeed (1,1)-(1,4), CurlNoiseForce001 strength 1200.
    return EmberRecipe(
        vec2<f32>(0.5, 1.5), vec2<f32>(0.5, 2.0),
        vec4<f32>(10.0, 0.5, 0.0, 1.0), vec4<f32>(10.0, 0.5, 0.0, 1.0),
        0.0, vec2<f32>(0.0), 0.0, vec2<f32>(20.0, 60.0), 1200.0,
        1.0, 4.0, 0.0, 2.0, 0.15,
    );
}

// ScaleSpriteSizeBySpeed: length from 1 to `stretch_max` at 1000 cm/s (linear curve).
fn ember_stretch(speed: f32, recipe: EmberRecipe) -> f32 {
    return 1.0 + (recipe.stretch_max - 1.0) * clamp(speed, 0.0001, 1000.0) / 1000.0;
}

// VP2 ScaleColor alpha: FloatFromCurve(NormalizedAge), keys (-0.003715,
// 1.413183), (0.266473, 1.004167), (1, 0). Linear between keys (UE: auto cubic).
fn ember_age_alpha(t: f32) -> f32 {
    if (t <= -0.003715) { return 1.413183; }
    if (t <= 0.266473) { return mix(1.413183, 1.004167, (t + 0.003715) / (0.266473 + 0.003715)); }
    return mix(1.004167, 0.0, clamp((t - 0.266473) / (1.0 - 0.266473), 0.0, 1.0));
}

// Particles.Color as drawn: VP1 ScaleColorBySpeed (RGB and A from 0 to 1);
// VP2 alpha from the age curve; sparks constant.
fn ember_color(particle: EmberParticle, recipe: EmberRecipe, kind: u32) -> vec4<f32> {
    let color = particle.color;
    if (recipe.speed_color > 0.5) {
        let speed = length(particle.velocity_lifetime.xyz);
        return color * clamp(max(speed - 100.0, 0.0001) / 1400.0, 0.0, 1.0);
    }
    if (kind == 1u) {
        let age = particle.position_age.w / max(particle.velocity_lifetime.w, 1e-4);
        return vec4<f32>(color.rgb, color.a * ember_age_alpha(age));
    }
    return color;
}
