//! Optional OpenAL Soft loopback EAX wet renderer. This never opens an output device;
//! Rodio/CPAL remains the ONLY system audio output. The OpenAL direct path is
//! muted using an EFX low-pass direct filter, leaving only authored reverb.
//! The library is loaded on demand, and absence never prevents the game loading.
use crate::eal::EalPreset;

#[cfg(not(windows))]
pub struct WetReverb;
#[cfg(not(windows))]
impl WetReverb {
    pub fn new(_: u32) -> Result<Self, String> {
        Err("OpenAL Soft EAL loopback is Windows-only in this patch".into())
    }
    pub fn set_preset(&mut self, _: &EalPreset) -> Result<(), String> {
        Ok(())
    }
    pub fn render(&mut self, _: &[f32; 256], output: &mut [f32; 512]) -> Result<(), String> {
        output.fill(0.0);
        Ok(())
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::{
        ffi::{c_char, c_void},
        mem,
        os::windows::ffi::OsStrExt,
        path::Path,
        ptr,
    };
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };

    type AlcDevice = *mut c_void;
    type AlcContext = *mut c_void;
    type FnOpen = unsafe extern "C" fn(*const c_char) -> AlcDevice;
    type FnCreate = unsafe extern "C" fn(AlcDevice, *const i32) -> AlcContext;
    type FnMake = unsafe extern "C" fn(AlcContext) -> i8;
    type FnDestroy = unsafe extern "C" fn(AlcContext);
    type FnClose = unsafe extern "C" fn(AlcDevice) -> i8;
    type FnProc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type FnAlcProc = unsafe extern "C" fn(AlcDevice, *const c_char) -> *mut c_void;
    type FnRender = unsafe extern "C" fn(AlcDevice, *mut c_void, i32);
    type FnGen = unsafe extern "C" fn(i32, *mut u32);
    type FnDel = unsafe extern "C" fn(i32, *const u32);
    type FnSourcei = unsafe extern "C" fn(u32, i32, i32);
    type FnSource3i = unsafe extern "C" fn(u32, i32, i32, i32, i32);
    type FnGetSourcei = unsafe extern "C" fn(u32, i32, *mut i32);
    type FnSource = unsafe extern "C" fn(u32);
    type FnQueue = unsafe extern "C" fn(u32, i32, *const u32);
    type FnUnqueue = unsafe extern "C" fn(u32, i32, *mut u32);
    type FnBufferData = unsafe extern "C" fn(u32, i32, *const c_void, i32, i32);
    type FnEffecti = unsafe extern "C" fn(u32, i32, i32);
    type FnEffectf = unsafe extern "C" fn(u32, i32, f32);
    type FnEffectfv = unsafe extern "C" fn(u32, i32, *const f32);
    type FnFilteri = unsafe extern "C" fn(u32, i32, i32);
    type FnFilterf = unsafe extern "C" fn(u32, i32, f32);
    type FnSloti = unsafe extern "C" fn(u32, i32, i32);
    type FnError = unsafe extern "C" fn() -> i32;
    type FnAlcError = unsafe extern "C" fn(AlcDevice) -> i32;
    type FnFormat = unsafe extern "C" fn(AlcDevice, i32, i32, i32) -> i8;
    type FnAlcExtension = unsafe extern "C" fn(AlcDevice, *const c_char) -> i8;
    type FnAlExtension = unsafe extern "C" fn(*const c_char) -> i8;

