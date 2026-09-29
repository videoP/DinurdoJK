//! Optional client-side event preparation workers.
//!
//! The receive queue remains the authority for event ordering. Workers only
//! perform side-effect-free semantic preparation; all presenter/audio/FX state
//! mutation is applied later on the CGame thread in `receive_sequence` order.

use std::{sync::OnceLock, time::Instant};

use jka_assets::{saber::SaberDefinitions, siege::SiegeClassVisual};
use rayon::prelude::*;

use crate::{
    audio::{SoundDecodeJob, SoundDecodeResult},
    thread_activity::{self, Task, ThreadSlot},
};

use super::{
    event_presenter::{prepare_event_visual, PreparedEventVisual},
    sound_presenter::{prepare_sound_event, PreparedSoundEvent},
    weapon_fx::{prepare_entity_event, PreparedFxEvent},
    ClientGameState, PresentationEvent,
};

const MAX_EVENT_WORKERS: usize = 8;

#[derive(Debug)]
pub(crate) struct PreparedPresentationEvent {
    pub event: PresentationEvent,
    pub sound: Option<PreparedSoundEvent>,
    pub fx: PreparedFxEvent,
    pub visual: PreparedEventVisual,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EventWorkerStats {
    pub jobs: u32,
    pub pool_threads: u32,
    pub parallel: bool,
    pub wall_ms: f64,
}

#[derive(Debug)]
pub(crate) struct PreparedEventBatch {
    pub events: Vec<PreparedPresentationEvent>,
    pub stats: EventWorkerStats,
}

pub(crate) struct SoundDecodeBatch {
    pub results: Vec<SoundDecodeResult>,
    pub stats: EventWorkerStats,
}

static EVENT_WORKER_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

fn requested_worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .saturating_sub(1)
        .clamp(1, MAX_EVENT_WORKERS)
}

fn worker_pool() -> Option<&'static rayon::ThreadPool> {
    EVENT_WORKER_POOL
        .get_or_init(|| {
            let workers = requested_worker_count();
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .thread_name(|index| format!("jka-event-{index}"))
                .build()
                .map_err(|error| eprintln!("Event worker pool unavailable: {error}"))
                .ok()
        })
        .as_ref()
}

pub(crate) fn worker_count() -> usize {
    worker_pool().map_or(0, |pool| pool.current_num_threads())
}

fn prepare_one(
    event: PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    sound_saber_definitions: Option<&SaberDefinitions>,
    fx_saber_definitions: &SaberDefinitions,
) -> PreparedPresentationEvent {
    let sound = sound_saber_definitions
        .map(|definitions| prepare_sound_event(&event, game, siege_classes, definitions));
    let fx = prepare_entity_event(&event, game, siege_classes, fx_saber_definitions);
    let visual = prepare_event_visual(&event, game);
    PreparedPresentationEvent {
        event,
        sound,
        fx,
        visual,
    }
}

fn prepare_one_profiled(
    event: PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    sound_saber_definitions: Option<&SaberDefinitions>,
    fx_saber_definitions: &SaberDefinitions,
) -> PreparedPresentationEvent {
    // This function is only called from EVENT_WORKER_POOL. Rayon assigns a
    // stable 0-based index to each worker in that pool, so each event thread
    // gets its own independent profiler slot instead of colliding with the
    // map-job WORKER 0..7 slots.
    let _activity = rayon::current_thread_index()
        .and_then(ThreadSlot::event_worker)
        .map(|slot| thread_activity::activity(slot, Task::EventPrep));
    prepare_one(
        event,
        game,
        siege_classes,
        sound_saber_definitions,
        fx_saber_definitions,
    )
}

pub(crate) fn prepare_inline(
    event: PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    sound_saber_definitions: Option<&SaberDefinitions>,
    fx_saber_definitions: &SaberDefinitions,
) -> PreparedPresentationEvent {
    prepare_one(
        event,
        game,
        siege_classes,
        sound_saber_definitions,
        fx_saber_definitions,
    )
}

fn decode_sound_profiled(job: SoundDecodeJob) -> SoundDecodeResult {
    let _activity = rayon::current_thread_index()
        .and_then(ThreadSlot::event_worker)
        .map(|slot| thread_activity::activity(slot, Task::EventSoundDecode));
    job.decode()
}

