//! Capture.
use crate::renderer::{
    mpsc, Duration, Instant, PathBuf, Renderer, ScreenshotFormat, ScreenshotOutput,
};
use image::ImageEncoder;
#[path = "capture_alpha.rs"]
pub(in crate::renderer) mod alpha;

pub(in crate::renderer) struct ScreenshotReadbackBuffer {
    pub(in crate::renderer) buffer: wgpu::Buffer,
    pub(in crate::renderer) size: u64,
}

pub(in crate::renderer) enum ScreenshotDestination {
    File {
        directory: PathBuf,
        metadata: crate::screenshot::ScreenshotMetadata,
        format: ScreenshotFormat,
    },
    Clipboard,
    FrontendScene { path: PathBuf, neutral_gamma: f32, captured_gamma: f32, is_neutral: bool, surface_is_srgb: bool },
    // None: HDR compositor already resolved straight sRGB. Some: resolve on
    // readback, with the bool indicating whether the source surface is sRGB.
    VideoFrame { frame_index: u64, depth: bool, alpha: bool, sdr_unpremultiply: Option<bool> },
}

pub(in crate::renderer) struct ScreenshotReadback {
    pub(in crate::renderer) destination: ScreenshotDestination,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) padded_bytes_per_row: u32,
    pub(in crate::renderer) buffer_size: u64,
    pub(in crate::renderer) format: wgpu::TextureFormat,
    pub(in crate::renderer) submission_index: Option<wgpu::SubmissionIndex>,
    pub(in crate::renderer) copy_record_ms: f64,
    pub(in crate::renderer) copy_submit_ms: f64,
    pub(in crate::renderer) inline_copy: bool,
    pub(in crate::renderer) reused_buffer: bool,
    pub(in crate::renderer) depth_requested: bool,
    pub(in crate::renderer) started: Instant,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) enum ScreenshotPixelFormat {
    Bgra,
    Rgba,
}

pub(in crate::renderer) struct ScreenshotEncodeJob {
    pub(in crate::renderer) pixels: Vec<u8>,
    pub(in crate::renderer) depth_pixels: Option<Vec<u8>>,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) padded_bytes_per_row: u32,
    pub(in crate::renderer) pixel_format: ScreenshotPixelFormat,
    pub(in crate::renderer) destination: ScreenshotDestination,
    pub(in crate::renderer) copy_record_ms: f64,
    pub(in crate::renderer) copy_submit_ms: f64,
    pub(in crate::renderer) map_request_ms: f64,
    pub(in crate::renderer) gpu_wait_ms: f64,
    pub(in crate::renderer) readback_copy_ms: f64,
    pub(in crate::renderer) inline_copy: bool,
    pub(in crate::renderer) reused_buffer: bool,
    pub(in crate::renderer) started: Instant,
}

#[cfg(windows)]
pub(in crate::renderer) fn screenshot_timestamp() -> String {
    #[repr(C)]
    struct WinSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut WinSystemTime);
    }

    let mut local = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    unsafe {
        GetLocalTime(local.as_mut_ptr());
        let local = local.assume_init();
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            local.year, local.month, local.day, local.hour, local.minute, local.second
        )
    }
}

#[cfg(not(windows))]
pub(in crate::renderer) fn screenshot_timestamp() -> String {
    // The client is Windows-first. Keep the same sortable filename shape on
    // other targets without adding a date/time dependency; this fallback is UTC.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0_i64, |duration| {
            duration.as_secs().min(i64::MAX as u64) as i64
        });
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}_{hour:02}-{minute:02}-{second:02}")
}

