//! C ABI to the statically linked EalMan/OpenAL scene. Created, used and dropped
//! exclusively by the EAL worker; no native object crosses an audio thread.
pub(crate) const MAX_SOURCES: usize = 96;
pub(crate) const FRAMES: usize = 256;
/// Native PCM input keeps one block ahead to retain continuous filter history.
pub(crate) const PREROLL_BLOCKS: u64 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct VoiceBlock {
    pub id: u64,
    pub position: [f32; 3],
    pub active: u32,
    pub name: [u8; 96],
    pub pcm: [f32; FRAMES],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SceneFrame {
    pub listener: [f32; 3],
    pub ahead: [f32; 3],
    pub up: [f32; 3],
    pub hrtf: u32,
    /// 0=EAX callback (queue fallback); 1=PCM; 2..6=queue diagnostics;
    /// 7=forced EAX callback; 8=callback tone; 9=legacy EAX queue.
    pub debug_mode: u32,
    pub voices: [VoiceBlock; MAX_SOURCES],
}
// Enforce Rust/C++ ABI layout at compile time (native/eal_scene.cpp also asserts).
const _: [(); 1144] = [(); std::mem::size_of::<VoiceBlock>()];
const _: [(); 109872] = [(); std::mem::size_of::<SceneFrame>()];

impl SceneFrame {
    pub fn silent() -> Self {
        // All fields are numeric; zero is valid even for inactive voice slots.
        let mut frame: Self = unsafe { std::mem::zeroed() };
        frame.ahead = [1.0, 0.0, 0.0];
        frame.up = [0.0, 0.0, 1.0];
        frame
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct SceneStats {
    pub sources: u32,
    pub rooms: u32,
    pub filtered_sources: u32,
    pub diffracted_sources: u32,
    pub directional_rooms: u32,
    pub hrtf: u32,
    pub queue_starves: u32,
    pub restarts: u32,
    pub multi_processed: u32,
    pub buffer_reuse: u32,
    pub input_edge_max: f32,
    pub output_edge_max: f32,
    pub callback_underfeed_samples: u64,
    pub callback_dropped_samples: u64,
    /// 0=raw PCM reference, 1=callback transport, 2=legacy queue.
    pub transport: u32,
    pub hrtf_status: u32,
}

const _: [(); 72] = [(); std::mem::size_of::<SceneStats>()];

pub(crate) struct NativeScene {
    #[cfg(windows)]
    scene: *mut std::ffi::c_void,
}
#[cfg(windows)]
unsafe extern "C" {
    fn djk_eal_create(
        dll: *const u16,
        data: *const u8,
        bytes: u32,
        rate: u32,
        error: *mut u8,
        capacity: u32,
    ) -> *mut std::ffi::c_void;
    fn djk_eal_render(
        scene: *mut std::ffi::c_void,
        frame: *const SceneFrame,
        stereo: *mut f32,
        stats: *mut SceneStats,
        error: *mut u8,
        capacity: u32,
    ) -> i32;
    fn djk_eal_destroy(scene: *mut std::ffi::c_void);
}
impl NativeScene {
    #[cfg(windows)]
    pub fn new(rate: u32, map: &crate::eal::EalMap) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        let path = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("missing executable directory")?
            .join("OpenALSoft.x86_64.dll");
        #[cfg(test)]
        let path = std::env::var_os("EAL_TEST_OPENAL_DLL")
            .map(std::path::PathBuf::from)
            .unwrap_or(path);
        let dll: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut error = [0u8; 512];
        let scene = unsafe {
            djk_eal_create(
                dll.as_ptr(),
                map.raw_data().as_ptr(),
                map.raw_data().len() as u32,
                rate,
                error.as_mut_ptr(),
                error.len() as u32,
            )
        };
        if scene.is_null() {
            return Err(message(&error));
        }
        Ok(Self { scene })
    }
    #[cfg(not(windows))]
    pub fn new(_: u32, _: &crate::eal::EalMap) -> Result<Self, String> {
        Err("Native EAL scene currently requires Windows".into())
    }

    pub fn render(
        &mut self,
        frame: &SceneFrame,
        output: &mut [f32; FRAMES * 2],
    ) -> Result<SceneStats, String> {
        #[cfg(windows)]
        {
            let mut stats = SceneStats::default();
            let mut error = [0u8; 512];
            if unsafe {
                djk_eal_render(
                    self.scene,
                    frame,
                    output.as_mut_ptr(),
                    &mut stats,
                    error.as_mut_ptr(),
                    error.len() as u32,
                )
            } == 0
            {
                return Err(message(&error));
            }
            Ok(stats)
        }
        #[cfg(not(windows))]
        {
            let _ = (frame, output);
            Err("Native EAL scene currently requires Windows".into())
        }
    }
}
#[cfg(windows)]
fn message(error: &[u8]) -> String {
    String::from_utf8_lossy(error.split(|&c| c == 0).next().unwrap_or_default()).into_owned()
}
#[cfg(windows)]
impl Drop for NativeScene {
    fn drop(&mut self) {
        unsafe { djk_eal_destroy(self.scene) };
    }
}

#[cfg(all(test, windows))]
#[path = "eal_native_tests.rs"]
pub(crate) mod tests;
