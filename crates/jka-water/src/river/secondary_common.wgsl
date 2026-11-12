// Native river whitewater (SW_River_Secondary_Emitter): particle layout,
// Niagara GPU random numbers and curve LUTs, shared by the update kernel and
// the sprite draw. Copyright Epic Games, Inc. All Rights Reserved.
// See secondary.rs; the RIVER_SPRAY_* constants are generated there.
struct RiverSprayParticle {
    position_age: vec4<f32>,      // Particles.Position (river cm, Z up), Age
    velocity_lifetime: vec4<f32>, // Particles.Velocity (cm/s), Lifetime
    size: vec4<f32>,              // SpriteSize xy, Initial.SpriteSize xy
    misc: vec4<f32>,              // SpriteRotation (deg), NormalizedAge, MaterialRandom, ViewRecycler.TravelFraction
    state: vec4<i32>,             // VisibilityTag, VisibilityTagCache, flags, UniqueID
};
const RIVER_SPRAY_DO_RECYCLE: i32 = 1;
const RIVER_SPRAY_DO_REJECTION: i32 = 2;

// NiagaraEmitterInstanceShader.usf rand3/rand4. Two's-complement wrapping is
// identical in u32, and the 24 kept bits do not depend on the shift's sign.
fn river_spray_rand3(s1: i32, s2: i32, s3: i32, s4: i32) -> vec3<f32> {
    let packed = s4 | bitcast<i32>(bitcast<u32>(s3) << 16u);
    var v = bitcast<vec3<u32>>(vec3<i32>(s1, s2, packed)) * 1664525u + 1013904223u;
    v.x += v.y * v.z; v.y += v.z * v.x; v.z += v.x * v.y;
    v.x += v.y * v.z; v.y += v.z * v.x; v.z += v.x * v.y;
    return vec3<f32>((v >> vec3<u32>(8u)) & vec3<u32>(0x00ffffffu)) / 16777216.0;
}
fn river_spray_rand4(s1: i32, s2: i32, s3: i32, s4: i32) -> vec4<f32> {
    var v = bitcast<vec4<u32>>(vec4<i32>(s4, s1, s2, s3)) * 1664525u + 1013904223u;
    v.x += v.y * v.w; v.y += v.z * v.x; v.z += v.x * v.y; v.w += v.y * v.z;
    v.x += v.y * v.w; v.y += v.z * v.x; v.z += v.x * v.y; v.w += v.y * v.z;
    return vec4<f32>((v >> vec4<u32>(8u)) & vec4<u32>(0x00ffffffu)) / 16777216.0;
}

// GetCurveLUTIndices with LUTMinTime 0 and LUTInvTimeRange 1 (no serialized
// override), then lerp of the two LUT entries.
fn river_spray_alpha(normalized_age: f32) -> f32 {
    var lut = RIVER_SPRAY_ALPHA_LUT;
    let last = RIVER_SPRAY_ALPHA_COUNT - 1u;
    let t = saturate(normalized_age) * f32(last);
    let a = min(u32(floor(t)), last);
    let b = min(a + 1u, last);
    return mix(lut[a], lut[b], fract(t));
}
fn river_spray_size_scale(normalized_age: f32) -> f32 {
    var lut = RIVER_SPRAY_SIZE_LUT;
    let last = RIVER_SPRAY_SIZE_COUNT - 1u;
    let t = saturate(normalized_age) * f32(last);
    let a = min(u32(floor(t)), last);
    let b = min(a + 1u, last);
    return mix(lut[a], lut[b], fract(t));
}

// Exponent-bit test; independent of fast-math NaN assumptions.
fn river_spray_finite3(v: vec3<f32>) -> bool {
    return all((bitcast<vec3<u32>>(v) & vec3<u32>(0x7f800000u)) != vec3<u32>(0x7f800000u));
}
