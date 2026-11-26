//! Ordered, bounded streaming PNG/JPEG encoder for panorama outputs that never
//! exist as one image. A producer (GPU strip readback or CPU projector) sends
//! complete RGB8 row bands in order; this side encodes them on its own thread.
use super::{check_cancel, progress, UserEvent};
use std::{cell::RefCell, fs::File, io::{BufWriter, Write}, path::PathBuf, sync::{atomic::AtomicBool, mpsc, Arc}, time::{Duration, Instant}};
use winit::event_loop::EventLoopProxy;

/// Bands queued between producer and encoder, beyond the one being encoded.
const QUEUED_BANDS: usize = 2;

pub(super) struct Band { pub rows: u32, pub rgb: Vec<u8> }

/// Producer half. Buffers are recycled, so memory stays at a few bands.
pub(super) struct BandSender {
    tx: mpsc::SyncSender<Band>,
    recycle: mpsc::Receiver<Vec<u8>>,
    row_bytes: usize,
    pub(super) send_wait: Duration,
}
impl BandSender {
    pub(super) fn buffer(&self, rows: u32) -> Vec<u8> {
        let mut buffer = self.recycle.try_recv().unwrap_or_default();
        buffer.resize(self.row_bytes * rows as usize, 0);
        buffer
    }
    /// Err means the encoder stopped; join it for the actual error.
    pub(super) fn send(&mut self, band: Band) -> Result<(), ()> {
        let started = Instant::now();
        let result = self.tx.send(band).map_err(|_| ());
        self.send_wait += started.elapsed();
        result
    }
}

/// Encoder half: serves rows in non-decreasing order.
pub(super) struct BandStream {
    rx: mpsc::Receiver<Band>,
    recycle: mpsc::Sender<Vec<u8>>,
    row_bytes: usize,
    current: Option<Band>,
    start: u32,
    next: u32,
    input_wait: Duration,
}
impl BandStream {
    fn row(&mut self, y: u32) -> Result<&[u8], String> {
        loop {
            if let Some(band) = &self.current {
                if y < self.start { return Err("Panorama rows requested out of order".into()); }
                if y < self.start + band.rows { break; }
            }
            let started = Instant::now();
            let band = self.rx.recv().map_err(|_| "Panorama row producer stopped".to_owned())?;
            self.input_wait += started.elapsed();
            if band.rgb.len() < band.rows as usize * self.row_bytes { return Err("Short panorama row band".into()); }
            if let Some(old) = self.current.replace(band) {
                let _ = self.recycle.send(old.rgb);
            }
            self.start = self.next;
            self.next += self.current.as_ref().map_or(0, |b| b.rows);
        }
        let offset = (y - self.start) as usize * self.row_bytes;
        Ok(&self.current.as_ref().unwrap().rgb[offset..offset + self.row_bytes])
    }
}

pub(super) fn channel(width: u32) -> (BandSender, BandStream) {
    let (tx, rx) = mpsc::sync_channel(QUEUED_BANDS);
    let (recycle_tx, recycle_rx) = mpsc::channel();
    let row_bytes = width as usize * 3;
    (BandSender { tx, recycle: recycle_rx, row_bytes, send_wait: Duration::ZERO },
     BandStream { rx, recycle: recycle_tx, row_bytes, current: None, start: 0, next: 0, input_wait: Duration::ZERO })
}

pub(super) struct EncodeJob {
    pub path: PathBuf,
    pub jpeg: bool,
    pub width: u32,
    pub height: u32,
    pub xmp: String,
    pub cancel: Arc<AtomicBool>,
    pub proxy: EventLoopProxy<UserEvent>,
}

#[derive(Default, Clone, Copy)]
pub(super) struct EncodeStats { pub wall: Duration, pub input_wait: Duration, pub bytes: u64 }

/// Producer error meaning "the encoder hung up"; the encoder's error wins.
pub(super) const ENCODER_STOPPED: &str = "Panorama encoder stopped";

