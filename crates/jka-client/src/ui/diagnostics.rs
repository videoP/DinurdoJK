//! Diagnostics.
use crate::ui::{rect, rect_outline, text, CullDebugMode, UiSnapshot, UiVertex};

pub(in crate::ui) fn build_reflection_debug_legend(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    w: u32,
    h: u32,
) {
    let x = 14.0;
    let y = 14.0;
    let width = 246.0;
    let height = 112.0;
    rect(out, x, y, width, height, [0.01, 0.015, 0.022, 0.88], w, h);
    text(
        out,
        &format!("REFLECTIONS: {}", ui.video.reflection_quality.label()),
        x + 10.0,
        y + 8.0,
        1.15,
        [0.92, 0.95, 1.0, 1.0],
        w,
        h,
    );
    let rows = [
        ("PLANAR", [1.0, 0.05, 0.85, 1.0]),
        ("SSR HIT", [0.05, 1.0, 0.18, 1.0]),
        ("PROBE / CUBEMAP", [0.06, 0.20, 1.0, 1.0]),
        ("SSR ELIGIBLE / NO HIT", [0.18, 0.18, 0.18, 1.0]),
        ("NO ENHANCED REFLECTION", [0.0, 0.0, 0.0, 1.0]),
    ];
    for (index, (label, color)) in rows.into_iter().enumerate() {
        let row_y = y + 29.0 + index as f32 * 15.0;
        rect(out, x + 10.0, row_y + 1.0, 10.0, 10.0, color, w, h);
        text(
            out,
            label,
            x + 27.0,
            row_y,
            0.92,
            [0.82, 0.88, 0.95, 1.0],
            w,
            h,
        );
    }
}

pub(in crate::ui) fn build_fps_simple(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let label = format!("{:.0} FPS", ui.perf.fps);
    let scale = 1.55;
    let glyph_w = 6.0 * scale;
    let x = (w as f32 - 12.0 - label.chars().count() as f32 * glyph_w).max(12.0);
    // Keep the simple FPS counter readable like the chat text, but with a
    // tight screen-space shadow instead of a resolution-scaled offset.
    text(
        out,
        &label,
        x + 1.0,
        13.0,
        scale,
        [0.0, 0.0, 0.0, 0.72],
        w,
        h,
    );
    text(out, &label, x, 12.0, scale, [1.0; 4], w, h);
}

pub(in crate::ui) fn compact_thread_name(name: &str) -> String {
    let prefixes = [
        ("jka-model-pack-", "PACK"), ("jka-g2-skin-", "G2SKIN"),
        ("jka-fx-geom-", "FXGEO"), ("jka-event-", "EV"),
        ("jka-worker-", "MAP"), ("jka-asset-", "ASSET"),
        ("jka-snow-shell-", "SNOW"), ("jka-rayon-", "RAYON"),
        ("jka-static-ao-ray-", "AO RAY"), ("jka-static-ao-vertex-", "AO VTX"),
    ];
    for (prefix, label) in prefixes {
        if let Some(index) = name.strip_prefix(prefix) { return format!("{label}{index}"); }
    }
    match name {
        "jka-encoder-finish" => "FINISH",
        "jka-thread-monitor" => "CPU MONITOR",
        "jka-fx" => "FX SIM",
        "jka-cloth-physics" => "CLOTH",
        "jka-jiggle-physics" => "JIGGLE",
        "MAIN" => "MAIN",
        "RENDER" => "RENDER",
        "MAP LOADER" => "MAP",
        "WORKER 0" => "MAP0",
        "WORKER 1" => "MAP1",
        "WORKER 2" => "MAP2",
        "WORKER 3" => "MAP3",
        "WORKER 4" => "MAP4",
        "WORKER 5" => "MAP5",
        "WORKER 6" => "MAP6",
        "WORKER 7" => "MAP7",
        "EVENT 0" => "E0",
        "EVENT 1" => "E1",
        "EVENT 2" => "E2",
        "EVENT 3" => "E3",
        "EVENT 4" => "E4",
        "EVENT 5" => "E5",
        "EVENT 6" => "E6",
        "EVENT 7" => "E7",
        "ASSET 0" => "A0",
        "ASSET 1" => "A1",
        _ => name,
    }.to_owned()
}

