//! jaPRO `cg_speedometer`, ported from `hud_strafehelper.c`: `DF_SetSpeedometer`,
//! `DF_DrawSpeedometer`, `DF_DrawAccelMeter`, `DF_DrawJumpHeight`,
//! `DF_DrawJumpDistance`, `DF_DrawVerticalSpeed`, `DF_DrawYawSpeed`,
//! `DF_GraphAddSpeed`/`DF_DrawSpeedGraph` and the old lagometer-style speed graph.
//!
//! Like the C (which keeps this state in `static` locals and `cg.*`), the
//! readout is a small state machine stepped once per rendered frame. [`Speedometer::update`]
//! returns the draw list in cgame's 640x480 virtual space; `ui.rs` maps it to pixels.
//!
//! Intentional differences from the C:
//! * `cgs.newHud` / `cg_hudFiles` x offsets are not applied (there are no hud files).
//! * `cgs.widthRatioCoef` is treated as 1, like the race timer next to it.
//! * `CG_Text_Paint` ignores the alignment half of `ITEM_ALIGN_RIGHT | ITEM_TEXTSTYLE_OUTLINED`
//!   (it only switches on the combined value, which lands on a drop-shadow style), so
//!   every string is left-aligned at its x with a drop shadow - as drawn here.
//! * The old speed graph shares `cg_lagometer` / `cg_lagometerX/Y` with the lag graph
//!   (`lagometer.rs`), exactly as `DF_DrawSpeedGraphOld` does.

/// `cg_speedometer` bits (`SPEEDOMETER_*`).
pub mod flag {
    pub const ENABLE: u32 = 1 << 0;
    pub const GROUNDSPEED: u32 = 1 << 1;
    pub const JUMPHEIGHT: u32 = 1 << 2;
    pub const JUMPDISTANCE: u32 = 1 << 3;
    pub const VERTICALSPEED: u32 = 1 << 4;
    pub const YAWSPEED: u32 = 1 << 5;
    pub const ACCELMETER: u32 = 1 << 6;
    pub const SPEEDGRAPH: u32 = 1 << 7;
    pub const KPH: u32 = 1 << 8;
    pub const MPH: u32 = 1 << 9;
    pub const JUMPS: u32 = 1 << 10;
    pub const COLORS: u32 = 1 << 11;
    pub const JUMPSCOLORS1: u32 = 1 << 12;
    pub const JUMPSCOLORS2: u32 = 1 << 13;
    pub const SPEEDGRAPHOLD: u32 = 1 << 14;
    pub const XYZ: u32 = 1 << 15;
}

/// `speedometerSettings[]` in `CG_SpeedometerSettings_f`, indexed by bit number.
pub const TOGGLE_LABELS: [&str; 16] = [
    "Enable speedometer",
    "Pre-speed display",
    "Jump height display",
    "Jump distance display",
    "Vertical speed indicator",
    "Yaw speed indicator",
    "Accel meter",
    "Speed graph",
    "Display speed in kilometers instead of units",
    "Display speed in imperial miles instead of units",
    "Pre-speed jumps array",
    "Disable speedometer colors",
    "Array Colors 1",
    "Array Colors 2",
    "Old Speedgraph",
    "XYZ Speed",
];

/// `CG_SpeedometerSettings_f` toggle of bit `index`: 8/9 and 12/13 behave as
/// radio pairs (turning one on turns its partner off).
pub fn toggle(value: u32, index: usize) -> u32 {
    let mask = (1u32 << TOGGLE_LABELS.len()) - 1;
    let bit = 1u32 << index;
    let group = match index {
        8 | 9 => (1 << 8) | (1 << 9),
        12 | 13 => (1 << 12) | (1 << 13),
        _ => return bit ^ (value & mask),
    };
    (value & !(group & !bit)) ^ bit
}

/// `SPEEDOMETER_NUM_SAMPLES` / `SPEEDOMETER_MIN_RANGE` and the colour ramp of the graph.
const GRAPH_SAMPLES: usize = 500;
const GRAPH_MIN_RANGE: f32 = 900.0;
const SPEED_MED: f32 = 1000.0;
const SPEED_FAST: f32 = 1600.0;
/// `ACCEL_SAMPLES`, `PERCENT_SAMPLES`, `YAW_FRAMES`, `SPEED_SAMPLES`.
const ACCEL_SAMPLES: usize = 16;
const PERCENT_SAMPLES: usize = 16;
const YAW_FRAMES: usize = 16;
const OLD_GRAPH_SAMPLES: usize = 256;
/// `ARRAY_LEN(cg.lastGroundSpeeds)`.
const GROUND_SPEEDS: usize = 512;

