//! Background workers for JKA videoMap: native Media Foundation H.264
//! promotion and the original RoQ decoder. The render thread only polls frames.
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, TryRecvError, TrySendError}, Arc,
    },
    thread, time::{Duration, Instant},
};
use crate::media_mp4::Mp4Info;

pub(in crate::renderer) enum VideoDecoder {
    Failed,
    Roq(RoqWorker),
    Mp4 {
        video: Mp4Video,
        fallback_roq: Option<Arc<[u8]>>,
    },
}

pub(in crate::renderer) struct WorldVideoPlayer {
    pub(in crate::renderer) texture: usize,
    pub(in crate::renderer) decoder: VideoDecoder,
}

/// RoQ decoding happens on a dedicated, low-frequency worker, independent
/// of the renderer's current FPS. Delta frames always decode in sequence.
pub(in crate::renderer) struct RoqWorker {
    width: u32,
    height: u32,
    rx: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
}

impl RoqWorker {
    pub(in crate::renderer) fn open(bytes: Arc<[u8]>) -> Result<Self, String> {
        let mut decoder = jka_assets::roq::RoqVideo::open(bytes)?;
        let (width, height) = (decoder.width(), decoder.height());
        let period = Duration::from_secs_f64(1.0 / f64::from(decoder.fps().max(1)));
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        thread::Builder::new().name("jka-roq-decode".into()).spawn(move || {
            let mut next = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                if decoder.next_frame().is_none() {
                    decoder.rewind();
                    if decoder.next_frame().is_none() { break; }
                }
                let frame = decoder.current_frame().to_vec();
                match tx.try_send(frame) {
                    Ok(()) | Err(TrySendError::Full(_)) => (),
                    Err(TrySendError::Disconnected(_)) => break,
                }
                next += period;
                let now = Instant::now();
                if let Some(wait) = next.checked_duration_since(now) {
                    thread::sleep(wait);
                } else if now.duration_since(next) > period {
                    next = now;
                }
            }
        }).map_err(|e| format!("could not start RoQ worker: {e}"))?;
        Ok(Self { width, height, rx, stop })
    }

    pub(in crate::renderer) fn size(&self) -> (u32, u32) { (self.width, self.height) }

    pub(in crate::renderer) fn poll(&self) -> Option<Vec<u8>> {
        self.rx.try_recv().ok()
    }
}

