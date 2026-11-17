use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
    OnceLock,
};
use std::time::Instant;

pub const SLOT_COUNT: usize = 32;

#[cfg(windows)]
mod windows;
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
    Worker8 = 11,
    Worker9 = 12,
    Worker10 = 13,
    Worker11 = 14,
    Worker12 = 15,
    Worker13 = 16,
    Worker14 = 17,
    Worker15 = 18,
    Event0 = 19,
    Event1 = 20,
    Event2 = 21,
    Event3 = 22,
    Event4 = 23,
    Event5 = 24,
    Event6 = 25,
    Event7 = 26,
    Asset0 = 27,
    Asset1 = 28,
    UiCatalog = 29,
    BackgroundIo = 30,
    BackgroundCpu = 31,
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
            8 => Some(Self::Worker8),
            9 => Some(Self::Worker9),
            10 => Some(Self::Worker10),
            11 => Some(Self::Worker11),
            12 => Some(Self::Worker12),
            13 => Some(Self::Worker13),
            14 => Some(Self::Worker14),
            15 => Some(Self::Worker15),
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
    UiBuild = 20,
    UiTessellate = 21,
    UiCatalog = 22,
    MapMaterials = 23,
    MapGeometry = 24,
    MapFxLightProfiles = 25,
    BackgroundIo = 26,
    BackgroundCpu = 27,
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
            Self::UiBuild => "UI BUILD",
            Self::UiTessellate => "UI TESSELLATE",
            Self::UiCatalog => "UI CATALOG",
            Self::MapMaterials => "MATERIALS",
            Self::MapGeometry => "WORLD GEOMETRY",
            Self::MapFxLightProfiles => "FX LIGHT PROFILES",
            Self::BackgroundIo => "BACKGROUND I/O",
            Self::BackgroundCpu => "BACKGROUND CPU",
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
            20 => Self::UiBuild,
            21 => Self::UiTessellate,
            22 => Self::UiCatalog,
            23 => Self::MapMaterials,
            24 => Self::MapGeometry,
            25 => Self::MapFxLightProfiles,
            26 => Self::BackgroundIo,
            27 => Self::BackgroundCpu,
            _ => Self::Idle,
        }
    }
}

struct SlotState {
    thread_id: AtomicU32,
    thread_created: AtomicU64,
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
            thread_id: AtomicU32::new(0),
            thread_created: AtomicU64::new(0),
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
    /// Task of an enclosing guard on the same slot, restored on drop. The
    /// enclosing guard keeps ownership of the busy-time accounting.
    outer_task: Option<u8>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let now = now_ns();
        let state = state(self.slot);
        if let Some(outer) = self.outer_task {
            state.task.store(outer, Ordering::Relaxed);
            return;
        }
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
    #[cfg(windows)]
    {
        thread_local! { static ID: (u32, u64) = windows::current_identity(); }
        ID.with(|&(id, created)| {
            let slot = state(slot);
            if slot.thread_id.load(Ordering::Relaxed) != id || slot.thread_created.load(Ordering::Relaxed) != created {
                slot.thread_created.store(created, Ordering::Relaxed);
                slot.thread_id.store(id, Ordering::Release);
            }
        });
    }
    let now = now_ns();
    let state = state(slot);
    if state.active.load(Ordering::Acquire) {
        // Nested section (e.g. UI build inside the main tick): relabel the
        // slot without restarting or double-counting its busy interval.
        let outer = state.task.swap(task as u8, Ordering::Relaxed);
        return ActivityGuard {
            slot,
            started_ns: now,
            outer_task: Some(outer),
        };
    }
    state.task.store(task as u8, Ordering::Relaxed);
    state.busy_started_ns.store(now, Ordering::Relaxed);
    state.active.store(true, Ordering::Release);
    ActivityGuard {
        slot,
        started_ns: now,
        outer_task: None,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RawThreadActivity {
    pub slot: ThreadSlot,
    #[cfg_attr(windows, allow(dead_code))] // read by the non-Windows sampler
    pub active: bool,
    pub task: Task,
    #[cfg_attr(windows, allow(dead_code))] // read by the non-Windows sampler
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
        ThreadSlot::Worker8,
        ThreadSlot::Worker9,
        ThreadSlot::Worker10,
        ThreadSlot::Worker11,
        ThreadSlot::Worker12,
        ThreadSlot::Worker13,
        ThreadSlot::Worker14,
        ThreadSlot::Worker15,
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
        ThreadSlot::UiCatalog,
        ThreadSlot::BackgroundIo,
        ThreadSlot::BackgroundCpu,
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
        ThreadSlot::Worker8 => "WORKER 8",
        ThreadSlot::Worker9 => "WORKER 9",
        ThreadSlot::Worker10 => "WORKER 10",
        ThreadSlot::Worker11 => "WORKER 11",
        ThreadSlot::Worker12 => "WORKER 12",
        ThreadSlot::Worker13 => "WORKER 13",
        ThreadSlot::Worker14 => "WORKER 14",
        ThreadSlot::Worker15 => "WORKER 15",
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
        ThreadSlot::UiCatalog => "UI CATALOG",
        ThreadSlot::BackgroundIo => "BACKGROUND I/O",
        ThreadSlot::BackgroundCpu => "BACKGROUND CPU",
    }
}

#[cfg(not(windows))]
static SAMPLE_LAST_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(windows))]
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

#[derive(Debug, Clone)]
pub struct ThreadActivitySample {
    pub id: u32,
    pub name: std::sync::Arc<str>,
    pub active: bool,
    pub task: &'static str,
    pub busy_percent: f32,
}

#[cfg(not(windows))]
fn scoped_sample() -> [ThreadActivitySample; SLOT_COUNT] {
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
            id: 0,
            name: slot_label(entry.slot).into(),
            active: entry.active,
            task: entry.task.label(),
            busy_percent,
        }
    })
}

/// Only the advanced HUD requests sampling. Windows measures kernel + user CPU
/// time, excluding channel waits, joins, sleeping and GPU waits. Other platforms
/// retain the explicitly labelled scoped wall-time fallback.
pub fn sample(enabled: bool) -> std::sync::Arc<[ThreadActivitySample]> {
    #[cfg(windows)]
    { windows::sample(enabled) }
    #[cfg(not(windows))]
    { if enabled { std::sync::Arc::from(scoped_sample()) } else { std::sync::Arc::from([]) } }
}

pub const CPU_SAMPLE_LABEL: &str = if cfg!(windows) { "THREAD CPU / CORE" } else { "THREAD SCOPED BUSY" };

/// Label the otherwise unnamed global Rayon threads on their first job. Named
/// dedicated pools keep their identity, and the caller helping a pool stays MAIN.
pub fn label_rayon_worker() {
    #[cfg(windows)]
    {
        thread_local! { static LABELLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
        LABELLED.with(|labelled| {
            if labelled.get() { return; }
            if let Some(index) = rayon::current_thread_index() {
                if std::thread::current().name().is_none() {
                    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadDescription};
                    let name: Vec<u16> = format!("jka-rayon-{index}").encode_utf16().chain(Some(0)).collect();
                    unsafe { SetThreadDescription(GetCurrentThread(), name.as_ptr()); }
                }
                labelled.set(true);
            }
        });
    }
}
