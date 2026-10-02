//! jaPRO `cg_lagometer`, ported from `cg_draw.c`: `CG_AddLagometerFrameInfo`,
//! `CG_AddLagometerSnapshotInfo`, `CG_DrawLagometer` and `CG_DrawDisconnect`.
//!
//! The recorders keep the same two 256-sample rings as the C `lagometer` struct
//! (`frameSamples`: how far cgame time sits ahead of / behind the newest snapshot,
//! `snapshotSamples`: each snapshot's ping, -1 for a dropped one). [`Lagometer::draw`]
//! returns the draw list in cgame's 640x480 virtual space; `ui.rs` maps it to pixels.
//!
//! `cg_lagometer` modes (as in the C): 0 only the "connection interrupted" warning,
//! 1 graph in the `gfx/2d/lag` frame, 2 frame plus average ping / interpolation
//! numbers, 3 numbers over a frameless graph. The same cvars also place and frame the
//! old speed graph (`speedometer.rs`, `DF_DrawSpeedGraphOld`), which stacks above it.
//! Mode 4 is an addition of this client: mode 3 plus a second row of numbers under the
//! graph (packet loss over the visible window, and its peak ping).
//!
//! Notes on how this maps onto the C:
//! * `cgs.widthRatioCoef` (`cl_ratioFix`, on by default) is applied exactly as written:
//!   the caller passes `640 * height / (480 * width)` in [`DrawInput`].
//! * `cg_hudFiles` is 0 (jaPRO's default, and this client has no hud files), so the
//!   graph sits 16 higher.
//! * Dropped snapshots are the gaps in `message_num` between snapshots read for a live
//!   server. `trap->GetSnapshot` fails for exactly those numbers in the C, so the
//!   samples are the same.
//! * `REAL_CMD_BACKUP` follows `cl_commandsize` (jaPRO's cgame default is 64): the
//!   warning looks at the command that many back. The usercmd ring itself is 512 deep.

pub const LAG_SAMPLES: usize = 256;
/// `MAX_LAGOMETER_PING` / `MAX_LAGOMETER_RANGE`.
const MAX_LAGOMETER_PING: f32 = 900.0;
const MAX_LAGOMETER_RANGE: f32 = 300.0;
/// `SNAPFLAG_RATE_DELAYED`: the server held this snapshot back to respect `rate`.
const SNAPFLAG_RATE_DELAYED: u8 = 1;
/// `BIGCHAR_WIDTH` / `BIGCHAR_HEIGHT`.
pub const BIGCHAR: f32 = 16.0;
/// `CMD_BACKUP` of the client's usercmd ring (`REAL_CMD_BACKUP` in `CG_DrawDisconnect`).
pub const COMMAND_BACKUP: i32 = jka_protocol::session::CMD_BACKUP as i32;

/// `g_color_table` entries the graph uses.
const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// `cg_lagometer`, `cg_lagometerX` and `cg_lagometerY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub mode: i32,
    pub x: i32,
    pub y: i32,
    /// `cl_commandsize` (jaPRO's cgame mirror of it, default 64).
    pub command_size: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: 0, x: 48, y: 144, command_size: DEFAULT_COMMAND_SIZE }
    }
}

/// `cl_commandsize`'s default in jaPRO's `cg_xcvar.h`.
pub const DEFAULT_COMMAND_SIZE: i32 = 64;

impl Settings {
    /// `REAL_CMD_BACKUP` in `CG_DrawDisconnect`: `cl_commandsize` when it is 4..=512,
    /// else the full `CMD_BACKUP`.
    pub fn real_command_backup(&self) -> i32 {
        if (4..=COMMAND_BACKUP).contains(&self.command_size) {
            self.command_size
        } else {
            COMMAND_BACKUP
        }
    }
}

/// `cgs.widthRatioCoef` with `cl_ratioFix 1` for a window of this size.
pub fn width_ratio_coef(width: u32, height: u32) -> f32 {
    if width == 0 || height == 0 {
        return 1.0;
    }
    (640.0 * height as f32) / (480.0 * width as f32)
}

