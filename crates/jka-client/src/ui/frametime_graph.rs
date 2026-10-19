//! Fully transparent native frametime bars. One X pixel per frame.
//! CPU intervals are mutually exclusive after accounting for early-submit
//! nesting and cadence-boundary normalization; async GPU data is readout only.
use super::{rect, text, UiVertex};

const PX_PER_MS: f32 = 24.0;
const TOP: f32 = 56.0;
const SEGMENT_COLORS: [[f32; 3]; 6] = [
    [0.35, 0.66, 1.00], // Prepare / theme blue
    [0.28, 0.86, 0.84], // Acquire / teal
    [0.67, 0.58, 1.00], // Encode / lavender
    [0.44, 0.90, 0.56], // Submit / green
    [1.00, 0.72, 0.24], // Present / amber
    [0.66, 0.72, 0.80], // Other / pacing
];
const ACCENT: [f32; 3] = [0.35, 0.66, 1.00];
const WARNING: [f32; 3] = [1.0, 0.72, 0.24];
const DANGER: [f32; 3] = [1.0, 0.45, 0.40];
const LABEL: [f32; 4] = [0.93, 0.95, 0.98, 0.98];
const FPS_TICKS: &[u32] = &[
    1000, 500, 333, 250, 200, 166, 125, 100,
    83, 71, 62, 55, 50, 45, 41, 38, 35, 33, 31, 29, 27, 25,
];
const MIN_TICK_LABEL_GAP: f32 = 16.0;

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if (b-a).abs() < 0.000_001 { return if x > b { 1.0 } else { 0.0 }; }
    let t = ((x-a)/(b-a)).clamp(0.0, 1.0);
    t*t*(3.0-2.0*t)
}
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0]+(b[0]-a[0])*t, a[1]+(b[1]-a[1])*t, a[2]+(b[2]-a[2])*t]
}
fn severity_color(ms: f32, [p80,p95,p99]: [f32; 3]) -> [f32; 4] {
    let amber = smoothstep(p80, p95, ms);
    let coral = smoothstep(p95, p99, ms);
    let rgb = mix(mix(ACCENT, WARNING, amber), DANGER, coral);
    [rgb[0], rgb[1], rgb[2], 1.0]
}

