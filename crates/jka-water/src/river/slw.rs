//! Single Layer Water for the selected live river material: the resolved
//! coefficient/surface graph (slw-reference.json) and the engine's FULL-quality
//! water volume lighting (SingleLayerWaterShading.ush, SingleLayerWaterCommon.ush,
//! DistortionCommon.ush, ParticipatingMediaCommon.ush, ShadingCommon.ush,
//! BRDF.ush, SystemTextures.cpp PreintegratedGF). Copyright Epic Games, Inc.
//! All Rights Reserved. Scene inputs (scene color/depth without water, light
//! colours, shadows, reflections) are explicit: nothing here substitutes them.
//! Lengths are Unreal centimetres; the map adapter converts at its boundary.

pub const SHADER: &str = include_str!("slw.wgsl");

/// Live-instance values reached by the resolved SLW and surface graphs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    pub scattering: [f32; 4],
    pub absorption: [f32; 4],
    pub anisotropy: f32,
    pub velocity_to_mask_waves: f32,
    pub water_albedo: [f32; 3],
    pub caustics_max_intensity: f32,
    pub water_specular: f32,
    pub water_fresnel_specular: f32,
    pub water_roughness: f32,
    pub water_fresnel_roughness: f32,
    pub water_fresnel_exponent: f32,
    pub refraction: f32,
    pub refraction_far: f32,
    pub depth_for_min_refraction: f32,
    pub critical_angle_dot: f32,
    pub critical_angle_width: f32,
    pub critical_angle_bend_width: f32,
    pub refraction_bottom_amount: f32,
    pub lip_refraction: f32,
    pub refraction_depth_bias: f32,
}
impl Default for Parameters {
    fn default() -> Self {
        Self {
            scattering: [0.7656229734420776, 1.0, 0.9241690039634705, 0.5],
            absorption: [20.83333396911621, 25.0, 24.67391586303711, 8.0],
            anisotropy: 0.10000000149011612,
            velocity_to_mask_waves: 100.0,
            water_albedo: [0.08333300054073334; 3],
            caustics_max_intensity: 1.0,
            water_specular: 0.22499999403953552,
            water_fresnel_specular: 0.22499999403953552,
            water_roughness: 0.019999999552965164,
            water_fresnel_roughness: 0.10000000149011612,
            water_fresnel_exponent: 5.0,
            refraction: 1.149999976158142,
            refraction_far: 1.0199999809265137,
            depth_for_min_refraction: 5000.0,
            critical_angle_dot: 0.22499999403953552,
            critical_angle_width: 0.009999999776482582,
            critical_angle_bend_width: -0.75,
            refraction_bottom_amount: 1.0,
            lip_refraction: 0.949999988079071,
            refraction_depth_bias: 0.0,
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}
fn saturate(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// SingleLayerWaterMaterialOutput inputs, before the engine's max/clamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients {
    pub scattering: [f32; 3],
    pub absorption: [f32; 3],
    pub phase_g: f32,
    pub color_scale_behind_water: [f32; 3],
}

/// `river_scattering` is MF_ShallowWaterRiver's ShallowWater_SLW_Scattering
/// (foam::present's `scattering`); `water_info_velocity` is WaterBodyData's
/// DecodeWaterInfoVelocity sample; `two_sided_sign` is +1 front, -1 back.
pub fn coefficients(p: &Parameters, river_scattering: [f32; 3], water_info_velocity: [f32; 2], two_sided_sign: f32) -> Coefficients {
    let speed = (water_info_velocity[0] * water_info_velocity[0] + water_info_velocity[1] * water_info_velocity[1]).sqrt();
    let blend = saturate(speed / p.velocity_to_mask_waves);
    let front = saturate(two_sided_sign);
    let scattering = std::array::from_fn(|i| {
        let base = 0.0 + p.scattering[i] * p.scattering[3];
        let river = 0.0 + blend * (river_scattering[i] - 0.0);
        1.0 * ((base + river) / 1000.0 * front)
    });
    let absorption = std::array::from_fn(|i| 1.0 * ((1.0 / p.absorption[i]) / p.absorption[3] * front));
    Coefficients {
        scattering,
        absorption,
        phase_g: p.anisotropy,
        color_scale_behind_water: [1.0; 3],
    }
}

/// UMaterialExpressionFresnel with BaseReflectFraction 0 and the pixel normal.
pub fn fresnel(n_dot_v: f32, exponent: f32) -> f32 {
    let base = (1.0 - n_dot_v.max(0.0)).abs().max(1.0e-4);
    base.max(0.0).powf(exponent) * (1.0 - 0.0) + 0.0
}

/// WaterAttributes then Water_Underside; MF_ShallowWaterRiver's foam lerps
/// (foam::attributes) follow, then its opacity add.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub base_color: [f32; 3],
    pub metallic: f32,
    pub specular: f32,
    pub roughness: f32,
    pub opacity: f32,
    /// Material Refraction (pixel-normal strength) and RefractionDepthBias.
    pub refraction: [f32; 2],
}

/// `n_dot_v` is dot(PixelNormalWS, CameraVectorWS); `pixel_depth` in cm.
pub fn surface(p: &Parameters, n_dot_v: f32, two_sided_sign: f32, pixel_depth: f32) -> Surface {
    let f = fresnel(n_dot_v, p.water_fresnel_exponent);
    let specular = lerp(p.water_specular, p.water_fresnel_specular, f);
    let roughness = lerp(p.water_roughness, p.water_fresnel_roughness, f);
    let front = saturate(two_sided_sign);
    let critical = p.critical_angle_dot - n_dot_v;
    let tir = lerp(saturate(critical / p.critical_angle_width), 0.0, front);
    let refraction = lerp(p.refraction, p.refraction_far, saturate(pixel_depth / p.depth_for_min_refraction));
    let bottom = lerp(1.0, refraction, p.refraction_bottom_amount);
    let lip = lerp(bottom, p.lip_refraction, 1.0 - saturate(critical / p.critical_angle_bend_width));
    Surface {
        base_color: p.water_albedo.map(|a| a / p.caustics_max_intensity),
        metallic: 0.0,
        specular: lerp(specular, 1.0, tir),
        roughness,
        opacity: lerp(0.0, 1.0, tir),
        refraction: [lerp(lip, refraction, front), p.refraction_depth_bias],
    }
}

/// ShadingCommon.ush: DielectricSpecularToF0 then DielectricF0ToIor.
/// `half` is float on the desktop SM5/SM6 targets traced here.
pub fn water_ior(specular: f32) -> f32 {
    let f0 = 0.08 * specular;
    2.0 / (1.0 - f0.min(0.99).sqrt()) - 1.0
}

/// SingleLayerWaterCommon.ush WaterRefract.
pub fn water_refract(v: [f32; 3], n: [f32; 3], eta: f32) -> Option<[f32; 3]> {
    let cos_i = dot(v, n);
    let cos_t2 = 1.0 - eta * eta * (1.0 - cos_i * cos_i);
    if cos_t2 < 0.0 {
        return None;
    }
    let cos_t = -(if cos_i >= 0.0 { 1.0 } else { -1.0 }) * cos_t2.sqrt();
    Some(std::array::from_fn(|i| (eta * cos_i + cos_t) * n[i] - eta * v[i]))
}

pub fn isotropic_phase() -> f32 {
    1.0 / (4.0 * std::f32::consts::PI)
}
pub fn schlick_phase(g: f32, cos_theta: f32) -> f32 {
    let k = 1.55 * g - 0.55 * g * g * g;
    let factor = 1.0 + k * cos_theta;
    (1.0 - k * k) / (4.0 * std::f32::consts::PI * factor * factor)
}

/// GSystemTextures PreintegratedGF on desktop: 128x32 PF_G16R16 (x=NoV,
/// y=Roughness), 128 Hammersley samples, f32 accumulation, unorm16 rounding.
pub const GF_SIZE: [u32; 2] = [128, 32];
pub fn preintegrated_gf() -> Vec<[u16; 2]> {
    let [w, h] = GF_SIZE;
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let roughness = (y as f32 + 0.5) / h as f32;
        let m = roughness * roughness;
        let m2 = m * m;
        for x in 0..w {
            let n_dot_v = (x as f32 + 0.5) / w as f32;
            let v = [(1.0 - n_dot_v * n_dot_v).sqrt(), 0.0, n_dot_v];
            let (mut a, mut b) = (0.0f32, 0.0f32);
            const SAMPLES: u32 = 128;
            for i in 0..SAMPLES {
                let e1 = i as f32 / SAMPLES as f32;
                let e2 = (f64::from(i.reverse_bits()) / 4294967296.0) as f32;
                let phi = 2.0 * std::f32::consts::PI * e1;
                let cos_theta = ((1.0 - e2) / (1.0 + (m2 - 1.0) * e2)).sqrt();
                let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
                let hv = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];
                let vh = dot(v, hv);
                let l: [f32; 3] = std::array::from_fn(|k| 2.0 * vh * hv[k] - v[k]);
                let n_dot_l = l[2].max(0.0);
                let n_dot_h = hv[2].max(0.0);
                let v_dot_h = vh.max(0.0);
                if n_dot_l > 0.0 {
                    let smith_v = n_dot_l * (n_dot_v * (1.0 - m) + m);
                    let smith_l = n_dot_v * (n_dot_l * (1.0 - m) + m);
                    let vis = 0.5 / (smith_v + smith_l);
                    let weight = n_dot_l * vis * (4.0 * v_dot_h / n_dot_h);
                    let mut fc = 1.0 - v_dot_h;
                    fc *= (fc * fc) * (fc * fc);
                    a += weight * (1.0 - fc);
                    b += weight * fc;
                }
            }
            a /= SAMPLES as f32;
            b /= SAMPLES as f32;
            let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as i32 as u16;
            out.push([q(a), q(b)]);
        }
    }
    out
}

