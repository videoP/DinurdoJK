use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    OnceLock,
};
use std::time::Instant;

pub const SLOT_COUNT: usize = 21;
const RECENT_NS: u64 = 750_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadSlot {
    Main = 0,
    Render = 1,
    MapLoader = 2,
    Worker0 = 3,
    Worker1 = 4,
    Worker2 = 5,
    Worker3 = 6,
    Worker4 = 7,
    Worker5 = 8,
    Worker6 = 9,
    Worker7 = 10,
    Event0 = 11,
    Event1 = 12,
    Event2 = 13,
    Event3 = 14,
    Event4 = 15,
    Event5 = 16,
    Event6 = 17,
    Event7 = 18,
    Asset0 = 19,
    Asset1 = 20,
}

impl ThreadSlot {
    pub const fn worker(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Worker0),
            1 => Some(Self::Worker1),
            2 => Some(Self::Worker2),
            3 => Some(Self::Worker3),
            4 => Some(Self::Worker4),
            5 => Some(Self::Worker5),
            6 => Some(Self::Worker6),
            7 => Some(Self::Worker7),
            _ => None,
        }
    }

    pub const fn asset_worker(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Asset0),
            1 => Some(Self::Asset1),
            _ => None,
        }
    }

    pub const fn event_worker(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Event0),
            1 => Some(Self::Event1),
            2 => Some(Self::Event2),
            3 => Some(Self::Event3),
            4 => Some(Self::Event4),
            5 => Some(Self::Event5),
            6 => Some(Self::Event6),
            7 => Some(Self::Event7),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Task {
    Idle = 0,
    Input = 1,
    MainTick = 2,
    Event = 3,
    RenderFrame = 4,
    RenderCommands = 5,
    MapPrepare = 6,
    MapBspParse = 7,
    MapCollision = 8,
    MapShaderParse = 9,
    MapTextureDecode = 10,
    MapGrass = 11,
    MapGi = 12,
    MapOcean = 13,
    MapAcoustics = 14,
    MapAudioBake = 15,
    MapPortalPlans = 16,
    EventPrep = 17,
    EventSoundDecode = 18,
    AssetLoad = 19,
}

impl Task {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Input => "INPUT",
            Self::MainTick => "SIM / CLIENT TICK",
            Self::Event => "EVENT DISPATCH",
            Self::RenderFrame => "RENDER FRAME",
            Self::RenderCommands => "RENDER COMMANDS",
            Self::MapPrepare => "MAP PREP",
            Self::MapBspParse => "BSP PARSE",
            Self::MapCollision => "COLLISION BUILD",
            Self::MapShaderParse => "SHADER PARSE",
            Self::MapTextureDecode => "TEXTURE DECODE",
            Self::MapGrass => "GRASS BUILD",
            Self::MapGi => "VOXEL GI",
            Self::MapOcean => "OCEAN MESH",
            Self::MapAcoustics => "ACOUSTICS",
            Self::MapAudioBake => "AUDIO BAKE",
            Self::MapPortalPlans => "PVS DRAW PLANS",
            Self::EventPrep => "EVENT PREP",
            Self::EventSoundDecode => "EVENT AUDIO DECODE",
            Self::AssetLoad => "ASSET LOAD",
        }
    }

    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Input,
            2 => Self::MainTick,
            3 => Self::Event,
            4 => Self::RenderFrame,
            5 => Self::RenderCommands,
            6 => Self::MapPrepare,
            7 => Self::MapBspParse,
            8 => Self::MapCollision,
            9 => Self::MapShaderParse,
            10 => Self::MapTextureDecode,
            11 => Self::MapGrass,
            12 => Self::MapGi,
            13 => Self::MapOcean,
            14 => Self::MapAcoustics,
            15 => Self::MapAudioBake,
            16 => Self::MapPortalPlans,
            17 => Self::EventPrep,
            18 => Self::EventSoundDecode,
            19 => Self::AssetLoad,
            _ => Self::Idle,
        }
    }
}

struct SlotState {
    active: AtomicBool,
    task: AtomicU8,
    last_task: AtomicU8,
    busy_total_ns: AtomicU64,
    busy_started_ns: AtomicU64,
    last_active_ns: AtomicU64,
}

impl SlotState {
    const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            task: AtomicU8::new(Task::Idle as u8),
            last_task: AtomicU8::new(Task::Idle as u8),
            busy_total_ns: AtomicU64::new(0),
            busy_started_ns: AtomicU64::new(0),
            last_active_ns: AtomicU64::new(0),
        }
    }
}

static START: OnceLock<Instant> = OnceLock::new();
static SLOTS: [SlotState; SLOT_COUNT] = [
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
    SlotState::new(),
];

fn now_ns() -> u64 {
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn state(slot: ThreadSlot) -> &'static SlotState {
    &SLOTS[slot as usize]
}

pub struct ActivityGuard {
    slot: ThreadSlot,
    started_ns: u64,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let now = now_ns();
        let state = state(self.slot);
        state
            .busy_total_ns
            .fetch_add(now.saturating_sub(self.started_ns), Ordering::Relaxed);
        let task = state.task.swap(Task::Idle as u8, Ordering::Relaxed);
        state.last_task.store(task, Ordering::Relaxed);
        state.last_active_ns.store(now, Ordering::Relaxed);
        state.active.store(false, Ordering::Release);
    }
}