/// Compresses a padded GPU readback buffer to a quality-95 4:2:0 JPEG. The
/// second value names the input path taken, for the screenshot timing log.
pub(in crate::renderer) fn encode_screenshot_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    pixel_format: ScreenshotPixelFormat,
) -> Result<(Vec<u8>, &'static str), String> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = width * 4;
    let padded_row_bytes = padded_bytes_per_row as usize;
    let pixel_format = match pixel_format {
        ScreenshotPixelFormat::Bgra => libjpeg_turbo_rs::PixelFormat::Bgra,
        ScreenshotPixelFormat::Rgba => libjpeg_turbo_rs::PixelFormat::Rgba,
    };

    if row_bytes == padded_row_bytes {
        let byte_len = row_bytes
            .checked_mul(height)
            .ok_or_else(|| "screenshot byte size overflowed usize".to_owned())?;
        let jpeg = libjpeg_turbo_rs::compress(
            &pixels[..byte_len],
            width,
            height,
            pixel_format,
            95,
            libjpeg_turbo_rs::Subsampling::S420,
        )
        .map_err(|error| format!("JPEG encode failed: {error}"))?;
        Ok((jpeg, "direct"))
    } else {
        let mut encoder = libjpeg_turbo_rs::ScanlineEncoder::new(width, height, pixel_format);
        encoder.set_quality(95);
        encoder.set_subsampling(libjpeg_turbo_rs::Subsampling::S420);
        for row_index in 0..height {
            let row_start = row_index * padded_row_bytes;
            encoder
                .write_scanline(&pixels[row_start..row_start + row_bytes])
                .map_err(|error| format!("JPEG scanline encode failed: {error}"))?;
        }
        let jpeg = encoder
            .finish()
            .map_err(|error| format!("JPEG encode failed: {error}"))?;
        Ok((jpeg, "scanline"))
    }
}