/// The fields of a snapshot `CG_AddLagometerSnapshotInfo` looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub message_num: i32,
    pub server_time: i32,
    /// `snap->ps.commandTime`.
    pub command_time: i32,
    /// `snap->ping` as the client computed it (0 for demos and the local server).
    pub ping: i32,
    pub flags: u8,
}

/// What `CG_DrawDisconnect` decided to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    #[default]
    Ok,
    /// `cg.mMapChange`: "Server Changing Maps / Please wait...".
    MapChange,
    /// Past every usercmd buffer: "Connection Interrupted" and the blinking phone jack.
    Interrupted,
}

/// Which stock image a [`Pic`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// `gfx/2d/lag`: the graph's frame.
    Lag,
    /// `gfx/2d/net`: the phone jack.
    Net,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pic {
    pub icon: Icon,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

/// `CG_Text_Paint(.., 0.5, colorWhite, .., ITEM_TEXTSTYLE_SHADOWEDMORE, FONT_SMALL)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    /// Left edge, or right edge when `right` is set (the font's width is only known
    /// where it is loaded, so the UI does the `x - CG_Text_Width(..)` subtraction).
    pub x: f32,
    pub y: f32,
    pub right: bool,
}

/// `CG_DrawBigString`: 16x16 glyphs in the fixed charset, white, drop-shadowed.
#[derive(Debug, Clone, PartialEq)]
pub struct BigText {
    pub text: String,
    pub x: f32,
    pub y: f32,
    /// `BIGCHAR_WIDTH * cgs.widthRatioCoef`.
    pub char_w: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ui {
    pub pics: Vec<Pic>,
    pub rects: Vec<Rect>,
    pub texts: Vec<Text>,
    pub big_texts: Vec<BigText>,
}

impl Ui {
    pub fn is_empty(&self) -> bool {
        self.pics.is_empty() && self.rects.is_empty() && self.texts.is_empty() && self.big_texts.is_empty()
    }
}

/// What `CG_DrawLagometer` reads besides its own samples.
#[derive(Debug, Clone)]
pub struct DrawInput {
    /// `cgs.widthRatioCoef`.
    pub width_ratio_coef: f32,
    /// `cgs.localServer`: no graph and no warning against an in-process server.
    pub local_server: bool,
    /// `cg_noPredict || g_synchronousClients`: label the graph "snc".
    pub snc: bool,
    pub link: Link,
    /// The warning strings (`MP_INGAME` StringEd, already resolved).
    pub interrupted_text: String,
    pub map_change_text: String,
    pub please_wait_text: String,
}

impl Default for DrawInput {
    fn default() -> Self {
        Self {
            width_ratio_coef: 1.0,
            local_server: false,
            snc: false,
            link: Link::Ok,
            interrupted_text: String::new(),
            map_change_text: String::new(),
            please_wait_text: String::new(),
        }
    }
}

/// The C `lagometer` struct plus the little cgame state the recorders and the
/// warning read.
#[derive(Debug, Clone)]
pub struct Lagometer {
    frame_samples: [i32; LAG_SAMPLES],
    frame_count: u32,
    snapshot_flags: [i32; LAG_SAMPLES],
    snapshot_samples: [i32; LAG_SAMPLES],
    snapshot_count: u32,
    /// `message_num` of the last snapshot read, to find the ones that never arrived.
    last_message_num: Option<i32>,
    /// `cg.time` at the last frame sample (drives the warning icon's blink).
    time: i32,
}

impl Default for Lagometer {
    fn default() -> Self {
        Self {
            frame_samples: [0; LAG_SAMPLES],
            frame_count: 0,
            snapshot_flags: [0; LAG_SAMPLES],
            snapshot_samples: [0; LAG_SAMPLES],
            snapshot_count: 0,
            last_message_num: None,
            time: 0,
        }
    }
}

fn frame_slot(count: u32) -> usize {
    (count as usize) & (LAG_SAMPLES - 1)
}

/// `CG_DrawStrlen`: characters that are not `^x` colour escapes.
fn visible_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut count = 0;
    while index < bytes.len() {
        if bytes[index] == b'^' && index + 1 < bytes.len() && bytes[index + 1] != b'^' {
            index += 2;
        } else {
            count += 1;
            index += 1;
        }
    }
    count
}