/// The user-facing `cg_speedometer*` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub flags: u32,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub jumps: i32,
    pub jumps_x: f32,
    pub jumps_y: f32,
    pub jump_goal: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { flags: 0, x: 132.0, y: 459.0, size: 0.75, jumps: 10, jumps_x: 185.0, jumps_y: 300.0, jump_goal: 0.0 }
    }
}

/// What the C reads from `state`, `cg` and the predicted playerstate each frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    /// `state.cgaz.v` (horizontal speed), `state.cgaz.vxyz` (3D speed), `state.velocity[2]`.
    pub v: f32,
    pub vxyz: f32,
    pub vertical: f32,
    /// `state.cgaz.s`: the wishspeed of the current command.
    pub wishspeed: f32,
    /// `state.speed` (`ps.speed`) and `state.cgaz.frametime`.
    pub player_speed: f32,
    pub cgaz_frametime: f32,
    /// `groundEntityNum != ENTITYNUM_NONE`.
    pub on_ground: bool,
    pub pm_time: i32,
    pub view_yaw: f32,
    pub origin: [f32; 3],
    /// `ps.fd.forceJumpZStart`.
    pub force_jump_z_start: f32,
    /// `cg.time` and `cg.frametime` (ms), `trap->Milliseconds()`.
    pub time_ms: i32,
    pub frame_ms: i32,
    pub real_ms: i64,
    /// `cg_strafeHelper & SHELPER_ACCELMETER` also runs the speedometer block.
    pub strafehelper_accelmeter: bool,
    /// `cg_lagometer`, `cg_lagometerX/Y`: the old speed graph shares them.
    pub lagometer: crate::lagometer::Settings,
    /// `cgs.widthRatioCoef` (the old speed graph scales its x extents by it).
    pub width_ratio_coef: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// Latin-1: each `char` is one font glyph (the acceleration label is `\u{b5}`).
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
    /// `Some(thickness)` for an outline (`CG_DrawRect`), `None` for a fill.
    pub outline: Option<f32>,
}

/// Draw list for one frame, in 640x480 virtual space. The three parts sit in
/// unrelated places, so the HUD editor moves them independently.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ui {
    pub size: f32,
    /// The settings this frame was built with (the HUD editor sizes its outlines from them).
    pub settings: Settings,
    /// The main readout row: speed, pre-speed, jump height/distance, vertical and
    /// yaw speed, plus the accel meter's rectangles.
    pub texts: Vec<Text>,
    pub rects: Vec<Rect>,
    /// The pre-speed jumps array (`cg_speedometerJumpsX/Y`).
    pub jump_texts: Vec<Text>,
    /// The speed graph(s).
    pub graph_rects: Vec<Rect>,
    /// The old speed graph's `gfx/2d/lag` frame and speed readout (`cg_lagometer` 1-3).
    pub graph_pics: Vec<crate::lagometer::Pic>,
    pub graph_texts: Vec<crate::lagometer::Text>,
}

const WHITE: [f32; 4] = [1.0; 4];

/// `S_COLOR_*` prefixes used by the readout.
const GREEN: &str = "^2";
const RED: &str = "^1";
const WHITE_CODE: &str = "^7";

pub struct Speedometer {
    // DF_DrawSpeedometer statics.
    last_speed: f32,
    previous_accels: [f32; ACCEL_SAMPLES],
    accel_index: u32,
    last_update: i64,
    jumps_counter: usize,
    clear_on_next_jump: bool,
    // DF_DrawAccelMeter statics.
    previous_times: [f32; PERCENT_SAMPLES],
    percent_index: u32,
    // DF_DrawYawSpeed statics.
    previous_yaws: [u16; YAW_FRAMES],
    yaw_index: u16,
    yaw_last_update: i64,
    // `cg.*` fields.
    last_ground_speeds: [f32; GROUND_SPEEDS],
    last_ground_speed: f32,
    last_ground_time: i32,
    first_time_in_air: bool,
    last_ground_position: [f32; 3],
    last_jump_height: f32,
    last_jump_height_time: i32,
    last_jump_distance: f32,
    last_jump_distance_time: i32,
    last_yaw: f32,
    previous_speed: f32,
    last_z_speed: f32,
    was_on_ground: bool,
    /// `firstSpeed` (file-global).
    first_speed: f32,
    // Speed graph: `speedSamples`, `oldestSpeedSample`, `maxSpeedSample`.
    graph: [f32; GRAPH_SAMPLES],
    oldest: usize,
    max_index: usize,
    // `speedgraph.frameSamples` / `frameCount`, fed every frame.
    old_graph: [i32; OLD_GRAPH_SAMPLES],
    old_graph_count: u32,
}