/// Run `produce` on this thread and the encoder on a scoped thread, so row
/// production overlaps encoding. Both are joined before returning; the
/// `.part` file is never published here. Returns encoder stats and the time
/// the producer spent blocked on encoder backpressure.
pub(super) fn run(job: EncodeJob, produce: impl FnOnce(&mut BandSender) -> Result<(), String>) -> Result<(EncodeStats, Duration), String> {
    let (mut sender, stream) = channel(job.width);
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().name("photo-encode".into())
            .spawn_scoped(scope, move || encode(stream, job)).map_err(|e| e.to_string())?;
        let produced = produce(&mut sender);
        let send_wait = sender.send_wait;
        drop(sender);
        let encoded = worker.join().unwrap_or_else(|_| Err("Panorama encoder panicked".into()));
        match (produced, encoded) {
            (Err(error), Err(encoder)) if error == ENCODER_STOPPED => Err(encoder),
            (Err(error), _) => Err(error),
            (Ok(()), encoded) => encoded.map(|stats| (stats, send_wait)),
        }
    })
}

/// Runs on the encoder thread. Writes only `job.path` (a private scratch file).
pub(super) fn encode(mut stream: BandStream, job: EncodeJob) -> Result<EncodeStats, String> {
    let started = Instant::now();
    check_cancel(&job.cancel)?;
    let io = |e: std::io::Error| format!("Panorama encode: {e}");
    let file = File::create(&job.path).map_err(io)?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    let mut last = Instant::now();
    let mut report = |rows: u32, force: bool| {
        if force || last.elapsed() >= Duration::from_millis(500) {
            progress(&job.proxy, format!("PHOTO: projecting and encoding {rows}/{} rows", job.height), u64::from(rows), u64::from(job.height), started);
            last = Instant::now();
        }
    };
    if job.jpeg {
        let (w, h) = (u16::try_from(job.width).map_err(|_| "JPEG width exceeds 65,535")?, u16::try_from(job.height).map_err(|_| "JPEG height exceeds 65,535")?);
        let error = RefCell::new(None);
        let sink = JpegSink { writer: &mut writer, cancel: &job.cancel, error: &error };
        // Quality 95, 4:4:4, baseline, standard tables: bounded streaming.
        let mut encoder = jpeg_encoder::Encoder::new(sink, 95);
        encoder.set_progressive(false);
        encoder.set_optimized_huffman_tables(false);
        encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
        let mut app1 = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        app1.extend_from_slice(job.xmp.as_bytes());
        encoder.add_app_segment(1, &app1).map_err(|e| format!("Panorama encode: {e}"))?;
        let rows = JpegRows { stream: RefCell::new(&mut stream), error: &error, width: w, height: h, report: RefCell::new(&mut report) };
        let result = encoder.encode_image(rows);
        check_cancel(&job.cancel)?;
        if let Some(error) = error.into_inner() { return Err(error); }
        result.map_err(|e| format!("Panorama encode: {e}"))?;
    } else {
        let mut png = png::Encoder::new(&mut writer, job.width, job.height);
        png.set_color(png::ColorType::Rgb);
        png.set_depth(png::BitDepth::Eight);
        png.add_itxt_chunk("XML:com.adobe.xmp".into(), job.xmp.clone()).map_err(|e| format!("Panorama encode: {e}"))?;
        let mut header = png.write_header().map_err(|e| format!("Panorama encode: {e}"))?;
        let mut rows = header.stream_writer_with_size(256 * 1024).map_err(|e| format!("Panorama encode: {e}"))?;
        for y in 0..job.height {
            if y % 64 == 0 { check_cancel(&job.cancel)?; }
            rows.write_all(stream.row(y)?).map_err(io)?;
            report(y + 1, y + 1 == job.height);
        }
        rows.finish().map_err(|e| format!("Panorama encode: {e}"))?;
        header.finish().map_err(|e| format!("Panorama encode: {e}"))?;
    }
    writer.flush().map_err(io)?;
    let bytes = writer.get_ref().metadata().map_or(0, |m| m.len());
    drop(writer);
    check_cancel(&job.cancel)?;
    Ok(EncodeStats { wall: started.elapsed(), input_wait: stream.input_wait, bytes })
}

