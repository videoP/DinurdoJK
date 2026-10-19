//! Baked emote particle effects. Tracks are recorded in the emote's model
//! space relative to `model_root`, exactly like prop attachment tracks, so
//! they follow the selected model's pose, ground correction and yaw.
use super::{transform_jka_model_point, transform_jka_model_vector, CosmeticDraw, Matrix3x4, PlayerModelAsset, PlayerPresenter};
use crate::fx::system::{FxDraw, FxLight, FxLightKind};
use jka_emotes::effects::{Align, Facing, Kind, Particle};

/// Per-frame draw budget for one emote's particles.
const MAX_PARTICLES: usize = 2500;

#[derive(Default)]
pub struct EmoteEffectFrame {
    pub draws: Vec<FxDraw>,
    pub meshes: Vec<CosmeticDraw>,
    pub lights: Vec<FxLight>,
}

impl PlayerPresenter {
    pub(super) fn emote_effect_frame(active: &crate::emotes::Preview, model: &PlayerModelAsset, pose: &[Matrix3x4], axis: [[f32; 3]; 3], origin: [f32; 3], entity_num: u16) -> EmoteEffectFrame {
        let mut out = EmoteEffectFrame::default();
        let emote = &active.emote;
        let Some(effects) = emote.effects.as_ref() else { return out };
        let Some(track) = effects.data.as_ref() else { return out };
        if !crate::emotes::effects_enabled() {
            return out;
        }
        let frames = (emote.end_frame - emote.first_frame).max(1) as usize;
        let loop_start = emote.loop_start_frame.map(|n| (n - emote.first_frame).max(0) as usize).or(emote.looping.then_some(0));
        let seconds = jka_emotes::presentation::clock(active.elapsed_ms, emote.fps, frames, loop_start);
        let track_loop = loop_start.map(|l| (l as f32 / emote.fps * track.fps).round() as usize);
        let mut particles = Vec::new();
        track.sample(seconds, track_loop, &mut particles);
        if particles.is_empty() {
            return out;
        }
        // model_root skin + bind translation, as for root-attached props.
        let root = model.gla.skeleton.iter().position(|b| b.name.eq_ignore_ascii_case("model_root"));
        let skin = root.and_then(|i| pose.get(i)).copied().unwrap_or([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        let bind = root.map_or([0.; 3], |i| {
            let b = &model.gla.skeleton[i].base_pose;
            [b[0][3], b[1][3], b[2][3]]
        });
        let point = |p: [f32; 3]| {
            let q = [p[0] + bind[0], p[1] + bind[1], p[2] + bind[2]];
            let m = std::array::from_fn(|r| skin[r][0] * q[0] + skin[r][1] * q[1] + skin[r][2] * q[2] + skin[r][3]);
            transform_jka_model_point(m, axis, origin)
        };
        let vector = |v: [f32; 3]| {
            let m = std::array::from_fn(|r| skin[r][0] * v[0] + skin[r][1] * v[1] + skin[r][2] * v[2]);
            transform_jka_model_vector(m, axis)
        };
        let unit = {
            let v = vector([1., 0., 0.]);
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-4)
        };
        // Player models stand on the floor at origin z + DEFAULT_MINS_2 (-24).
        let ground = origin[2] - 24.0 * (axis[2][0] * axis[2][0] + axis[2][1] * axis[2][1] + axis[2][2] * axis[2][2]).sqrt();
        let mut ribbons: Vec<(u8, u16, f32, [f32; 3], f32, [u8; 4])> = Vec::new();
        for p in particles.iter().take(MAX_PARTICLES) {
            let Some(renderer) = effects.renderers.get(p.renderer as usize) else { continue };
            let world = point(p.position);
            if !world.iter().all(|n| n.is_finite()) {
                continue;
            }
            match renderer.kind {
                Kind::Sprite => out.draws.push(sprite(p, renderer, world, vector(p.direction), vector(p.normal), unit, ground)),
                Kind::Ribbon => ribbons.push((p.renderer, p.ribbon, p.link, world, p.size[0] * unit * 0.5, p.rgba)),
                Kind::Mesh => {
                    let Some(model_path) = renderer.models.get(p.mesh as usize).or(renderer.models.first()) else { continue };
                    let [x, y, z, w] = p.orientation;
                    // Quaternion columns in model space, then model -> world.
                    let cols = [
                        [1. - 2. * (y * y + z * z), 2. * (x * y + w * z), 2. * (x * z - w * y)],
                        [2. * (x * y - w * z), 1. - 2. * (x * x + z * z), 2. * (y * z + w * x)],
                        [2. * (x * z + w * y), 2. * (y * z - w * x), 1. - 2. * (x * x + y * y)],
                    ];
                    let axis = std::array::from_fn(|c| {
                        let v = vector(cols[c]);
                        [v[0] * p.scale[c], v[1] * p.scale[c], v[2] * p.scale[c]]
                    });
                    // Override material from the Niagara mesh renderer, if any.
                    let custom_shader = (!renderer.shader.is_empty()).then(|| intern(&renderer.shader));
                    let face_camera = (renderer.facing == Facing::Camera).then_some(renderer.align == Align::Custom);
                    out.meshes.push(CosmeticDraw { entity_num, model: model_path.clone(), origin: world, axis, rgba: p.rgba.map(|c| c as f32 / 255.), custom_shader, skin_pose: None, face_camera });
                }
                Kind::Light => {
                    let a = p.rgba[3] as f32 / 255.;
                    out.lights.push(FxLight {
                        kind: FxLightKind::AuthoredEffect,
                        origin: world,
                        radius: (p.radius * unit).clamp(8., 600.),
                        source_radius: 0.,
                        rgb: [0, 1, 2].map(|i| p.rgba[i] as f32 / 255. * a),
                        spot: None,
                        segment: None,
                        blade_segments: None,
                    });
                }
            }
        }
        // Ribbons: connect particles of one ribbon in link order.
        ribbons.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
        for pair in ribbons.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.0 != b.0 || a.1 != b.1 {
                continue;
            }
            let shader = effects.renderers[a.0 as usize].shader.clone();
            let rgba = std::array::from_fn(|i| ((a.5[i] as u16 + b.5[i] as u16) / 2) as u8);
            out.draws.push(FxDraw::Line { start: a.3, end: b.3, width: (a.4 + b.4) * 0.5, rgba, shader });
        }
        if out.lights.len() > 4 {
            out.lights.sort_by(|a, b| b.radius.total_cmp(&a.radius));
            out.lights.truncate(4);
        }
        out
    }
}