impl Default for Speedometer {
    fn default() -> Self {
        Self {
            last_speed: 0.0,
            previous_accels: [0.0; ACCEL_SAMPLES],
            accel_index: 0,
            last_update: 0,
            jumps_counter: 0,
            clear_on_next_jump: false,
            previous_times: [0.0; PERCENT_SAMPLES],
            percent_index: 0,
            previous_yaws: [0; YAW_FRAMES],
            yaw_index: 0,
            yaw_last_update: 0,
            last_ground_speeds: [0.0; GROUND_SPEEDS],
            last_ground_speed: 0.0,
            last_ground_time: 0,
            first_time_in_air: false,
            last_ground_position: [0.0; 3],
            last_jump_height: 0.0,
            last_jump_height_time: 0,
            last_jump_distance: 0.0,
            last_jump_distance_time: 0,
            last_yaw: 0.0,
            previous_speed: 0.0,
            last_z_speed: 0.0,
            was_on_ground: false,
            first_speed: 0.0,
            graph: [0.0; GRAPH_SAMPLES],
            oldest: 0,
            max_index: 0,
            old_graph: [0; OLD_GRAPH_SAMPLES],
            old_graph_count: 0,
        }
    }
}

/// A static sample of every part of the readout for the HUD editor.
pub fn preview(settings: &Settings) -> Ui {
    let mut ui = Ui { size: settings.size, settings: *settings, ..Ui::default() };
    let y = settings.y;
    let mut x = settings.x;
    let text = |ui: &mut Ui, value: &str, x: f32, color: [f32; 4]| {
        ui.texts.push(Text { text: value.to_owned(), x, y: y.trunc(), color });
    };
    text(&mut ui, "^2\u{b5}:", x, WHITE);
    text(&mut ui, "   612", x, [1.0, 0.6, 0.6, 1.0]);
    x += 52.0;
    text(&mut ui, "704", x, [1.0, 0.7, 0.7, 1.0]);
    x += 52.0;
    ui.rects.push(Rect { x: x - 88.0 - 0.75, y: y - 10.75, w: 37.75, h: 13.75, color: [0.0, 0.0, 0.0, 1.0], outline: Some(0.5) });
    ui.rects.push(Rect { x: x - 88.0 + 0.25, y: y - 9.9, w: 24.0, h: 12.0, color: [1.0, 0.0, 0.0, 1.0], outline: None });
    text(&mut ui, "58.3", x, WHITE);
    x += 42.0;
    text(&mut ui, "214.6", x, WHITE);
    x += 62.0;
    text(&mut ui, "270", x, WHITE);
    x += 42.0;
    text(&mut ui, "210", x, WHITE);
    for (index, value) in ["704", "761", "802"].into_iter().enumerate() {
        ui.jump_texts.push(Text {
            text: value.to_owned(),
            x: settings.jumps_x + index as f32 * 52.0,
            y: settings.jumps_y,
            color: WHITE,
        });
    }
    let (gx, gy, gw, gh) = (245.0, 456.0, 150.0, 22.0);
    for i in 1..GRAPH_SAMPLES {
        let fraction = i as f32 / GRAPH_SAMPLES as f32;
        let level = 0.35 + 0.65 * (fraction * std::f32::consts::TAU * 2.0).sin().abs();
        ui.graph_rects.push(Rect {
            x: gx + fraction * gw,
            y: gy + (1.0 - level) * gh,
            w: gw / GRAPH_SAMPLES as f32,
            h: level * gh,
            color: [fraction, 1.0 - fraction, 0.5, 0.8],
            outline: None,
        });
    }
    ui
}

/// `AngleSubtract`.
fn angle_subtract(a1: f32, a2: f32) -> f32 {
    let mut a = a1 - a2;
    while a > 180.0 {
        a -= 360.0;
    }
    while a < -180.0 {
        a += 360.0;
    }
    a
}

/// `1 / ((a / s) * (a / s))`: the brightness the "over the wishspeed" colours fade with.
fn over_ratio(speed: f32, wishspeed: f32) -> f32 {
    let ratio = speed / wishspeed;
    1.0 / (ratio * ratio)
}

