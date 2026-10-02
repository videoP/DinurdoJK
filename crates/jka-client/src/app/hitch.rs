//! Live "hitch" recorder.
//!
//! A `.dm_26` demo only holds what the server sent (snapshots), and demo playback
//! never runs prediction, so it cannot show a client-side prediction/ground-trace
//! hitch. This keeps a rolling window of per-frame client state while connected,
//! flags frames that look wrong on their own, and writes the window out as a CSV
//! (`<game>/hitch/hitch-*.csv`) when `hitchmark` is run (bind it to a key and
//! tap it right after feeling a hitch).
//!
//! Rows are app ticks, not presented frames. A JSON sidecar records the server
//! identity, raw public configstrings, and column semantics. Live cvars belong
//! to each CSV row so changing a setting during the ring cannot erase its history.
//! Columns are documented in [`HEADER`]. Everything is read-only diagnostics:
//! nothing here feeds back into Pmove, the view or the usercmds.

use super::*;
use crate::cgame::player_presenter::ViewerAnimDebug;
use std::{collections::VecDeque, fmt::Write as _};

/// Seconds of history kept for a dump.
const WINDOW_MS: f64 = 5_000.0;
/// Hard cap so very high frame rates cannot grow the ring without bound.
const MAX_FRAMES: usize = 16_384;
/// Console lines for flagged frames are limited to this many per second.
const CONSOLE_LINES_PER_SECOND: f64 = 3.0;
const ENTITYNUM_NONE: i32 = 1023;
/// Origin residual (`eye_resid` column), in JKA units, above which a frame is flagged `EYE`.
const EYE_THRESHOLD_UNITS: f32 = 2.0;
/// Prediction correction, in JKA units, above which a frame is flagged `MISS`.
const MISS_FLAG_UNITS: f32 = 0.5;
/// Model draw origin vs predicted origin, in JKA units, above which a frame is flagged `MODEL`.
const MODEL_OFFSET_UNITS: f32 = 1.5;
/// Foot-bolt movement in one tick of the same animation, in units, above which a frame is flagged `POSE`.
const POSE_JUMP_UNITS: f32 = 3.0;
/// Posed bone (hand/head/chest/foot) distance from its extrapolated path, in model-space
/// units, above which a tick is flagged `BONE`.
const BONE_POP_UNITS: f32 = 4.0;
/// Facing change of the drawn model in one tick, in degrees, above which a tick is flagged `TURN`.
const TURN_POP_DEGREES: f32 = 20.0;

const FLAG_EYE: u16 = 1 << 0;
const FLAG_DT: u16 = 1 << 1;
const FLAG_MISS: u16 = 1 << 2;
const FLAG_GROUND_FLICKER: u16 = 1 << 3;
const FLAG_SNAP_GAP: u16 = 1 << 5;
const FLAG_SERVER_TIME: u16 = 1 << 6;
const FLAG_ANIM_FLICKER: u16 = 1 << 7;
const FLAG_ANIM_JUMP: u16 = 1 << 8;
const FLAG_NO_DRAW: u16 = 1 << 9;
const FLAG_MODEL_OFFSET: u16 = 1 << 10;
const FLAG_POSE_JUMP: u16 = 1 << 11;
const FLAG_MODEL_SWAP: u16 = 1 << 12;
const FLAG_BONE_POP: u16 = 1 << 13;
const FLAG_TURN_POP: u16 = 1 << 14;
const FLAG_MARK: u16 = 1 << 15;

const FLAG_NAMES: [(u16, &str); 15] = [
    (FLAG_EYE, "EYE"),
    (FLAG_DT, "DT"),
    (FLAG_MISS, "MISS"),
    (FLAG_GROUND_FLICKER, "GFLICK"),
    (FLAG_SNAP_GAP, "SNAPGAP"),
    (FLAG_SERVER_TIME, "STIME"),
    (FLAG_ANIM_FLICKER, "AFLICK"),
    (FLAG_ANIM_JUMP, "AJUMP"),
    (FLAG_NO_DRAW, "NODRAW"),
    (FLAG_MODEL_OFFSET, "MODEL"),
    (FLAG_POSE_JUMP, "POSE"),
    (FLAG_MODEL_SWAP, "MODELSWAP"),
    (FLAG_BONE_POP, "BONE"),
    (FLAG_TURN_POP, "TURN"),
    (FLAG_MARK, "MARK"),
];

const HEADER: &str = "t_ms,dt_ms,flags,server_time,st_delta,ping,cmd_no,cmd_age,cmd_buttons,cmd_fwd,cmd_right,cmd_up,\
prov_buttons,prov_up,\
cam_x,cam_y,cam_z,eye_resid,\
disp_x,disp_y,disp_z,disp_vx,disp_vy,disp_vz,disp_ground,disp_pm_flags,disp_pm_type,disp_legs,\
comm_x,comm_y,comm_z,comm_ground,\
verr_x,verr_y,verr_z,\
gt_frac,gt_ent,gt_nz,gt_hitz,gp_frac,gp_ent,gp_nz,\
miss_seq,miss_len,\
snap_msg,snap_time,snap_gap_ms,snap_cmdtime,snap_x,snap_y,snap_z,snap_ground,\
disp_torso,disp_inair,snap_legs,snap_torso,\
anim_legs_in,anim_legs,anim_legs_frame,anim_legs_old,anim_legs_bl,\
anim_torso_in,anim_torso,anim_torso_frame,anim_torso_old,anim_torso_bl,\
pose_time,call_time,submit_geo,surfaces,ent_x,ent_y,ent_z,ent_pitch,ent_yaw,ent_roll,\
footl_x,footl_y,footl_z,footr_x,footr_y,footr_z,model_hash,model_settled,\
rhand_x,rhand_y,rhand_z,lhand_x,lhand_y,lhand_z,head_x,head_y,head_z,chest_x,chest_y,chest_z,\
rend_x,rend_y,rend_z,rend_yaw,\
snap_vx,snap_vy,snap_vz,comm_vx,comm_vy,comm_vz,\
cl_maxpackets,cl_commandRate,cl_commandPacing,cl_packetdup,com_maxfps,cl_timeNudge,cg_nopredict,cg_errorDecay,rate,snaps,cg_groundTraceDebug,cg_thirdPerson,cg_thirdPersonCameraDamp,cg_thirdPersonTargetDamp,\
cmd_time,cmd_pitch,cmd_yaw,cmd_roll,prov_time,prov_pitch,prov_yaw,prov_roll,\
pred_valid,pred_time,pred_backend,pred_pmove_fixed,pred_pmove_float,pred_pmove_msec,pred_stepslide,pred_tracemask,pred_jcinfo,\
base_msg,base_time,base_cmdtime,base_gravity,base_speed,base_clientnum,base_eflags,base_pm_flags,base_pm_time,\
disp_cmdtime,disp_gravity,disp_speed,disp_pm_time,disp_pitch,disp_yaw,disp_roll,comm_cmdtime,comm_pm_time,\
snap_gravity,snap_speed,snap_pm_flags,snap_pm_time,snap_pm_type,snap_clientnum,snap_eflags,snap_pitch,snap_yaw,snap_roll,\
tx_packets,tx_empty_packets,tx_cmd_transmissions,tx_last_seq,tx_last_realtime,tx_last_cmd_count,tx_first_cmdtime,tx_last_cmdtime,udp_sent,udp_send_errors,rx_msg,rx_dropped,server_id,\
miss_cmdtime,miss_dx,miss_dy,miss_dz,miss_pred_pm_flags,miss_replay_pm_flags,miss_pred_pm_time,miss_replay_pm_time,miss_pred_vx,miss_pred_vy,miss_pred_vz,miss_replay_vx,miss_replay_vy,miss_replay_vz,miss_pred_yaw,miss_replay_yaw\n";

