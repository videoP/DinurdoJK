//! Id Software RoQ (Quake III / Jedi Academy cinematic) video decoder.
//!
//! Only the video stream is decoded: shader `videoMap` playback is silent in the
//! engine (`CIN_loop | CIN_silent`), so audio chunks are skipped. Frames are
//! produced as tightly packed RGBA8, already converted from the codec's
//! full-range YCbCr.

use std::sync::Arc;

const SIGNATURE: u16 = 0x1084;
const CHUNK_QUAD_INFO: u16 = 0x1001;
const CHUNK_QUAD_CODEBOOK: u16 = 0x1002;
const CHUNK_QUAD_VQ: u16 = 0x1011;
const HEADER_LEN: usize = 8;
const CHUNK_HEADER_LEN: usize = 8;
const DEFAULT_FPS: u32 = 30;

/// One 2x2 codebook cell as four RGBA pixels, row-major.
type Cell = [[u8; 4]; 4];

pub struct RoqVideo {
    data: Arc<[u8]>,
    width: u32,
    height: u32,
    fps: u32,
    /// Offset of the first chunk after the file header.
    start: usize,
    cursor: usize,
    /// Frame buffers are padded to whole 16x16 macroblocks.
    stride: usize,
    padded_height: usize,
    current: Vec<u8>,
    previous: Vec<u8>,
    cells_2x2: Vec<Cell>,
    cells_4x4: Vec<[u8; 4]>,
    output: Vec<u8>,
}

impl RoqVideo {
    pub fn open(data: impl Into<Arc<[u8]>>) -> Result<Self, String> {
        let data: Arc<[u8]> = data.into();
        if data.len() < HEADER_LEN || u16::from_le_bytes([data[0], data[1]]) != SIGNATURE {
            return Err("not a RoQ file".into());
        }
        let fps = u16::from_le_bytes([data[6], data[7]]) as u32;
        let fps = if fps == 0 { DEFAULT_FPS } else { fps };

        let mut offset = HEADER_LEN;
        let mut size = None;
        while let Some((id, len, _)) = chunk_header(&data, offset) {
            let body = offset + CHUNK_HEADER_LEN;
            if id == CHUNK_QUAD_INFO && body + 4 <= data.len() {
                let w = u16::from_le_bytes([data[body], data[body + 1]]) as u32;
                let h = u16::from_le_bytes([data[body + 2], data[body + 3]]) as u32;
                size = Some((w, h));
                break;
            }
            offset = body.saturating_add(len);
        }
        let (width, height) = size.ok_or("RoQ file has no QUAD_INFO chunk")?;
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(format!("unsupported RoQ size {width}x{height}"));
        }
        let stride = width.next_multiple_of(16) as usize;
        let padded_height = height.next_multiple_of(16) as usize;
        let frame_len = stride * padded_height * 4;
        let mut black = vec![0u8; frame_len];
        black.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        Ok(Self {
            data,
            width,
            height,
            fps,
            start: HEADER_LEN,
            cursor: HEADER_LEN,
            stride,
            padded_height,
            previous: black.clone(),
            current: black,
            cells_2x2: vec![[[0, 0, 0, 255]; 4]; 256],
            cells_4x4: vec![[0; 4]; 256],
            output: vec![0; (width * height * 4) as usize],
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Restart from the first frame (video maps loop).
    pub fn rewind(&mut self) {
        self.cursor = self.start;
        for frame in [&mut self.current, &mut self.previous] {
            frame.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[0, 0, 0, 255]));
        }
    }

    /// The most recently decoded frame (black before the first `next_frame`).
    pub fn current_frame(&self) -> &[u8] {
        &self.output
    }

    /// Decode the next frame, returning cropped RGBA8, or `None` at end of stream.
    pub fn next_frame(&mut self) -> Option<&[u8]> {
        loop {
            let (id, len, arg) = chunk_header(&self.data, self.cursor)?;
            let body = self.cursor + CHUNK_HEADER_LEN;
            let end = body.saturating_add(len).min(self.data.len());
            self.cursor = body.saturating_add(len);
            match id {
                CHUNK_QUAD_CODEBOOK => self.read_codebook(body, end, arg, len),
                CHUNK_QUAD_VQ => {
                    self.previous.copy_from_slice(&self.current);
                    self.decode_vq(body, end, arg);
                    self.crop();
                    return Some(&self.output);
                }
                _ => {}
            }
        }
    }