impl Lagometer {
    /// `CG_AddLagometerFrameInfo`: the interpolate / extrapolate bar for this frame.
    pub fn add_frame_info(&mut self, cg_time: i32, latest_snapshot_time: i32) {
        self.frame_samples[frame_slot(self.frame_count)] = cg_time.wrapping_sub(latest_snapshot_time);
        self.frame_count = self.frame_count.wrapping_add(1);
        self.time = cg_time;
    }

    /// `CG_AddLagometerSnapshotInfo` for a snapshot that arrived. `demo` is
    /// `cg.demoPlayback` (with `cgs.svfps`): demos carry no ping, so cgame derives one.
    pub fn add_snapshot_info(&mut self, snapshot: &SnapshotInfo, demo: bool, svfps: i32) {
        if !demo {
            // Anything between the previous snapshot and this one never arrived.
            if let Some(last) = self.last_message_num {
                let missing = snapshot.message_num.wrapping_sub(last).wrapping_sub(1);
                for _ in 0..missing.clamp(0, LAG_SAMPLES as i32) {
                    self.add_dropped();
                }
            }
            self.last_message_num = Some(snapshot.message_num);
        }

        let mut ping = snapshot.ping;
        if demo {
            // `lagometer.frameSamples[frameCount & (LAG_SAMPLES - 2)]` - the mask drops
            // the low bit, exactly as written in jaPRO.
            let frame = self.frame_samples[(self.frame_count as usize) & (LAG_SAMPLES - 2)];
            let frame_ms = 1000 / if svfps == 0 { 20 } else { svfps };
            ping = snapshot
                .server_time
                .wrapping_sub(snapshot.command_time)
                .wrapping_sub(frame_ms)
                .wrapping_add(frame);
            if ping <= 0 {
                ping = 1;
            }
        }

        let slot = frame_slot(self.snapshot_count);
        self.snapshot_samples[slot] = ping;
        self.snapshot_flags[slot] = i32::from(snapshot.flags);
        self.snapshot_count = self.snapshot_count.wrapping_add(1);
    }

    /// `CG_AddLagometerSnapshotInfo(NULL)`: a dropped packet.
    pub fn add_dropped(&mut self) {
        self.snapshot_samples[frame_slot(self.snapshot_count)] = -1;
        self.snapshot_count = self.snapshot_count.wrapping_add(1);
    }

    /// The decision at the top of `CG_DrawDisconnect`. `cmd_server_time` is
    /// `serverTime` of `GetUserCmd(currentCmdNumber - REAL_CMD_BACKUP + 1)` (0 when that
    /// command is gone) and `snapshot_command_time` is `cg.snap->ps.commandTime`.
    pub fn link(&self, map_change: bool, cmd_server_time: i32, snapshot_command_time: i32) -> Link {
        if map_change {
            return Link::MapChange;
        }
        if cmd_server_time <= snapshot_command_time || cmd_server_time > self.time {
            // The oldest buffered command is already acknowledged (or is from before a
            // map_restart): nothing is outstanding.
            return Link::Ok;
        }
        Link::Interrupted
    }

    /// `CG_DrawLagometer`. `None` when there is nothing to draw.
    pub fn draw(&self, settings: &Settings, input: &DrawInput) -> Option<Ui> {
        let mut ui = Ui::default();
        if settings.mode != 0 && !input.local_server {
            self.draw_graph(settings, input, &mut ui);
        }
        self.draw_disconnect(input, &mut ui);
        (!ui.is_empty()).then_some(ui)
    }