pub(crate) fn build_frame_time_graph_vertices(
    out: &mut Vec<UiVertex>,
    recent_to_old: impl Iterator<Item=(f32, [f32; 6])>,
    width: u32, height: u32, cutoffs: [f32; 3], gpu: Option<[Option<f32>; 5]>,
) {
    out.clear();
    if width == 0 || height == 0 { return; }
    let w = width as f32;
    let h = height as f32;

    // No filled background, no header panel, no darkening rectangle: menus,
    // gameplay, and HDR exposure underneath are preserved pixel-for-pixel.
    text(out, "FRAME TIME   1 PX / FRAME   FPS SCALE", 8.0, 3.0,
        1.80, LABEL, width, height);
    let legend = ["PREP", "ACQUIRE", "ENCODE", "SUBMIT", "PRESENT", "OTHER"];
    let mut legend_x = 8.0;
    for (name, rgb) in legend.iter().zip(SEGMENT_COLORS) {
        if legend_x + (name.len() as f32 * 9.0 + 26.0) > w { break; }
        rect(out, legend_x, 29.0, 12.0, 12.0,
            [rgb[0], rgb[1], rgb[2], 0.98], width, height);
        text(out, name, legend_x+17.0, 28.0, 1.50,
            LABEL, width, height);
        legend_x += name.len() as f32 * 9.0 + 32.0;
    }
    // When GPU timings are already enabled, expose a few important independent
    // system timings. They arrive later than their frame; never add these to
    // CPU bar segments or label them as aligned to a single displayed bar.
    if let Some([ocean_sim, ocean_surface, clouds, blue_clouds, world]) = gpu {
        let ms = |value: Option<f32>| value.map_or("--".to_string(), |v| format!("{v:.2}"));
        let message = format!(
            "GPU ASYNC MS: FFT {}  OCEAN {}  CLOUDS {}  BW CLOUDS {}  WORLD {}",
            ms(ocean_sim), ms(ocean_surface), ms(clouds), ms(blue_clouds), ms(world),
        );
        text(out, &message, 8.0, 44.0, 1.35,
            [0.73, 0.85, 0.98, 0.96], width, height);
    } else {
        text(out, "GPU SUBSYSTEMS: r_gpuTimings 1 (OPTIONAL / ASYNC)",
            8.0, 44.0, 1.35, [0.73, 0.85, 0.98, 0.88], width, height);
    }

    // Faint horizontal guides using integer FPS labels. Internally the graph
    // remains in frame milliseconds; the labels convert with ms = 1000 / fps.
    let mut last_labeled_y = f32::NEG_INFINITY;
    for &fps in FPS_TICKS {
        let ms = 1000.0 / fps as f32;
        let y = TOP + ms * PX_PER_MS;
        if y >= h { break; }
        if y - last_labeled_y < MIN_TICK_LABEL_GAP { continue; }
        rect(out, 0.0, y, w, 1.0, [0.35, 0.66, 1.0, 0.16], width, height);
        last_labeled_y = y;
    }

    for (age, (ms, phases)) in recent_to_old.enumerate() {
        if age >= width as usize { break; }
        let x = w - age as f32 - 1.0;
        let visible = (ms.max(0.0)*PX_PER_MS).max(1.0).min((h-TOP).max(0.0));
        if visible <= 0.0 { continue; }
        let mut y = TOP;
        for (i, phase) in phases.iter().enumerate() {
            let remaining = (TOP+visible-y).max(0.0);
            let length = (phase.max(0.0)*PX_PER_MS).min(remaining);
            if length <= 0.0 { continue; }
            let rgb = SEGMENT_COLORS[i];
            rect(out, x, y, 1.0, length,
                [rgb[0], rgb[1], rgb[2], 0.97], width, height);
            y += length;
        }
        // Percentile severity is marked on the *tip*; the category colors
        // still remain readable even for the 5%/1% slowest frames.
        let tip = severity_color(ms, cutoffs);
        rect(out, x, TOP+visible-2.0_f32.min(visible), 1.0,
            2.0_f32.min(visible), tip, width, height);
    }

    // UI primitives paint in submission order: emit the FPS labels *after* the
    // bars so slow-frame columns never obscure the Y-axis text. Guides remain
    // underneath the bars. The same spacing rule keeps labels paired with
    // their guide lines, without retaining geometry or extra per-frame state.
    let mut last_labeled_y = f32::NEG_INFINITY;
    for &fps in FPS_TICKS {
        let y = TOP + 1000.0 / fps as f32 * PX_PER_MS;
        if y >= h { break; }
        if y - last_labeled_y < MIN_TICK_LABEL_GAP { continue; }
        text(out, &format!("{fps} FPS"), 7.0, y + 2.0, 1.5,
            [0.90, 0.94, 0.98, 1.0], width, height);
        last_labeled_y = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bars_clip_to_physical_bottom() {
        let mut v = Vec::new();
        build_frame_time_graph_vertices(&mut v,
            [(0.5, [0.1,0.1,0.1,0.1,0.1,0.0]),
             (10.0, [1.0,1.0,2.0,1.0,0.5,4.5])].into_iter(),
            128, 120, [0.5,0.8,2.0], None);
        assert!(!v.is_empty());
        assert!(v.iter().all(|vertex| vertex.position[1].is_finite()));
        assert!(v.iter().any(|vertex| (vertex.position[1]+1.0).abs()<0.0001));
    }
    #[test]
    fn no_full_screen_tinted_background_rectangles() {
        let mut v = Vec::new();
        build_frame_time_graph_vertices(&mut v,
            [(1.0, [0.1,0.1,0.1,0.1,0.1,0.5])].into_iter(),
            1920, 1080, [0.8,0.9,1.0], None);
        assert!(v.iter().all(|p| p.position[1].is_finite()));
        // No single triangle spans the entire width AND a significant height.
        for triangle in v.chunks_exact(3) {
            let min_x = triangle.iter().map(|v| v.position[0]).fold(f32::INFINITY, f32::min);
            let max_x = triangle.iter().map(|v| v.position[0]).fold(f32::NEG_INFINITY, f32::max);
            let min_y = triangle.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            let max_y = triangle.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
            assert!(!(max_x-min_x > 1.9 && max_y-min_y > 0.03));
        }
    }
}