pub(in crate::ui) fn draw_perf_metric(
    out: &mut Vec<UiVertex>,
    label: &str,
    value: &str,
    x: f32,
    y: f32,
    label_scale: f32,
    value_scale: f32,
    label_color: [f32; 4],
    value_color: [f32; 4],
    w: u32,
    h: u32,
) {
    text(out, label, x, y, label_scale, label_color, w, h);
    text(
        out,
        value,
        x,
        y + 11.0 * label_scale + 4.0,
        value_scale,
        value_color,
        w,
        h,
    );
}

fn perf_text_line(out: &mut Vec<UiVertex>, available: f32, label: &str, x: f32, y: f32,
    scale: f32, color: [f32; 4], w: u32, h: u32) {
    let fitted = scale.min(available / (label.chars().count().max(1) as f32 * 6.0));
    text(out, label, x, y, fitted, color, w, h);
}

pub(in crate::ui) const PERF_HEADER_H: f32 = 38.0;

pub(in crate::ui) const PERF_CPU_H: f32 = 78.0;

pub(in crate::ui) const PERF_CLIENT_H: f32 = 161.0;

pub(in crate::ui) const PERF_INPUT_H: f32 = 108.0;

pub(in crate::ui) const PERF_THREAD_HEADER_H: f32 = 38.0;

pub(in crate::ui) const PERF_THREAD_ROW_H: f32 = 32.0;

fn thread_rows(h: u32, gpu_enabled: bool, debug_culling: bool, thread_count: usize) -> usize {
    let fixed = 12.0 + PERF_HEADER_H + PERF_CPU_H
        + if gpu_enabled { 112.0 } else { 42.0 }
        + PERF_CLIENT_H + PERF_INPUT_H + PERF_THREAD_HEADER_H
        + if debug_culling { 42.0 } else { 0.0 } + 18.0;
    let available = ((h as f32 - 28.0 - fixed) / PERF_THREAD_ROW_H).floor().max(1.0) as usize;
    available.min(11).min(thread_count.div_ceil(2).max(1))
}

fn thread_page(thread_count: usize, rows: usize, requested: u32) -> (usize, usize, std::ops::Range<usize>) {
    let capacity = rows.max(1) * 2;
    let pages = thread_count.div_ceil(capacity).max(1);
    let page = (requested as usize).min(pages - 1);
    let start = (page * capacity).min(thread_count);
    (page, pages, start..(start + capacity).min(thread_count))
}

/// `x, y, width, height` of the `cg_drawFPS 2` profiler panel.
pub(in crate::ui) fn perf_panel_rect(
    w: u32,
    h: u32,
    gpu_enabled: bool,
    debug_culling: bool,
    thread_count: usize,
) -> (f32, f32, f32, f32) {
    let (_, _, width, height) = unscaled_perf_panel_rect(w, h, gpu_enabled, debug_culling, thread_count);
    let scale = ((h as f32 - 28.0).max(1.0) / height)
        .min((w as f32 - 28.0).max(1.0) / width).min(1.0);
    let width = width * scale;
    let height = height * scale;
    ((w as f32 - width - 14.0).max(14.0), 14.0, width, height)
}

fn unscaled_perf_panel_rect(
    w: u32,
    h: u32,
    gpu_enabled: bool,
    debug_culling: bool,
    thread_count: usize,
) -> (f32, f32, f32, f32) {
    let margin = 14.0;
    let panel_w = 900.0_f32.min((w as f32 - margin * 2.0).max(620.0));
    let x = (w as f32 - panel_w - margin).max(margin);
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let thread_rows = thread_rows(h, gpu_enabled, debug_culling, thread_count);
    let footer_h = if debug_culling { 42.0 } else { 0.0 };
    let panel_h = 12.0
        + PERF_HEADER_H
        + PERF_CPU_H
        + gpu_h
        + PERF_CLIENT_H
        + PERF_INPUT_H
        + PERF_THREAD_HEADER_H
        + thread_rows as f32 * PERF_THREAD_ROW_H
        + footer_h
        + 18.0;
    (x, margin, panel_w, panel_h)
}

