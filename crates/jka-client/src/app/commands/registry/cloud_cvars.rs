//! Cloud cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_BLUECLOUDDEBUG {
        runtime: CvarDef::new("r_blueCloudDebug", "0", "0..5",
            "0 normal; 1 cloud opacity without lighting/air; 2 sun transmittance; 3 selected Beer cascade; 4 Beer UV coverage (green valid, red outside); 5 copied scene depth. Diagnostics are not performance comparisons.",
            CvarType::Int { min: 0, max: 5 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_debug as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_debug = value as u8;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDFREEZE {
        runtime: CvarDef::new("r_blueCloudFreeze", "0", "0..1",
            "Freeze native atmosphere wind/time and weather simulation at the current frame; camera remains movable. Also enables still-frame convergence, so do not benchmark FPS while frozen.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_freeze as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_freeze = value != 0;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDSHADOWMODE {
        runtime: CvarDef::new("r_blueCloudShadowMode", "0", "0..1",
            "0 native; 1 bypass distant Beer attenuation in cloud lighting/shafts (diagnostic). Shadow-map generation and ground shadows continue.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_shadow_mode as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_shadow_mode = value as u8;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDSUNSTEPS {
        runtime: CvarDef::new("r_blueCloudSunSteps", "-1", "-1..8",
            "Local sunlight march samples: -1 native tier; 0..8 override. Lower values trade small cloud lighting detail for speed; Beer shadows remain.",
            CvarType::Int { min: -1, max: 8 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_sun_steps as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_sun_steps = value;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDCOMPOSITE {
        runtime: CvarDef::new("r_blueCloudComposite", "1", "0..1",
            "0 hide the cloud overlay while retaining cloud march, environment and shadows; 1 normal. Isolates ocean capture/order; not an ocean-order fix or full cloud-cost benchmark.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_composite as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_composite = value != 0;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDALTITUDEREBASE {
        runtime: CvarDef::new("r_blueCloudAltitudeRebase", "0", "0..1",
            "Experimental negative-altitude A/B: 0 current placement; 1 translate all resolved cloud layers once into the lowered reference. Haze/rain particles/lightning are unchanged; not a complete atmosphere fix.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.blue_water_environment.cloud_altitude_rebase as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.video.blue_water_environment.cloud_altitude_rebase = value != 0;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUECLOUDSHADOWFAR {
        runtime: CvarDef::new("r_blueCloudShadowFar", "0", "0..200000",
            "Beer cascade fitting distance in metres: 0 native camera; try 10000/60000/200000. Independent fitting projection preserves scene depth. Larger ranges reduce shadow texel density.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.blue_water_environment.cloud_shadow_far.to_string()),
            |app, value| App::cvar_set_bluecloud_shadow_far(app, APP_R_BLUECLOUDSHADOWFAR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_bluecloud_shadow_far, None),
        config: CvarBinding::NONE,
        implementations: [impl App {
            fn cvar_set_bluecloud_shadow_far(app: &mut Self, name: &str, value: &str) -> Result<(), String> {
                app.video.blue_water_environment.cloud_shadow_far = App::parse_cvar_finite(name, value)?.clamp(0.0, 200000.0);
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }
        }]
    }
    APP_R_BLUECLOUDFARSTEPSCALE {
        runtime: CvarDef::new("r_blueCloudFarStepScale", "1", "1..4",
            "Distant primary-ray step multiplier. 1 native; try 1.5/2. Ramps for cloud entry 1500..5000m away, preserves near-ray steps, max distance and iteration limit. May miss thin distant wisps.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.blue_water_environment.cloud_far_step_scale.to_string()),
            |app, value| App::cvar_set_bluecloud_far_step_scale(app, APP_R_BLUECLOUDFARSTEPSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_bluecloud_far_step_scale, None),
        config: CvarBinding::NONE,
        implementations: [impl App {
            fn cvar_set_bluecloud_far_step_scale(app: &mut Self, name: &str, value: &str) -> Result<(), String> {
                app.video.blue_water_environment.cloud_far_step_scale = App::parse_cvar_finite(name, value)?.clamp(1.0, 4.0);
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }
        }]
    }
    APP_R_BLUESKYALTITUDE {
        runtime:CvarDef::new("r_blueSkyAltitude","","-1000.0..15000.0","Cloud base (m)",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.altitude.to_string()),|app,value|App::cvar_set_bluesky_altitude(app,APP_R_BLUESKYALTITUDE.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_altitude,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.altitude=v.clamp(-1000.0, 15000.0) as f32;}}}),archive:Some((27300,|state|state.video.blue_water_environment.altitude.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_altitude(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.altitude=App::parse_cvar_finite(name,value)?.clamp(-1000.0,15000.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYTHICKNESS {
        runtime:CvarDef::new("r_blueSkyThickness","","50.0..12000.0","Cloud thickness (m)",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.thickness.to_string()),|app,value|App::cvar_set_bluesky_thickness(app,APP_R_BLUESKYTHICKNESS.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_thickness,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.thickness=v.clamp(50.0, 12000.0) as f32;}}}),archive:Some((27301,|state|state.video.blue_water_environment.thickness.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_thickness(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.thickness=App::parse_cvar_finite(name,value)?.clamp(50.0,12000.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYHORIZON {
        runtime:CvarDef::new("r_blueSkyHorizon","","0.0..1.0","Horizon coverage",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.horizon.to_string()),|app,value|App::cvar_set_bluesky_horizon(app,APP_R_BLUESKYHORIZON.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_horizon,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.horizon=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27302,|state|state.video.blue_water_environment.horizon.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_horizon(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.horizon=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYAMBIENT {
        runtime:CvarDef::new("r_blueSkyAmbient","","0.0..3.0","Cloud ambient light",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.ambient.to_string()),|app,value|App::cvar_set_bluesky_ambient(app,APP_R_BLUESKYAMBIENT.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_ambient,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.ambient=v.clamp(0.0, 3.0) as f32;}}}),archive:Some((27303,|state|state.video.blue_water_environment.ambient.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_ambient(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.ambient=App::parse_cvar_finite(name,value)?.clamp(0.0,3.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYBASESHADOW {
        runtime:CvarDef::new("r_blueSkyBaseShadow","","0.0..1.0","Cloud base shadow",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.base_shadow.to_string()),|app,value|App::cvar_set_bluesky_base_shadow(app,APP_R_BLUESKYBASESHADOW.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_base_shadow,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.base_shadow=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27304,|state|state.video.blue_water_environment.base_shadow.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_base_shadow(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.base_shadow=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYHIGHAMOUNT {
        runtime:CvarDef::new("r_blueSkyHighAmount","","0.0..1.0","High cloud amount",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.high_amount.to_string()),|app,value|App::cvar_set_bluesky_high_amount(app,APP_R_BLUESKYHIGHAMOUNT.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_high_amount,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.high_amount=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27305,|state|state.video.blue_water_environment.high_amount.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_high_amount(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.high_amount=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYHIGHALTITUDE {
        runtime:CvarDef::new("r_blueSkyHighAltitude","","2000.0..20000.0","High cloud altitude (m)",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.high_altitude.to_string()),|app,value|App::cvar_set_bluesky_high_altitude(app,APP_R_BLUESKYHIGHALTITUDE.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_high_altitude,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.high_altitude=v.clamp(2000.0, 20000.0) as f32;}}}),archive:Some((27306,|state|state.video.blue_water_environment.high_altitude.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_high_altitude(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.high_altitude=App::parse_cvar_finite(name,value)?.clamp(2000.0,20000.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMOONPHASE {
        runtime:CvarDef::new("r_blueSkyMoonPhase","","0.0..1.0","Moon phase",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.moon_phase.to_string()),|app,value|App::cvar_set_bluesky_moon_phase(app,APP_R_BLUESKYMOONPHASE.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_moon_phase,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.moon_phase=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27307,|state|state.video.blue_water_environment.moon_phase.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_moon_phase(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.moon_phase=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMOONELEVATION {
        runtime:CvarDef::new("r_blueSkyMoonElevation","","-90.0..90.0","Moon elevation",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.moon_elevation.to_string()),|app,value|App::cvar_set_bluesky_moon_elevation(app,APP_R_BLUESKYMOONELEVATION.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_moon_elevation,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.moon_elevation=v.clamp(-90.0, 90.0) as f32;}}}),archive:Some((27308,|state|state.video.blue_water_environment.moon_elevation.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_moon_elevation(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.moon_elevation=App::parse_cvar_finite(name,value)?.clamp(-90.0,90.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMOONAZIMUTH {
        runtime:CvarDef::new("r_blueSkyMoonAzimuth","","0.0..360.0","Moon azimuth",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.moon_azimuth.to_string()),|app,value|App::cvar_set_bluesky_moon_azimuth(app,APP_R_BLUESKYMOONAZIMUTH.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_moon_azimuth,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.moon_azimuth=v.clamp(0.0, 360.0) as f32;}}}),archive:Some((27309,|state|state.video.blue_water_environment.moon_azimuth.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_moon_azimuth(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.moon_azimuth=App::parse_cvar_finite(name,value)?.clamp(0.0,360.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYRAYLEIGH {
        runtime:CvarDef::new("r_blueSkyRayleigh","","0.0..1.5","Rayleigh / blue scattering",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.rayleigh.to_string()),|app,value|App::cvar_set_bluesky_rayleigh(app,APP_R_BLUESKYRAYLEIGH.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_rayleigh,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.rayleigh=v.clamp(0.0, 1.5) as f32;}}}),archive:Some((27310,|state|state.video.blue_water_environment.rayleigh.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_rayleigh(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.rayleigh=App::parse_cvar_finite(name,value)?.clamp(0.0,1.5) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYTURBIDITY {
        runtime:CvarDef::new("r_blueSkyTurbidity","","1.0..10.0","Atmospheric turbidity",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.turbidity.to_string()),|app,value|App::cvar_set_bluesky_turbidity(app,APP_R_BLUESKYTURBIDITY.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_turbidity,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.turbidity=v.clamp(1.0, 10.0) as f32;}}}),archive:Some((27311,|state|state.video.blue_water_environment.turbidity.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_turbidity(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.turbidity=App::parse_cvar_finite(name,value)?.clamp(1.0,10.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMIE {
        runtime:CvarDef::new("r_blueSkyMie","","0.0..2.0","Mie / sun aureole",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.mie.to_string()),|app,value|App::cvar_set_bluesky_mie(app,APP_R_BLUESKYMIE.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_mie,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.mie=v.clamp(0.0, 2.0) as f32;}}}),archive:Some((27312,|state|state.video.blue_water_environment.mie.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_mie(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.mie=App::parse_cvar_finite(name,value)?.clamp(0.0,2.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMIEG {
        runtime:CvarDef::new("r_blueSkyMieG","","0.0..0.95","Mie directionality",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.mie_g.to_string()),|app,value|App::cvar_set_bluesky_mie_g(app,APP_R_BLUESKYMIEG.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_mie_g,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.mie_g=v.clamp(0.0, 0.95) as f32;}}}),archive:Some((27313,|state|state.video.blue_water_environment.mie_g.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_mie_g(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.mie_g=App::parse_cvar_finite(name,value)?.clamp(0.0,0.95) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMULTIPLE {
        runtime:CvarDef::new("r_blueSkyMultiple","","0.0..3.0","Multiple scattering",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.multiple.to_string()),|app,value|App::cvar_set_bluesky_multiple(app,APP_R_BLUESKYMULTIPLE.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_multiple,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.multiple=v.clamp(0.0, 3.0) as f32;}}}),archive:Some((27314,|state|state.video.blue_water_environment.multiple.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_multiple(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.multiple=App::parse_cvar_finite(name,value)?.clamp(0.0,3.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYRAIN {
        runtime:CvarDef::new("r_blueSkyRain","","0.0..1.0","Native rainfall",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.rain.to_string()),|app,value|App::cvar_set_bluesky_rain(app,APP_R_BLUESKYRAIN.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_rain,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.rain=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27315,|state|state.video.blue_water_environment.rain.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_rain(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.rain=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYRAINSPEED {
        runtime:CvarDef::new("r_blueSkyRainSpeed","","0.25..4.0","Native rain fall speed multiplier",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.rain_speed.to_string()),|app,value|App::cvar_set_bluesky_rain_speed(app,APP_R_BLUESKYRAINSPEED.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_rain_speed,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.rain_speed=v.clamp(0.25,4.0);}}}),archive:Some((27325,|state|state.video.blue_water_environment.rain_speed.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_rain_speed(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.rain_speed=App::parse_cvar_finite(name,value)?.clamp(0.25,4.0); app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYLIGHTNING {
        runtime:CvarDef::new("r_blueSkyLightning","","0.0..30.0","Lightning strikes / minute",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.lightning.to_string()),|app,value|App::cvar_set_bluesky_lightning(app,APP_R_BLUESKYLIGHTNING.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_lightning,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.lightning=v.clamp(0.0, 30.0) as f32;}}}),archive:Some((27316,|state|state.video.blue_water_environment.lightning.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_lightning(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.lightning=App::parse_cvar_finite(name,value)?.clamp(0.0,30.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYFOG {
        runtime:CvarDef::new("r_blueSkyFog","","0.0..1.0","Sea fog banks",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.fog.to_string()),|app,value|App::cvar_set_bluesky_fog(app,APP_R_BLUESKYFOG.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_fog,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.fog=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27317,|state|state.video.blue_water_environment.fog.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_fog(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.fog=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYVISIBILITY {
        runtime:CvarDef::new("r_blueSkyVisibility","","200.0..100000.0","Haze visibility (m)",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.visibility.to_string()),|app,value|App::cvar_set_bluesky_visibility(app,APP_R_BLUESKYVISIBILITY.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_visibility,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.visibility=v.clamp(200.0, 100000.0) as f32;}}}),archive:Some((27318,|state|state.video.blue_water_environment.visibility.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_visibility(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.visibility=App::parse_cvar_finite(name,value)?.clamp(200.0,100000.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYQUALITY {
        runtime:CvarDef::new("r_blueSkyQuality","","0..3","Native quality",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.quality.to_string()),|app,value|App::cvar_set_bluesky_quality(app,APP_R_BLUESKYQUALITY.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_quality,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.quality=v.clamp(0.0, 3.0) as u8;}}}),archive:Some((27319,|state|state.video.blue_water_environment.quality.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_quality(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.quality=App::parse_cvar_finite(name,value)?.clamp(0.0,3.0) as u8; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUECLOUDQUALITY {
        runtime:CvarDef::new("r_blueCloudQuality","4","0..4","Blue Water clouds: 0 Low, 1 Medium, 2 High, 3 Ultra, 4 match sky quality. Controls march detail, lighting and cloud shadow budgets independently of the sky.",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.cloud_quality.to_string()),|app,value|App::cvar_set_bluecloud_quality(app,APP_R_BLUECLOUDQUALITY.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluecloud_quality,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.cloud_quality=v.clamp(0.0, 4.0) as u8;}}}),archive:Some((27328,|state|state.video.blue_water_environment.cloud_quality.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluecloud_quality(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.cloud_quality=App::parse_cvar_finite(name,value)?.clamp(0.0,4.0) as u8; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYGENUS {
        runtime:CvarDef::new("r_blueSkyGenus","","0..7","Native cloud type",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.genus.to_string()),|app,value|App::cvar_set_bluesky_genus(app,APP_R_BLUESKYGENUS.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_genus,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.genus=v.clamp(0.0, 7.0) as u8;}}}),archive:Some((27320,|state|state.video.blue_water_environment.genus.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_genus(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.genus=App::parse_cvar_finite(name,value)?.clamp(0.0,7.0) as u8; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYHIGHGENUS {
        runtime:CvarDef::new("r_blueSkyHighGenus","","0..3","High clouds",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.high_genus.to_string()),|app,value|App::cvar_set_bluesky_high_genus(app,APP_R_BLUESKYHIGHGENUS.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_high_genus,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.high_genus=v.clamp(0.0, 3.0) as u8;}}}),archive:Some((27321,|state|state.video.blue_water_environment.high_genus.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_high_genus(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.high_genus=App::parse_cvar_finite(name,value)?.clamp(0.0,3.0) as u8; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYWEATHERBACKEND {
        runtime:CvarDef::new("r_blueSkyWeatherBackend","","0..2","Weather renderer",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.weather_backend.to_string()),|app,value|App::cvar_set_bluesky_weather_backend(app,APP_R_BLUESKYWEATHERBACKEND.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_weather_backend,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.weather_backend=v.clamp(0.0, 2.0) as u8;}}}),archive:Some((27322,|state|state.video.blue_water_environment.weather_backend.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_weather_backend(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.weather_backend=App::parse_cvar_finite(name,value)?.clamp(0.0,2.0) as u8; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYMOONCUSTOM {
        runtime:CvarDef::new("r_blueSkyMoonCustom","","0..1","Manual moon position",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.moon_custom.to_string()),|app,value|App::cvar_set_bluesky_moon_custom(app,APP_R_BLUESKYMOONCUSTOM.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_moon_custom,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {settings.blue_water_environment.moon_custom=parse_bool(value).unwrap_or(settings.blue_water_environment.moon_custom);}),archive:Some((27323,|state|state.video.blue_water_environment.moon_custom.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_moon_custom(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.moon_custom=parse_bool(value).ok_or_else(||format!("{name} expects 0/1"))?; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYWORLDLIGHTING {
        runtime:CvarDef::new("r_blueSkyWorldLighting","","0..1","Sky world lighting",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.world_lighting.to_string()),|app,value|App::cvar_set_bluesky_world_lighting(app,APP_R_BLUESKYWORLDLIGHTING.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_world_lighting,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.world_lighting=v.clamp(0.0, 1.0);}}}),archive:Some((27324,|state|state.video.blue_water_environment.world_lighting.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_world_lighting(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.world_lighting=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0); app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYSTRIKEPREVIEW {
        runtime:CvarDef::new("r_blueSkyStrikePreview","0","0|1","Hold a branched ground strike in front of the camera for inspection",CvarType::String,CvarFlags::NONE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.strike_preview.to_string()),|app,value|App::cvar_set_bluesky_strike_preview(app,APP_R_BLUESKYSTRIKEPREVIEW.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_strike_preview,None),
        config:CvarBinding::NONE,
        implementations:[impl App {fn cvar_set_bluesky_strike_preview(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.strike_preview=parse_bool(value).ok_or_else(||format!("{name} expects 0/1"))?; app.sync_post_effects();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYFILMIC {
        runtime:CvarDef::new("r_blueSkyFilmic","1","0|1","Sky-referenced exposure (eye adaptation, AgX, night readability); active only under the Blue Water sky",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.filmic.to_string()),|app,value|App::cvar_set_bluesky_filmic(app,APP_R_BLUESKYFILMIC.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_filmic,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {settings.blue_water_environment.filmic=parse_bool(value).unwrap_or(settings.blue_water_environment.filmic);}),archive:Some((27326,|state|state.video.blue_water_environment.filmic.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_filmic(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.filmic=parse_bool(value).ok_or_else(||format!("{name} expects 0/1"))?; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }
    APP_R_BLUESKYNIGHTSKY {
        runtime:CvarDef::new("r_blueSkyNightSky","1","0.0..1.0","Native night sky brightness (dome glow and floor)",CvarType::String,CvarFlags::ARCHIVE,LatchScope::None,
            |app:&App|CvarValue::String(app.video.blue_water_environment.night_sky.to_string()),|app,value|App::cvar_set_bluesky_night_sky(app,APP_R_BLUESKYNIGHTSKY.entry.name,&value.to_string())).with_text_setter(App::cvar_set_bluesky_night_sky,None),
        config:CvarBinding {load_video:Some(|settings,_name,value,_context| {if let Ok(v)=value.parse::<f32>() {if v.is_finite() {settings.blue_water_environment.night_sky=v.clamp(0.0, 1.0) as f32;}}}),archive:Some((27327,|state|state.video.blue_water_environment.night_sky.to_string())),..CvarBinding::NONE},
        implementations:[impl App {fn cvar_set_bluesky_night_sky(app:&mut Self,name:&str,value:&str)->Result<(),String>{app.video.blue_water_environment.night_sky=App::parse_cvar_finite(name,value)?.clamp(0.0,1.0) as f32; app.sync_post_effects();app.mark_config_dirty();app.publish_ui();Ok(())}}]
    }

    APP_R_SKYBACKEND {
        runtime: CvarDef::new("r_skyBackend", "0", "0|1",
            "0 authored JKA sky; 1 original Blue Water atmosphere, celestial bodies and environment lighting.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.sky_backend as i32),
            |app, value| {
                let CvarValue::Int(backend) = value else { unreachable!() };
                app.video.sky_backend = backend as u8;
                app.sync_post_effects();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_CLOUDBACKEND {
        runtime: CvarDef::new("r_cloudBackend", "0", "0|1",
            "0 JKA volumetric clouds; 1 original Blue Water cloud march, reconstruction and shadows.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.cloud_backend as i32),
            |app, value| {
                let CvarValue::Int(backend) = value else { unreachable!() };
                app.video.cloud_backend = backend as u8;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_CLOUDPORTALDEPTH {
        runtime: CvarDef::new("r_cloudPortalDepth", "0", "0|1|2",
            "Sky-portal geometry vs volumetric clouds. 0 clouds draw over portal geometry; 1 depth-only redraw of the portal view; 2 reuse the portal pass depth (no extra draw, needs MSAA off).",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.cloud_portal_depth as i32),
            |app, value| {
                let CvarValue::Int(mode) = value else { unreachable!() };
                app.video.cloud_portal_depth = mode as u8;
                app.sync_post_effects(); app.publish_ui(); Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_CLOUDAERIAL {
        runtime: CvarDef::new(
                "r_cloudAerial",
                "",
                "custom",
                "Controls r_cloudAerial.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudaerial(app)),
                |app, value| {
                    App::cvar_set_r_cloudaerial(app, APP_R_CLOUDAERIAL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudaerial, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_aerial = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_aerial)
                    }
                }),
                archive: Some((80, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_aerial)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudaerial(&self) -> String {
                    format!("{:.3}", self.video.cloud_aerial)
                }
                fn cvar_set_r_cloudaerial(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudaerial".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDBASEVARIATION {
        runtime: CvarDef::new(
                "r_cloudBaseVariation",
                "",
                "custom",
                "Controls r_cloudBaseVariation.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudbasevariation(app)),
                |app, value| {
                    App::cvar_set_r_cloudbasevariation(
                        app,
                        APP_R_CLOUDBASEVARIATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudbasevariation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_base_variation = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_base_variation)
                    }
                }),
                archive: Some((76, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_base_variation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudbasevariation(&self) -> String {
                    format!("{:.3}", self.video.cloud_base_variation)
                }
                fn cvar_set_r_cloudbasevariation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudbasevariation".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDCOVERAGE {
        runtime: CvarDef::new(
                "r_cloudCoverage",
                "",
                "custom",
                "Controls r_cloudCoverage.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudcoverage(app)),
                |app, value| {
                    App::cvar_set_r_cloudcoverage(app, APP_R_CLOUDCOVERAGE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudcoverage, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_coverage = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_coverage)
                    }
                }),
                archive: Some((67, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_coverage)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudcoverage(&self) -> String {
                    format!("{:.3}", self.video.cloud_coverage)
                }
                fn cvar_set_r_cloudcoverage(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.set_cloud_coverage(Self::parse_cvar_finite(name, value)?);
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDEMPTYSKIP {
        runtime: CvarDef::new(
                "r_cloudEmptySkip",
                "",
                "0..1",
                "A/B adaptive empty-space skipping in the primary volumetric cloud march.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudemptyskip(app)),
                |app, value| {
                    App::cvar_set_r_cloudemptyskip(app, APP_R_CLOUDEMPTYSKIP.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudemptyskip, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_empty_skip = parse_bool(value).unwrap_or(settings.cloud_empty_skip)
                    }
                }),
                archive: Some((79, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_empty_skip))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudemptyskip(&self) -> String {
                    bool_value(self.video.cloud_empty_skip)
                }
                fn cvar_set_r_cloudemptyskip(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudemptyskip".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDHEIGHT {
        runtime: CvarDef::new(
                "r_cloudHeight",
                "",
                "custom",
                "Controls r_cloudHeight.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudheight(app)),
                |app, value| {
                    App::cvar_set_r_cloudheight(app, APP_R_CLOUDHEIGHT.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudheight, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_height = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(CLOUD_HEIGHT_MIN, CLOUD_HEIGHT_MAX))
                            .unwrap_or(settings.cloud_height)
                    }
                }),
                archive: Some((68, |state| {
                    let settings = state.video;
                    format!("{:.1}", settings.cloud_height)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudheight(&self) -> String {
                    format!("{:.1}", self.video.cloud_height)
                }
                fn cvar_set_r_cloudheight(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.set_cloud_height(Self::parse_cvar_finite(name, value)?);
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDHISTORYBLEND {
        runtime: CvarDef::new(
                "r_cloudHistoryBlend",
                "",
                "custom",
                "Controls r_cloudHistoryBlend.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudhistoryblend(app)),
                |app, value| {
                    App::cvar_set_r_cloudhistoryblend(
                        app,
                        APP_R_CLOUDHISTORYBLEND.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudhistoryblend, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_history_blend = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 0.98))
                            .unwrap_or(settings.cloud_history_blend)
                    }
                }),
                archive: Some((82, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_history_blend)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudhistoryblend(&self) -> String {
                    format!("{:.3}", self.video.cloud_history_blend)
                }
                fn cvar_set_r_cloudhistoryblend(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudhistoryblend".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDHISTORYDEPTHREJECT {
        runtime: CvarDef::new(
                "r_cloudHistoryDepthReject",
                "",
                "custom",
                "Controls r_cloudHistoryDepthReject.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudhistorydepthreject(app)),
                |app, value| {
                    App::cvar_set_r_cloudhistorydepthreject(
                        app,
                        APP_R_CLOUDHISTORYDEPTHREJECT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudhistorydepthreject, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_history_depth_reject =
                            parse_bool(value).unwrap_or(settings.cloud_history_depth_reject)
                    }
                }),
                archive: Some((84, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_history_depth_reject))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudhistorydepthreject(&self) -> String {
                    bool_value(self.video.cloud_history_depth_reject)
                }
                fn cvar_set_r_cloudhistorydepthreject(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_cloudhistorydepthreject".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        if lower == "r_cloudskyambient" {
                            self.video.cloud_sky_ambient = enabled;
                        } else {
                            self.video.cloud_history_depth_reject = enabled;
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDMOTIONREJECT {
        runtime: CvarDef::new(
                "r_cloudMotionReject",
                "",
                "custom",
                "Controls r_cloudMotionReject.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudmotionreject(app)),
                |app, value| {
                    App::cvar_set_r_cloudmotionreject(
                        app,
                        APP_R_CLOUDMOTIONREJECT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudmotionreject, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_motion_reject = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_motion_reject)
                    }
                }),
                archive: Some((83, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_motion_reject)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudmotionreject(&self) -> String {
                    format!("{:.3}", self.video.cloud_motion_reject)
                }
                fn cvar_set_r_cloudmotionreject(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudmotionreject".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDQUALITY {
        runtime: CvarDef::new(
                "r_cloudQuality",
                "",
                "custom",
                "JKA cloud raymarch quality (0..1): steps, shadow samples and detailed distance. Blue Water clouds use r_blueCloudQuality.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudquality(app)),
                |app, value| {
                    App::cvar_set_r_cloudquality(app, APP_R_CLOUDQUALITY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudquality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_quality = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_quality)
                    }
                }),
                archive: Some((66, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_quality)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudquality(&self) -> String {
                    format!("{:.3}", self.video.cloud_quality)
                }
                fn cvar_set_r_cloudquality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.set_cloud_quality(Self::parse_cvar_finite(name, value)?);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDRENDERRESOLUTION {
        runtime: CvarDef::new(
                "r_cloudRenderResolution",
                "",
                "custom",
                "Cloud buffer resolution for both JKA and Blue Water clouds: quarter (25%), half (50%), 75, full (100%). Lower resolutions reduce march/reconstruction cost and soften detail; shadow maps are controlled separately by the cloud quality tier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudrenderresolution(app)),
                |app, value| {
                    App::cvar_set_r_cloudrenderresolution(
                        app,
                        APP_R_CLOUDRENDERRESOLUTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudrenderresolution, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_render_resolution = CloudRenderResolution::from_config(value)
                            .unwrap_or(settings.cloud_render_resolution)
                    }
                }),
                archive: Some((72, |state| {
                    let settings = state.video;
                    format!("{}", settings.cloud_render_resolution.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudrenderresolution(&self) -> String {
                    {
                        self.video.cloud_render_resolution.config_value().to_owned()
                    }
                }
                fn cvar_set_r_cloudrenderresolution(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.cloud_render_resolution = CloudRenderResolution::from_config(value)
                            .ok_or_else(|| format!("{name}: expected full|75|half|quarter"))?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDS {
        runtime: CvarDef::new(
                "r_clouds",
                "",
                "custom",
                "Controls r_clouds.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_clouds(app)),
                |app, value| App::cvar_set_r_clouds(app, APP_R_CLOUDS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_clouds, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.clouds = parse_bool(value).unwrap_or(settings.clouds)
                }),
                archive: Some((64, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.clouds))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_clouds(&self) -> String {
                    bool_value(self.video.clouds)
                }
                fn cvar_set_r_clouds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.clouds = Self::parse_cvar_bool(name, value)?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDSHADOWS {
        runtime: CvarDef::new(
                "r_cloudShadows",
                "",
                "custom",
                "Controls r_cloudShadows.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudshadows(app)),
                |app, value| {
                    App::cvar_set_r_cloudshadows(app, APP_R_CLOUDSHADOWS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_shadows = parse_bool(value).unwrap_or(settings.cloud_shadows)
                    }
                }),
                archive: Some((71, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_shadows))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudshadows(&self) -> String {
                    bool_value(self.video.cloud_shadows)
                }
                fn cvar_set_r_cloudshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudshadows".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDSHAPEEVOLUTION {
        runtime: CvarDef::new(
                "r_cloudShapeEvolution",
                "",
                "0..1",
                "Enable slow relative evolution of cloud detail and billows.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudshapeevolution(app)),
                |app, value| {
                    App::cvar_set_r_cloudshapeevolution(
                        app,
                        APP_R_CLOUDSHAPEEVOLUTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudshapeevolution, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_shape_evolution =
                            parse_bool(value).unwrap_or(settings.cloud_shape_evolution)
                    }
                }),
                archive: Some((77, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_shape_evolution))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudshapeevolution(&self) -> String {
                    bool_value(self.video.cloud_shape_evolution)
                }
                fn cvar_set_r_cloudshapeevolution(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_cloudshapeevolution".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDSHEAR {
        runtime: CvarDef::new(
                "r_cloudShear",
                "",
                "custom",
                "Controls r_cloudShear.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudshear(app)),
                |app, value| {
                    App::cvar_set_r_cloudshear(app, APP_R_CLOUDSHEAR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudshear, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_shear = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_shear)
                    }
                }),
                archive: Some((75, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_shear)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudshear(&self) -> String {
                    format!("{:.3}", self.video.cloud_shear)
                }
                fn cvar_set_r_cloudshear(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudshear".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDSIZE {
        runtime: CvarDef::new(
                "r_cloudSize",
                "",
                "custom",
                "Controls r_cloudSize.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudsize(app)),
                |app, value| App::cvar_set_r_cloudsize(app, APP_R_CLOUDSIZE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_cloudsize, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_size = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_size)
                    }
                }),
                archive: Some((86, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudsize(&self) -> String {
                    format!("{:.3}", self.video.cloud_size)
                }
                fn cvar_set_r_cloudsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudsize".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDSKYAMBIENT {
        runtime: CvarDef::new(
                "r_cloudSkyAmbient",
                "",
                "custom",
                "Controls r_cloudSkyAmbient.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudskyambient(app)),
                |app, value| {
                    App::cvar_set_r_cloudskyambient(
                        app,
                        APP_R_CLOUDSKYAMBIENT.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudskyambient, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_sky_ambient = parse_bool(value).unwrap_or(settings.cloud_sky_ambient)
                    }
                }),
                archive: Some((81, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_sky_ambient))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudskyambient(&self) -> String {
                    bool_value(self.video.cloud_sky_ambient)
                }
                fn cvar_set_r_cloudskyambient(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudskyambient".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        if lower == "r_cloudskyambient" {
                            self.video.cloud_sky_ambient = enabled;
                        } else {
                            self.video.cloud_history_depth_reject = enabled;
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTEMPORAL {
        runtime: CvarDef::new(
                "r_cloudTemporal",
                "",
                "custom",
                "Controls r_cloudTemporal.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudtemporal(app)),
                |app, value| {
                    App::cvar_set_r_cloudtemporal(app, APP_R_CLOUDTEMPORAL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudtemporal, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_temporal = parse_bool(value).unwrap_or(settings.cloud_temporal)
                    }
                }),
                archive: Some((73, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_temporal))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudtemporal(&self) -> String {
                    bool_value(self.video.cloud_temporal)
                }
                fn cvar_set_r_cloudtemporal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_cloudtemporal".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTEMPORALDEPTHFIX {
        runtime: CvarDef::new(
                "r_cloudTemporalDepthFix",
                "",
                "custom",
                "Controls r_cloudTemporalDepthFix.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudtemporaldepthfix(app)),
                |app, value| {
                    App::cvar_set_r_cloudtemporaldepthfix(
                        app,
                        APP_R_CLOUDTEMPORALDEPTHFIX.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudtemporaldepthfix, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_temporal_depth_fix =
                            parse_bool(value).unwrap_or(settings.cloud_temporal_depth_fix)
                    }
                }),
                archive: Some((74, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_temporal_depth_fix))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudtemporaldepthfix(&self) -> String {
                    bool_value(self.video.cloud_temporal_depth_fix)
                }
                fn cvar_set_r_cloudtemporaldepthfix(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_cloudtemporaldepthfix".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTERRAININTERACTION {
        runtime: CvarDef::new(
                "r_cloudTerrainInteraction",
                "",
                "0..1",
                "Enable terrain-driven lifting of low cloud shapes.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudterraininteraction(app)),
                |app, value| {
                    App::cvar_set_r_cloudterraininteraction(
                        app,
                        APP_R_CLOUDTERRAININTERACTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudterraininteraction, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_terrain_interaction =
                            parse_bool(value).unwrap_or(settings.cloud_terrain_interaction)
                    }
                }),
                archive: Some((78, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cloud_terrain_interaction))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudterraininteraction(&self) -> String {
                    bool_value(self.video.cloud_terrain_interaction)
                }
                fn cvar_set_r_cloudterraininteraction(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_cloudterraininteraction".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTHICKNESS {
        runtime: CvarDef::new(
                "r_cloudThickness",
                "",
                "custom",
                "Controls r_cloudThickness.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudthickness(app)),
                |app, value| {
                    App::cvar_set_r_cloudthickness(app, APP_R_CLOUDTHICKNESS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_cloudthickness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_thickness = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(CLOUD_THICKNESS_MIN, CLOUD_THICKNESS_MAX))
                            .unwrap_or(settings.cloud_thickness)
                    }
                }),
                archive: Some((69, |state| {
                    let settings = state.video;
                    format!("{:.1}", settings.cloud_thickness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudthickness(&self) -> String {
                    format!("{:.1}", self.video.cloud_thickness)
                }
                fn cvar_set_r_cloudthickness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.set_cloud_thickness(Self::parse_cvar_finite(name, value)?);
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTHICKNESSVARIATION {
        runtime: CvarDef::new(
                "r_cloudThicknessVariation",
                "",
                "custom",
                "Controls r_cloudThicknessVariation.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudthicknessvariation(app)),
                |app, value| {
                    App::cvar_set_r_cloudthicknessvariation(
                        app,
                        APP_R_CLOUDTHICKNESSVARIATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cloudthicknessvariation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_thickness_variation = value
                            .parse::<f32>()
                            .ok()
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.cloud_thickness_variation)
                    }
                }),
                archive: Some((85, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.cloud_thickness_variation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudthicknessvariation(&self) -> String {
                    format!("{:.3}", self.video.cloud_thickness_variation)
                }
                fn cvar_set_r_cloudthicknessvariation(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let lower = "r_cloudthicknessvariation".to_owned();
                    {
                        let amount = Self::parse_cvar_finite(name, value)?;
                        let video = &mut self.video;
                        match lower.as_str() {
                            "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                            "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                            "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                            "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                            "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                            "r_cloudthicknessvariation" => {
                                video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                            }
                            _ => video.cloud_size = amount.clamp(0.0, 1.0),
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CLOUDTYPE {
        runtime: CvarDef::new(
                "r_cloudType",
                "",
                "custom",
                "Controls r_cloudType.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cloudtype(app)),
                |app, value| App::cvar_set_r_cloudtype(app, APP_R_CLOUDTYPE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_cloudtype, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cloud_type = CloudType::from_config(value).unwrap_or(settings.cloud_type)
                    }
                }),
                archive: Some((65, |state| {
                    let settings = state.video;
                    format!("{}", settings.cloud_type.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cloudtype(&self) -> String {
                    self.video.cloud_type.config_value().to_owned()
                }
                fn cvar_set_r_cloudtype(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.cloud_type = ui::CloudType::from_config(value)
                            .ok_or_else(|| format!("{name}: expected cumulus|stratus|storm"))?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
    APP_R_SKYFRACTAL {
        runtime: CvarDef::new(
                "r_skyFractal", "", "0|1",
                "Experimental Star Nest sky override: 0 map sky, 1 procedural fractal sky. Affects BSP sky faces only. High GPU cost; A/B test.",
                CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
                |app: &App| CvarValue::String(bool_value(app.video.sky_fractal_override)),
                |app, value| App::cvar_set_r_skyfractal(app, APP_R_SKYFRACTAL.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_r_skyfractal, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.sky_fractal_override = parse_bool(value).unwrap_or(settings.sky_fractal_override)
                }),
                archive: Some((66, |state| format!("{}", u8::from(state.video.sky_fractal_override)))),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_set_r_skyfractal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.sky_fractal_override = Self::parse_cvar_bool(name, value)?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
}
