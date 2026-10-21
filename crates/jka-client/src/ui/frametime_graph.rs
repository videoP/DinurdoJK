//! Fully transparent native frametime bars. One X pixel per frame.
//! CPU intervals are mutually exclusive after accounting for early-submit
//! nesting and cadence-boundary normalization; async GPU data is readout only.
use super::{rect, text, UiVertex};

const PX_PER_MS: f32 = 24.0;
// Header is now a compact, fully transparent subset of cg_drawFPS 2.
// All quantities explicitly distinguish CPU durations, asynchronously measured
// GPU timestamps, and hardware pipeline stats for *one* BSP render pass.
const TOP: f32 = 195.0;

#[derive(Default, Clone, Copy)]
pub(crate) struct FrameGraphSummary {
    pub average_fps: f32,
    pub fps_one_percent_low: f32,
    pub cpu: Option<[f64; 5]>,
    pub shader: Option<crate::runtime::WorldShaderPrograms>,
    pub world_draws: Option<u32>,
    pub pipeline_counts: Option<(u32, u32)>,
    pub cull: Option<[u32; 5]>,
    /// Measured hardware VS invocations / clipped primitives / FS invocations.
    /// Scope is the first primary BSP render pass (NOT all world passes).
    pub bsp: Option<(u8, [u64; 3])>,
    /// Measured VS / clipped primitives / FS for the OceanSurface interval, and
    /// the ocean occlusion query's samples passed (async, separate cadences).
    pub ocean: Option<([u64; 3], u64)>,
    /// All-map coarse BSP batch eligibility (not per-frame visible draw counts).
    /// total, eligible, global, sky, fusion, ocean stage, sprite2, RGB3,
    /// special tcGen, glass, other. Disabled for active modes 0/1.
    pub fallback_map: Option<(u8, [u32; 11])>,
}

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
    width: u32, height: u32, cutoffs: [f32; 3], gpu: Option<[Option<f32>; 11]>,
    summary: FrameGraphSummary,
) {
    out.clear();
    if width == 0 || height == 0 { return; }
    let w = width as f32;
    let h = height as f32;
    // Keep the graph valid for tiny test/preview viewports while retaining the
    // full diagnostic header on normal game windows.
    let top = TOP.min((h - 16.0).max(0.0));

    // No filled background, no header panel, no darkening rectangle: menus,
    // gameplay, and HDR exposure underneath are preserved pixel-for-pixel.
    text(out, &format!(
        "FPS {:.0}   1% LOW {:.0}   CPU CADENCE P95 {:.2} MS   FPS SCALE / 1 PX PER FRAME",
        summary.average_fps, summary.fps_one_percent_low, cutoffs[1],
    ), 8.0, 3.0, 1.6, LABEL, width, height);

    let ms = |v: f64| if v.is_finite() { format!("{v:.2}") } else { "--".to_owned() };
    if let Some([prep, acquire, encode, submit, present]) = summary.cpu {
        text(out, &format!(
            "CPU MS   PREP {}   ACQUIRE {}   ENCODE {}   SUBMIT {}   PRESENT {}",
            ms(prep), ms(acquire), ms(encode), ms(submit), ms(present),
        ), 8.0, 21.0, 1.38, LABEL, width, height);
    }
    // Timestamp values are asynchronous and NOT synchronized to the specific
    // CPU graph columns, and must not be summed into CPU bar segments.
    let gpu_get = |v: Option<f32>| v.map_or("--".to_owned(), |v| format!("{v:.2}"));
    if let Some([world, depth, hiz, cull, cluster, post, ui, fft, ocean, clouds, bw_clouds]) = gpu {
        text(out, &format!(
            "GPU ASYNC MS   WORLD {}   DEPTH {}   HIZ {}   CULL {}   CLUSTER {}   POST {}   UI {}",
            gpu_get(world), gpu_get(depth), gpu_get(hiz), gpu_get(cull),
            gpu_get(cluster), gpu_get(post), gpu_get(ui),
        ), 8.0, 40.0, 1.25, [0.73, 0.85, 0.98, 0.96], width, height);
        text(out, &format!(
            "GPU OTHER ASYNC MS   FFT {}   OCEAN {}   CLOUDS {}   BW CLOUDS {}",
            gpu_get(fft), gpu_get(ocean), gpu_get(clouds), gpu_get(bw_clouds),
        ), 8.0, 57.0, 1.25, [0.73, 0.85, 0.98, 0.96], width, height);
    } else {
        text(out, "GPU ASYNC MS --   enable r_gpuTimings 1 to measure GPU pass times",
            8.0, 40.0, 1.38, [0.73, 0.85, 0.98, 0.96], width, height);
    }
    if let Some(shader) = summary.shader {
        let [general, special, minimal, shared, generated] = shader.binds;
        text(out, &format!(
            "WORLD SHADER {}/{}   BINDS GEN {} SPEC {} MIN {} SHARED {} GENERATED {}   BATCHES {}   PSOS GEN {} SPEC {}",
            shader.requested, shader.active, general, special, minimal, shared, generated,
            summary.world_draws.unwrap_or_default(),
            summary.pipeline_counts.map_or(0, |v| v.0),
            summary.pipeline_counts.map_or(0, |v| v.1),
        ), 8.0, 75.0, 1.3, LABEL, width, height);
    }
    if let Some([visible, frustum, hiz, pvs, area]) = summary.cull {
        text(out, &format!(
            "CULL ASYNC  VIS {}  FRUSTUM {}  HIZ {}  PVS {}  AREA {}",
            visible, frustum, hiz, pvs, area,
        ), 8.0, 94.0, 1.30, [0.75, 0.86, 0.98, 0.98], width, height);
    }
    if let Some((mode, [vs, clip, fs])) = summary.bsp {
        let screen_pixels = f64::from(width) * f64::from(height);
        text(out, &format!(
            "WGPU30 PRIMARY BSP (ASYNC MODE {})   VS {}   TRI OUT {}   FS {}   FS/SCREEN {:.2}X",
            mode, vs, clip, fs, fs as f64 / screen_pixels.max(1.0),
        ), 8.0, 113.0, 1.35, [0.55, 0.95, 0.80, 1.0], width, height);
    } else {
        text(out, "WGPU30 PRIMARY BSP   PIPELINE STATS -- (UNSUPPORTED OR WAITING FOR QUERY)",
            8.0, 113.0, 1.35, [0.72, 0.82, 0.90, 0.98], width, height);
    }
    text(out, "FS INVOCATIONS INCLUDE OVERDRAW/HELPER LANES; NOT TEXEL READS OR RAM BYTES",
        8.0, 131.0, 1.15, [0.72, 0.82, 0.90, 0.90], width, height);

    if let Some((mode, [total, eligible, global, sky, fusion, ocean, sprite2, rgb3, tcgen, glass, other])) = summary.fallback_map {
        text(out, &format!(
            "MAP BSP MODE {mode} ELIGIBILITY (NOT VISIBLE DRAWS)   OK {eligible}/{total}  GLOBAL {global}  SKY {sky}  FUSED {fusion}  OCEAN-STAGE {ocean}  SPRITE2 {sprite2}  RGB3 {rgb3}  TCGEN {tcgen}  GLASS {glass}  OTHER {other}"
        ), 8.0, 148.0, 1.18, [1.0, 0.82, 0.52, 0.98], width, height);
    }
    if let Some(([vs, tris, fs], passed)) = summary.ocean {
        let ratio = if passed > 0 { format!("{:.2}X", fs as f64 / passed as f64) } else { "--".to_owned() };
        text(out, &format!(
            "OCEAN SURFACE (ASYNC)   VS {vs}   TRI OUT {tris}   FS {fs}   SAMPLES PASSED {passed}   FS/PASSED {ratio}"
        ), 560.0, 168.0, 1.2, [0.55, 0.85, 1.0, 0.98], width, height);
    }
    let legend = ["PREP", "ACQUIRE", "ENCODE", "SUBMIT", "PRESENT", "OTHER"];
    let mut legend_x = 8.0;
    for (name, rgb) in legend.iter().zip(SEGMENT_COLORS) {
        if legend_x + (name.len() as f32 * 9.0 + 26.0) > w { break; }
        rect(out, legend_x, 169.0, 12.0, 12.0,
            [rgb[0], rgb[1], rgb[2], 0.98], width, height);
        text(out, name, legend_x+17.0, 168.0, 1.50,
            LABEL, width, height);
        legend_x += name.len() as f32 * 9.0 + 32.0;
    }

    // Faint horizontal guides using integer FPS labels. Internally the graph
    // remains in frame milliseconds; the labels convert with ms = 1000 / fps.
    let mut last_labeled_y = f32::NEG_INFINITY;
    for &fps in FPS_TICKS {
        let ms = 1000.0 / fps as f32;
        let y = top + ms * PX_PER_MS;
        if y >= h { break; }
        if y - last_labeled_y < MIN_TICK_LABEL_GAP { continue; }
        rect(out, 0.0, y, w, 1.0, [0.35, 0.66, 1.0, 0.16], width, height);
        last_labeled_y = y;
    }

    for (age, (ms, phases)) in recent_to_old.enumerate() {
        if age >= width as usize { break; }
        let x = w - age as f32 - 1.0;
        let visible = (ms.max(0.0)*PX_PER_MS).max(1.0).min((h-top).max(0.0));
        if visible <= 0.0 { continue; }
        let mut y = top;
        for (i, phase) in phases.iter().enumerate() {
            let remaining = (top+visible-y).max(0.0);
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
        rect(out, x, top+visible-2.0_f32.min(visible), 1.0,
            2.0_f32.min(visible), tip, width, height);
    }

    // UI primitives paint in submission order: emit the FPS labels *after* the
    // bars so slow-frame columns never obscure the Y-axis text. Guides remain
    // underneath the bars. The same spacing rule keeps labels paired with
    // their guide lines, without retaining geometry or extra per-frame state.
    let mut last_labeled_y = f32::NEG_INFINITY;
    for &fps in FPS_TICKS {
        let y = top + 1000.0 / fps as f32 * PX_PER_MS;
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
            128, 120, [0.5,0.8,2.0], None, FrameGraphSummary::default());
        assert!(!v.is_empty());
        assert!(v.iter().all(|vertex| vertex.position[1].is_finite()));
        assert!(v.iter().any(|vertex| (vertex.position[1]+1.0).abs()<0.0001));
    }
    #[test]
    fn no_full_screen_tinted_background_rectangles() {
        let mut v = Vec::new();
        build_frame_time_graph_vertices(&mut v,
            [(1.0, [0.1,0.1,0.1,0.1,0.1,0.5])].into_iter(),
            1920, 1080, [0.8,0.9,1.0], None, FrameGraphSummary::default());
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