pub(in crate::ui) fn build_perf(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let start = out.len();
    build_perf_contents(out, ui, w, h);
    let gpu = ui.perf.gpu_ms.is_some();
    let debug = ui.video.cull_debug != CullDebugMode::Off;
    let (rx, ry, rw, _) = unscaled_perf_panel_rect(w, h, gpu, debug, ui.threads.len());
    let (x, y, width, _) = perf_panel_rect(w, h, gpu, debug, ui.threads.len());
    let scale = width / rw;
    if scale < 1.0 {
        // Scale the entire panel uniformly at small resolutions. The HUD editor
        // uses the same rectangle, so its bounds match the actual vertices.
        for vertex in &mut out[start..] {
            let px = (vertex.position[0] + 1.0) * 0.5 * w.max(1) as f32;
            let py = (1.0 - vertex.position[1]) * 0.5 * h.max(1) as f32;
            vertex.position = [
                (x + (px - rx) * scale) / w.max(1) as f32 * 2.0 - 1.0,
                1.0 - (y + (py - ry) * scale) / h.max(1) as f32 * 2.0,
            ];
        }
    }
}

fn build_perf_contents(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let p = ui.perf;
    let debug_culling = ui.video.cull_debug != CullDebugMode::Off;
    let gpu_enabled = p.gpu_ms.is_some();
    let (x, y, panel_w, panel_h) = unscaled_perf_panel_rect(w, h, gpu_enabled, debug_culling, ui.threads.len());
    let header_h = PERF_HEADER_H;
    let cpu_h = PERF_CPU_H;
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let client_h = PERF_CLIENT_H;
    let input_h = PERF_INPUT_H;
    let thread_header_h = PERF_THREAD_HEADER_H;
    let thread_row_h = PERF_THREAD_ROW_H;
    let thread_rows = thread_rows(h, gpu_enabled, debug_culling, ui.threads.len());
    let (page, pages, thread_range) = thread_page(ui.threads.len(), thread_rows, ui.video.perf_thread_page);
    rect(out, x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.76], w, h);

    let section_color = [0.60, 0.74, 0.88, 1.0];
    let body_color = [0.88, 0.93, 0.98, 1.0];
    let muted_color = [0.64, 0.72, 0.80, 1.0];

    text(
        out,
        &format!("{:.0} FPS", p.fps),
        x + 12.0,
        y + 6.0,
        2.15,
        [1.0; 4],
        w,
        h,
    );
    text(
        out,
        &format!("CPU RENDER  {:.3} MS", p.frame_ms),
        x + 176.0,
        y + 10.0,
        1.42,
        body_color,
        w,
        h,
    );

    let cpu_colors = [
        [0.22, 0.58, 0.90, 0.98], // prep
        [0.20, 0.78, 0.82, 0.98], // acquire
        [0.72, 0.42, 0.95, 0.98], // encode
        [0.95, 0.58, 0.20, 0.98], // submit
        [0.94, 0.78, 0.28, 0.98], // present API
        [0.32, 0.36, 0.42, 0.98], // other
    ];
    let wall = p.frame_ms.max(0.000_001);
    let prep = p.cpu_prepare_ms.max(0.0).min(wall);
    let acquire = p.cpu_acquire_ms.max(0.0).min((wall - prep).max(0.0));
    let encode = p
        .cpu_encode_ms
        .max(0.0)
        .min((wall - prep - acquire).max(0.0));
    let submit = p
        .cpu_submit_ms
        .max(0.0)
        .min((wall - prep - acquire - encode).max(0.0));
    let present = p
        .cpu_present_ms
        .max(0.0)
        .min((wall - prep - acquire - encode - submit).max(0.0));
    let other = (wall - prep - acquire - encode - submit - present).max(0.0);
    let cpu_segments = [prep, acquire, encode, submit, present, other];
    let bar_x = x + 12.0;
    let bar_y = y + header_h + 2.0;
    let bar_w = panel_w - 24.0;
    let bar_h = 19.0;
    let mut cursor = bar_x;
    for (value, color) in cpu_segments.into_iter().zip(cpu_colors) {
        let width = ((value / wall) as f32 * bar_w).max(0.0);
        if width > 0.0 {
            rect(out, cursor, bar_y, width, bar_h, color, w, h);
            cursor += width;
        }
    }
    rect_outline(
        out,
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        1.0,
        [0.78, 0.84, 0.90, 0.90],
        w,
        h,
    );

    let cpu_legend = [
        ("PREP", prep, cpu_colors[0]),
        ("ACQUIRE", acquire, cpu_colors[1]),
        ("ENCODE", encode, cpu_colors[2]),
        ("SUBMIT", submit, cpu_colors[3]),
        ("PRESENT", present, cpu_colors[4]),
        ("OTHER", other, cpu_colors[5]),
    ];
    let cpu_col_w = bar_w / cpu_legend.len() as f32;
    for (i, (label, value, color)) in cpu_legend.into_iter().enumerate() {
        let lx = x + 12.0 + i as f32 * cpu_col_w;
        text(out, label, lx, bar_y + 25.0, 1.05, color, w, h);
        text(
            out,
            &format!("{value:.3} MS"),
            lx,
            bar_y + 42.0,
            1.18,
            body_color,
            w,
            h,
        );
    }

    let mut next_y = y + header_h + cpu_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    if let Some(gpu_frame) = p.gpu_ms {
        text(
            out,
            &format!("GPU FRAME  {:.3} MS", gpu_frame),
            x + 12.0,
            next_y + 2.0,
            1.30,
            body_color,
            w,
            h,
        );
        let gpu_values = [
            p.gpu_depth_ms.unwrap_or(0.0),
            p.gpu_hiz_ms.unwrap_or(0.0),
            p.gpu_cull_ms.unwrap_or(0.0),
            p.gpu_cluster_ms.unwrap_or(0.0),
            p.gpu_world_ms.unwrap_or(0.0),
            p.gpu_post_ms.unwrap_or(0.0),
            p.gpu_ui_ms.unwrap_or(0.0),
        ];
        let gpu_colors = [
            [0.38, 0.66, 0.94, 0.98],
            [0.30, 0.82, 0.82, 0.98],
            [0.42, 0.88, 0.56, 0.98],
            [0.78, 0.82, 0.32, 0.98],
            [0.96, 0.62, 0.24, 0.98],
            [0.76, 0.42, 0.92, 0.98],
            [0.96, 0.46, 0.68, 0.98],
            [0.32, 0.36, 0.42, 0.98],
        ];
        let known: f64 = gpu_values.iter().sum();
        let gpu_other = (gpu_frame - known).max(0.0);
        let gpu_bar_y = next_y + 20.0;
        let mut cursor = bar_x;
        for (value, color) in gpu_values
            .into_iter()
            .chain(std::iter::once(gpu_other))
            .zip(gpu_colors)
        {
            let width = ((value / gpu_frame.max(0.000_001)) as f32 * bar_w).max(0.0);
            if width > 0.0 {
                rect(out, cursor, gpu_bar_y, width, bar_h, color, w, h);
                cursor += width;
            }
        }
        rect_outline(
            out,
            bar_x,
            gpu_bar_y,
            bar_w,
            bar_h,
            1.0,
            [0.78, 0.84, 0.90, 0.90],
            w,
            h,
        );
        let labels = [
            "DEPTH", "HIZ", "CULL", "CLUSTER", "WORLD", "POST", "UI", "OTHER",
        ];
        let values = [
            p.gpu_depth_ms.unwrap_or(0.0),
            p.gpu_hiz_ms.unwrap_or(0.0),
            p.gpu_cull_ms.unwrap_or(0.0),
            p.gpu_cluster_ms.unwrap_or(0.0),
            p.gpu_world_ms.unwrap_or(0.0),
            p.gpu_post_ms.unwrap_or(0.0),
            p.gpu_ui_ms.unwrap_or(0.0),
            gpu_other,
        ];
        let gpu_col_w = bar_w / 4.0;
        for row in 0..2 {
            for col in 0..4 {
                let i = row * 4 + col;
                let lx = x + 12.0 + col as f32 * gpu_col_w;
                let ly = gpu_bar_y + 27.0 + row as f32 * 23.0;
                text(out, labels[i], lx, ly, 0.98, gpu_colors[i], w, h);
                text(
                    out,
                    &format!("{:.3} MS", values[i]),
                    lx + 68.0,
                    ly,
                    1.04,
                    body_color,
                    w,
                    h,
                );
            }
        }
    } else {
        text(
            out,
            "GPU TIMESTAMPS OFF  (RENDERING MENU)",
            x + 12.0,
            next_y + 8.0,
            1.08,
            muted_color,
            w,
            h,
        );
    }

    next_y += gpu_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        &format!("CLIENT / CGAME  {:.3} MS", p.client_total_ms),
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );
    let client_metrics = [
        ("SNAPSHOT", p.client_snapshot_ms),
        ("EVENTS", p.client_events_ms),
        ("ENTITIES", p.client_entity_present_ms),
        ("PLAYERS", p.client_player_present_ms),
        ("FOLLOWED", p.client_followed_player_ms),
        ("G2 POSE", p.ghoul2_pose_ms),
        ("G2 SKIN", p.ghoul2_skin_ms),
        ("AUDIO", p.client_audio_ms),
    ];
    let client_col_w = (panel_w - 24.0) / 4.0;
    for (i, (label, value)) in client_metrics.into_iter().enumerate() {
        let row = i / 4;
        let col = i % 4;
        let lx = x + 12.0 + col as f32 * client_col_w;
        let ly = next_y + 25.0 + row as f32 * 32.0;
        text(out, label, lx, ly, 0.94, muted_color, w, h);
        text(
            out,
            &format!("{value:.3} MS"),
            lx,
            ly + 15.0,
            1.05,
            body_color,
            w,
            h,
        );
    }
    perf_text_line(
        out, panel_w - 24.0,
        &format!(
            "EVENT PREP {:.3} MS / {} JOBS / {} POOL THREADS / {}   AUDIO DECODE {:.3} MS / {} JOBS / {}",
            p.client_event_prepare_ms,
            p.client_event_worker_jobs,
            p.client_event_worker_threads,
            if p.client_event_worker_parallel { "WORKERS" } else { "INLINE" },
            p.client_event_sound_decode_ms,
            p.client_event_sound_decode_jobs,
            if p.client_event_sound_decode_parallel { "WORKERS" } else { "INLINE" },
        ),
        x + 12.0,
        next_y + 91.0,
        0.92,
        muted_color,
        w,
        h,
    );

    perf_text_line(
        out, panel_w - 24.0,
        &format!(
            "G2 {} POSES (MOTION {} / {:.3} MS) / {} BOLTS ({:.3} MS) / {} SURF / {} VERTS   CULL {}/{} LOD [{}/{}/{}/{}]   FX {:.3} + TESS {:.3} MS   DYN REPACK {:.3} MS / {} SURF / {} VERTS / {} INDICES",
            p.ghoul2_pose_evals,
            p.ghoul2_motion_pose_evals,
            p.ghoul2_motion_pose_ms,
            p.ghoul2_bolt_queries,
            p.ghoul2_bolt_ms,
            p.ghoul2_surfaces,
            p.ghoul2_vertices,
            p.ghoul2_frustum_culled,
            p.ghoul2_frustum_tests,
            p.ghoul2_lod_counts[0],
            p.ghoul2_lod_counts[1],
            p.ghoul2_lod_counts[2],
            p.ghoul2_lod_counts[3],
            p.client_fx_ms,
            p.client_fx_tessellate_ms,
            p.dynamic_model_prepare_ms,
            p.dynamic_model_surfaces,
            p.dynamic_model_vertices,
            p.dynamic_model_indices,
        ),
        x + 12.0,
        next_y + 111.0,
        0.92,
        muted_color,
        w,
        h,
    );

    perf_text_line(
        out, panel_w - 24.0,
        &format!(
            "FX DRAWS {} [SPR {} ORIENT {} LINE {} QUAD {} MESH {} CYL {}]   OUT {} SURF / CPU {} SURF {} V {} I / GPU {} BATCH {} INST / GPU FX {} MS",
            p.client_fx_draws,
            p.client_fx_sprites,
            p.client_fx_oriented_quads,
            p.client_fx_lines,
            p.client_fx_quads,
            p.client_fx_meshes,
            p.client_fx_cylinders,
            p.client_fx_render_surfaces,
            p.client_fx_cpu_geom_surfaces,
            p.client_fx_cpu_vertices,
            p.client_fx_cpu_indices,
            p.client_fx_gpu_sprite_batches,
            p.client_fx_gpu_sprite_instances,
            p.gpu_fx_sprites_ms.map_or_else(|| "--".to_owned(), |ms| format!("{ms:.3}")),
        ),
        x + 12.0,
        next_y + 128.0,
        0.86,
        muted_color,
        w,
        h,
    );

    next_y += client_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        if ui.video.input_latelatch {
            "INPUT LATENCY (MOUSE)   SUBFRAME + LATE-LATCH"
        } else {
            "INPUT LATENCY (MOUSE)   SUBFRAME EVENT-RATE"
        },
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );

    if let (
        Some(event_to_sim),
        Some(sim_to_render),
        Some(event_to_present_call),
        Some(present_call_max),
    ) = (
        p.input_event_to_sim_ms,
        p.input_sim_to_render_ms,
        p.input_event_to_present_call_ms,
        p.input_event_to_present_call_max_ms,
    ) {
        let metric_y = next_y + 22.0;
        let col_w = (panel_w - 24.0) / 3.0;
        draw_perf_metric(
            out,
            "EVENT>SIM",
            &format!("{event_to_sim:.3} MS"),
            x + 12.0,
            metric_y,
            1.00,
            1.38,
            [0.44, 0.84, 0.98, 1.0],
            body_color,
            w,
            h,
        );
        draw_perf_metric(
            out,
            if ui.video.input_latelatch {
                "SIM>LATCH"
            } else {
                "SIM>RENDER"
            },
            &format!("{sim_to_render:.3} MS"),
            x + 12.0 + col_w,
            metric_y,
            1.00,
            1.38,
            [0.54, 0.90, 0.58, 1.0],
            body_color,
            w,
            h,
        );
        draw_perf_metric(
            out,
            "EVENT>PRESENT CALL",
            &format!("{event_to_present_call:.3} MS"),
            x + 12.0 + col_w * 2.0,
            metric_y,
            1.00,
            1.38,
            [0.66, 0.74, 0.90, 1.0],
            body_color,
            w,
            h,
        );
        text(
            out,
            &if ui.video.input_latelatch {
                format!(
                    "{} SAMPLES / 500 MS   WORST PRESENT CALL {:.3} MS",
                    p.input_latency_samples, present_call_max,
                )
            } else {
                format!(
                    "{} SAMPLES / 500 MS   WORST PRESENT CALL {:.3} MS   CPU PRESENT RETURN, NOT SCANOUT",
                    p.input_latency_samples, present_call_max
                )
            },
            x + 12.0,
            next_y + 69.0,
            0.92,
            muted_color,
            w,
            h,
        );
        if ui.video.input_latelatch {
            text(
                out,
                &format!(
                    "EVENT>LATCH {:.3} MS   LATCH>SUBMIT {:.3} MS   LATCH>PRESENT {:.3} MS   PRESENT=CPU RETURN, NOT SCANOUT",
                    p.input_event_to_latch_ms.unwrap_or(0.0),
                    p.input_latch_to_submit_ms.unwrap_or(0.0),
                    p.input_latch_to_present_call_ms.unwrap_or(0.0),
                ),
                x + 12.0,
                next_y + 84.0,
                0.92,
                muted_color,
                w,
                h,
            );
        }
    } else {
        text(
            out,
            "MOVE THE MOUSE IN-GAME TO SAMPLE LATENCY",
            x + 12.0,
            next_y + 34.0,
            1.18,
            muted_color,
            w,
            h,
        );
    }

    next_y += input_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        &format!("{} (~500 MS)  PAGE {}/{}  {} LIVE THREADS", crate::thread_activity::CPU_SAMPLE_LABEL, page + 1, pages, ui.threads.len()),
        x + 12.0,
        next_y + 1.0,
        1.0,
        section_color,
        w,
        h,
    );
    text(out, &format!("r_perfThreadPage 0..{}  |  {}", pages - 1,
        if cfg!(windows) { "100% = ONE CORE; WAITS EXCLUDED" } else { "SCOPED WALL TIME; INCLUDES WAITS" }),
        x + 12.0, next_y + 17.0, 0.9, muted_color, w, h);
    let thread_column_w = (panel_w - 24.0) * 0.5;
    for (index, thread) in ui.threads[thread_range].iter().enumerate() {
        let (column, row) = (index / thread_rows, index % thread_rows);
        let column_x = x + 12.0 + column as f32 * thread_column_w;
        let thread_bar_x = column_x;
        let thread_bar_w = (thread_column_w * 0.32).min(140.0);
        let ty = next_y + thread_header_h + row as f32 * thread_row_h;
        let name = compact_thread_name(&thread.name);
        let max_name = ((thread_column_w - 72.0) / 6.0).max(8.0) as usize;
        let name: String = name.chars().take(max_name).collect();
        let (bar_color, task_color) = if thread.active {
            ([0.34, 0.94, 0.48, 1.0], [0.34, 0.94, 0.48, 1.0])
        } else if matches!(thread.task, "IDLE" | "WAIT / IDLE") {
            ([0.34, 0.39, 0.44, 1.0], [0.54, 0.60, 0.66, 1.0])
        } else {
            ([0.40, 0.68, 0.92, 1.0], [0.70, 0.84, 0.98, 1.0])
        };

        text(out, &name, column_x, ty, 1.0, body_color, w, h);
        rect(
            out,
            thread_bar_x,
            ty + 14.0,
            thread_bar_w,
            12.0,
            [0.10, 0.12, 0.14, 0.95],
            w,
            h,
        );
        let fill = thread_bar_w * (thread.busy_percent / 100.0).clamp(0.0, 1.0);
        if fill > 0.0 {
            rect(out, thread_bar_x, ty + 14.0, fill, 12.0, bar_color, w, h);
        }
        rect_outline(
            out,
            thread_bar_x,
            ty + 14.0,
            thread_bar_w,
            12.0,
            1.0,
            [0.42, 0.48, 0.54, 0.9],
            w,
            h,
        );
        text(
            out,
            &format!("{:>5.1}%", thread.busy_percent),
            column_x + thread_column_w - 70.0,
            ty,
            1.0,
            body_color,
            w,
            h,
        );
        text(
            out,
            &format!("{}  T{}", thread.task, thread.id),
            thread_bar_x + thread_bar_w + 8.0,
            ty + 15.0,
            0.85,
            task_color,
            w,
            h,
        );
    }

    if debug_culling {
        let cy = next_y + thread_header_h + thread_rows as f32 * thread_row_h + 4.0;
        rect(
            out,
            x + 10.0,
            cy - 5.0,
            panel_w - 20.0,
            1.0,
            [1.0, 1.0, 1.0, 0.08],
            w,
            h,
        );
        text(
            out,
            &format!(
                "CULL VISIBLE {}  FRUSTUM {}  HI-Z {}  PVS {}  AREA {}",
                p.cull_visible,
                p.cull_frustum_rejected,
                p.cull_hiz_rejected,
                p.cull_pvs_rejected,
                p.cull_area_rejected,
            ),
            x + 12.0,
            cy + 2.0,
            1.10,
            [0.94, 0.90, 0.78, 1.0],
            w,
            h,
        );
        text(
            out,
            match ui.video.cull_debug {
                CullDebugMode::Counters => "CULL COUNTERS ONLY: NO XRAY OVERLAY",
                CullDebugMode::HiZOnly => "DOTS: HI-Z MAGENTA | ALL REASONS COUNTED",
                _ => "DOTS: FRUSTUM RED HI-Z MAGENTA PVS CYAN AREA YELLOW",
            },
            x + 12.0,
            cy + 20.0,
            1.02,
            [0.78, 0.86, 0.94, 1.0],
            w,
            h,
        );
    }
}

