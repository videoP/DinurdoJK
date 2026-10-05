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

pub(in crate::ui) fn compact_thread_name(name: &'static str) -> &'static str {
    match name {
        "MAIN" => "MAIN",
        "RENDER" => "RENDER",
        "MAP LOADER" => "MAP",
        "WORKER 0" => "W0",
        "WORKER 1" => "W1",
        "WORKER 2" => "W2",
        "WORKER 3" => "W3",
        "WORKER 4" => "W4",
        "WORKER 5" => "W5",
        "WORKER 6" => "W6",
        "WORKER 7" => "W7",
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
    }
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

pub(in crate::ui) const PERF_HEADER_H: f32 = 38.0;

pub(in crate::ui) const PERF_CPU_H: f32 = 78.0;

pub(in crate::ui) const PERF_CLIENT_H: f32 = 161.0;

pub(in crate::ui) const PERF_INPUT_H: f32 = 108.0;

pub(in crate::ui) const PERF_THREAD_HEADER_H: f32 = 28.0;

pub(in crate::ui) const PERF_THREAD_ROW_H: f32 = 32.0;

/// `x, y, width, height` of the `cg_drawFPS 2` profiler panel.
pub(in crate::ui) fn perf_panel_rect(
    w: u32,
    gpu_enabled: bool,
    debug_culling: bool,
    thread_count: usize,
) -> (f32, f32, f32, f32) {
    let margin = 14.0;
    let panel_w = 900.0_f32.min((w as f32 - margin * 2.0).max(620.0));
    let x = (w as f32 - panel_w - margin).max(margin);
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let thread_two_columns = thread_count > 11;
    // The original profiler has 11 core/map slots. Keep those together in
    // the left column and put the event-pool and asset-loader slots in the right column.
    // This preserves the old panel height instead of adding eight more rows.
    let thread_rows = if thread_two_columns { 11 } else { thread_count };
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
    let p = ui.perf;
    let debug_culling = ui.video.cull_debug != CullDebugMode::Off;
    let gpu_enabled = p.gpu_ms.is_some();
    let (x, y, panel_w, panel_h) = perf_panel_rect(w, gpu_enabled, debug_culling, ui.threads.len());
    let header_h = PERF_HEADER_H;
    let cpu_h = PERF_CPU_H;
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let client_h = PERF_CLIENT_H;
    let input_h = PERF_INPUT_H;
    let thread_header_h = PERF_THREAD_HEADER_H;
    let thread_row_h = PERF_THREAD_ROW_H;
    let thread_two_columns = ui.threads.len() > 11;
    let thread_rows = if thread_two_columns {
        11
    } else {
        ui.threads.len()
    };
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
    text(
        out,
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

    text(
        out,
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

    text(
        out,
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
        "THREAD UTILIZATION  (~500 MS)",
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );
    let thread_column_w = (panel_w - 24.0) * 0.5;
    for (index, thread) in ui.threads.into_iter().enumerate() {
        let (column, row) = if thread_two_columns && index >= 11 {
            (1, index - 11)
        } else {
            (0, index)
        };
        let column_x = x + 12.0 + column as f32 * thread_column_w;
        let thread_bar_x = if thread_two_columns {
            column_x + 72.0
        } else {
            x + 100.0
        };
        let thread_bar_w = if thread_two_columns { 150.0 } else { 320.0 };
        let ty = next_y + thread_header_h + row as f32 * thread_row_h;
        let name = compact_thread_name(thread.name);
        let (bar_color, task_color) = if thread.active {
            ([0.34, 0.94, 0.48, 1.0], [0.34, 0.94, 0.48, 1.0])
        } else if thread.task == "IDLE" {
            ([0.34, 0.39, 0.44, 1.0], [0.54, 0.60, 0.66, 1.0])
        } else {
            ([0.40, 0.68, 0.92, 1.0], [0.70, 0.84, 0.98, 1.0])
        };

        text(out, name, column_x, ty + 3.0, 1.18, body_color, w, h);
        rect(
            out,
            thread_bar_x,
            ty + 6.0,
            thread_bar_w,
            17.0,
            [0.10, 0.12, 0.14, 0.95],
            w,
            h,
        );
        let fill = thread_bar_w * (thread.busy_percent / 100.0).clamp(0.0, 1.0);
        if fill > 0.0 {
            rect(out, thread_bar_x, ty + 6.0, fill, 17.0, bar_color, w, h);
        }
        rect_outline(
            out,
            thread_bar_x,
            ty + 6.0,
            thread_bar_w,
            17.0,
            1.0,
            [0.42, 0.48, 0.54, 0.9],
            w,
            h,
        );
        text(
            out,
            &format!("{:>5.1}%", thread.busy_percent),
            thread_bar_x + thread_bar_w + 8.0,
            ty + 3.0,
            1.12,
            body_color,
            w,
            h,
        );
        text(
            out,
            thread.task,
            thread_bar_x + thread_bar_w + 64.0,
            ty + 3.0,
            1.04,
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
            "XRAY: FRUSTUM RED  HI-Z MAGENTA  PVS CYAN  AREA YELLOW",
            x + 12.0,
            cy + 20.0,
            1.02,
            [0.78, 0.86, 0.94, 1.0],
            w,
            h,
        );
    }
}