impl Speedometer {
    /// Step the readout by one rendered frame. `None` when nothing is enabled.
    pub fn update(&mut self, settings: &Settings, input: &Input) -> Option<Ui> {
        // CG_AddSpeedGraphFrameInfo runs every frame, drawn or not.
        self.old_graph[(self.old_graph_count as usize) & (OLD_GRAPH_SAMPLES - 1)] = input.v as i32;
        self.old_graph_count = self.old_graph_count.wrapping_add(1);

        let flags = settings.flags;
        if flags & flag::ENABLE == 0 && !input.strafehelper_accelmeter {
            return None;
        }

        let mut ui = Ui { size: settings.size, settings: *settings, ..Ui::default() };
        let mut xpos = settings.x;
        let mut jumps_xpos = settings.jumps_x;

        // DF_SetSpeedometer.
        let mut speed = if flags & flag::XYZ != 0 { input.vxyz } else { input.v };
        if flags & flag::KPH != 0 {
            speed *= 0.102_869_996_7;
        } else if flags & flag::MPH != 0 {
            speed *= 0.063_920_432_71;
        } else if flags != 0 {
            speed = (speed + 0.5).floor();
        }

        self.draw_speedometer(settings, input, speed, &mut xpos, &mut jumps_xpos, &mut ui);

        if flags & flag::ACCELMETER != 0 || input.strafehelper_accelmeter {
            self.draw_accel_meter(settings, input, xpos, &mut ui);
        }
        if flags & flag::JUMPHEIGHT != 0 {
            self.draw_jump_height(settings, input, &mut xpos, &mut ui);
        }
        if flags & flag::JUMPDISTANCE != 0 {
            self.draw_jump_distance(settings, input, &mut xpos, &mut ui);
        }
        if flags & flag::VERTICALSPEED != 0 {
            draw_vertical_speed(settings, input, &mut xpos, &mut ui);
        }
        if flags & flag::YAWSPEED != 0 {
            self.draw_yaw_speed(settings, input, &mut xpos, &mut ui);
        }
        if flags & flag::SPEEDGRAPH != 0 {
            self.graph_add_speed(input.vxyz);
            self.draw_speed_graph(&mut ui);
        }
        if flags & flag::SPEEDGRAPHOLD != 0 {
            self.draw_speed_graph_old(&input.lagometer, input.width_ratio_coef, input.v, &mut ui);
        }

        // The end of the C HUD routine: `wasOnGround` and `cg.lastZSpeed`.
        self.was_on_ground = input.on_ground;
        self.last_z_speed = input.vertical;
        Some(ui)
    }

    fn text(ui: &mut Ui, text: String, x: f32, y: f32, color: [f32; 4]) {
        ui.texts.push(Text { text, x, y, color });
    }

    /// `DF_DrawSpeedometer`.
    fn draw_speedometer(
        &mut self,
        settings: &Settings,
        input: &Input,
        speed: f32,
        xpos: &mut f32,
        jumps_xpos: &mut f32,
        ui: &mut Ui,
    ) {
        let flags = settings.flags;
        let s = input.wishspeed;
        let mut color_speed = WHITE;

        let accel = speed - self.last_speed;
        self.last_speed = speed;

        if speed > s && flags & flag::COLORS == 0 {
            let c = over_ratio(speed, s);
            color_speed[1] = c;
            color_speed[2] = c;
        }

        if input.real_ms - self.last_update > 5 {
            // Don't sample faster than this.
            self.last_update = input.real_ms;
            self.previous_accels[(self.accel_index as usize) % ACCEL_SAMPLES] = accel;
            self.accel_index = self.accel_index.wrapping_add(1);
        }
        let mut total: f32 = self.previous_accels.iter().sum();
        if total == 0.0 {
            total = 1.0;
        }
        let avg_accel = total / ACCEL_SAMPLES as f32 - 0.0625;

        let tint = if avg_accel > 0.0 {
            GREEN
        } else if avg_accel < 0.0 {
            RED
        } else {
            WHITE_CODE
        };
        let number = format!("   {speed:.0}");
        let label = if flags & flag::KPH != 0 {
            "k:"
        } else if flags & flag::MPH != 0 {
            "m: "
        } else {
            "\u{b5}:"
        };
        Self::text(ui, format!("{tint}{label}"), *xpos, settings.y, WHITE);
        Self::text(ui, number, *xpos, settings.y, color_speed);
        *xpos += 52.0;

        if flags & flag::GROUNDSPEED != 0 || (flags != 0 && flags & flag::JUMPS != 0) {
            self.draw_ground_speeds(settings, input, speed, xpos, jumps_xpos, ui);
        }
    }

