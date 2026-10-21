//! Heatmap overlay for the measured per-tile GPU cost (`gputilecost`).
//! Values are GPU timestamp deltas, not estimates: each tile's number is the
//! pass time with every screen-space pass scissored to that tile, minus the
//! time with a 1x1 scissor (vertex, compute and fixed pass cost).
use super::{rect, rect_outline, text, UiVertex};

const LABEL: [f32; 4] = [0.93, 0.95, 0.98, 0.98];
const GRID: [f32; 4] = [0.0, 0.0, 0.0, 0.55];

pub(crate) struct TileCostView<'a> {
    pub cols: u32,
    pub rows: u32,
    /// Tile cost in ms, row-major, `cols * rows` entries. NaN = no data.
    pub costs: &'a [f64],
    pub header: &'a [String],
}

/// Cold (blue) -> green -> yellow -> red, by share of the most expensive tile.
fn heat(t: f32) -> [f32; 3] {
    const STOPS: [[f32; 3]; 4] = [[0.10, 0.25, 0.90], [0.10, 0.80, 0.30], [0.95, 0.85, 0.10], [0.95, 0.15, 0.10]];
    let t = t.clamp(0.0, 1.0) * 3.0;
    let i = (t.floor() as usize).min(2);
    let f = t - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

pub(crate) fn build_tile_cost_vertices(out: &mut Vec<UiVertex>, view: &TileCostView<'_>, width: u32, height: u32) {
    out.clear();
    if width == 0 || height == 0 || view.cols == 0 || view.rows == 0 { return; }
    let max = view.costs.iter().copied().filter(|v| v.is_finite()).fold(0.0f64, f64::max);
    let total: f64 = view.costs.iter().copied().filter(|v| v.is_finite()).sum();
    let (w, h) = (width as f32, height as f32);
    for row in 0..view.rows {
        for col in 0..view.cols {
            let x0 = (col * width / view.cols) as f32;
            let y0 = (row * height / view.rows) as f32;
            let x1 = ((col + 1) * width / view.cols) as f32;
            let y1 = ((row + 1) * height / view.rows) as f32;
            let cost = view.costs.get((row * view.cols + col) as usize).copied().unwrap_or(f64::NAN);
            if cost.is_finite() {
                let t = if max > 0.0 { (cost / max) as f32 } else { 0.0 };
                let [r, g, b] = heat(t);
                rect(out, x0, y0, x1 - x0, y1 - y0, [r, g, b, 0.20 + 0.35 * t], width, height);
                // Microseconds, and the tile's share of the summed tile cost.
                let share = if total > 0.0 { cost / total * 100.0 } else { 0.0 };
                text(out, &format!("{:.0}us\n{:.1}%", cost * 1000.0, share),
                    x0 + 4.0, y0 + 4.0, 1.25, LABEL, width, height);
            } else {
                text(out, "--", x0 + 4.0, y0 + 4.0, 1.25, LABEL, width, height);
            }
            rect_outline(out, x0, y0, x1 - x0, y1 - y0, 1.0, GRID, width, height);
        }
    }
    let mut y = (h * 0.5).min(8.0);
    for line in view.header {
        rect(out, 4.0, y - 2.0, (line.len() as f32 * 6.0 * 1.4 + 8.0).min(w - 8.0), 9.0 * 1.4 + 4.0,
            [0.0, 0.0, 0.0, 0.65], width, height);
        text(out, line, 8.0, y, 1.4, LABEL, width, height);
        y += 9.0 * 1.4 + 6.0;
    }
}