impl Drop for RoqWorker {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

/// Windows Media Foundation demuxes MP4 and decodes H.264 on this worker.
/// It tries a DXGI-backed D3D11 hardware decoder first, then software.
/// Frames are converted to RGBA for WGPU on the worker.
pub(in crate::renderer) struct Mp4Video {
    rx: Receiver<Result<Vec<u8>, String>>,
    stop: Arc<AtomicBool>,
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl Mp4Video {
    pub(in crate::renderer) fn open(data: Arc<[u8]>, info: Mp4Info) -> Result<Self, String> {
        #[cfg(not(windows))]
        { let _ = (data, info); return Err("native H.264 promotion currently requires Windows".into()); }
        #[cfg(windows)]
        {
            let (tx, rx) = mpsc::sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            thread::Builder::new().name("jka-h264-decode".into()).spawn(move || {
                if let Err(error) = play_mp4_on_worker(&data, info, &flag, &tx) {
                    // Avoid blocking here if a stale frame occupies the mailbox;
                    // disconnection will trigger the same RoQ fallback next poll.
                    let _ = tx.try_send(Err(error));
                }
            }).map_err(|e| format!("could not start H.264 worker: {e}"))?;
            Ok(Self { rx, stop })
        }
    }

    /// Never waits for decoding; one completed video frame at most per tick.
    pub(in crate::renderer) fn poll(&self) -> Result<Option<Vec<u8>>, String> {
        match self.rx.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("H.264 decoder stopped".into()),
        }
    }
}

impl Drop for Mp4Video {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

#[cfg(windows)]
fn play_mp4_on_worker(
    data: &[u8], info: Mp4Info, stop: &AtomicBool,
    tx: &mpsc::SyncSender<Result<Vec<u8>, String>>,
) -> Result<(), String> {
    use std::{fs, io::Write, os::windows::ffi::OsStrExt, path::PathBuf};
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
                ID3D11Device, ID3D11DeviceContext,
            },
        },
        Media::MediaFoundation::{
            MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateMediaType,
            MFCreateSourceReaderFromURL, MFShutdown, MFStartup, IMFAttributes,
            IMFSourceReader, MFMediaType_Video, MFVideoFormat_RGB32,
            MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
            MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_D3D_MANAGER,
            MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
            MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
            MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
        },
        System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
    };
    use windows::{core::PCWSTR, Win32::Media::MediaFoundation::MF_VERSION};

    // The game asset VFS provides compressed PK3 bytes. Let Windows do MP4
    // demuxing instead of taking on an extra native demuxer crate. SourceReader
    // accepts local URLs; these securely named scratch files are removed on exit.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
    }
    let dir = std::env::temp_dir().join("DinurdoJK-video");
    fs::create_dir_all(&dir).map_err(|e| format!("MP4 temp directory: {e}"))?;
    let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| now.as_nanos()).unwrap_or(0);
    let path = dir.join(format!("{}-{serial}-{nonce}.mp4", std::process::id()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true)
        .open(&path).map_err(|e| format!("MP4 temp create: {e}"))?;
    let _cleanup = Scratch(path.clone());
    file.write_all(data).map_err(|e| format!("MP4 temp write: {e}"))?;
    drop(file);
    if stop.load(Ordering::Relaxed) { return Ok(()); }

    // All Windows COM objects are created, used and released on this thread.
    // MF startup/shutdown are balanced even when playback fails midway.
    struct ComGuard;
    impl Drop for ComGuard { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    struct MfGuard;
    impl Drop for MfGuard { fn drop(&mut self) { unsafe { let _ = MFShutdown(); } } }
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()
            .map_err(|e| format!("COM init: {e}"))?;
    }
    let _com = ComGuard;
    unsafe { MFStartup(MF_VERSION, 0).map_err(|e| format!("Media Foundation init: {e}"))?; }
    let _mf = MfGuard;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // Requires OsStrExt only on Windows.
    fn make_hardware_attrs() -> Result<(IMFAttributes, ID3D11Device, ID3D11DeviceContext), String> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None, D3D11_SDK_VERSION,
                Some(&mut device), None, Some(&mut context),
            ).map_err(|e| format!("D3D11 device: {e}"))?;
            let device = device.ok_or("D3D11 returned no device")?;
            let context = context.ok_or("D3D11 returned no context")?;
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)
                .map_err(|e| format!("DXGI manager: {e}"))?;
            let manager = manager.ok_or("DXGI returned no manager")?;
            manager.ResetDevice(&device, token)
                .map_err(|e| format!("DXGI attach device: {e}"))?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 5).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no reader attributes")?;
            attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager)
                .map_err(|e| format!("hardware reader device: {e}"))?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| e.to_string())?;
            // This inserts a compatible colorspace converter when needed.
            // Do NOT use the slower ENABLE_VIDEO_PROCESSING thumbnail converter
            // for the hardware-backed path.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(|e| e.to_string())?;
            Ok((attrs, device, context))
        }
    }
    fn software_attrs() -> Result<IMFAttributes, String> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no software reader attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(|e| e.to_string())?;
            Ok(attrs)
        }
    }
    fn configure_reader(path: &[u16], attrs: &IMFAttributes) -> Result<IMFSourceReader, String> {
        unsafe {
            let reader = MFCreateSourceReaderFromURL(PCWSTR(path.as_ptr()), attrs)
                .map_err(|e| format!("open MP4: {e}"))?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| e.to_string())?;
            reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                .map_err(|e| format!("select MP4 video: {e}"))?;
            let ty = MFCreateMediaType().map_err(|e| e.to_string())?;
            ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| e.to_string())?;
            reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &ty)
                .map_err(|e| format!("H.264 RGB32 decode output: {e}"))?;
            Ok(reader)
        }
    }

    // Keep D3D device/context alive for the entire Reader lifetime; they must
    // outlive the DXGI manager installed into the reader's decoder chain.
    let hardware = make_hardware_attrs();
    let (attrs, _device, _context, label) = match hardware {
        Ok((attrs, device, context)) => (attrs, Some(device), Some(context), "DXVA candidate"),
        Err(e) => {
            eprintln!("[JKA] MP4 hardware init unavailable ({e}); trying software");
            (software_attrs()?, None, None, "software")
        }
    };
    let mut use_hardware_attrs = label == "DXVA candidate";
    let mut reader = match configure_reader(&wide, &attrs) {
        Ok(reader) => reader,
        Err(e) if use_hardware_attrs => {
            eprintln!("[JKA] MP4 hardware reader rejected ({e}); trying software");
            use_hardware_attrs = false;
            configure_reader(&wide, &software_attrs()?)?
        }
        Err(e) => return Err(e),
    };
    eprintln!("[JKA] MP4 video: {} reader active (hardware is not guaranteed without transform inspection)",
        if use_hardware_attrs { "DXVA-enabled" } else { "software" });

    let mut base_pts = None::<i64>;
    let mut started = Instant::now();
    let mut successive_empty_loops = 0u8;
    let mut frames_this_loop = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let mut flags = 0u32;
        let mut pts = 0i64;
        let mut sample = None;
        unsafe {
            reader.ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None,
                Some(&mut flags), Some(&mut pts), Some(&mut sample))
                .map_err(|e| format!("H.264 ReadSample: {e}"))?;
        }
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            // A malformed/empty stream must not spin reopening forever.
            if frames_this_loop == 0 { return Err("MP4 reached EOF without decoded frames".into()); }
            // Reopening avoids keeping a process-global, seekable PROPVARIANT.
            if use_hardware_attrs {
                reader = configure_reader(&wide, &attrs)?;
            } else {
                let fallback_attrs = software_attrs()?;
                reader = configure_reader(&wide, &fallback_attrs)?;
            }
            base_pts = None;
            started = Instant::now();
            successive_empty_loops = 0;
            frames_this_loop = 0;
            continue;
        }
        if flags & (MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
            return Err("MP4 changed video format during playback".into());
        }
        let Some(sample) = sample else {
            successive_empty_loops = successive_empty_loops.saturating_add(1);
            if successive_empty_loops > 30 { return Err("MP4 provided no decoded video frames".into()); }
            continue;
        };
        successive_empty_loops = 0;
        frames_this_loop += 1;
        let media_type = unsafe { reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) }
            .map_err(|e| e.to_string())?;
        let frame_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }
            .map_err(|e| format!("MP4 frame size: {e}"))?;
        let width = (frame_size >> 32) as u32;
        let height = frame_size as u32;
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(format!("MP4 invalid decoded frame dimensions {width}x{height}"));
        }
        let stride = unsafe { media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|v| v as i32).unwrap_or((width * 4) as i32);
        let row_bytes = width as usize * 4;
        let pitch = stride.unsigned_abs() as usize;
        if pitch < row_bytes { return Err("MP4 output stride too small".into()); }
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|e| format!("MP4 frame buffer: {e}"))?;
        let mut ptr = std::ptr::null_mut();
        let mut length = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut length)) }
            .map_err(|e| format!("MP4 frame lock: {e}"))?;
        // Every Lock must be balanced even when pixel validation fails.
        let converted = (|| -> Result<Vec<u8>, String> {
            let available = length as usize;
            let needed = pitch.checked_mul(height as usize - 1)
                .and_then(|a| a.checked_add(row_bytes)).ok_or("MP4 stride overflow")?;
            if ptr.is_null() || available < needed { return Err("MP4 frame buffer truncated".into()); }
            let src = unsafe { std::slice::from_raw_parts(ptr, available) };
            let mut rgba = vec![0u8; row_bytes * height as usize];
            for y in 0..height as usize {
                let source_y = if stride < 0 { height as usize - 1 - y } else { y };
                let row = &src[source_y * pitch..source_y * pitch + row_bytes];
                for (x, bgra) in row.chunks_exact(4).enumerate() {
                    let dst = (y * width as usize + x) * 4;
                    rgba[dst..dst+4].copy_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
                }
            }
            Ok(if (width, height) == (info.width, info.height) { rgba } else {
                crate::media_mp4::resize_rgba_nearest(&rgba, width, height, info.width, info.height)
            })
        })();
        unsafe { let _ = buffer.Unlock(); }
        let rgba = converted?;

        // Media Foundation timestamps are 100-nanosecond units, and unlike a
        // fixed FPS approximation they correctly pace variable-rate assets.
        let first = *base_pts.get_or_insert(pts);
        let elapsed = ((pts - first).max(0) as u64).min(60 * 60 * 10_000_000);
        let target = Duration::from_nanos(elapsed * 100);
        while !stop.load(Ordering::Relaxed) {
            let remaining = target.saturating_sub(started.elapsed());
            if remaining.is_zero() { break; }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        if stop.load(Ordering::Relaxed) { break; }
        match tx.try_send(Ok(rgba)) {
            Ok(()) | Err(TrySendError::Full(_)) => (),
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
    Ok(())
}
