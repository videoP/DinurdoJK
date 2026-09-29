//! Presentation-side equivalent of OpenJK `CG_EntityEvent`.
//!
//! Snapshot/event recognition stays in `cgame.rs`.  This module converts an
//! accepted JKA event into renderer/audio-facing presentation work.  The Rust
//! SoundPresenter handles supported audio events before this visual dispatcher.
//! Its fallback sound cases remain PARTIAL when audio is unavailable.

use std::sync::Arc;

use jka_protocol::entity_event::EntityEvent;

use crate::{
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex},
    scene,
};

use super::{ClientGameState, PresentationEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDispatchResult {
    Handled(&'static str),
    Partial(&'static str),
    Unhandled,
}

impl EventDispatchResult {
    pub fn status(self) -> String {
        match self {
            Self::Handled(reason) => format!("HANDLED:{reason}"),
            Self::Partial(reason) => format!("PARTIAL:{reason}"),
            Self::Unhandled => "UNHANDLED".to_owned(),
        }
    }

    pub fn class(self) -> EventDispatchClass {
        match self {
            Self::Handled(_) => EventDispatchClass::Handled,
            Self::Partial(_) => EventDispatchClass::Partial,
            Self::Unhandled => EventDispatchClass::Unhandled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDispatchClass {
    Handled,
    Partial,
    Unhandled,
}

#[derive(Debug, Clone)]
struct EventFlash {
    entity_num: u16,
    origin: [f32; 3],
    start_time: i32,
    duration_ms: i32,
    radius: f32,
    color: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparedEventVisual {
    result: EventDispatchResult,
    flash: Option<PreparedFlash>,
}

#[derive(Debug, Clone, Copy)]
struct PreparedFlash {
    duration_ms: i32,
    radius: f32,
    color: [f32; 3],
}

pub(crate) fn prepare_event_visual(
    event: &PresentationEvent,
    game: &ClientGameState,
) -> PreparedEventVisual {
    let (result, flash) = match event.event {
        EntityEvent::EV_SABER_ATTACK => (EventDispatchResult::Partial("SABER_ATTACK_NO_AUDIO"), None),
        // OpenJK EV_SABER_HIT / BLOCK / CLASHFLARE are handled by WeaponFx
        // using the authored .efx path plus CG_SaberClashFlare. Do not fall
        // back to the old hand-made crossed-billboard flash.
        EntityEvent::EV_SABER_HIT
        | EntityEvent::EV_SABER_BLOCK
        | EntityEvent::EV_SABER_CLASHFLARE => (EventDispatchResult::Unhandled, None),
        EntityEvent::EV_SABER_UNHOLSTER => (EventDispatchResult::Partial("SABER_UNHOLSTER_NO_AUDIO"), None),
        EntityEvent::EV_SHIELD_HIT => (
            EventDispatchResult::Handled("SHIELD_HIT_FLASH"),
            Some(PreparedFlash { duration_ms: 120, radius: 13.0, color: [0.25, 0.55, 1.0] }),
        ),
        EntityEvent::EV_GENERAL_SOUND | EntityEvent::EV_GLOBAL_SOUND | EntityEvent::EV_ENTITY_SOUND => {
            let result = if game.sound_qpath(event.parm).is_some() {
                EventDispatchResult::Partial("SOUND_NO_AUDIO_BACKEND")
            } else {
                EventDispatchResult::Partial("SOUND_RESOURCE_MISSING")
            };
            (result, None)
        }
        EntityEvent::EV_GLOBAL_TEAM_SOUND => (EventDispatchResult::Partial("GLOBAL_TEAM_SOUND_PENDING"), None),
        EntityEvent::EV_PREDEFSOUND => (EventDispatchResult::Partial("PREDEFINED_SOUND_NO_AUDIO_BACKEND"), None),
        EntityEvent::EV_PLAY_EFFECT => (EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING"), None),
        EntityEvent::EV_PLAY_EFFECT_ID | EntityEvent::EV_PLAY_PORTAL_EFFECT_ID => {
            let result = if game.effect_qpath(event.parm).is_some() {
                EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING")
            } else {
                EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING")
            };
            (result, None)
        }
        EntityEvent::EV_ITEM_PICKUP | EntityEvent::EV_ITEM_RESPAWN | EntityEvent::EV_ITEM_POP => {
            (EventDispatchResult::Partial("ITEM_PRESENTATION_PENDING"), None)
        }
        EntityEvent::EV_FOOTSTEP
        | EntityEvent::EV_FOOTSTEP_METAL
        | EntityEvent::EV_FOOTSPLASH
        | EntityEvent::EV_FOOTWADE
        | EntityEvent::EV_SWIM
        | EntityEvent::EV_STEP_4
        | EntityEvent::EV_FALL
        | EntityEvent::EV_JUMP
        | EntityEvent::EV_PAIN => (EventDispatchResult::Partial("MOVEMENT_DAMAGE_NO_AUDIO"), None),
        EntityEvent::EV_FORCE_DRAINED => (EventDispatchResult::Partial("FORCE_DRAIN_FX_PENDING"), None),
        _ => (EventDispatchResult::Unhandled, None),
    };
    PreparedEventVisual { result, flash }
}

#[derive(Default)]
pub struct EventPresenter {
    flashes: Vec<EventFlash>,
}

impl EventPresenter {
    pub(crate) fn dispatch_prepared(
        &mut self,
        event: &PresentationEvent,
        prepared: PreparedEventVisual,
    ) -> EventDispatchResult {
        if let Some(flash) = prepared.flash {
            self.flash(event, flash.duration_ms, flash.radius, flash.color);
        }
        prepared.result
    }

    fn flash(&mut self, event: &PresentationEvent, duration_ms: i32, radius: f32, color: [f32; 3]) {
        self.flashes.push(EventFlash {
            entity_num: event.entity_num,
            origin: event.position,
            start_time: event.server_time,
            duration_ms,
            radius,
            color,
        });
    }

    pub fn present(&mut self, current_time: i32) -> Vec<DynamicModelSurface> {
        self.flashes.retain(|flash| {
            current_time.saturating_sub(flash.start_time) <= flash.duration_ms
        });
        self.flashes
            .iter()
            .filter_map(|flash| flash_surface(flash, current_time))
            .collect()
    }

    pub fn clear(&mut self) {
        self.flashes.clear();
    }
}

fn flash_surface(flash: &EventFlash, current_time: i32) -> Option<DynamicModelSurface> {
    let age = current_time.saturating_sub(flash.start_time).max(0) as f32;
    let duration = flash.duration_ms.max(1) as f32;
    let alpha = (1.0 - age / duration).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return None;
    }
    let radius = flash.radius * (0.75 + 0.35 * (age / duration));
    let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut vertices = Vec::with_capacity(12);
    let mut indices = Vec::with_capacity(36);
    for pair in [(0usize, 1usize), (0, 2), (1, 2)] {
        let a = axes[pair.0];
        let b = axes[pair.1];
        let points = [
            add_scaled(flash.origin, a, -radius, b, -radius),
            add_scaled(flash.origin, a, radius, b, -radius),
            add_scaled(flash.origin, a, radius, b, radius),
            add_scaled(flash.origin, a, -radius, b, radius),
        ];
        let base = vertices.len() as u32;
        for (index, point) in points.into_iter().enumerate() {
            vertices.push(DynamicModelVertex {
                position: scene::render_position(point),
                normal: scene::render_position(a),
                uv: match index {
                    0 => [0.0, 0.0],
                    1 => [1.0, 0.0],
                    2 => [1.0, 1.0],
                    _ => [0.0, 1.0],
                },
                color: [flash.color[0], flash.color[1], flash.color[2], alpha * 0.55],
            });
        }
        indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
            base + 2, base + 1, base,
            base + 3, base + 2, base,
        ]);
    }
    Some(DynamicModelSurface {
        entity_num: flash.entity_num,
        wireframe_class: DynamicWireframeClass::Effect,
        raster_visible: true,
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        lighting_origin: None,
        rt_rigid: None,
        rt_skinned_key: None,
        ghoul2_gpu: None,
        fx_gpu_sprites: None,
        texture: None,
        alpha_mode: DynamicModelAlphaMode::Additive,
    })
}

fn add_scaled(
    origin: [f32; 3],
    a: [f32; 3],
    sa: f32,
    b: [f32; 3],
    sb: f32,
) -> [f32; 3] {
    [
        origin[0] + a[0] * sa + b[0] * sb,
        origin[1] + a[1] * sa + b[1] * sb,
        origin[2] + a[2] * sa + b[2] * sb,
    ]
}