    /// The pre-speed / jumps-array half of `DF_DrawSpeedometer`.
    fn draw_ground_speeds(
        &mut self,
        settings: &Settings,
        input: &Input,
        speed: f32,
        xpos: &mut f32,
        jumps_xpos: &mut f32,
        ui: &mut Ui,
    ) {
        let flags = settings.flags;
        let s = input.wishspeed;
        let mut color_ground_speed = WHITE;
        let mut color_ground_speeds = WHITE;

        let jumps_limit = settings.jumps.clamp(0, GROUND_SPEEDS as i32 - 1) as usize;

        if input.on_ground || input.vertical < 0.0 {
            // On the ground, or moving down.
            self.first_time_in_air = false;
        } else if !self.first_time_in_air {
            // Moving up for the first time.
            self.first_time_in_air = true;
            self.last_ground_speed = speed;
            self.last_ground_time = input.time_ms;
            if flags & flag::JUMPS != 0 {
                if self.clear_on_next_jump {
                    self.last_ground_speeds = [0.0; GROUND_SPEEDS];
                    self.jumps_counter = 0;
                    self.clear_on_next_jump = false;
                }
                if self.jumps_counter < GROUND_SPEEDS {
                    self.last_ground_speeds[self.jumps_counter] = self.last_ground_speed;
                    self.jumps_counter += 1;
                }
            }
        }

        if flags & flag::JUMPS != 0 {
            if (input.on_ground && input.pm_time <= 0 && input.v < s) || input.v == 0.0 {
                self.clear_on_next_jump = true;
            }
            if settings.jumps != 0 && self.jumps_counter < jumps_limit {
                // Still in the first n jumps: print the array.
                for i in 0..=jumps_limit {
                    let value = self.last_ground_speeds[i];
                    let color = over_ratio(value, s);
                    let text = format!("{value:.0}");
                    if flags & flag::JUMPSCOLORS1 != 0 {
                        color_ground_speeds[1] = color;
                        color_ground_speeds[2] = color;
                    } else if flags & flag::JUMPSCOLORS2 != 0 {
                        let gained = if i > 0 {
                            value > self.last_ground_speeds[i - 1]
                        } else {
                            value > self.first_speed
                        };
                        if gained {
                            color_ground_speeds[0] = color;
                            color_ground_speeds[1] = 1.0;
                            color_ground_speeds[2] = color;
                        } else {
                            color_ground_speeds[0] = 1.0;
                            color_ground_speeds[1] = color;
                            color_ground_speeds[2] = color;
                        }
                    }
                    if text != "0" {
                        ui.jump_texts.push(Text { text, x: *jumps_xpos, y: settings.jumps_y, color: color_ground_speeds });
                        *jumps_xpos += 52.0;
                    }
                }
            } else if settings.jumps != 0 && self.jumps_counter >= jumps_limit {
                // Out of the first n jumps: shuffle the array down.
                self.first_speed = self.last_ground_speeds[0];
                for i in 0..jumps_limit {
                    self.last_ground_speeds[i] = self.last_ground_speeds[i + 1];
                }
                self.last_ground_speeds[jumps_limit] = 0.0;
                if self.jumps_counter > 0 {
                    self.jumps_counter -= 1;
                }
            }
        }

        // Note the C precedence: `lastGroundSpeed / s * lastGroundSpeed / s`.
        let ground_color = self.last_ground_speed / s * self.last_ground_speed / s;
        let ground_color = 1.0 / ground_color;
        if settings.jump_goal != 0.0 && settings.jump_goal <= self.last_ground_speed && self.jumps_counter == 1 {
            color_ground_speed[0] = ground_color;
            color_ground_speed[1] = 1.0;
            color_ground_speed[2] = ground_color;
        } else if self.last_ground_speed > s && flags & flag::COLORS == 0 {
            color_ground_speed[0] = 1.0;
            color_ground_speed[1] = ground_color;
            color_ground_speed[2] = ground_color;
        }

        if self.last_ground_time > input.time_ms - 1500
            && flags & flag::GROUNDSPEED != 0
            && self.last_ground_speed != 0.0
        {
            Self::text(ui, format!("{:.0}", self.last_ground_speed), *xpos, settings.y, color_ground_speed);
        }
        *xpos += 52.0;
    }

