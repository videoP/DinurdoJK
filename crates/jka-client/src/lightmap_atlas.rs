//! Padded lightmap atlas built once on map load.
const PAGE: usize = 128;
const PAD: usize = 2;
const TILE: usize = PAGE + PAD * 2;

#[derive(Debug)]
pub struct Atlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    columns: usize,
}
impl Atlas {
    pub fn new(bytes: &[u8]) -> Self {
        let count = bytes.len() / (PAGE * PAGE * 3);
        assert!(count > 0 && bytes.len().is_multiple_of(PAGE * PAGE * 3));
        let columns = (count as f64).sqrt().ceil() as usize;
        let width = columns * TILE;
        let height = count.div_ceil(columns) * TILE;
        let mut rgba = vec![0; width * height * 4];
        for (page, data) in bytes
            .as_chunks::<{ PAGE * PAGE * 3 }>()
            .0
            .iter()
            .enumerate()
        {
            for y in 0..TILE {
                for x in 0..TILE {
                    let sx = x.saturating_sub(PAD).min(PAGE - 1);
                    let sy = y.saturating_sub(PAD).min(PAGE - 1);
                    let src = (sy * PAGE + sx) * 3;
                    let dst =
                        (((page / columns) * TILE + y) * width + (page % columns) * TILE + x) * 4;
                    rgba[dst..dst + 3].copy_from_slice(&data[src..src + 3]);
                    rgba[dst + 3] = 255;
                }
            }
        }
        Self {
            rgba,
            width: width as u32,
            height: height as u32,
            columns,
        }
    }

    pub fn uv(&self, page: usize, uv: [f32; 2]) -> [f32; 2] {
        [
            ((page % self.columns * TILE + PAD) as f32 + uv[0] * PAGE as f32) / self.width as f32,
            ((page / self.columns * TILE + PAD) as f32 + uv[1] * PAGE as f32) / self.height as f32,
        ]
    }
}
