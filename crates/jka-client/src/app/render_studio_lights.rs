//! Recording-independent, deterministic light rigs on the output timeline.
use crate::{renderer::TransientLight, scene};

pub(in crate::app) use jka_studio::render_studio_lights::*;

/// Studio lights as renderer lights. Project data lives in `jka_studio`.
pub(in crate::app) trait StudioLightRender {
    fn render_light(&self, time: f64, smooth: bool) -> Option<TransientLight>;
}
impl StudioLightRender for StudioLight {
    fn render_light(&self, time: f64, smooth: bool) -> Option<TransientLight> {
        if !self.enabled || time < self.start || time >= self.end() { return None; }
        let key = self.sample(time, smooth);
        let intensity = key.intensity * self.emitter.level((time - self.start).max(0.0));
        if intensity <= 0.0 { return None; }
        let position = scene::render_position(key.position);
        let spot = (self.kind == StudioLightKind::Spot).then(|| {
            let delta = glam::Vec3::from_array(scene::render_position(key.target)) - glam::Vec3::from_array(position);
            let direction = delta.try_normalize().unwrap_or(glam::Vec3::X).to_array();
            crate::fx::system::FxSpotLight { direction,
                inner_cone_cos: key.inner_angle.to_radians().cos(), outer_cone_cos: key.outer_angle.to_radians().cos() }
        });
        Some(TransientLight { kind: crate::fx::system::FxLightKind::AuthoredEffect, position,
            color: key.color, intensity, radius: key.radius, source_radius: key.source_radius,
            spot, segment: None, blade_segments: None })
    }
}

pub(in crate::app) trait StudioEnvironmentRender {
    fn render_sample(self) -> crate::renderer::StudioSceneLighting;
}
impl StudioEnvironmentRender for StudioEnvironmentKey {
    fn render_sample(self)->crate::renderer::StudioSceneLighting {
        // world directions transform like world positions; JKA <-> WGPU is a
        // pure axis convention conversion with no translation.
        let dir=glam::Vec3::from_array(scene::render_position(self.sun_direction))
            .normalize_or_zero().to_array();
        crate::renderer::StudioSceneLighting {
            ambient:[self.ambient_color[0],self.ambient_color[1],self.ambient_color[2],self.ambient_intensity],
            sun:[self.sun_color[0],self.sun_color[1],self.sun_color[2],self.sun_intensity],
            sun_direction:[dir[0],dir[1],dir[2],0.0],
        }
    }
}