#[derive(Debug, Clone, Copy)]
struct HitchMiss {
    sequence: u64,
    length: f32,
    command_time: i32,
    delta: [f32; 3],
    predicted: crate::net::PredictionStateDebug,
    replayed: crate::net::PredictionStateDebug,
}

/// Sampled from live application state each tick, never from an archived cfg.
#[derive(Debug, Clone, Copy, Default)]
struct HitchSettings {
    max_packets: u32,
    command_rate: u32,
    command_pacing: bool,
    packet_dup: u32,
    fps_cap: u32,
    time_nudge: i32,
    no_predict: bool,
    error_decay: f32,
    rate: u32,
    snaps: u32,
    ground_trace_debug: u8,
    third_person: bool,
    camera_damp: f32,
    target_damp: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct HitchFrame {
    t_ms: f64,
    dt_ms: f32,
    flags: u16,
    server_time: i32,
    server_time_delta: i32,
    ping: i32,
    cmd_number: i32,
    /// cl.serverTime minus the newest committed usercmd's time: how far the
    /// provisional step has to extrapolate.
    cmd_age: i32,
    cmd_buttons: i32,
    cmd_time: i32,
    cmd_angles: [i32; 3],
    cmd_move: [i8; 3],
    prov_buttons: i32,
    prov_up: i8,
    prov_time: i32,
    prov_angles: [i32; 3],
    cam: [f32; 3],
    eye_resid: f32,
    display: crate::net::PredictionStateDebug,
    committed_origin: [f32; 3],
    committed_ground: i32,
    view_error: [f32; 3],
    ground_trace: Option<crate::net::PredictionTraceDebug>,
    ground_probe: Option<crate::net::PredictionTraceDebug>,
    miss_sequence: u64,
    miss_length: f32,
    correction: Option<HitchMiss>,
    snap_message: i32,
    snap_time: i32,
    snap_gap_ms: f32,
    snap_command_time: i32,
    snap_origin: [f32; 3],
    snap_ground: i32,
    snap_legs: i32,
    snap_torso: i32,
    snap_velocity: [f32; 3],
    committed_velocity: [f32; 3],
    settings: HitchSettings,
    prediction: Option<crate::net::PredictionFrameDebug>,
    solo_states: Option<(crate::net::PredictionStateDebug, crate::net::PredictionStateDebug)>,
    snap_state: crate::net::PredictionStateDebug,
    packets: jka_protocol::session::PacketDebug,
    transport: crate::net::TransportDebug,
    rx_message: i32,
    rx_dropped: i32,
    server_id: i32,
    anim: ViewerAnimDebug,
}

/// What the app hands the recorder each live client frame.
pub(super) struct HitchSample {
    pub now: Instant,
    pub dt: Duration,
    pub server_time: i32,
    pub ping: i32,
    pub cmd: Option<jka_protocol::netchan::UserCmd>,
    pub cmd_number: i32,
    pub provisional: Option<jka_protocol::netchan::UserCmd>,
    /// Final rendered eye position, JKA coordinates.
    pub camera: [f32; 3],
    pub debug: Option<crate::net::PredictionFrameDebug>,
    /// Solo presentation state and newest authoritative local snapshot.
    solo_states: Option<(crate::net::PredictionStateDebug, crate::net::PredictionStateDebug)>,
    miss: Option<HitchMiss>,
    pub snap: Option<SnapSample>,
    pub anim: Option<ViewerAnimDebug>,
    settings: HitchSettings,
    packets: jka_protocol::session::PacketDebug,
    transport: crate::net::TransportDebug,
    rx_message: i32,
    rx_dropped: i32,
    server_id: i32,
}

pub(super) struct SnapSample {
    pub message_num: i32,
    pub server_time: i32,
    pub command_time: i32,
    pub origin: [f32; 3],
    pub ground: i32,
    pub legs_anim: i32,
    pub torso_anim: i32,
    pub velocity: [f32; 3],
    pub state: crate::net::PredictionStateDebug,
}

pub(super) struct HitchRecorder {
    started: Instant,
    connection_key: Option<(Instant, i32)>,
    recording_connection: Option<serde_json::Value>,
    frames: VecDeque<HitchFrame>,
    last_miss_sequence: u64,
    last_snap_message: i32,
    last_snap_arrival_ms: f64,
    snap_gaps: [f32; 16],
    snap_gap_count: usize,
    last_console_ms: f64,
    suppressed: u32,
    dumps: u32,
}

impl Default for HitchRecorder {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            connection_key: None,
            recording_connection: None,
            frames: VecDeque::new(),
            last_miss_sequence: 0,
            last_snap_message: -1,
            last_snap_arrival_ms: 0.0,
            snap_gaps: [0.0; 16],
            snap_gap_count: 0,
            last_console_ms: f64::NEG_INFINITY,
            suppressed: 0,
            dumps: 0,
        }
    }
}

fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = [0.0_f32; 32];
    let count = values.len().min(sorted.len());
    sorted[..count].copy_from_slice(&values[..count]);
    sorted[..count].sort_unstable_by(f32::total_cmp);
    sorted[count / 2]
}

impl HitchRecorder {
    pub(super) fn clear(&mut self) {
        self.frames.clear();
        self.connection_key = None;
        self.recording_connection = None;
        self.last_miss_sequence = 0;
        self.last_snap_message = -1;
        self.last_snap_arrival_ms = 0.0;
        self.snap_gap_count = 0;
        self.last_console_ms = f64::NEG_INFINITY;
        self.suppressed = 0;
    }

    pub(super) fn len(&self) -> usize {
        self.frames.len()
    }

    /// Record one frame. Returns a console line when the frame was flagged and
    /// the rate limit allows saying so.
    pub(super) fn push(&mut self, sample: HitchSample) -> Option<String> {
        // A reconnect or map restart must not label old-session ticks with the
        // new connection's metadata or compare reused miss/command sequences.
        if self.frames.back().is_some_and(|previous| {
            previous.server_id != sample.server_id || sample.cmd_number < previous.cmd_number
        }) {
            self.clear();
        }
        let t_ms = sample.now.saturating_duration_since(self.started).as_secs_f64() * 1000.0;
        let dt_ms = sample.dt.as_secs_f32() * 1000.0;
        let mut frame = HitchFrame {
            t_ms,
            dt_ms,
            server_time: sample.server_time,
            ping: sample.ping,
            cmd_number: sample.cmd_number,
            settings: sample.settings,
            packets: sample.packets,
            transport: sample.transport,
            rx_message: sample.rx_message,
            rx_dropped: sample.rx_dropped,
            server_id: sample.server_id,
            cam: sample.camera,
            ..HitchFrame::default()
        };
        if let Some(cmd) = sample.cmd {
            frame.cmd_time = cmd.server_time;
            frame.cmd_angles = cmd.angles;
            frame.cmd_age = sample.server_time - cmd.server_time;
            frame.cmd_buttons = cmd.buttons;
            frame.cmd_move = [cmd.forward_move, cmd.right_move, cmd.up_move];
        }
        if let Some(provisional) = sample.provisional {
            frame.prov_time = provisional.server_time;
            frame.prov_angles = provisional.angles;
            frame.prov_buttons = provisional.buttons;
            frame.prov_up = provisional.up_move;
        }
        // The predictor keeps its last frame when it was not run this frame; the
        // server_time stamp tells the two apart in the CSV (see `disp_*`).
        if let Some(debug) = sample.debug {
            frame.prediction = Some(debug);
            frame.display = debug.display;
            frame.committed_origin = debug.committed.origin;
            frame.committed_ground = debug.committed.ground_entity;
            frame.committed_velocity = debug.committed.velocity;
            frame.view_error = debug.view_error;
            frame.ground_trace = debug.ground_trace;
            frame.ground_probe = debug.ground_probe;
        } else if let Some((display, committed)) = sample.solo_states {
            frame.solo_states = sample.solo_states;
            frame.display = display;
            frame.committed_origin = committed.origin;
            frame.committed_ground = committed.ground_entity;
            frame.committed_velocity = committed.velocity;
            frame.cmd_time = committed.command_time;
            frame.cmd_age = sample.server_time - committed.command_time;
        } else {
            frame.display.ground_entity = ENTITYNUM_NONE;
            frame.committed_ground = ENTITYNUM_NONE;
        }
        if let Some(miss) = sample.miss {
            if miss.sequence != self.last_miss_sequence {
                self.last_miss_sequence = miss.sequence;
                frame.miss_sequence = miss.sequence;
                frame.miss_length = miss.length;
                frame.correction = Some(miss);
            }
        }
        if let Some(snap) = &sample.snap {
            frame.snap_message = snap.message_num;
            frame.snap_time = snap.server_time;
            frame.snap_command_time = snap.command_time;
            frame.snap_origin = snap.origin;
            frame.snap_ground = snap.ground;
            frame.snap_legs = snap.legs_anim;
            frame.snap_torso = snap.torso_anim;
            frame.snap_velocity = snap.velocity;
            frame.snap_state = snap.state;
        }
        if let Some(anim) = sample.anim {
            frame.anim = anim;
        }

        let mut flags = 0u16;
        let previous = self.frames.back().copied();

        if let Some(previous) = previous {
            frame.server_time_delta = frame.server_time - previous.server_time;
            // cl.serverTime should advance by about the real frame time. Backwards or
            // a jump means the displayed prediction time itself hitched.
            if frame.server_time_delta < 0 || (frame.server_time_delta as f32 - dt_ms).abs() > 4.0 + dt_ms * 0.5 {
                flags |= FLAG_SERVER_TIME;
            }
            // Path jerk: how far this frame's display origin is from where the previous
            // step's velocity carried it. Steps are measured in server time (the
            // display advances in whole ms, 0/1/2 per tick), not wall time.
            if self.frames.len() >= 2 {
                let before = self.frames[self.frames.len() - 2];
                let previous_step = (previous.server_time - before.server_time) as f32;
                let step = frame.server_time_delta as f32;
                if previous_step > 0.0 && step > 0.0 && t_ms - before.t_ms < 100.0 {
                    let scale = step / previous_step;
                    // The player's own path (display origin), not `cam`: in third person
                    // the camera also orbits with the mouse, which is not a hitch.
                    let resid: [f32; 3] = std::array::from_fn(|axis| {
                        let velocity = previous.display.origin[axis] - before.display.origin[axis];
                        frame.display.origin[axis] - previous.display.origin[axis] - velocity * scale
                    });
                    frame.eye_resid = resid.iter().map(|v| v * v).sum::<f32>().sqrt();
                    if frame.eye_resid > EYE_THRESHOLD_UNITS {
                        flags |= FLAG_EYE;
                    }
                }
            }
        }

        // Frame-time spike against the recent median.
        let mut recent = [0.0_f32; 32];
        let mut recent_count = 0;
        for (slot, f) in recent.iter_mut().zip(self.frames.iter().rev()) {
            *slot = f.dt_ms;
            recent_count += 1;
        }
        let typical = median(&recent[..recent_count]);
        if typical > 0.0 && dt_ms > typical * 2.5 && dt_ms > 12.0 {
            flags |= FLAG_DT;
        }

        if frame.miss_sequence != 0 && frame.miss_length >= MISS_FLAG_UNITS {
            flags |= FLAG_MISS;
        }

        // The provisional step runs ahead of the committed state, so display and
        // committed ground legitimately differ around takeoff/landing; both are
        // in the CSV (`disp_ground`, `comm_ground`) but not flagged.
        let grounded = |entity: i32| entity != ENTITYNUM_NONE && entity >= 0;
        // A ground-state flip that undoes itself within two frames / 16 ms.
        let state_of = |f: &HitchFrame| grounded(f.display.ground_entity);
        let n = self.frames.len();
        if sample.debug.is_some() || sample.solo_states.is_some() {
            let now_state = grounded(frame.display.ground_entity);
            for span in 1..=2usize {
                if n < span + 1 {
                    break;
                }
                let flipped = (1..=span).all(|back| state_of(&self.frames[n - back]) != now_state);
                if flipped
                    && state_of(&self.frames[n - span - 1]) == now_state
                    && t_ms - self.frames[n - span].t_ms <= 16.0
                {
                    flags |= FLAG_GROUND_FLICKER;
                    break;
                }
            }
        }

        // Model animation: the played legs animation switching away and back within
        // a few ticks, or the legs frame skipping ahead inside one animation.
        if sample.anim.is_some() && n >= 4 {
            let played = |back: usize| self.frames[n - back].anim.legs_anim;
            let now_anim = frame.anim.legs_anim;
            if played(1) != now_anim
                && (2..=4).any(|back| played(back) == now_anim && (1..back).all(|mid| played(mid) != now_anim))
            {
                flags |= FLAG_ANIM_FLICKER;
            }
            let before = &self.frames[n - 1].anim;
            if before.legs_anim == now_anim && (frame.anim.legs_frame - before.legs_frame).abs() > 3 {
                flags |= FLAG_ANIM_JUMP;
            }
        }

        // The model itself (Ghoul2): not submitted at all, drawn away from the
        // predicted origin, a pose pop between ticks, or a model swap.
        if sample.anim.is_some() {
            let anim = &frame.anim;
            if anim.submit_geometry && anim.surfaces == 0 && anim.call_time != 0 {
                flags |= FLAG_NO_DRAW;
            }
            if anim.pose_time != 0 {
                let away: f32 = (0..3)
                    .map(|axis| (anim.origin[axis] - frame.display.origin[axis]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                if away > MODEL_OFFSET_UNITS {
                    flags |= FLAG_MODEL_OFFSET;
                }
            }
            if let Some(before) = self.frames.back().map(|f| &f.anim).filter(|b| b.pose_time != 0 && anim.pose_time != 0) {
                if before.model_hash != anim.model_hash || before.model_settled != anim.model_settled {
                    flags |= FLAG_MODEL_SWAP;
                } else if before.legs_anim == anim.legs_anim && before.torso_anim == anim.torso_anim && dt_ms < 4.0 {
                    let moved = (0..2)
                        .map(|foot| {
                            (0..3).map(|axis| (anim.feet[foot][axis] - before.feet[foot][axis]).powi(2)).sum::<f32>().sqrt()
                        })
                        .fold(0.0_f32, f32::max);
                    if moved > POSE_JUMP_UNITS {
                        flags |= FLAG_POSE_JUMP;
                    }
                }
            }
        }

        // The skeleton as posed and drawn: any tracked point leaving its extrapolated
        // path, or the drawn facing turning abruptly. Steps are pose-time (ms).
        if sample.anim.is_some() && n >= 2 && frame.anim.pose_time != 0 {
            let (a, b) = (&self.frames[n - 1].anim, &self.frames[n - 2].anim);
            let step = (frame.anim.pose_time - a.pose_time) as f32;
            let previous_step = (a.pose_time - b.pose_time) as f32;
            if a.pose_time != 0 && b.pose_time != 0 && step > 0.0 && previous_step > 0.0 && step < 50.0 {
                let scale = step / previous_step;
                let points = |anim: &ViewerAnimDebug| {
                    [anim.feet[0], anim.feet[1], anim.bones[0], anim.bones[1], anim.bones[2], anim.bones[3]]
                };
                let (now_points, a_points, b_points) = (points(&frame.anim), points(a), points(b));
                let worst = (0..6)
                    .map(|point| {
                        (0..3)
                            .map(|axis| {
                                let velocity = a_points[point][axis] - b_points[point][axis];
                                (now_points[point][axis] - a_points[point][axis] - velocity * scale).powi(2)
                            })
                            .sum::<f32>()
                            .sqrt()
                    })
                    .fold(0.0_f32, f32::max);
                if worst > BONE_POP_UNITS {
                    flags |= FLAG_BONE_POP;
                }
                let mut turn = (frame.anim.render_yaw - a.render_yaw).abs() % 360.0;
                if turn > 180.0 {
                    turn = 360.0 - turn;
                }
                if turn > TURN_POP_DEGREES {
                    flags |= FLAG_TURN_POP;
                }
            }
        }

        // Snapshot arrival cadence, in wall time.
        if let Some(snap) = &sample.snap {
            if snap.message_num != self.last_snap_message {
                if self.last_snap_message >= 0 {
                    let gap = (t_ms - self.last_snap_arrival_ms) as f32;
                    frame.snap_gap_ms = gap;
                    let typical = median(&self.snap_gaps[..self.snap_gap_count.min(self.snap_gaps.len())]);
                    if self.snap_gap_count >= 4 && gap > typical * 2.5 && gap > 70.0 {
                        flags |= FLAG_SNAP_GAP;
                    }
                    self.snap_gaps[self.snap_gap_count % self.snap_gaps.len()] = gap;
                    self.snap_gap_count += 1;
                }
                self.last_snap_message = snap.message_num;
                self.last_snap_arrival_ms = t_ms;
            }
        }

        frame.flags = flags;
        self.frames.push_back(frame);
        while self.frames.len() > MAX_FRAMES
            || self.frames.front().is_some_and(|oldest| t_ms - oldest.t_ms > WINDOW_MS)
        {
            self.frames.pop_front();
        }

        if flags == 0 {
            return None;
        }
        if t_ms - self.last_console_ms < 1000.0 / CONSOLE_LINES_PER_SECOND {
            self.suppressed += 1;
            return None;
        }
        self.last_console_ms = t_ms;
        let suppressed = std::mem::take(&mut self.suppressed);
        Some(format!(
            "^3HITCH^7 t={:.0}ms {} eye={:.2} dt={:.1} miss={:.2} ground disp/comm={}/{} vZ={:.0}{}",
            t_ms,
            flag_text(flags),
            frame.eye_resid,
            dt_ms,
            frame.miss_length,
            frame.display.ground_entity,
            frame.committed_ground,
            frame.display.velocity[2],
            if suppressed > 0 { format!(" (+{suppressed} more)") } else { String::new() },
        ))
    }

    /// Tag the newest frame, so the CSV shows when the key was pressed.
    pub(super) fn mark(&mut self) {
        if let Some(frame) = self.frames.back_mut() {
            frame.flags |= FLAG_MARK;
        }
    }

    pub(super) fn next_dump_number(&mut self) -> u32 {
        self.dumps += 1;
        self.dumps
    }

    /// A copy of the ring for formatting and writing off the main thread: the
    /// CSV is several MB, and formatting it inline stalls the frame it is run on.
    pub(super) fn snapshot(&self) -> HitchSnapshot {
        HitchSnapshot(self.frames.clone())
    }
}

pub(super) struct HitchSnapshot(VecDeque<HitchFrame>);

impl HitchSnapshot {
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn to_csv(&self) -> String {
        let mut out = String::with_capacity(self.0.len() * 400 + HEADER.len());
        out.push_str(HEADER);
        for f in &self.0 {
            let _ = write!(
                out,
                "{:.3},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},\
{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},{},{:#x},{},{},\
{:.3},{:.3},{:.3},{},\
{:.3},{:.3},{:.3},",
                f.t_ms, f.dt_ms, flag_text(f.flags), f.server_time, f.server_time_delta, f.ping,
                f.cmd_number, f.cmd_age, f.cmd_buttons, f.cmd_move[0], f.cmd_move[1], f.cmd_move[2],
                f.prov_buttons, f.prov_up,
                f.cam[0], f.cam[1], f.cam[2], f.eye_resid,
                f.display.origin[0], f.display.origin[1], f.display.origin[2],
                f.display.velocity[0], f.display.velocity[1], f.display.velocity[2],
                f.display.ground_entity, f.display.pm_flags, f.display.pm_type, f.display.legs_anim,
                f.committed_origin[0], f.committed_origin[1], f.committed_origin[2], f.committed_ground,
                f.view_error[0], f.view_error[1], f.view_error[2],
            );
            match f.ground_trace {
                Some(t) => {
                    let _ = write!(out, "{:.4},{},{:.3},{:.2},", t.fraction, t.entity, t.normal[2], t.hit_end[2]);
                }
                None => out.push_str(",,,,"),
            }
            match f.ground_probe {
                Some(t) => {
                    let _ = write!(out, "{:.4},{},{:.3},", t.fraction, t.entity, t.normal[2]);
                }
                None => out.push_str(",,,"),
            }
            let _ = write!(
                out,
                "{},{:.3},{},{},{:.1},{},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{},{},{:.3},{},{},{},{},{:.3},\
{},{},{},{},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},\
{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:#x},{},\
{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},\
{:.3},{:.3},{:.3},{:.2},\
{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},",
                f.miss_sequence, f.miss_length,
                f.snap_message, f.snap_time, f.snap_gap_ms, f.snap_command_time,
                f.snap_origin[0], f.snap_origin[1], f.snap_origin[2], f.snap_ground,
                f.display.torso_anim, f.display.in_air_anim, f.snap_legs, f.snap_torso,
                f.anim.legs_input, f.anim.legs_anim, f.anim.legs_frame, f.anim.legs_old_frame, f.anim.legs_backlerp,
                f.anim.torso_input, f.anim.torso_anim, f.anim.torso_frame, f.anim.torso_old_frame, f.anim.torso_backlerp,
                f.anim.pose_time, f.anim.call_time, u8::from(f.anim.submit_geometry), f.anim.surfaces,
                f.anim.origin[0], f.anim.origin[1], f.anim.origin[2],
                f.anim.angles[0], f.anim.angles[1], f.anim.angles[2],
                f.anim.feet[0][0], f.anim.feet[0][1], f.anim.feet[0][2],
                f.anim.feet[1][0], f.anim.feet[1][1], f.anim.feet[1][2],
                f.anim.model_hash, u8::from(f.anim.model_settled),
                f.anim.bones[0][0], f.anim.bones[0][1], f.anim.bones[0][2],
                f.anim.bones[1][0], f.anim.bones[1][1], f.anim.bones[1][2],
                f.anim.bones[2][0], f.anim.bones[2][1], f.anim.bones[2][2],
                f.anim.bones[3][0], f.anim.bones[3][1], f.anim.bones[3][2],
                f.anim.render_origin[0], f.anim.render_origin[1], f.anim.render_origin[2], f.anim.render_yaw,
                f.snap_velocity[0], f.snap_velocity[1], f.snap_velocity[2],
                f.committed_velocity[0], f.committed_velocity[1], f.committed_velocity[2],
            );
            let s = f.settings;
            let _ = write!(out, "{},{},{},{},{},{},{},{:.3},{},{},{},{},{:.3},{:.3},",
                s.max_packets, s.command_rate, u8::from(s.command_pacing), s.packet_dup,
                s.fps_cap, s.time_nudge, u8::from(s.no_predict), s.error_decay,
                s.rate, s.snaps, s.ground_trace_debug, u8::from(s.third_person), s.camera_damp, s.target_damp);
            let _ = write!(out, "{},{},{},{},{},{},{},{},",
                f.cmd_time, f.cmd_angles[0], f.cmd_angles[1], f.cmd_angles[2],
                f.prov_time, f.prov_angles[0], f.prov_angles[1], f.prov_angles[2]);
            if let Some(p) = f.prediction {
                let _ = write!(out, "1,{},{},{},{},{},{},{:#x},{:#x},",
                    p.server_time, p.settings.server_mod, p.settings.pmove_fixed,
                    p.settings.pmove_float, p.settings.pmove_msec, p.settings.step_slide_fix,
                    p.settings.tracemask, p.settings.jcinfo);
                let _ = write!(out, "{},{},{},{},{:.3},{},{:#x},{:#x},{},",
                    p.base_message, p.base_time, p.base.command_time, p.base.gravity, p.base.speed,
                    p.base.client_num, p.base.e_flags, p.base.pm_flags, p.base.pm_time);
                let _ = write!(out, "{},{},{:.3},{},{:.3},{:.3},{:.3},{},{},",
                    p.display.command_time, p.display.gravity, p.display.speed, p.display.pm_time,
                    p.display.view_angles[0], p.display.view_angles[1], p.display.view_angles[2],
                    p.committed.command_time, p.committed.pm_time);
            } else if let Some((display, committed)) = f.solo_states {
                out.push_str("0,");
                for _ in 0..17 { out.push(','); }
                let _ = write!(out, "{},{},{:.3},{},{:.3},{:.3},{:.3},{},{},",
                    display.command_time, display.gravity, display.speed, display.pm_time,
                    display.view_angles[0], display.view_angles[1], display.view_angles[2],
                    committed.command_time, committed.pm_time);
            } else {
                // Keep absence distinct from actual zero settings/playerstate.
                out.push_str("0,");
                for _ in 0..26 { out.push(','); }
            }
            let ps = f.snap_state;
            let _ = write!(out, "{},{:.3},{:#x},{},{},{},{:#x},{:.3},{:.3},{:.3},",
                ps.gravity, ps.speed, ps.pm_flags, ps.pm_time, ps.pm_type, ps.client_num, ps.e_flags,
                ps.view_angles[0], ps.view_angles[1], ps.view_angles[2]);
            let tx = f.packets;
            let _ = write!(out, "{},{},{},{},{},{},{},{},{},{},{},{},{},",
                tx.packets, tx.empty_packets, tx.command_transmissions, tx.last_sequence,
                tx.last_realtime, tx.last_command_count, tx.first_command_time, tx.last_command_time,
                f.transport.sent_datagrams, f.transport.send_errors, f.rx_message, f.rx_dropped, f.server_id);
            if let Some(m) = f.correction {
                let _ = writeln!(out, "{},{:.3},{:.3},{:.3},{:#x},{:#x},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
                    m.command_time, m.delta[0], m.delta[1], m.delta[2],
                    m.predicted.pm_flags, m.replayed.pm_flags, m.predicted.pm_time, m.replayed.pm_time,
                    m.predicted.velocity[0], m.predicted.velocity[1], m.predicted.velocity[2],
                    m.replayed.velocity[0], m.replayed.velocity[1], m.replayed.velocity[2],
                    m.predicted.view_angles[1], m.replayed.view_angles[1]);
            } else {
                for _ in 0..15 { out.push(','); }
                out.push('\n');
            }
        }
        out
    }
}

fn flag_text(flags: u16) -> String {
    let names: Vec<&str> = FLAG_NAMES
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if names.is_empty() { "-".to_owned() } else { names.join("+") }
}

impl App {
    /// Called once per live client frame, after the camera is final.
    pub(super) fn record_hitch_frame(&mut self, now: Instant, dt: Duration) {
        if self.game_session.as_ref().is_some_and(|session| session.local) {
            self.record_solo_hitch_frame(now, dt);
            return;
        }
        let Some(net) = self.net.as_ref() else { self.hitch.clear(); return };
        if net.state() != jka_protocol::session::ConnectionState::Active {
            self.hitch.clear();
            return;
        }
        let Some(playback) = self.game_session.as_ref().filter(|s| s.live && !s.local) else {
            self.hitch.clear();
            return;
        };
        let session = net.session();
        let cmd_number = session.cmd_number();
        let connection_key = (net.connection_started(), session.server_id());
        if self.hitch.connection_key != Some(connection_key)
            || self.hitch.frames.back().is_some_and(|frame| cmd_number < frame.cmd_number)
        {
            self.hitch.clear();
            self.hitch.connection_key = Some(connection_key);
            let public_info = |index| session.decoder().configstrings.get(&index)
                .map(|bytes| crate::cgame::bytes_to_lossless_ascii(bytes));
            self.hitch.recording_connection = Some(serde_json::json!({
                "address": session.server().to_string(),
                "server_id": session.server_id(),
                "serverinfo": public_info(0),
                "systeminfo": public_info(1),
            }));
        }
        let snap = playback.current_snapshot.as_ref().map(|snapshot| SnapSample {
            message_num: snapshot.message_num,
            server_time: snapshot.server_time,
            command_time: snapshot.player_state.field_i32("commandTime").unwrap_or(0),
            origin: [
                snapshot.player_state.field_f32("origin[0]").unwrap_or(0.0),
                snapshot.player_state.field_f32("origin[1]").unwrap_or(0.0),
                snapshot.player_state.field_f32("origin[2]").unwrap_or(0.0),
            ],
            ground: snapshot.player_state.field_i32("groundEntityNum").unwrap_or(ENTITYNUM_NONE),
            legs_anim: snapshot.player_state.field_i32("legsAnim").unwrap_or(0),
            torso_anim: snapshot.player_state.field_i32("torsoAnim").unwrap_or(0),
            velocity: [
                snapshot.player_state.field_f32("velocity[0]").unwrap_or(0.0),
                snapshot.player_state.field_f32("velocity[1]").unwrap_or(0.0),
                snapshot.player_state.field_f32("velocity[2]").unwrap_or(0.0),
            ],
            state: crate::net::prediction_state_debug(&snapshot.player_state),
        });
        let miss = self.predictor.latest_prediction_miss().map(|miss| HitchMiss {
            sequence: miss.sequence,
            length: miss.length,
            command_time: miss.command_time,
            delta: miss.delta,
            predicted: miss.predicted,
            replayed: miss.server,
        });
        let sample = HitchSample {
            now,
            dt,
            server_time: session.server_time(),
            ping: session.ping(),
            cmd: session.command(cmd_number),
            cmd_number,
            provisional: self.live_provisional,
            camera: crate::scene::jka_position(self.camera.position.to_array()),
            debug: self.predictor.prediction_debug_frame(),
            solo_states: None,
            miss,
            snap,
            anim: playback.player_presenter.viewer_anim_debug(),
            settings: HitchSettings {
                max_packets: self.network.max_packets,
                command_rate: self.network.command_rate,
                command_pacing: self.network.command_pacing,
                packet_dup: self.network.packet_dup,
                fps_cap: self.video.fps_cap,
                time_nudge: self.network.time_nudge,
                no_predict: self.network.no_predict,
                error_decay: self.network.error_decay,
                rate: self.network.rate,
                snaps: self.network.snaps,
                ground_trace_debug: self.network.ground_trace_debug,
                third_person: self.third_person.enabled,
                camera_damp: self.third_person.camera_damp,
                target_damp: self.third_person.target_damp,
            },
            packets: session.packet_debug(),
            transport: net.transport_debug(),
            rx_message: session.server_message_sequence(),
            rx_dropped: session.dropped_packets(),
            server_id: session.server_id(),
        };
        if let Some(line) = self.hitch.push(sample) {
            self.push_console_line(line);
        }
    }

    fn record_solo_hitch_frame(&mut self, now: Instant, dt: Duration) {
        let (Some(server), Some(playback)) = (self.local_server.as_ref(), self.game_session.as_ref()) else {
            self.hitch.clear();
            return;
        };
        let Some(current) = playback.current_snapshot.as_ref() else { return };
        let latest = playback.live_snapshots.back()
            .or(playback.next_snapshot.as_ref()).unwrap_or(current);
        let connection_key = (server.diagnostic_epoch(), 0);
        if self.hitch.connection_key != Some(connection_key) {
            self.hitch.clear();
            self.hitch.connection_key = Some(connection_key);
            self.hitch.recording_connection = Some(serde_json::json!({
                "kind": "solo", "map": server.map_name(), "server_id": 0,
                "physics_msec_at_start": self.video.physics_msec,
            }));
        }
        let committed = crate::net::prediction_state_debug(&latest.player_state);
        let snapshot_state = crate::net::prediction_state_debug(&current.player_state);
        let mut display = snapshot_state;
        // Reuse the interpolated entity supplied to the player presenter.
        if let Some(entity) = playback.audio_followed_entity.as_ref() {
            display.origin = entity.origin;
            display.view_angles = entity.angles;
            display.ground_entity = entity.state.field_i32("groundEntityNum").unwrap_or(display.ground_entity);
            display.legs_anim = entity.state.field_i32("legsAnim").unwrap_or(display.legs_anim);
            display.torso_anim = entity.state.field_i32("torsoAnim").unwrap_or(display.torso_anim);
        }
        let snap = SnapSample {
            message_num: current.message_num, server_time: current.server_time,
            command_time: snapshot_state.command_time, origin: snapshot_state.origin,
            ground: snapshot_state.ground_entity, legs_anim: snapshot_state.legs_anim,
            torso_anim: snapshot_state.torso_anim, velocity: snapshot_state.velocity,
            state: snapshot_state,
        };
        let sample = HitchSample {
            now, dt, server_time: server.presentation_time(), ping: 0,
            cmd: None, cmd_number: latest.message_num, provisional: None,
            camera: crate::scene::jka_position(self.camera.position.to_array()),
            debug: None, solo_states: Some((display, committed)), miss: None,
            snap: Some(snap), anim: playback.player_presenter.viewer_anim_debug(),
            settings: HitchSettings {
                max_packets: self.network.max_packets, command_rate: self.network.command_rate,
                command_pacing: self.network.command_pacing, packet_dup: self.network.packet_dup,
                fps_cap: self.video.fps_cap, time_nudge: self.network.time_nudge,
                no_predict: self.network.no_predict, error_decay: self.network.error_decay,
                rate: self.network.rate, snaps: self.network.snaps,
                ground_trace_debug: self.network.ground_trace_debug,
                third_person: self.third_person.enabled,
                camera_damp: self.third_person.camera_damp, target_damp: self.third_person.target_damp,
            },
            packets: Default::default(), transport: Default::default(),
            rx_message: latest.message_num, rx_dropped: 0, server_id: 0,
        };
        if let Some(line) = self.hitch.push(sample) { self.push_console_line(line); }
    }

    /// `hitchmark [label]`: write the last few seconds of frames to disk.
    pub(super) fn hitch_mark(&mut self, label: Option<&str>) {
        if self.hitch.len() == 0 {
            self.push_console_line(
                "^3hitchmark: nothing recorded (needs SOLO GAME or a live server and cg_hitchrecord 1)".to_owned(),
            );
            return;
        }
        self.hitch.mark();
        let number = self.hitch.next_dump_number();
        let label: String = label
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        let stem = if label.is_empty() {
            format!("hitch-{}-{number}", demo_recording_timestamp())
        } else {
            format!("hitch-{}-{number}-{label}", demo_recording_timestamp())
        };
        let destination = self.game_write_path(PathBuf::from("hitch").join(format!("{stem}.csv")));
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_line(format!("^1hitchmark: couldn't create {}: {error}", parent.display()));
                return;
            }
        }
        // Copy the ring now (cheap), format and write on a worker so the mark
        // does not cost the very frame hitch it is meant to record.
        let snapshot = self.hitch.snapshot();
        let frames = snapshot.len();
        let metadata = serde_json::json!({
            "schema_version": 3,
            "sample_source": "app_tick",
            "client_version": env!("CARGO_PKG_VERSION"),
            "connection_at_recording": self.hitch.recording_connection.clone(),
            "rows": frames,
            "columns": {
                "eye_resid": "Display-origin path residual in JKA units; does not measure the camera.",
                "cmd_pitch_yaw_roll": "Raw usercmd angle shorts; 65536 units per revolution.",
                "pred_valid": "1 when prediction debug state exists; pred_time identifies its tick. Blank prediction fields mean unavailable.",
                "base": "Snapshot actually used to replay commands; can differ from snap_* (current presentation snapshot).",
                "miss_replay": "Replayed state compared against the previous prediction at miss_cmdtime; not the raw snapshot state.",
                "tx": "Packets prepared by the protocol, including repeated commands; does not establish server receipt.",
                "udp_sent": "Datagrams accepted by the local UDP socket, including connectionless messages; does not establish server receipt.",
                "rx_dropped": "Gap before the most recent received netchan message, not a cumulative loss counter.",
                "live_settings": "cl_*, cg_*, com_maxfps, rate and snaps are sampled from live state on every row.",
                "solo": "kind=solo: disp_* discrete fields/velocity come from the current presentation snapshot; origin/angles/ground/anims come from its interpolated player entity. comm_* comes from the newest local snapshot. cmd_no counts local snapshots, cmd_time is the newest physics commandTime. Network cvars do not set Solo physics cadence; prediction/trace fields are unavailable. Rows are app ticks, not GPU presents.",
            },
        });
        let path = destination.clone();
        let spawned = std::thread::Builder::new().name("jka-hitch-write".into()).spawn(move || {
            let result = std::fs::write(&path, snapshot.to_csv()).and_then(|()| {
                let json = serde_json::to_vec_pretty(&metadata).map_err(std::io::Error::other)?;
                std::fs::write(path.with_extension("json"), json)
            });
            let line = match result {
                Ok(()) => format!("^2hitchmark: wrote {frames} frame(s) to {}", path.display()),
                Err(error) => format!("^1hitchmark: couldn't write {}: {error}", path.display()),
            };
            crate::logging::write_line_with_path(
                crate::logging::Level::Info,
                format_args!("{line}"),
                path,
            );
        });
        match spawned {
            Ok(_) => self.push_console_line(format!("^2hitchmark: writing {frames} frame(s) to {}", destination.display())),
            Err(error) => self.push_console_line(format!("^1hitchmark: couldn't start writer: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_live_setting_changes_and_csv_alignment() {
        let first = HitchFrame {
            settings: HitchSettings { max_packets: 125, command_rate: 125, ..Default::default() },
            ..Default::default()
        };
        let second = HitchFrame {
            settings: HitchSettings { max_packets: 1000, command_rate: 60, ..Default::default() },
            prediction: Some(crate::net::PredictionFrameDebug {
                base_message: 42,
                base: crate::net::PredictionStateDebug { gravity: 800, speed: 320.0, ..Default::default() },
                ..Default::default()
            }),
            packets: jka_protocol::session::PacketDebug { packets: 5, empty_packets: 2, ..Default::default() },
            correction: Some(HitchMiss {
                sequence: 7, length: 3.0, command_time: 100, delta: [0.0, 3.0, 0.0],
                predicted: Default::default(), replayed: Default::default(),
            }),
            ..Default::default()
        };
        let third = HitchFrame {
            solo_states: Some((
                crate::net::PredictionStateDebug { command_time: 1000, gravity: 800, ..Default::default() },
                crate::net::PredictionStateDebug { command_time: 1008, ..Default::default() },
            )),
            ..Default::default()
        };
        let csv = HitchSnapshot(VecDeque::from([first, second, third])).to_csv();
        let lines: Vec<_> = csv.lines().collect();
        assert_eq!(lines.len(), 4, "one physical line per app tick");
        let columns: Vec<_> = lines[0].split(',').collect();
        let index = |name| columns.iter().position(|&column| column == name).unwrap();
        let rows: Vec<Vec<_>> = lines[1..].iter().map(|line| line.split(',').collect()).collect();
        for row in &rows { assert_eq!(row.len(), columns.len()); }
        assert_eq!(rows[0][index("cl_maxpackets")], "125");
        assert_eq!(rows[1][index("cl_maxpackets")], "1000");
        assert_eq!(rows[1][index("cl_commandRate")], "60");
        assert_eq!(rows[0][index("pred_valid")], "0");
        assert_eq!(rows[0][index("base_gravity")], "");
        assert_eq!(rows[1][index("base_msg")], "42");
        assert_eq!(rows[1][index("base_gravity")], "800");
        assert_eq!(rows[1][index("base_speed")], "320.000");
        assert_eq!(rows[1][index("tx_packets")], "5");
        assert_eq!(rows[1][index("tx_empty_packets")], "2");
        assert_eq!(rows[0][index("miss_cmdtime")], "");
        assert_eq!(rows[1][index("miss_cmdtime")], "100");
        assert_eq!(rows[1][index("miss_dy")], "3.000");
        assert_eq!(rows[2][index("pred_valid")], "0", "Solo is not online prediction");
        assert_eq!(rows[2][index("base_gravity")], "");
        assert_eq!(rows[2][index("disp_cmdtime")], "1000");
        assert_eq!(rows[2][index("comm_cmdtime")], "1008");
    }

    #[test]
    fn a_new_session_cannot_reuse_old_miss_sequences_or_frames() {
        let mut recorder = HitchRecorder::default();
        recorder.frames.push_back(HitchFrame { server_id: 10, cmd_number: 100, ..Default::default() });
        recorder.last_miss_sequence = 7;
        let sample = HitchSample {
            now: Instant::now(), dt: Duration::from_millis(1), server_time: 5, ping: 100,
            cmd: None, cmd_number: 1, provisional: None, camera: [0.0; 3], debug: None, solo_states: None,
            miss: Some(HitchMiss {
                sequence: 7, length: 3.0, command_time: 5, delta: [0.0, 3.0, 0.0],
                predicted: Default::default(), replayed: Default::default(),
            }),
            snap: None, anim: None, settings: Default::default(), packets: Default::default(),
            transport: Default::default(), rx_message: 1, rx_dropped: 0, server_id: 20,
        };
        recorder.push(sample);
        assert_eq!(recorder.frames.len(), 1);
        assert_eq!(recorder.frames[0].server_id, 20);
        assert_eq!(recorder.frames[0].miss_sequence, 7);
    }
}