    // All constants from OpenAL Soft's public AL/efx.h and AL/alext.h.
    const ALC_FREQUENCY: i32 = 0x1007;
    const ALC_FORMAT_CHANNELS_SOFT: i32 = 0x1990;
    const ALC_FORMAT_TYPE_SOFT: i32 = 0x1991;
    const ALC_STEREO_SOFT: i32 = 0x1501;
    const ALC_FLOAT_SOFT: i32 = 0x1406;
    const AL_FORMAT_MONO_FLOAT32: i32 = 0x10010;
    const AL_SOURCE_STATE: i32 = 0x1010;
    const AL_PLAYING: i32 = 0x1012;
    const AL_BUFFERS_PROCESSED: i32 = 0x1016;
    const AL_DIRECT_FILTER: i32 = 0x20005;
    const AL_AUXILIARY_SEND_FILTER: i32 = 0x20006;
    const AL_EFFECT_TYPE: i32 = 0x8001;
    const AL_EFFECT_EAXREVERB: i32 = 0x8000;
    const AL_EFFECTSLOT_EFFECT: i32 = 0x0001;
    const AL_FILTER_TYPE: i32 = 0x8001;
    const AL_FILTER_LOWPASS: i32 = 0x0001;
    const AL_LOWPASS_GAIN: i32 = 0x0001;
    const AL_LOWPASS_GAINHF: i32 = 0x0002;

    unsafe fn symbol<T: Copy>(module: isize, key: &'static [u8]) -> Result<T, String> {
        // SAFETY: function address is copied into an ABI-matched function pointer.
        let func = unsafe { GetProcAddress(module as _, key.as_ptr()) }
            .ok_or_else(|| format!("OpenAL Soft missing {}", String::from_utf8_lossy(key)))?;
        Ok(unsafe { mem::transmute_copy(&func) })
    }
    unsafe fn extension<T: Copy>(getter: FnProc, key: &'static [u8]) -> Result<T, String> {
        let func = unsafe { getter(key.as_ptr().cast()) };
        if func.is_null() {
            return Err(format!(
                "OpenAL Soft missing {}",
                String::from_utf8_lossy(key)
            ));
        }
        Ok(unsafe { mem::transmute_copy(&func) })
    }

    // Created, rendered, and destroyed by one effects worker. Native pointers
    // deliberately make this object !Send. Every AL call uses a thread-local
    // context, so overlapping old/new workers cannot change each other's context.
    pub struct WetReverb {
        module: isize,
        device: AlcDevice,
        context: AlcContext,
        make: FnMake,
        destroy: FnDestroy,
        close: FnClose,
        render: FnRender,
        source: u32,
        effect: u32,
        slot: u32,
        filter: u32,
        buffers: [u32; 3],
        queued: usize,
        next_buffer: usize,
        source_play: FnSource,
        source_stop: FnSource,
        source_get: FnGetSourcei,
        queue: FnQueue,
        unqueue: FnUnqueue,
        buffer_data: FnBufferData,
        effect_f: FnEffectf,
        effect_i: FnEffecti,
        effect_fv: FnEffectfv,
        slot_i: FnSloti,
        del_sources: FnDel,
        del_buffers: FnDel,
        del_effects: FnDel,
        del_slots: FnDel,
        del_filters: FnDel,
        rate: i32,
        error: FnError,
        alc_error: FnAlcError,
    }