pub fn activity(slot: ThreadSlot, task: Task) -> ActivityGuard {
    let now = now_ns();
    let state = state(slot);
    state.task.store(task as u8, Ordering::Relaxed);
    state.busy_started_ns.store(now, Ordering::Relaxed);
    state.active.store(true, Ordering::Release);
    ActivityGuard {
        slot,
        started_ns: now,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RawThreadActivity {
    pub slot: ThreadSlot,
    pub active: bool,
    pub task: Task,
    pub busy_total_ns: u64,
}

pub fn snapshot() -> [RawThreadActivity; SLOT_COUNT] {
    let now = now_ns();
    [
        ThreadSlot::Main,
        ThreadSlot::Render,
        ThreadSlot::MapLoader,
        ThreadSlot::Worker0,
        ThreadSlot::Worker1,
        ThreadSlot::Worker2,
        ThreadSlot::Worker3,
        ThreadSlot::Worker4,
        ThreadSlot::Worker5,
        ThreadSlot::Worker6,
        ThreadSlot::Worker7,
        ThreadSlot::Event0,
        ThreadSlot::Event1,
        ThreadSlot::Event2,
        ThreadSlot::Event3,
        ThreadSlot::Event4,
        ThreadSlot::Event5,
        ThreadSlot::Event6,
        ThreadSlot::Event7,
        ThreadSlot::Asset0,
        ThreadSlot::Asset1,
    ]
    .map(|slot| {
        let state = state(slot);
        let active = state.active.load(Ordering::Acquire);
        let current_task = Task::from_raw(state.task.load(Ordering::Relaxed));
        let last_task = Task::from_raw(state.last_task.load(Ordering::Relaxed));
        let last_active = state.last_active_ns.load(Ordering::Relaxed);
        let mut busy_total_ns = state.busy_total_ns.load(Ordering::Relaxed);
        if active {
            busy_total_ns = busy_total_ns
                .saturating_add(now.saturating_sub(state.busy_started_ns.load(Ordering::Relaxed)));
        }
        let task = if active {
            current_task
        } else if now.saturating_sub(last_active) <= RECENT_NS {
            last_task
        } else {
            Task::Idle
        };
        RawThreadActivity {
            slot,
            active,
            task,
            busy_total_ns,
        }
    })
}

pub const fn slot_label(slot: ThreadSlot) -> &'static str {
    match slot {
        ThreadSlot::Main => "MAIN",
        ThreadSlot::Render => "RENDER",
        ThreadSlot::MapLoader => "MAP LOADER",
        ThreadSlot::Worker0 => "WORKER 0",
        ThreadSlot::Worker1 => "WORKER 1",
        ThreadSlot::Worker2 => "WORKER 2",
        ThreadSlot::Worker3 => "WORKER 3",
        ThreadSlot::Worker4 => "WORKER 4",
        ThreadSlot::Worker5 => "WORKER 5",
        ThreadSlot::Worker6 => "WORKER 6",
        ThreadSlot::Worker7 => "WORKER 7",
        ThreadSlot::Event0 => "EVENT 0",
        ThreadSlot::Event1 => "EVENT 1",
        ThreadSlot::Event2 => "EVENT 2",
        ThreadSlot::Event3 => "EVENT 3",
        ThreadSlot::Event4 => "EVENT 4",
        ThreadSlot::Event5 => "EVENT 5",
        ThreadSlot::Event6 => "EVENT 6",
        ThreadSlot::Event7 => "EVENT 7",
        ThreadSlot::Asset0 => "ASSET 0",
        ThreadSlot::Asset1 => "ASSET 1",
    }
}

static SAMPLE_LAST_NS: AtomicU64 = AtomicU64::new(0);
static SAMPLE_BUSY_NS: [AtomicU64; SLOT_COUNT] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

#[derive(Debug, Clone, Copy)]
pub struct ThreadActivitySample {
    pub slot: ThreadSlot,
    pub active: bool,
    pub task: Task,
    pub busy_percent: f32,
}

pub fn sample() -> [ThreadActivitySample; SLOT_COUNT] {
    let now = now_ns();
    let previous_time = SAMPLE_LAST_NS.swap(now, Ordering::Relaxed);
    let elapsed = now.saturating_sub(previous_time).max(1);
    let raw = snapshot();
    raw.map(|entry| {
        let previous_busy =
            SAMPLE_BUSY_NS[entry.slot as usize].swap(entry.busy_total_ns, Ordering::Relaxed);
        let busy_delta = entry.busy_total_ns.saturating_sub(previous_busy);
        let busy_percent = if previous_time == 0 {
            0.0
        } else {
            (busy_delta as f64 * 100.0 / elapsed as f64).clamp(0.0, 100.0) as f32
        };
        ThreadActivitySample {
            slot: entry.slot,
            active: entry.active,
            task: entry.task,
            busy_percent,
        }
    })
}
