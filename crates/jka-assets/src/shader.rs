//! Native interpretation of common JKA/Q3 shader material declarations.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RgbGen {
    #[default]
    Identity,
    Vertex,
    ExactVertex,
    OneMinusVertex,
    Const,
}

impl RgbGen {
    pub fn uses_vertex_color(self) -> bool {
        matches!(self, Self::Vertex | Self::ExactVertex | Self::OneMinusVertex)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlphaGen {
    #[default]
    Identity,
    Const,
    Vertex,
    OneMinusVertex,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum TcGen {
    #[default]
    Base,
    Lightmap,
    Vector([f32; 3], [f32; 3]),
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TcMod {
    Scroll(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    Transform([f32; 6]),
    Turb {
        base: f32,
        amplitude: f32,
        phase: f32,
        frequency: f32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SurfaceSpriteType {
    #[default]
    Vertical,
    Oriented,
    Effect,
    Flattened,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SurfaceSpriteFacing {
    #[default]
    Normal,
    HangDown,
    AnyAngle,
    FaceUp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSprite {
    pub kind: SurfaceSpriteType,
    pub width: f32,
    pub height: f32,
    pub density: f32,
    pub fade_dist: f32,
    pub fade_max: f32,
    pub fade_scale: f32,
    pub variance: [f32; 2],
    pub wind: f32,
    pub wind_idle: f32,
    pub vert_skew: f32,
    /// JKA effect-sprite lifetime in milliseconds.
    pub fx_duration: f32,
    /// Fractional width/height growth applied over one effect lifetime.
    pub fx_grow: [f32; 2],
    pub fx_alpha_start: f32,
    pub fx_alpha_end: f32,
    pub facing: SurfaceSpriteFacing,
}

impl SurfaceSprite {
    fn new(kind: SurfaceSpriteType, width: f32, height: f32, density: f32, fade_dist: f32) -> Self {
        Self {
            kind,
            width,
            height,
            density,
            fade_dist,
            fade_max: fade_dist * 1.33,
            fade_scale: 0.0,
            variance: [0.0; 2],
            wind: 0.0,
            wind_idle: 0.0,
            vert_skew: 0.0,
            fx_duration: 1000.0,
            fx_grow: [0.0; 2],
            fx_alpha_start: 1.0,
            fx_alpha_end: 0.0,
            facing: SurfaceSpriteFacing::Normal,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Stage {
    pub image: String,
    pub clamp: bool,
    /// Rend2 material companion maps authored directly on this diffuse stage.
    pub normal_map: Option<String>,
    pub normal_height_map: Option<String>,
    /// Optional Rend2 normalScale X/Y authored on this stage.
    pub normal_scale: Option<[f32; 2]>,
    pub rmo_map: Option<String>,
    /// rmosMap uses the RMO alpha channel as a per-pixel specular scale.
    pub rmo_specular_alpha: bool,
    pub specular_map: Option<String>,
    /// Fixed Rend2 roughness, including the inverse of an authored `gloss`.
    pub roughness: Option<f32>,
    /// Fixed linear-space dielectric reflectance from specularReflectance/specularScale.
    pub specular_reflectance: Option<[f32; 3]>,
    pub parallax_depth: Option<f32>,
    pub blend: String,
    pub alpha_test: String,
    pub alpha: Option<f32>,
    pub color: Option<[f32; 3]>,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
    pub tc_gen: TcGen,
    pub tc_mods: Vec<TcMod>,
    pub depth_write: bool,
    pub depth_equal: bool,
    /// OpenJK/Rend2 stage marker used by the glow/bloom path.
    pub glow: bool,
    /// Jedi Academy `surfaceSprites` metadata. This describes a procedural
    /// surface effect; it is not an ordinary material pass.
    pub surface_sprite: Option<SurfaceSprite>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Shader {
    pub cull: String,
    /// Classic id Tech 3 portal/mirror surface. The renderer can use this as
    /// an authored planar-reflection mask instead of guessing from gloss.
    pub portal: bool,
    pub sky: bool,
    pub sky_box: Option<String>,
    pub nodraw: bool,
    pub translucent: bool,
    /// q3map2 alpha-shadow metadata; retained separately from visible blending.
    pub alpha_shadow: bool,
    /// q3map2 light-filter metadata for transmissive lighting materials.
    pub light_filter: bool,
    /// q3map2 compile-time exclusion from fog volumes/global fog. The BSP
    /// normally bakes this into dsurface.fogNum; retaining the directive lets
    /// the runtime safely repair malformed/legacy global-fog assignments.
    pub no_fog: bool,
    /// Authored `surfaceparm water`; retained separately from generic translucency.
    pub water: bool,
    /// q3map gameplay/content metadata needed when previewing an uncompiled .map.
    /// These are deliberately separate from visible translucency: `trans` alone
    /// does not make a brush non-solid.
    pub nonsolid: bool,
    pub player_clip: bool,
    pub monster_clip: bool,
    pub bot_clip: bool,
    pub shot_clip: bool,
    pub trigger: bool,
    pub lava: bool,
    pub slime: bool,
    pub fog: bool,
    pub ladder: bool,
    pub slick: bool,
    /// q3map2/Jedi Academy content/surfaceparm deltas used by direct `.map`
    /// collision. A source face begins with JA's default SOLID|OPAQUE and these
    /// masks reproduce the compiler's surfaceParm table without relying on the
    /// shader name.
    pub collision_contents_add: u32,
    pub collision_contents_clear: u32,
    pub collision_surface_flags_add: u32,
    pub collision_surface_flags_clear: u32,
    pub polygon_offset: bool,
    pub fogparms: Option<FogParameters>,
    /// q3map/q3map2 authored area-emitter intensity. Zero means this shader
    /// is not a surface light.
    pub surface_light: f32,
    /// Optional image used by q3map to derive the emitter chromaticity.
    pub light_image: Option<String>,
    /// Optional q3map area-light subdivision size in map units.
    pub light_subdivide: Option<f32>,
    /// Directional suns authored by q3map/q3map2 sky shaders. `sun` is the
    /// legacy JKA alias for `q3map_sun`; q3map_sunExt carries the same first
    /// six parameters plus penumbra controls.
    pub suns: Vec<Sun>,
    pub stages: Vec<Stage>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogParameters {
    pub color: [f32; 3],
    pub depth_for_opaque: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sun {
    /// q3map normalizes the RGB vector before applying intensity.
    pub color: [f32; 3],
    pub intensity: f32,
    /// Map-space azimuth in degrees: 0 east, 90 north.
    pub azimuth: f32,
    /// Degrees above the map-space horizon: 0 horizon, 90 overhead.
    pub elevation: f32,
    /// q3map_sunExt penumbra width in degrees. Zero for legacy sun directives.
    pub deviance: f32,
    /// q3map_sunExt jitter sample count. Zero for legacy sun directives.
    pub samples: u32,
}

// Jedi Academy inherits the SOF2/RBSP q3map2 surfaceParm table. Keep the
// compiler semantics here because a custom shader name can carry gameplay
// contents which cannot be recovered from filename heuristics alone.
fn apply_ja_collision_surfaceparm(shader: &mut Shader, parm: &str) {
    const SOLID: u32 = 0x0000_0001;
    const LAVA: u32 = 0x0000_0002;
    const WATER: u32 = 0x0000_0004;
    const FOG: u32 = 0x0000_0008;
    const PLAYERCLIP: u32 = 0x0000_0010;
    const MONSTERCLIP: u32 = 0x0000_0020;
    const BOTCLIP: u32 = 0x0000_0040;
    const SHOTCLIP: u32 = 0x0000_0080;
    const TRIGGER: u32 = 0x0000_0400;
    const NODROP: u32 = 0x0000_0800;
    const TERRAIN: u32 = 0x0000_1000;
    const LADDER: u32 = 0x0000_2000;
    const ABSEIL: u32 = 0x0000_4000;
    const OPAQUE: u32 = 0x0000_8000;
    const OUTSIDE: u32 = 0x0001_0000;
    const SLIME: u32 = 0x0002_0000;
    const DETAIL: u32 = 0x0800_0000;
    const INSIDE: u32 = 0x1000_0000;
    const TRANSLUCENT: u32 = 0x8000_0000;

    const SURF_SKY: u32 = 0x0000_2000;
    const SURF_SLICK: u32 = 0x0000_4000;
    const SURF_METALSTEPS: u32 = 0x0000_8000;
    const SURF_FORCEFIELD: u32 = 0x0001_0000;
    const SURF_NODAMAGE: u32 = 0x0004_0000;
    const SURF_NOIMPACT: u32 = 0x0008_0000;
    const SURF_NOMARKS: u32 = 0x0010_0000;
    const SURF_NODRAW: u32 = 0x0020_0000;
    const SURF_NOSTEPS: u32 = 0x0040_0000;
    const SURF_NODLIGHT: u32 = 0x0080_0000;
    const SURF_NOMISCENTS: u32 = 0x0100_0000;
    const SURF_FORCESIGHT: u32 = 0x0200_0000;

    let mut add_contents = 0u32;
    let mut clear_contents = 0u32;
    let mut add_surface = 0u32;
    let clear_surface = 0u32;
    match parm {
        "origin" => clear_contents |= SOLID,
        "areaportal" => {
            add_contents |= TRANSLUCENT;
            clear_contents |= SOLID | OPAQUE;
        }
        "trans" => add_contents |= TRANSLUCENT,
        "detail" => add_contents |= DETAIL,
        "nodraw" => add_surface |= SURF_NODRAW,
        "nonsolid" => clear_contents |= SOLID,
        "nonopaque" => clear_contents |= OPAQUE,
        "trigger" => {
            add_contents |= TRIGGER;
            clear_contents |= SOLID | OPAQUE;
        }
        "water" => {
            add_contents |= WATER;
            clear_contents |= SOLID;
        }
        "slime" => {
            add_contents |= SLIME;
            clear_contents |= SOLID;
        }
        "lava" => {
            add_contents |= LAVA;
            clear_contents |= SOLID;
        }
        "shotclip" | "weaponclip" => {
            add_contents |= SHOTCLIP;
            clear_contents |= SOLID | OPAQUE;
        }
        "playerclip" => {
            add_contents |= PLAYERCLIP;
            clear_contents |= SOLID | OPAQUE;
        }
        "monsterclip" => {
            add_contents |= MONSTERCLIP;
            clear_contents |= SOLID | OPAQUE;
        }
        "botclip" => {
            add_contents |= BOTCLIP;
            clear_contents |= SOLID | OPAQUE;
        }
        "nodrop" => {
            add_contents |= NODROP;
            clear_contents |= SOLID | OPAQUE;
        }
        "terrain" => {
            add_contents |= TERRAIN;
            clear_contents |= SOLID | OPAQUE;
        }
        "ladder" => {
            add_contents |= LADDER;
            clear_contents |= SOLID | OPAQUE;
        }
        "abseil" => {
            add_contents |= ABSEIL;
            clear_contents |= SOLID | OPAQUE;
        }
        "outside" => {
            add_contents |= OUTSIDE;
            clear_contents |= SOLID | OPAQUE;
        }
        "fog" => {
            add_contents |= FOG;
            clear_contents |= SOLID | OPAQUE;
        }
        "inside" => add_contents |= INSIDE,
        "sky" => add_surface |= SURF_SKY,
        "slick" => add_surface |= SURF_SLICK,
        "metalsteps" => add_surface |= SURF_METALSTEPS,
        "forcefield" => add_surface |= SURF_FORCEFIELD,
        "nodamage" => add_surface |= SURF_NODAMAGE,
        "noimpact" => add_surface |= SURF_NOIMPACT,
        "nomarks" => add_surface |= SURF_NOMARKS,
        "nosteps" => add_surface |= SURF_NOSTEPS,
        "nodlight" => add_surface |= SURF_NODLIGHT,
        "nomiscents" => add_surface |= SURF_NOMISCENTS,
        "forcesight" => add_surface |= SURF_FORCESIGHT,
        _ => {}
    }
    shader.collision_contents_add |= add_contents;
    shader.collision_contents_clear |= clear_contents;
    shader.collision_surface_flags_add |= add_surface;
    shader.collision_surface_flags_clear |= clear_surface;
}

impl Shader {
    pub fn primary(&self) -> Option<&Stage> {
        self.stages
            .iter()
            .find(|s| {
                !s.image.is_empty() && (!s.image.starts_with('$') || s.image == "$whiteimage")
            })
            .or_else(|| self.stages.iter().find(|s| s.image == "$lightmap"))
    }
}

fn numbers(args: &[String]) -> Result<Vec<f32>, String> {
    args.iter()
        .flat_map(|a| a.split(['(', ')', ',']))
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.parse::<f32>()
                .map_err(|_| format!("invalid shader number {a}"))
        })
        .collect()
}

fn vec3(args: &[String]) -> Result<[f32; 3], String> {
    let v = numbers(args)?;
    if v.len() != 3 || v.iter().any(|n| !n.is_finite()) {
        return Err("expected finite vec3".into());
    }
    Ok([v[0], v[1], v[2]])
}

fn parse_sun(args: &[String], extended: bool) -> Result<Sun, String> {
    let values = numbers(args)?;
    let required = if extended { 8 } else { 6 };
    if values.len() < required || values.iter().any(|value| !value.is_finite()) {
        return Err(if extended {
            "q3map_sunExt expects RGB intensity azimuth elevation deviance samples".into()
        } else {
            "sun/q3map_sun expects RGB intensity azimuth elevation".into()
        });
    }
    let length = (values[0] * values[0] + values[1] * values[1] + values[2] * values[2]).sqrt();
    if length <= 1e-6 {
        return Err("sun color must be non-zero".into());
    }
    Ok(Sun {
        color: [values[0] / length, values[1] / length, values[2] / length],
        intensity: values[3].max(0.0),
        azimuth: values[4],
        elevation: values[5],
        deviance: if extended { values[6].max(0.0) } else { 0.0 },
        samples: if extended {
            values[7].max(0.0).round() as u32
        } else {
            0
        },
    })
}

fn parse_surface_sprite(args: &[String]) -> Result<SurfaceSprite, String> {
    if args.len() < 5 {
        return Err("surfaceSprites expects type width height density fadedist".into());
    }
    let kind = match args[0].to_ascii_lowercase().as_str() {
        "vertical" => SurfaceSpriteType::Vertical,
        "oriented" => SurfaceSpriteType::Oriented,
        "effect" => SurfaceSpriteType::Effect,
        // Raven's later shader sets and community content also use flattened.
        "flattened" => SurfaceSpriteType::Flattened,
        value => return Err(format!("unsupported surfaceSprites type {value}")),
    };
    let values = numbers(&args[1..5])?;
    let [width, height, density, fade_dist]: [f32; 4] = values
        .try_into()
        .map_err(|_| "surfaceSprites expects four numeric parameters".to_string())?;
    if ![width, height, density, fade_dist]
        .iter()
        .all(|value| value.is_finite())
        || width <= 0.0
        || height <= 0.0
        || density <= 0.0
        || fade_dist < 32.0
    {
        return Err("invalid surfaceSprites dimensions/density/fadedist".into());
    }
    Ok(SurfaceSprite::new(kind, width, height, density, fade_dist))
}

fn surface_sprite_value(args: &[String], name: &str) -> Result<f32, String> {
    args.first()
        .ok_or_else(|| format!("{name} expects one numeric parameter"))?
        .parse::<f32>()
        .map_err(|_| format!("invalid {name} value"))
        .and_then(|value| {
            value
                .is_finite()
                .then_some(value)
                .ok_or_else(|| format!("invalid {name} value"))
        })
}

/// Quake shader statements are line-oriented; braces delimit shaders and stages.
pub fn parse(text: &str) -> Result<BTreeMap<String, Shader>, String> {
    let tokens = tokenize(text)?;
    let mut cursor = 0;
    let mut result = BTreeMap::new();
    while cursor < tokens.len() {
        if tokens[cursor] == "\n" {
            cursor += 1;
            continue;
        }
        let name = tokens[cursor].to_ascii_lowercase();
        cursor += 1;
        while tokens.get(cursor).is_some_and(|t| t == "\n") {
            cursor += 1;
        }
        if tokens.get(cursor).is_none_or(|t| t != "{") {
            return Err(format!("expected shader block after {name}"));
        }
        cursor += 1;
        let mut shader = Shader::default();
        let mut stage: Option<Stage> = None;
        let mut closed = false;
        while cursor < tokens.len() {
            match tokens[cursor].as_str() {
                "\n" => {
                    cursor += 1;
                    continue;
                }
                "{" => {
                    if stage.is_some() {
                        return Err("nested shader stage".into());
                    }
                    stage = Some(Stage::default());
                    cursor += 1;
                    continue;
                }
                "}" => {
                    cursor += 1;
                    if let Some(stage) = stage.take() {
                        shader.stages.push(stage);
                        continue;
                    }
                    closed = true;
                    break;
                }
                _ => (),
            }
            let key = tokens[cursor].to_ascii_lowercase();
            cursor += 1;
            let start = cursor;
            while cursor < tokens.len() && !["\n", "{", "}"].contains(&tokens[cursor].as_str()) {
                cursor += 1;
            }
            let args = &tokens[start..cursor];
            let first = args
                .first()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if let Some(stage) = &mut stage {
                match key.as_str() {
                    "map" | "clampmap" => {
                        stage.image = first;
                        stage.clamp = key == "clampmap";
                    }
                    "animmap" | "oneshotanimmap" => {
                        stage.image = args.get(1).cloned().unwrap_or_default();
                        stage.unsupported.push(format!("{key} (first frame only)"));
                    }
                    "normalmap" => stage.normal_map = args.first().cloned(),
                    "normalheightmap" => stage.normal_height_map = args.first().cloned(),
                    "normalscale" => {
                        let values = numbers(args)?;
                        if let Some(&x) = values.first() {
                            let y = values.get(1).copied().unwrap_or(x);
                            if x.is_finite() && y.is_finite() {
                                stage.normal_scale = Some([x, y]);
                            }
                            // Rend2 allows a third normalScale component to set
                            // parallax height. Because parsing is ordered, a later
                            // parallaxDepth still correctly overrides it.
                            if let Some(&height) = values.get(2) {
                                if height.is_finite() {
                                    stage.parallax_depth = Some(height.max(0.0));
                                }
                            }
                        }
                    }
                    "rmomap" | "rmosmap" => {
                        stage.rmo_map = args.first().cloned();
                        stage.rmo_specular_alpha = key == "rmosmap";
                    }
                    "specmap" | "specularmap" => stage.specular_map = args.first().cloned(),
                    "gloss" => {
                        stage.roughness = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                            .map(|value| 1.0 - value.clamp(0.0, 1.0));
                    }
                    "roughness" => {
                        stage.roughness = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                            .map(|value| value.clamp(0.0, 1.0));
                    }
                    "specularreflectance" => {
                        stage.specular_reflectance = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                            .map(|value| [value.clamp(0.0, 1.0); 3]);
                    }
                    "specularscale" => {
                        let values = numbers(args)?;
                        match values.as_slice() {
                            [r, g, b] if r.is_finite() && g.is_finite() && b.is_finite() => {
                                stage.specular_reflectance = Some([
                                    r.clamp(0.0, 1.0),
                                    g.clamp(0.0, 1.0),
                                    b.clamp(0.0, 1.0),
                                ]);
                            }
                            [r, g, b, gloss]
                                if r.is_finite() && g.is_finite() && b.is_finite() && gloss.is_finite() =>
                            {
                                stage.specular_reflectance = Some([
                                    r.clamp(0.0, 1.0),
                                    g.clamp(0.0, 1.0),
                                    b.clamp(0.0, 1.0),
                                ]);
                                stage.roughness = Some(1.0 - gloss.clamp(0.0, 1.0));
                            }
                            _ => stage.unsupported.push("specularScale (invalid arguments)".into()),
                        }
                    }
                    "parallaxdepth" => {
                        stage.parallax_depth = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                            .map(|value| value.max(0.0));
                    }
                    "blendfunc" => stage.blend = args.join(" ").to_ascii_lowercase(),
                    "alphafunc" => stage.alpha_test = first,
                    "alphagen" if first == "const" => {
                        stage.alpha = args
                            .get(1)
                            .and_then(|a| a.parse::<f32>().ok())
                            .filter(|a| a.is_finite())
                            .map(|a| a.clamp(0.0, 1.0));
                        if stage.alpha.is_some() {
                            stage.alpha_gen = AlphaGen::Const;
                        } else {
                            stage
                                .unsupported
                                .push("alphagen const (invalid argument)".into());
                        }
                    }
                    "alphagen" if first == "vertex" => stage.alpha_gen = AlphaGen::Vertex,
                    "alphagen" if first == "oneminusvertex" => {
                        stage.alpha_gen = AlphaGen::OneMinusVertex
                    }
                    "alphagen" if first == "identity" => stage.alpha_gen = AlphaGen::Identity,
                    "alphagen" => stage
                        .unsupported
                        .push(format!("alphagen {}", args.join(" "))),
                    "rgbgen" if first == "vertex" || first == "vertexlit" => {
                        // Preserve Rend2's vertex-color modulation semantics here. BSP
                        // LIGHTMAP_BY_VERTEX handling is applied separately by scene prep.
                        stage.rgb_gen = RgbGen::Vertex
                    }
                    "rgbgen" if first == "exactvertex" || first == "exactvertexlit" => {
                        // exactVertexLit is the Rend2 counterpart of exactVertex.
                        stage.rgb_gen = RgbGen::ExactVertex
                    }
                    "rgbgen" if first == "oneminusvertex" => {
                        stage.rgb_gen = RgbGen::OneMinusVertex
                    }
                    "rgbgen" if first == "const" => {
                        let v = vec3(&args[1..])?;
                        stage.color = Some(v.map(|n| n.clamp(0.0, 1.0)));
                        stage.rgb_gen = RgbGen::Const;
                    }
                    "rgbgen"
                        if ["identity", "identitylighting", "lightingdiffuse"]
                            .contains(&first.as_str()) =>
                    {
                        // identityLighting and lightingDiffuse are intentionally
                        // approximated as identity by this renderer today. Keep
                        // the generation mode explicit so RGB and alpha sources
                        // are never accidentally coupled.
                        stage.rgb_gen = RgbGen::Identity;
                    }
                    "rgbgen" => stage
                        .unsupported
                        .push(format!("rgbgen {}", args.join(" "))),
                    "tcgen" | "texgen" => match first.as_str() {
                        "base" | "texture" => stage.tc_gen = TcGen::Base,
                        "lightmap" => stage.tc_gen = TcGen::Lightmap,
                        "environment" | "environmentmapped" => stage.tc_gen = TcGen::Environment,
                        "vector" => {
                            let v = numbers(&args[1..])?;
                            if v.len() != 6 || v.iter().any(|n| !n.is_finite()) {
                                return Err("invalid tcGen vector".into());
                            }
                            stage.tc_gen = TcGen::Vector([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
                        }
                        _ => stage.unsupported.push(format!("tcgen {first}")),
                    },
                    "tcmod" => {
                        let modifier = match first.as_str() {
                            "scroll" => {
                                let v = numbers(&args[1..])?;
                                (v.len() == 2).then(|| TcMod::Scroll(v[0], v[1]))
                            }
                            "scale" => {
                                let v = numbers(&args[1..])?;
                                (v.len() == 2).then(|| TcMod::Scale(v[0], v[1]))
                            }
                            "rotate" => {
                                let v = numbers(&args[1..])?;
                                (v.len() == 1).then(|| TcMod::Rotate(v[0]))
                            }
                            "transform" => {
                                let v = numbers(&args[1..])?;
                                (v.len() == 6)
                                    .then(|| TcMod::Transform([v[0], v[1], v[2], v[3], v[4], v[5]]))
                            }
                            "turb" => {
                                let v = numbers(&args[1..])?;
                                (v.len() == 4).then(|| TcMod::Turb {
                                    base: v[0],
                                    amplitude: v[1],
                                    phase: v[2],
                                    frequency: v[3],
                                })
                            }
                            _ => None,
                        };
                        if let Some(modifier) = modifier {
                            stage.tc_mods.push(modifier);
                        } else {
                            stage.unsupported.push(format!("tcmod {}", args.join(" ")));
                        }
                    }
                    "surfacesprites" => {
                        stage.surface_sprite = Some(parse_surface_sprite(args)?);
                    }
                    "ssfademax" => {
                        let value = surface_sprite_value(args, "ssFadeMax")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            if value > sprite.fade_dist {
                                sprite.fade_max = value;
                            } else {
                                stage.unsupported.push(format!(
                                    "ssFadeMax {value} (must exceed fadeDist {})",
                                    sprite.fade_dist
                                ));
                            }
                        } else {
                            stage
                                .unsupported
                                .push("ssFadeMax before surfaceSprites".into());
                        }
                    }
                    "ssfadescale" => {
                        let value = surface_sprite_value(args, "ssFadeScale")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.fade_scale = value.max(0.0);
                        }
                    }
                    "ssvariance" => {
                        let values = numbers(args)?;
                        if values.len() == 2 && values.iter().all(|value| value.is_finite()) {
                            if let Some(sprite) = &mut stage.surface_sprite {
                                sprite.variance = [values[0].max(0.0), values[1].max(0.0)];
                            }
                        } else {
                            stage
                                .unsupported
                                .push(format!("ssVariance {}", args.join(" ")));
                        }
                    }
                    "sswind" => {
                        let value = surface_sprite_value(args, "ssWind")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.wind = value.max(0.0);
                        }
                    }
                    "sswindidle" => {
                        let value = surface_sprite_value(args, "ssWindIdle")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.wind_idle = value.max(0.0);
                        }
                    }
                    "ssvertskew" => {
                        let value = surface_sprite_value(args, "ssVertSkew")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.vert_skew = value;
                        }
                    }
                    "ssfxduration" => {
                        let value = surface_sprite_value(args, "ssFXDuration")?;
                        if let Some(sprite) = &mut stage.surface_sprite {
                            if value > 0.0 {
                                sprite.fx_duration = value;
                            } else {
                                stage
                                    .unsupported
                                    .push(format!("ssFXDuration {value} (must be > 0)"));
                            }
                        } else {
                            stage
                                .unsupported
                                .push("ssFXDuration before surfaceSprites".into());
                        }
                    }
                    "ssfxgrow" => {
                        let values = numbers(args)?;
                        if values.len() == 2 && values.iter().all(|value| value.is_finite() && *value >= 0.0) {
                            if let Some(sprite) = &mut stage.surface_sprite {
                                sprite.fx_grow = [values[0], values[1]];
                            }
                        } else {
                            stage.unsupported.push(format!("ssFXGrow {}", args.join(" ")));
                        }
                    }
                    "ssfxalpharange" => {
                        let values = numbers(args)?;
                        if values.len() == 2
                            && values.iter().all(|value| value.is_finite() && (0.0..=1.0).contains(value))
                        {
                            if let Some(sprite) = &mut stage.surface_sprite {
                                sprite.fx_alpha_start = values[0];
                                sprite.fx_alpha_end = values[1];
                            }
                        } else {
                            stage
                                .unsupported
                                .push(format!("ssFXAlphaRange {}", args.join(" ")));
                        }
                    }
                    "sshangdown" => {
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.facing = SurfaceSpriteFacing::HangDown;
                        }
                    }
                    "ssanyangle" => {
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.facing = SurfaceSpriteFacing::AnyAngle;
                        }
                    }
                    "ssfaceup" => {
                        if let Some(sprite) = &mut stage.surface_sprite {
                            sprite.facing = SurfaceSpriteFacing::FaceUp;
                        }
                    }
                    "depthwrite" => stage.depth_write = true,
                    "glow" => stage.glow = true,
                    "depthfunc" if first == "equal" => stage.depth_equal = true,
                    "depthfunc" => stage.unsupported.push(format!("depthfunc {first}")),
                    "detail" => (),
                    _ => stage.unsupported.push(key),
                }
            } else {
                match key.as_str() {
                    "cull" => shader.cull = first,
                    "portal" => shader.portal = true,
                    "sun" | "q3map_sun" => shader.suns.push(parse_sun(args, false)?),
                    "sunext" | "q3map_sunext" => shader.suns.push(parse_sun(args, true)?),
                    "surfaceparm" => {
                        apply_ja_collision_surfaceparm(&mut shader, first.as_str());
                        match first.as_str() {
                        "sky" => shader.sky = true,
                        "nodraw" => shader.nodraw = true,
                        "nonsolid" => shader.nonsolid = true,
                        "playerclip" => { shader.player_clip = true; shader.nonsolid = true; },
                        "monsterclip" => { shader.monster_clip = true; shader.nonsolid = true; },
                        "botclip" => { shader.bot_clip = true; shader.nonsolid = true; },
                        "shotclip" | "weaponclip" => { shader.shot_clip = true; shader.nonsolid = true; },
                        "trigger" => { shader.trigger = true; shader.nonsolid = true; },
                        "water" => {
                            shader.translucent = true;
                            shader.water = true;
                            shader.nonsolid = true;
                        }
                        "lava" => {
                            shader.translucent = true;
                            shader.lava = true;
                            shader.nonsolid = true;
                        }
                        "slime" => {
                            shader.translucent = true;
                            shader.slime = true;
                            shader.nonsolid = true;
                        }
                        "fog" => { shader.fog = true; shader.nonsolid = true; },
                        "ladder" => shader.ladder = true,
                        "slick" => shader.slick = true,
                        "trans" | "nonopaque" => shader.translucent = true,
                        "alphashadow" => shader.alpha_shadow = true,
                        "lightfilter" => {
                            shader.light_filter = true;
                            shader.translucent = true;
                        }
                        _ => (),
                        }
                    },
                    "skyparms" => {
                        shader.sky = true;
                        if first != "-" {
                            shader.sky_box = Some(first);
                        }
                    }
                    "fogparms" => {
                        let values = numbers(args)?;
                        if values.len() != 4 || values.iter().any(|value| !value.is_finite()) {
                            return Err("expected fogparms (r g b) depth-to-opaque".into());
                        }
                        shader.fogparms = Some(FogParameters {
                            color: [values[0], values[1], values[2]],
                            depth_for_opaque: values[3].max(0.001),
                        });
                    }
                    "polygonoffset" => shader.polygon_offset = true,
                    "q3map_surfacelight" => {
                        shader.surface_light = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                            .unwrap_or(0.0)
                            .max(0.0);
                    }
                    "q3map_lightimage" => shader.light_image = args.first().cloned(),
                    "q3map_lightsubdivide" => {
                        shader.light_subdivide = args
                            .first()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite() && *value > 0.0);
                    }
                    "q3map_nofog" => shader.no_fog = true,
                    "sort" => {
                        // The `portal` keyword is only shorthand for `sort portal`
                        // in the original renderer. Accept both authoring styles
                        // so mirror shaders from stock/custom JKA content are
                        // recognized consistently.
                        if first == "portal" || first == "1" {
                            shader.portal = true;
                        } else {
                            shader.unsupported.push(format!("sort {first}"));
                        }
                    }
                    "deformvertexes" => shader.unsupported.push(key),
                    _ => (),
                }
            }
        }
        if !closed {
            return Err(format!("unterminated shader {name}"));
        }
        result.entry(name).or_insert(shader);
    }
    Ok(result)
}

fn tokenize(text: &str) -> Result<Vec<String>, String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err("shader text exceeds limit".into());
    }
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                result.push("\n".into());
                i += 1;
            }
            b if b.is_ascii_whitespace() => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                    if bytes[i] == b'\n' {
                        result.push("\n".into());
                    }
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    return Err("unterminated shader comment".into());
                }
                i += 2;
            }
            b'{' | b'}' => {
                result.push(char::from(bytes[i]).to_string());
                i += 1;
            }
            b'"' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i == bytes.len() {
                    return Err("unterminated shader string".into());
                }
                result.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
                i += 1;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !b"{}".contains(&bytes[i])
                {
                    i += 1;
                }
                result.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_general_texture_coordinate_pipeline() {
        let shaders = parse("textures/test/tcmods {\n{\nmap textures/x\ntcGen vector ( 1 0 0 ) ( 0 1 0 )\ntcMod scale 2 3\ntcMod scroll .1 .2\ntcMod rotate 45\n}\n}\n").unwrap();
        let stage = shaders["textures/test/tcmods"].primary().unwrap();
        assert!(matches!(stage.tc_gen, TcGen::Vector(_, _)));
        assert_eq!(stage.tc_mods.len(), 3);
    }

    #[test]
    fn parses_map_fog_parameters_as_depth_to_opaque() {
        let shaders = parse("textures/test/fog {\nfogparms ( .2 .3 .4 ) 640\n}\n").unwrap();
        let fog = shaders["textures/test/fog"].fogparms.unwrap();
        assert_eq!(fog.color, [0.2, 0.3, 0.4]);
        assert_eq!(fog.depth_for_opaque, 640.0);
    }

    #[test]
    fn parses_q3map_nofog_metadata() {
        let shaders = parse(
            "textures/test/nofog\n{\n q3map_nofog\n { map textures/test/base }\n}\n",
        )
        .unwrap();
        assert!(shaders["textures/test/nofog"].no_fog);
    }

    #[test]
    fn parses_q3map_surface_light_metadata() {
        let shaders = parse(
            "textures/test/lamp {\n\
             q3map_lightimage textures/test/lamp_glow.tga\n\
             q3map_surfacelight 850\n\
             q3map_lightsubdivide 96\n\
             { map textures/test/lamp.tga }\n\
             }\n",
        )
        .unwrap();
        let shader = &shaders["textures/test/lamp"];
        assert_eq!(shader.surface_light, 850.0);
        assert_eq!(
            shader.light_image.as_deref(),
            Some("textures/test/lamp_glow.tga")
        );
        assert_eq!(shader.light_subdivide, Some(96.0));
    }

    #[test]
    fn parses_jka_collision_surfaceparm_masks() {
        let shaders = parse(
            "textures/test/playerclip_custom {\n\
             surfaceparm playerclip\n\
             surfaceparm inside\n\
             surfaceparm slick\n\
             surfaceparm metalsteps\n\
             surfaceparm forcesight\n\
             }\n\
             textures/test/water_custom {\n\
             surfaceparm water\n\
             surfaceparm nonopaque\n\
             }\n",
        )
        .unwrap();

        let clip = &shaders["textures/test/playerclip_custom"];
        assert_eq!(clip.collision_contents_add & 0x1000_0010, 0x1000_0010);
        assert_eq!(clip.collision_contents_clear & 0x0000_8001, 0x0000_8001);
        assert_eq!(clip.collision_surface_flags_add & 0x0200_c000, 0x0200_c000);

        let water = &shaders["textures/test/water_custom"];
        assert_eq!(water.collision_contents_add & 0x0000_0004, 0x0000_0004);
        assert_eq!(water.collision_contents_clear & 0x0000_8001, 0x0000_8001);
    }

    #[test]
    fn parses_authored_portal_and_sort_portal() {
        let shaders = parse(
            "textures/test/mirror_a {\nportal\n{ map textures/test/mirror }\n}\n\
             textures/test/mirror_b {\nsort portal\n{ map textures/test/mirror }\n}\n",
        )
        .unwrap();
        assert!(shaders["textures/test/mirror_a"].portal);
        assert!(shaders["textures/test/mirror_b"].portal);
    }

    #[test]
    fn parses_jka_sun_and_q3map_sunext() {
        let shaders = parse(
            "textures/skies/test {\n\
             sun 255 128 64 250 90 45\n\
             q3map_sunExt 1 1 1 100 180 30 2 16\n\
             surfaceparm sky\n\
             }\n",
        )
        .unwrap();
        let shader = &shaders["textures/skies/test"];
        assert_eq!(shader.suns.len(), 2);
        assert_eq!(shader.suns[0].intensity, 250.0);
        assert_eq!(shader.suns[0].azimuth, 90.0);
        assert_eq!(shader.suns[0].elevation, 45.0);
        assert_eq!(shader.suns[1].deviance, 2.0);
        assert_eq!(shader.suns[1].samples, 16);
        let color = shader.suns[0].color;
        let length = (color[0] * color[0] + color[1] * color[1] + color[2] * color[2]).sqrt();
        assert!((length - 1.0).abs() < 1e-5);
    }
    #[test]
    fn parses_rend2_material_companions() {
        let shaders = parse(
            "textures/test/pbr {\n{\nmap textures/test/wall\nnormalHeightMap textures/test/wall_nh\nrmoMap textures/test/wall_rmo\nparallaxDepth 0.02\n}\n}\n",
        )
        .unwrap();
        let stage = shaders["textures/test/pbr"].primary().unwrap();
        assert_eq!(
            stage.normal_height_map.as_deref(),
            Some("textures/test/wall_nh")
        );
        assert_eq!(stage.rmo_map.as_deref(), Some("textures/test/wall_rmo"));
        assert_eq!(stage.parallax_depth, Some(0.02));
    }
    #[test]
    fn parses_rend2_material_controls() {
        let shaders = parse(
            "textures/test/pbr_controls {\n{\nmap textures/test/wall\nnormalScale 1.5 0.75 0.03\nrmosMap textures/test/wall_rmos\ngloss 0.8\nspecularScale 0.12 0.34 0.56 0.9\nparallaxDepth 0.02\n}\n}\n",
        )
        .unwrap();
        let stage = shaders["textures/test/pbr_controls"].primary().unwrap();
        assert_eq!(stage.normal_scale, Some([1.5, 0.75]));
        assert_eq!(stage.rmo_map.as_deref(), Some("textures/test/wall_rmos"));
        assert!(stage.rmo_specular_alpha);
        assert_eq!(stage.specular_reflectance, Some([0.12, 0.34, 0.56]));
        assert!((stage.roughness.unwrap() - 0.1).abs() < 1e-6);
        // Explicit parallaxDepth appears later and therefore wins over normalScale.z.
        assert_eq!(stage.parallax_depth, Some(0.02));
    }

    #[test]
    fn parses_jka_saber_glow_stage() {
        let shaders = parse(
            "gfx/effects/sabers/red_glow {\ncull twosided\n{\nmap gfx/effects/sabers/red_glow2\nblendFunc GL_ONE GL_ONE\nglow\nrgbGen vertex\n}\n}\n",
        )
        .unwrap();
        let shader = &shaders["gfx/effects/sabers/red_glow"];
        let stage = shader.primary().unwrap();
        assert_eq!(shader.cull, "twosided");
        assert_eq!(stage.image, "gfx/effects/sabers/red_glow2");
        assert_eq!(stage.blend, "gl_one gl_one");
        assert!(stage.glow);
        assert_eq!(stage.rgb_gen, RgbGen::Vertex);
        assert!(stage.unsupported.is_empty());
    }

    #[test]
    fn parses_jka_surface_sprite_metadata() {
        let shaders = parse(
            "textures/test/grass {\n{\nmap textures/test/ground\n}\n{\nmap gfx/sprites/y_grass_tall\nsurfaceSprites vertical 32 36 42 500\nssFadeMax 1500\nssFadeScale 1\nssVariance 1 2\nssWind 0.5\nalphaFunc GE192\nblendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA\ndepthWrite\nrgbGen vertex\n}\n}\n",
        )
        .unwrap();
        let stage = &shaders["textures/test/grass"].stages[1];
        let sprite = stage.surface_sprite.expect("surface sprite");
        assert_eq!(sprite.kind, SurfaceSpriteType::Vertical);
        assert_eq!(sprite.width, 32.0);
        assert_eq!(sprite.height, 36.0);
        assert_eq!(sprite.density, 42.0);
        assert_eq!(sprite.fade_dist, 500.0);
        assert_eq!(sprite.fade_max, 1500.0);
        assert_eq!(sprite.fade_scale, 1.0);
        assert_eq!(sprite.variance, [1.0, 2.0]);
        assert_eq!(sprite.wind, 0.5);
    }
    #[test]
    fn parses_jka_effect_surface_sprite_controls() {
        let shaders = parse(
            "textures/test/rain {\n{\nclampmap textures/test/splash\nsurfaceSprites effect 1.5 1.5 64 512\nssVariance 1 0.75\nssFXDuration 135\nssFXGrow 6 6\nssFXAlphaRange 0.30 0\nssFadeMax 768\nssFadeScale 0.5\nblendFunc GL_ONE GL_ONE\n}\n}\n",
        )
        .unwrap();
        let stage = shaders["textures/test/rain"].primary().unwrap();
        let sprite = stage.surface_sprite.expect("effect surface sprite");
        assert_eq!(sprite.kind, SurfaceSpriteType::Effect);
        assert_eq!(sprite.fx_duration, 135.0);
        assert_eq!(sprite.fx_grow, [6.0, 6.0]);
        assert_eq!(sprite.fx_alpha_start, 0.30);
        assert_eq!(sprite.fx_alpha_end, 0.0);
        assert_eq!(sprite.fade_max, 768.0);
        assert_eq!(sprite.fade_scale, 0.5);
        assert!(stage.unsupported.is_empty());
    }

    #[test]
    fn parses_vertex_rgb_and_alpha_generation_independently() {
        let shaders = parse(
            "textures/test/blend {\n{\nmap textures/test/a\nrgbGen exactVertex\nalphaGen vertex\n}\n{\nmap textures/test/b\nrgbGen vertex\nalphaGen oneMinusVertex\n}\n}\n",
        )
        .unwrap();
        let stages = &shaders["textures/test/blend"].stages;
        assert_eq!(stages[0].rgb_gen, RgbGen::ExactVertex);
        assert_eq!(stages[0].alpha_gen, AlphaGen::Vertex);
        assert_eq!(stages[1].rgb_gen, RgbGen::Vertex);
        assert_eq!(stages[1].alpha_gen, AlphaGen::OneMinusVertex);
        assert!(stages[0].unsupported.is_empty());
        assert!(stages[1].unsupported.is_empty());
    }

    #[test]
    fn parses_rend2_vertex_lit_rgb_generation() {
        let shaders = parse(
            "textures/test/rend2 {\n{\nmap textures/test/a\nrgbGen vertexLit\n}\n{\nmap textures/test/b\nrgbGen exactVertexLit\n}\n}\n",
        )
        .unwrap();
        let stages = &shaders["textures/test/rend2"].stages;
        assert_eq!(stages[0].rgb_gen, RgbGen::Vertex);
        assert_eq!(stages[1].rgb_gen, RgbGen::ExactVertex);
        assert!(stages[0].unsupported.is_empty());
        assert!(stages[1].unsupported.is_empty());
    }

}