/// Decode already-staged compressed SFX bytes. The VFS read and the eventual
/// cache/audio mutation remain on the owner thread; workers only run codec CPU.
/// Indexed parallel collect keeps first-request order for deterministic commit.
pub(crate) fn decode_sound_batch(
    jobs: Vec<SoundDecodeJob>,
    workers_enabled: bool,
) -> SoundDecodeBatch {
    let started = Instant::now();
    let job_count = u32::try_from(jobs.len()).unwrap_or(u32::MAX);
    let pool = workers_enabled
        .then(worker_pool)
        .flatten()
        .filter(|_| jobs.len() > 1);

    let (results, parallel, pool_threads) = if let Some(pool) = pool {
        let results = pool.install(|| {
            jobs
                .into_par_iter()
                .map(decode_sound_profiled)
                .collect::<Vec<_>>()
        });
        (results, true, pool.current_num_threads() as u32)
    } else {
        (
            jobs.into_iter().map(SoundDecodeJob::decode).collect(),
            false,
            0,
        )
    };

    SoundDecodeBatch {
        results,
        stats: EventWorkerStats {
            jobs: job_count,
            pool_threads,
            parallel,
            wall_ms: started.elapsed().as_secs_f64() * 1000.0,
        },
    }
}

/// Prepare a receive-queue batch. `workers_enabled=false` deliberately runs
/// the identical semantic preparation on the CGame thread, making this a clean
/// A/B test of execution location rather than two behavior implementations.
///
/// Single-event batches stay on the caller even when enabled. Real JKA traffic
/// is often one event at a time; paying a parallel dispatch barrier for a lone
/// cheap event would only manufacture overhead for the benchmark.
pub(crate) fn prepare_batch(
    events: Vec<PresentationEvent>,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    sound_saber_definitions: Option<&SaberDefinitions>,
    fx_saber_definitions: &SaberDefinitions,
    workers_enabled: bool,
) -> PreparedEventBatch {
    let started = Instant::now();
    let jobs = u32::try_from(events.len()).unwrap_or(u32::MAX);
    let pool = workers_enabled
        .then(worker_pool)
        .flatten()
        .filter(|_| events.len() > 1);

    let (events, parallel, pool_threads) = if let Some(pool) = pool {
        // Vec's indexed parallel iterator preserves input ordering on collect,
        // so the CGame application phase still sees receive_sequence order.
        let prepared = pool.install(|| {
            events
                .into_par_iter()
                .map(|event| {
                    prepare_one_profiled(
                        event,
                        game,
                        siege_classes,
                        sound_saber_definitions,
                        fx_saber_definitions,
                    )
                })
                .collect::<Vec<_>>()
        });
        (prepared, true, pool.current_num_threads() as u32)
    } else {
        (
            events
                .into_iter()
                .map(|event| {
                    prepare_one(
                        event,
                        game,
                        siege_classes,
                        sound_saber_definitions,
                        fx_saber_definitions,
                    )
                })
                .collect(),
            false,
            0,
        )
    };

    debug_assert!(events.windows(2).all(|pair| {
        pair[0].event.receive_sequence < pair[1].event.receive_sequence
    }));

    PreparedEventBatch {
        events,
        stats: EventWorkerStats {
            jobs,
            pool_threads,
            parallel,
            wall_ms: started.elapsed().as_secs_f64() * 1000.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_protocol::{
        entity_event::EntityEvent,
        gamestate::{EntityState, ENTITY_FIELDS},
    };

    fn event(sequence: u64, kind: EntityEvent) -> PresentationEvent {
        PresentationEvent {
            receive_sequence: sequence,
            source_entity_num: sequence as u16,
            entity_num: sequence as u16,
            event: kind,
            raw_event: kind as i32,
            parm: 0,
            position: [0.0; 3],
            event_only_entity: false,
            server_time: sequence as i32,
            state: EntityState {
                number: sequence as u16,
                fields: [0; ENTITY_FIELDS.len()],
            },
        }
    }

    #[test]
    fn worker_batch_preserves_receive_queue_order() {
        let game = ClientGameState::new();
        let definitions = SaberDefinitions::default();
        let input = vec![
            event(7, EntityEvent::EV_SABER_HIT),
            event(8, EntityEvent::EV_FOOTSTEP),
            event(9, EntityEvent::EV_SABER_BLOCK),
        ];
        let batch = prepare_batch(input, &game, &[], Some(&definitions), &definitions, true);
        let sequences = batch
            .events
            .iter()
            .map(|prepared| prepared.event.receive_sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![7, 8, 9]);
        assert_eq!(batch.stats.jobs, 3);
    }

    #[test]
    fn one_event_never_uses_parallel_barrier() {
        let game = ClientGameState::new();
        let definitions = SaberDefinitions::default();
        let batch = prepare_batch(
            vec![event(11, EntityEvent::EV_SABER_HIT)],
            &game,
            &[],
            Some(&definitions),
            &definitions,
            true,
        );
        assert!(!batch.stats.parallel);
        assert_eq!(batch.stats.pool_threads, 0);
    }
}