    /// `DF_DrawAccelMeter`.
    fn draw_accel_meter(&mut self, settings: &Settings, input: &Input, xpos: f32, ui: &mut Ui) {
        let optimal_accel = input.player_speed * input.cgaz_frametime;
        let potential_speed = (self.previous_speed * self.previous_speed - optimal_accel * optimal_accel
            + 2.0 * (input.wishspeed * optimal_accel))
            .sqrt();
        let accel = input.v - self.previous_speed;

        let x = xpos - if settings.flags & flag::GROUNDSPEED != 0 { 88.0 } else { 52.0 };
        ui.rects.push(Rect {
            x: x - 0.75,
            y: settings.y - 10.75,
            w: 37.75,
            h: 13.75,
            color: [0.0, 0.0, 0.0, 1.0],
            outline: Some(0.5),
        });

        let mut actual_accel = accel;
        if actual_accel < 0.0 {
            actual_accel = 0.001;
        } else if actual_accel > potential_speed - input.v {
            actual_accel = (potential_speed - input.v) * 0.99;
        }
        self.previous_times[(self.percent_index as usize) % PERCENT_SAMPLES] =
            actual_accel / (potential_speed - input.v);
        self.percent_index = self.percent_index.wrapping_add(1);

        let mut total: f32 = self.previous_times.iter().sum();
        if total == 0.0 {
            total = 1.0;
        }
        let percent = total / PERCENT_SAMPLES as f32;
        if percent != 0.0 && percent.is_finite() && input.v != 0.0 {
            ui.rects.push(Rect {
                x: x + 0.25,
                y: settings.y - 9.9,
                w: 36.0 * percent,
                h: 12.0,
                color: [1.0, 0.0, 0.0, 1.0],
                outline: None,
            });
        }
        self.previous_speed = input.v;
    }

    /// `DF_DrawJumpHeight`.
    fn draw_jump_height(&mut self, settings: &Settings, input: &Input, xpos: &mut f32, ui: &mut Ui) {
        if input.force_jump_z_start == -65536.0 {
            // Coming back from a teleport. The C returns before advancing xpos.
            return;
        }
        if input.force_jump_z_start != 0.0 && self.last_z_speed > 0.0 && input.vertical <= 0.0 {
            // Going up, now going down: print the height.
            self.last_jump_height = input.origin[2] - input.force_jump_z_start;
            self.last_jump_height_time = input.time_ms;
        }
        if self.last_jump_height_time > input.time_ms - 1500 && self.last_jump_height > 0.0 {
            Self::text(ui, format!("{:.1}", self.last_jump_height), *xpos, settings.y.trunc(), WHITE);
        }
        *xpos += 42.0;
    }

    /// `DF_DrawJumpDistance`.
    fn draw_jump_distance(&mut self, settings: &Settings, input: &Input, xpos: &mut f32, ui: &mut Ui) {
        if input.on_ground {
            if !self.was_on_ground {
                // Just landed.
                let dx = input.origin[0] - self.last_ground_position[0];
                let dy = input.origin[1] - self.last_ground_position[1];
                self.last_jump_distance = dx.hypot(dy);
                self.last_jump_distance_time = input.time_ms;
            }
            self.last_ground_position = input.origin;
        }
        if self.last_jump_distance_time > input.time_ms - 1500 && self.last_jump_distance > 0.0 {
            Self::text(ui, format!("{:.1}", self.last_jump_distance), *xpos, settings.y.trunc(), WHITE);
        }
        *xpos += 62.0;
    }

    /// `DF_DrawYawSpeed`.
    fn draw_yaw_speed(&mut self, settings: &Settings, input: &Input, xpos: &mut f32, ui: &mut Ui) {
        let diff = angle_subtract(input.view_yaw, self.last_yaw);
        let mut frametime = input.frame_ms as f32 / 1000.0;
        if frametime <= 0.0 {
            frametime = 0.001;
        }
        let yawspeed = (diff / frametime).abs();

        if input.real_ms - self.yaw_last_update > 20 {
            self.yaw_last_update = input.real_ms;
            self.previous_yaws[(self.yaw_index as usize) % YAW_FRAMES] = yawspeed as i32 as u16;
            self.yaw_index = self.yaw_index.wrapping_add(1);
        }
        let mut total: i32 = self.previous_yaws.iter().map(|&yaw| i32::from(yaw)).sum();
        if total == 0 {
            total = 1;
        }
        let yaw = (total as f32 / YAW_FRAMES as f32) as i32;
        if yaw != 0 {
            let rounded = (yaw as f32 + 0.5) as i32;
            let text = if yawspeed > 320.0 {
                format!("^1{rounded:03}")
            } else if yawspeed > 265.0 {
                format!("^3{rounded:03}")
            } else {
                format!("{rounded:03}")
            };
            Self::text(ui, text, *xpos, settings.y.trunc(), WHITE);
        }
        self.last_yaw = input.view_yaw;
        *xpos += 16.0;
    }

    /// `DF_GraphAddSpeed`.
    fn graph_add_speed(&mut self, speed: f32) {
        if speed > self.graph[self.max_index] {
            self.max_index = self.oldest;
            self.graph[self.oldest] = speed;
            self.oldest = (self.oldest + 1) % GRAPH_SAMPLES;
            return;
        }
        self.graph[self.oldest] = speed;
        let overwritten_max = self.max_index == self.oldest;
        self.oldest += 1;
        if overwritten_max {
            // The old maximum was overwritten: find a new one.
            self.max_index = 0;
            for i in 1..GRAPH_SAMPLES {
                if self.graph[i] > self.graph[self.max_index] {
                    self.max_index = i;
                }
            }
        }
        self.oldest %= GRAPH_SAMPLES;
    }