/// Encodes the same post-processed WGPU surface used by `/screenshot` as a
/// lossless RGBA PNG. WGPU rows are padded to COPY_BYTES_PER_ROW_ALIGNMENT and
/// Windows' common surface format is BGRA, so normalize both here on the worker.
pub(in crate::renderer) fn encode_screenshot_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    pixel_format: ScreenshotPixelFormat,
) -> Result<Vec<u8>, String> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let row_bytes = width_usize
        .checked_mul(4)
        .ok_or_else(|| "screenshot row size overflowed usize".to_owned())?;
    let padded_row_bytes = padded_bytes_per_row as usize;
    let byte_len = row_bytes
        .checked_mul(height_usize)
        .ok_or_else(|| "screenshot byte size overflowed usize".to_owned())?;
    let mut rgba = Vec::with_capacity(byte_len);

    for row_index in 0..height_usize {
        let row_start = row_index
            .checked_mul(padded_row_bytes)
            .ok_or_else(|| "screenshot row offset overflowed usize".to_owned())?;
        let row_end = row_start
            .checked_add(row_bytes)
            .ok_or_else(|| "screenshot row end overflowed usize".to_owned())?;
        let row = pixels
            .get(row_start..row_end)
            .ok_or_else(|| "screenshot readback buffer is shorter than expected".to_owned())?;
        match pixel_format {
            ScreenshotPixelFormat::Rgba => rgba.extend_from_slice(row),
            ScreenshotPixelFormat::Bgra => {
                for pixel in row.chunks_exact(4) {
                    rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
        }
    }

    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &rgba,
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    Ok(png)
}

/// Writes the clipboard screenshot's JPEG to the temp directory so it can ride
/// on the clipboard as a file drop. Older copies are swept first; a pasted file
/// is read immediately by chat apps, so a day of grace is plenty.
pub(in crate::renderer) fn write_clipboard_jpeg(jpeg: &[u8]) -> Result<PathBuf, String> {
    const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
    let dir = std::env::temp_dir().join("jka-clipboard-screenshots");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = dir.join(format!("shot{}.jpg", screenshot_timestamp()));
    std::fs::write(&path, jpeg)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(path)
}

pub(in crate::renderer) fn encode_screenshot_job(
    job: ScreenshotEncodeJob,
) -> Result<ScreenshotOutput, String> {
    let padded_row_bytes = job.padded_bytes_per_row as usize;

    match job.destination {
        ScreenshotDestination::File {
            directory: screenshot_dir,
            metadata,
            format,
        } => {
            let encode_started = Instant::now();
            let (bytes, extension, codec_detail) = match format {
                ScreenshotFormat::Jpeg => {
                    let (mut jpeg, input_mode) = encode_screenshot_jpeg(
                        &job.pixels,
                        job.width,
                        job.height,
                        job.padded_bytes_per_row,
                        job.pixel_format,
                    )?;
                    crate::screenshot::embed_metadata(&mut jpeg, &metadata)?;
                    (jpeg, "jpg", input_mode)
                }
                ScreenshotFormat::Png => {
                    let mut png = encode_screenshot_png(
                        &job.pixels,
                        job.width,
                        job.height,
                        job.padded_bytes_per_row,
                        job.pixel_format,
                    )?;
                    crate::screenshot::embed_png_metadata(&mut png, &metadata)?;
                    (png, "png", "rgba8")
                }
            };
            let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;

            let file_io_started = Instant::now();
            std::fs::create_dir_all(&screenshot_dir).map_err(|error| {
                format!("could not create {}: {error}", screenshot_dir.display())
            })?;
            let path = screenshot_dir.join(format!(
                "shot{}.{}",
                screenshot_timestamp(),
                extension
            ));
            std::fs::write(&path, &bytes)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            let file_io_ms = file_io_started.elapsed().as_secs_f64() * 1000.0;

            let total_ms = job.started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[JKA SCREENSHOT] {}x{} copy_record={:.3}ms copy_submit={:.3}ms map_request={:.3}ms gpu_wait={:.3}ms readback_copy={:.3}ms encode={:.3}ms file_io={:.3}ms total={:.3}ms buffer={} copy={} codec={} codec_detail={} output=file encode=worker",
                job.width,
                job.height,
                job.copy_record_ms,
                job.copy_submit_ms,
                job.map_request_ms,
                job.gpu_wait_ms,
                job.readback_copy_ms,
                encode_ms,
                file_io_ms,
                total_ms,
                if job.reused_buffer { "reused" } else { "new" },
                if job.inline_copy { "inline" } else { "late-submit" },
                extension,
                codec_detail,
            );
            Ok(ScreenshotOutput::Saved(path))
        }
        ScreenshotDestination::FrontendScene { path, neutral_gamma, captured_gamma, is_neutral, surface_is_srgb } => {
            // Source is the post-processed scene BEFORE egui/HUD. Copy its
            // pixels into a lossless PNG, neutralizing only shader output gamma
            // (later display grades are only approximately reversible from 8-bit output).
            let mut rgba = Vec::with_capacity(job.width as usize * job.height as usize * 4);
            let row_bytes = job.width as usize * 4;
            let padded = job.padded_bytes_per_row as usize;
            for y in 0..job.height as usize {
                let row = &job.pixels[y * padded..y * padded + row_bytes];
                match job.pixel_format {
                    ScreenshotPixelFormat::Rgba => rgba.extend_from_slice(row),
                    ScreenshotPixelFormat::Bgra => for pixel in row.chunks_exact(4) {
                        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                    },
                }
            }
            crate::frontend_cache_color::transform_rgba(
                &mut rgba, neutral_gamma, surface_is_srgb,
            );
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&rgba, job.width, job.height, image::ExtendedColorType::Rgba8)
                .map_err(|error| format!("menu PNG encode failed: {error}"))?;
            let parent = path.parent().ok_or("invalid frontend scene cache path")?;
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            // Atomic replacement on the same volume; never accept partial PNGs.
            let temp = path.with_extension("png.tmp");
            std::fs::write(&temp, png).map_err(|e| e.to_string())?;
            // Publish metadata first; PNG is the ready marker for cache readers.
            crate::frontend_cache_color::write_cache_metadata(
                &path, is_neutral, captured_gamma, job.width, job.height,
            )?;
            std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
            Ok(ScreenshotOutput::FrontendSceneSaved(path))
        }
        ScreenshotDestination::VideoFrame { frame_index, alpha, sdr_unpremultiply, .. } => {
            let row_bytes = job.width as usize * 4;
            let padded = job.padded_bytes_per_row as usize;
            let mut bgra = Vec::with_capacity(row_bytes * job.height as usize);
            for y in 0..job.height as usize {
                let row = &job.pixels[y * padded..y * padded + row_bytes];
                match job.pixel_format {
                    ScreenshotPixelFormat::Bgra => bgra.extend_from_slice(row),
                    ScreenshotPixelFormat::Rgba => {
                        for px in row.chunks_exact(4) {
                            bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                        }
                    }
                }
            }
            if alpha {
                if let Some(srgb_surface)=sdr_unpremultiply {alpha::resolve_bgra(&mut bgra,srgb_surface);}
            }
            let depth_f32le = job.depth_pixels.map(|pixels| {
                let mut depth = Vec::with_capacity(row_bytes * job.height as usize);
                for y in 0..job.height as usize {
                    depth.extend_from_slice(&pixels[y * padded..y * padded + row_bytes]);
                }
                depth
            });
            Ok(ScreenshotOutput::VideoFrame {
                frame_index,
                width: job.width,
                height: job.height,
                bgra,
                depth_f32le,
            })
        }
        ScreenshotDestination::Clipboard => {
            // PrintScreen is clipboard-only: same GPU readback as /screenshot.
            // The clipboard carries the raw bitmap (CF_DIB, for image editors)
            // plus a temp JPEG as a file drop (for chat apps, which then upload
            // the compressed file instead of re-encoding the bitmap to a large
            // PNG). A JPEG failure falls back to bitmap-only. This runs on the
            // screenshot worker so the render thread never blocks on it.
            let jpeg_started = Instant::now();
            let jpeg_file = encode_screenshot_jpeg(
                &job.pixels,
                job.width,
                job.height,
                job.padded_bytes_per_row,
                job.pixel_format,
            )
            .and_then(|(jpeg, _)| write_clipboard_jpeg(&jpeg))
            .map_err(|error| eprintln!("[JKA SCREENSHOT] clipboard JPEG skipped: {error}"))
            .ok();
            let jpeg_ms = jpeg_started.elapsed().as_secs_f64() * 1000.0;

            let clipboard_started = Instant::now();
            crate::clipboard::set_image_rgba8(
                job.width,
                job.height,
                &job.pixels,
                padded_row_bytes,
                matches!(job.pixel_format, ScreenshotPixelFormat::Bgra),
                jpeg_file.as_deref(),
            )
            .map_err(|error| format!("clipboard copy failed: {error}"))?;
            let clipboard_ms = clipboard_started.elapsed().as_secs_f64() * 1000.0;
            let total_ms = job.started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[JKA SCREENSHOT] {}x{} copy_record={:.3}ms copy_submit={:.3}ms map_request={:.3}ms gpu_wait={:.3}ms readback_copy={:.3}ms jpeg_420={:.3}ms clipboard={:.3}ms total={:.3}ms buffer={} copy={} output=clipboard jpeg_file={}",
                job.width,
                job.height,
                job.copy_record_ms,
                job.copy_submit_ms,
                job.map_request_ms,
                job.gpu_wait_ms,
                job.readback_copy_ms,
                jpeg_ms,
                clipboard_ms,
                total_ms,
                if job.reused_buffer { "reused" } else { "new" },
                if job.inline_copy { "inline" } else { "late-submit" },
                if jpeg_file.is_some() { "yes" } else { "no" },
            );
            Ok(ScreenshotOutput::Clipboard)
        }
    }
}