    fn draw_graph(&self, settings: &Settings, input: &DrawInput, ui: &mut Ui) {
        let coef = input.width_ratio_coef;
        let x = 640.0 - settings.x as f32 * coef;
        // `cg_hudFiles 0`: the graph sits 16 higher.
        let y = 480.0 - settings.y as f32 - 16.0;
        let top = y;

        if settings.mode < 3 {
            ui.pics.push(Pic { icon: Icon::Lag, x, y, w: 48.0 * coef, h: 48.0 });
        }
        let x = x - 1.0 * coef;

        let (ax, mut ay, aw, ah) = (x, y, 48usize, 48.0f32);

        let mut range = ah / 3.0;
        let mid = ay + range;
        let mut vscale = range / MAX_LAGOMETER_RANGE;

        // The frame interpolate / extrapolate graph.
        let mut avg_interp = 0.0f32;
        for a in 0..aw {
            let i = frame_slot(self.frame_count.wrapping_sub(1).wrapping_sub(a as u32));
            let sample = self.frame_samples[i] as f32;
            avg_interp += sample;
            let mut v = sample * vscale;
            let bar_x = ax + (aw - a) as f32 * coef;
            if v > 0.0 {
                if v > range {
                    v = range;
                }
                ui.rects.push(Rect { x: bar_x, y: mid - v, w: 1.0 * coef, h: v, color: YELLOW });
            } else if v < 0.0 {
                v = (-v).min(range);
                ui.rects.push(Rect { x: bar_x, y: mid, w: 1.0 * coef, h: v, color: BLUE });
            }
        }
        let avg_interp = (avg_interp / aw as f32) * -1.0;

        // The snapshot latency / drop graph.
        range = ah / 2.0;
        vscale = range / MAX_LAGOMETER_PING;
        let mut avg_ping = 0.0f32;
        let mut highest_ping = 1i32;
        let (mut received, mut dropped) = (0u32, 0u32);
        for a in 0..aw {
            let i = frame_slot(self.snapshot_count.wrapping_sub(1).wrapping_sub(a as u32));
            let sample = self.snapshot_samples[i];
            if sample > highest_ping {
                highest_ping = sample;
            }
            let bar_x = ax + (aw - a) as f32 * coef;
            if sample > 0 {
                received += 1;
                avg_ping += sample as f32;
                let color = if self.snapshot_flags[i] & i32::from(SNAPFLAG_RATE_DELAYED) != 0 {
                    YELLOW
                } else {
                    GREEN
                };
                let v = (sample as f32 * vscale).min(range);
                ui.rects.push(Rect { x: bar_x, y: ay + ah - v, w: 1.0 * coef, h: v, color });
            } else if sample < 0 {
                dropped += 1;
                avg_ping += highest_ping as f32;
                ui.rects.push(Rect { x: bar_x, y: ay + ah - range, w: 1.0 * coef, h: range, color: RED });
            }
        }
        let avg_ping = avg_ping / aw as f32;

        ay -= 1.0;

        if input.snc {
            ui.big_texts.push(BigText {
                text: "snc".to_owned(),
                x: ax + 1.0,
                y: ay - 2.0,
                char_w: BIGCHAR * coef,
            });
            ay += BIGCHAR - 1.0;
        }

        if settings.mode == 2 || settings.mode == 3 || settings.mode == 4 {
            ui.texts.push(Text { text: format!("{avg_ping:.0}"), x: ax + 3.0 * coef, y: ay, right: false });
            ui.texts.push(Text {
                text: format!("{avg_interp:04.1}"),
                x: ax + aw as f32 * coef,
                y: ay,
                right: true,
            });
        }

        if settings.mode == 4 {
            // Not in jaPRO: packet loss over the samples shown (red once any is lost)
            // and the highest ping among them, in a row under the graph.
            let seen = received + dropped;
            let loss = if seen == 0 { 0.0 } else { dropped as f32 * 100.0 / seen as f32 };
            let tint = if dropped > 0 { "^1" } else { "^7" };
            let row = top + ah + 11.0;
            ui.texts.push(Text { text: format!("{tint}{loss:.0}%"), x: ax + 3.0 * coef, y: row, right: false });
            ui.texts.push(Text {
                text: format!("{}", if received + dropped == 0 { 0 } else { highest_ping }),
                x: ax + aw as f32 * coef,
                y: row,
                right: true,
            });
        }
    }