    impl WetReverb {
        pub fn new(rate: u32) -> Result<Self, String> {
            #[cfg(test)]
            if let Some(path) = std::env::var_os("EAL_TEST_OPENAL_DLL") {
                return Self::new_with_path(rate, Path::new(&path));
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let path = exe
                .parent()
                .ok_or("missing executable directory")?
                .join("OpenALSoft.x86_64.dll");
            Self::new_with_path(rate, &path)
        }
        pub(crate) fn new_with_path(rate: u32, path: &Path) -> Result<Self, String> {
            if rate == 0 || rate > 192_000 {
                return Err("invalid EAL audio sample rate".into());
            }
            if !path.is_absolute() {
                return Err("OpenAL DLL path must be absolute".into());
            }
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                let module = LoadLibraryExW(
                    path.as_ptr(),
                    ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                );
                if module.is_null() {
                    return Err(
                        "OpenALSoft.x86_64.dll not available; run build.ps1 -Fast or install the packaged OpenAL Soft runtime beside DinurdoJK.exe"
                            .into(),
                    );
                }
                let result = Self::init(module as isize, rate as i32);
                if result.is_err() {
                    FreeLibrary(module);
                }
                result
            }
        }
        unsafe fn init(module: isize, rate: i32) -> Result<Self, String> {
            unsafe {
                let alc_proc: FnAlcProc = symbol(module, b"alcGetProcAddress\0")?;
                let open: FnOpen = {
                    let raw = alc_proc(
                        ptr::null_mut(),
                        b"alcLoopbackOpenDeviceSOFT\0".as_ptr().cast(),
                    );
                    if raw.is_null() {
                        return Err("OpenAL Soft loopback extension unavailable".into());
                    }
                    mem::transmute_copy(&raw)
                };
                let create: FnCreate = symbol(module, b"alcCreateContext\0")?;
                let destroy: FnDestroy = symbol(module, b"alcDestroyContext\0")?;
                let close: FnClose = symbol(module, b"alcCloseDevice\0")?;
                let render: FnRender = {
                    let raw = alc_proc(ptr::null_mut(), b"alcRenderSamplesSOFT\0".as_ptr().cast());
                    if raw.is_null() {
                        return Err("OpenAL Soft loopback rendering unavailable".into());
                    }
                    mem::transmute_copy(&raw)
                };
                let dev = open(ptr::null());
                if dev.is_null() {
                    return Err("OpenAL Soft loopback device unavailable".into());
                }
                let capabilities = (|| -> Result<(FnMake, FnAlcError), String> {
                    let has: FnAlcExtension = symbol(module, b"alcIsExtensionPresent\0")?;
                    for name in [c"ALC_EXT_EFX", c"ALC_EXT_thread_local_context"] {
                        if has(dev, name.as_ptr()) == 0 {
                            return Err(format!("OpenAL missing {}", name.to_string_lossy()));
                        }
                    }
                    let raw = alc_proc(dev, c"alcSetThreadContext".as_ptr());
                    if raw.is_null() {
                        return Err("OpenAL missing thread-local context binding".into());
                    }
                    let make: FnMake = mem::transmute_copy(&raw);
                    let raw = alc_proc(dev, c"alcIsRenderFormatSupportedSOFT".as_ptr());
                    if raw.is_null() {
                        return Err("OpenAL missing loopback format query".into());
                    }
                    let format: FnFormat = mem::transmute_copy(&raw);
                    if format(dev, rate, ALC_STEREO_SOFT, ALC_FLOAT_SOFT) == 0 {
                        return Err(
                            "OpenAL does not support requested stereo float loopback rate".into(),
                        );
                    }
                    Ok((make, symbol(module, b"alcGetError\0")?))
                })();
                let (make, alc_error) = match capabilities {
                    Ok(v) => v,
                    Err(e) => {
                        close(dev);
                        return Err(e);
                    }
                };
                let attrs = [
                    ALC_FORMAT_CHANNELS_SOFT,
                    ALC_STEREO_SOFT,
                    ALC_FORMAT_TYPE_SOFT,
                    ALC_FLOAT_SOFT,
                    ALC_FREQUENCY,
                    rate,
                    0,
                ];
                let ctx = create(dev, attrs.as_ptr());
                if ctx.is_null() {
                    close(dev);
                    return Err("OpenAL Soft loopback context unavailable".into());
                }
                if make(ctx) == 0 {
                    destroy(ctx);
                    close(dev);
                    return Err("OpenAL Soft cannot activate loopback context".into());
                }
                let result = (|| {
                    let error: FnError = symbol(module, b"alGetError\0")?;
                    let has: FnAlExtension = symbol(module, b"alIsExtensionPresent\0")?;
                    if has(c"AL_EXT_FLOAT32".as_ptr()) == 0 {
                        return Err("OpenAL missing float PCM support".into());
                    }
                    if has(c"EAX4.0".as_ptr()) == 0 {
                        return Err("OpenAL runtime lacks EAX 4.0; rebuild the packaged EAX-enabled OpenAL Soft runtime".into());
                    }
                    let get: FnProc = symbol(module, b"alGetProcAddress\0")?;
                    for name in [c"EAXSet", c"EAXGet"] {
                        if get(name.as_ptr()).is_null() {
                            return Err(format!("OpenAL missing {}", name.to_string_lossy()));
                        }
                    }
                    let gen_sources: FnGen = symbol(module, b"alGenSources\0")?;
                    let del_sources: FnDel = symbol(module, b"alDeleteSources\0")?;
                    let gen_buffers: FnGen = symbol(module, b"alGenBuffers\0")?;
                    let del_buffers: FnDel = symbol(module, b"alDeleteBuffers\0")?;
                    let source_i: FnSourcei = symbol(module, b"alSourcei\0")?;
                    let source_3i: FnSource3i = symbol(module, b"alSource3i\0")?;
                    let source_get: FnGetSourcei = symbol(module, b"alGetSourcei\0")?;
                    let source_play: FnSource = symbol(module, b"alSourcePlay\0")?;
                    let source_stop: FnSource = symbol(module, b"alSourceStop\0")?;
                    let queue: FnQueue = symbol(module, b"alSourceQueueBuffers\0")?;
                    let unqueue: FnUnqueue = symbol(module, b"alSourceUnqueueBuffers\0")?;
                    let buffer_data: FnBufferData = symbol(module, b"alBufferData\0")?;
                    let gen_effects: FnGen = extension(get, b"alGenEffects\0")?;
                    let del_effects: FnDel = extension(get, b"alDeleteEffects\0")?;
                    let effect_i: FnEffecti = extension(get, b"alEffecti\0")?;
                    let effect_f: FnEffectf = extension(get, b"alEffectf\0")?;
                    let effect_fv: FnEffectfv = extension(get, b"alEffectfv\0")?;
                    let gen_slots: FnGen = extension(get, b"alGenAuxiliaryEffectSlots\0")?;
                    let del_slots: FnDel = extension(get, b"alDeleteAuxiliaryEffectSlots\0")?;
                    let slot_i: FnSloti = extension(get, b"alAuxiliaryEffectSloti\0")?;
                    let gen_filters: FnGen = extension(get, b"alGenFilters\0")?;
                    let del_filters: FnDel = extension(get, b"alDeleteFilters\0")?;
                    let filter_i: FnFilteri = extension(get, b"alFilteri\0")?;
                    let filter_f: FnFilterf = extension(get, b"alFilterf\0")?;
                    let mut source = 0;
                    let mut effect = 0;
                    let mut slot = 0;
                    let mut filter = 0;
                    let mut buffers = [0u32; 3];
                    gen_sources(1, &mut source);
                    gen_buffers(3, buffers.as_mut_ptr());
                    gen_effects(1, &mut effect);
                    gen_slots(1, &mut slot);
                    gen_filters(1, &mut filter);
                    if [
                        source, effect, slot, filter, buffers[0], buffers[1], buffers[2],
                    ]
                    .contains(&0)
                    {
                        return Err("OpenAL Soft failed to allocate EAX effects".into());
                    }
                    effect_i(effect, AL_EFFECT_TYPE, AL_EFFECT_EAXREVERB);
                    slot_i(slot, AL_EFFECTSLOT_EFFECT, effect as i32);
                    filter_i(filter, AL_FILTER_TYPE, AL_FILTER_LOWPASS);
                    filter_f(filter, AL_LOWPASS_GAIN, 0.0);
                    filter_f(filter, AL_LOWPASS_GAINHF, 0.0);
                    source_i(source, AL_DIRECT_FILTER, filter as i32);
                    source_3i(source, AL_AUXILIARY_SEND_FILTER, slot as i32, 0, 0);
                    let code = error();
                    if code != 0 {
                        return Err(format!(
                            "OpenAL EAX effects initialization failed: AL {code:#x}"
                        ));
                    }
                    Ok(Self {
                        module,
                        device: dev,
                        context: ctx,
                        make,
                        destroy,
                        close,
                        render,
                        source,
                        effect,
                        slot,
                        filter,
                        buffers,
                        queued: 0,
                        next_buffer: 0,
                        source_play,
                        source_stop,
                        source_get,
                        queue,
                        unqueue,
                        buffer_data,
                        effect_f,
                        effect_i,
                        effect_fv,
                        slot_i,
                        del_sources,
                        del_buffers,
                        del_effects,
                        del_slots,
                        del_filters,
                        rate,
                        error,
                        alc_error,
                    })
                })();
                make(ptr::null_mut());
                if result.is_err() {
                    destroy(ctx);
                    close(dev);
                }
                result
            }
        }
        pub fn set_preset(&mut self, preset: &EalPreset) -> Result<(), String> {
            if !matches!(preset.eax_words.len(), 14 | 28) {
                return Err("invalid EAX preset size".into());
            }
            unsafe {
                if (self.make)(self.context) == 0 {
                    return Err("OpenAL context activation failed".into());
                }
                // This engine is inactive during room changes. Recreate effect
                // state so the next room cannot inherit stale tails or EAX3 fields.
                (self.slot_i)(self.slot, AL_EFFECTSLOT_EFFECT, 0);
                (self.effect_i)(self.effect, AL_EFFECT_TYPE, 0);
                (self.effect_i)(self.effect, AL_EFFECT_TYPE, AL_EFFECT_EAXREVERB);
                let w = &preset.eax_words;
                let mb = |i: usize| w.get(i).copied().unwrap_or(0) as i32;
                let f = |i: usize| f32::from_bits(w.get(i).copied().unwrap_or(0));
                let gain = |v: i32| millibels_to_gain(v as f32);
                let set = |id: i32, val: f32| {
                    if val.is_finite() {
                        (self.effect_f)(self.effect, id, val);
                    }
                };
                let cl = |value: f32, lo: f32, hi: f32| {
                    if value.is_finite() {
                        value.clamp(lo, hi)
                    } else {
                        lo
                    }
                };
                if w.len() == 28 {
                    // EAX 3 layout from EalMan_DataTypes.h / EAXListenerProperties.
                    set(1, super::eax_environment_density(f(1)));
                    set(2, cl(f(2), 0.0, 1.0));
                    set(3, cl(gain(mb(3)), 0.0, 1.0));
                    set(4, cl(gain(mb(4)), 0.0, 1.0));
                    set(5, cl(gain(mb(5)), 0.0, 1.0));
                    set(6, cl(f(6), 0.1, 20.0));
                    set(7, cl(f(7), 0.1, 2.0));
                    set(8, cl(f(8), 0.1, 2.0));
                    set(9, cl(gain(mb(9)), 0.0, 3.16));
                    set(10, cl(f(10), 0.0, 0.3));
                    (self.effect_fv)(self.effect, 11, [f(11), f(12), f(13)].as_ptr());
                    set(12, cl(gain(mb(14)), 0.0, 10.0));
                    set(13, cl(f(15), 0.0, 0.1));
                    (self.effect_fv)(self.effect, 14, [f(16), f(17), f(18)].as_ptr());
                    set(15, cl(f(19), 0.075, 0.25));
                    set(16, cl(f(20), 0.0, 1.0));
                    set(17, cl(f(21), 0.04, 4.0));
                    set(18, cl(f(22), 0.0, 1.0));
                    set(19, cl(millibels_to_gain(f(23)), 0.892, 1.0));
                    set(20, cl(f(24), 1000.0, 20000.0));
                    set(21, cl(f(25), 20.0, 1000.0));
                    set(22, cl(f(26), 0.0, 10.0));
                    (self.effect_i)(self.effect, 23, if mb(27) & 0x20 != 0 { 1 } else { 0 });
                } else if w.len() == 14 {
                    // EAX 2 layout (no room LF, echo or modulation properties).
                    set(1, super::eax_environment_density(f(10)));
                    set(2, cl(f(11), 0.0, 1.0));
                    set(3, cl(gain(mb(0)), 0.0, 1.0));
                    set(4, cl(gain(mb(1)), 0.0, 1.0));
                    set(6, cl(f(3), 0.1, 20.0));
                    set(7, cl(f(4), 0.1, 2.0));
                    set(9, cl(gain(mb(5)), 0.0, 3.16));
                    set(10, cl(f(6), 0.0, 0.3));
                    set(12, cl(gain(mb(7)), 0.0, 10.0));
                    set(13, cl(f(8), 0.0, 0.1));
                    set(19, cl(millibels_to_gain(f(12)), 0.892, 1.0));
                    set(22, cl(f(2), 0.0, 10.0));
                    (self.effect_i)(self.effect, 23, if mb(13) & 0x20 != 0 { 1 } else { 0 });
                }
                (self.slot_i)(self.slot, AL_EFFECTSLOT_EFFECT, self.effect as i32);
                let code = (self.error)();
                (self.make)(ptr::null_mut());
                if code != 0 {
                    return Err(format!("OpenAL EAX preset failed: AL {code:#x}"));
                }
                Ok(())
            }
        }
        pub fn render(&mut self, mono: &[f32; 256], out: &mut [f32; 512]) -> Result<(), String> {
            unsafe {
                if (self.make)(self.context) == 0 {
                    out.fill(0.0);
                    return Err("OpenAL context activation failed".into());
                }
                // Streaming queue; each used buffer is returned before refilling.
                let mut processed = 0;
                (self.source_get)(self.source, AL_BUFFERS_PROCESSED, &mut processed);
                while processed > 0 && self.queued > 0 {
                    let mut id = 0;
                    (self.unqueue)(self.source, 1, &mut id);
                    self.queued -= 1;
                    processed -= 1;
                }
                let id = self.buffers[self.next_buffer];
                // Loopback renders *exactly* 256 frames. At most 3 outstanding buffers.
                if self.queued < 3 {
                    (self.buffer_data)(
                        id,
                        AL_FORMAT_MONO_FLOAT32,
                        mono.as_ptr().cast(),
                        (mono.len() * mem::size_of::<f32>()) as i32,
                        self.rate,
                    );
                    (self.queue)(self.source, 1, &id);
                    self.queued += 1;
                    self.next_buffer = (self.next_buffer + 1) % self.buffers.len();
                }
                let mut state = 0;
                (self.source_get)(self.source, AL_SOURCE_STATE, &mut state);
                if state != AL_PLAYING {
                    (self.source_play)(self.source);
                }
                (self.render)(self.device, out.as_mut_ptr().cast(), 256);
                let al = (self.error)();
                let alc = (self.alc_error)(self.device);
                (self.make)(ptr::null_mut());
                if al != 0 || alc != 0 {
                    out.fill(0.0);
                    return Err(format!("OpenAL loopback failed: AL {al:#x}, ALC {alc:#x}"));
                }
                if out.iter().any(|v| !v.is_finite()) {
                    out.fill(0.0);
                    return Err("OpenAL produced non-finite audio".into());
                }
                Ok(())
            }
        }
    }
    impl Drop for WetReverb {
        fn drop(&mut self) {
            unsafe {
                if (self.make)(self.context) != 0 {
                    (self.source_stop)(self.source);
                    (self.del_sources)(1, &self.source);
                    (self.del_buffers)(3, self.buffers.as_ptr());
                    (self.slot_i)(self.slot, AL_EFFECTSLOT_EFFECT, 0);
                    (self.del_slots)(1, &self.slot);
                    (self.del_effects)(1, &self.effect);
                    (self.del_filters)(1, &self.filter);
                    (self.make)(ptr::null_mut());
                }
                (self.destroy)(self.context);
                (self.close)(self.device);
                FreeLibrary(self.module as _);
            }
        }
    }
}
#[cfg(windows)]
pub use win::WetReverb;