    /// `DF_DrawSpeedGraph` with the rect/colours `CG_DrawSpeedometer`'s caller passes.
    fn draw_speed_graph(&self, ui: &mut Ui) {
        let (gx, gy, gw, gh) = (640.0 * 0.5 - 150.0 / 2.0, 480.0 - 22.0 - 2.0, 150.0, 22.0);
        let fore_alpha = 0.8;
        let fast = [1.0_f32, 0.0, 0.0];
        let medium = [0.0_f32, 1.0, 0.0];
        let slow = [0.0_f32, 0.0, 1.0];
        let lerp = |f: f32, a: [f32; 3], b: [f32; 3]| [0, 1, 2].map(|i| a[i] + f * (b[i] - a[i]));

        let max = self.graph[self.max_index].max(GRAPH_MIN_RANGE);
        // The back colour is fully transparent, so the C's backdrop draws nothing.
        for i in 1..GRAPH_SAMPLES {
            let val = self.graph[(self.oldest + i) % GRAPH_SAMPLES];
            let rgb = if val < SPEED_MED {
                lerp(val / SPEED_MED, slow, medium)
            } else if val < SPEED_FAST {
                lerp((val - SPEED_MED) / (SPEED_FAST - SPEED_MED), medium, fast)
            } else {
                fast
            };
            let top = gy + (1.0 - val / max) * gh;
            ui.graph_rects.push(Rect {
                x: gx + i as f32 / GRAPH_SAMPLES as f32 * gw,
                y: top,
                w: gw / GRAPH_SAMPLES as f32,
                h: val * gh / max,
                color: [rgb[0], rgb[1], rgb[2], fore_alpha],
                outline: None,
            });
        }
    }

    /// `DF_DrawSpeedGraphOld`: shares `cg_lagometer`'s position and frame. Modes 1 and 2
    /// draw it as a 48x48 graph in the `gfx/2d/lag` frame, 2 (and 3 while moving) adds the
    /// speed; every other mode is the frameless 144 px tall graph, 96 higher.
    fn draw_speed_graph_old(
        &self,
        lagometer: &crate::lagometer::Settings,
        coef: f32,
        speed: f32,
        ui: &mut Ui,
    ) {
        let (x, mut y) = crate::lagometer::speed_graph_origin(lagometer, coef);
        let framed = lagometer.mode == 1 || lagometer.mode == 2;
        if framed {
            ui.graph_pics.push(crate::lagometer::Pic {
                icon: crate::lagometer::Icon::Lag,
                x,
                y,
                w: 48.0 * coef,
                h: 48.0,
            });
        }
        if lagometer.mode == 2 || (lagometer.mode == 3 && speed != 0.0) {
            ui.graph_texts.push(crate::lagometer::Text {
                text: format!("{speed:.0}"),
                x: x + 2.0 * coef,
                y,
                right: false,
            });
        }
        if !framed {
            y -= 96.0;
        }
        let ah = if framed { 48.0 } else { 48.0 * 3.0 };
        let (ax, ay, aw) = (x - 1.0 * coef, y, 48usize);
        let vscale = ah / 3000.0;
        for a in 0..aw {
            let i = (self.old_graph_count.wrapping_sub(1).wrapping_sub(a as u32) as usize) & (OLD_GRAPH_SAMPLES - 1);
            let mut v = self.old_graph[i] as f32;
            if v > 0.0 {
                v = (v * vscale).min(ah);
                ui.graph_rects.push(Rect {
                    x: ax + (aw - a) as f32 * coef,
                    y: ay + ah - v,
                    w: 1.0 * coef,
                    h: v,
                    color: [0.0, 1.0, 0.0, 1.0],
                    outline: None,
                });
            }
        }
    }
}