struct JpegRows<'a, 'b, R: FnMut(u32, bool)> {
    stream: RefCell<&'a mut BandStream>,
    error: &'b RefCell<Option<String>>,
    width: u16,
    height: u16,
    report: RefCell<&'a mut R>,
}
impl<R: FnMut(u32, bool)> jpeg_encoder::ImageBuffer for JpegRows<'_, '_, R> {
    fn get_jpeg_color_type(&self) -> jpeg_encoder::JpegColorType { jpeg_encoder::JpegColorType::Ycbcr }
    fn width(&self) -> u16 { self.width }
    fn height(&self) -> u16 { self.height }
    fn fill_buffers(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
        let mut stream = self.stream.borrow_mut();
        let row = if self.error.borrow().is_none() {
            match stream.row(u32::from(y)) { Ok(row) => Some(row), Err(error) => { *self.error.borrow_mut() = Some(error); None } }
        } else { None };
        let Some(row) = row else {
            // The sink rejects all further output; keep the encoder's row shape.
            for buffer in &mut buffers[..3] { buffer.extend(std::iter::repeat_n(0, self.width as usize)); }
            return;
        };
        for rgb in row.chunks_exact(3) {
            let (luma, cb, cr) = jpeg_encoder::rgb_to_ycbcr(rgb[0], rgb[1], rgb[2]);
            buffers[0].push(luma); buffers[1].push(cb); buffers[2].push(cr);
        }
        (self.report.borrow_mut())(u32::from(y) + 1, u32::from(y) + 1 == u32::from(self.height));
    }
}

pub(super) struct JpegSink<'a, W> {
    pub(super) writer: W,
    pub(super) cancel: &'a AtomicBool,
    pub(super) error: &'a RefCell<Option<String>>,
}
impl<W: Write> Write for JpegSink<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        check_cancel(self.cancel).map_err(std::io::Error::other)?;
        if let Some(error) = self.error.borrow().as_ref() { return Err(std::io::Error::other(error.clone())); }
        self.writer.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.writer.flush() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jpeg_sink_rejects_cancel_and_projection_errors() {
        let cancel = AtomicBool::new(false);
        let error = RefCell::new(None);
        let mut bytes = Vec::new();
        let mut sink = JpegSink { writer: &mut bytes, cancel: &cancel, error: &error };
        sink.write_all(b"ok").unwrap();
        *error.borrow_mut() = Some("projection error".into());
        assert!(sink.write_all(b"no").is_err());
        *error.borrow_mut() = None;
        cancel.store(true, std::sync::atomic::Ordering::Release);
        assert!(sink.write_all(b"no").is_err());
        drop(sink);
        assert_eq!(bytes, b"ok");
    }
    #[test]
    fn band_stream_serves_rows_in_order_and_recycles() {
        let (mut tx, mut rx) = channel(2);
        let mut a = tx.buffer(2); a.copy_from_slice(&[1,1,1,1,1,1, 2,2,2,2,2,2]);
        tx.send(Band { rows: 2, rgb: a }).unwrap();
        let mut b = tx.buffer(1); b.copy_from_slice(&[3,3,3,3,3,3]);
        tx.send(Band { rows: 1, rgb: b }).unwrap();
        assert_eq!(rx.row(0).unwrap()[0], 1);
        assert_eq!(rx.row(1).unwrap()[0], 2);
        assert_eq!(rx.row(2).unwrap()[0], 3);
        assert_eq!(rx.row(2).unwrap()[0], 3);
        assert!(rx.row(0).is_err());
        assert_eq!(tx.buffer(1).len(), 6);
        drop(tx);
        assert!(rx.row(3).is_err());
    }
}