/// BRDF.ush EnvBRDF given the bilinear PreintegratedGF sample at (NoV, Roughness).
pub fn env_brdf(specular_color: [f32; 3], gf: [f32; 2]) -> [f32; 3] {
    let shadow = saturate(50.0 * specular_color[1]);
    specular_color.map(|s| s * gf[0] + shadow * gf[1])
}

/// SampleSceneDepthWithoutWaterBilateral after its gathers: scene depths and
/// packed refraction masks in Gather order (x,y,z,w = (0,1),(1,1),(1,0),(0,0)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bilateral {
    pub scene_depth: f32,
    pub weights: [f32; 4],
    pub refraction_mask: [f32; 4],
}
pub fn bilateral(uv: [f32; 2], size: [f32; 2], depth4: [f32; 4], mask4: [f32; 4], water_depth: f32, slope: f32) -> Bilateral {
    let inv = size.map(|s| 1.0 / s);
    let uv: [f32; 2] = std::array::from_fn(|i| uv[i] + (0.5 / 256.0) * inv[i]);
    let fr: [f32; 2] = std::array::from_fn(|i| {
        let v = uv[i] * size[i] - 0.5;
        v - v.floor()
    });
    let bilinear = [
        (1.0 - fr[0]) * fr[1],
        fr[0] * fr[1],
        fr[0] * (1.0 - fr[1]),
        (1.0 - fr[0]) * (1.0 - fr[1]),
    ];
    let is_water = |m: f32| m >= 128.0 / 255.0;
    let mut weights: [f32; 4] = std::array::from_fn(|i| {
        let bilateral = if depth4[i] + slope >= water_depth { 1.0 } else { 0.0 };
        let water = if is_water(mask4[i]) { 1.0 } else { 0.0 };
        bilinear[i] * (bilateral * water)
    });
    let sum = weights[0] + weights[1] + weights[2] + weights[3];
    let scale = if sum > 1e-5 { 1.0 / sum } else { 0.0 };
    weights = weights.map(|w| w * scale);
    Bilateral {
        scene_depth: dot4(depth4, weights),
        weights,
        refraction_mask: mask4.map(|m| if is_water(m) { (m - 128.0 / 255.0) / (127.0 / 255.0) } else { 0.0 }),
    }
}
pub fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// ComputeBufferUVDistortion (REFRACTION_USE_PIXEL_NORMAL_OFFSET, no clip),
/// PostProcessUVDistortion, then EvaluateWaterVolumeLighting's 30 cm
/// thickness fade. View-space normals; `view_to_buffer` is ViewSize *
/// BufferInvSize; `distortion` is SingleLayerWater.DistortionParams.
pub fn water_uv_distortion(
    view_normal: [f32; 3],
    view_vertex_normal: [f32; 3],
    refraction: [f32; 2],
    view_to_buffer: [f32; 2],
    distortion: [f32; 4],
    behind_depth: f32,
    surface_depth: f32,
) -> [f32; 2] {
    let strength = refraction[0] - 1.0;
    let fov = [distortion[0], distortion[1] * distortion[0]];
    let fudge = [0.00023, -0.00023];
    let bias = -refraction[1];
    let range = (bias * 0.5).abs().clamp(0.0, 50.0);
    let inv_width = 1.0 / range.max(1.0);
    let post = saturate((behind_depth - surface_depth) * inv_width + bias);
    std::array::from_fn(|i| {
        let uv = (view_vertex_normal[i] - view_normal[i]) * strength * view_to_buffer[i];
        let uv = uv * (distortion[2 + i] * fudge[i] * fov[i]);
        let uv = uv * post * 4.0;
        uv * saturate((behind_depth - surface_depth) * 1.0 / 30.0)
    })
}