    /// `CG_DrawDisconnect`.
    fn draw_disconnect(&self, input: &DrawInput, ui: &mut Ui) {
        if input.local_server {
            return;
        }
        let coef = input.width_ratio_coef;
        let centered = |text: &str, y: f32| BigText {
            text: text.to_owned(),
            x: 320.0 - visible_len(text) as f32 * BIGCHAR * coef / 2.0,
            y,
            char_w: BIGCHAR * coef,
        };
        match input.link {
            Link::Ok => {}
            Link::MapChange => {
                ui.big_texts.push(centered(&input.map_change_text, 100.0));
                ui.big_texts.push(centered(&input.please_wait_text, 200.0));
            }
            Link::Interrupted => {
                ui.big_texts.push(centered(&input.interrupted_text, 100.0));
                // Blink the icon.
                if (self.time >> 9) & 1 == 0 {
                    ui.pics.push(Pic {
                        icon: Icon::Net,
                        x: 640.0 - 48.0 * coef,
                        y: 480.0 - 48.0,
                        w: 48.0 * coef,
                        h: 48.0,
                    });
                }
            }
        }
    }
}

/// Where `DF_DrawSpeedGraphOld` anchors itself from the lagometer cvars: the lag
/// graph's corner, 56 higher (and the 16 for `cg_hudFiles 0`).
pub fn speed_graph_origin(settings: &Settings, width_ratio_coef: f32) -> (f32, f32) {
    (640.0 - settings.x as f32 * width_ratio_coef, 480.0 - settings.y as f32 - 56.0 - 16.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(message_num: i32, ping: i32, flags: u8) -> SnapshotInfo {
        SnapshotInfo { message_num, server_time: message_num * 50, command_time: message_num * 50 - 40, ping, flags }
    }

    fn input() -> DrawInput {
        DrawInput {
            interrupted_text: "Connection Interrupted".to_owned(),
            map_change_text: "Server Changing Maps".to_owned(),
            please_wait_text: "Please wait...".to_owned(),
            ..DrawInput::default()
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn rect_close(rect: &Rect, x: f32, y: f32, h: f32, color: [f32; 4]) -> bool {
        close(rect.x, x) && close(rect.y, y) && close(rect.w, 1.0) && close(rect.h, h) && rect.color == color
    }

    #[test]
    fn mode_zero_draws_nothing_while_the_link_is_fine() {
        let meter = Lagometer::default();
        assert_eq!(meter.draw(&Settings::default(), &input()), None);
    }

    #[test]
    fn frame_samples_are_cg_time_minus_latest_snapshot_time() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(1_150, 1_100);
        meter.add_frame_info(1_160, 1_200);
        assert_eq!(meter.frame_samples[0], 50);
        assert_eq!(meter.frame_samples[1], -40);
        assert_eq!(meter.frame_count, 2);
    }

    #[test]
    fn missing_message_numbers_become_dropped_samples() {
        let mut meter = Lagometer::default();
        meter.add_snapshot_info(&snapshot(10, 40, 0), false, 20);
        meter.add_snapshot_info(&snapshot(13, 45, 0), false, 20);
        assert_eq!(&meter.snapshot_samples[..4], &[40, -1, -1, 45]);
        assert_eq!(meter.snapshot_count, 4);
        // A repeat or a reset is not a gap.
        meter.add_snapshot_info(&snapshot(13, 45, 0), false, 20);
        meter.add_snapshot_info(&snapshot(2, 45, 0), false, 20);
        assert_eq!(meter.snapshot_count, 6);
    }

    #[test]
    fn demo_ping_is_derived_like_japro() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(1_080, 1_000); // frameSamples[0] = 80
        meter.add_frame_info(1_090, 1_000); // frameSamples[1] = 90, count = 2
        // (serverTime - commandTime) - 1000/svfps + frameSamples[count & 254 = 2]
        let sample = SnapshotInfo { message_num: 1, server_time: 1_000, command_time: 940, ping: 0, flags: 0 };
        meter.add_snapshot_info(&sample, true, 20);
        // frameSamples[2] is still 0: 60 - 50 + 0.
        assert_eq!(meter.snapshot_samples[0], 10);
        let late = SnapshotInfo { server_time: 1_000, command_time: 990, ..sample };
        meter.add_snapshot_info(&late, true, 20);
        // 10 - 50 + 0 <= 0 is clamped to 1.
        assert_eq!(meter.snapshot_samples[1], 1);
        // A demo never reports gaps.
        meter.add_snapshot_info(&SnapshotInfo { message_num: 50, ..sample }, true, 0);
        assert_eq!(meter.snapshot_count, 3);
    }

    #[test]
    fn graph_bars_and_frame_follow_cg_draw_lagometer() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(1_150, 1_000); // +150 ms: yellow, half of the 16 px range
        meter.add_snapshot_info(&snapshot(1, 450, 0), false, 20); // 450 of 900: half of 24 px
        let settings = Settings { mode: 1, ..Settings::default() };
        let ui = meter.draw(&settings, &input()).expect("graph");

        // Frame at x = 640 - 48, y = 480 - 144 - 16.
        assert_eq!(ui.pics, vec![Pic { icon: Icon::Lag, x: 592.0, y: 320.0, w: 48.0, h: 48.0 }]);
        let (ax, ay) = (591.0, 320.0);
        // a = 0 is the newest sample, drawn at the right edge (aw - 0).
        let frame_bar = ui.rects.iter().find(|rect| rect.color == YELLOW).expect("yellow");
        assert!(rect_close(frame_bar, ax + 48.0, ay + 16.0 - 8.0, 8.0, YELLOW), "{frame_bar:?}");
        let ping_bar = ui.rects.iter().find(|rect| rect.color == GREEN).expect("green");
        assert!(rect_close(ping_bar, ax + 48.0, ay + 48.0 - 12.0, 12.0, GREEN), "{ping_bar:?}");
        // Mode 1 has no numbers.
        assert!(ui.texts.is_empty());
    }

    #[test]
    fn rate_delayed_snapshots_are_yellow_and_drops_are_full_height_red() {
        let mut meter = Lagometer::default();
        meter.add_snapshot_info(&snapshot(1, 900, SNAPFLAG_RATE_DELAYED), false, 20);
        meter.add_snapshot_info(&snapshot(3, 100, 0), false, 20); // message 2 dropped
        let ui = meter.draw(&Settings { mode: 1, ..Settings::default() }, &input()).expect("graph");
        let reds: Vec<_> = ui.rects.iter().filter(|rect| rect.color == RED).collect();
        assert_eq!(reds.len(), 1);
        assert!(close(reds[0].h, 24.0));
        assert!(close(reds[0].y, 320.0 + 48.0 - 24.0));
        // Oldest sample is the rate-delayed 900 ms one: clamped to the 24 px range.
        let yellow = ui.rects.iter().find(|rect| rect.color == YELLOW).expect("yellow");
        assert!(close(yellow.h, 24.0));
    }

    #[test]
    fn negative_frame_offsets_hang_below_the_midline_in_blue() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(1_000, 1_600); // -600 ms: clamped to the 16 px range
        let ui = meter.draw(&Settings { mode: 1, ..Settings::default() }, &input()).expect("graph");
        let blue = ui.rects.iter().find(|rect| rect.color == BLUE).expect("blue");
        assert!(close(blue.y, 320.0 + 16.0));
        assert!(close(blue.h, 16.0));
    }

    #[test]
    fn mode_three_has_numbers_but_no_frame_and_mode_two_has_both() {
        let mut meter = Lagometer::default();
        for number in 1..=48 {
            meter.add_snapshot_info(&snapshot(number, 60, 0), false, 20);
            meter.add_frame_info(number * 50 + 20, number * 50);
        }
        let three = meter.draw(&Settings { mode: 3, ..Settings::default() }, &input()).expect("graph");
        assert!(three.pics.is_empty());
        let two = meter.draw(&Settings { mode: 2, ..Settings::default() }, &input()).expect("graph");
        assert_eq!(two.pics.len(), 1);
        for ui in [&three, &two] {
            assert_eq!(ui.texts.len(), 2);
            assert_eq!(ui.texts[0], Text { text: "60".to_owned(), x: 591.0 + 3.0, y: 319.0, right: false });
            // Average of +20 ms offsets, negated.
            assert_eq!(ui.texts[1], Text { text: "-20.0".to_owned(), x: 591.0 + 48.0, y: 319.0, right: true });
        }
    }

    #[test]
    fn snc_label_pushes_the_numbers_down() {
        let meter = Lagometer::default();
        let mut with_snc = input();
        with_snc.snc = true;
        let ui = meter.draw(&Settings { mode: 2, ..Settings::default() }, &with_snc).expect("graph");
        assert_eq!(ui.big_texts, vec![BigText { text: "snc".to_owned(), x: 592.0, y: 317.0, char_w: 16.0 }]);
        assert_eq!(ui.texts[0].y, 319.0 + BIGCHAR - 1.0);
    }

    #[test]
    fn a_local_server_has_no_graph_and_no_warning() {
        let meter = Lagometer::default();
        let mut local = input();
        local.local_server = true;
        local.link = Link::Interrupted;
        assert_eq!(meter.draw(&Settings { mode: 2, ..Settings::default() }, &local), None);
    }

    #[test]
    fn link_follows_cg_draw_disconnect() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(5_000, 4_950);
        assert_eq!(meter.link(true, 0, 0), Link::MapChange);
        // The oldest buffered command was already acknowledged.
        assert_eq!(meter.link(false, 4_000, 4_000), Link::Ok);
        assert_eq!(meter.link(false, 3_000, 4_000), Link::Ok);
        // Newer than cg.time: a map_restart reset the clock.
        assert_eq!(meter.link(false, 6_000, 4_000), Link::Ok);
        assert_eq!(meter.link(false, 4_500, 4_000), Link::Interrupted);
    }

    #[test]
    fn warning_text_is_centred_and_the_icon_blinks() {
        let mut meter = Lagometer::default();
        let mut interrupted = input();
        interrupted.link = Link::Interrupted;

        meter.add_frame_info(0, 0); // (0 >> 9) & 1 == 0: icon shown
        let ui = meter.draw(&Settings::default(), &interrupted).expect("warning");
        assert_eq!(ui.pics, vec![Pic { icon: Icon::Net, x: 592.0, y: 432.0, w: 48.0, h: 48.0 }]);
        assert_eq!(ui.big_texts.len(), 1);
        assert_eq!(ui.big_texts[0].y, 100.0);
        assert_eq!(ui.big_texts[0].x, 320.0 - 22.0 * 8.0);

        meter.add_frame_info(512, 512); // (512 >> 9) & 1 == 1: icon hidden
        let ui = meter.draw(&Settings::default(), &interrupted).expect("warning");
        assert!(ui.pics.is_empty());
        assert_eq!(ui.big_texts.len(), 1);
    }

    #[test]
    fn map_change_draws_both_lines_and_no_icon() {
        let meter = Lagometer::default();
        let mut changing = input();
        changing.link = Link::MapChange;
        let ui = meter.draw(&Settings::default(), &changing).expect("message");
        assert!(ui.pics.is_empty());
        assert_eq!(ui.big_texts.len(), 2);
        assert_eq!(ui.big_texts[0].y, 100.0);
        assert_eq!(ui.big_texts[1].y, 200.0);
        assert_eq!(ui.big_texts[1].x, 320.0 - 14.0 * 8.0);
    }

    #[test]
    fn strlen_skips_colour_escapes() {
        assert_eq!(visible_len("^1abc^7d"), 4);
        // The first '^' is followed by '^', so it is text; the second starts a colour.
        assert_eq!(visible_len("^^1"), 1);
        assert_eq!(visible_len("snc"), 3);
    }

    #[test]
    fn speed_graph_origin_sits_above_the_lag_graph() {
        assert_eq!(speed_graph_origin(&Settings::default(), 1.0), (592.0, 480.0 - 144.0 - 56.0 - 16.0));
        // Widescreen: the x offset shrinks with the width ratio.
        assert_eq!(speed_graph_origin(&Settings::default(), 0.75), (604.0, 480.0 - 144.0 - 56.0 - 16.0));
    }

    #[test]
    fn width_ratio_matches_cl_ratio_fix() {
        assert_eq!(width_ratio_coef(640, 480), 1.0);
        assert!(close(width_ratio_coef(1920, 1080), 0.75));
        assert_eq!(width_ratio_coef(0, 1080), 1.0);
    }

    #[test]
    fn widescreen_squeezes_x_but_not_y() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(1_150, 1_000);
        let mut wide = input();
        wide.width_ratio_coef = 0.75;
        let ui = meter.draw(&Settings { mode: 1, ..Settings::default() }, &wide).expect("graph");
        // x = 640 - 48 * 0.75, 48 * 0.75 wide, still 48 tall and at the same y.
        assert_eq!(ui.pics, vec![Pic { icon: Icon::Lag, x: 604.0, y: 320.0, w: 36.0, h: 48.0 }]);
        let bar = ui.rects.iter().find(|rect| rect.color == YELLOW).expect("yellow");
        assert!(close(bar.x, 604.0 - 0.75 + 48.0 * 0.75));
        assert!(close(bar.w, 0.75));
        assert!(close(bar.h, 8.0));
    }

    #[test]
    fn widescreen_scales_the_warning_glyphs() {
        let mut meter = Lagometer::default();
        meter.add_frame_info(0, 0);
        let mut wide = input();
        wide.width_ratio_coef = 0.75;
        wide.link = Link::Interrupted;
        let ui = meter.draw(&Settings::default(), &wide).expect("warning");
        assert_eq!(ui.big_texts[0].char_w, 12.0);
        assert_eq!(ui.big_texts[0].x, 320.0 - 22.0 * 12.0 / 2.0);
        assert_eq!(ui.pics, vec![Pic { icon: Icon::Net, x: 604.0, y: 432.0, w: 36.0, h: 48.0 }]);
    }

    #[test]
    fn command_size_picks_the_command_the_warning_checks() {
        let mut settings = Settings::default();
        assert_eq!(settings.real_command_backup(), 64);
        settings.command_size = 3; // below 4: falls back to CMD_BACKUP
        assert_eq!(settings.real_command_backup(), COMMAND_BACKUP);
        settings.command_size = 513;
        assert_eq!(settings.real_command_backup(), COMMAND_BACKUP);
        settings.command_size = 512;
        assert_eq!(settings.real_command_backup(), 512);
        settings.command_size = 4;
        assert_eq!(settings.real_command_backup(), 4);
    }

    #[test]
    fn mode_four_adds_loss_and_peak_under_a_frameless_graph() {
        let mut meter = Lagometer::default();
        meter.add_snapshot_info(&snapshot(1, 80, 0), false, 20);
        meter.add_snapshot_info(&snapshot(2, 120, 0), false, 20);
        meter.add_snapshot_info(&snapshot(4, 60, 0), false, 20); // message 3 never arrived
        let ui = meter.draw(&Settings { mode: 4, ..Settings::default() }, &input()).expect("graph");
        assert!(ui.pics.is_empty());
        assert_eq!(ui.texts.len(), 4);
        // 1 dropped of 4 samples = 25%, red; peak ping 120.
        assert_eq!(ui.texts[2], Text { text: "^125%".to_owned(), x: 594.0, y: 320.0 + 48.0 + 11.0, right: false });
        assert_eq!(ui.texts[3], Text { text: "120".to_owned(), x: 639.0, y: 320.0 + 48.0 + 11.0, right: true });

        let clean = Lagometer::default();
        let ui = clean.draw(&Settings { mode: 4, ..Settings::default() }, &input()).expect("graph");
        assert_eq!(ui.texts[2].text, "^70%");
        assert_eq!(ui.texts[3].text, "0");
    }
}