/// Effect shader names live for the whole session; the cosmetic draw path
/// takes `&'static str`, so each distinct name is leaked exactly once.
fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static str>>> = std::sync::OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = names.get(name) {
        return found;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(name.to_owned(), leaked);
    leaked
}

fn sprite(p: &Particle, r: &jka_emotes::effects::Renderer, origin: [f32; 3], direction: [f32; 3], normal: [f32; 3], unit: f32, ground: f32) -> FxDraw {
    let cols = r.sub_image[0].max(1) as u32;
    let rows = r.sub_image[1].max(1) as u32;
    let index = p.sub_image as u32 % (cols * rows);
    let (u, v) = ((index % cols) as f32 / cols as f32, (index / cols) as f32 / rows as f32);
    let nonzero = |d: [f32; 3]| d.iter().any(|c| c.abs() > 1e-3);
    let align = (r.align != Align::None).then_some(direction).filter(|d| nonzero(*d));
    let facing = (r.facing == Facing::Custom).then_some(normal).filter(|d| nonzero(*d));
    FxDraw::Billboard {
        origin,
        half: [p.size[0] * unit * 0.5, p.size[1] * unit * 0.5],
        rotation: p.rotation,
        uv: [u, v, u + 1. / cols as f32, v + 1. / rows as f32],
        rgba: p.rgba,
        align,
        facing,
        // Flat custom-facing decals (ripples, rings) lie on the floor by design.
        ground: facing.is_none().then_some(ground),
        pivot: r.pivot,
        shader: r.shader.clone(),
    }
}