impl Renderer {
    pub(in crate::renderer) fn request_screenshot(
        &mut self,
        directory: PathBuf,
        metadata: crate::screenshot::ScreenshotMetadata,
        format: ScreenshotFormat,
    ) {
        // Capture the destination together with the request. `fs_game` can change
        // without a renderer restart, so renderer construction time is not a safe
        // place to decide where a game-relative write belongs.
        self.request_screenshot_capture(ScreenshotDestination::File {
            directory,
            metadata,
            format,
        });
    }

    pub(in crate::renderer) fn request_frontend_scene_capture(&mut self, path: PathBuf, neutral: bool) {
        // Capture effective gamma at the same time as the readback request.
        // Hardware/baked methods are NOT invertible image-space corrections.
        let captured_gamma = self.output_gamma();
        let neutral_gamma = if neutral { captured_gamma } else { 1.0 };
        let format = self.hdr_output.as_ref().map_or(
            self.config.format, |_| wgpu::TextureFormat::Bgra8UnormSrgb,
        );
        let surface_is_srgb = matches!(format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
        );
        // A user-requested screenshot has priority over opportunistic caching.
        if self.screenshot_requested.is_none() {
            self.request_screenshot_capture(ScreenshotDestination::FrontendScene {
                path, neutral_gamma, captured_gamma, is_neutral: neutral, surface_is_srgb,
            });
        }
    }

    pub(in crate::renderer) fn request_clipboard_capture(&mut self) {
        self.request_screenshot_capture(ScreenshotDestination::Clipboard);
    }

    pub(in crate::renderer) fn request_video_frame_capture(&mut self, frame_index: u64, depth: bool, alpha: bool) {
        let sdr_unpremultiply = self.hdr_output.is_none().then_some(self.config.format.is_srgb());
        self.request_screenshot_capture(ScreenshotDestination::VideoFrame {
            frame_index, depth, alpha, sdr_unpremultiply,
        });
    }