    fn read_codebook(&mut self, mut pos: usize, end: usize, arg: u16, len: usize) {
        let mut count_2x2 = (arg >> 8) as usize;
        if count_2x2 == 0 {
            count_2x2 = 256;
        }
        let mut count_4x4 = (arg & 0xFF) as usize;
        if count_4x4 == 0 && count_2x2 * 6 < len {
            count_4x4 = 256;
        }
        let data = &self.data;
        for cell in self.cells_2x2.iter_mut().take(count_2x2) {
            if pos + 6 > end {
                return;
            }
            let [y0, y1, y2, y3, u, v] = [0, 1, 2, 3, 4, 5].map(|i| data[pos + i]);
            *cell = [y0, y1, y2, y3].map(|y| yuv_to_rgba(y, u, v));
            pos += 6;
        }
        for cell in self.cells_4x4.iter_mut().take(count_4x4) {
            if pos + 4 > end {
                return;
            }
            *cell = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            pos += 4;
        }
    }

    fn decode_vq(&mut self, start: usize, end: usize, arg: u16) {
        let data = Arc::clone(&self.data);
        let mut reader = Reader { data: &data, pos: start, end };
        let motion_bias = (((arg >> 8) as u8) as i8 as i32, (arg as u8) as i8 as i32);
        let mut flags = 0u16;
        let mut flag_pos = -1i32;
        let (width, height) = (self.width as usize, self.height as usize);
        let (mut mb_x, mut mb_y) = (0usize, 0usize);

        let mut next_code = |reader: &mut Reader| -> Option<u8> {
            if flag_pos < 0 {
                flags = reader.u16()?;
                flag_pos = 7;
            }
            let code = ((flags >> (flag_pos * 2)) & 3) as u8;
            flag_pos -= 1;
            Some(code)
        };

        'blocks: while mb_y < height {
            for (dy, dx) in [(0, 0), (0, 8), (8, 0), (8, 8)] {
                let (x, y) = (mb_x + dx, mb_y + dy);
                let Some(code) = next_code(&mut reader) else { break 'blocks };
                match code {
                    0 => {}
                    1 => {
                        let Some(byte) = reader.u8() else { break 'blocks };
                        let (mx, my) = motion_vector(byte, motion_bias);
                        self.copy_block(x, y, mx, my, 8);
                    }
                    2 => {
                        let Some(index) = reader.u8() else { break 'blocks };
                        let quads = self.cells_4x4[index as usize];
                        for (q, &cell) in quads.iter().enumerate() {
                            let (qx, qy) = (x + (q & 1) * 4, y + (q >> 1) * 4);
                            self.paint_cell(qx, qy, cell as usize, 2);
                        }
                    }
                    _ => {
                        for k in 0..4 {
                            let (x, y) = (x + (k & 1) * 4, y + (k >> 1) * 4);
                            let Some(code) = next_code(&mut reader) else { break 'blocks };
                            match code {
                                0 => {}
                                1 => {
                                    let Some(byte) = reader.u8() else { break 'blocks };
                                    let (mx, my) = motion_vector(byte, motion_bias);
                                    self.copy_block(x, y, mx, my, 4);
                                }
                                2 => {
                                    let Some(index) = reader.u8() else { break 'blocks };
                                    let quads = self.cells_4x4[index as usize];
                                    for (q, &cell) in quads.iter().enumerate() {
                                        let (qx, qy) = (x + (q & 1) * 2, y + (q >> 1) * 2);
                                        self.paint_cell(qx, qy, cell as usize, 1);
                                    }
                                }
                                _ => {
                                    for q in 0..4 {
                                        let Some(cell) = reader.u8() else { break 'blocks };
                                        let (qx, qy) = (x + (q & 1) * 2, y + (q >> 1) * 2);
                                        self.paint_cell(qx, qy, cell as usize, 1);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            mb_x += 16;
            if mb_x >= width {
                mb_x = 0;
                mb_y += 16;
            }
        }
    }

    /// Paint a codebook cell at (x, y); `scale` 1 = 2x2 pixels, 2 = 4x4 pixels.
    fn paint_cell(&mut self, x: usize, y: usize, cell: usize, scale: usize) {
        let cell = self.cells_2x2[cell];
        for (i, pixel) in cell.iter().enumerate() {
            let (px, py) = ((i & 1) * scale, (i >> 1) * scale);
            for oy in 0..scale {
                for ox in 0..scale {
                    let (tx, ty) = (x + px + ox, y + py + oy);
                    if tx < self.stride && ty < self.padded_height {
                        let at = (ty * self.stride + tx) * 4;
                        self.current[at..at + 4].copy_from_slice(pixel);
                    }
                }
            }
        }
    }

    /// Copy a `size`-square block from the previous frame, displaced by (mx, my).
    fn copy_block(&mut self, x: usize, y: usize, mx: i32, my: i32, size: usize) {
        for row in 0..size {
            let dst_y = y + row;
            let src_y = dst_y as i32 + my;
            if dst_y >= self.padded_height || src_y < 0 || src_y as usize >= self.padded_height {
                continue;
            }
            for col in 0..size {
                let dst_x = x + col;
                let src_x = dst_x as i32 + mx;
                if dst_x >= self.stride || src_x < 0 || src_x as usize >= self.stride {
                    continue;
                }
                let src = (src_y as usize * self.stride + src_x as usize) * 4;
                let dst = (dst_y * self.stride + dst_x) * 4;
                self.current[dst..dst + 4].copy_from_slice(&self.previous[src..src + 4]);
            }
        }
    }

    fn crop(&mut self) {
        let row = self.width as usize * 4;
        for y in 0..self.height as usize {
            let src = y * self.stride * 4;
            self.output[y * row..(y + 1) * row].copy_from_slice(&self.current[src..src + row]);
        }
    }
}

fn motion_vector(byte: u8, (bias_x, bias_y): (i32, i32)) -> (i32, i32) {
    (8 - (byte >> 4) as i32 - bias_x, 8 - (byte & 0xF) as i32 - bias_y)
}

fn chunk_header(data: &[u8], offset: usize) -> Option<(u16, usize, u16)> {
    let header = data.get(offset..offset.checked_add(CHUNK_HEADER_LEN)?)?;
    Some((
        u16::from_le_bytes([header[0], header[1]]),
        u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize,
        u16::from_le_bytes([header[6], header[7]]),
    ))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        (self.pos < self.end).then(|| {
            let value = self.data[self.pos];
            self.pos += 1;
            value
        })
    }

    fn u16(&mut self) -> Option<u16> {
        let low = self.u8()?;
        let high = self.u8()?;
        Some(u16::from_le_bytes([low, high]))
    }
}

/// Full-range (JPEG) YCbCr to RGBA.
fn yuv_to_rgba(y: u8, u: u8, v: u8) -> [u8; 4] {
    let (y, u, v) = (y as f32, u as f32 - 128.0, v as f32 - 128.0);
    let clamp = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    [
        clamp(y + 1.402 * v),
        clamp(y - 0.344_136 * u - 0.714_136 * v),
        clamp(y + 1.772 * u),
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u16, arg: u16, body: &[u8]) -> Vec<u8> {
        let mut out = id.to_le_bytes().to_vec();
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(arg.to_le_bytes());
        out.extend(body);
        out
    }

    fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = SIGNATURE.to_le_bytes().to_vec();
        out.extend(0xFFFF_FFFFu32.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        for c in chunks {
            out.extend(c);
        }
        out
    }

    #[test]
    fn decodes_a_solid_16x16_frame_and_loops() {
        // One codebook entry: Y=200 U=V=128 (grey 200). Every 4x4 sub-block of every
        // 8x8 block is painted with the 2x2 cell via the "slide" code.
        let info = chunk(CHUNK_QUAD_INFO, 0, &[16, 0, 16, 0, 2, 0, 2, 0]);
        let mut book = vec![200, 200, 200, 200, 128, 128];
        book.extend([0, 0, 0, 0]);
        let book = chunk(CHUNK_QUAD_CODEBOOK, 0x0101, &book);
        // Four 8x8 blocks, each code 2 (SLD) = 0b10 -> flags 0b1010_1010_0000_0000.
        let mut vq = 0b1010_1010_0000_0000u16.to_le_bytes().to_vec();
        vq.extend([0, 0, 0, 0]);
        let vq = chunk(CHUNK_QUAD_VQ, 0, &vq);
        let mut video = RoqVideo::open(file(&[info, book, vq])).unwrap();
        assert_eq!((video.width(), video.height(), video.fps()), (16, 16, 24));
        let frame = video.next_frame().unwrap();
        assert_eq!(frame.len(), 16 * 16 * 4);
        assert!(frame.chunks_exact(4).all(|p| p == [200, 200, 200, 255]));
        assert!(video.next_frame().is_none());
        video.rewind();
        assert!(video.next_frame().is_some());
    }

    #[test]
    fn rejects_non_roq() {
        assert!(RoqVideo::open(vec![0u8; 32]).is_err());
    }

    /// `JKA_TEST_ROQ=<file.roq> cargo test -p jka-assets roq_file -- --ignored --nocapture`
    /// decodes every frame and writes the first/middle frame as a PPM next to the file.
    #[test]
    #[ignore]
    fn roq_file() {
        let path = std::env::var("JKA_TEST_ROQ").expect("JKA_TEST_ROQ");
        let mut video = RoqVideo::open(std::fs::read(&path).unwrap()).unwrap();
        let (w, h) = (video.width(), video.height());
        let mut count = 0;
        while let Some(frame) = video.next_frame() {
            if count == 0 || count == 60 {
                let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                frame.chunks_exact(4).for_each(|p| ppm.extend(&p[..3]));
                std::fs::write(format!("{path}.frame{count}.ppm"), ppm).unwrap();
            }
            count += 1;
        }
        println!("{w}x{h} @ {} fps, {count} frames", video.fps());
    }
}
