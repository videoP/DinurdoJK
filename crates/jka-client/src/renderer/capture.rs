//! Capture.
use crate::renderer::{mpsc, Duration, Instant, PathBuf, Renderer, ScreenshotOutput};

pub(in crate::renderer) struct ScreenshotReadbackBuffer {
    pub(in crate::renderer) buffer: wgpu::Buffer,
    pub(in crate::renderer) size: u64,
}

pub(in crate::renderer) enum ScreenshotDestination {
    File {
        directory: PathBuf,
        metadata: crate::screenshot::ScreenshotMetadata,
    },
    Clipboard,
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
    pub(in crate::renderer) started: Instant,
}

#[derive(Clone, Copy)]
pub(in crate::renderer) enum ScreenshotPixelFormat {
    Bgra,
    Rgba,
}

pub(in crate::renderer) struct ScreenshotEncodeJob {
    pub(in crate::renderer) pixels: Vec<u8>,
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
        } => {
            let jpeg_started = Instant::now();
            let (mut jpeg, jpeg_input_mode) = encode_screenshot_jpeg(
                &job.pixels,
                job.width,
                job.height,
                job.padded_bytes_per_row,
                job.pixel_format,
            )?;
            crate::screenshot::embed_metadata(&mut jpeg, &metadata)?;
            let jpeg_ms = jpeg_started.elapsed().as_secs_f64() * 1000.0;

            let file_io_started = Instant::now();
            std::fs::create_dir_all(&screenshot_dir).map_err(|error| {
                format!("could not create {}: {error}", screenshot_dir.display())
            })?;
            let path = screenshot_dir.join(format!("shot{}.jpg", screenshot_timestamp()));
            std::fs::write(&path, &jpeg)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            let file_io_ms = file_io_started.elapsed().as_secs_f64() * 1000.0;

            let total_ms = job.started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[JKA SCREENSHOT] {}x{} copy_record={:.3}ms copy_submit={:.3}ms map_request={:.3}ms gpu_wait={:.3}ms readback_copy={:.3}ms jpeg_420={:.3}ms file_io={:.3}ms total={:.3}ms buffer={} copy={} jpeg_input={} output=file encode=worker",
                job.width,
                job.height,
                job.copy_record_ms,
                job.copy_submit_ms,
                job.map_request_ms,
                job.gpu_wait_ms,
                job.readback_copy_ms,
                jpeg_ms,
                file_io_ms,
                total_ms,
                if job.reused_buffer { "reused" } else { "new" },
                if job.inline_copy { "inline" } else { "late-submit" },
                jpeg_input_mode,
            );
            Ok(ScreenshotOutput::Saved(path))
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
    ) {
        // Capture the destination together with the request. `fs_game` can change
        // without a renderer restart, so renderer construction time is not a safe
        // place to decide where a game-relative write belongs.
        self.request_screenshot_capture(ScreenshotDestination::File {
            directory,
            metadata,
        });
    }

    pub(in crate::renderer) fn request_clipboard_capture(&mut self) {
        self.request_screenshot_capture(ScreenshotDestination::Clipboard);
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

        let started = Instant::now();
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        if matches!(&destination, ScreenshotDestination::File { .. })
            && (width > u16::MAX as u32 || height > u16::MAX as u32)
        {
            self.screenshot_result = Some(Err(format!(
                "screenshot dimensions {width}x{height} exceed JPEG's 65535-pixel limit"
            )));
            return None;
        }
        let supported_format = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
        );
        if !supported_format {
            self.screenshot_result = Some(Err(format!(
                "unsupported screenshot surface format {:?}",
                self.config.format
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

        Some(ScreenshotReadback {
            destination,
            width,
            height,
            padded_bytes_per_row,
            buffer_size,
            format: self.config.format,
            submission_index: None,
            copy_record_ms: 0.0,
            copy_submit_ms: 0.0,
            inline_copy: false,
            reused_buffer,
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

        let mapped = slice.get_mapped_range();
        let copy_started = Instant::now();
        let pixels = mapped.to_vec();
        let readback_copy_ms = copy_started.elapsed().as_secs_f64() * 1000.0;
        drop(mapped);
        staging.buffer.unmap();

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
