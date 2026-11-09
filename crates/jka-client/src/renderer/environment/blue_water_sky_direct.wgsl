// Direct BSP sky: original Blue Water dome radiance and celestial atmosphere.
// The only changes from vendor/blue-water-sea-and-sky/sky/shaders/dome.wgsl
// are the BSP sky vertex mask and per-fragment view ray reconstructed from
// screen position. Native clouds continue using the original offscreen path.
#import naval_sky::common::{sky, air, PI}
#import naval_sky::atmosphere::{dome_sky, transmittance_to_space, aerosol_phase}
#import naval_sky::bruneton::rayleigh_phase_function
#import naval_sky::celestial::{celestial_radiance}
#import naval_sky::aerial::aerial_over_sky

struct HostCamera { view_projection: mat4x4<f32> }
@group(0) @binding(0) var<uniform> host: HostCamera;

@vertex fn sky_mask(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    var clip = host.view_projection * vec4<f32>(position, 1.0);
    clip.z = 0.0;
    return clip;
}

fn dome_radiance(direction: vec3<f32>) -> vec3<f32> {
    return dome_sky(direction, false, 0.0) + transmittance_to_space(direction, false) * celestial_radiance(direction);
}

const FLASH_TINT: vec3<f32> = vec3<f32>(0.8, 0.87, 1.0);
const FLASH_CORE: f32 = 60.0;
const RAYLEIGH_HEIGHT_M: f32 = 8000.0;
const MIE_HEIGHT_M: f32 = 1200.0;
fn flash_airlight(direction: vec3<f32>) -> vec3<f32> {
    let to_flash = sky.lightning_position - sky.camera_position;
    let reach = max(length(to_flash), 1.0);
    let along = dot(to_flash, direction);
    let offset = max(length(to_flash - direction * along), FLASH_CORE);
    let path = (0.5 * PI + atan(along / offset)) / offset;
    let nu = dot(to_flash / reach, direction);
    let height = max(sky.camera_position.y + direction.y * max(along, 0.0), 0.0);
    let scattering = air.rayleigh_scattering * (exp(-height / RAYLEIGH_HEIGHT_M) * rayleigh_phase_function(nu))
        + air.mie_scattering * (exp(-height / MIE_HEIGHT_M) * aerosol_phase(nu));
    let depth = (air.rayleigh_scattering + air.mie_extinction + air.absorption_extinction) * (reach * 0.001);
    return FLASH_TINT * (sky.lightning_intensity * 1e6 * path * 0.001) * scattering * exp(-depth);
}

@fragment fn sky_color(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    // Same corner_ray inverse-projection used by the upstream full-screen
    // triangle; reconstruct it on the BSP mask without an image texture.
    let clip = vec2<f32>(pixel.x * sky.viewport.z * 2.0 - 1.0,
                         1.0 - pixel.y * sky.viewport.w * 2.0);
    let point = sky.inverse_projection * vec4<f32>(clip, 0.5, 1.0);
    let direction = normalize((sky.view_to_world * vec4<f32>(point.xyz / point.w, 0.0)).xyz);
    var radiance = aerial_over_sky(dome_radiance(direction), pixel.xy * sky.viewport.zw);
    if sky.lightning_intensity > 0.0 {
        radiance += flash_airlight(direction);
    }
#if SKY_SDR == true
    // Radiance is linear HDR. When r_hdr=0 the scene is 8-bit: map only
    // this sky to display range rather than forcing *all* world pixels FP16.
    // This necessarily compresses highlights vs the original HDR pipeline.
    radiance = max(radiance, vec3<f32>(0.0));
    radiance = radiance / (vec3<f32>(1.0) + radiance);
#endif
    return vec4<f32>(radiance, 1.0);
}