/// `DF_DrawVerticalSpeed`.
fn draw_vertical_speed(settings: &Settings, input: &Input, xpos: &mut f32, ui: &mut Ui) {
    let vertical = input.vertical.abs();
    if vertical != 0.0 {
        Speedometer::text(ui, format!("{vertical:.0}"), *xpos, settings.y.trunc(), WHITE);
    }
    *xpos += 42.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            v: 500.0,
            vxyz: 500.0,
            wishspeed: 250.0,
            player_speed: 250.0,
            cgaz_frametime: 0.008,
            on_ground: true,
            real_ms: 1000,
            time_ms: 1000,
            frame_ms: 8,
            width_ratio_coef: 1.0,
            ..Input::default()
        }
    }

    fn settings(flags: u32) -> Settings {
        Settings { flags, ..Settings::default() }
    }

    #[test]
    fn toggle_radio_groups() {
        let speed = flag::ENABLE | flag::MPH;
        // Turning KPH on turns MPH off; the other bits stay.
        assert_eq!(toggle(speed, 8), flag::ENABLE | flag::KPH);
        // Turning it on again toggles it off.
        assert_eq!(toggle(flag::KPH, 8), 0);
        assert_eq!(toggle(flag::JUMPSCOLORS2, 12), flag::JUMPSCOLORS1);
        assert_eq!(toggle(0, 0), flag::ENABLE);
        assert_eq!(toggle(flag::ENABLE, 0), 0);
    }

    #[test]
    fn disabled_draws_nothing() {
        let mut speedo = Speedometer::default();
        assert!(speedo.update(&settings(0), &input()).is_none());
    }

    #[test]
    fn plain_speedometer_shows_label_and_rounded_speed() {
        let mut speedo = Speedometer::default();
        let ui = speedo.update(&settings(flag::ENABLE), &input()).expect("enabled");
        assert_eq!(ui.texts.len(), 2);
        assert_eq!(ui.texts[1].text, "   500");
        assert!(ui.texts[0].text.ends_with("\u{b5}:"));
        assert_eq!((ui.texts[0].x, ui.texts[0].y), (132.0, 459.0));
        // Above the wishspeed the number fades toward red.
        assert!(ui.texts[1].color[1] < 1.0);
    }

    #[test]
    fn old_speed_graph_follows_the_lagometer_cvars() {
        let mut speedo = Speedometer::default();
        let config = settings(flag::ENABLE | flag::SPEEDGRAPHOLD);

        // cg_lagometer 2: 48 px graph in the lag frame, with the speed written in it.
        let framed = Input { lagometer: crate::lagometer::Settings { mode: 2, ..Default::default() }, ..input() };
        let ui = speedo.update(&config, &framed).expect("enabled");
        assert_eq!(ui.graph_pics.len(), 1);
        assert_eq!((ui.graph_pics[0].x, ui.graph_pics[0].y), (592.0, 480.0 - 144.0 - 56.0 - 16.0));
        assert_eq!(ui.graph_texts[0].text, "500");
        assert!((ui.graph_rects[0].h - 500.0 * 48.0 / 3000.0).abs() < 1e-3);

        // cg_lagometer 0: the frameless 144 px graph, 96 higher.
        let ui = speedo.update(&config, &input()).expect("enabled");
        assert!(ui.graph_pics.is_empty() && ui.graph_texts.is_empty());
        assert!((ui.graph_rects[0].h - 500.0 * 144.0 / 3000.0).abs() < 1e-3);
    }

    #[test]
    fn kph_scales_the_speed() {
        let mut speedo = Speedometer::default();
        let ui = speedo.update(&settings(flag::ENABLE | flag::KPH), &input()).expect("enabled");
        assert_eq!(ui.texts[1].text, "   51");
        assert!(ui.texts[0].text.ends_with("k:"));
    }

    #[test]
    fn pre_speed_is_captured_on_takeoff() {
        let mut speedo = Speedometer::default();
        let config = settings(flag::ENABLE | flag::GROUNDSPEED);
        speedo.update(&config, &input());
        let airborne = Input { on_ground: false, vertical: 270.0, v: 640.0, time_ms: 1008, real_ms: 1008, ..input() };
        let ui = speedo.update(&config, &airborne).expect("enabled");
        // Speed, then the pre-speed at 132 + 52.
        let pre = ui.texts.iter().find(|text| text.x == 184.0).expect("pre-speed readout");
        assert_eq!(pre.text, "640");
    }

    #[test]
    fn jump_height_is_reported_at_the_apex() {
        let mut speedo = Speedometer::default();
        let config = settings(flag::ENABLE | flag::JUMPHEIGHT);
        let rising = Input { on_ground: false, vertical: 100.0, force_jump_z_start: 24.0, origin: [0.0, 0.0, 60.0], ..input() };
        speedo.update(&config, &rising);
        let apex = Input { vertical: -1.0, origin: [0.0, 0.0, 84.0], time_ms: 1100, ..rising };
        let ui = speedo.update(&config, &apex).expect("enabled");
        assert!(ui.texts.iter().any(|text| text.text == "60.0"));
    }

    #[test]
    fn graph_tracks_its_maximum() {
        let mut speedo = Speedometer::default();
        for speed in [100.0, 1200.0, 300.0] {
            speedo.graph_add_speed(speed);
        }
        assert_eq!(speedo.graph[speedo.max_index], 1200.0);
    }
}