    pub(in crate::renderer) fn video_depth_capture_requested(&self) -> bool {
        matches!(
            self.screenshot_requested.as_ref(),
            Some(ScreenshotDestination::VideoFrame { depth: true, .. })
        )
    }

    pub(in crate::renderer) fn request_screenshot_capture(
        &mut self,
        destination: ScreenshotDestination,
    ) {
        if !self.screenshot_supported {
            self.screenshot_result = Some(Err(
                "the current WGPU surface does not support COPY_SRC readback".into(),
            ));
            return;
        }
        self.screenshot_requested = Some(destination);
    }

    pub(in crate::renderer) fn take_screenshot_result(
        &mut self,
    ) -> Option<Result<ScreenshotOutput, String>> {
        self.screenshot_result
            .take()
            .or_else(|| self.screenshot_worker_rx.try_recv().ok())
    }

    pub(in crate::renderer) fn prepare_screenshot_readback(
        &mut self,
    ) -> Option<ScreenshotReadback> {
        let destination = self.screenshot_requested.take()?;
        let depth_requested = matches!(
            &destination,
            ScreenshotDestination::VideoFrame { depth: true, .. }
        );

        let started = Instant::now();
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        if matches!(
            &destination,
            ScreenshotDestination::File {
                format: ScreenshotFormat::Jpeg,
                ..
            }
        ) && (width > u16::MAX as u32 || height > u16::MAX as u32)
        {
            self.screenshot_result = Some(Err(format!(
                "screenshot dimensions {width}x{height} exceed JPEG's 65535-pixel limit"
            )));
            return None;
        }
        let supported_format = matches!(
            self.hdr_output.as_ref().map_or(self.config.format, |_| wgpu::TextureFormat::Bgra8UnormSrgb),
            wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
        );
        if !supported_format {
            self.screenshot_result = Some(Err(format!(
                "unsupported screenshot surface format {:?}",
                self.hdr_output.as_ref().map_or(self.config.format, |_| wgpu::TextureFormat::Bgra8UnormSrgb)
            )));
            return None;
        }

        let bytes_per_row = width.saturating_mul(4);
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = bytes_per_row.div_ceil(alignment) * alignment;
        let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
        let reused_buffer = self
            .screenshot_readback_buffer
            .as_ref()
            .is_some_and(|readback| readback.size >= buffer_size);
        if !reused_buffer {
            self.screenshot_readback_buffer = Some(ScreenshotReadbackBuffer {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA screenshot readback"),
                    size: buffer_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                size: buffer_size,
            });
        }
        if depth_requested
            && !self
                .video_depth_readback_buffer
                .as_ref()
                .is_some_and(|readback| readback.size >= buffer_size)
        {
            self.video_depth_readback_buffer = Some(ScreenshotReadbackBuffer {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA video depth readback"),
                    size: buffer_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                size: buffer_size,
            });
        }

        Some(ScreenshotReadback {
            destination,
            width,
            height,
            padded_bytes_per_row,
            buffer_size,
            format: self.hdr_output.as_ref().map_or(self.config.format, |_| wgpu::TextureFormat::Bgra8UnormSrgb),
            submission_index: None,
            copy_record_ms: 0.0,
            copy_submit_ms: 0.0,
            inline_copy: false,
            reused_buffer,
            depth_requested,
            started,
        })
    }