fn millibels_to_gain(value: f32) -> f32 {
    if value.is_finite() {
        10.0f32.powf(value.clamp(-10000.0, 2000.0) / 2000.0)
    } else {
        1.0
    }
}

fn eax_environment_density(size: f32) -> f32 {
    // OpenAL Soft's EAX-to-EFX conversion (al/effects/reverb.cpp).
    if size.is_finite() { (size.powi(3) / 16.0).clamp(0.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_authored_rooms_keep_openal_eax_density() {
        assert!((eax_environment_density(1.4) - 0.1715).abs() < 0.000001);
        assert!((eax_environment_density(1.8) - 0.3645).abs() < 0.000001);
        assert_eq!(eax_environment_density(7.5), 1.0);
        assert_eq!(eax_environment_density(f32::NAN), 0.0);
    }
    #[test]
    fn floating_point_air_absorption_and_positive_reverb_levels() {
        assert!((millibels_to_gain(-5.0) - 0.9942601).abs() < 0.000001);
        assert!((millibels_to_gain(2000.0) - 10.0).abs() < 0.000001);
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires EAL_TEST_OPENAL_DLL pointing to a compatible OpenAL Soft DLL"]
    fn native_loopback_is_wet_only_and_keeps_a_tail() {
        let path = std::path::PathBuf::from(
            std::env::var_os("EAL_TEST_OPENAL_DLL").expect("EAL_TEST_OPENAL_DLL"),
        );
        let mut words = vec![0u32; 28];
        for (i, value) in [
            (1, 7.5f32),
            (2, 1.0),
            (6, 0.5),
            (7, 0.8),
            (8, 1.0),
            (10, 0.02),
            (15, 0.03),
            (19, 0.25),
            (21, 0.25),
            (23, -5.0),
            (24, 5000.0),
            (25, 250.0),
        ] {
            words[i] = value.to_bits();
        }
        words[3] = (-1000i32) as u32;
        words[27] = 0x20;
        let preset = EalPreset {
            name: "Loopback test".into(),
            eax_words: words,
        };
        // Two independently owned threads exercise context isolation as well.
        let mut threads = Vec::new();
        for rate in [44100u32, 48000] {
            let path = path.clone();
            let preset = preset.clone();
            threads.push(std::thread::spawn(move || {
                let mut engine = WetReverb::new_with_path(rate, &path).unwrap();
                engine.set_preset(&preset).unwrap();
                let mut input = [0.0; 256];
                input[0] = 0.5;
                let mut output = [0.0; 512];
                // Let OpenAL's parameter interpolation settle before checking
                // the authored reflection delay against the impulse onset.
                for _ in 0..40 {
                    engine.render(&[0.0; 256], &mut output).unwrap();
                }
                engine.render(&input, &mut output).unwrap();
                let initial_peak = output.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
                println!(
                    "OpenAL initial wet block {rate} Hz: peak {initial_peak}, first samples {:?}",
                    &output[..8]
                );
                assert!(
                    initial_peak < 0.000001,
                    "direct signal leaked into wet output: peak {initial_peak}"
                );
                let mut energy = 0.0f64;
                let mut peak_us = 0;
                for _ in 0..200 {
                    let started = std::time::Instant::now();
                    engine.render(&[0.0; 256], &mut output).unwrap();
                    peak_us = peak_us.max(started.elapsed().as_micros());
                    energy += output.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
                }
                assert!(energy > 0.000001, "impulse produced no reverb tail");
                println!(
                    "OpenAL loopback {rate} Hz: tail energy {energy:.6}, max block {peak_us} us"
                );
                // A different format must reset EAX3-only properties cleanly.
                let mut words = vec![0u32; 14];
                for (i, value) in [
                    (3, 0.5f32),
                    (4, 0.8),
                    (6, 0.01),
                    (8, 0.02),
                    (10, 7.5),
                    (11, 1.0),
                    (12, -5.0),
                ] {
                    words[i] = value.to_bits();
                }
                engine
                    .set_preset(&EalPreset {
                        name: "EAX2".into(),
                        eax_words: words,
                    })
                    .unwrap();
                engine.render(&input, &mut output).unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