/// Refracted sample choice: distorted unless all its weights are zero.
/// Returns (behind depth, colour weights, use distorted UV).
pub fn select_behind(pixel: Bilateral, distorted: Bilateral) -> (f32, [f32; 4], bool) {
    if distorted.weights == [0.0; 4] {
        let w = std::array::from_fn(|k| pixel.weights[k] * pixel.refraction_mask[k]);
        (pixel.scene_depth, w, false)
    } else {
        let w = std::array::from_fn(|k| distorted.weights[k] * distorted.refraction_mask[k]);
        (distorted.scene_depth, w, true)
    }
}

/// SampleSceneColorWithoutWaterBilateral from the three channel gathers.
pub fn behind_color(red4: [f32; 4], green4: [f32; 4], blue4: [f32; 4], weights: [f32; 4]) -> [f32; 3] {
    [dot4(red4, weights), dot4(green4, weights), dot4(blue4, weights)]
}

/// EvaluateWaterVolumeLighting after its scene samples (camera above water,
/// main directional light not separated).
#[derive(Clone, Copy, Debug)]
pub struct VolumeInputs {
    pub coefficients: Coefficients,
    pub water_surface_depth: f32,
    pub behind_depth: f32,
    /// Bilateral scene colour without water, before OneOverPreExposure.
    pub behind_luminance: [f32; 3],
    pub one_over_pre_exposure: f32,
    /// Translated-world Z of the water pixel and of the behind-water point.
    pub surface_z: f32,
    pub behind_z: f32,
    pub camera_vector: [f32; 3],
    pub normal: [f32; 3],
    pub ior: f32,
    pub light_direction: [f32; 3],
    pub sun_illuminance: [f32; 3],
    pub ambient_illuminance: [f32; 3],
    pub env_brdf: [f32; 3],
    pub directional_shadow: f32,
    pub light_function: [f32; 3],
    pub water_visibility: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeOutput {
    pub luminance: [f32; 3],
    pub water_to_scene: [f32; 3],
    pub water_to_scene_to_light: [f32; 3],
}
pub fn volume_lighting(i: &VolumeInputs) -> VolumeOutput {
    let mut out = VolumeOutput {
        luminance: [0.0; 3],
        water_to_scene: [1.0; 3],
        water_to_scene_to_light: [1.0; 3],
    };
    if !(i.water_visibility > 0.0) {
        return out;
    }
    let depth = (i.behind_depth - i.water_surface_depth).max(0.0);
    let c = &i.coefficients;
    let scattering = c.scattering.map(|v| v.max(0.0));
    let absorption = c.absorption.map(|v| v.max(0.0));
    let phase_g = c.phase_g.clamp(-1.0, 1.0);
    let scale_behind = c.color_scale_behind_water.map(|v| lerp(1.0, v.max(0.0), saturate(depth * 0.02)));
    let extinction: [f32; 3] = std::array::from_fn(|k| scattering[k] + absorption[k]);
    let extinction_safe = extinction.map(|e| e.max(1e-5));
    let mut phase = 0.0;
    if let Some(ray) = water_refract(i.camera_vector, i.normal, 1.0 / i.ior) {
        let l = i.light_direction.map(|v| -v);
        phase = schlick_phase(phase_g, dot(l, ray));
    }
    let to_top = (i.surface_z - i.behind_z).max(0.0);
    let mean_to_light = extinction.map(|e| (-to_top * e).exp());
    for k in 0..3 {
        let behind = i.behind_luminance[k] * (mean_to_light[k] * i.one_over_pre_exposure);
        let sun = phase * i.sun_illuminance[k];
        let amb = isotropic_phase() * i.ambient_illuminance[k];
        let transmittance = (-(extinction[k] * depth)).exp();
        let incoming = amb + sun * i.light_function[k] * i.directional_shadow * 1.0;
        let amount = saturate(scattering[k] * (1.0 - transmittance) / extinction_safe[k]);
        let scattered = incoming * amount * (1.0 - i.env_brdf[k]);
        out.luminance[k] = i.water_visibility * (scattered + transmittance * (behind * scale_behind[k]));
        out.water_to_scene[k] = transmittance;
        out.water_to_scene_to_light[k] = transmittance * mean_to_light[k];
    }
    out
}

/// ShadingModels.ush DefaultLitBxDF for the forward directional light: no
/// anisotropy, zero sphere/line extent and r.Shading.EnergyConservation 0 (the
/// GGX energy LUTs are a pending port). Returns (diffuse, specular) per unit
/// light colour; the caller multiplies by the light's illuminance and shadow.
pub fn direct(n: [f32; 3], v: [f32; 3], l: [f32; 3], diffuse_color: [f32; 3], specular_color: [f32; 3], roughness: f32) -> ([f32; 3], [f32; 3]) {
    let n_dot_l = saturate(dot(n, l));
    if !(n_dot_l > 0.0) {
        return ([0.0; 3], [0.0; 3]);
    }
    let h0: [f32; 3] = std::array::from_fn(|i| v[i] + l[i]);
    let hl = dot(h0, h0).sqrt();
    let h = h0.map(|c| c / hl);
    let (n_dot_h, v_dot_h) = (dot(n, h), dot(v, h));
    let n_dot_v = saturate(dot(n, v).abs() + 1e-5);
    let pi = std::f32::consts::PI;
    let diffuse = diffuse_color.map(|c| c * (1.0 / pi) * (1.0 * n_dot_l));
    let a2 = roughness * roughness * roughness * roughness;
    let d0 = (n_dot_h * a2 - n_dot_h) * n_dot_h + 1.0;
    let d = a2 / (pi * d0 * d0);
    let a = a2.sqrt();
    let vis = 0.5 / (n_dot_l * (n_dot_v * (1.0 - a) + a) + n_dot_v * (n_dot_l * (1.0 - a) + a));
    let fc = (1.0 - v_dot_h).powi(5);
    let shadow = saturate(50.0 * specular_color[1]);
    let specular = specular_color.map(|s| (1.0 * n_dot_l) * ((d * vis) * (shadow * fc + (1.0 - fc) * s)));
    (diffuse, specular)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficients_follow_resolved_graph() {
        let p = Parameters::default();
        let c = coefficients(&p, [0.2, 0.4, 0.6], [30.0, 40.0], 1.0);
        assert!((c.scattering[1] - (0.5 + 0.5 * 0.4) / 1000.0).abs() < 1e-9);
        assert!((c.absorption[1] - 1.0 / 25.0 / 8.0).abs() < 1e-9);
        assert_eq!(coefficients(&p, [1.0; 3], [0.0; 2], -1.0).scattering, [0.0; 3]);
        assert_eq!(c.phase_g, p.anisotropy);
    }

    #[test]
    fn underside_critical_angle_forces_reflection() {
        let p = Parameters::default();
        let front = surface(&p, 0.1, 1.0, 100.0);
        assert_eq!(front.opacity, 0.0);
        let back = surface(&p, 0.1, -1.0, 100.0);
        assert_eq!((back.specular, back.opacity), (1.0, 1.0));
        assert!((front.refraction[0] - lerp(1.15, 1.02, 0.02)).abs() < 1e-6);
        // Water Specular 0.225 -> F0 0.018 -> Burley IOR 1.309906 (water-like).
        assert!((water_ior(front.specular) - 1.309_906).abs() < 1e-4);
    }

    #[test]
    fn preintegrated_gf_is_energy_bounded_and_monotone_in_specular_term() {
        let gf = preintegrated_gf();
        assert_eq!(gf.len(), 128 * 32);
        for row in gf.chunks(128) {
            for t in row {
                assert!(u32::from(t[0]) + u32::from(t[1]) <= 65535 + 2);
            }
        }
        // Grazing smooth surfaces are Fresnel dominated.
        assert!(gf[0][1] > gf[127][1]);
    }

    #[test]
    fn bilateral_rejects_foreground_and_non_water_and_normalizes() {
        let b = bilateral([0.5, 0.5], [8.0, 8.0], [100.0, 50.0, 100.0, 100.0], [1.0; 4], 80.0, 0.0);
        assert_eq!(b.weights[1], 0.0);
        assert!((b.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        let none = bilateral([0.5, 0.5], [8.0, 8.0], [100.0; 4], [0.2; 4], 80.0, 0.0);
        assert_eq!(none.weights, [0.0; 4]);
    }

    #[test]
    fn volume_lighting_conserves_transmittance_and_scatters() {
        let p = Parameters::default();
        let mut i = VolumeInputs {
            coefficients: coefficients(&p, [0.0; 3], [0.0; 2], 1.0),
            water_surface_depth: 500.0,
            behind_depth: 700.0,
            behind_luminance: [1.0; 3],
            one_over_pre_exposure: 1.0,
            surface_z: 0.0,
            behind_z: -150.0,
            camera_vector: [0.0, 0.6, 0.8],
            normal: [0.0, 0.0, 1.0],
            ior: 1.33,
            light_direction: [0.0, 0.0, -1.0],
            sun_illuminance: [10.0; 3],
            ambient_illuminance: [1.0; 3],
            env_brdf: [0.02; 3],
            directional_shadow: 1.0,
            light_function: [1.0; 3],
            water_visibility: 1.0,
        };
        let o = volume_lighting(&i);
        for k in 0..3 {
            assert!(o.water_to_scene[k] < 1.0 && o.water_to_scene[k] > 0.0);
            assert!(o.water_to_scene_to_light[k] <= o.water_to_scene[k]);
            assert!(o.luminance[k] > 0.0);
        }
        i.water_visibility = 0.0;
        assert_eq!(volume_lighting(&i).water_to_scene, [1.0; 3]);
    }
}