    pub(in crate::renderer) fn encode_screenshot_copy(
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        staging_buffer: &wgpu::Buffer,
        readback: &mut ScreenshotReadback,
    ) {
        let record_started = Instant::now();
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(readback.padded_bytes_per_row),
                    rows_per_image: Some(readback.height),
                },
            },
            wgpu::Extent3d {
                width: readback.width,
                height: readback.height,
                depth_or_array_layers: 1,
            },
        );
        readback.copy_record_ms += record_started.elapsed().as_secs_f64() * 1000.0;
    }

    pub(in crate::renderer) fn submit_screenshot_copy(
        &self,
        texture: &wgpu::Texture,
        readback: &mut ScreenshotReadback,
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA screenshot copy encoder"),
            });
        let staging_buffer = &self
            .screenshot_readback_buffer
            .as_ref()
            .expect("screenshot staging buffer must exist after preparation")
            .buffer;
        Self::encode_screenshot_copy(&mut encoder, texture, staging_buffer, readback);
        if readback.depth_requested {
            let depth_staging_buffer = &self
                .video_depth_readback_buffer
                .as_ref()
                .expect("video depth staging buffer must exist after preparation")
                .buffer;
            Self::encode_screenshot_copy(
                &mut encoder,
                &self.targets._linear_depth_texture,
                depth_staging_buffer,
                readback,
            );
        }
        let submit_started = Instant::now();
        readback.submission_index = Some(self.queue.submit([encoder.finish()]));
        readback.copy_submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;
    }

    pub(in crate::renderer) fn finish_screenshot_readback(&mut self, readback: ScreenshotReadback) {
        if let Err(error) = self.queue_screenshot_encode(readback) {
            self.screenshot_result = Some(Err(error));
        }
    }

    pub(in crate::renderer) fn queue_screenshot_encode(
        &self,
        readback: ScreenshotReadback,
    ) -> Result<(), String> {
        let staging = self
            .screenshot_readback_buffer
            .as_ref()
            .ok_or_else(|| "screenshot readback buffer is missing".to_owned())?;
        let slice = staging.buffer.slice(0..readback.buffer_size);
        let (mapped_tx, mapped_rx) = mpsc::sync_channel(1);
        let map_request_started = Instant::now();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = mapped_tx.send(result);
        });

        let depth_staging = if readback.depth_requested {
            Some(
                self.video_depth_readback_buffer
                    .as_ref()
                    .ok_or_else(|| "video depth readback buffer is missing".to_owned())?,
            )
        } else {
            None
        };
        let depth_slice = depth_staging
            .as_ref()
            .map(|staging| staging.buffer.slice(0..readback.buffer_size));
        let depth_mapped_rx = depth_slice.as_ref().map(|slice| {
            let (tx, rx) = mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            rx
        });
        let map_request_ms = map_request_started.elapsed().as_secs_f64() * 1000.0;

        let gpu_wait_started = Instant::now();
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: readback.submission_index.clone(),
                timeout: None,
            })
            .map_err(|error| format!("GPU readback wait failed: {error}"))?;
        let gpu_wait_ms = gpu_wait_started.elapsed().as_secs_f64() * 1000.0;
        mapped_rx
            .recv()
            .map_err(|_| "GPU screenshot mapping callback was lost".to_owned())?
            .map_err(|error| format!("GPU screenshot mapping failed: {error}"))?;
        if let Some(depth_rx) = depth_mapped_rx {
            depth_rx
                .recv()
                .map_err(|_| "GPU depth mapping callback was lost".to_owned())?
                .map_err(|error| format!("GPU depth mapping failed: {error}"))?;
        }

        let copy_started = Instant::now();
        let mapped = slice.get_mapped_range().map_err(|error| {
            staging.buffer.unmap();
            if let Some(depth_staging) = depth_staging {
                depth_staging.buffer.unmap();
            }
            format!("GPU screenshot readback range failed: {error}")
        })?;
        let pixels = mapped.to_vec();
        drop(mapped);
        staging.buffer.unmap();
        let depth_pixels = if let (Some(depth_staging), Some(depth_slice)) = (depth_staging, depth_slice) {
            let mapped = depth_slice.get_mapped_range().map_err(|error| {
                depth_staging.buffer.unmap();
                format!("GPU depth readback range failed: {error}")
            })?;
            let pixels = mapped.to_vec();
            drop(mapped);
            depth_staging.buffer.unmap();
            Some(pixels)
        } else {
            None
        };
        let readback_copy_ms = copy_started.elapsed().as_secs_f64() * 1000.0;

        let pixel_format = if matches!(
            readback.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            ScreenshotPixelFormat::Bgra
        } else {
            ScreenshotPixelFormat::Rgba
        };
        self.screenshot_worker_tx
            .send(ScreenshotEncodeJob {
                pixels,
                depth_pixels,
                width: readback.width,
                height: readback.height,
                padded_bytes_per_row: readback.padded_bytes_per_row,
                pixel_format,
                destination: readback.destination,
                copy_record_ms: readback.copy_record_ms,
                copy_submit_ms: readback.copy_submit_ms,
                map_request_ms,
                gpu_wait_ms,
                readback_copy_ms,
                inline_copy: readback.inline_copy,
                reused_buffer: readback.reused_buffer,
                started: readback.started,
            })
            .map_err(|_| "screenshot encoding worker stopped".to_owned())
    }
}