pub(in crate::app) trait StudioTimelineLights {
    fn lights_at(&self, time: f64) -> Vec<TransientLight>;
}
impl StudioTimelineLights for super::render_studio_timeline::StudioTimeline {
    fn lights_at(&self, time: f64) -> Vec<TransientLight> {
        let smooth = self.interpolation != super::render_studio_timeline::StudioInterpolation::Linear;
        self.light_layers.iter().filter(|l| l.enabled).flat_map(|l| &l.lights)
            .filter_map(|l| l.render_light(time, smooth)).take(MAX_LIGHTS).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct StudioLightSelection { pub layer: usize, pub light: Option<usize>, pub key: Option<usize> }
#[derive(Clone, Copy)]
pub(in crate::app) struct StudioLightDrag {
    pub selection: StudioLightSelection, pub pointer_x: f32, pub scroll: f32, pub initial: f64,
}
#[derive(Clone)]
pub(in crate::app) struct StudioLightHandleDrag {
    pub selection:StudioLightSelection,pub target:bool,pub key:StudioLightKey,
    pub spatial:super::render_studio_spatial::StudioSpatialDrag,
}
#[derive(Default)]
pub(in crate::app) struct StudioLightUi {
    pub selection: Option<StudioLightSelection>,
    pub drag: Option<StudioLightDrag>,
    pub position_drag: Option<StudioLightHandleDrag>,
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn scene_lighting_blackout_zeroes_only_ambient_and_sun() {
        let mut rig=StudioEnvironment::default();
        assert!(rig.valid());
        assert_eq!(rig.sample(15.0,false).ambient_intensity,1.0);
        assert_eq!(rig.sample(15.0,false).sun_intensity,0.0);
        let mut dark=rig.keys[0];
        dark.ambient_intensity=0.0;dark.sun_intensity=0.0;
        assert_eq!(rig.add_key(dark),Some(0));
        let output=rig.sample(800.0,true).render_sample();
        assert_eq!(output.ambient[3],0.0);
        assert_eq!(output.sun[3],0.0);
        assert!((glam::Vec3::from_array([output.sun_direction[0],output.sun_direction[1],output.sun_direction[2]]).length()-1.0).abs()<1e-5);
    }
    #[test] fn scene_lighting_keys_interpolate_scrub_and_legacy_projects_default() {
        let mut rig=StudioEnvironment::default();
        let mut next=rig.keys[0];next.time=10.0;next.ambient_intensity=0.0;next.sun_intensity=3.0;
        assert_eq!(rig.add_key(next),Some(1));
        assert_eq!(rig.sample(0.0,false).sun_intensity,0.0);
        assert_eq!(rig.sample(5.0,false).sun_intensity,1.5);
        assert_eq!(rig.sample(10.0,true).sun_intensity,3.0);
        assert_eq!(rig.sample(50.0,true).sun_intensity,3.0);
        assert_eq!(rig.sample(5.0,false),rig.sample(5.0,false));
        assert!(rig.valid());
        let json=serde_json::to_string(&rig).unwrap();
        let loaded:StudioEnvironment=serde_json::from_str(&json).unwrap();
        assert_eq!(rig,loaded);
        // Missing entire field on older projects gets the default rig.
        let wrapper=serde_json::json!({});
        #[derive(serde::Deserialize)]
        struct Legacy { #[serde(default)] scene_lighting:StudioEnvironment }
        let migrated:Legacy=serde_json::from_value(wrapper).unwrap();
        assert_eq!(migrated.scene_lighting,StudioEnvironment::default());
        next.sun_direction=[0.0;3];assert!(!next.valid());
    }
        #[test] fn spotlight_handle_changes_direction_without_changing_range_or_segment_length() {
        let mut key=StudioLightKey::default();let range=key.radius;
        key.aim_at([1000.0,300.0,40.0]);
        let direction=glam::Vec3::from_array(key.target)-glam::Vec3::from_array(key.position);
        assert!((direction.length()-SPOT_HANDLE_LENGTH).abs()<1e-4);assert_eq!(key.radius,range);
        key.move_position([-200.0,80.0,10.0]);
        let moved=glam::Vec3::from_array(key.aim_handle())-glam::Vec3::from_array(key.position);
        assert!((moved-direction).length()<1e-4);assert_eq!(key.radius,range);
        let mut source=light();source.kind=StudioLightKind::Spot;source.keys=vec![key];
        let spot=source.render_light(3.0,false).unwrap().spot.unwrap();
        let rendered=glam::Vec3::from_array(scene::render_position(direction.normalize().to_array()));
        assert!(rendered.dot(glam::Vec3::from_array(spot.direction))>0.99999);
        source.kind=StudioLightKind::Point;assert!(source.render_light(3.0,false).unwrap().spot.is_none());
    }
    fn light() -> StudioLight { StudioLight { name: "Key".into(), kind: StudioLightKind::Point,
        emitter: StudioLightEmitter::default(), enabled: true, start: 2.0, duration: 5.0, keys: vec![StudioLightKey::default(),
            StudioLightKey { time: 4.0, position: [164.0, -64.0, 96.0], intensity: 6.0, ..Default::default() }] } }
    #[test] fn light_motion_and_properties_use_output_time_and_hold_endpoints() {
        let source = light();
        let mid = source.sample(4.0, false);
        assert_eq!(mid.position, [114.0, -64.0, 96.0]); assert_eq!(mid.intensity, 4.0);
        assert_eq!(source.sample(100.0, false).position, source.keys[1].position);
        assert!(source.render_light(1.0, false).is_none()); assert!(source.render_light(7.0, false).is_none());
        assert_ne!(source.sample(3.0, true).position, source.sample(3.0, false).position);
    }
    #[test] fn group_visibility_and_independent_sources_survive_serialization() {
        let mut timeline = super::super::render_studio_timeline::StudioTimeline::default();
        let mut layer = StudioLightLayer::new(1); layer.lights = vec![light(), light()];
        layer.lights[1].kind = StudioLightKind::Spot;
        timeline.light_layers.push(layer);
        let encoded = serde_json::to_string(&timeline.light_layers).unwrap();
        timeline.light_layers = serde_json::from_str(&encoded).unwrap();
        assert!(valid_layers(&timeline.light_layers)); assert_eq!(timeline.lights_at(3.0).len(), 2);
        let spot = timeline.lights_at(3.0)[1].spot.unwrap();
        assert!((glam::Vec3::from_array(spot.direction).length() - 1.0).abs() < 1e-5);
        assert!(spot.inner_cone_cos >= spot.outer_cone_cos);
        timeline.light_layers[0].lights[0].enabled = false; assert_eq!(timeline.lights_at(3.0).len(), 1);
        timeline.light_layers[0].enabled = false; assert!(timeline.lights_at(3.0).is_empty());
    }
    #[test] fn emitter_waveforms_evaluate_in_output_time_and_survive_scrubbing() {
        let mut source = light();
        source.keys = vec![StudioLightKey { intensity: 4.0, ..Default::default() }];
        source.start = 2.0;
        source.duration = 10.0;
        source.emitter.kind = StudioLightEmitterKind::Strobe;
        source.emitter.frequency = 2.0;
        source.emitter.duty_cycle = 0.25;
        assert_eq!(source.render_light(2.0, false).unwrap().intensity, 4.0);
        assert!(source.render_light(2.2, false).is_none());
        assert_eq!(source.render_light(2.5, false).unwrap().intensity, 4.0);
        // Seeking backward and forward produces the identical illumination.
        let sampled = source.render_light(2.2, false);
        let _ = source.render_light(2.5, true);
        assert_eq!(sampled.is_some(), source.render_light(2.2, false).is_some());
        source.emitter.floor = 0.25;
        assert_eq!(source.render_light(2.2, false).unwrap().intensity, 1.0);
        source.emitter.kind = StudioLightEmitterKind::Pulse;
        source.emitter.frequency = 1.0;
        source.emitter.floor = 0.0;
        assert!((source.render_light(2.25, false).unwrap().intensity - 4.0).abs() < 1e-5);
        assert!(source.render_light(2.75, false).is_none());
        source.emitter.kind = StudioLightEmitterKind::Triangle;
        assert!((source.render_light(2.25, false).unwrap().intensity - 4.0).abs() < 1e-5);
        source.emitter.kind = StudioLightEmitterKind::Sawtooth;
        assert!((source.render_light(2.25, false).unwrap().intensity - 1.0).abs() < 1e-5);
        source.emitter.kind = StudioLightEmitterKind::InverseSawtooth;
        assert!((source.render_light(2.25, false).unwrap().intensity - 3.0).abs() < 1e-5);
        for kind in [StudioLightEmitterKind::Flicker, StudioLightEmitterKind::Noise] {
            source.emitter.kind = kind;
            assert_eq!(source.render_light(2.33, false).map(|l| l.intensity),
                source.render_light(2.33, false).map(|l| l.intensity));
        }
    }
    #[test] fn emitter_defaults_for_old_projects_and_validates_parameters() {
        let source = light();
        let mut value = serde_json::to_value(&source).unwrap();
        value.as_object_mut().unwrap().remove("emitter");
        let loaded: StudioLight = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.emitter.kind, StudioLightEmitterKind::Continuous);
        assert!(loaded.valid());
        assert_eq!(loaded.render_light(3.0, false).unwrap().intensity,
            source.render_light(3.0, false).unwrap().intensity);
        let mut invalid = source;
        invalid.emitter.frequency = f32::NAN;
        assert!(!invalid.valid());
        invalid.emitter.frequency = 1.0;
        invalid.emitter.duty_cycle = 0.0;
        assert!(!invalid.valid());
    }
    #[test] fn invalid_keys_and_duplicate_times_are_rejected() {
        let mut source = light(); source.keys[0].radius = f32::NAN; assert!(!source.valid());
        source = light(); source.keys[1].time = 0.0; assert!(!source.valid());
        source = light(); source.keys[0].inner_angle = 89.0; assert!(!source.valid());
        source = light(); assert_eq!(source.add_key(StudioLightKey { time: 2.0, ..Default::default() }), Some(1));
        assert_eq!(source.add_key(StudioLightKey { time: 2.0, intensity: 3.0, ..Default::default() }), Some(1));
        assert_eq!(source.keys.len(), 3);
    }
}