#[cfg(test)]
mod thread_hud_tests {
    use super::*;

    #[test]
    fn every_live_thread_is_reachable_once_at_multiple_resolutions() {
        for height in [480, 720, 800, 1080, 1440] {
            for gpu in [false, true] {
                for count in [0, 1, 22, 63, 128, 257] {
                    let rows = thread_rows(height, gpu, false, count);
                    let (_, pages, _) = thread_page(count, rows, 0);
                    let visited: Vec<_> = (0..pages).flat_map(|p| thread_page(count, rows, p as u32).2).collect();
                    assert_eq!(visited, (0..count).collect::<Vec<_>>());
                    let (page, _, range) = thread_page(count, rows, u32::MAX);
                    assert_eq!(page, pages - 1);
                    assert!(range.end <= count);
                }
            }
        }
    }

    #[test]
    fn panel_and_worker_rows_fit_viewport_with_gpu_timestamps_and_culling() {
        let mut ui = UiSnapshot::default();
        ui.threads = (0..63).map(|id| crate::ui::ThreadPerfStats {
            id, name: "jka-model-pack-0".into(), task: "MODEL PACK", active: true, busy_percent: 43.0,
        }).collect();
        ui.perf.gpu_ms = Some(1.0);
        for (width, height) in [(640, 480), (1280, 720), (1920, 1080)] {
          for debug in [false, true] {
            ui.video.cull_debug = if debug { CullDebugMode::RejectionReasons } else { CullDebugMode::Off };
            let (x, y, w, h) = perf_panel_rect(width, height, true, debug, 63);
            assert!(x >= 0.0 && y >= 0.0 && x + w <= width as f32 && y + h <= height as f32);
            let mut vertices = Vec::new();
            build_perf(&mut vertices, &ui, width, height);
            let outside: Vec<_> = vertices.iter().filter(|v| v.position.iter().any(|p| !(-1.001..=1.001).contains(p))).take(8).map(|v| v.position).collect();
            assert!(outside.is_empty(), "{width}x{height}: {outside:?}");
          }
        }
    }

    #[test]
    fn distinct_worker_pools_have_distinct_labels() {
        assert_eq!(compact_thread_name("jka-encoder-finish"), "FINISH");
        assert_eq!(compact_thread_name("jka-model-pack-1"), "PACK1");
        assert_eq!(compact_thread_name("jka-g2-skin-1"), "G2SKIN1");
        assert_eq!(compact_thread_name("jka-fx-geom-1"), "FXGEO1");
        assert_eq!(compact_thread_name("WORKER 1"), "MAP1");
    }
}
