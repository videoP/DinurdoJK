//! Presentation-side equivalent of OpenJK `CG_EntityEvent`.
//!
//! Snapshot/event recognition stays in `cgame.rs`.  This module converts an
//! accepted JKA event into renderer/audio-facing presentation work.  The Rust
//! SoundPresenter handles supported audio events before this visual dispatcher.
//! Its fallback sound cases remain PARTIAL when audio is unavailable.

use std::sync::Arc;

use crate::{
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicModelVertex},
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

#[derive(Default)]
pub struct EventPresenter {
    flashes: Vec<EventFlash>,
}

impl EventPresenter {
    pub fn dispatch(
        &mut self,
        event: &PresentationEvent,
        game: &ClientGameState,
    ) -> EventDispatchResult {
        match event.event {
            // EV_SABER_ATTACK: the persistent blade/hilt is player-state driven.
            // OpenJK also selects swing audio here; audio is the remaining part.
            29 => EventDispatchResult::Partial("SABER_ATTACK_NO_AUDIO"),
            30 => {
                self.flash(event, 95, 9.0, [1.0, 0.78, 0.28]);
                EventDispatchResult::Handled("SABER_HIT_FLASH")
            }
            31 => {
                self.flash(event, 90, 11.0, [0.75, 0.88, 1.0]);
                EventDispatchResult::Handled("SABER_BLOCK_FLASH")
            }
            // EV_SABER_CLASHFLARE: short clash flash at the authored event origin.
            32 => {
                self.flash(event, 85, 12.0, [0.90, 0.92, 1.0]);
                EventDispatchResult::Handled("SABER_CLASH_FLASH")
            }
            // EV_SABER_UNHOLSTER is mostly persistent player/saber state plus sound.
            33 => EventDispatchResult::Partial("SABER_UNHOLSTER_NO_AUDIO"),
            // EV_SHIELD_HIT
            110 => {
                self.flash(event, 120, 13.0, [0.25, 0.55, 1.0]);
                EventDispatchResult::Handled("SHIELD_HIT_FLASH")
            }

            // Configstring-backed sound events.  Resolution is performed here
            // so cg_debugEvents distinguishes "we know exactly what to play"
            // from an event that still lacks semantic handling.
            76 | 77 | 79 => {
                if game.sound_qpath(event.parm).is_some() {
                    EventDispatchResult::Partial("SOUND_NO_AUDIO_BACKEND")
                } else {
                    EventDispatchResult::Partial("SOUND_RESOURCE_MISSING")
                }
            }
            78 => EventDispatchResult::Partial("GLOBAL_TEAM_SOUND_PENDING"),
            // EV_PREDEFSOUND uses eventParm as a predefined enum rather than
            // CS_SOUNDS, but still requires the future audio backend.
            40 => EventDispatchResult::Partial("PREDEFINED_SOUND_NO_AUDIO_BACKEND"),

            // Effects are semantically decoded now, but the general .efx runtime
            // is a separate subsystem.  Keep them visible in cg_debugEvents.
            68 => EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING"),
            69 | 70 => {
                if game.effect_qpath(event.parm).is_some() {
                    EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING")
                } else {
                    EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING")
                }
            }

            // Item state/geometry is not yet presentation-complete.
            22 | 62 | 63 => EventDispatchResult::Partial("ITEM_PRESENTATION_PENDING"),

            // Movement/damage cases currently have visual state already but the
            // event-specific JKA audio/camera feedback is not implemented yet.
            2 | 3 | 4 | 5 | 6 | 7 | 11 | 16 | 89 => {
                EventDispatchResult::Partial("MOVEMENT_DAMAGE_NO_AUDIO")
            }
            // Force drained has source/target semantics that should be ported as
            // a real Force effect rather than approximated as an arbitrary flash.
            96 => EventDispatchResult::Partial("FORCE_DRAIN_FX_PENDING"),
            _ => EventDispatchResult::Unhandled,
        }
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
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        ghoul2_gpu: None,
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
