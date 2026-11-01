//! Authored river material controls; native defaults, generation unchanged.
use super::cvar_prelude::*;
app_cvars! {
    APP_R_RIVERSCATTERINGCOLOR {
        runtime: CvarDef::new("r_riverScatteringColor", "0.765623 1 0.924169", "r g b (0..1)", "Color of light scattered through the river.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.river_settings.water.scattering; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_river_scatteringcolor(app,"r_riverScatteringColor",&value.to_string())).with_text_setter(App::cvar_set_river_scatteringcolor,None),
        config: CvarBinding { load_video: Some(|settings,_,value,_| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v|v.is_finite()) { for i in 0..3 {settings.river_settings.water.scattering[i]=c[i].clamp(0.0,1.0);} } } }),
            archive: Some((29100,|s|{let c=s.video.river_settings.water.scattering;format!("{} {} {}",c[0],c[1],c[2])})), ..CvarBinding::NONE },
        implementations: [impl App { fn cvar_set_river_scatteringcolor(&mut self,name:&str,value:&str)->Result<(),String> { let c: [f32;3]=Self::parse_cvar_floats(name,value)?; for i in 0..3 {self.video.river_settings.water.scattering[i]=c[i].clamp(0.0,1.0);} self.commit_river_settings();Ok(()) } }]
    }
    APP_R_RIVERWATERALBEDO {
        runtime: CvarDef::new("r_riverWaterAlbedo", "0.083333 0.083333 0.083333", "r g b (0..1)", "Diffuse surface color; clear water is colored mainly by absorption and scattering.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.river_settings.water.water_albedo; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_river_wateralbedo(app,"r_riverWaterAlbedo",&value.to_string())).with_text_setter(App::cvar_set_river_wateralbedo,None),
        config: CvarBinding { load_video: Some(|settings,_,value,_| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v|v.is_finite()) { for i in 0..3 {settings.river_settings.water.water_albedo[i]=c[i].clamp(0.0,1.0);} } } }),
            archive: Some((29101,|s|{let c=s.video.river_settings.water.water_albedo;format!("{} {} {}",c[0],c[1],c[2])})), ..CvarBinding::NONE },
        implementations: [impl App { fn cvar_set_river_wateralbedo(&mut self,name:&str,value:&str)->Result<(),String> { let c: [f32;3]=Self::parse_cvar_floats(name,value)?; for i in 0..3 {self.video.river_settings.water.water_albedo[i]=c[i].clamp(0.0,1.0);} self.commit_river_settings();Ok(()) } }]
    }
    APP_R_RIVERFOAMCOLOR {
        runtime: CvarDef::new("r_riverFoamColor", "0.389404 0.755208 0.557239", "r g b (0..1)", "Base color of foam under scene lighting.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.river_settings.foam.base_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_river_foamcolor(app,"r_riverFoamColor",&value.to_string())).with_text_setter(App::cvar_set_river_foamcolor,None),
        config: CvarBinding { load_video: Some(|settings,_,value,_| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v|v.is_finite()) { for i in 0..3 {settings.river_settings.foam.base_color[i]=c[i].clamp(0.0,1.0);} } } }),
            archive: Some((29102,|s|{let c=s.video.river_settings.foam.base_color;format!("{} {} {}",c[0],c[1],c[2])})), ..CvarBinding::NONE },
        implementations: [impl App { fn cvar_set_river_foamcolor(&mut self,name:&str,value:&str)->Result<(),String> { let c: [f32;3]=Self::parse_cvar_floats(name,value)?; for i in 0..3 {self.video.river_settings.foam.base_color[i]=c[i].clamp(0.0,1.0);} self.commit_river_settings();Ok(()) } }]
    }
    APP_R_RIVERFOAMSCATTERINGCOLOR {
        runtime: CvarDef::new("r_riverFoamScatteringColor", "0.427083 1 0.67074", "r g b (0..1)", "Additional foam volume scattering color. Requires authored WaterInfo flow; source-only rivers have none.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.river_settings.foam.scattering_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_river_foamscatteringcolor(app,"r_riverFoamScatteringColor",&value.to_string())).with_text_setter(App::cvar_set_river_foamscatteringcolor,None),
        config: CvarBinding { load_video: Some(|settings,_,value,_| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v|v.is_finite()) { for i in 0..3 {settings.river_settings.foam.scattering_color[i]=c[i].clamp(0.0,1.0);} } } }),
            archive: Some((29103,|s|{let c=s.video.river_settings.foam.scattering_color;format!("{} {} {}",c[0],c[1],c[2])})), ..CvarBinding::NONE },
        implementations: [impl App { fn cvar_set_river_foamscatteringcolor(&mut self,name:&str,value:&str)->Result<(),String> { let c: [f32;3]=Self::parse_cvar_floats(name,value)?; for i in 0..3 {self.video.river_settings.foam.scattering_color[i]=c[i].clamp(0.0,1.0);} self.commit_river_settings();Ok(()) } }]
    }
    APP_R_RIVERSCATTERINGSTRENGTH {
        runtime: CvarDef::new("r_riverScatteringStrength", "0.5", "0..5", "Amount of light scattered through the water.", CvarType::Float {min:0.0,max:5.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.scattering[3]), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.scattering[3]=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.scattering[3]=v.clamp(0.0,5.0);}}}), archive:Some((29110,|s|s.video.river_settings.water.scattering[3].to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERABSORPTIONRED {
        runtime: CvarDef::new("r_riverAbsorptionRed", "20.833334", "0.1..200", "Red-channel distance parameter; larger values transmit more red light.", CvarType::Float {min:0.1,max:200.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.absorption[0]), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.absorption[0]=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.absorption[0]=v.clamp(0.1,200.0);}}}), archive:Some((29111,|s|s.video.river_settings.water.absorption[0].to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERABSORPTIONGREEN {
        runtime: CvarDef::new("r_riverAbsorptionGreen", "25", "0.1..200", "Green-channel distance parameter; larger values transmit more green light.", CvarType::Float {min:0.1,max:200.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.absorption[1]), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.absorption[1]=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.absorption[1]=v.clamp(0.1,200.0);}}}), archive:Some((29112,|s|s.video.river_settings.water.absorption[1].to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERABSORPTIONBLUE {
        runtime: CvarDef::new("r_riverAbsorptionBlue", "24.673916", "0.1..200", "Blue-channel distance parameter; larger values transmit more blue light.", CvarType::Float {min:0.1,max:200.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.absorption[2]), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.absorption[2]=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.absorption[2]=v.clamp(0.1,200.0);}}}), archive:Some((29113,|s|s.video.river_settings.water.absorption[2].to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERABSORPTIONSCALE {
        runtime: CvarDef::new("r_riverAbsorptionScale", "8", "0.1..100", "Multiplies all three absorption distances. Higher values make the river clearer.", CvarType::Float {min:0.1,max:100.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.absorption[3]), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.absorption[3]=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.absorption[3]=v.clamp(0.1,100.0);}}}), archive:Some((29114,|s|s.video.river_settings.water.absorption[3].to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERANISOTROPY {
        runtime: CvarDef::new("r_riverAnisotropy", "0.1", "-0.99..0.99", "Bias of scattered light toward forward or backward directions.", CvarType::Float {min:-0.99,max:0.99}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.anisotropy), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.anisotropy=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.anisotropy=v.clamp(-0.99,0.99);}}}), archive:Some((29115,|s|s.video.river_settings.water.anisotropy.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERROUGHNESS {
        runtime: CvarDef::new("r_riverRoughness", "0.02", "0.02..1", "Width of sun highlights and environment reflections.", CvarType::Float {min:0.02,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.water_roughness), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.water_roughness=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.water_roughness=v.clamp(0.02,1.0);}}}), archive:Some((29116,|s|s.video.river_settings.water.water_roughness.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFRESNELROUGHNESS {
        runtime: CvarDef::new("r_riverFresnelRoughness", "0.1", "0.02..1", "Surface roughness when viewing along the water.", CvarType::Float {min:0.02,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.water_fresnel_roughness), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.water_fresnel_roughness=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.water_fresnel_roughness=v.clamp(0.02,1.0);}}}), archive:Some((29117,|s|s.video.river_settings.water.water_fresnel_roughness.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERSPECULAR {
        runtime: CvarDef::new("r_riverSpecular", "0.225", "0..1", "Dielectric reflectivity; also changes the optical index of refraction.", CvarType::Float {min:0.0,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.water_specular), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.water_specular=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.water_specular=v.clamp(0.0,1.0);}}}), archive:Some((29118,|s|s.video.river_settings.water.water_specular.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFRESNELSPECULAR {
        runtime: CvarDef::new("r_riverFresnelSpecular", "0.225", "0..1", "Material specular value at grazing view angles.", CvarType::Float {min:0.0,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.water_fresnel_specular), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.water_fresnel_specular=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.water_fresnel_specular=v.clamp(0.0,1.0);}}}), archive:Some((29119,|s|s.video.river_settings.water.water_fresnel_specular.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERREFRACTION {
        runtime: CvarDef::new("r_riverRefraction", "1.15", "1..2", "Bending of the scene through the water.", CvarType::Float {min:1.0,max:2.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.refraction), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.refraction=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.refraction=v.clamp(1.0,2.0);}}}), archive:Some((29120,|s|s.video.river_settings.water.refraction.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERREFRACTIONFAR {
        runtime: CvarDef::new("r_riverRefractionFar", "1.02", "1..2", "Refraction strength at the distant-water transition.", CvarType::Float {min:1.0,max:2.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.refraction_far), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.refraction_far=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.refraction_far=v.clamp(1.0,2.0);}}}), archive:Some((29121,|s|s.video.river_settings.water.refraction_far.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERREFRACTIONDISTANCE {
        runtime: CvarDef::new("r_riverRefractionDistance", "5000", "1..50000", "View depth at which distant refraction is reached.", CvarType::Float {min:1.0,max:50000.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.water.depth_for_min_refraction), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.water.depth_for_min_refraction=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.water.depth_for_min_refraction=v.clamp(1.0,50000.0);}}}), archive:Some((29122,|s|s.video.river_settings.water.depth_for_min_refraction.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERWAVEAMOUNT {
        runtime: CvarDef::new("r_riverWaveAmount", "1", "0..5", "Strength of the existing flow-advected FFT detail normals.", CvarType::Float {min:0.0,max:5.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.wave_amount), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.wave_amount=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.wave_amount=v.clamp(0.0,5.0);}}}), archive:Some((29123,|s|s.video.river_settings.wave_amount.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERWAVEHEIGHT {
        runtime: CvarDef::new("r_riverWaveHeight", "5", "0..30", "Multiplier of the existing flow-advected FFT displacement.", CvarType::Float {min:0.0,max:30.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.wave_height), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.wave_height=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.wave_height=v.clamp(0.0,30.0);}}}), archive:Some((29124,|s|s.video.river_settings.wave_height.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFOAMGAIN {
        runtime: CvarDef::new("r_riverFoamGain", "1", "0..5", "Amplifies existing simulated foam; does not generate new foam.", CvarType::Float {min:0.0,max:5.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.foam.gain), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.foam.gain=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.foam.gain=v.clamp(0.0,5.0);}}}), archive:Some((29125,|s|s.video.river_settings.foam.gain.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFOAMOPACITY {
        runtime: CvarDef::new("r_riverFoamOpacity", "1", "0..1", "Coverage of the water by existing foam.", CvarType::Float {min:0.0,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.foam.opacity), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.foam.opacity=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.foam.opacity=v.clamp(0.0,1.0);}}}), archive:Some((29126,|s|s.video.river_settings.foam.opacity.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFOAMROUGHNESS {
        runtime: CvarDef::new("r_riverFoamRoughness", "0.1", "0.02..1", "Width of highlights and reflections on foam.", CvarType::Float {min:0.02,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.foam.roughness), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.foam.roughness=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.foam.roughness=v.clamp(0.02,1.0);}}}), archive:Some((29127,|s|s.video.river_settings.foam.roughness.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFOAMSPECULAR {
        runtime: CvarDef::new("r_riverFoamSpecular", "0.5", "0..1", "Dielectric reflectivity of foam.", CvarType::Float {min:0.0,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.foam.specular), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.foam.specular=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.foam.specular=v.clamp(0.0,1.0);}}}), archive:Some((29128,|s|s.video.river_settings.foam.specular.to_string())), ..CvarBinding::NONE }, implementations: []
    }
    APP_R_RIVERFOAMNORMALFLATNESS {
        runtime: CvarDef::new("r_riverFoamNormalFlatness", "0.3", "0..1", "Flattens the foam texture normals toward the surface normal.", CvarType::Float {min:0.0,max:1.0}, CvarFlags::ARCHIVE, LatchScope::None,
            |app:&App|CvarValue::Float(app.video.river_settings.foam.normal_flatness), |app,value|{let CvarValue::Float(v)=value else {unreachable!()};app.video.river_settings.foam.normal_flatness=v;app.commit_river_settings();Ok(())}),
        config: CvarBinding { load_video:Some(|s,_,v,_|{if let Ok(v)=v.parse::<f32>(){if v.is_finite(){s.river_settings.foam.normal_flatness=v.clamp(0.0,1.0);}}}), archive:Some((29129,|s|s.video.river_settings.foam.normal_flatness.to_string())), ..CvarBinding::NONE }, implementations: []
    }
}
