//! Ocean cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_BLUEWATERCOASTSHELTER {
        runtime: CvarDef::new("r_blueWaterCoastShelter", "0", "0|1",
            "Upstream coast: 1 includes land/wind-fetch shelter; 0 keeps depth breaking and moving surf only. Shelter can remove long waves under weak wind. Use bluewater_coast_debug to inspect local attenuation.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_coast_shelter),
            |app,value| {let CvarValue::Bool(v)=value else {unreachable!()};
                app.video.ocean_settings.blue_water_coast_shelter=v;app.commit_ocean_settings();Ok(())}),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {settings.ocean_settings.blue_water_coast_shelter=value=="1";}),
            archive: Some((27345,|state|u8::from(state.video.ocean_settings.blue_water_coast_shelter).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERCOAST {
        runtime: CvarDef::new("r_blueWaterCoast", "0", "0|1",
            "Original Blue Water coastal waves/surf on native or Hybrid: depth breaking, land fetch shelter and moving surf bands. Separate from the r_coastal shallow-bank solver; 0 restores open-ocean waves.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_coast),
            |app,value| {let CvarValue::Bool(v)=value else {unreachable!()};
                app.video.ocean_settings.blue_water_coast=v;app.commit_ocean_settings();Ok(())}),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {settings.ocean_settings.blue_water_coast=value=="1";}),
            archive: Some((27344,|state|u8::from(state.video.ocean_settings.blue_water_coast).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKESOURCEHINTS {
        runtime: CvarDef::new("r_lakeSourceHints", "1", "0|1",
            "Use each promoted JKA water shader's source hue, alpha-blend clarity, authored deformation, roughness and tcMod scroll heading in Lake mode. 0 restores the global Lake material.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.lake.source_hints),
            |app, value| { let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.lake.source_hints = v; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { settings.ocean_settings.lake.source_hints = value == "1"; }),
            archive: Some((27377, |state| u8::from(state.video.ocean_settings.lake.source_hints).to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEWINDSPEED {
        runtime: CvarDef::new("r_lakeWindSpeed", "0", "0..40", "Lake ripple wind speed in metres per second; independent of ocean spectrum.",
            CvarType::Float { min: 0.0, max: 40.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.wind_speed),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.lake.wind_speed=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() {settings.ocean_settings.lake.wind_speed=v;}
            }),
            archive: Some((27340, |state| state.video.ocean_settings.lake.wind_speed.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKEDIRECTION {
        runtime: CvarDef::new("r_lakeDirection", "52", "-180..180", "Lake ripple direction in degrees from +X toward world +Z.",
            CvarType::Float { min: -180.0, max: 180.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.direction),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.lake.direction=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() {settings.ocean_settings.lake.direction=v;}
            }),
            archive: Some((27341, |state| state.video.ocean_settings.lake.direction.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKETRANSPARENCY {
        runtime: CvarDef::new("r_lakeTransparency", "0.5", "0.1..10", "Lake optical clarity multiplier; higher values reveal more of the bed.",
            CvarType::Float { min: 0.1, max: 10.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.transparency),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.lake.transparency=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() {settings.ocean_settings.lake.transparency=v;}
            }),
            archive: Some((27342, |state| state.video.ocean_settings.lake.transparency.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKEREFLECTIONS {
        runtime: CvarDef::new("r_lakeReflections", "0.525", "0..1", "Lake world/sky reflection and sun glitter strength; 0 removes reflections.",
            CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.reflections),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.reflections=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.reflections=v.clamp(0.0,1.0); } }
            }),
            archive: Some((27363, |state| state.video.ocean_settings.lake.reflections.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKEAMPLITUDE {
        runtime: CvarDef::new("r_lakeAmplitude", "6.65", "0..12", "Lake geometric amplitude in inches above/below the map plane; 0 is flat.",
            CvarType::Float { min: 0.0, max: 12.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.amplitude),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.lake.amplitude=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() {
                    if v.is_finite() {settings.ocean_settings.lake.amplitude=v.clamp(0.0,12.0);}
                }
            }),
            archive: Some((27360, |state| state.video.ocean_settings.lake.amplitude.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_LAKEWATERCOLOR {
        runtime: CvarDef::new("r_lakeWaterColor", "0.41818184 0.9619174 1", "r g b (0..1)",
            "Lake optical color for absorption and scattering; independent of surface tint and fog.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_water_color(app, APP_R_LAKEWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27361, |state| { let c = state.video.ocean_settings.lake.water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_lake_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_LAKESPEED {
        runtime: CvarDef::new("r_lakeSpeed", "1.45", "0..3", "Lake deformation animation speed; 0 freezes it, 1 is normal.",
            CvarType::Float { min: 0.0, max: 3.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.speed),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.speed=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| {
                if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.speed=v.clamp(0.0,3.0); } }
            }),
            archive: Some((27364, |state| state.video.ocean_settings.lake.speed.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKETRANSMISSION {
        runtime: CvarDef::new("r_lakeTransmission", "1", "0..1.0", "Cheapwater transmission: 0 preserves opaque shading, 1 reveals the bed with depth absorption.",
            CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.cheap_transmission),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.cheap_transmission=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.cheap_transmission=v.clamp(0.0,1.0); } } }),
            archive: Some((27369, |state| state.video.ocean_settings.lake.cheap_transmission.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEREFRACTION {
        runtime: CvarDef::new("r_lakeRefraction", "0.026", "0..0.05", "Cheapwater screen-space refraction strength; reuses its moving normal layers. Zero disables distortion.",
            CvarType::Float { min: 0.0, max: 0.05 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.cheap_refraction),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.cheap_refraction=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.cheap_refraction=v.clamp(0.0,0.05); } } }),
            archive: Some((27370, |state| state.video.ocean_settings.lake.cheap_refraction.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEDEPTHCOLOR {
        runtime: CvarDef::new("r_lakeDepthColor", "0.52", "0.0..1.0", "Cheapwater shallow/deep color blend. Zero preserves the base material.",
            CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.depth_color_strength),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.depth_color_strength=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.depth_color_strength=v.clamp(0.0,1.0); } } }),
            archive: Some((27371, |state| state.video.ocean_settings.lake.depth_color_strength.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKECOLORDEPTH {
        runtime: CvarDef::new("r_lakeColorDepth", "52", "0.1..100.0", "Vertical bed depth in metres at which Cheapwater reaches its deep color.",
            CvarType::Float { min: 0.1, max: 100.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.color_depth),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.color_depth=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.color_depth=v.clamp(0.1,100.0); } } }),
            archive: Some((27372, |state| state.video.ocean_settings.lake.color_depth.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKESHALLOWCOLOR {
        runtime: CvarDef::new("r_lakeShallowColor", "0 0.043898553 0.025940072", "r g b (0..1)", "Cheapwater shallow color for depth coloring.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.ocean_settings.lake.shallow_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_lake_shallow_color(app, APP_R_LAKESHALLOWCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_shallow_color, None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.shallow_color=c.map(|v| v.clamp(0.0,1.0)); } } }),
            archive: Some((27373, |state| { let c=state.video.ocean_settings.lake.shallow_color; format!("{} {} {}",c[0],c[1],c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_shallow_color(&mut self,name:&str,value:&str)->Result<(),String> {
                self.video.ocean_settings.lake.shallow_color=Self::parse_cvar_floats(name,value)?.map(|v| v.clamp(0.0,1.0));
                self.commit_ocean_settings();Ok(())
            }
        }]
    }
    APP_R_LAKEDEEPCOLOR {
        runtime: CvarDef::new("r_lakeDeepColor", "0 0.08160421 0.061574105", "r g b (0..1)", "Cheapwater deep color for depth coloring.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.ocean_settings.lake.deep_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_lake_deep_color(app, APP_R_LAKEDEEPCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_deep_color, None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.deep_color=c.map(|v| v.clamp(0.0,1.0)); } } }),
            archive: Some((27374, |state| { let c=state.video.ocean_settings.lake.deep_color; format!("{} {} {}",c[0],c[1],c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_deep_color(&mut self,name:&str,value:&str)->Result<(),String> {
                self.video.ocean_settings.lake.deep_color=Self::parse_cvar_floats(name,value)?.map(|v| v.clamp(0.0,1.0));
                self.commit_ocean_settings();Ok(())
            }
        }]
    }
    APP_R_LAKEOPAQUEDEPTH {
        runtime: CvarDef::new("r_lakeOpaqueDepth", "8", "0.25..100 feet", "Cheapwater vertical bed depth in feet where nonlinear transparency reaches zero. 3-inch water remains almost fully clear at the default 8 feet.",
            CvarType::Float { min: 0.25, max: 100.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.opaque_depth),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.opaque_depth=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.opaque_depth=v.clamp(0.25,100.0); } } }),
            archive: Some((27375, |state| state.video.ocean_settings.lake.opaque_depth.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEWAKE {
        runtime: CvarDef::new("r_lakeWake", "10", "0..10", "Cheapwater player wake/ripple strength. Stronger ripples use the same rendering work. Independent of the ocean interaction solver. 0 disables.",
            CvarType::Float { min: 0.0, max: 10.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.lake.wake_strength),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.wake_strength=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.lake.wake_strength=v.clamp(0.0,10.0); } } }),
            archive: Some((27376, |state| state.video.ocean_settings.lake.wake_strength.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEPLANAR {
        runtime: CvarDef::new("r_lakePlanar", "1", "0|1", "Compatibility alias: 1 selects lake Planar, 0 selects Sky. Use r_lakeReflectionMode for Sky/SSR/Planar comparisons.",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.lake.reflection_mode == 2),
            |app,value| { let CvarValue::Bool(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.reflection_mode=if v {2} else {0};app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { settings.ocean_settings.lake.reflection_mode=if value=="1" {2} else {0}; }),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKESHADER {
        runtime: CvarDef::new("r_lakeShader", "1", "0|1", "Lake material A/B: 0 original optical water, 1 Cheapwater material with neutral water reflections. Geometry and reflection selection are shared.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.lake.shader as i32),
            |app,value| { let CvarValue::Int(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.shader=v as u8;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<u8>() {settings.ocean_settings.lake.shader=v.min(1);} }),
            archive: Some((27367, |state| state.video.ocean_settings.lake.shader.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKEREFLECTIONMODE {
        runtime: CvarDef::new("r_lakeReflectionMode", "2", "0..2", "Lake-local reflections: 0 Sky, 1 SSR, 2 Planar. Independent of global reflection mode and quality.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.lake.reflection_mode as i32),
            |app,value| { let CvarValue::Int(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.reflection_mode=v as u8;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Ok(v)=value.parse::<u8>() {settings.ocean_settings.lake.reflection_mode=v.min(2);} }),
            archive: Some((27368, |state| state.video.ocean_settings.lake.reflection_mode.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKETINT {
        runtime: CvarDef::new("r_lakeTint", "0.09994107 0.09994107 0.09994107", "r g b (0..1)",
            "Lake surface tint, independent of optical color and fog; white is neutral.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.tint_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_tint(app, APP_R_LAKETINT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_tint, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.tint_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27365, |state| { let c = state.video.ocean_settings.lake.tint_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_tint(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.tint_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_LAKEFOGCOLOR {
        runtime: CvarDef::new("r_lakeFogColor", "0 0.00003021201 0.01662098", "r g b (0..1)",
            "Lake underwater fog color, independent of the surface color.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_fog_color(app, APP_R_LAKEFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27362, |state| { let c = state.video.ocean_settings.lake.fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_lake_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.fog_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDGERSTNERAMPLITUDE {
        runtime: CvarDef::new("r_hybridGerstnerAmplitude", "128", "0.0..16384.0",
            "Hybrid-only directional Gerstner amplitude; independent of Godot and Blue Water FFT.",
            CvarType::Float { min: 0.0, max: 16384.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_swell.amplitude),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_swell.amplitude = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.hybrid_swell.amplitude = v.clamp(0.0, 16384.0); }
                }
            }),
            archive: Some((25180, |state| state.video.ocean_settings.hybrid_swell.amplitude.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDGERSTNERWAVELENGTH {
        runtime: CvarDef::new("r_hybridGerstnerWavelength", "1400", "64.0..32768.0",
            "Hybrid-only directional Gerstner wavelength; independent of Godot and Blue Water FFT.",
            CvarType::Float { min: 64.0, max: 32768.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_swell.wavelength),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_swell.wavelength = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.hybrid_swell.wavelength = v.clamp(64.0, 32768.0); }
                }
            }),
            archive: Some((25181, |state| state.video.ocean_settings.hybrid_swell.wavelength.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDGERSTNERSPEED {
        runtime: CvarDef::new("r_hybridGerstnerSpeed", "180", "-4096.0..4096.0",
            "Hybrid-only directional Gerstner speed; independent of Godot and Blue Water FFT.",
            CvarType::Float { min: -4096.0, max: 4096.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_swell.speed),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_swell.speed = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.hybrid_swell.speed = v.clamp(-4096.0, 4096.0); }
                }
            }),
            archive: Some((25183, |state| state.video.ocean_settings.hybrid_swell.speed.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDGERSTNERSTEEPNESS {
        runtime: CvarDef::new("r_hybridGerstnerSteepness", "0.65", "0.0..1.0",
            "Hybrid-only directional Gerstner steepness; independent of Godot and Blue Water FFT.",
            CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_swell.steepness),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_swell.steepness = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.hybrid_swell.steepness = v.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((25184, |state| state.video.ocean_settings.hybrid_swell.steepness.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDBREAKER {
        runtime: CvarDef::new("r_hybridBreaker", "0", "0..2",
            "Hybrid crest A/B: 0 original; 1 conservative horizontal compression limit; 2 bounded crest folds, signed normals and spilling foam. Preserves vertical wave height. Applies live.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.hybrid_breaker)),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_breaker = v as u8;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.hybrid_breaker = v.min(2); }
            }),
            archive: Some((25221, |state| state.video.ocean_settings.hybrid_breaker.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDFFTBLEND {
        runtime: CvarDef::new("r_hybridFftBlend", "1", "0..1",
            "Hybrid ocean: 1 pure Blue Water FFT; 0 authored Gerstner; intermediate values mix FFT and analytical displacement. Native and Godot unaffected.",
            CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_fft_blend),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_fft_blend = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.hybrid_fft_blend = v.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((25189, |state| state.video.ocean_settings.hybrid_fft_blend.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_BLUEWATERMESHSEGMENTS {
        runtime: CvarDef::new("r_blueWaterMeshSegments", "64", "64|128|256",
            "Blue Water native clipmap segments per level: 64/128/256. Independent of r_oceanMeshQuality.", CvarType::Int { min: 64, max: 256 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_mesh_segments as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                if !matches!(value, 64 | 128 | 256) { return Err("r_blueWaterMeshSegments: use 64, 128, or 256".into()); }
                app.video.ocean_settings.blue_water_mesh_segments = value as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u32>() {
                    if matches!(v, 64 | 128 | 256) { settings.ocean_settings.blue_water_mesh_segments = v; }
                }
            }),
            archive: Some((25190, |state| state.video.ocean_settings.blue_water_mesh_segments.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERMESHLEVELS {
        runtime: CvarDef::new("r_blueWaterMeshLevels", "6", "6..9",
            "Blue Water clipmap level count; 9 is native reference. Rebuilds geometry.", CvarType::Int { min: 6, max: 9 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_mesh_levels as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_mesh_levels = value as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if matches!(v, 6..=9) { settings.ocean_settings.blue_water_mesh_levels = v; }
                }
            }),
            archive: Some((25191, |state| state.video.ocean_settings.blue_water_mesh_levels.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERSSRSTEPS {
        runtime: CvarDef::new("r_blueWaterSSRSteps", "16", "4..32",
            "Native Blue Water reflection ray steps (compile-time specialization; rebuilds only its surface pipeline).",
            CvarType::Int { min: 4, max: 32 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_ssr_steps as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_ssr_steps = v as u32;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u32>() { settings.ocean_settings.blue_water_ssr_steps = v.clamp(4, 32); }
            }),
            archive: Some((25201, |state| state.video.ocean_settings.blue_water_ssr_steps.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANWAKE {
        runtime: CvarDef::new("r_oceanWake", "0", "0..1",
            "Native / Hybrid ocean: 0 JKA's own water interaction only; 1 Blue Water's ship wake (dispersive wake field, trail foam, bow waves, hull foam, impact rings) for A/B. Drive it with the oceanwake command.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.blue_wake)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.blue_wake = v as u8;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.blue_wake = v.min(1); }
            }),
            archive: Some((27330, |state| state.video.ocean_settings.blue_wake.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANSPINDRIFT {
        runtime: CvarDef::new("r_oceanSpindrift", "0", "0..1",
            "Native / Hybrid ocean: Blue Water's spindrift, wisps of spray torn off breaking crests from a gale's wind (13 m/s, set the wind in Water settings). 0 off.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.spindrift)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.spindrift = v as u8;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.spindrift = v.min(1); }
            }),
            archive: Some((27331, |state| state.video.ocean_settings.spindrift.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_HYBRIDREFRACTIONMODE {
        runtime: CvarDef::new("r_hybridRefractionMode", "1", "0..2",
            "Hybrid only: 0 off, 1 cheap screen offset, 2 ray trace.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.hybrid_refraction_mode as i32),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() }; app.video.ocean_settings.hybrid_refraction_mode = v as u8; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.hybrid_refraction_mode = v.min(2); } }),
            archive: Some((25202, |state| state.video.ocean_settings.hybrid_refraction_mode.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDCAUSTICSMODE {
        runtime: CvarDef::new("r_hybridCausticsMode", "3", "0..4",
            "Hybrid: 0 native, 1 native FFT only, 2 off, 3 Godot wave curvature, 4 animated procedural.",
            CvarType::Int { min: 0, max: 4 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.hybrid_caustics_mode as i32),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() }; app.video.ocean_settings.hybrid_caustics_mode = v as u8; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.hybrid_caustics_mode = v.min(4); } }),
            archive: Some((25203, |state| state.video.ocean_settings.hybrid_caustics_mode.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDUNDERWATERMODE {
        runtime: CvarDef::new("r_hybridUnderwaterMode", "4", "0..4",
            "Hybrid only: 4 Godot surface+optics when submerged, 0/1/3 native, 2 composite off.",
            CvarType::Int { min: 0, max: 4 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.hybrid_underwater_mode as i32),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() }; app.video.ocean_settings.hybrid_underwater_mode = v as u8; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.hybrid_underwater_mode = v.min(4); } }),
            archive: Some((25204, |state| state.video.ocean_settings.hybrid_underwater_mode.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDWATERLINEDEBUG {
        runtime: CvarDef::new("r_hybridWaterlineDebug", "0", "0..3",
            "Hybrid mode 4 debug: 0 off; 1 CPU/optics log; 2 color Godot mesh; 3 CAMERA-XZ GPU height gauge drawn 100u forward: yellow displaced wave, orange raw wave, magenta native eye probe, cyan flat, green/red eye tick. No GPU readback.",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.hybrid_waterline_debug)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_waterline_debug = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_HYBRIDSSR {
        runtime: CvarDef::new("r_hybridSSR", "0", "0|1",
            "Hybrid only: native screen-space reflection tracing (sky/env reflection remains).",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.hybrid_ssr),
            |app, value| { let CvarValue::Bool(v) = value else { unreachable!() }; app.video.ocean_settings.hybrid_ssr = v; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { settings.ocean_settings.hybrid_ssr = parse_bool(value).unwrap_or(settings.ocean_settings.hybrid_ssr); }),
            archive: Some((25205, |state| u8::from(state.video.ocean_settings.hybrid_ssr).to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDTRANSPARENCY {
        runtime: CvarDef::new("r_hybridTransparency", "1", "0.1..10",
            "Hybrid clarity multiplier for both Godot submerged absorption distance and Blue Water surface.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_transparency.to_string()),
            |app, value| App::cvar_set_hybrid_transparency(app, APP_R_HYBRIDTRANSPARENCY.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_transparency, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_transparency = v.clamp(0.1, 10.0); } }
            }),
            archive: Some((25210, |state| state.video.ocean_settings.hybrid_transparency.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_transparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_transparency = Self::parse_cvar_finite(name, value)?.clamp(0.1, 10.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDFOGCOLOR {
        runtime: CvarDef::new("r_hybridFogColor", "", "r g b (0..1)",
            "Hybrid-only fog color shared by Godot submerged optics and native scattering.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.hybrid_fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_hybrid_fog_color(app, APP_R_HYBRIDFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.hybrid_fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25211, |state| { let c = state.video.ocean_settings.hybrid_fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_fog_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDWATERCOLOR {
        runtime: CvarDef::new("r_hybridWaterColor", "", "r g b (0..1)",
            "Hybrid-only water body color; native Blue Water is unaffected.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.hybrid_water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_hybrid_water_color(app, APP_R_HYBRIDWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.hybrid_water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25212, |state| { let c = state.video.ocean_settings.hybrid_water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDCAUSTICSSCALE {
        runtime: CvarDef::new("r_hybridCausticsScale", "3", "1..6",
            "Hybrid Godot-receiver caustic pattern density: 3x matches 181m Blue Water to 57m Godot scale.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_caustics_scale.to_string()),
            |app, value| App::cvar_set_hybrid_caustics_scale(app, APP_R_HYBRIDCAUSTICSSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_caustics_scale, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_caustics_scale = v.clamp(1.0, 6.0); } }
            }),
            archive: Some((25213, |state| state.video.ocean_settings.hybrid_caustics_scale.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_caustics_scale(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_caustics_scale = Self::parse_cvar_finite(name, value)?.clamp(1.0, 6.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDCAUSTICSGAIN {
        runtime: CvarDef::new("r_hybridCausticsGain", "1.5", "0..4",
            "Hybrid-only caustic focus contrast multiplier, applied after Godot receiver calculation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_caustics_gain.to_string()),
            |app, value| App::cvar_set_hybrid_caustics_gain(app, APP_R_HYBRIDCAUSTICSGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_caustics_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_caustics_gain = v.clamp(0.0, 4.0); } }
            }),
            archive: Some((25214, |state| state.video.ocean_settings.hybrid_caustics_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_caustics_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_caustics_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDCAUSTICSFAST {
        runtime: CvarDef::new("r_hybridCausticsFast", "0", "0|1",
            "Hybrid A/B: 3-tap forward-difference caustics versus reference 4-tap central differences.", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.hybrid_caustics_fast),
            |app, value| { let CvarValue::Bool(v) = value else { unreachable!() }; app.video.ocean_settings.hybrid_caustics_fast = v; app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { settings.ocean_settings.hybrid_caustics_fast = parse_bool(value).unwrap_or(settings.ocean_settings.hybrid_caustics_fast); }),
            archive: Some((25215, |state| u8::from(state.video.ocean_settings.hybrid_caustics_fast).to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_HYBRIDPROCEDURALSCALE {
        runtime: CvarDef::new("r_hybridProceduralScale", "1", "0.25..4",
            "Hybrid animated caustic pattern density. Does not increase FFT or GPU generation cost.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_procedural_scale.to_string()),
            |app, value| App::cvar_set_hybrid_procedural_scale(app, APP_R_HYBRIDPROCEDURALSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_procedural_scale, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_procedural_scale = v.clamp(0.25, 4.0); } }
            }),
            archive: Some((25217, |state| state.video.ocean_settings.hybrid_procedural_scale.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_procedural_scale(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_procedural_scale = Self::parse_cvar_finite(name, value)?.clamp(0.25, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDPROCEDURALSPEED {
        runtime: CvarDef::new("r_hybridProceduralSpeed", "1", "0..3",
            "Hybrid animated caustic evolution speed, modulated by ocean roughness.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_procedural_speed.to_string()),
            |app, value| App::cvar_set_hybrid_procedural_speed(app, APP_R_HYBRIDPROCEDURALSPEED.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_procedural_speed, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_procedural_speed = v.clamp(0.0, 3.0); } }
            }),
            archive: Some((25218, |state| state.video.ocean_settings.hybrid_procedural_speed.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_procedural_speed(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_procedural_speed = Self::parse_cvar_finite(name, value)?.clamp(0.0, 3.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERBOUNDARY {
        runtime: CvarDef::new("r_blueWaterBoundaryMode", "3", "0..3",
            "0 exact JKA footprint+PVS; 1 bounds/PVS only; 2 no boundary/PVS test (diagnostic); 3 exact with per-tile candidate masks.", CvarType::Int { min: 0, max: 3 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_boundary_mode as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_boundary_mode = value as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if matches!(v, 0..=3) { settings.ocean_settings.blue_water_boundary_mode = v; }
                }
            }),
            archive: Some((25192, |state| state.video.ocean_settings.blue_water_boundary_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERCAUSTICMODE {
        runtime: CvarDef::new("r_blueWaterCausticsMode", "0", "0..3",
            "Native: 0 compute every frame, 1 on FFT updates, 2 disabled. Legacy 3 (Godot) now belongs to r_hybridCausticsMode.", CvarType::Int { min: 0, max: 3 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_caustics_mode as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_caustics_mode = value as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if matches!(v, 0..=3) { settings.ocean_settings.blue_water_caustics_mode = v; }
                }
            }),
            archive: Some((25193, |state| state.video.ocean_settings.blue_water_caustics_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERUNDERWATERMODE {
        runtime: CvarDef::new("r_blueWaterUnderwaterMode", "0", "0..4",
            "Native: 0 full, 1 no motes, 2 no composite, 3 submerged-only. Legacy 4 now belongs to r_hybridUnderwaterMode.",
            CvarType::Int { min: 0, max: 4 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_underwater_mode as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_underwater_mode = value as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if v <= 4 { settings.ocean_settings.blue_water_underwater_mode = v; }
                }
            }),
            archive: Some((25197, |state| state.video.ocean_settings.blue_water_underwater_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERPREPAREGODOTMESH {
        runtime: CvarDef::new("r_blueWaterPrepareGodotMesh", "0", "0|1", "A/B: update Godot ocean clipmap indices above water in Blue Water mode. Hybrid mode always prepares them for submersion.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_prepare_godot_mesh),
            |app, value| {
                let CvarValue::Bool(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_prepare_godot_mesh = value;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.ocean_settings.blue_water_prepare_godot_mesh = v; }
            }),
            archive: Some((25194, |state| if state.video.ocean_settings.blue_water_prepare_godot_mesh { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERADAPTER {
        runtime: CvarDef::new("r_blueWaterAdapter", "1", "0|1", "A/B: generate legacy displacement/normal textures for JKA water consumers. 0 freezes these textures, Blue Water surface unaffected.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_adapter),
            |app, value| {
                let CvarValue::Bool(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_adapter = value;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.ocean_settings.blue_water_adapter = v; }
            }),
            archive: Some((25195, |state| if state.video.ocean_settings.blue_water_adapter { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERSSR {
        runtime: CvarDef::new("r_blueWaterSSR", "1", "0|1",
            "A/B: native screen-space reflection rays only; sky/environment reflection remains.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_ssr),
            |app, value| {
                let CvarValue::Bool(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_ssr = value;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.ocean_settings.blue_water_ssr = v; }
            }),
            archive: Some((25196, |state| if state.video.ocean_settings.blue_water_ssr { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERREFRACTION {
        runtime: CvarDef::new("r_blueWaterRefraction", "0.1", "0..0.15",
            "Blue Water refraction strength; 0 disables. Select cheap screen offset or physical trace with r_blueWaterRefractionMode.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_refraction.to_string()),
            |app, value| App::cvar_set_blue_water_refraction(app, APP_R_BLUEWATERREFRACTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_refraction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_refraction = v.clamp(0.0, 0.15); }
                }
            }),
            archive: Some((25198, |state| state.video.ocean_settings.blue_water_refraction.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_refraction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_refraction = Self::parse_cvar_finite(name, value)?.clamp(0.0, 0.15);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERREFRACTIONMODE {
        runtime: CvarDef::new("r_blueWaterRefractionMode", "2", "0..2",
            "A/B: 0 off; 1 inexpensive Godot-style screen offset; 2 physical scene ray trace.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_refraction_mode as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_refraction_mode = value as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if v <= 2 { settings.ocean_settings.blue_water_refraction_mode = v; }
                }
            }),
            archive: Some((25199, |state| state.video.ocean_settings.blue_water_refraction_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_BLUEWATERTRANSPARENCY {
        runtime: CvarDef::new("r_blueWaterTransparency", "1", "0.1..10",
            "Optical travel distance multiplier for Blue Water; higher is clearer, 1 preserves upstream calibration.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_optics.transparency.to_string()),
            |app, value| App::cvar_set_blue_water_transparency(app, APP_R_BLUEWATERTRANSPARENCY.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_transparency, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_optics.transparency = v.clamp(0.1, 10.0); }
                }
            }),
            archive: Some((25155, |state| state.video.ocean_settings.blue_water_optics.transparency.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_transparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.transparency = Self::parse_cvar_finite(name, value)?.clamp(0.1, 10.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWATERCOLOR {
        runtime: CvarDef::new("r_blueWaterWaterColor", "", "r g b (0..1)",
            "sRGB tint of the Blue Water body and transmitted crest light.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.blue_water_optics.water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_blue_water_water_color(app, APP_R_BLUEWATERWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.blue_water_optics.water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25156, |state| { let c = state.video.ocean_settings.blue_water_optics.water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.water_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERFOGCOLOR {
        runtime: CvarDef::new("r_blueWaterFogColor", "", "r g b (0..1)",
            "sRGB color of Blue Water underwater scattering.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.blue_water_optics.fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_blue_water_fog_color(app, APP_R_BLUEWATERFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.blue_water_optics.fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25157, |state| { let c = state.video.ocean_settings.blue_water_optics.fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.fog_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }

    APP_R_BLUEWATERHEIGHT {
        runtime: CvarDef::new("r_blueWaterHeight", "", "0.0..=60.0",
            "Significant wave height in metres (4 times the surface height standard deviation). Zero is flat.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.height.to_string()),
            |app, value| App::cvar_set_blue_water_height(app, APP_R_BLUEWATERHEIGHT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_height, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.height = v.clamp(0.0, 60.0); }
                }
            }),
            archive: Some((25150, |state| state.video.ocean_settings.blue_water_waves.height.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_height(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.height = Self::parse_cvar_finite(name, value)?.clamp(0.0, 60.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERDIRECTION {
        runtime: CvarDef::new("r_blueWaterDirection", "", "-180.0..=180.0",
            "Direction waves travel, in degrees from +X toward world +Z.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.direction.to_string()),
            |app, value| App::cvar_set_blue_water_direction(app, APP_R_BLUEWATERDIRECTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_direction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.direction = v.clamp(-180.0, 180.0); }
                }
            }),
            archive: Some((25151, |state| state.video.ocean_settings.blue_water_waves.direction.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_direction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.direction = Self::parse_cvar_finite(name, value)?.clamp(-180.0, 180.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWAVELENGTH {
        runtime: CvarDef::new("r_blueWaterWavelength", "", "2.0..=512.0",
            "Dominant crest spacing in metres. Controls wave speed through the native dispersion relation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.wavelength.to_string()),
            |app, value| App::cvar_set_blue_water_wavelength(app, APP_R_BLUEWATERWAVELENGTH.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_wavelength, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.wavelength = v.clamp(2.0, 512.0); }
                }
            }),
            archive: Some((25152, |state| state.video.ocean_settings.blue_water_waves.wavelength.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_wavelength(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.wavelength = Self::parse_cvar_finite(name, value)?.clamp(2.0, 512.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERCHOPPINESS {
        runtime: CvarDef::new("r_blueWaterChoppiness", "", "0.0..=4.0",
            "Horizontal displacement strength. Zero keeps only vertical waves.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.choppiness.to_string()),
            |app, value| App::cvar_set_blue_water_choppiness(app, APP_R_BLUEWATERCHOPPINESS.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_choppiness, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.choppiness = v.clamp(0.0, 4.0); }
                }
            }),
            archive: Some((25153, |state| state.video.ocean_settings.blue_water_waves.choppiness.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_choppiness(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.choppiness = Self::parse_cvar_finite(name, value)?.clamp(0.0, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWINDSPEED {
        runtime: CvarDef::new("r_blueWaterWindSpeed", "", "0.1..=40.0",
            "Reference wind speed in metres per second. Changes the native spectral shape and fine surface roughness; independent of weather wind.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.wind_speed.to_string()),
            |app, value| App::cvar_set_blue_water_wind_speed(app, APP_R_BLUEWATERWINDSPEED.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_wind_speed, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.wind_speed = v.clamp(0.1, 40.0); }
                }
            }),
            archive: Some((25154, |state| state.video.ocean_settings.blue_water_waves.wind_speed.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_wind_speed(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.wind_speed = Self::parse_cvar_finite(name, value)?.clamp(0.1, 40.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }

    APP_CG_WATERWAKE {
        runtime: CvarDef::new("cg_waterwake", "1", "0|1|2",
            "0 off; 1 original JKA water wake on vanilla water; 2 enhanced vanilla wake and enabled Godot water interaction.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.water_wake.to_string()),
            |app, value| App::cvar_set_water_wake(app, APP_CG_WATERWAKE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_water_wake, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.water_wake = mode.min(2);
                }
            }),
            archive: Some((25007, |state| state.video.water_wake.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_water_wake(&mut self, name: &str, value: &str) -> Result<(), String> {
                let mode: u8 = value.trim().parse()
                    .map_err(|_| format!("{name} expects 0, 1 or 2"))?;
                if mode > 2 { return Err(format!("{name} expects 0, 1 or 2")); }
                self.video.water_wake = mode;
                if let Some(session) = self.game_session.as_mut() {
                    session.water_wake_mode = mode;
                }
                // The stored r_oceanInteraction preference remains untouched.
                // Only the renderer-facing copy is gated by mode 2.
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANTYPE {
        runtime: CvarDef::new("r_oceanType", "4", "0..5",
            "Water promotion: 0 stock JKA, 1 Godot ocean, 2 native Blue Water ocean, 3 Hybrid ocean, 4 Blue Water lakes/ponds (subtle relief, no FFT), 5 Gerstner ocean (Hybrid with godot-water-shader's waves, colour and foam, lit by Blue Water).",
            CvarType::Int { min: 0, max: 5 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.ocean_type())),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.set_ocean_type(value as u8);
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if v <= 5 {
                        settings.ocean = v != 0;
                        if v != 0 {
                            // 5 is Hybrid (backend 2) with the Gerstner surface.
                            settings.ocean_settings.backend = if v == 5 { 2 } else { v - 1 };
                            settings.ocean_settings.gerstner_water = v == 5;
                        }
                    }
                }
            }),
            archive: Some((25225, |state| if !state.video.ocean {
                "0".to_owned()
            } else if state.video.ocean_settings.gerstner_active() {
                "5".to_owned()
            } else {
                (state.video.ocean_settings.backend + 1).to_string()
            })),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_GERSTNERWAVESCALE {
        runtime: CvarDef::new("r_gerstnerWaveScale", "1", "0.1..20",
            "Gerstner ocean (r_oceanType 5): godot-water-shader's sea this many times larger. Lengths scale by it and time by its square root, so a larger sea keeps the same shape and slows as real swell does. 1 is the original (0.4 m, 6 m waves). Applies live.",
            CvarType::Float { min: 0.1, max: 20.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.gerstner_wave_scale),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.gerstner_wave_scale = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.gerstner_wave_scale = v.clamp(0.1, 20.0); }
                }
            }),
            archive: Some((25231, |state| state.video.ocean_settings.gerstner_wave_scale.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_GERSTNERFOAM {
        runtime: CvarDef::new("r_gerstnerFoam", "7", "0..7",
            "Gerstner ocean (r_oceanType 5) foam bits: 1 near persistent cascade (15 m), 2 far persistent cascade (30 m), 4 distant compression foam past them (per pixel, no persistence). 3 is the original; 7 all. Distances scale with r_gerstnerWaveScale. Applies live.",
            CvarType::Int { min: 0, max: 7 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.gerstner_foam)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.gerstner_foam = v.clamp(0, 7) as u8;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.gerstner_foam = v.min(7); }
            }),
            archive: Some((25232, |state| state.video.ocean_settings.gerstner_foam.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_GERSTNERFOAMRATE {
        runtime: CvarDef::new("r_gerstnerFoamRate", "60", "0..60",
            "Gerstner ocean (r_oceanType 5): persistent foam simulation steps per second for each cascade, staggered so the near and far cascades rarely step in the same frame. 0 steps every rendered frame (the original). Applies live.",
            CvarType::Float { min: 0.0, max: 60.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.gerstner_foam_rate),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.gerstner_foam_rate = v;
                app.commit_ocean_settings(); Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.gerstner_foam_rate = v.clamp(0.0, 60.0); }
                }
            }),
            archive: Some((25233, |state| state.video.ocean_settings.gerstner_foam_rate.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_GERSTNERWATERCOLOR {
        runtime: CvarDef::new("r_gerstnerWaterColor", "0 0.21568628 0.25490198", "r g b (0..1)",
            "Gerstner ocean (r_oceanType 5) water color, sRGB (the original's 0, 55, 65). Crests blend toward r_gerstnerCrestColor.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.gerstner_water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_gerstner_water_color(app, APP_R_GERSTNERWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_gerstner_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.gerstner_water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25234, |state| { let c = state.video.ocean_settings.gerstner_water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_gerstner_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.gerstner_water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_GERSTNERCRESTCOLOR {
        runtime: CvarDef::new("r_gerstnerCrestColor", "0 0.3133 0.1517", "r g b (0..1)",
            "Gerstner ocean (r_oceanType 5) crest color, sRGB: wave tops blend from r_gerstnerWaterColor toward it (the original's green).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.gerstner_crest_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_gerstner_crest_color(app, APP_R_GERSTNERCRESTCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_gerstner_crest_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.gerstner_crest_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25235, |state| { let c = state.video.ocean_settings.gerstner_crest_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_gerstner_crest_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.gerstner_crest_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANSURFACEPIXELS {
        runtime: CvarDef::new("r_oceanSurfacePixels", "1", "0|1",
            "Diagnostic A/B: 0 skips promoted ocean surface draws (Godot, native Blue Water, Hybrid and infinity); FFT, foam and mesh updates continue. 1 renders normally.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.surface_pixels),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!() };
                if app.video.ocean_settings.surface_pixels != enabled {
                    app.video.ocean_settings.surface_pixels = enabled;
                    app.commit_ocean_settings();
                }
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(enabled) = parse_bool(value) { settings.ocean_settings.surface_pixels = enabled; }
            }),
            archive: Some((25226, |state| (state.video.ocean_settings.surface_pixels as u8).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANADAPTIVEMESH {
        runtime: CvarDef::new("r_oceanAdaptiveMesh", "0", "0..2",
            "0 native clipmap; 1 whole-ring curvature; 2 experimental stitched per-tile LOD with temporal vertex geomorph. Independent screen-space LOD still applies.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.adaptive_mesh)),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh = v.clamp(0, 2) as u8;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.adaptive_mesh = v.min(2); }
            }),
            archive: Some((25375, |state| state.video.ocean_settings.adaptive_mesh.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANADAPTIVEMESHTOLERANCE {
        runtime: CvarDef::new("r_oceanAdaptiveMeshTolerance", "1", "0.25..16",
            "Mode 2 local mesh error tolerance (1 original, 2 twice the allowed projected/normal error, 4 more aggressive). Larger values remove more triangles but may reduce crest fidelity. Modes 0/1 unaffected.",
            CvarType::Float { min: 0.25, max: 16.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.adaptive_mesh_tolerance),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh_tolerance = if v.is_finite() { v.clamp(0.25, 16.0) } else { 1.0 };
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() {
                    settings.ocean_settings.adaptive_mesh_tolerance = v.clamp(0.25, 16.0);
                } }
            }),
            archive: Some((25376, |state| state.video.ocean_settings.adaptive_mesh_tolerance.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANADAPTIVEMESHDEBUG {
        runtime: CvarDef::new("r_oceanAdaptiveMeshDebug", "0", "0|1",
            "Print actual native ocean triangles vs identically PVS-pruned full-detail reference every second. Debug only.",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.adaptive_mesh_debug),
            |app, value| { let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh_debug = v;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding::NONE, implementations: []
    }
    APP_R_OCEANSCREENLODPIXELS {
        runtime: CvarDef::new("r_oceanScreenLodPixels", "0", "0..16",
            "Native/Hybrid screen-space ring LOD. Protects closest 3 rings with adaptive mesh Off; with curvature-adaptive On, coarsens selected rings as requested instead of being overridden. Boundary vertices remain stitched.",
            CvarType::Float { min: 0.0, max: 16.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.screen_lod_pixels),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.screen_lod_pixels = v;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.screen_lod_pixels = v.clamp(0.0, 16.0); } } }),
            archive: Some((25227, |state| state.video.ocean_settings.screen_lod_pixels.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANPIXELDETAIL {
        runtime: CvarDef::new("r_oceanPixelDetail", "0", "0..5",
            "Native Blue Water: 0 full resolution; 1 true 1/2 resolution (2x2); 2 true 1/4 resolution (4x4); 3 distance-aware 2x2; 4 distance-aware 4x4; 5 adaptive 2x2 (full shading only on 2x2 blocks the 2x2 shading cannot reconstruct, see r_oceanPixelContrast). Full-res wave geometry/depth remains.",
            CvarType::Int { min: 0, max: 5 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.pixel_detail_mode as i32),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.pixel_detail_mode = v as u8;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.pixel_detail_mode = v.min(5); } }),
            archive: Some((25228, |state| state.video.ocean_settings.pixel_detail_mode.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANCOSTPROBE {
        runtime: CvarDef::new("r_oceanCostProbe", "0", "0..67108863",
            "Diagnostic A/B, not archived: bitmask stubbing Blue Water surface features to size their GPU cost (changes the image). 1 SSR, 2 foam, 4 glints, 8 wake, 16 rain, 32 refraction, 64 FFT waves, 128 fog, 256 shadows, 512 sky lobe, 1024 hull shelter, 2048 underside, 4096 admission floor only; foam parts 131072 spread reads, 262144 breaking groups, 524288 gale streaks, 1048576 pattern reads; 2097152 value_noise reduced to a fract (noise cost bound); 4194304 wave sample + foam patterns as 8 reads (baked-cache bound); register packing across the reflection march: 8388608 fp16 pack2x16float, 16777216 fp16 wider set (sky, daylight, variances), 33554432 bfloat16 by shifts (not folded by the driver).",
            CvarType::Int { min: 0, max: 67108863 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.surface_cost_probe as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.surface_cost_probe = v as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANSKIPFADEDCASCADES {
        runtime: CvarDef::new("r_oceanSkipFadedCascades", "0", "0|1",
            "Native Blue Water A/B, not archived: 1 skips a wave cascade's texture reads at pixels where it has fully faded with distance (lossless: every use is scaled by its zero detail).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.skip_faded_cascades),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.skip_faded_cascades = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANWAVESTATISTICS {
        runtime: CvarDef::new("r_oceanWaveStatistics", "0", "0|1|2|3",
            "Native Blue Water A/B, not archived (lossy): 1 draws a wave cascade's slopes as their spectral statistics where a pixel's long side has averaged it (grazing far water), skipping its derivative reads; 2 also folds its foam spread read into the foam read; 3 debug: water shows how statistical each cascade is (red finest, green middle, blue largest).",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.wave_statistics)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.wave_statistics = v.clamp(0, 3) as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANPIXELDEBUG {
        runtime: CvarDef::new("r_oceanPixelDebug", "0", "0|1",
            "Native Blue Water diagnostic, not archived: with r_oceanPixelDetail 5, tint the 2x2 blocks that get full shading by why (yellow water edge, blue depth step, red luminance).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.pixel_detail_debug),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.pixel_detail_debug = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUEWATERSSRTILES {
        runtime: CvarDef::new("r_blueWaterSSRTiles", "1", "0|1",
            "Native Blue Water, not archived: 1 builds upstream's reflection tile table so reflection rays whose screen path holds only sky (or seabed below the wave envelope) skip the march, image-identical; 0 traces every ray (A/B).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_ssr_tiles),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_ssr_tiles = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANOPTICSRECEIVERONLY {
        runtime: CvarDef::new("r_oceanOpticsReceiverOnly", "1", "0|1",
            "Native Blue Water with Godot caustics, not archived: 1 runs the optical capture for its caustic receiver output alone when nothing else reads the fogged capture (image-identical); 0 runs the full capture (A/B).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.optics_receiver_only),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.optics_receiver_only = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANFARSHADER {
        runtime: CvarDef::new("r_oceanFarShader", "0", "0..134217727",
            "Native Blue Water A/B, not archived: 0 one shader everywhere; 1 water past the sky walls uses a slimmer shader (no screen-space reflection, refraction, wake or hull shelter; foam at its far-field average); above 1 an explicit stub mask (r_oceanCostProbe bits).",
            CvarType::Int { min: 0, max: 134217727 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.far_shader as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.far_shader = v as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANPIXELDISTANCE {
        runtime: CvarDef::new("r_oceanPixelDistance", "100", "1..2000",
            "Native/Hybrid metres from camera where full-resolution near-water shading transitions to true reduced-resolution material.",
            CvarType::Float { min: 1.0, max: 2000.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.pixel_detail_distance),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.pixel_detail_distance = v;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.pixel_detail_distance = v.clamp(1.0, 2000.0); } } }),
            archive: Some((25229, |state| state.video.ocean_settings.pixel_detail_distance.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANPIXELCONTRAST {
        runtime: CvarDef::new("r_oceanPixelContrast", "0.1", "0.001..0.5",
            "Native/Hybrid r_oceanPixelDetail 5: how far the 2x2-shaded water's luminance may curve across a 2x2 block (share of its level); a 16x16 tile with 8+ such blocks gets full shading. Lower = more full-shaded tiles (closer to Full, slower). Costa: 0.1 = +20% FPS, 0.05 = break-even.",
            CvarType::Float { min: 0.001, max: 0.5 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.pixel_detail_contrast),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.pixel_detail_contrast = v;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| { if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.pixel_detail_contrast = v.clamp(0.001, 0.5); } } }),
            archive: Some((25230, |state| state.video.ocean_settings.pixel_detail_contrast.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANBACKEND {
        runtime: CvarDef::new("r_oceanBackend", "0", "0|1",
            "Legacy compatibility alias: 0 Godot; 1 native Blue Water. Use r_oceanType for Off/Hybrid.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.backend != 0)),
            |app, value| {
                let CvarValue::Int(backend) = value else { unreachable!() };
                let backend = backend as u8;
                if app.video.ocean_settings.backend != backend || !app.video.ocean {
                    app.set_ocean_type(backend + 1);
                }
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANSKYEXTEND {
        runtime: CvarDef::new("r_oceanSkyExtend", "2", "0|1|2",
            "0 original water; 1 camera-relative 3D sky extension; 2 3D clipmap plus mesh-free far ocean. Coastal unchanged.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.ocean_sky_extend as i32),
            |app, value| {
                let CvarValue::Int(mode) = value else { unreachable!() };
                let mode = mode as u8;
                if app.ocean_sky_extend != mode {
                    app.ocean_sky_extend = mode;
                    app.render_command(RenderCommand::SetOceanSkyExtend(mode));
                }
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTAL {
        runtime: CvarDef::new("r_coastal", "0", "0|1",
            "Opt-in GPU shallow-bank solver for one promoted pool; r_coastalPool selects its 1-based pool number (0 automatic).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.coastal_enabled),
            |app, value| {
                let CvarValue::Bool(enabled)=value else {unreachable!()};
                app.coastal_enabled=enabled;app.coastal_live_sent=None;Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTALPOOL {
        runtime: CvarDef::new("r_coastalPool", "0", "0..8",
            "Live coastal pool: 0 selects an eligible shallow-bank pool automatically; 1..8 match the debug Pool numbers.",
            CvarType::Int {min:0,max:8}, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.coastal_pool.into()),
            |app, value| {
                let CvarValue::Int(pool)=value else {unreachable!()};
                app.coastal_pool=pool as u8;app.coastal_live_sent=None;Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTALDEBUG {
        runtime: CvarDef::new(
                "r_coastalDebug",
                "0",
                "0..3",
                "Phase 2.5 promoted-water evaluation: 0 off, 1 treatment tint, 2 boundary faces, 3 bathymetry/depth cells.",
                CvarType::Int { min: 0, max: 3 },
                CvarFlags::NONE,
                LatchScope::None,
                |app: &App| CvarValue::Int(app.coastal_debug_mode.into()),
                |app, value| {
                    let CvarValue::Int(mode) = value else { unreachable!() };
                    app.coastal_debug_mode = mode as u8;
                    app.coastal_debug_sent = None;
                    Ok(())
                },
            ),
        config: CvarBinding::NONE,
        implementations: []
    }
    // A single switch for the opt-in visual corrections; does not mutate
    // the individual A/B CVar values or the simulation state.
    APP_R_OCEANVISUALFIXES {
        runtime: CvarDef::new("r_oceanVisualFixes", "0", "0..2",
            "0 original; 1 balanced whitecaps + lit foam + promoted-water body; 2 stronger Godot-like look. No FFT reset.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.visual_fixes as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.visual_fixes = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.visual_fixes = v.min(2); }
            }),
            archive: Some((25018, |state| state.video.ocean_settings.visual_fixes.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANFOAMSOURCE {
        runtime: CvarDef::new("r_oceanFoamSource", "0", "0..2",
            "FFT Jacobian foam generation: 0 original, 1 slightly earlier crests and 1.3x growth, 2 stronger earlier crests and 1.6x growth. Bundle selects minimum level.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.foam_source_mode as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.foam_source_mode = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.foam_source_mode = v.min(2); }
            }),
            archive: Some((25019, |state| state.video.ocean_settings.foam_source_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    // Which swell controls follow the master amplitude (see ocean::links).
    APP_R_OCEANLINKS {
        runtime: CvarDef::new("r_oceanLinks", "127", "0..127",
            "Bitmask of ocean controls that follow the swell amplitude: 1 wavelength, 2 speed, 4 choppiness, 8 wind chop, 16 foam, 32 spray, 64 roughness. 127 all linked (default), 0 none.",
            CvarType::Int { min: 0, max: 127 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.links as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.links = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.links = v & crate::ocean::links::ALL; }
            }),
            archive: Some((25020, |state| state.video.ocean_settings.links.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    // Independent surface-shading A/B controls. Defaults retain the current look.
    APP_R_OCEANFOAMDETAIL {
        runtime: CvarDef::new("r_oceanFoamDetail", "1", "0|1",
            "0 keep whitecaps when normals LOD out, 1 (default) sample all cascades at any distance like Godot and keep far normals.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.foam_detail_mode as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.foam_detail_mode = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.foam_detail_mode = v.min(1); }
            }),
            archive: Some((25010, |state| state.video.ocean_settings.foam_detail_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANFOAMDRIFT {
        runtime: CvarDef::new("r_oceanFoamDrift", "0", "0..2",
            "Rolls the persistent FFT whitecap foam downwind at this fraction of wind speed, so foam is not pinned to the tile that formed it (the visible 88 m repeat). 0 off, 0.2 is the Unity port's default.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_drift.to_string()),
            |app, value| App::cvar_set_foam_drift(app, APP_R_OCEANFOAMDRIFT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_foam_drift, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_drift = v.clamp(0.0, 2.0); }
                }
            }),
            archive: Some((25034, |state| state.video.ocean_settings.foam_drift.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_foam_drift(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_drift = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANFOAMACTIVITY {
        runtime: CvarDef::new("r_oceanFoamActivity", "0", "0..3",
            "Promoted water foam attenuation: 0 original, 1 omit texture heuristic, 2 omit amplitude gate, 3 omit both. No FFT reset.",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.foam_activity_mode as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.foam_activity_mode = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.foam_activity_mode = v.min(3); }
            }),
            archive: Some((25011, |state| state.video.ocean_settings.foam_activity_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANFOAMGAIN {
        runtime: CvarDef::new("r_oceanFoamGain", "1", "0..16",
            "Post-simulation breaker foam strength before smoothstep. 1 unchanged, 4 or 8 shows marginal foam.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_gain.to_string()),
            |app, value| App::cvar_set_ab_foam_gain(app, APP_R_OCEANFOAMGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_foam_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_gain = v.clamp(0.0, 16.0); }
                }
            }),
            archive: Some((25012, |state| state.video.ocean_settings.foam_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_foam_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 16.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANCOLORMODE {
        runtime: CvarDef::new("r_oceanColorMode", "0", "0..3",
            "Color A/B: 0 original, 1 override authored base with configured ocean color, 2 opaque lit water, 3 both.",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.color_mode as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.color_mode = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.color_mode = v.min(3); }
            }),
            archive: Some((25014, |state| state.video.ocean_settings.color_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANFOAMBRIGHTNESS {
        runtime: CvarDef::new("r_oceanFoamBrightness", "1", "0..8",
            "Multiplier on linear foam RGB in ocean surface illumination, 1 unchanged.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_brightness.to_string()),
            |app, value| App::cvar_set_ab_foam_brightness(app, APP_R_OCEANFOAMBRIGHTNESS.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_foam_brightness, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_brightness = v.clamp(0.0, 8.0); }
                }
            }),
            archive: Some((25015, |state| state.video.ocean_settings.foam_brightness.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_foam_brightness(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_brightness = Self::parse_cvar_finite(name, value)?.clamp(0.0, 8.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANSUNGAIN {
        runtime: CvarDef::new("r_oceanSunGain", "1", "0..8",
            "Multiplier on direct sun diffuse/specular contribution (not sky ambient), 1 unchanged.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.sun_gain.to_string()),
            |app, value| App::cvar_set_ab_sun_gain(app, APP_R_OCEANSUNGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_sun_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.sun_gain = v.clamp(0.0, 8.0); }
                }
            }),
            archive: Some((25016, |state| state.video.ocean_settings.sun_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_sun_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.sun_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 8.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANFOAMDEBUG {
        runtime: CvarDef::new("r_oceanFoamDebug", "0", "0..3",
            "Foam diagnostic: 0 shaded water, 1 raw sampled FFT foam, 2 after promoted gates, 3 final breaker + windrows.",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.foam_debug as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.foam_debug = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.foam_debug = v.min(3); }
            }),
            archive: Some((25017, |state| state.video.ocean_settings.foam_debug.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTION {
        runtime: CvarDef::new("r_oceanInteraction", "", "0|1",
            "Nearby player water deformation; inherits r_oceanUpdates.", CvarType::String,
            CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.ocean_settings.interaction)),
            |app, value| App::cvar_set_ocean_interaction(app, APP_R_OCEANINTERACTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.ocean_settings.interaction = parse_bool(value).unwrap_or(settings.ocean_settings.interaction);
            }),
            archive: Some((25000, |state| bool_value(state.video.ocean_settings.interaction))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction = Self::parse_cvar_bool(name, value)?;
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONSTRENGTH {
        runtime: CvarDef::new("r_oceanInteractionStrength", "", "nonnegative finite number",
            "Nearby water interaction force scale.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_strength.to_string()),
            |app, value| App::cvar_set_ocean_interaction_strength(app, APP_R_OCEANINTERACTIONSTRENGTH.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_strength, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_strength = v.max(0.0); } }
            }),
            archive: Some((25001, |state| state.video.ocean_settings.interaction_strength.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_strength(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction_strength = Self::parse_cvar_finite(name,value)?.max(0.0);
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONFX {
        runtime: CvarDef::new("r_oceanInteractionFx", "", "0..5",
            "Player water FX: 0 off, 1 entry droplets, 2 wake spray, 3 both, 4 entry jets, 5 all.", CvarType::String,
            CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_splash_mode.to_string()),
            |app, value| App::cvar_set_ocean_interaction_fx(app, APP_R_OCEANINTERACTIONFX.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_fx, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.parse::<u8>() {
                    settings.ocean_settings.interaction_splash_mode = mode.min(5);
                }
            }),
            archive: Some((25006, |state| state.video.ocean_settings.interaction_splash_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_fx(&mut self, name: &str, value: &str) -> Result<(), String> {
                let mode = value.parse::<u8>()
                    .map_err(|_| format!("{name} must be in the range 0..5"))?;
                if mode > 5 {
                    return Err(format!("{name} must be in the range 0..5"));
                }
                self.video.ocean_settings.interaction_splash_mode = mode;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONLIFETIME {
        runtime: CvarDef::new("r_oceanInteractionLifetime", "", "0.1..60 seconds",
            "Seconds after the last moving input before retiring the local field.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_lifetime.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONLIFETIME.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_lifetime = v.clamp(0.1,60.0); } }
            }),
            archive: Some((25002, |state| state.video.ocean_settings.interaction_lifetime.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_falloff(&mut self, name: &str, value: &str) -> Result<(), String> {
                let v = Self::parse_cvar_finite(name,value)?;
                match name.to_ascii_lowercase().as_str() {
                    "r_oceaninteractionlifetime" => self.video.ocean_settings.interaction_lifetime = v.clamp(0.1,60.0),
                    "r_oceaninteractionfade" => self.video.ocean_settings.interaction_fade = v.clamp(0.0,60.0),
                    "r_oceaninteractiondamping" => self.video.ocean_settings.interaction_damping = v.max(0.0),
                    "r_oceaninteractionedgefade" => self.video.ocean_settings.interaction_edge_fade = v.clamp(0.0,256.0),
                    _ => unreachable!(),
                }
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONFADE {
        runtime: CvarDef::new("r_oceanInteractionFade", "", "0..60 seconds",
            "Duration of the final smooth fade before field retirement; zero disables temporal fading.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_fade.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONFADE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_fade = v.clamp(0.0,60.0); } }
            }),
            archive: Some((25003, |state| state.video.ocean_settings.interaction_fade.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONDAMPING {
        runtime: CvarDef::new("r_oceanInteractionDamping", "", "nonnegative finite number",
            "Velocity decay per second; zero disables bulk damping; height decay is 0.175 times this rate.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_damping.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONDAMPING.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_damping = v.max(0.0); } }
            }),
            archive: Some((25004, |state| state.video.ocean_settings.interaction_damping.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONEDGEFADE {
        runtime: CvarDef::new("r_oceanInteractionEdgeFade", "", "0..256 world units",
            "Width of the local-window edge fade; sponge width is 1.5 times this value; zero disables both.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_edge_fade.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONEDGEFADE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_edge_fade = v.clamp(0.0,256.0); } }
            }),
            archive: Some((25005, |state| state.video.ocean_settings.interaction_edge_fade.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONDEBUG {
        runtime: CvarDef::new("r_oceanInteractionDebug", "", "0|1",
            "Show nearby interaction height (red/blue) and slopes (green).", CvarType::String,
            CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.ocean_settings.interaction_debug)),
            |app, value| App::cvar_set_ocean_interaction_debug(app, APP_R_OCEANINTERACTIONDEBUG.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_debug, None),
        config: CvarBinding::NONE,
        implementations: [impl App {
            fn cvar_set_ocean_interaction_debug(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction_debug = Self::parse_cvar_bool(name,value)?;
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEAN {
        runtime: CvarDef::new(
                "r_ocean",
                "",
                "0|1",
                "GPU ocean. Enabling on a world loaded without ocean requires applying video settings.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ocean(app)),
                |app, value| App::cvar_set_r_ocean(app, APP_R_OCEAN.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_ocean, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean = parse_bool(value).unwrap_or(settings.ocean)
                }),
                archive: Some((93, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ocean(&self) -> String {
                    bool_value(self.video.ocean)
                }
                fn cvar_set_r_ocean(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session.ocean_interaction_enabled = self.video.ocean && self.video.ocean_settings.interaction && self.video.water_wake == 2;
                        }
                        // OFF releases the FFT resources immediately, and ON can be
                        // restored live only when the loaded world already carries its
                        // promoted ocean clipmap.
                        if !self.video.ocean || self.applied_ocean {
                            self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANAUTHORING {
        runtime: CvarDef::new(
                "r_oceanAuthoring",
                "",
                "custom",
                "Controls r_oceanAuthoring.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanauthoring(app)),
                |app, value| {
                    App::cvar_set_r_oceanauthoring(app, APP_R_OCEANAUTHORING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanauthoring, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    let v: Vec<_> = value.split_whitespace().collect();
                    if v.len() == 11 {
                        let floats: Option<Vec<f32>> = v[..10].iter().map(|s| s.parse().ok()).collect();
                        if let (Some(f), Ok(seed)) = (floats, v[10].parse()) {
                            settings.ocean_settings.authored = crate::ocean::OceanAuthoring {
                                amplitude: f[0],
                                wavelength: f[1],
                                speed: f[2],
                                direction: f[3],
                                steepness: f[4],
                                slosh: f[5],
                                wind_chop: f[6],
                                foam: f[7],
                                foam_lifetime: f[8],
                                spray: f[9],
                                seed,
                            }
                            .sanitize();
                        }
                    }
                }),
                archive: Some((10419, |state| {
                    let settings = state.video;
                    let a = settings.ocean_settings.authored;
                    format!(
                        "{} {} {} {} {} {} {} {} {} {} {}",
                        a.amplitude,
                        a.wavelength,
                        a.speed,
                        a.direction,
                        a.steepness,
                        a.slosh,
                        a.wind_chop,
                        a.foam,
                        a.foam_lifetime,
                        a.spray,
                        a.seed
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanauthoring(&self) -> String {
                    {
                        let a = self.video.ocean_settings.authored;
                        format!(
                            "{} {} {} {} {} {} {} {} {} {} {}",
                            a.amplitude,
                            a.wavelength,
                            a.speed,
                            a.direction,
                            a.steepness,
                            a.slosh,
                            a.wind_chop,
                            a.foam,
                            a.foam_lifetime,
                            a.spray,
                            a.seed,
                        )
                    }
                }
                fn cvar_set_r_oceanauthoring(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let words: Vec<&str> = value.split_whitespace().collect();
                        let expected = || format!("{name}: expected ten numbers and a seed");
                        if words.len() != 11 {
                            return Err(expected());
                        }
                        let mut f = [0.0_f32; 10];
                        for (slot, word) in f.iter_mut().zip(&words) {
                            *slot = word
                                .parse::<f32>()
                                .ok()
                                .filter(|number| number.is_finite())
                                .ok_or_else(expected)?;
                        }
                        let seed = words[10].parse().map_err(|_| expected())?;
                        self.video.ocean_settings.authored = crate::ocean::OceanAuthoring {
                            amplitude: f[0],
                            wavelength: f[1],
                            speed: f[2],
                            direction: f[3],
                            steepness: f[4],
                            slosh: f[5],
                            wind_chop: f[6],
                            foam: f[7],
                            foam_lifetime: f[8],
                            spray: f[9],
                            seed,
                        }
                        .sanitize();
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE1 {
        runtime: CvarDef::new(
                "r_oceanCascade1",
                "",
                "custom",
                "Controls r_oceanCascade1.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade1(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade1(app, APP_R_OCEANCASCADE1.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade1, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[0] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[0])
                    }
                }),
                archive: Some((103, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[0].tile_length[0],
                        settings.ocean_settings.cascades[0].tile_length[1],
                        settings.ocean_settings.cascades[0].displacement_scale,
                        settings.ocean_settings.cascades[0].normal_scale,
                        settings.ocean_settings.cascades[0].wind_speed,
                        settings.ocean_settings.cascades[0].wind_direction,
                        settings.ocean_settings.cascades[0].fetch_length,
                        settings.ocean_settings.cascades[0].swell,
                        settings.ocean_settings.cascades[0].spread,
                        settings.ocean_settings.cascades[0].detail,
                        settings.ocean_settings.cascades[0].whitecap,
                        settings.ocean_settings.cascades[0].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade1(&self) -> String {
                    let name = "r_oceancascade1";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade1".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE2 {
        runtime: CvarDef::new(
                "r_oceanCascade2",
                "",
                "custom",
                "Controls r_oceanCascade2.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade2(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade2(app, APP_R_OCEANCASCADE2.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade2, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[1] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[1])
                    }
                }),
                archive: Some((104, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[1].tile_length[0],
                        settings.ocean_settings.cascades[1].tile_length[1],
                        settings.ocean_settings.cascades[1].displacement_scale,
                        settings.ocean_settings.cascades[1].normal_scale,
                        settings.ocean_settings.cascades[1].wind_speed,
                        settings.ocean_settings.cascades[1].wind_direction,
                        settings.ocean_settings.cascades[1].fetch_length,
                        settings.ocean_settings.cascades[1].swell,
                        settings.ocean_settings.cascades[1].spread,
                        settings.ocean_settings.cascades[1].detail,
                        settings.ocean_settings.cascades[1].whitecap,
                        settings.ocean_settings.cascades[1].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade2(&self) -> String {
                    let name = "r_oceancascade2";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade2".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE3 {
        runtime: CvarDef::new(
                "r_oceanCascade3",
                "",
                "custom",
                "Controls r_oceanCascade3.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade3(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade3(app, APP_R_OCEANCASCADE3.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade3, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[2] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[2])
                    }
                }),
                archive: Some((105, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[2].tile_length[0],
                        settings.ocean_settings.cascades[2].tile_length[1],
                        settings.ocean_settings.cascades[2].displacement_scale,
                        settings.ocean_settings.cascades[2].normal_scale,
                        settings.ocean_settings.cascades[2].wind_speed,
                        settings.ocean_settings.cascades[2].wind_direction,
                        settings.ocean_settings.cascades[2].fetch_length,
                        settings.ocean_settings.cascades[2].swell,
                        settings.ocean_settings.cascades[2].spread,
                        settings.ocean_settings.cascades[2].detail,
                        settings.ocean_settings.cascades[2].whitecap,
                        settings.ocean_settings.cascades[2].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade3(&self) -> String {
                    let name = "r_oceancascade3";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade3(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade3".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCAUSTICSMAXDISTANCE {
        runtime: CvarDef::new("r_oceanCausticsMaxDistance", "80", "0..500 metres",
            "Godot and Hybrid Godot caustics: camera-to-receiver range with fade over outer 25%; 0 disables. Native Blue Water has its own height cull.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.optics.caustic_max_distance_m.to_string()),
            |app, value| App::cvar_set_r_oceancausticsmaxdistance(app, APP_R_OCEANCAUSTICSMAXDISTANCE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_r_oceancausticsmaxdistance, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.optics.caustic_max_distance_m = v.clamp(0.0, 500.0); }
                }
            }),
            archive: Some((25216, |state| state.video.ocean_settings.optics.caustic_max_distance_m.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_r_oceancausticsmaxdistance(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.optics.caustic_max_distance_m = Self::parse_cvar_finite(name, value)?.clamp(0.0, 500.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANCAUSTICS {
        runtime: CvarDef::new(
                "r_oceanCaustics",
                "",
                "0..8",
                "Experimental FFT curvature caustics; not shadow-aware yet.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancaustics(app)),
                |app, value| {
                    App::cvar_set_r_oceancaustics(app, APP_R_OCEANCAUSTICS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancaustics, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10417, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.caustics)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancaustics(&self) -> String {
                    self.video.ocean_settings.optics.caustics.to_string()
                }
                fn cvar_set_r_oceancaustics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANDEPTHDARKENING {
        runtime: CvarDef::new(
                "r_oceanDepthDarkening",
                "",
                "0.01..32",
                "Underwater sunlight penetration multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceandepthdarkening(app)),
                |app, value| {
                    App::cvar_set_r_oceandepthdarkening(
                        app,
                        APP_R_OCEANDEPTHDARKENING.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceandepthdarkening, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10415, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.depth_darkening)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceandepthdarkening(&self) -> String {
                    self.video.ocean_settings.optics.depth_darkening.to_string()
                }
                fn cvar_set_r_oceandepthdarkening(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANFOAMCOLOR {
        runtime: CvarDef::new(
                "r_oceanFoamColor",
                "",
                "custom",
                "Controls r_oceanFoamColor.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanfoamcolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanfoamcolor(app, APP_R_OCEANFOAMCOLOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanfoamcolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.foam_color =
                        parse_vec3(value).unwrap_or(settings.ocean_settings.foam_color);
                }),
                archive: Some((102, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4}",
                        settings.ocean_settings.foam_color[0],
                        settings.ocean_settings.foam_color[1],
                        settings.ocean_settings.foam_color[2]
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanfoamcolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.foam_color;
                        format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanfoamcolor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.foam_color = Self::parse_cvar_floats(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANFOGCOLOR {
        runtime: CvarDef::new(
                "r_oceanFogColor",
                "",
                "R G B (0..1)",
                "Water scattering tint.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanfogcolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanfogcolor(app, APP_R_OCEANFOGCOLOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanfogcolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.optics.fog_color =
                            parse_vec3(value).unwrap_or(settings.ocean_settings.optics.fog_color)
                    }
                }),
                archive: Some((10412, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{} {} {}", o.fog_color[0], o.fog_color[1], o.fog_color[2])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanfogcolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.optics.fog_color;
                        format!("{} {} {}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanfogcolor(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let values = value
                            .split_whitespace()
                            .map(str::parse::<f32>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| "Expected three finite color components".to_owned())?;
                        if values.len() != 3 || !values.iter().all(|v| v.is_finite()) {
                            return Err("Expected three finite color components".to_owned());
                        }
                        self.video.ocean_settings.optics.fog_color = [values[0], values[1], values[2]];
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANTRANSPARENCY {
        runtime: CvarDef::new(
                "r_oceanTransparency",
                "",
                "1..65536",
                "Water transparency: how far light travels through the water before it is absorbed, in game units. Lower is murkier, above and below the surface.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceantransparency(app)),
                |app, value| {
                    App::cvar_set_r_oceantransparency(
                        app,
                        APP_R_OCEANTRANSPARENCY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceantransparency, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10413, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.transparency)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceantransparency(&self) -> String {
                    self.video.ocean_settings.optics.transparency.to_string()
                }
                fn cvar_set_r_oceantransparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANMAPSIZE {
        runtime: CvarDef::new(
                "r_oceanMapSize",
                "",
                "custom",
                "Controls r_oceanMapSize.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanmapsize(app)),
                |app, value| {
                    App::cvar_set_r_oceanmapsize(app, APP_R_OCEANMAPSIZE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanmapsize, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<u32>() {
                        settings.ocean_settings.map_size = v;
                    }
                }),
                archive: Some((94, |state| {
                    let settings = state.video;
                    format!("{}", settings.ocean_settings.map_size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanmapsize(&self) -> String {
                    self.video.ocean_settings.map_size.to_string()
                }
                fn cvar_set_r_oceanmapsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 128|256|512|1024"))?;
                        if !matches!(requested, 128 | 256 | 512 | 1024) {
                            return Err(format!("{name}: expected 128|256|512|1024"));
                        }
                        self.video.ocean_settings.map_size = requested;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANMESHQUALITY {
        runtime: CvarDef::new(
                "r_oceanMeshQuality",
                "",
                "custom",
                "Controls r_oceanMeshQuality.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanmeshquality(app)),
                |app, value| {
                    App::cvar_set_r_oceanmeshquality(
                        app,
                        APP_R_OCEANMESHQUALITY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanmeshquality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<u8>() {
                        settings.ocean_settings.mesh_quality = v;
                    }
                }),
                archive: Some((95, |state| {
                    let settings = state.video;
                    format!("{}", settings.ocean_settings.mesh_quality)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanmeshquality(&self) -> String {
                    self.video.ocean_settings.mesh_quality.to_string()
                }
                fn cvar_set_r_oceanmeshquality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.mesh_quality =
                            match value.trim().to_ascii_lowercase().as_str() {
                                "0" | "low" => 0,
                                "1" | "high" => 1,
                                _ => return Err(format!("{name}: expected 0|1 or low|high")),
                            };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANNORMALSTRENGTH {
        runtime: CvarDef::new(
                "r_oceanNormalStrength",
                "",
                "custom",
                "Controls r_oceanNormalStrength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceannormalstrength(app)),
                |app, value| {
                    App::cvar_set_r_oceannormalstrength(
                        app,
                        APP_R_OCEANNORMALSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceannormalstrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.normal_strength = v;
                        }
                    }
                }),
                archive: Some((98, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.ocean_settings.normal_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceannormalstrength(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.normal_strength)
                }
                fn cvar_set_r_oceannormalstrength(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.ocean_settings.normal_strength = Self::parse_cvar_finite(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANREFRACTION {
        runtime: CvarDef::new(
                "r_oceanRefraction",
                "",
                "0..0.15",
                "Wave distortion of transmitted scene color.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanrefraction(app)),
                |app, value| {
                    App::cvar_set_r_oceanrefraction(
                        app,
                        APP_R_OCEANREFRACTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanrefraction, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10416, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.refraction)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanrefraction(&self) -> String {
                    self.video.ocean_settings.optics.refraction.to_string()
                }
                fn cvar_set_r_oceanrefraction(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANROUGHNESS {
        runtime: CvarDef::new(
                "r_oceanRoughness",
                "",
                "custom",
                "Controls r_oceanRoughness.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanroughness(app)),
                |app, value| {
                    App::cvar_set_r_oceanroughness(app, APP_R_OCEANROUGHNESS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanroughness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.roughness = v;
                        }
                    }
                }),
                archive: Some((97, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.ocean_settings.roughness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanroughness(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.roughness)
                }
                fn cvar_set_r_oceanroughness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.roughness = Self::parse_cvar_finite(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANSEASPRAY {
        runtime: CvarDef::new(
                "r_oceanSeaSpray",
                "",
                "custom",
                "Controls r_oceanSeaSpray.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanseaspray(app)),
                |app, value| {
                    App::cvar_set_r_oceanseaspray(app, APP_R_OCEANSEASPRAY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanseaspray, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.sea_spray =
                        parse_bool(value).unwrap_or(settings.ocean_settings.sea_spray);
                }),
                archive: Some((99, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.sea_spray))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanseaspray(&self) -> String {
                    bool_value(self.video.ocean_settings.sea_spray)
                }
                fn cvar_set_r_oceanseaspray(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.sea_spray = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANUNDERWATERCULL {
        runtime: CvarDef::new(
                "r_oceanUnderwaterCull",
                "",
                "0..32",
                "Cull geometry that water absorption hides, as an absorption-distance multiple (floored at 2): everything past it (min 256) when underwater, and anything under that much water when viewed from above. Zero disables.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanunderwatercull(app)),
                |app, value| {
                    App::cvar_set_r_oceanunderwatercull(
                        app,
                        APP_R_OCEANUNDERWATERCULL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanunderwatercull, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10418, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.underwater_cull)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanunderwatercull(&self) -> String {
                    self.video.ocean_settings.optics.underwater_cull.to_string()
                }
                fn cvar_set_r_oceanunderwatercull(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANUPDATES {
        runtime: CvarDef::new(
                "r_oceanUpdates",
                "",
                "custom",
                "Controls r_oceanUpdates.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanupdates(app)),
                |app, value| {
                    App::cvar_set_r_oceanupdates(app, APP_R_OCEANUPDATES.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanupdates, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.updates_per_second = v;
                        }
                    }
                }),
                archive: Some((96, |state| {
                    let settings = state.video;
                    format!("{:.2}", settings.ocean_settings.updates_per_second)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanupdates(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.updates_per_second)
                }
                fn cvar_set_r_oceanupdates(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.updates_per_second =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 60.0);
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANDYNAMICUPDATES {
        runtime: CvarDef::new(
            "r_oceanDynamicUpdates", "0", "0|1",
            "0 original; 1 near two ocean mesh LODs sample the live 48 Hz FFT, third LOD uses a 32 Hz cached displacement, outer LODs 24 Hz; wind foam appearance follows the same ring rates. FFT cascades remain synchronized.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.dynamic_update_rates),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!() };
                app.video.ocean_settings.dynamic_update_rates = enabled;
                app.commit_ocean_settings();
                Ok(())
            },
        ),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.ocean_settings.dynamic_update_rates =
                    parse_bool(value).unwrap_or(settings.ocean_settings.dynamic_update_rates);
            }),
            archive: Some((25030, |state| u8::from(state.video.ocean_settings.dynamic_update_rates).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_OCEANWINDFOAMSYNC {
        runtime: CvarDef::new(
            "r_oceanWindFoamSync", "1", "0|1",
            "1 update wind-foam transport only when the finest FFT cascade publishes its new normal/foam field; 0 legacy every-render-frame update.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.wind_foam_sync),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!() };
                app.video.ocean_settings.wind_foam_sync = enabled;
                app.commit_ocean_settings();
                Ok(())
            },
        ),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.ocean_settings.wind_foam_sync =
                    parse_bool(value).unwrap_or(settings.ocean_settings.wind_foam_sync);
            }),
            archive: Some((25031, |state| u8::from(state.video.ocean_settings.wind_foam_sync).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_OCEANWATERCOLOR {
        runtime: CvarDef::new(
                "r_oceanWaterColor",
                "",
                "custom",
                "Controls r_oceanWaterColor.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanwatercolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanwatercolor(
                        app,
                        APP_R_OCEANWATERCOLOR.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanwatercolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.water_color =
                        parse_vec3(value).unwrap_or(settings.ocean_settings.water_color);
                }),
                archive: Some((101, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4}",
                        settings.ocean_settings.water_color[0],
                        settings.ocean_settings.water_color[1],
                        settings.ocean_settings.water_color[2]
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanwatercolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.water_color;
                        format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanwatercolor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.water_color = Self::parse_cvar_floats(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANWINDFOAM {
        runtime: CvarDef::new(
                "r_oceanWindFoam",
                "",
                "custom",
                "Controls r_oceanWindFoam.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanwindfoam(app)),
                |app, value| {
                    App::cvar_set_r_oceanwindfoam(app, APP_R_OCEANWINDFOAM.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanwindfoam, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.wind_foam_streaks =
                        parse_bool(value).unwrap_or(settings.ocean_settings.wind_foam_streaks);
                }),
                archive: Some((100, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.wind_foam_streaks))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanwindfoam(&self) -> String {
                    bool_value(self.video.ocean_settings.wind_foam_streaks)
                }
                fn cvar_set_r_oceanwindfoam(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.wind_foam_streaks = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANOPTICSCULL {
        runtime: CvarDef::new(
                "r_oceanOpticsCull",
                "",
                "0|1",
                "Skip the underwater optics capture entirely when the camera is dry and no ocean surface is on screen. Nothing can sample it then.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticscull(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticscull(app, APP_R_OCEANOPTICSCULL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticscull, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.cull =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.cull);
                }),
                archive: Some((10424, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.cull))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticscull(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.cull)
                }
                fn cvar_set_r_oceanopticscull(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.cull = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANOPTICSSCISSOR {
        runtime: CvarDef::new(
                "r_oceanOpticsScissor",
                "",
                "0|1",
                "Restrict the underwater optics capture to the screen rectangle the visible water covers instead of the whole framebuffer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticsscissor(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticsscissor(app, APP_R_OCEANOPTICSSCISSOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticsscissor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.scissor =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.scissor);
                }),
                archive: Some((10425, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.scissor))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticsscissor(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.scissor)
                }
                fn cvar_set_r_oceanopticsscissor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.scissor = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANSIMWHENHIDDEN {
        runtime: CvarDef::new(
                "r_oceanSimWhenHidden",
                "",
                "0|1",
                "Keep advancing the FFT wave simulation even while no water is visible. Off freezes the field, which costs nothing and cannot be seen.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceansimwhenhidden(app)),
                |app, value| {
                    App::cvar_set_r_oceansimwhenhidden(app, APP_R_OCEANSIMWHENHIDDEN.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceansimwhenhidden, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.simulate_when_hidden =
                        parse_bool(value).unwrap_or(settings.ocean_settings.simulate_when_hidden);
                }),
                archive: Some((10426, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.simulate_when_hidden))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceansimwhenhidden(&self) -> String {
                    bool_value(self.video.ocean_settings.simulate_when_hidden)
                }
                fn cvar_set_r_oceansimwhenhidden(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.simulate_when_hidden = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }


    APP_R_OCEANOPTICSMASKS {
        runtime: CvarDef::new(
                "r_oceanOpticsMasks",
                "",
                "0|1",
                "Honour water outline masks in the underwater optics capture. Off is visually wrong on hollow water; it exists to measure the mask cost.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticsmasks(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticsmasks(app, APP_R_OCEANOPTICSMASKS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticsmasks, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.masks =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.masks);
                }),
                archive: Some((10427, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.masks))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticsmasks(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.masks)
                }
                fn cvar_set_r_oceanopticsmasks(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.masks = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

}
