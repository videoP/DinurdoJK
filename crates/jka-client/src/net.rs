//! Live multiplayer connection: UDP transport around the sans-IO protocol
//! session, userinfo cvars, OpenJK command forwarding tables, usercmd
//! generation (CL_CreateCmd) and client-side prediction
//! (CG_PredictPlayerState) over the pinned OpenJK Pmove.

use std::{
    collections::HashSet,
    io::ErrorKind,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use jka_movement::{
    CollisionWorld, EntityClip, NetworkPlayerState, TraceQuery, TraceResult, TraceWorld, PlayerState as NativePlayerState, PmoveContext, PredictSettings,
    SaberMovementInfo, UserCmd as MovementCmd,
};
use jka_protocol::{
    commands::{atoi, info_value},
    netchan::UserCmd,
    server::{PlayerState, Snapshot},
    session::{set_info_value, ClientSession, ConnectionState, SessionEvent, CMD_BACKUP},
};

pub mod mod_support;

// ---------------------------------------------------------------- cvars --

/// Archived CVAR_USERINFO / network cvars. Defaults follow OpenJK.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkSettings {
    pub name: String,
    pub rate: u32,
    pub snaps: u32,
    pub max_packets: u32,
    /// `cl_packetdup`: repeat the usercmds of this many earlier packets (0..=5).
    pub packet_dup: u32,
    pub time_nudge: i32,
    /// `net_port`: preferred local UDP port for the live game socket. Like TaystJK/OpenJK,
    /// connection startup scans this port and the next 9 ports if it is occupied.
    pub net_port: u16,
    pub saber1: String,
    pub saber2: String,
    pub color1: u8,
    pub color2: u8,
    /// jaPRO `cp_sbRGB1` / `cp_sbRGB2`: packed `r | g << 8 | b << 16` blade colour
    /// used when `color1` / `color2` is `SABER_RGB`.
    pub sb_rgb1: u32,
    pub sb_rgb2: u32,
    pub forcepowers: String,
    pub sex: String,
    pub password: String,
    /// `char_color_red/green/blue`: the player tint the server relays as
    /// `customRGBA`; only shaders reading the entity colour follow it.
    pub char_color: [u8; 3],
    /// `handicap`: the server clamps it to 1..100 and uses it as max health.
    pub handicap: u32,
    /// `cg_predictItems`: tells the server whether this client predicts pickups.
    pub predict_items: bool,
    /// Read-only `teamoverlay` CVAR_USERINFO mirror. OpenJK/TaystJK set this
    /// from cg_drawTeamOverlay so the game server knows whether to send tinfo.
    pub team_overlay: bool,
    /// jaPRO `cp_cosmetics`: bitfield of enabled model cosmetics.
    pub cosmetics: u32,
    /// `fs_game` for local (solo) games: the game directory mounted over base
    /// when a local map loads. A server's own `fs_game` overrides it while connected.
    pub fs_game: String,
    /// jaPRO `cp_clanPwd`. Never archived, as in jaPRO.
    pub clan_pwd: String,
    /// jaPRO `ui_username`: account name for the login menu / `login` command.
    pub ui_username: String,
    /// jaPRO `ui_password`. jaPRO archives it in plain text; here it lives for
    /// the session only, like `cp_clanPwd`.
    pub ui_password: String,
    /// `rconPassword` (CVAR_TEMP in jaPRO, so never archived or sent in userinfo).
    pub rcon_password: String,
    /// `rconAddress`: server used by `rcon` while not connected. Not archived.
    pub rcon_address: String,
    /// jaPRO ROM cvar `cg_displayCameraPosition` ("thirdPerson range vertOffset").
    /// The app refreshes it from the live camera before every userinfo send.
    pub display_camera_position: String,
    /// jaPRO ROM cvar `cg_displayNetSettings` ("cl_maxPackets cl_timeNudge com_maxFPS").
    pub display_net_settings: String,
    pub error_decay: f32,
    pub no_predict: bool,
    pub show_miss: bool,
    /// Session-only prediction instrumentation. This intentionally does not
    /// affect Pmove or the state sent to the server.
    pub prediction_debug: bool,
    /// Flash a HUD warning when a prediction correction exceeds the threshold.
    pub prediction_miss_highlight: bool,
    /// Keep the rolling per-frame history that `hitchmark` writes out. Session-only.
    pub hitch_record: bool,
    /// Per-present model anchor CSV. Session-only; disk I/O runs on a worker.
    pub model_frame_debug: bool,
    /// `cg_groundTraceDebug`: 0 off, 1 = ground transitions and server mismatches, 2 = every new
    /// command. Prints this client's ground-trace results to the console. Session-only.
    pub ground_trace_debug: u8,
    /// `cg_physicsDiag`: 0 off, 1 = report snapshot intervals whose replay does not reproduce the
    /// server's state, 2 = every interval. Session-only.
    pub physics_diag: u8,
    /// `cg_snapMode`: how the native pmove rounds velocity after each step (the engine's
    /// `trap_SnapVector`). -1 = detect per server (default), 0 OpenJK nearest, 1 truncate,
    /// 2 floor, 3 nearest-even, 4 none.
    pub snap_mode: i32,
    /// `cg_predictBackend`: which native pmove predicts. -1 = detect (JA+/jaPRO and
    /// servers advertising TaystJK movement capabilities use the shared backend; plain Base/other
    /// servers are replay-tested), 0 stock OpenJK, 1 TaystJK shared BG/Pmove. Session-only.
    pub predict_backend: i32,
    /// Minimum correction distance, in JKA units, for the visual miss warning.
    pub prediction_miss_threshold: f32,
    /// Legacy compatibility escape hatch; normal joins now use the explicit missing-map prompt.
    pub allow_missing_map: bool,
    /// Allow HTTP package autodownload when the server advertises a TaystJK mvhttp/mvhttpurl endpoint.
    pub allow_http_downloads: bool,
    /// Allow the stock protocol-26 svc_download transport.
    pub allow_legacy_downloads: bool,
    /// Usercmds per second (authored; not tied to com_maxfps).
    pub command_rate: u32,
    /// Pace usercmds on `cl.serverTime` so their spacing is exactly 1000/cl_commandRate
    /// ms (race physics steps by the command interval); false = wall-clock pacing.
    pub command_pacing: bool,
    /// JAPRO userinfo bitfield, also consumed by the native predictor.
    pub plugin_disable: i32,
    /// jaPRO `cl_chatBubbleSelf`: send BUTTON_TALK (the chat balloon over your
    /// head) while a console/chat/menu has the keyboard.
    pub chat_bubble_self: bool,
    /// jaPRO `cl_chatBubbleUnfocused`: also while the game window is unfocused.
    pub chat_bubble_unfocused: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            name: "Padawan".into(),
            rate: 50000,
            snaps: 100,
            max_packets: 100,
            packet_dup: 1,
            time_nudge: 0,
            net_port: jka_protocol::DEFAULT_PORT,
            saber1: "single_1".into(),
            saber2: "none".into(),
            color1: 4,
            color2: 4,
            sb_rgb1: 0,
            sb_rgb2: 0,
            forcepowers: "7-1-032330000000001333".into(),
            sex: "male".into(),
            password: String::new(),
            char_color: [255; 3],
            handicap: 100,
            predict_items: true,
            team_overlay: false,
            cosmetics: 0,
            fs_game: "japro".into(),
            clan_pwd: "none".into(),
            ui_username: String::new(),
            ui_password: String::new(),
            rcon_password: String::new(),
            rcon_address: String::new(),
            display_camera_position: "1 80 16".into(),
            display_net_settings: "125 0 125".into(),
            error_decay: 100.0,
            no_predict: false,
            show_miss: false,
            prediction_debug: false,
            prediction_miss_highlight: false,
            hitch_record: false,
            model_frame_debug: false,
            ground_trace_debug: 0,
            physics_diag: 0,
            snap_mode: -1,
            predict_backend: -1,
            prediction_miss_threshold: 8.0,
            allow_missing_map: false,
            allow_http_downloads: true,
            allow_legacy_downloads: true,
            command_rate: 125,
            command_pacing: true,
            plugin_disable: 1536,
            chat_bubble_self: true,
            chat_bubble_unfocused: true,
        }
    }
}

impl NetworkSettings {
    pub fn cvar_value(&self, name: &str) -> Option<String> {
        Some(match name.to_ascii_lowercase().as_str() {
            "name" => self.name.clone(),
            "rate" => self.rate.to_string(),
            "snaps" => self.snaps.to_string(),
            "cl_maxpackets" => self.max_packets.to_string(),
            "cl_packetdup" => self.packet_dup.to_string(),
            "cl_timenudge" => self.time_nudge.to_string(),
            "net_port" => self.net_port.to_string(),
            "saber1" => self.saber1.clone(),
            "saber2" => self.saber2.clone(),
            "color1" => self.color1.to_string(),
            "color2" => self.color2.to_string(),
            "cp_sbrgb1" => self.sb_rgb1.to_string(),
            "cp_sbrgb2" => self.sb_rgb2.to_string(),
            "forcepowers" => self.forcepowers.clone(),
            "sex" => self.sex.clone(),
            "password" => self.password.clone(),
            "char_color_red" => self.char_color[0].to_string(),
            "char_color_green" => self.char_color[1].to_string(),
            "char_color_blue" => self.char_color[2].to_string(),
            "handicap" => self.handicap.to_string(),
            "cg_predictitems" => u8::from(self.predict_items).to_string(),
            "teamoverlay" => u8::from(self.team_overlay).to_string(),
            "cp_cosmetics" => (self.cosmetics as i32).to_string(),
            "fs_game" => self.fs_game.clone(),
            "cp_clanpwd" => self.clan_pwd.clone(),
            "ui_username" => self.ui_username.clone(),
            "ui_password" => self.ui_password.clone(),
            "rconpassword" => self.rcon_password.clone(),
            "rconaddress" => self.rcon_address.clone(),
            "cg_displaycameraposition" => self.display_camera_position.clone(),
            "cg_displaynetsettings" => self.display_net_settings.clone(),
            "cg_errordecay" => format!("{}", self.error_decay),
            "cg_nopredict" => u8::from(self.no_predict).to_string(),
            "cg_showmiss" => u8::from(self.show_miss).to_string(),
            "cg_predictiondebug" => u8::from(self.prediction_debug).to_string(),
            "cg_predictionmisshighlight" => u8::from(self.prediction_miss_highlight).to_string(),
            "cg_hitchrecord" => u8::from(self.hitch_record).to_string(),
            "cg_modelframedebug" => u8::from(self.model_frame_debug).to_string(),
            "cg_groundtracedebug" => self.ground_trace_debug.to_string(),
            "cg_physicsdiag" => self.physics_diag.to_string(),
            "cg_snapmode" => self.snap_mode.to_string(),
            "cg_predictbackend" => self.predict_backend.to_string(),
            "cg_predictionmissthreshold" => format!("{}", self.prediction_miss_threshold),
            "cl_allowmissingmap" => u8::from(self.allow_missing_map).to_string(),
            "cl_allowhttpdownload" => u8::from(self.allow_http_downloads).to_string(),
            "cl_allowdownload" => u8::from(self.allow_legacy_downloads).to_string(),
            "cl_commandrate" => self.command_rate.to_string(),
            "cl_commandpacing" => u8::from(self.command_pacing).to_string(),
            "cp_plugindisable" => self.plugin_disable.to_string(),
            "cl_chatbubbleself" => u8::from(self.chat_bubble_self).to_string(),
            "cl_chatbubbleunfocused" => u8::from(self.chat_bubble_unfocused).to_string(),
            _ => return None,
        })
    }

    /// `None` when `name` is not a network cvar. `Some(Ok(userinfo_changed))`.
    pub fn set_cvar(&mut self, name: &str, value: &str) -> Option<Result<bool, String>> {
        let number = |min: i64, max: i64| -> Result<i64, String> {
            value
                .trim()
                .parse::<i64>()
                .ok()
                .map(|v| v.clamp(min, max))
                .ok_or_else(|| format!("{name}: expected an integer"))
        };
        let text = || -> Result<String, String> {
            if value.bytes().any(|b| matches!(b, b'\\' | b';' | b'"')) {
                Err(format!("{name}: can't use keys or values with \\ ; or \""))
            } else {
                Ok(value.to_owned())
            }
        };
        let result = match name.to_ascii_lowercase().as_str() {
            "name" => text().map(|v| { self.name = v; true }),
            "rate" => number(1000, 90000).map(|v| { self.rate = v as u32; true }),
            "snaps" => number(1, 125).map(|v| { self.snaps = v as u32; true }),
            "cl_maxpackets" => number(15, 1000).map(|v| { self.max_packets = v as u32; false }),
            "cl_packetdup" => number(0, 5).map(|v| { self.packet_dup = v as u32; false }),
            "cl_timenudge" => number(-900, 900).map(|v| { self.time_nudge = v as i32; false }),
            // TaystJK/OpenJK register net_port as CVAR_LATCH: changing it does not
            // disturb an existing socket; the new value is used on the next connect.
            "net_port" => number(0, u16::MAX as i64).map(|v| { self.net_port = v as u16; false }),
            "saber1" => text().map(|v| { self.saber1 = v; true }),
            "saber2" => text().map(|v| { self.saber2 = v; true }),
            "color1" => number(0, 255).map(|v| { self.color1 = v as u8; true }),
            "color2" => number(0, 255).map(|v| { self.color2 = v as u8; true }),
            "cp_sbrgb1" => number(0, 0xFF_FFFF).map(|v| { self.sb_rgb1 = v as u32; true }),
            "cp_sbrgb2" => number(0, 0xFF_FFFF).map(|v| { self.sb_rgb2 = v as u32; true }),
            "forcepowers" => text().map(|v| { self.forcepowers = v; true }),
            "sex" => text().map(|v| { self.sex = v; true }),
            "password" => text().map(|v| { self.password = v; true }),
            "char_color_red" => number(0, 255).map(|v| { self.char_color[0] = v as u8; true }),
            "char_color_green" => number(0, 255).map(|v| { self.char_color[1] = v as u8; true }),
            "char_color_blue" => number(0, 255).map(|v| { self.char_color[2] = v as u8; true }),
            "handicap" => number(1, 100).map(|v| { self.handicap = v as u32; true }),
            "cg_predictitems" => Ok({ self.predict_items = atoi(value.as_bytes()) != 0; true }),
            // CVAR_ROM in OpenJK/TaystJK. cg_drawTeamOverlay owns this value.
            "teamoverlay" => Err("teamoverlay is read-only; use cg_drawTeamOverlay".to_owned()),
            "cp_cosmetics" => number(i64::from(i32::MIN), i64::from(i32::MAX)).map(|v| { self.cosmetics = v as i32 as u32; true }),
            "fs_game" => {
                // A sibling directory name, as `resolve_fs_game_directory` accepts.
                let name = value.trim();
                let valid = name.len() <= 63
                    && name != "."
                    && name != ".."
                    && !name.bytes().any(|b| b < 0x20 || b == 0x7f || matches!(b, b'/' | b'\\' | b':' | b';' | b'"'));
                if valid {
                    self.fs_game = name.to_owned();
                    Ok(false)
                } else {
                    Err(format!("{name}: expected a game directory name like japro (empty or base for none)"))
                }
            }
            "cp_clanpwd" => text().map(|v| { self.clan_pwd = v; true }),
            // Client-only, so not userinfo (`false`). Passwords may hold `;`/`"`
            // characters in a real Cvar, but the console splits on them anyway.
            "ui_username" => text().map(|v| { self.ui_username = v; false }),
            "ui_password" => text().map(|v| { self.ui_password = v; false }),
            "rconpassword" => Ok({ self.rcon_password = value.to_owned(); false }),
            "rconaddress" => Ok({ self.rcon_address = value.trim().to_owned(); false }),
            // CVAR_ROM in jaPRO: derived state the app keeps current, not user-editable.
            "cg_displaycameraposition" | "cg_displaynetsettings" => {
                Err(format!("{name} is read-only"))
            }
            "cg_errordecay" => value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| { self.error_decay = v.clamp(0.0, 500.0); false })
                .ok_or_else(|| format!("{name}: expected a number")),
            "cg_nopredict" => Ok({ self.no_predict = atoi(value.as_bytes()) != 0; false }),
            "cg_showmiss" => Ok({ self.show_miss = atoi(value.as_bytes()) != 0; false }),
            "cg_predictiondebug" => Ok({ self.prediction_debug = atoi(value.as_bytes()) != 0; false }),
            "cg_predictionmisshighlight" => Ok({ self.prediction_miss_highlight = atoi(value.as_bytes()) != 0; false }),
            "cg_hitchrecord" => Ok({ self.hitch_record = atoi(value.as_bytes()) != 0; false }),
            "cg_modelframedebug" => Ok({ self.model_frame_debug = atoi(value.as_bytes()) != 0; false }),
            "cg_groundtracedebug" => Ok({ self.ground_trace_debug = atoi(value.as_bytes()).clamp(0, 2) as u8; false }),
            "cg_physicsdiag" => Ok({ self.physics_diag = atoi(value.as_bytes()).clamp(0, 2) as u8; false }),
            "cg_snapmode" => Ok({ self.snap_mode = atoi(value.as_bytes()).clamp(-1, 4); false }),
            "cg_predictbackend" => Ok({ self.predict_backend = atoi(value.as_bytes()).clamp(-1, 1); false }),
            "cg_predictionmissthreshold" => value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| { self.prediction_miss_threshold = v.clamp(0.0, 4096.0); false })
                .ok_or_else(|| format!("{name}: expected a number")),
            "cl_allowmissingmap" => Ok({ self.allow_missing_map = atoi(value.as_bytes()) != 0; false }),
            "cl_allowhttpdownload" => Ok({ self.allow_http_downloads = atoi(value.as_bytes()) != 0; false }),
            "cl_allowdownload" => Ok({ self.allow_legacy_downloads = atoi(value.as_bytes()) != 0; false }),
            "cl_commandrate" => number(15, 1000).map(|v| { self.command_rate = v as u32; false }),
            "cl_commandpacing" => Ok({ self.command_pacing = atoi(value.as_bytes()) != 0; false }),
            "cp_plugindisable" => number(0, i32::MAX as i64).map(|v| { self.plugin_disable = v as i32; true }),
            "cl_chatbubbleself" => Ok({ self.chat_bubble_self = atoi(value.as_bytes()) != 0; false }),
            "cl_chatbubbleunfocused" => Ok({ self.chat_bubble_unfocused = atoi(value.as_bytes()) != 0; false }),
            _ => return None,
        };
        Some(result)
    }

    /// Cvar_InfoString(CVAR_USERINFO) for the stock userinfo cvars.
    pub fn userinfo(&self, model: &str) -> Vec<u8> {
        let mut info = Vec::new();
        // Info_SetValueForKey prepends, so insert in reverse of the desired order.
        let pairs: [(&str, String); 16] = [
            ("char_color_blue", self.char_color[2].to_string()),
            ("char_color_green", self.char_color[1].to_string()),
            ("char_color_red", self.char_color[0].to_string()),
            ("saber2", self.saber2.clone()),
            ("saber1", self.saber1.clone()),
            ("cg_predictItems", u8::from(self.predict_items).to_string()),
            ("teamoverlay", u8::from(self.team_overlay).to_string()),
            ("sex", self.sex.clone()),
            ("handicap", self.handicap.to_string()),
            ("color2", self.color2.to_string()),
            ("color1", self.color1.to_string()),
            ("forcepowers", self.forcepowers.clone()),
            ("model", model.to_owned()),
            ("snaps", self.snaps.to_string()),
            ("rate", self.rate.to_string()),
            ("name", self.name.clone()),
        ];
        for (key, value) in pairs {
            let value = crate::cgame::text_to_jka_bytes(&value);
            set_info_value(&mut info, key.as_bytes(), &value);
        }
        if !self.password.is_empty() {
            let password = crate::cgame::text_to_jka_bytes(&self.password);
            set_info_value(&mut info, b"password", &password);
        }
        info
    }

    pub fn userinfo_for_server(&self, model: &str, server_info: &[u8]) -> Vec<u8> {
        self.userinfo_for_capabilities(
            model,
            mod_support::ServerMod::detect(server_info),
            mod_support::supports_rgb_sabers(server_info),
        )
    }

    pub fn userinfo_for_mod(&self, model: &str, server_mod: mod_support::ServerMod) -> Vec<u8> {
        self.userinfo_for_capabilities(model, server_mod, server_mod.supports_rgb_sabers())
    }

    fn userinfo_for_capabilities(
        &self,
        model: &str,
        server_mod: mod_support::ServerMod,
        rgb_sabers: bool,
    ) -> Vec<u8> {
        let mut info = self.userinfo(model);
        if rgb_sabers {
            // jaPRO / JA+ relay these as c3/c4 in the client configstring.
            set_info_value(&mut info, b"cp_sbRGB1", self.sb_rgb1.to_string().as_bytes());
            set_info_value(&mut info, b"cp_sbRGB2", self.sb_rgb2.to_string().as_bytes());
        } else {
            // Stock servers and clients only know the six base colours; send the
            // closest one instead of a value they would draw as blue or garbage.
            let base = |color: u8, rgb: u32| -> u8 {
                match i32::from(color) {
                    SABER_RGB => nearest_base_saber_color(rgb),
                    c if c > SABER_PURPLE => nearest_base_saber_color(rgb),
                    _ => color,
                }
            };
            set_info_value(&mut info, b"color1", base(self.color1, self.sb_rgb1).to_string().as_bytes());
            set_info_value(&mut info, b"color2", base(self.color2, self.sb_rgb2).to_string().as_bytes());
        }
        // TaystJK declares cjp_client CVAR_USERINFO|CVAR_ROM, so it is present
        // on every connect. JA+ uses the value to enable plugin behavior including
        // the extended 15-field scores record with trailing deaths.
        set_info_value(&mut info, b"cjp_client", b"1.4JAPRO");
        if server_mod == mod_support::ServerMod::Japro {
            set_info_value(&mut info, b"cp_pluginDisable", self.plugin_disable.to_string().as_bytes());
            set_info_value(&mut info, b"cp_cosmetics", (self.cosmetics as i32).to_string().as_bytes());
            set_info_value(&mut info, b"cp_clanPwd", self.clan_pwd.as_bytes());
            set_info_value(&mut info, b"cg_displayCameraPosition", self.display_camera_position.as_bytes());
            set_info_value(&mut info, b"cg_displayNetSettings", self.display_net_settings.as_bytes());
        }
        info
    }

    pub fn write_cfg(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(out, "seta cp_pluginDisable \"{}\"", self.plugin_disable);
        let _ = writeln!(out, "seta name \"{}\"", self.name);
        let _ = writeln!(out, "seta rate \"{}\"", self.rate);
        let _ = writeln!(out, "seta snaps \"{}\"", self.snaps);
        let _ = writeln!(out, "seta cl_maxpackets \"{}\"", self.max_packets);
        let _ = writeln!(out, "seta cl_packetdup \"{}\"", self.packet_dup);
        let _ = writeln!(out, "seta cl_timeNudge \"{}\"", self.time_nudge);
        let _ = writeln!(out, "seta saber1 \"{}\"", self.saber1);
        let _ = writeln!(out, "seta saber2 \"{}\"", self.saber2);
        let _ = writeln!(out, "seta color1 \"{}\"", self.color1);
        let _ = writeln!(out, "seta color2 \"{}\"", self.color2);
        let _ = writeln!(out, "seta cp_sbRGB1 \"{}\"", self.sb_rgb1);
        let _ = writeln!(out, "seta cp_sbRGB2 \"{}\"", self.sb_rgb2);
        let _ = writeln!(out, "seta forcepowers \"{}\"", self.forcepowers);
        let _ = writeln!(out, "seta sex \"{}\"", self.sex);
        let _ = writeln!(out, "seta char_color_red \"{}\"", self.char_color[0]);
        let _ = writeln!(out, "seta char_color_green \"{}\"", self.char_color[1]);
        let _ = writeln!(out, "seta char_color_blue \"{}\"", self.char_color[2]);
        let _ = writeln!(out, "seta handicap \"{}\"", self.handicap);
        let _ = writeln!(out, "seta cg_predictItems \"{}\"", u8::from(self.predict_items));
        let _ = writeln!(out, "seta cp_cosmetics \"{}\"", self.cosmetics as i32);
        let _ = writeln!(out, "seta fs_game \"{}\"", self.fs_game);
        let _ = writeln!(out, "seta ui_username \"{}\"", self.ui_username);
        let _ = writeln!(out, "seta cg_errorDecay \"{}\"", self.error_decay);
        let _ = writeln!(out, "seta cg_noPredict \"{}\"", u8::from(self.no_predict));
        let _ = writeln!(out, "seta cl_allowMissingMap \"{}\"", u8::from(self.allow_missing_map));
        let _ = writeln!(out, "seta cl_allowHttpDownload \"{}\"", u8::from(self.allow_http_downloads));
        let _ = writeln!(out, "seta cl_allowDownload \"{}\"", u8::from(self.allow_legacy_downloads));
        let _ = writeln!(out, "seta cl_commandRate \"{}\"", self.command_rate);
        let _ = writeln!(out, "seta cl_commandPacing \"{}\"", u8::from(self.command_pacing));
        let _ = writeln!(out, "seta cl_chatBubbleSelf \"{}\"", u8::from(self.chat_bubble_self));
        let _ = writeln!(out, "seta cl_chatBubbleUnfocused \"{}\"", u8::from(self.chat_bubble_unfocused));
    }
}

/// `saber_colors_t`: the six stock colours, then jaPRO's custom-RGB colour.
pub const SABER_PURPLE: i32 = 5;
pub const SABER_RGB: i32 = 6;

/// Stock blade colours as sent in `color1` (red, orange, yellow, green, blue,
/// purple), with the RGB they are drawn with (CG_RGBForSaberColor).
const BASE_SABER_RGB: [[f32; 3]; 6] = [
    [255.0, 51.0, 51.0],
    [255.0, 128.0, 26.0],
    [255.0, 255.0, 51.0],
    [51.0, 255.0, 51.0],
    [51.0, 102.0, 255.0],
    [230.0, 51.0, 255.0],
];

/// Packed `cp_sbRGB` (`r | g << 8 | b << 16`) of a stock `color1` value; used to
/// start the RGB sliders from the blade colour the player already has.
pub fn base_saber_rgb_packed(color: u8) -> u32 {
    let rgb = BASE_SABER_RGB[usize::from(color).min(BASE_SABER_RGB.len() - 1)];
    (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16)
}

/// The stock `color1` value whose blade looks most like the packed `cp_sbRGB`.
pub fn nearest_base_saber_color(packed_rgb: u32) -> u8 {
    let rgb = [(packed_rgb & 255) as f32, ((packed_rgb >> 8) & 255) as f32, ((packed_rgb >> 16) & 255) as f32];
    (0..BASE_SABER_RGB.len())
        .min_by(|&a, &b| {
            let distance = |i: usize| (0..3).map(|c| (rgb[c] - BASE_SABER_RGB[i][c]).powi(2)).sum::<f32>();
            distance(a).total_cmp(&distance(b))
        })
        .unwrap_or(4) as u8
}

// ------------------------------------------------------------- commands --

/// cl_input.cpp inputCmds generic commands -> genCmds_t value.
pub fn generic_command(name: &str) -> Option<u8> {
    Some(match name.to_ascii_lowercase().as_str() {
        "sv_saberswitch" => 1,
        "engage_duel" => 2,
        "force_heal" => 3,
        "force_speed" => 4,
        "force_throw" => 5,
        "force_pull" => 6,
        "force_distract" => 7,
        "force_rage" => 8,
        "force_protect" => 9,
        "force_absorb" => 10,
        "force_healother" => 11,
        "force_forcepowerother" => 12,
        "force_seeing" => 13,
        "use_seeker" => 14,
        "use_field" => 15,
        "use_bacta" => 16,
        "use_electrobinoculars" => 17,
        "zoom" => 18,
        "use_sentry" => 19,
        "use_jetpack" => 20,
        "use_bactabig" => 21,
        "use_healthdisp" => 22,
        "use_ammodisp" => 23,
        "use_eweb" => 24,
        "use_cloak" => 25,
        "saberattackcycle" => jka_movement::GENCMD_SABERATTACKCYCLE,
        "taunt" => 27,
        "bow" => 28,
        "meditate" => 29,
        "flourish" => 30,
        "gloat" => 31,
        _ => return None,
    })
}

/// cl_input.cpp `+button` commands -> usercmd button bit.
pub fn button_bit(command: &str) -> Option<i32> {
    Some(match command.to_ascii_lowercase().as_str() {
        "+attack" | "+button0" => 1 << 0,
        "+button1" => 1 << 1,
        "+button2" => 1 << 2,
        "+button3" => 1 << 3,
        "+button4" => 1 << 4,
        "+use" | "+button5" => 1 << 5,
        "+force_grip" | "+button6" => 1 << 6,
        "+altattack" | "+button7" => 1 << 7,
        "+button8" => 1 << 8,
        "+useforce" | "+button9" => 1 << 9,
        "+force_lightning" | "+button10" => 1 << 10,
        "+force_drain" | "+button11" => 1 << 11,
        "+button12" => 1 << 12,
        "+button13" => 1 << 13,
        "+button14" => 1 << 14,
        "+button15" => 1 << 15,
        _ => return None,
    })
}

const BUTTON_TALK: i32 = 2;
const BUTTON_USE_HOLDABLE: i32 = 4;
const BUTTON_WALKING: i32 = 16;
const BUTTON_USE: i32 = 32;
const BUTTON_ANY: i32 = 256;
const BUTTON_FORCEPOWER: i32 = 512;

// ------------------------------------------------------------ transport --

/// `MAX_RCON_MESSAGE`, including the 4 byte connectionless header.
const MAX_RCON_MESSAGE: usize = 1024;

/// CL_Rcon_f's transport: a connectionless `rcon <password> <command>` datagram
/// to `target` and the connectionless `print` replies the server sends back.
/// It owns its own socket so it works both in a game and from the main menu
/// (`rconAddress`), exactly like the engine's unconnected rcon.
pub struct RconChannel {
    socket: UdpSocket,
    target: SocketAddr,
}

impl RconChannel {
    pub fn open(target: SocketAddr) -> Result<Self, String> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("UDP bind failed: {error}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| format!("UDP socket setup failed: {error}"))?;
        Ok(Self { socket, target })
    }

    pub fn target(&self) -> SocketAddr { self.target }

    /// `command` is the rest of the console line after `rcon `, verbatim.
    pub fn send(&self, password: &str, command: &str) -> Result<(), String> {
        let mut message = b"\xff\xff\xff\xffrcon ".to_vec();
        message.extend_from_slice(password.as_bytes());
        message.push(b' ');
        message.extend_from_slice(command.as_bytes());
        // Q_strcat truncates at MAX_RCON_MESSAGE - 1, then NUL terminates.
        message.truncate(MAX_RCON_MESSAGE - 1);
        message.push(0);
        self.socket
            .send_to(&message, self.target)
            .map(|_| ())
            .map_err(|error| format!("rcon send to {}: {error}", self.target))
    }

    /// The text of every `print` reply received since the last poll. Datagrams
    /// from any other address are ignored (NET_CompareAdr(from, rcon_address)).
    pub fn poll(&self) -> Vec<Vec<u8>> {
        let mut buffer = [0u8; 4096];
        let mut replies = Vec::new();
        loop {
            match self.socket.recv_from(&mut buffer) {
                Ok((len, from)) => {
                    if from != self.target || len < 5 || buffer[..4] != [0xff; 4] {
                        continue;
                    }
                    let body = &buffer[4..len];
                    let split = body.iter().position(|b| b.is_ascii_whitespace()).unwrap_or(body.len());
                    if body[..split].eq_ignore_ascii_case(b"print") {
                        let text = body.get(split + 1..).unwrap_or_default();
                        let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                        replies.push(text[..end].to_vec());
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            }
        }
        replies
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TransportDebug {
    pub sent_datagrams: u64,
    pub send_errors: u64,
}

pub struct NetClient {
    socket: UdpSocket,
    session: ClientSession,
    epoch: Instant,
    transport_debug: TransportDebug,
    pub server_name: String,
}

fn random_u32() -> u32 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mixed = (nanos as u64) ^ ((std::process::id() as u64) << 32);
    let mut x = mixed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    (x ^ (x >> 32)) as u32
}

/// NET_StringToAdr: `host[:port]`, IPv4, default port 29070.
pub fn resolve_server(text: &str) -> Result<SocketAddr, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Bad server address".into());
    }
    let with_port = if text.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
        text.to_owned()
    } else {
        format!("{text}:{}", jka_protocol::DEFAULT_PORT)
    };
    with_port
        .to_socket_addrs()
        .map_err(|error| format!("Bad server address {text}: {error}"))?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| format!("Bad server address {text}: no IPv4 address"))
}

/// TaystJK/OpenJK NET_OpenIP parity for the live client socket. The requested
/// `net_port` is tried first, followed by the next 9 ports. `0` asks the OS for
/// an ephemeral port and succeeds on the first attempt.
fn bind_live_socket(net_port: u16) -> Result<UdpSocket, String> {
    let attempts: u16 = if net_port == 0 { 1 } else { 10 };
    let mut last_error = None;
    for offset in 0..attempts {
        let Some(port) = net_port.checked_add(offset) else { break };
        match UdpSocket::bind(("0.0.0.0", port)) {
            Ok(socket) => return Ok(socket),
            Err(error) => last_error = Some((port, error)),
        }
    }
    match last_error {
        Some((port, error)) => Err(format!(
            "UDP bind failed: net_port {net_port} (last tried {port}): {error}"
        )),
        None => Err(format!("UDP bind failed: invalid net_port {net_port}")),
    }
}

impl NetClient {
    #[cfg(test)]
    pub fn connect(server_name: &str, userinfo: Vec<u8>, net_port: u16) -> Result<Self, String> {
        Self::connect_inner(server_name, userinfo, net_port, false)
    }

    pub fn connect_preflight(server_name: &str, userinfo: Vec<u8>, net_port: u16) -> Result<Self, String> {
        Self::connect_inner(server_name, userinfo, net_port, true)
    }

    fn connect_inner(server_name: &str, userinfo: Vec<u8>, net_port: u16, preflight: bool) -> Result<Self, String> {
        let server = resolve_server(server_name)?;
        let socket = bind_live_socket(net_port)?;
        socket
            .set_nonblocking(true)
            .map_err(|error| format!("UDP socket setup failed: {error}"))?;
        let epoch = Instant::now();
        let seed = random_u32();
        let session = if preflight {
            ClientSession::connect_preflight(
                server,
                userinfo,
                (seed & 0xffff) as u16,
                (seed >> 1) as i32,
                0,
            )
        } else {
            ClientSession::connect(
                server,
                userinfo,
                (seed & 0xffff) as u16,
                (seed >> 1) as i32,
                0,
            )
        };
        let mut client = Self {
            socket, session, epoch, server_name: server_name.to_owned(),
            transport_debug: TransportDebug::default(),
        };
        client.flush();
        Ok(client)
    }

    /// cls.realtime.
    pub fn realtime(&self) -> i32 {
        self.epoch.elapsed().as_millis().min(i32::MAX as u128) as i32
    }

    pub fn session(&self) -> &ClientSession { &self.session }
    pub fn session_mut(&mut self) -> &mut ClientSession { &mut self.session }
    pub(crate) fn transport_debug(&self) -> TransportDebug { self.transport_debug }
    pub fn local_port(&self) -> u16 { self.socket.local_addr().map_or(0, |addr| addr.port()) }
    pub(crate) fn connection_started(&self) -> Instant { self.epoch }

    pub fn resume_connect(&mut self) {
        let now = self.realtime();
        self.session.resume_connect(now);
        self.flush();
    }

    pub fn send_reliable_now(&mut self, command: &[u8]) -> Result<(), String> {
        self.session.add_reliable_command(command, false)?;
        let now = self.realtime();
        self.session.write_packet_now(now);
        self.flush();
        Ok(())
    }

    /// Com_EventLoop's packet delivery plus per-frame resend/timeout checks.
    pub fn pump(&mut self) -> Vec<SessionEvent> {
        let mut buffer = [0u8; 65536];
        loop {
            match self.socket.recv_from(&mut buffer) {
                Ok((len, from)) => {
                    let now = self.realtime();
                    self.session.packet_event(from, &buffer[..len], now);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                // Windows reports ICMP port-unreachable as ConnectionReset on UDP.
                Err(error) if error.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            }
        }
        let now = self.realtime();
        self.session.frame(now, 200_000);
        self.flush();
        self.session.take_events()
    }

    pub fn flush(&mut self) {
        let server = self.session.server();
        for packet in self.session.take_outgoing() {
            match self.socket.send_to(&packet, server) {
                Ok(length) if length == packet.len() => self.transport_debug.sent_datagrams += 1,
                _ => self.transport_debug.send_errors += 1,
            }
        }
    }

    pub fn disconnect(&mut self) {
        let now = self.realtime();
        self.session.disconnect(now);
        self.flush();
    }

    pub fn state(&self) -> ConnectionState { self.session.state() }
}

// ---------------------------------------------------------------- input --

/// CL_CreateCmd state that persists between frames (cl.viewangles,
/// cg.weaponSelect/forceSelect, pending generic command).
#[derive(Debug, Clone, Default)]
pub struct LiveInput {
    pub view_angles: [f32; 3],
    pub weapon_select: u8,
    pub force_select: u8,
    pub inventory_select: u8,
    generic_command: Option<u8>,
    was_pressed: i32,
    weapon_initialized: bool,
    last_spawn_count: i32,
}

pub struct CommandButtons<'a> {
    /// Lowercased `+command` names currently held.
    pub active: &'a HashSet<String>,
    /// TaystJK CG_DoAsync flipkick injection. Kept separate from physical
    /// bindings so a synthetic -moveup cannot erase a genuinely held jump key.
    pub forced_moveup: bool,
    /// Any key held outside menus (BUTTON_ANY).
    pub any_key: bool,
    /// cl_run 1 semantics: +speed walks.
    pub talking: bool,
}

impl LiveInput {
    pub fn queue_generic_command(&mut self, value: u8) {
        self.generic_command = Some(value);
    }

    /// IN_KeyDown's wasPressed: a button tapped between frames still fires.
    pub fn note_pressed(&mut self, command: &str) {
        if let Some(bit) = button_bit(command) {
            self.was_pressed |= bit;
        }
    }

    /// Seed cg.weaponSelect / forceSelect from the authoritative state
    /// (CG_SetInitialSnapshot and CG_Respawn).
    pub fn sync_selection(&mut self, ps: &PlayerState) {
        let spawn_count = ps.persistant[4];
        if !self.weapon_initialized || spawn_count != self.last_spawn_count {
            self.weapon_select = ps.field_i32("weapon").unwrap_or(0).clamp(0, 18) as u8;
            let force = ps.field_i32("fd.forcePowerSelected").unwrap_or(0);
            self.force_select = force.clamp(0, 17) as u8;
            self.weapon_initialized = true;
            self.last_spawn_count = spawn_count;
        }
    }

    /// CL_MouseMove: yaw/pitch accumulate in cl.viewangles.
    pub fn apply_mouse(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.view_angles[1] -= yaw_delta;
        self.view_angles[0] += pitch_delta;
    }

    /// CL_CreateCmd without joystick/keyboard look. Consumes one-shot state
    /// (tapped buttons, pending generic command).
    pub fn create_cmd(&mut self, buttons: &CommandButtons<'_>) -> UserCmd {
        self.view_angles[1] = self.view_angles[1].rem_euclid(360.0);
        let cmd = self.build_cmd(buttons, self.was_pressed, self.generic_command.unwrap_or(0));
        self.was_pressed = 0;
        self.generic_command = None;
        cmd
    }

    /// The command CL_CreateCmd would build right now, without consuming
    /// anything: used for the per-frame provisional prediction step.
    pub fn preview_cmd(&self, buttons: &CommandButtons<'_>) -> UserCmd {
        self.build_cmd(buttons, 0, 0)
    }

    fn build_cmd(&self, buttons: &CommandButtons<'_>, was_pressed: i32, generic_command: u8) -> UserCmd {
        let mut cmd = UserCmd::default();
        // CL_CmdButtons
        for command in buttons.active {
            if let Some(bit) = button_bit(command) {
                cmd.buttons |= bit;
            }
        }
        cmd.buttons |= was_pressed;
        if cmd.buttons & BUTTON_FORCEPOWER != 0 && cmd.buttons & BUTTON_USE != 0 {
            cmd.buttons &= !BUTTON_FORCEPOWER;
            cmd.buttons |= BUTTON_USE_HOLDABLE;
        }
        if buttons.talking {
            cmd.buttons |= BUTTON_TALK;
        }
        if buttons.any_key {
            cmd.buttons |= BUTTON_ANY;
        }
        // CL_KeyMove with cl_run 1.
        let held = |name: &str| {
            buttons.active.contains(name) || (name == "+moveup" && buttons.forced_moveup)
        };
        let movespeed: i32 = if held("+speed") {
            cmd.buttons |= BUTTON_WALKING;
            64
        } else {
            127
        };
        let axis = |positive: &str, negative: &str| -> i8 {
            let value = movespeed * i32::from(held(positive)) - movespeed * i32::from(held(negative));
            value.clamp(-128, 127) as i8
        };
        cmd.forward_move = axis("+forward", "+back");
        cmd.right_move = axis("+moveright", "+moveleft");
        cmd.up_move = axis("+moveup", "+movedown");
        // CL_FinishMove
        cmd.weapon = self.weapon_select;
        cmd.force_selection = self.force_select;
        cmd.inventory_selection = self.inventory_select;
        cmd.generic_command = generic_command;
        for axis in 0..3 {
            cmd.angles[axis] = jka_movement::angle_to_short(self.view_angles[axis]);
        }
        cmd
    }

    /// CG_WeaponSelectable against the predicted state.
    fn selectable(ps: &PlayerState, weapon: i32) -> bool {
        if weapon <= 0 {
            return false;
        }
        let Some((ammo_index, energy, alt_energy)) = jka_movement::weapon_info(weapon) else { return false };
        let ammo = ps.ammo.get(ammo_index.max(0) as usize).copied().unwrap_or(0);
        if ammo < energy && ammo < alt_energy {
            return false;
        }
        const WP_DET_PACK: i32 = 14;
        if weapon == WP_DET_PACK && ammo < 1 && ps.field_i32("hasDetPackPlanted").unwrap_or(0) == 0 {
            return false;
        }
        ps.stats[4] & (1 << weapon) != 0
    }

    fn can_select(ps: &PlayerState) -> bool {
        const PMF_FOLLOW: i32 = 4096;
        const PM_SPECTATOR: i32 = 4;
        ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW == 0
            && ps.field_i32("pm_type").unwrap_or(0) != PM_SPECTATOR
            && ps.field_i32("emplacedIndex").unwrap_or(0) == 0
    }

    /// CG_NextWeapon_f / CG_PrevWeapon_f.
    pub fn cycle_weapon(&mut self, ps: &PlayerState, forward: bool) {
        const WP_FLECHETTE: i32 = 10;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_DET_PACK: i32 = 14;
        const WP_CONCUSSION: i32 = 15;
        const WP_BRYAR_OLD: i32 = 16;
        const WP_NUM_WEAPONS: i32 = 19;
        if !Self::can_select(ps) {
            return;
        }
        let original = self.weapon_select;
        let mut weapon = i32::from(self.weapon_select);
        for _ in 0..WP_NUM_WEAPONS {
            weapon = if forward {
                match weapon {
                    WP_FLECHETTE => WP_CONCUSSION,
                    WP_CONCUSSION => WP_ROCKET_LAUNCHER,
                    WP_DET_PACK => WP_BRYAR_OLD,
                    w => w + 1,
                }
            } else {
                match weapon {
                    WP_ROCKET_LAUNCHER => WP_CONCUSSION,
                    WP_CONCUSSION => WP_FLECHETTE,
                    WP_BRYAR_OLD => WP_DET_PACK,
                    w => w - 1,
                }
            };
            if weapon >= WP_NUM_WEAPONS {
                weapon = 0;
            }
            if weapon < 0 {
                weapon = WP_NUM_WEAPONS - 1;
            }
            if Self::selectable(ps, weapon) {
                self.weapon_select = weapon as u8;
                return;
            }
        }
        self.weapon_select = original;
    }

    /// CG_OutOfAmmoChange with cg_autoSwitch 1 ("safe"): pick the highest
    /// selectable weapon that is not the one that just ran dry, skipping the
    /// explosive/placed weapons.
    pub fn out_of_ammo_change(&mut self, ps: &PlayerState, old_weapon: i32, auto_switch: u8) {
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_THERMAL: i32 = 12;
        const WP_TRIP_MINE: i32 = 13;
        const WP_DET_PACK: i32 = 14;
        const LAST_USEABLE_WEAPON: i32 = 16;
        for weapon in (1..=LAST_USEABLE_WEAPON).rev() {
            if auto_switch == 1 && matches!(weapon, WP_ROCKET_LAUNCHER | WP_THERMAL | WP_TRIP_MINE | WP_DET_PACK) {
                continue;
            }
            if weapon != old_weapon && Self::selectable(ps, weapon) {
                self.weapon_select = weapon as u8;
                return;
            }
        }
    }

    /// CG_ItemPickup's weapon auto-select: switch to a freshly picked-up weapon
    /// (`tag`) that is better than the current one, never away from the saber.
    pub fn pickup_autoswitch(&mut self, ps: &PlayerState, tag: i32, auto_switch: u8) {
        const WP_SABER: i32 = 3;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_THERMAL: i32 = 12;
        const WP_TRIP_MINE: i32 = 13;
        const WP_DET_PACK: i32 = 14;
        let current = ps.field_i32("weapon").unwrap_or(0);
        let better = tag > current && current != WP_SABER;
        let allowed = match auto_switch {
            0 => false,
            1 => better && !matches!(tag, WP_TRIP_MINE | WP_DET_PACK | WP_THERMAL | WP_ROCKET_LAUNCHER),
            _ => better,
        };
        if allowed && (0..19).contains(&tag) {
            self.weapon_select = tag as u8;
        }
    }

    /// OpenJK BG_CycleForce, used by CG_NextForcePower_f / CG_PrevForcePower_f.
    pub fn cycle_force(&mut self, ps: &PlayerState, forward: bool) {
        // codemp/game/bg_misc.c forcePowerSorted[] -- keep OpenJK's authored order.
        const FORCE_POWER_SORTED: [i32; 18] = [5, 0, 10, 9, 11, 1, 2, 3, 4, 14, 7, 13, 8, 6, 12, 15, 16, 17];
        const FP_LEVITATION: i32 = 1;
        const FP_SABER_OFFENSE: i32 = 15;
        const FP_SABER_DEFENSE: i32 = 16;
        const FP_SABERTHROW: i32 = 17;

        let selected = i32::from(self.force_select);
        if !(0..FORCE_POWER_SORTED.len() as i32).contains(&selected) {
            return;
        }
        let Some(start) = FORCE_POWER_SORTED.iter().position(|&power| power == selected) else {
            return;
        };
        let known = ps.field_i32("fd.forcePowersKnown").unwrap_or(0);
        let mut index = start;
        loop {
            index = if forward {
                (index + 1) % FORCE_POWER_SORTED.len()
            } else if index == 0 {
                FORCE_POWER_SORTED.len() - 1
            } else {
                index - 1
            };
            if index == start {
                return;
            }
            let power = FORCE_POWER_SORTED[index];
            if known & (1 << power) == 0 || power == selected {
                continue;
            }
            if matches!(power, FP_LEVITATION | FP_SABER_OFFENSE | FP_SABER_DEFENSE | FP_SABERTHROW) {
                continue;
            }
            self.force_select = power as u8;
            return;
        }
    }

    /// Vanilla CG_Weapon_f. Returns true when it turned into sv_saberswitch.
    pub fn select_weapon_slot(&mut self, ps: &PlayerState, argument: &str) -> bool {
        const WP_STUN_BATON: i32 = 1;
        const WP_MELEE: i32 = 2;
        const WP_SABER: i32 = 3;
        const WP_THERMAL: i32 = 12;
        const WP_DET_PACK: i32 = 14;
        const LAST_USEABLE_WEAPON: i32 = 16;
        if ps.field_i32("pm_flags").unwrap_or(0) & 4096 != 0 || ps.field_i32("emplacedIndex").unwrap_or(0) != 0 {
            return false;
        }
        let mut num = atoi(argument.as_bytes());
        if num < 1 || num > LAST_USEABLE_WEAPON {
            return false;
        }
        if num == 1 && ps.field_i32("weapon") == Some(WP_SABER) {
            if ps.field_i32("weaponTime").unwrap_or(0) < 1 {
                self.queue_generic_command(1);
                return true;
            }
            return false;
        }
        if num > WP_STUN_BATON {
            num += 2;
        } else {
            num = if ps.stats[4] & (1 << WP_SABER) != 0 { WP_SABER } else { WP_MELEE };
        }
        if num > LAST_USEABLE_WEAPON + 1 {
            return false;
        }
        if (WP_THERMAL..=WP_DET_PACK).contains(&num) {
            let current = ps.field_i32("weapon").unwrap_or(0);
            let mut weapon = if (WP_THERMAL..=WP_DET_PACK).contains(&current) { current + 1 } else { WP_THERMAL };
            for _ in 0..=4 {
                if weapon > WP_DET_PACK {
                    weapon = WP_THERMAL;
                }
                if Self::selectable(ps, weapon) {
                    num = weapon;
                    break;
                }
                weapon += 1;
            }
        }
        if Self::selectable(ps, num) {
            self.weapon_select = num as u8;
        }
        false
    }
}

// ----------------------------------------------------------- prediction --

/// cgs.* / pmove cvars CG_PredictPlayerState reads from server info.
pub fn predict_settings(configstrings: &std::collections::BTreeMap<u16, Vec<u8>>, ps: &PlayerState) -> PredictSettings {
    let server = configstrings.get(&0).map(Vec::as_slice).unwrap_or_default();
    let system = configstrings.get(&1).map(Vec::as_slice).unwrap_or_default();
    let detected_mod = mod_support::ServerMod::detect(server);
    let japro = detected_mod == mod_support::ServerMod::Japro;
    let japlus = detected_mod == mod_support::ServerMod::Japlus;
    let int = |info: &[u8], key: &[u8], default: i32| info_value(info, key).map_or(default, atoi);
    let taystjk_info = int(server, b"taystJKinfo", 0);
    let legacy_fixes = configstrings.get(&36).and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| {
            let value = value.trim();
            if let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else if value.starts_with('0') && value.len() > 1 {
                u32::from_str_radix(value, 8).ok()
            } else { value.parse().ok() }
        }).unwrap_or(0);
    let tayst_movement = taystjk_info & mod_support::TAYSTJK_INFO_MOVEMENT_MASK != 0 || legacy_fixes != 0;
    // TaystJK also recognizes Raven SDK gamecode independently of gamename.
    let mut base_game = detected_mod == mod_support::ServerMod::Base;
    if info_value(server, b"g_saberWallDamageScale").is_some() {
        base_game = true;
    }
    // TaystJK servers make this authoritative when present: a `0` must be able
    // to override even a basejka-looking gamename that is backed by OpenJK.
    if let Some(value) = info_value(server, b"sv_legacyGameAPI") {
        base_game = atoi(value) != 0;
    }
    const CONTENTS_SOLID: i32 = 0x1;
    const CONTENTS_PLAYERCLIP: i32 = 0x10;
    const CONTENTS_BODY: i32 = 0x100;
    const CONTENTS_TERRAIN: i32 = 0x1000;
    const PM_SPECTATOR: i32 = 4;
    const PM_DEAD: i32 = 5;
    const TEAM_SPECTATOR: i32 = 3;
    let mut tracemask = CONTENTS_SOLID | CONTENTS_PLAYERCLIP | CONTENTS_BODY | CONTENTS_TERRAIN;
    let pm_type = ps.field_i32("pm_type").unwrap_or(0);
    if pm_type == PM_DEAD {
        tracemask &= !CONTENTS_BODY;
    }
    if ps.persistant[3] == TEAM_SPECTATOR || pm_type == PM_SPECTATOR {
        tracemask &= !CONTENTS_BODY;
    }
    PredictSettings {
        pmove_fixed: int(system, b"pmove_fixed", 0),
        pmove_msec: if japro { int(system, b"pmove_msec", 8).clamp(1, 66) } else { int(system, b"pmove_msec", 8).clamp(8, 33) },
        pmove_float: int(system, b"pmove_float", 0),
        gametype: int(server, b"g_gametype", 0),
        debug_melee: int(server, b"g_debugMelee", 0),
        // cg_servercmds.c: atoi(Info_ValueForKey("g_stepSlideFix")), so a server that
        // never advertises it (retail JKA) is predicted with the fix off.
        step_slide_fix: int(server, b"g_stepSlideFix", 0),
        no_spec_move: int(server, b"g_noSpecMove", 0),
        tracemask,
        no_footsteps: i32::from(int(server, b"dmflags", 0) & 32 != 0),
        // TaystJK's shared BG/Pmove backend contains the JA+/jaPRO branches.
        // Base/unknown stays on stock by default unless auto-detection or an
        // explicit cg_predictBackend selection chooses the shared backend.
        backend: i32::from(japro || japlus || tayst_movement),
        // Compact Rust-side identity consumed by native japro_configure:
        // 0 Base/other, 1 jaPRO, 2 JA+.
        server_mod: if japro { 1 } else if japlus { 2 } else { 0 },
        // TaystJK stores JA+ `jp_cinfo` here, while jaPRO mirrors `jcinfo`
        // into both cgs.cinfo and cgs.jcinfo.
        cinfo: if japlus { int(server, b"jp_cinfo", 0) } else if japro { int(server, b"jcinfo", 0) } else { 0 },
        jcinfo: if japro { int(server, b"jcinfo", 0) } else { 0 },
        jcinfo2: if japro { int(server, b"jcinfo2", 0) } else { 0 },
        // TaystJK deliberately reads feature flags independently of gamename.
        // This lets Base/other game modules advertise compatible behavior.
        taystjk_info,
        dmflags: int(server, b"dmflags", 0),
        hook_pull: if japro { int(server, b"g_hookStrength", 0) } else if japlus { 800 } else { 0 },
        // Current TaystJK parses `restricts` after mod detection rather than
        // gating it on jaPRO, so preserve any server that advertises it.
        restricts: int(server, b"restricts", 0),
        base_game: i32::from(base_game),
        plugin_disable: 1536,
        // CS_LEGACY_FIXES is a protocol-visible capability configstring, not a
        // jaPRO-only field. The TaystJK backend is responsible for deciding
        // which fixes actually affect the current server/mod.
        legacy_fixes,
    }
}

fn to_native(ps: &PlayerState) -> NetworkPlayerState {
    NetworkPlayerState {
        fields: ps.fields.clone(),
        stats: ps.stats,
        persistant: ps.persistant,
        ammo: ps.ammo,
        powerups: ps.powerups,
    }
}

fn from_native(state: NetworkPlayerState) -> PlayerState {
    let mut player = PlayerState::default();
    player.fields = state.fields;
    player.stats = state.stats;
    player.persistant = state.persistant;
    player.ammo = state.ammo;
    player.powerups = state.powerups;
    player
}

pub(crate) fn movement_cmd(cmd: &UserCmd) -> MovementCmd {
    MovementCmd {
        server_time: cmd.server_time,
        angles: cmd.angles,
        buttons: cmd.buttons,
        weapon: cmd.weapon,
        force_selection: cmd.force_selection,
        inventory_selection: cmd.inventory_selection,
        generic_command: cmd.generic_command,
        forward_move: cmd.forward_move,
        right_move: cmd.right_move,
        up_move: cmd.up_move,
    }
}

fn origin(ps: &PlayerState) -> [f32; 3] {
    [
        ps.field_f32("origin[0]").unwrap_or(0.0),
        ps.field_f32("origin[1]").unwrap_or(0.0),
        ps.field_f32("origin[2]").unwrap_or(0.0),
    ]
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PredictionStateDebug {
    pub command_time: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub ground_entity: i32,
    pub pm_flags: i32,
    pub pm_type: i32,
    pub pm_time: i32,
    pub gravity: i32,
    pub speed: f32,
    pub view_angles: [f32; 3],
    pub client_num: i32,
    pub e_flags: i32,
    pub legs_anim: i32,
    pub torso_anim: i32,
    pub in_air_anim: i32,
    pub model_scale: i32,
}

pub(crate) fn prediction_state_debug(ps: &PlayerState) -> PredictionStateDebug {
    PredictionStateDebug {
        command_time: ps.field_i32("commandTime").unwrap_or(0),
        origin: origin(ps),
        velocity: [
            ps.field_f32("velocity[0]").unwrap_or(0.0),
            ps.field_f32("velocity[1]").unwrap_or(0.0),
            ps.field_f32("velocity[2]").unwrap_or(0.0),
        ],
        ground_entity: ps.field_i32("groundEntityNum").unwrap_or(-1),
        pm_flags: ps.field_i32("pm_flags").unwrap_or(0),
        pm_type: ps.field_i32("pm_type").unwrap_or(0),
        pm_time: ps.field_i32("pm_time").unwrap_or(0),
        gravity: ps.field_i32("gravity").unwrap_or(0),
        speed: ps.field_f32("speed").unwrap_or(0.0),
        view_angles: ["viewangles[0]", "viewangles[1]", "viewangles[2]"]
            .map(|name| ps.field_f32(name).unwrap_or(0.0)),
        client_num: ps.field_i32("clientNum").unwrap_or(0),
        e_flags: ps.field_i32("eFlags").unwrap_or(0),
        legs_anim: ps.field_i32("legsAnim").unwrap_or(0),
        torso_anim: ps.field_i32("torsoAnim").unwrap_or(0),
        in_air_anim: ps.field_i32("inAirAnim").unwrap_or(0),
        model_scale: ps.field_i32("iModelScale").unwrap_or(0),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PredictionTraceDebug {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub fraction: f32,
    pub hit_end: [f32; 3],
    pub normal: [f32; 3],
    pub entity: i32,
    pub start_solid: bool,
    pub all_solid: bool,
}

impl PredictionTraceDebug {
    fn from_trace(query: TraceQuery, result: TraceResult) -> Self {
        Self {
            start: query.start,
            end: query.end,
            fraction: result.fraction,
            hit_end: result.end,
            normal: result.normal,
            entity: result.entity,
            start_solid: result.start_solid != 0,
            all_solid: result.all_solid != 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PredictionMissDebug {
    pub sequence: u64,
    pub detected_at: Instant,
    pub command_time: i32,
    pub delta: [f32; 3],
    pub length: f32,
    pub predicted: PredictionStateDebug,
    pub server: PredictionStateDebug,
    /// Ground trace from the previous predicted frame: this is the trace that
    /// may have produced the state which is now being corrected.
    pub previous_ground_trace: Option<PredictionTraceDebug>,
    /// 64-unit look-down trace used by PM_GroundTraceMissed, when present.
    pub previous_ground_probe: Option<PredictionTraceDebug>,
    /// Ground trace from the replay which follows this correction.
    pub replay_ground_trace: Option<PredictionTraceDebug>,
    pub replay_ground_probe: Option<PredictionTraceDebug>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PredictionFrameDebug {
    pub server_time: i32,
    /// The actual replay base, which can be the next snapshot.
    pub base_message: i32,
    pub base_time: i32,
    pub base: PredictionStateDebug,
    pub settings: PredictSettings,
    pub display: PredictionStateDebug,
    pub committed: PredictionStateDebug,
    pub view_error: [f32; 3],
    pub ground_trace: Option<PredictionTraceDebug>,
    pub ground_probe: Option<PredictionTraceDebug>,
}

const EF_TELEPORT_BIT: i32 = 1 << 3;

/// One entry of cg_solidEntities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidEntity {
    pub number: i32,
    pub generic_enemy_index: i32,
    pub skip_movement: bool,
    pub clip: EntityClip,
    /// World-space bounds of the clip volume (`r.absmin`/`r.absmax`). Traces
    /// that stay clear of them skip the entity, like SV_AreaEntities does;
    /// `None` always tests it (CG_ClipMoveToEntities).
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

/// CG_BuildSolidList over this frame's lerped entities: triggers and items
/// are not solid; `solid` is either SOLID_BMODEL or an encoded bbox. Brush
/// models sit where their trajectory puts them at `physics_time`
/// (CG_ClipMoveToEntities: `BG_EvaluateTrajectory(&currentState.pos,
/// cg.physicsTime)`), the time the predicted commands are replayed from, not
/// at the smoothed render time.
pub fn solid_entities(entities: &[crate::cgame::PresentedEntity], physics_time: i32) -> Vec<SolidEntity> {
    const ET_ITEM: i32 = 2;
    const ET_PUSH_TRIGGER: i32 = 10;
    const ET_TELEPORT_TRIGGER: i32 = 11;
    const SOLID_BMODEL: i32 = 0x00ff_ffff;
    entities
        .iter()
        .filter(|entity| !matches!(entity.entity_type, ET_ITEM | ET_PUSH_TRIGGER | ET_TELEPORT_TRIGGER))
        .filter_map(|entity| {
            let solid = entity.state.field_i32("solid").unwrap_or(0);
            if solid == 0 {
                return None;
            }
            let clip = if solid == SOLID_BMODEL {
                EntityClip::InlineModel {
                    index: entity.state.field_i32("modelindex").unwrap_or(0),
                    origin: crate::cgame::evaluate_entity_trajectory(&entity.state, "pos", physics_time).unwrap_or(entity.origin),
                    angles: entity.angles,
                }
            } else {
                // Encoded bbox: x, down, up - 32.
                let x = (solid & 255) as f32;
                let zd = ((solid >> 8) & 255) as f32;
                let zu = ((solid >> 16) & 255) as f32 - 32.0;
                EntityClip::Box { mins: [-x, -x, -zd], maxs: [x, x, zu], origin: entity.origin }
            };
            Some(SolidEntity {
                number: i32::from(entity.number),
                generic_enemy_index: entity.state.field_i32("genericenemyindex").unwrap_or(0),
                skip_movement: false,
                clip,
                bounds: None,
            })
        })
        .collect()
}

const ET_MOVER: i32 = 6;
const ENTITYNUM_MAX_NORMAL: i32 = 1022;
const PMF_FOLLOW_FLAG: i32 = 4096;
const PERS_TEAM: usize = 3;
const TEAM_SPECTATOR: i32 = 3;
/// jaPRO `STAT_RACEMODE`, `STAT_MOVEMENTSTYLE` and the `MV_OCPM` entry of `movementStyle_e`.
const STAT_RACEMODE: usize = 11;
const STAT_MOVEMENTSTYLE: usize = 13;
const MV_OCPM: i32 = 16;

/// CG_AdjustPositionForMover: carry `position` with the mover it stands on
/// from `from_time` to `to_time`. A ground entity that is not an ET_MOVER, and
/// spectators or followers, are left where they are. Rotation is not applied,
/// like the original ("FIXME: origin change when on a rotating object").
pub fn adjust_position_for_mover(
    entities: &[crate::cgame::PresentedEntity],
    ps: &PlayerState,
    position: [f32; 3],
    from_time: i32,
    to_time: i32,
) -> [f32; 3] {
    if ps.persistant[PERS_TEAM] == TEAM_SPECTATOR || ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW_FLAG != 0 {
        return position;
    }
    let mover_num = ps.field_i32("groundEntityNum").unwrap_or(ENTITYNUM_MAX_NORMAL);
    if mover_num <= 0 || mover_num >= ENTITYNUM_MAX_NORMAL {
        return position;
    }
    let Some(mover) = entities
        .iter()
        .find(|entity| i32::from(entity.number) == mover_num && entity.entity_type == ET_MOVER)
    else {
        return position;
    };
    let evaluate = |time| crate::cgame::evaluate_entity_trajectory(&mover.state, "pos", time);
    let (Some(old), Some(new)) = (evaluate(from_time), evaluate(to_time)) else {
        return position;
    };
    std::array::from_fn(|axis| position[axis] + (new[axis] - old[axis]))
}

/// The provisional step repeats held inputs a few ms past a command that has
/// already run them. Its discrete results (a jump or flip animation started by
/// a held or tapped jump key) are taken back by the next real command, which
/// shows up as a one-frame pose flash on the model. Movement stays predicted;
/// the model's animation follows the committed state.
fn keep_committed_animation(display: &mut PlayerState, committed: &PlayerState) {
    let get = |ps: &PlayerState, name: &str| ps.field_i32(name).unwrap_or(0);
    let (display_legs, display_torso) = (get(display, "legsAnim"), get(display, "torsoAnim"));
    if display_legs == get(committed, "legsAnim") {
        return;
    }
    // BOTH_ animations drive legs and torso together; leave an independent
    // torso (saber attacks, weapon fire) alone.
    let both = display_legs == display_torso;
    let copied: &[&str] = if both {
        &["legsAnim", "legsTimer", "torsoAnim", "torsoTimer"]
    } else {
        &["legsAnim", "legsTimer"]
    };
    for name in copied {
        let value = get(committed, name);
        display.set_field_bits(name, value as u32);
    }
}

fn set_origin(ps: &mut PlayerState, origin: [f32; 3]) {
    for (axis, value) in origin.into_iter().enumerate() {
        ps.set_field_bits(&format!("origin[{axis}]"), value.to_bits());
    }
}

/// CG_Trace / CG_PointContents: the world plus CG_ClipMoveToEntities.
pub struct PredictionWorld<'a> {
    pub world: &'a mut CollisionWorld,
    pub solids: &'a [SolidEntity],
    pub client_num: i32,
}

impl TraceWorld for PredictionWorld<'_> {
    fn trace(&mut self, query: TraceQuery) -> TraceResult {
        const MAX_CLIENTS: i32 = 32;
        const MAX_GENTITIES: i32 = 1024;
        let mut result = self.world.trace(query);
        for solid in self.solids {
            if solid.skip_movement || solid.number == query.pass_entity {
                continue;
            }
            if let Some((low, high)) = solid.bounds {
                let clear = (0..3).any(|axis| {
                    let (start, end) = (query.start[axis], query.end[axis]);
                    start.min(end) + query.mins[axis] > high[axis] || start.max(end) + query.maxs[axis] < low[axis]
                });
                if clear {
                    continue;
                }
            }
            // Objects owned by the predicted client never block it.
            if solid.number > MAX_CLIENTS && solid.generic_enemy_index - MAX_GENTITIES == self.client_num {
                continue;
            }
            let mut trace = self.world.trace_entity(query, solid.clip);
            trace.entity = if trace.fraction != 1.0 { solid.number } else { jka_movement::ENTITY_NONE };
            if trace.all_solid != 0 || trace.fraction < result.fraction {
                trace.entity = solid.number;
                result = trace;
            } else if trace.start_solid != 0 {
                result.start_solid = 1;
                result.entity = solid.number;
            }
            if result.all_solid != 0 {
                return result;
            }
        }
        result
    }

    fn point_contents(&mut self, point: [f32; 3], pass_entity: i32) -> i32 {
        let mut contents = self.world.point_contents(point, pass_entity);
        for solid in self.solids {
            if solid.number == pass_entity {
                continue;
            }
            if let EntityClip::InlineModel { index, origin, angles } = solid.clip {
                if index > 0 {
                    contents |= self.world.inline_model_contents(point, index, origin, angles);
                }
            }
        }
        contents
    }
}

/// Thin diagnostic wrapper around the real CG_Trace implementation. It does
/// not alter collision results; it only remembers the characteristic traces
/// used by OpenJK PM_GroundTrace (0.25 units down) and
/// PM_GroundTraceMissed (64 units down).
struct PredictionTraceWorld<'a, 'b> {
    inner: PredictionWorld<'a>,
    ground_trace: &'b mut Option<PredictionTraceDebug>,
    ground_probe: &'b mut Option<PredictionTraceDebug>,
    enabled: bool,
}

impl TraceWorld for PredictionTraceWorld<'_, '_> {
    fn trace(&mut self, query: TraceQuery) -> TraceResult {
        let result = self.inner.trace(query);
        if !self.enabled {
            return result;
        }
        let same_xy = (query.start[0] - query.end[0]).abs() < 0.001
            && (query.start[1] - query.end[1]).abs() < 0.001;
        let down = query.start[2] - query.end[2];
        if same_xy && (down - 0.25).abs() < 0.001 {
            *self.ground_trace = Some(PredictionTraceDebug::from_trace(query, result));
        } else if same_xy && (down - 64.0).abs() < 0.01 {
            *self.ground_probe = Some(PredictionTraceDebug::from_trace(query, result));
        }
        result
    }

    fn point_contents(&mut self, point: [f32; 3], pass_entity: i32) -> i32 {
        self.inner.point_contents(point, pass_entity)
    }
}

/// cg.predictedPlayerState and its error-decay bookkeeping.
#[derive(Default)]
pub struct Predictor {
    native: Option<NativePlayerState>,
    /// cg.predictedPlayerState from committed usercmds only; the base for
    /// OpenJK's prediction-error detection.
    predicted: Option<PlayerState>,
    /// What this frame shows: `predicted` advanced by the provisional
    /// command to the current cl.serverTime, when one was run.
    display: Option<PlayerState>,
    /// usercmd angles `display` was predicted with.
    display_angles: Option<[i32; 3]>,
    /// Host-only OpenJK Pmove result. Kept out of protocol PlayerState.
    predicted_view_forced: bool,
    display_view_forced: bool,
    predicted_error: [f32; 3],
    predicted_error_time: i32,
    old_time: i32,
    last_snapshot: Option<(i32, i32, i32)>,
    this_frame_teleport: bool,
    /// `cgs.clientinfo[predicted client].saber[]`, which BG_MySaber hands to Pmove.
    saber_movement: Option<[SaberMovementInfo; 2]>,
    /// The loadout the native player currently carries.
    saber_installed: Option<[SaberMovementInfo; 2]>,
    /// Foot bolts of the predicted client's rendered Ghoul2 pose (`pmove_t::ghoul2`).
    foot_bolts: Option<[[f32; 3]; 2]>,
    pub misses: Vec<String>,
    miss_sequence: u64,
    latest_miss: Option<PredictionMissDebug>,
    debug_frame: Option<PredictionFrameDebug>,
    last_ground_trace: Option<PredictionTraceDebug>,
    last_ground_probe: Option<PredictionTraceDebug>,
    last_view_error: [f32; 3],
    /// What the last replay ran with, for the `predsettings` console command.
    last_regime: Option<(PredictSettings, i32, i32)>,
    /// `cg_groundTraceDebug`: what each new command's ground trace predicted, kept until the
    /// server's snapshot for that command time arrives and can be compared against it.
    ground_log: std::collections::VecDeque<GroundRecord>,
    ground_logged_cmd: i32,
    ground_compared_snapshot: i32,
    ground_previous: i32,
    /// `cg_physicsDiag`: the previous base snapshot (message number, state) and running totals.
    diag_previous: Option<(i32, PlayerState)>,
    diag_compared: u32,
    diag_matched: u32,
    /// Per candidate rounding mode: how many compared intervals it reproduced exactly, and the
    /// votes (intervals where it was exact and at least one other mode was not).
    diag_combo_matched: [u32; 10],
    combo_votes: [u32; 10],
    /// What detection currently selects, and the rounding mode last handed to the native side.
    auto_backend: i32,
    auto_snap: i32,
    snap_mode_applied: Option<i32>,
}

/// One predicted command's ground state, for `cg_groundTraceDebug`.
#[derive(Debug, Clone, Copy)]
struct GroundRecord {
    cmd_number: i32,
    cmd_time: i32,
    origin: [f32; 3],
    velocity: [f32; 3],
    ground_entity: i32,
    pm_flags: i32,
    pm_time: i32,
    legs_anim: i32,
    trace: Option<PredictionTraceDebug>,
    probe: Option<PredictionTraceDebug>,
}

/// Replay of one snapshot interval compared with the server's state (see `cg_physicsDiag`).
struct IntervalCheck {
    exact: bool,
    origin_delta: [f32; 3],
    velocity_delta: [f32; 3],
    ground: (i32, i32),
    flags: (i32, i32),
    timer: (i32, i32),
}

fn check_interval(replayed: &PlayerState, server: &PlayerState) -> IntervalCheck {
    let delta = |name: &str| replayed.field_f32(name).unwrap_or(0.0) - server.field_f32(name).unwrap_or(0.0);
    let origin_delta = [delta("origin[0]"), delta("origin[1]"), delta("origin[2]")];
    let velocity_delta = [delta("velocity[0]"), delta("velocity[1]"), delta("velocity[2]")];
    let pair = |name: &str, default: i32| (replayed.field_i32(name).unwrap_or(default), server.field_i32(name).unwrap_or(default));
    let (ground, flags, timer) = (pair("groundEntityNum", -1), pair("pm_flags", 0), pair("pm_time", 0));
    let magnitude = (origin_delta[0].powi(2) + origin_delta[1].powi(2) + origin_delta[2].powi(2)).sqrt();
    let exact = magnitude < 0.01
        && velocity_delta.iter().all(|v| v.abs() < 0.01)
        && ground.0 == ground.1
        && flags.0 == flags.1
        && timer.0 == timer.1;
    IntervalCheck { exact, origin_delta, velocity_delta, ground, flags, timer }
}

/// Index of a (backend, rounding) pair in the vote arrays.
fn combo_index(backend: i32, snap: i32) -> usize {
    (backend.clamp(0, 1) * 5 + snap.clamp(0, 4)) as usize
}

fn combo_name(backend: i32, snap: i32) -> String {
    let backend = if backend == 1 { "TaystJK backend" } else { "stock backend" };
    let snap = ["nearest", "truncate", "floor", "nearest-even", "no rounding"][snap.clamp(0, 4) as usize];
    format!("{backend}, {snap} rounding")
}

/// `cg_groundTraceDebug 1` reports a snapshot comparison once the origin differs by more than this.
const DRIFT_UNITS: f32 = 3.0;

fn ground_trace_text(label: &str, trace: Option<PredictionTraceDebug>) -> String {
    match trace {
        Some(t) => format!(
            "{label} z {:.2}->{:.2} frac={:.4} hit=({:.2},{:.2},{:.2}) ent={} nz={:.3}{}{}",
            t.start[2], t.end[2], t.fraction, t.hit_end[0], t.hit_end[1], t.hit_end[2], t.entity, t.normal[2],
            if t.start_solid { " STARTSOLID" } else { "" },
            if t.all_solid { " ALLSOLID" } else { "" },
        ),
        None => format!("{label} -"),
    }
}

pub struct PredictionInput<'a> {
    pub session: &'a ClientSession,
    pub snap: &'a Snapshot,
    pub next: Option<&'a Snapshot>,
    pub time: i32,
    pub movement: &'a PmoveContext,
    pub world: &'a mut CollisionWorld,
    /// This frame's lerped packet entities (for CG_BuildSolidList).
    pub entities: &'a [crate::cgame::PresentedEntity],
    pub settings: &'a NetworkSettings,
    /// Current input as a not-yet-sent usercmd stamped with cl.serverTime.
    /// OpenJK builds a real usercmd every client frame; we pace real ones
    /// (cl_commandRate) and predict this one only for display.
    pub provisional: Option<UserCmd>,
}

impl Predictor {
    pub fn reset(&mut self) {
        *self = Self::default();
        // The rounding mode is process-wide native state; solo play and the next server start at
        // OpenJK's and detection starts over.
        jka_movement::set_snap_mode(0);
    }

    /// Install the predicted client's equipped sabers (what CG_NewClientInfo's
    /// WP_SetSaber leaves in `cgs.clientinfo[].saber[]`). Pmove consults them
    /// through BG_MySaber for stances, special attacks and style rules.
    pub fn set_saber_movement(&mut self, sabers: [SaberMovementInfo; 2]) {
        self.saber_movement = Some(sabers.map(SaberMovementInfo::for_prediction));
    }

    /// The predicted client's `*l_leg_foot` / `*r_leg_foot` in model space, from
    /// the last presented pose. PM_AdjustStandAnimForSlope (leg dangle) needs
    /// them; without a model Pmove skips the slope anims like a spectator.
    pub fn set_foot_bolts(&mut self, bolts: Option<[[f32; 3]; 2]>) {
        self.foot_bolts = bolts;
    }

    /// One line per setting the last replay ran with (`predsettings`), including
    /// which rules of PmoveSingle's end-of-frame velocity snap and Pmove()'s
    /// command chopping apply under them.
    pub fn regime_report(&self, command_rate: &str) -> Vec<String> {
        const MV_NAMES: [&str; 19] = [
            "siege", "jka", "qw", "cpm", "q3", "pjk", "wsw", "rjq3", "rjcpm", "swoop", "jetpack", "speed", "sp",
            "slick", "botcpm", "coop", "ocpm", "tribes", "surf",
        ];
        const JAPRO_CINFO_HIGHFPSFIX: i32 = 1 << 21;
        let Some((settings, race, style)) = self.last_regime else {
            return vec!["predsettings: no prediction has run yet (connect to a server first)".to_owned()];
        };
        let race = race != 0 && settings.server_mod == 1;
        let ocpm = style == MV_OCPM;
        let style_name = usize::try_from(style).ok().and_then(|i| MV_NAMES.get(i)).copied().unwrap_or("?");
        let snap = if race && !ocpm || (!race && settings.pmove_float > 1) {
            "OFF (float velocity)"
        } else if settings.pmove_float != 0 {
            "OFF (pmove_float 1)"
        } else if settings.jcinfo & JAPRO_CINFO_HIGHFPSFIX != 0 {
            "ON, except when a command's msec is <4 or >25 (g_fixHighFPSAbuse)"
        } else {
            "ON (integer velocity every step)"
        };
        let chop = if race {
            if ocpm {
                "race+OCPM: every step chopped to 8 ms; server also rounds each cmd time up to a multiple of 8 (this client does not, like the stock cgame)"
            } else {
                "race: steps are the full command msec (rolls chopped to 8); server rounds cmd time to 3 ms when msec<3 (this client does not, like the stock cgame)"
            }
        } else if settings.pmove_fixed != 0 {
            "pmove_fixed: steps chopped to pmove_msec; cmd times rounded up to pmove_msec"
        } else {
            "steps chopped to 66 ms"
        };
        vec![
            format!(
                "predsettings: backend={} server_mod={} race={} style={style_name}({style}) pmove_fixed={} pmove_float={} pmove_msec={} cl_commandRate={command_rate}",
                settings.backend, settings.server_mod, u8::from(race), settings.pmove_fixed, settings.pmove_float, settings.pmove_msec
            ),
            format!("  velocity snap: {snap}"),
            format!("  step chopping: {chop}"),
            format!(
                "  jp_cinfo={:#x} jcinfo={:#x} jcinfo2={:#x} taystJKinfo={:#x} step_slide_fix={} provisional_step={}",
                settings.cinfo,
                settings.jcinfo,
                settings.jcinfo2,
                settings.taystjk_info,
                settings.step_slide_fix,
                if settings.pmove_fixed == 0 { "on" } else { "off (pmove_fixed)" },
            ),
            "  not predicted here: push triggers / jump pads, teleporter touch, item pickups (CG_TouchTriggerPrediction)".to_owned(),
        ]
    }

    /// The playerstate to present this frame (see `display`).
    pub fn predicted(&self) -> Option<&PlayerState> {
        self.display.as_ref().or(self.predicted.as_ref())
    }

    /// usercmd angles behind [`Self::predicted`].
    pub fn predicted_command_angles(&self) -> Option<[i32; 3]> {
        self.display_angles
    }

    /// True when the Pmove step backing the presented state called
    /// OpenJK PM_SetPMViewAngle for the local player.
    pub fn view_forced(&self) -> bool {
        if self.display.is_some() {
            self.display_view_forced
        } else {
            self.predicted.is_some() && self.predicted_view_forced
        }
    }

    /// CG_CalcViewValues: the decaying prediction error added to the view.
    pub fn view_error(&mut self, time: i32, error_decay: f32) -> [f32; 3] {
        let error = if error_decay <= 0.0 {
            [0.0; 3]
        } else {
            let f = (error_decay - (time - self.predicted_error_time) as f32) / error_decay;
            if f > 0.0 && f < 1.0 {
                self.predicted_error.map(|e| e * f)
            } else {
                self.predicted_error_time = 0;
                [0.0; 3]
            }
        };
        self.last_view_error = error;
        if let Some(frame) = &mut self.debug_frame {
            frame.view_error = error;
        }
        error
    }

    pub fn prediction_debug_frame(&self) -> Option<PredictionFrameDebug> {
        self.debug_frame
    }

    pub fn latest_prediction_miss(&self) -> Option<&PredictionMissDebug> {
        self.latest_miss.as_ref()
    }

    /// CG_PredictPlayerState (vanilla, no vehicles).
    pub fn predict(&mut self, input: PredictionInput<'_>) -> Result<(), String> {
        let PredictionInput { session, snap, next, time, movement, world, entities, settings, provisional } = input;
        let client_num = snap.player_state.field_i32("clientNum").unwrap_or(0);
        // cg.thisFrameTeleport is raised by CG_TransitionSnapshot when the
        // playerstate teleport bit or client number changes.
        let snap_key = (
            snap.message_num,
            snap.player_state.field_i32("eFlags").unwrap_or(0) & EF_TELEPORT_BIT,
            snap.player_state.field_i32("clientNum").unwrap_or(0),
        );
        if let Some(previous) = self.last_snapshot {
            if previous.0 != snap_key.0 && (previous.1 != snap_key.1 || previous.2 != snap_key.2) {
                self.this_frame_teleport = true;
            }
        }
        self.last_snapshot = Some(snap_key);
        // TaystJK CG_SetNextSnap marks the whole snapshot transition as a
        // no-interpolate boundary not only for EF_TELEPORT_BIT, but also when
        // follow clientNum changes or SNAPFLAG_SERVERCOUNT toggles. Prediction
        // must use the same gate before choosing nextSnap as its base/solid list.
        let next_frame_teleport = next
            .is_some_and(|next| crate::cgame::snapshot_discontinuity(snap, next));
        let old_time = self.old_time;
        self.old_time = time;

        if self.predicted.is_none() {
            self.predicted = Some(snap.player_state.clone());
        }
        const PMF_FOLLOW: i32 = 4096;
        if settings.no_predict || snap.player_state.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 {
            // CG_InterpolatePlayerState: callers use the interpolated snapshot.
            self.predicted = None;
            self.display = None;
            self.display_angles = None;
            self.predicted_view_forced = false;
            self.display_view_forced = false;
            self.debug_frame = None;
            self.last_view_error = [0.0; 3];
            self.latest_miss = None;
            self.last_ground_trace = None;
            self.last_ground_probe = None;
            return Ok(());
        }
        let old = self.predicted.clone().expect("seeded above");

        let current = session.cmd_number();
        let first = current - CMD_BACKUP as i32 + 1;
        let command = |number: i32| session.command(number).unwrap_or_default();
        let oldest = command(first);
        if oldest.server_time > snap.player_state.field_i32("commandTime").unwrap_or(0) && oldest.server_time < time {
            return Ok(());
        }
        let latest = command(current);
        let using_next = next.filter(|_| !next_frame_teleport && !self.this_frame_teleport);
        let base = using_next.unwrap_or(snap);
        let base_ps = &base.player_state;
        let mut prediction_settings = predict_settings(&session.decoder().configstrings, base_ps);
        prediction_settings.plugin_disable = settings.plugin_disable;
        // Backend and server identity are separate. JA+ and jaPRO use the
        // TaystJK backend by default. Base/other servers only require it for
        // movement-related taystJKinfo bits or CS_LEGACY_FIXES; presentation-only
        // flags (RGB/black sabers) must not silently change prediction.
        // Plain Base/unknown can still auto-detect stock vs Tayst from replay.
        let fixed_tayst_backend = prediction_settings.server_mod != 0
            || prediction_settings.taystjk_info & mod_support::TAYSTJK_INFO_MOVEMENT_MASK != 0
            || prediction_settings.legacy_fixes != 0;
        let effective_backend = if settings.predict_backend >= 0 {
            settings.predict_backend
        } else if fixed_tayst_backend {
            1
        } else {
            self.auto_backend
        };
        prediction_settings.backend = effective_backend;
        let effective_snap_mode = if settings.snap_mode >= 0 { settings.snap_mode } else { self.auto_snap };
        if self.snap_mode_applied != Some(effective_snap_mode) {
            jka_movement::set_snap_mode(effective_snap_mode);
            self.snap_mode_applied = Some(effective_snap_mode);
        }
        let regime = (prediction_settings, base_ps.stats[STAT_RACEMODE], base_ps.stats[STAT_MOVEMENTSTYLE]);
        if self.last_regime != Some(regime) {
            // This is diagnostic context, not normal gameplay output. Keep it visible when the
            // dedicated prediction diagnostics are enabled, or under the global verbose gate.
            let line = format!(
                "PRED-SETTINGS backend={} pmove_fixed={} pmove_float={} pmove_msec={} step_slide_fix={} gravity={} speed={:.3} race={} style={} jcinfo={:#x} base_snapshot={} cmdtime={}",
                if prediction_settings.backend == 1 { "TaystJK" } else { "stock" },
                prediction_settings.pmove_fixed,
                prediction_settings.pmove_float,
                prediction_settings.pmove_msec,
                prediction_settings.step_slide_fix,
                base_ps.field_i32("gravity").unwrap_or(0),
                base_ps.field_f32("speed").unwrap_or(0.0),
                regime.1,
                regime.2,
                prediction_settings.jcinfo,
                base.message_num,
                base_ps.field_i32("commandTime").unwrap_or(0),
            );
            if settings.prediction_debug {
                self.misses.push(line);
            } else {
                devprintln!(2, "{line}");
            }
        }
        self.last_regime = Some(regime);
        // cg.physicsTime: the snapshot time the replay starts from, which is
        // where movers are clipped and where the ground-mover adjustment starts.
        let mut physics_time = base.server_time;
        if prediction_settings.server_mod == 1
            && physics_time - base_ps.field_i32("commandTime").unwrap_or(0) > 8
            && base_ps.stats[STAT_MOVEMENTSTYLE] == MV_OCPM
        {
            physics_time = base_ps.field_i32("commandTime").unwrap_or(0) + 8;
        }
        let mut solids = solid_entities(entities, physics_time);
        let native = match &mut self.native {
            Some(native) => {
                native.configure(&prediction_settings)?;
                native.set_network(&to_native(base_ps))?;
                native
            }
            slot => {
                self.saber_installed = None;
                slot.insert(NativePlayerState::from_network(&to_native(base_ps))?)
            }
        };
        if self.saber_installed != self.saber_movement {
            if let Some(sabers) = self.saber_movement {
                native.set_saber_movement_info(0, sabers[0])?;
                native.set_saber_movement_info(1, sabers[1])?;
            }
            self.saber_installed = self.saber_movement;
        }

        native.set_foot_bolts(self.foot_bolts)?;
        native.configure(&prediction_settings)?;
        if prediction_settings.backend == 1 {
            let prediction_entities: Vec<_> = entities.iter().map(|entity| {
                let int = |key| entity.state.field_i32(key).unwrap_or(0);
                let float = |key| entity.state.field_f32(key).unwrap_or(0.0);
                jka_movement::PredictionEntity {
                    number: i32::from(entity.number), entity_type: entity.entity_type,
                    model_index: int("modelindex"), bolt1: int("bolt1"),
                    trajectory_type: int("pos.trType"),
                    origin: [float("pos.trBase[0]"), float("pos.trBase[1]"), float("pos.trBase[2]")],
                    velocity: [float("pos.trDelta[0]"), float("pos.trDelta[1]"), float("pos.trDelta[2]")],
                    angular_velocity: [float("apos.trDelta[0]"), float("apos.trDelta[1]"), float("apos.trDelta[2]")],
                    legs_anim: int("legsAnim"), torso_anim: int("torsoAnim"), saber_move: int("saberMove"),
                }
            }).collect();
            native.set_prediction_entities(&prediction_entities)?;
            let mut skip = [false; 1024];
            for entity in &prediction_entities {
                // set_prediction_entities checked the native entity-number bounds.
                skip[entity.number as usize] = !native.clips_prediction_entity(entity);
            }
            for solid in &mut solids {
                solid.skip_movement = skip[solid.number as usize];
            }
        }
        let debug_enabled = settings.prediction_debug
            || settings.prediction_miss_highlight
            || settings.hitch_record
            || settings.ground_trace_debug > 0;
        let mut frame_ground_trace = None;
        let mut frame_ground_probe = None;
        let mut world = PredictionTraceWorld {
            inner: PredictionWorld { world, solids: &solids, client_num },
            ground_trace: &mut frame_ground_trace,
            ground_probe: &mut frame_ground_probe,
            enabled: debug_enabled,
        };

        let mut moved = false;
        let mut replay_view_forced = self.predicted_view_forced;
        let mut command_time = base_ps.field_i32("commandTime").unwrap_or(0);
        let mut miss_sequence_this_frame = None;
        // CG_PredictPlayerState compares the previous frame's prediction with this replay at
        // the same commandTime. Stock does it while processing a *later* command, which always
        // exists there (one usercmd per frame). Here ticks outnumber commands ~7:1, so a snapshot
        // that lands on a tick with no new command was never compared: the correction was applied
        // raw (no miss, no cg_errorDecay smoothing), a hard pop of several to tens of units.
        // Capture the replayed state the moment it reaches that commandTime instead.
        let old_command_time = old.field_i32("commandTime").unwrap_or(0);
        let mut replayed_at_old: Option<PlayerState> = (command_time == old_command_time)
            .then(|| from_native(native.network()));
        for number in first..=current {
            let mut cmd = command(number);
            if prediction_settings.pmove_fixed != 0 {
                movement.update_view_angles(native, movement_cmd(&cmd));
            }
            if cmd.server_time <= command_time || cmd.server_time > latest.server_time {
                continue;
            }
            if prediction_settings.pmove_fixed != 0 {
                let msec = prediction_settings.pmove_msec;
                cmd.server_time = ((cmd.server_time + msec - 1) / msec) * msec;
            }
            let record_ground = settings.ground_trace_debug > 0 && number > self.ground_logged_cmd;
            if record_ground {
                *world.ground_trace = None;
                *world.ground_probe = None;
            }
            movement.predict(native, movement_cmd(&cmd), &prediction_settings, &mut world)?;
            let native_view = native.view();
            if record_ground {
                self.ground_logged_cmd = number;
                let record = GroundRecord {
                    cmd_number: number,
                    cmd_time: cmd.server_time,
                    origin: native_view.origin,
                    velocity: native_view.velocity,
                    ground_entity: native_view.ground_entity,
                    pm_flags: native_view.pm_flags,
                    pm_time: native_view.pm_time,
                    legs_anim: native_view.legs_anim,
                    trace: *world.ground_trace,
                    probe: *world.ground_probe,
                };
                let describe = |r: &GroundRecord| {
                    format!(
                        "cmd={} t={} ground={} o=({:.2},{:.2},{:.2}) v=({:.0},{:.0},{:.0}) pmf={:#x} pmt={} | {} | {}",
                        r.cmd_number, r.cmd_time, r.ground_entity, r.origin[0], r.origin[1], r.origin[2],
                        r.velocity[0], r.velocity[1], r.velocity[2], r.pm_flags, r.pm_time,
                        ground_trace_text("trace.25", r.trace), ground_trace_text("probe64", r.probe),
                    )
                };
                let changed = record.ground_entity != self.ground_previous;
                if settings.ground_trace_debug >= 2 || changed {
                    let tag = if changed { format!("GT {}->{}", self.ground_previous, record.ground_entity) } else { "GT".to_owned() };
                    self.misses.push(format!("^3{tag}^7 {}", describe(&record)));
                }
                self.ground_previous = record.ground_entity;
                self.ground_log.push_back(record);
                while self.ground_log.len() > 512 {
                    self.ground_log.pop_front();
                }
            }
            command_time = native_view.command_time;
            replay_view_forced = native_view.view_forced != 0;
            moved = true;
            if replayed_at_old.is_none() && command_time == old_command_time {
                replayed_at_old = Some(from_native(native.network()));
            }
        }
        if let Some(replayed) = replayed_at_old.take() {
            let command_time = old_command_time;
            if self.this_frame_teleport {
                self.predicted_error = [0.0; 3];
                self.this_frame_teleport = false;
            } else {
                // The mover under the replayed state has moved since last frame.
                let now = adjust_position_for_mover(entities, &replayed, origin(&replayed), physics_time, old_time);
                let before = origin(&old);
                let delta = [before[0] - now[0], before[1] - now[1], before[2] - now[2]];
                let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                if length > 0.1 {
                if debug_enabled {
                    self.miss_sequence = self.miss_sequence.wrapping_add(1);
                    let sequence = self.miss_sequence;
                    let mut server = prediction_state_debug(&replayed);
                    // Match the mover-adjusted position used by the actual
                    // prediction-error calculation above.
                    server.origin = now;
                    self.latest_miss = Some(PredictionMissDebug {
                        sequence,
                        detected_at: Instant::now(),
                        command_time,
                        delta,
                        length,
                        predicted: prediction_state_debug(&old),
                        server,
                        previous_ground_trace: self.last_ground_trace,
                        previous_ground_probe: self.last_ground_probe,
                        replay_ground_trace: None,
                        replay_ground_probe: None,
                    });
                    miss_sequence_this_frame = Some(sequence);
                }
                if settings.show_miss {
                    let describe = |ps: &PlayerState| {
                        let velocity = [
                            ps.field_f32("velocity[0]").unwrap_or(0.0),
                            ps.field_f32("velocity[1]").unwrap_or(0.0),
                            ps.field_f32("velocity[2]").unwrap_or(0.0),
                        ];
                        format!(
                            "o={:?} v={:?} ground={} pm_flags={:#x} legs={}",
                            origin(ps),
                            velocity,
                            ps.field_i32("groundEntityNum").unwrap_or(-1),
                            ps.field_i32("pm_flags").unwrap_or(0),
                            ps.field_i32("legsAnim").unwrap_or(0),
                        )
                    };
                    self.misses.push(format!(
                        "Prediction miss: {length} at commandTime {command_time} | predicted {} | server {}",
                        describe(&old),
                        describe(&replayed)
                    ));
                }
                if settings.prediction_miss_highlight && length >= settings.prediction_miss_threshold {
                    // The red screen-edge flash fired for this one: leave a timestamped line in the
                    // log saying what was corrected (the flash itself is not recorded anywhere).
                    let predicted = prediction_state_debug(&old);
                    let mut server = prediction_state_debug(&replayed);
                    server.origin = now;
                    self.misses.push(format!(
                        "^1PRED-FLASH^7 miss={length:.2}u delta=[{:.2},{:.2},{:.2}] cmd={command_time} | predicted o=({:.2},{:.2},{:.2}) v=({:.0},{:.0},{:.0}) ground={} pmf={:#x} legs={} | replay o=({:.2},{:.2},{:.2}) v=({:.0},{:.0},{:.0}) ground={} pmf={:#x} legs={}",
                        delta[0], delta[1], delta[2],
                        predicted.origin[0], predicted.origin[1], predicted.origin[2],
                        predicted.velocity[0], predicted.velocity[1], predicted.velocity[2],
                        predicted.ground_entity, predicted.pm_flags, predicted.legs_anim,
                        server.origin[0], server.origin[1], server.origin[2],
                        server.velocity[0], server.velocity[1], server.velocity[2],
                        server.ground_entity, server.pm_flags, server.legs_anim,
                    ));
                }
                if settings.prediction_debug {
                    let predicted = prediction_state_debug(&old);
                    let mut server = prediction_state_debug(&replayed);
                    server.origin = now;
                    self.misses.push(format!(
                        "PRED-DIAG miss={length:.3} delta=[{:.3},{:.3},{:.3}] cmd={} | pred o={:?} v={:?} ground={} pm_type={} flags={:#x} legs={} torso={} inAir={} scale={} | replay o={:?} v={:?} ground={} pm_type={} flags={:#x} legs={} torso={} inAir={} scale={}",
                        delta[0], delta[1], delta[2], command_time,
                        predicted.origin, predicted.velocity, predicted.ground_entity, predicted.pm_type,
                        predicted.pm_flags, predicted.legs_anim, predicted.torso_anim,
                        predicted.in_air_anim, predicted.model_scale,
                        server.origin, server.velocity, server.ground_entity, server.pm_type,
                        server.pm_flags, server.legs_anim, server.torso_anim,
                        server.in_air_anim, server.model_scale,
                    ));
                    if let Some(trace) = self.last_ground_trace {
                        self.misses.push(format!(
                            "PRED-DIAG previous groundTrace start={:?} end={:?} frac={:.4} hit={:?} normal={:?} ent={} startSolid={} allSolid={}",
                            trace.start, trace.end, trace.fraction, trace.hit_end, trace.normal,
                            trace.entity, trace.start_solid, trace.all_solid,
                        ));
                    }
                    if let Some(trace) = self.last_ground_probe {
                        self.misses.push(format!(
                            "PRED-DIAG previous groundProbe64 start={:?} end={:?} frac={:.4} hit={:?} normal={:?} ent={} startSolid={} allSolid={}",
                            trace.start, trace.end, trace.fraction, trace.hit_end, trace.normal,
                            trace.entity, trace.start_solid, trace.all_solid,
                        ));
                    }
                }
                if settings.error_decay > 0.0 {
                    let t = time - self.predicted_error_time;
                    let f = ((settings.error_decay - t as f32) / settings.error_decay).max(0.0);
                    self.predicted_error = self.predicted_error.map(|e| e * f);
                } else {
                    self.predicted_error = [0.0; 3];
                }
                for axis in 0..3 {
                    self.predicted_error[axis] += delta[axis];
                }
                self.predicted_error_time = old_time;
                }
            }
        }
        if settings.ground_trace_debug > 0 && base.message_num != self.ground_compared_snapshot {
            // The server's ground state for this snapshot's commandTime against what this client
            // predicted for that same command when it was first run.
            self.ground_compared_snapshot = base.message_num;
            let server_time = base_ps.field_i32("commandTime").unwrap_or(0);
            let server_ground = base_ps.field_i32("groundEntityNum").unwrap_or(-1);
            let server_origin = origin(base_ps);
            if let Some(record) = self.ground_log.iter().rev().find(|r| r.cmd_time == server_time).copied() {
                let delta = [
                    record.origin[0] - server_origin[0],
                    record.origin[1] - server_origin[1],
                    record.origin[2] - server_origin[2],
                ];
                let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                let grounded = |entity: i32| entity != 1023 && entity >= 0;
                let ground_differs = grounded(record.ground_entity) != grounded(server_ground)
                    || (grounded(server_ground) && record.ground_entity != server_ground);
                if ground_differs || distance > DRIFT_UNITS || settings.ground_trace_debug >= 2 {
                    let verdict = if ground_differs { "^1GT-MISMATCH" } else if distance > DRIFT_UNITS { "^3GT-DRIFT" } else { "^2GT-OK" };
                    self.misses.push(format!(
                        "{verdict}^7 snap#{} cmd={} t={} | predicted ground={} o=({:.2},{:.2},{:.2}) v=({:.0},{:.0},{:.0}) pmf={:#x} pmt={} legs={} | server ground={} o=({:.2},{:.2},{:.2}) v=({:.0},{:.0},{:.0}) pmf={:#x} pmt={} legs={} | delta=({:.2},{:.2},{:.2}) |d|={distance:.2} | {}",
                        base.message_num, record.cmd_number, server_time,
                        record.ground_entity, record.origin[0], record.origin[1], record.origin[2],
                        record.velocity[0], record.velocity[1], record.velocity[2],
                        record.pm_flags, record.pm_time, record.legs_anim,
                        server_ground, server_origin[0], server_origin[1], server_origin[2],
                        base_ps.field_f32("velocity[0]").unwrap_or(0.0),
                        base_ps.field_f32("velocity[1]").unwrap_or(0.0),
                        base_ps.field_f32("velocity[2]").unwrap_or(0.0),
                        base_ps.field_i32("pm_flags").unwrap_or(0),
                        base_ps.field_i32("pm_time").unwrap_or(0),
                        base_ps.field_i32("legsAnim").unwrap_or(0),
                        delta[0], delta[1], delta[2],
                        ground_trace_text("trace.25", record.trace),
                    ));
                }
            }
        }
        let replay_ground_trace = *world.ground_trace;
        let replay_ground_probe = *world.ground_probe;
        let backend_candidates: Vec<i32> = if settings.predict_backend >= 0 {
            vec![settings.predict_backend]
        } else if fixed_tayst_backend {
            vec![1]
        } else {
            vec![0, 1]
        };
        let snap_candidates: Vec<i32> = if settings.snap_mode >= 0 {
            vec![settings.snap_mode]
        } else if fixed_tayst_backend {
            vec![0]
        } else {
            vec![0, 1, 2]
        };
        let combos: Vec<(i32, i32)> = backend_candidates
            .iter()
            .flat_map(|&backend| snap_candidates.iter().map(move |&snap| (backend, snap)))
            .collect();
        let detecting = combos.len() > 1;
        if (settings.physics_diag > 0 || detecting)
            && self.diag_previous.as_ref().map_or(true, |(message, _)| *message != base.message_num)
        {
            // Does this client's pmove reproduce the server's? Replay exactly the commands the server
            // ran between the previous snapshot and this one, starting from the previous snapshot's
            // state, and compare with this snapshot. Any difference is a pure physics mismatch over
            // one snapshot interval, with no long replay chain, time sync or smoothing involved.
            // The same interval is replayed under every candidate (stock or TaystJK backend, each
            // velocity rounding) and the candidate that reproduces the server wins the vote: at 1 ms
            // steps the rounding, and any modded server's movement, decide friction and gravity.
            if let Some((_, previous)) = self.diag_previous.take() {
                let start = previous.field_i32("commandTime").unwrap_or(0);
                let end = base_ps.field_i32("commandTime").unwrap_or(0);
                let skipped = (previous.field_i32("eFlags").unwrap_or(0) ^ base_ps.field_i32("eFlags").unwrap_or(0))
                    & EF_TELEPORT_BIT
                    != 0
                    || previous.field_i32("clientNum") != base_ps.field_i32("clientNum")
                    || previous.field_i32("pm_type") != base_ps.field_i32("pm_type");
                if end > start && !skipped {
                    let mut results: Vec<(i32, i32, IntervalCheck, PlayerState)> = Vec::new();
                    let (mut steps, mut min_gap, mut max_gap) = (0u32, i32::MAX, 0i32);
                    for &(backend, snap) in &combos {
                        jka_movement::set_snap_mode(snap);
                        let mut candidate = prediction_settings;
                        candidate.backend = backend;
                        let mut probe = NativePlayerState::from_network(&to_native(&previous))?;
                        probe.configure(&candidate)?;
                        probe.set_foot_bolts(self.foot_bolts)?;
                        let (mut count, mut low, mut high, mut last) = (0u32, i32::MAX, 0i32, start);
                        for number in first..=current {
                            let mut cmd = command(number);
                            let mut time = cmd.server_time;
                            if candidate.pmove_fixed != 0 {
                                let msec = candidate.pmove_msec;
                                time = ((time + msec - 1) / msec) * msec;
                            }
                            if time <= start {
                                continue;
                            }
                            if time > end {
                                break;
                            }
                            if candidate.pmove_fixed != 0 {
                                movement.update_view_angles(&mut probe, movement_cmd(&cmd));
                                cmd.server_time = time;
                            }
                            low = low.min(time - last);
                            high = high.max(time - last);
                            last = time;
                            movement.predict(&mut probe, movement_cmd(&cmd), &candidate, &mut world)?;
                            count += 1;
                        }
                        if count > 0 && last == end {
                            let replayed = from_native(probe.network());
                            let check = check_interval(&replayed, base_ps);
                            (steps, min_gap, max_gap) = (count, low, high);
                            results.push((backend, snap, check, replayed));
                        }
                    }
                    jka_movement::set_snap_mode(effective_snap_mode);
                    if !results.is_empty() {
                        if detecting && results.len() == combos.len() {
                            for (backend, snap, check, _) in &results {
                                self.diag_combo_matched[combo_index(*backend, *snap)] += u32::from(check.exact);
                            }
                            let exact: Vec<(i32, i32)> = results.iter().filter(|r| r.2.exact).map(|r| (r.0, r.1)).collect();
                            // Informative only when the candidates disagree about this interval.
                            if !exact.is_empty() && exact.len() < results.len() {
                                for &(backend, snap) in &exact {
                                    self.combo_votes[combo_index(backend, snap)] += 1;
                                }
                                if self.combo_votes.iter().sum::<u32>() > 400 {
                                    self.combo_votes = self.combo_votes.map(|votes| votes / 2);
                                }
                                let best = combos
                                    .iter()
                                    .copied()
                                    .max_by_key(|&(backend, snap)| self.combo_votes[combo_index(backend, snap)])
                                    .unwrap_or((effective_backend, effective_snap_mode));
                                let best_votes = self.combo_votes[combo_index(best.0, best.1)];
                                let current_votes = self.combo_votes[combo_index(effective_backend, effective_snap_mode)];
                                if best != (effective_backend, effective_snap_mode)
                                    && best_votes >= 4
                                    && best_votes >= current_votes + 3
                                {
                                    if settings.predict_backend < 0 && !fixed_tayst_backend {
                                        self.auto_backend = best.0;
                                    }
                                    if settings.snap_mode < 0 && !fixed_tayst_backend {
                                        self.auto_snap = best.1;
                                    }
                                    jka_movement::set_snap_mode(if settings.snap_mode >= 0 { settings.snap_mode } else { self.auto_snap });
                                    self.snap_mode_applied = Some(if settings.snap_mode >= 0 { settings.snap_mode } else { self.auto_snap });
                                    self.misses.push(format!(
                                        "^3PREDICT-MODE^7 detected: {} reproduces this server best ({best_votes} votes vs {current_votes} for {}); applying it",
                                        combo_name(best.0, best.1),
                                        combo_name(effective_backend, effective_snap_mode),
                                    ));
                                }
                            }
                        }
                        let active = results
                            .iter()
                            .find(|r| r.0 == effective_backend && r.1 == effective_snap_mode)
                            .unwrap_or(&results[0]);
                        let (check, replayed) = (&active.2, &active.3);
                        let field = |ps: &PlayerState, name: &str| ps.field_f32(name).unwrap_or(0.0);
                        let magnitude = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                        self.diag_compared += 1;
                        self.diag_matched += u32::from(check.exact);
                        if settings.physics_diag > 0 && (!check.exact || settings.physics_diag >= 2) {
                            self.misses.push(format!(
                                "{}^7 snap#{} cmdtime {start}->{end} steps={steps} gaps={min_gap}..{max_gap}ms using {} | origin d=({:.3},{:.3},{:.3}) |d|={:.3} | vel d=({:.2},{:.2},{:.2}) | start v=({:.0},{:.0},{:.0}) replay v=({:.2},{:.2},{:.2}) server v=({:.2},{:.2},{:.2}) | ground {}/{} pmf {:#x}/{:#x} pmt {}/{} (replay/server)",
                                if check.exact { "^2PHYS-OK" } else { "^1PHYS-DIAG" },
                                base.message_num,
                                combo_name(active.0, active.1),
                                check.origin_delta[0], check.origin_delta[1], check.origin_delta[2], magnitude(check.origin_delta),
                                check.velocity_delta[0], check.velocity_delta[1], check.velocity_delta[2],
                                field(&previous, "velocity[0]"), field(&previous, "velocity[1]"), field(&previous, "velocity[2]"),
                                field(replayed, "velocity[0]"), field(replayed, "velocity[1]"), field(replayed, "velocity[2]"),
                                field(base_ps, "velocity[0]"), field(base_ps, "velocity[1]"), field(base_ps, "velocity[2]"),
                                check.ground.0, check.ground.1, check.flags.0, check.flags.1, check.timer.0, check.timer.1,
                            ));
                        }
                        if settings.physics_diag > 0 && self.diag_compared % 20 == 0 {
                            let by_candidate = if detecting {
                                let parts: Vec<String> = combos
                                    .iter()
                                    .map(|&(backend, snap)| format!("{} {}", combo_name(backend, snap), self.diag_combo_matched[combo_index(backend, snap)]))
                                    .collect();
                                format!("; exact per candidate: {}; using {}", parts.join(" | "), combo_name(effective_backend, effective_snap_mode))
                            } else {
                                String::new()
                            };
                            self.misses.push(format!(
                                "^3PHYS-DIAG summary^7: {} of {} snapshot intervals reproduced exactly ({} commands per interval, gaps {min_gap}..{max_gap} ms){by_candidate}",
                                self.diag_matched, self.diag_compared, steps,
                            ));
                        }
                    }
                }
            }
            self.diag_previous = Some((base.message_num, base_ps.clone()));
        }
        if let Some(sequence) = miss_sequence_this_frame {
            if let Some(miss) = self.latest_miss.as_mut().filter(|miss| miss.sequence == sequence) {
                miss.replay_ground_trace = replay_ground_trace;
                miss.replay_ground_probe = replay_ground_probe;
            }
            if settings.prediction_debug {
                if let Some(trace) = replay_ground_trace {
                    self.misses.push(format!(
                        "PRED-DIAG replay groundTrace start={:?} end={:?} frac={:.4} hit={:?} normal={:?} ent={} startSolid={} allSolid={}",
                        trace.start, trace.end, trace.fraction, trace.hit_end, trace.normal,
                        trace.entity, trace.start_solid, trace.all_solid,
                    ));
                }
                if let Some(trace) = replay_ground_probe {
                    self.misses.push(format!(
                        "PRED-DIAG replay groundProbe64 start={:?} end={:?} frac={:.4} hit={:?} normal={:?} ent={} startSolid={} allSolid={}",
                        trace.start, trace.end, trace.fraction, trace.hit_end, trace.normal,
                        trace.entity, trace.start_solid, trace.all_solid,
                    ));
                }
            }
        }
        // OpenJK leaves cg.predictedPlayerState at the base snapshot's state
        // when no command was replayed.
        let mut predicted = if moved { from_native(native.network()) } else { base_ps.clone() };
        if moved {
            // Adjust for the movement of the ground entity up to this frame.
            let carried = adjust_position_for_mover(entities, &predicted, origin(&predicted), physics_time, time);
            set_origin(&mut predicted, carried);
            self.predicted_view_forced = replay_view_forced;
        }
        self.predicted = Some(predicted);
        self.display = self.predicted.clone();
        self.display_view_forced = self.predicted_view_forced;
        self.display_angles = session.command(current).map(|cmd| cmd.angles);
        if let Some(cmd) = provisional {
            let reached = native.view().command_time;
            if prediction_settings.pmove_fixed == 0 && cmd.server_time > reached && cmd.server_time > latest.server_time {
                movement.predict(native, movement_cmd(&cmd), &prediction_settings, &mut world)?;
                let native_view = native.view();
                let mut display = from_native(native.network());
                let carried = adjust_position_for_mover(entities, &display, origin(&display), physics_time, time);
                set_origin(&mut display, carried);
                if let Some(committed) = self.predicted.as_ref() {
                    keep_committed_animation(&mut display, committed);
                }
                self.display = Some(display);
                self.display_view_forced = native_view.view_forced != 0;
                self.display_angles = Some(cmd.angles);
            }
        }
        if debug_enabled {
            let display = self.display.as_ref().map(prediction_state_debug).unwrap_or_default();
            let committed = self.predicted.as_ref().map(prediction_state_debug).unwrap_or_default();
            self.last_ground_trace = *world.ground_trace;
            self.last_ground_probe = *world.ground_probe;
            self.debug_frame = Some(PredictionFrameDebug {
                server_time: time,
                base_message: base.message_num,
                base_time: base.server_time,
                base: prediction_state_debug(base_ps),
                settings: prediction_settings,
                display,
                committed,
                view_error: self.last_view_error,
                ground_trace: self.last_ground_trace,
                ground_probe: self.last_ground_probe,
            });
        } else {
            self.debug_frame = None;
            self.latest_miss = None;
            self.last_ground_trace = None;
            self.last_ground_probe = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mover_entity(number: u16, entity_type: i32, base: [f32; 3], delta: [f32; 3]) -> crate::cgame::PresentedEntity {
        use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};
        let mut state = EntityState { number, fields: [0; ENTITY_FIELDS.len()] };
        let mut set = |name: &str, bits: u32| {
            let index = ENTITY_FIELDS.iter().position(|(n, _)| *n == name).unwrap();
            state.fields[index] = bits;
        };
        set("pos.trType", 2); // TR_LINEAR
        set("pos.trTime", 0);
        for axis in 0..3 {
            set(&format!("pos.trBase[{axis}]"), base[axis].to_bits());
            set(&format!("pos.trDelta[{axis}]"), delta[axis].to_bits());
        }
        crate::cgame::PresentedEntity { number, entity_type, origin: base, angles: [0.0; 3], state }
    }

    fn standing_on(ground: i32) -> PlayerState {
        let mut ps = PlayerState::default();
        assert!(ps.set_field_bits("groundEntityNum", ground as u32));
        ps
    }

    #[test]
    fn ground_mover_carries_the_predicted_origin_between_times() {
        let entities = [mover_entity(70, ET_MOVER, [0.0; 3], [0.0, 0.0, 100.0])];
        let carried = adjust_position_for_mover(&entities, &standing_on(70), [10.0, 20.0, 30.0], 1000, 1250);
        assert_eq!(carried, [10.0, 20.0, 55.0]);
        // Going back in time undoes the same amount (oldTime < physicsTime).
        let back = adjust_position_for_mover(&entities, &standing_on(70), [10.0, 20.0, 30.0], 1250, 1000);
        assert_eq!(back, [10.0, 20.0, 5.0]);
    }

    #[test]
    fn only_ground_movers_carry_the_player() {
        let position = [1.0, 2.0, 3.0];
        let mover = mover_entity(70, ET_MOVER, [0.0; 3], [0.0, 0.0, 100.0]);
        let not_a_mover = mover_entity(71, 1, [0.0; 3], [0.0, 0.0, 100.0]);
        let entities = [mover, not_a_mover];
        // World ground, ENTITYNUM_NONE, a non-mover and a missing entity stay put.
        for ground in [0, 1023, 71, 72] {
            assert_eq!(adjust_position_for_mover(&entities, &standing_on(ground), position, 0, 500), position);
        }
        // Spectators and followers are never carried.
        let mut spectator = standing_on(70);
        spectator.persistant[PERS_TEAM] = TEAM_SPECTATOR;
        assert_eq!(adjust_position_for_mover(&entities, &spectator, position, 0, 500), position);
        let mut follower = standing_on(70);
        follower.set_field_bits("pm_flags", PMF_FOLLOW_FLAG as u32);
        assert_eq!(adjust_position_for_mover(&entities, &follower, position, 0, 500), position);
    }

    #[test]
    fn mover_solids_are_placed_at_physics_time() {
        let mut entity = mover_entity(70, ET_MOVER, [0.0; 3], [0.0, 0.0, 100.0]);
        let index = jka_protocol::gamestate::ENTITY_FIELDS.iter().position(|(n, _)| *n == "solid").unwrap();
        entity.state.fields[index] = 0x00ff_ffff;
        let solids = solid_entities(&[entity], 500);
        let EntityClip::InlineModel { origin, .. } = solids[0].clip else { panic!("bmodel expected") };
        assert_eq!(origin, [0.0, 0.0, 50.0]);
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with the stock mp/ffa3 BSP"]
    fn prediction_traces_clip_against_player_boxes_and_movers() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = jka_assets::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bsp = assets.read("maps/mp/ffa3.bsp", 64 << 20).unwrap().expect("mp/ffa3.bsp").bytes;
        let mut world = CollisionWorld::from_bsp(&bsp).unwrap();
        let spawn = jka_assets::bsp::Bsp::parse(&bsp).unwrap().deathmatch_spawns()[0].origin;
        let start = [spawn[0], spawn[1], spawn[2] + 16.0];
        let end = [spawn[0] + 64.0, spawn[1], spawn[2] + 16.0];
        let query = TraceQuery { start, mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 40.0], end, pass_entity: 4, mask: 0x1 | 0x10 | 0x100 | 0x1000 };
        assert_eq!(world.trace(query).fraction, 1.0, "spawn corridor must be open for this test");

        // A standing player (solid 15/24/40+32) 48 units ahead blocks at 18/64.
        let player = SolidEntity {
            number: 7,
            generic_enemy_index: 0,
            skip_movement: false,
            clip: EntityClip::Box { mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 40.0], origin: [spawn[0] + 48.0, spawn[1], spawn[2] + 16.0] },
            bounds: None,
        };
        let solids = [player];
        let mut prediction = PredictionWorld { world: &mut world, solids: &solids, client_num: 4 };
        let hit = prediction.trace(query);
        assert_eq!(hit.entity, 7);
        assert!((hit.fraction - 18.0 / 64.0).abs() < 0.02, "{}", hit.fraction);
        // The passed entity (ourselves) and owned objects are ignored.
        let ignored = PredictionWorld { world: &mut world, solids: &solids, client_num: 4 }.trace(TraceQuery { pass_entity: 7, ..query });
        assert_eq!(ignored.fraction, 1.0);
        let owned = [SolidEntity { number: 300, generic_enemy_index: 1024 + 4, skip_movement: false, clip: player.clip, bounds: None }];
        assert_eq!(PredictionWorld { world: &mut world, solids: &owned, client_num: 4 }.trace(query).fraction, 1.0);

        // An inline model translated onto the path blocks like CM_TransformedBoxTrace.
        let mover = [SolidEntity {
            number: 90,
            generic_enemy_index: 0,
            skip_movement: false,
            clip: EntityClip::InlineModel { index: 1, origin: [0.0; 3], angles: [0.0; 3] },
            bounds: None,
        }];
        let model_contents = PredictionWorld { world: &mut world, solids: &mover, client_num: 4 }.point_contents(start, 4);
        assert_eq!(model_contents & 1, 0, "spawn point is not inside *1 at its compiled position");
    }

    /// Live end-to-end check of CL_CreateCmd -> netchan -> server Pmove ->
    /// snapshot -> CG_PredictPlayerState. Set JKA_LIVE_SERVER (host[:port])
    /// and JKA_TEST_BASE; JKA_LIVE_TEAM=f joins the game instead of flying as
    /// a spectator. Reports cg_showMiss-style prediction errors.
    #[test]
    #[ignore = "connects to JKA_LIVE_SERVER; needs JKA_TEST_BASE for the map BSP"]
    fn live_prediction_tracks_the_server() {
        use std::time::Duration;
        let server = std::env::var("JKA_LIVE_SERVER").expect("set JKA_LIVE_SERVER");
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let seconds: u64 = std::env::var("JKA_LIVE_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
        let team = std::env::var("JKA_LIVE_TEAM").ok();
        let mut assets = jka_assets::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let animation = assets.read("models/players/_humanoid/animation.cfg", 60_000).unwrap().unwrap().bytes;
        let movement = PmoveContext::new(&animation).unwrap();
        let mut settings = NetworkSettings { name: "DinurdoPredict".into(), show_miss: true, ..NetworkSettings::default() };
        settings.max_packets = 60;
        let mut net = NetClient::connect(&server, settings.userinfo("kyle/default"), 0).unwrap();
        let mut world: Option<CollisionWorld> = None;
        let mut latest: Option<Snapshot> = None;
        let mut input = LiveInput::default();
        let mut predictor = Predictor::default();
        let (mut predictions, mut snapshots, mut misses) = (0u32, 0u32, Vec::<f32>::new());
        // Frames where prediction did not reach the newest usercmd (the
        // "exceeded PACKET_BACKUP on commands" freeze).
        let mut stale_frames = 0u32;
        // Frames whose displayed state reached the current cl.serverTime
        // through the provisional command (per-frame smoothness).
        let mut display_current = 0u32;
        let mut provisional: Option<UserCmd> = None;
        // Real usercmd cadence (the app paces with cl_commandRate).
        let command_ms: u128 = std::env::var("JKA_LIVE_CMD_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut last_command: Option<std::time::Instant> = None;
        let loop_ms: u64 = std::env::var("JKA_LIVE_LOOP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
        let started = std::time::Instant::now();
        let mut active_at = None;
        let mut sent_team = false;
        while started.elapsed() < Duration::from_secs(seconds + 15) {
            for event in net.pump() {
                match event {
                    SessionEvent::Gamestate => {
                        let map = net.session().decoder().map_name().unwrap();
                        let bsp = assets.read(&format!("maps/{map}.bsp"), 256 << 20).unwrap().expect("map BSP in JKA_TEST_BASE").bytes;
                        world = Some(CollisionWorld::from_bsp(&bsp).unwrap());
                        let now = net.realtime();
                        net.session_mut().set_primed(now);
                        println!("gamestate {map}; primed");
                    }
                    SessionEvent::Snapshot(snapshot) => {
                        snapshots += 1;
                        latest = Some(snapshot);
                    }
                    SessionEvent::Disconnected(reason) => panic!("disconnected: {reason}"),
                    SessionEvent::Print(text) => print!("print: {}", String::from_utf8_lossy(&text)),
                    _ => {}
                }
            }
            let now = net.realtime();
            let time = net.session_mut().set_cgame_time(now, 0);
            if net.state() >= ConnectionState::Primed {
                if let Some(snapshot) = &latest {
                    input.sync_selection(&snapshot.player_state);
                }
                // Fly/run in a slow circle and hop every second.
                let t = started.elapsed().as_secs_f32();
                input.apply_mouse(0.35, 0.0);
                let mut held: HashSet<String> = ["+forward"].into_iter().map(String::from).collect();
                if std::env::var_os("JKA_LIVE_NOJUMP").is_none() && t.fract() < 0.15 {
                    held.insert("+moveup".into());
                }
                let buttons = CommandButtons { active: &held, forced_moveup: false, any_key: true, talking: false };
                if last_command.is_none_or(|last| last.elapsed().as_millis() >= command_ms) {
                    last_command = Some(std::time::Instant::now());
                    let cmd = input.create_cmd(&buttons);
                    net.session_mut().create_command(cmd);
                }
                let mut preview = input.preview_cmd(&buttons);
                preview.server_time = net.session().server_time();
                provisional = Some(preview);
            }
            net.session_mut().set_packet_dup(settings.packet_dup as i32);
            net.session_mut().send_commands(now, settings.max_packets as i32);
            net.flush();
            if let (Some(time), Some(snapshot), Some(world)) = (time, latest.as_ref(), world.as_mut()) {
                let active = *active_at.get_or_insert(started.elapsed());
                if let (Some(team), false) = (&team, sent_team) {
                    if started.elapsed() > active + Duration::from_secs(1) {
                        net.session_mut().add_reliable_command(format!("team {team}").as_bytes(), false).unwrap();
                        sent_team = true;
                    }
                }
                predictor
                    .predict(PredictionInput {
                        session: net.session(),
                        snap: snapshot,
                        next: None,
                        time,
                        movement: &movement,
                        world,
                        entities: &[],
                        settings: &settings,
                        provisional,
                    })
                    .unwrap();
                predictions += 1;
                let newest = net.session().command(net.session().cmd_number()).map_or(0, |cmd| cmd.server_time);
                let predicted_time = predictor.predicted().and_then(|ps| ps.field_i32("commandTime")).unwrap_or(0);
                if predicted_time < newest {
                    stale_frames += 1;
                }
                if predicted_time >= time {
                    display_current += 1;
                }
                for miss in predictor.misses.drain(..) {
                    println!("{miss}");
                    misses.push(miss.split_whitespace().nth(2).unwrap().parse().unwrap());
                }
                if started.elapsed() > active + Duration::from_secs(seconds) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(loop_ms));
        }
        net.disconnect();
        let ps = predictor.predicted().cloned().expect("prediction ran");
        misses.sort_by(f32::total_cmp);
        let large = misses.iter().filter(|&&m| m > 1.0).count();
        println!("stale prediction frames: {stale_frames} of {predictions} (loop {loop_ms} ms, commands {})", net.session().cmd_number());
        println!("display reached cl.serverTime on {display_current} of {predictions} frames");
        println!(
            "snapshots {snapshots}, predictions {predictions}, misses {} (>{}u: {large}), median {:?}, max {:?}, final pm_type {:?} origin {:?}",
            misses.len(),
            1,
            misses.get(misses.len() / 2),
            misses.last(),
            ps.field_i32("pm_type"),
            origin(&ps)
        );
        assert!(snapshots > 100, "server stream stalled");
        assert!(predictions > 100);
        assert!(large * 20 <= predictions as usize, "more than 5% of frames mispredicted by over a unit");
        assert!(stale_frames * 20 <= predictions, "prediction stalled behind the newest command");
    }

    #[test]
    fn server_addresses_default_to_29070() {
        assert_eq!(resolve_server("127.0.0.1").unwrap(), "127.0.0.1:29070".parse().unwrap());
        assert_eq!(resolve_server("127.0.0.1:29071").unwrap(), "127.0.0.1:29071".parse().unwrap());
        assert!(resolve_server("").is_err());
    }

    #[test]
    fn chat_bubble_cvars_default_on_and_round_trip() {
        let mut settings = NetworkSettings::default();
        assert_eq!(settings.cvar_value("cl_chatBubbleSelf").as_deref(), Some("1"));
        assert_eq!(settings.cvar_value("cl_chatBubbleUnfocused").as_deref(), Some("1"));
        assert_eq!(settings.set_cvar("cl_chatBubbleSelf", "0").unwrap().unwrap(), false);
        assert!(!settings.chat_bubble_self && settings.chat_bubble_unfocused);
        let mut cfg = String::new();
        settings.write_cfg(&mut cfg);
        assert!(cfg.contains("seta cl_chatBubbleSelf \"0\""));
        assert!(cfg.contains("seta cl_chatBubbleUnfocused \"1\""));
    }

    #[test]
    fn rgb_saber_userinfo_follows_server_mod() {
        use mod_support::ServerMod;
        let mut settings = NetworkSettings::default();
        assert_eq!(settings.set_cvar("color1", "6").unwrap().unwrap(), true);
        assert_eq!(settings.set_cvar("cp_sbRGB1", "16711680").unwrap().unwrap(), true); // pure blue

        // jaPRO / JA+ relay the RGB and keep colour 6.
        for server_mod in [ServerMod::Japro, ServerMod::Japlus] {
            let info = settings.userinfo_for_mod("kyle/default", server_mod);
            assert_eq!(info_value(&info, b"color1"), Some(b"6".as_slice()));
            assert_eq!(info_value(&info, b"cp_sbRGB1"), Some(b"16711680".as_slice()));
        }
        // Everyone else gets the closest stock colour and no RGB key.
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"color1"), Some(b"4".as_slice()));
        assert_eq!(info_value(&info, b"cp_sbRGB1"), None);
        // A stock colour is never rewritten.
        settings.set_cvar("color2", "2").unwrap().unwrap();
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"color2"), Some(b"2".as_slice()));

        assert_eq!(nearest_base_saber_color(0x0000FF), 0); // red
        assert_eq!(nearest_base_saber_color(0x00FF00), 3); // green
        assert_eq!(nearest_base_saber_color(base_saber_rgb_packed(5)), 5);
        let mut cfg = String::new();
        settings.write_cfg(&mut cfg);
        assert!(cfg.contains("seta cp_sbRGB1 \"16711680\""));

        // The mod name and the feature mask are independent in TaystJK. A Base
        // server can advertise RGB support without becoming JA+/jaPRO.
        let info = settings.userinfo_for_server(
            "kyle/default",
            br"\gamename\basejka\taystJKinfo\1",
        );
        assert_eq!(info_value(&info, b"color1"), Some(b"6".as_slice()));
        assert_eq!(info_value(&info, b"cp_sbRGB1"), Some(b"16711680".as_slice()));
    }

    #[test]
    fn char_color_is_a_userinfo_cvar_like_openjk() {
        use mod_support::ServerMod;
        let mut settings = NetworkSettings::default();
        let info = settings.userinfo("kyle/default");
        for key in [&b"char_color_red"[..], b"char_color_green", b"char_color_blue"] {
            assert_eq!(info_value(&info, key), Some(b"255".as_slice()));
        }
        assert_eq!(settings.set_cvar("char_color_red", "10").unwrap().unwrap(), true);
        assert_eq!(settings.set_cvar("char_color_green", "300").unwrap().unwrap(), true); // clamped
        assert_eq!(settings.set_cvar("char_color_blue", "0").unwrap().unwrap(), true);
        assert_eq!(settings.set_cvar("handicap", "50").unwrap().unwrap(), true);
        assert_eq!(settings.cvar_value("CHAR_COLOR_GREEN").as_deref(), Some("255"));
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"char_color_red"), Some(b"10".as_slice()));
        assert_eq!(info_value(&info, b"char_color_green"), Some(b"255".as_slice()));
        assert_eq!(info_value(&info, b"char_color_blue"), Some(b"0".as_slice()));
        assert_eq!(info_value(&info, b"handicap"), Some(b"50".as_slice()));
        assert_eq!(info_value(&info, b"cp_cosmetics"), None);
        let mut cfg = String::new();
        settings.write_cfg(&mut cfg);
        assert!(cfg.contains("seta char_color_red \"10\""));
        assert!(!cfg.contains("cp_clanPwd"));
    }

    #[test]
    fn cosmetics_bit_31_round_trips_as_a_signed_int() {
        let mut settings = NetworkSettings::default();
        // Super Saiyan is bit 31; jaPRO stores the mask as a signed int.
        let mask = (1u32 << 31) | (1 << 3);
        settings.set_cvar("cp_cosmetics", &(mask as i32).to_string()).unwrap().unwrap();
        assert_eq!(settings.cosmetics, mask);
        assert_eq!(settings.cvar_value("cp_cosmetics").as_deref(), Some((mask as i32).to_string().as_str()));
    }

    #[test]
    fn japro_only_userinfo_is_sent_to_japro_servers() {
        use mod_support::ServerMod;
        let mut settings = NetworkSettings::default();
        settings.set_cvar("cp_cosmetics", "5").unwrap().unwrap();
        settings.display_camera_position = "0 120 8".into();
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Japro);
        assert_eq!(info_value(&info, b"cp_cosmetics"), Some(b"5".as_slice()));
        assert_eq!(info_value(&info, b"cp_clanPwd"), Some(b"none".as_slice()));
        assert_eq!(info_value(&info, b"cg_displayCameraPosition"), Some(b"0 120 8".as_slice()));
        assert_eq!(info_value(&info, b"cg_displayNetSettings"), Some(b"125 0 125".as_slice()));
        assert!(jka_protocol::netchan::connect_packet(&info).is_ok());
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"cg_displayNetSettings"), None);
        assert!(settings.set_cvar("cg_displayNetSettings", "1 2 3").unwrap().is_err());
    }

    #[test]
    fn userinfo_contains_stock_keys_in_openjk_order() {
        let info = NetworkSettings::default().userinfo("kyle/default");
        let text = String::from_utf8(info.clone()).unwrap();
        assert!(text.starts_with("\\name\\Padawan\\rate\\50000\\snaps\\100\\model\\kyle/default"), "{text}");
        assert!(text.ends_with("\\char_color_blue\\255"), "{text}");
        assert_eq!(info_value(&info, b"teamtask"), None);
        assert!(jka_protocol::netchan::connect_packet(text.as_bytes()).is_ok());
    }

    #[test]
    fn team_overlay_userinfo_mirrors_cgame_request_bit() {
        let mut settings = NetworkSettings::default();
        let info = settings.userinfo("kyle/default");
        assert_eq!(info_value(&info, b"teamoverlay"), Some(b"0".as_slice()));
        settings.team_overlay = true;
        let info = settings.userinfo("kyle/default");
        assert_eq!(info_value(&info, b"teamoverlay"), Some(b"1".as_slice()));
        assert!(settings.set_cvar("teamoverlay", "0").unwrap().is_err());
    }

    #[test]
    fn net_port_defaults_to_jka_port_but_is_not_userinfo() {
        let settings = NetworkSettings::default();
        assert_eq!(settings.net_port, jka_protocol::DEFAULT_PORT);
        assert_eq!(info_value(&settings.userinfo("kyle/default"), b"net_port"), None);
    }

    #[test]
    fn japro_settings_follow_configstrings_and_clear_on_server_change() {
        let ps = PlayerState::default();
        let mut config = std::collections::BTreeMap::from([
            (0, br"\gamename\JaPRO 1.4\jcinfo\123\jcinfo2\8\g_hookStrength\900".to_vec()),
            (1, br"\pmove_fixed\1\pmove_msec\1".to_vec()),
            (36, b"0x7".to_vec()),
        ]);
        let settings = predict_settings(&config, &ps);
        assert_eq!((settings.backend, settings.server_mod, settings.cinfo, settings.jcinfo, settings.jcinfo2), (1, 1, 123, 123, 8));
        assert_eq!((settings.pmove_msec, settings.hook_pull, settings.legacy_fixes), (1, 900, 7));
        config.insert(0, br"\gamename\basejka\jcinfo\123".to_vec());
        let settings = predict_settings(&config, &ps);
        assert_eq!((settings.backend, settings.server_mod, settings.cinfo, settings.jcinfo, settings.jcinfo2, settings.legacy_fixes), (0, 0, 0, 0, 0, 7));
        assert_eq!(settings.pmove_msec, 8);
        assert_eq!(settings.hook_pull, 0);
        config.clear();
        assert_eq!(predict_settings(&config, &ps).server_mod, 0);
    }


    #[test]
    fn japlus_and_tayst_feature_flags_are_independent_of_japro() {
        let ps = PlayerState::default();
        let mut config = std::collections::BTreeMap::from([(
            0,
            br"\gamename\JA+ Mod\jp_cinfo\4660\taystJKinfo\33\restricts\7".to_vec(),
        )]);
        let settings = predict_settings(&config, &ps);
        assert_eq!((settings.backend, settings.server_mod, settings.cinfo), (1, 2, 4660));
        assert_eq!(settings.hook_pull, 800);
        assert_eq!((settings.taystjk_info, settings.restricts), (33, 7));

        config.insert(0, br"\gamename\basejka\taystJKinfo\9".to_vec());
        let settings = predict_settings(&config, &ps);
        assert_eq!((settings.backend, settings.server_mod, settings.taystjk_info, settings.base_game), (1, 0, 9, 1));

        config.remove(&36);
        config.insert(0, br"\gamename\basejka\taystJKinfo\1".to_vec());
        let settings = predict_settings(&config, &ps);
        assert_eq!(settings.backend, 0, "RGB-only capability must not change movement implementation");

        // A TaystJK server explicitly advertises whether its game module uses
        // the Raven SDK API. This overrides gamename, including a false value.
        config.insert(0, br"\gamename\basejka\taystJKinfo\9\sv_legacyGameAPI\0".to_vec());
        assert_eq!(predict_settings(&config, &ps).base_game, 0);
        config.insert(0, br"\gamename\futuremod\sv_legacyGameAPI\1".to_vec());
        assert_eq!(predict_settings(&config, &ps).base_game, 1);
    }

    #[test]
    fn step_slide_fix_follows_serverinfo_and_is_off_when_absent() {
        let ps = PlayerState::default();
        let mut config = std::collections::BTreeMap::from([(0, br"\gamename\basejka".to_vec())]);
        assert_eq!(predict_settings(&config, &ps).step_slide_fix, 0);
        config.insert(0, br"\gamename\basejka\g_stepSlideFix\1".to_vec());
        assert_eq!(predict_settings(&config, &ps).step_slide_fix, 1);
        config.insert(0, br"\gamename\basejka\g_stepSlideFix\0".to_vec());
        assert_eq!(predict_settings(&config, &ps).step_slide_fix, 0);
    }

    #[test]
    fn japro_preferences_are_archived_and_advertised_only_for_japro() {
        use mod_support::ServerMod;
        let mut settings = NetworkSettings::default();
        assert_eq!(settings.set_cvar("cp_pluginDisable", "1048576").unwrap().unwrap(), true);
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Japro);
        assert_eq!(info_value(&info, b"cjp_client"), Some(b"1.4JAPRO".as_slice()));
        assert_eq!(info_value(&info, b"cp_pluginDisable"), Some(b"1048576".as_slice()));
        assert!(jka_protocol::netchan::connect_packet(&info).is_ok());
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Japlus);
        assert_eq!(info_value(&info, b"cjp_client"), Some(b"1.4JAPRO".as_slice()));
        assert_eq!(info_value(&info, b"cp_pluginDisable"), None);
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"cjp_client"), Some(b"1.4JAPRO".as_slice()));
        assert_eq!(info_value(&info, b"cp_pluginDisable"), None);
        let mut cfg = String::new();
        settings.write_cfg(&mut cfg);
        assert!(cfg.contains("seta cp_pluginDisable \"1048576\""));
    }

    #[test]
    fn create_cmd_maps_kbuttons_and_movement() {
        let mut input = LiveInput::default();
        input.view_angles = [10.0, -90.0, 0.0];
        input.note_pressed("+attack");
        let active: HashSet<String> = ["+forward", "+moveleft", "+speed", "+altattack"].into_iter().map(String::from).collect();
        let cmd = input.create_cmd(&CommandButtons { active: &active, forced_moveup: false, any_key: true, talking: false });
        assert_eq!(cmd.forward_move, 64);
        assert_eq!(cmd.right_move, -64);
        assert_eq!(cmd.buttons & (1 | 128 | BUTTON_WALKING | BUTTON_ANY), 1 | 128 | BUTTON_WALKING | BUTTON_ANY);
        assert_eq!(cmd.angles[1], jka_movement::angle_to_short(270.0));
        // wasPressed is consumed by one command.
        let cmd = input.create_cmd(&CommandButtons { active: &HashSet::new(), forced_moveup: false, any_key: false, talking: false });
        assert_eq!(cmd.buttons, 0);
        assert_eq!(cmd.forward_move, 0);
    }

    #[test]
    fn generic_commands_are_one_shot() {
        let mut input = LiveInput::default();
        input.queue_generic_command(generic_command("force_throw").unwrap());
        let none = HashSet::new();
        let buttons = CommandButtons { active: &none, forced_moveup: false, any_key: false, talking: false };
        assert_eq!(input.create_cmd(&buttons).generic_command, 5);
        assert_eq!(input.create_cmd(&buttons).generic_command, 0);
    }

    #[test]
    fn weapon_slots_follow_cg_weapon_f() {
        let mut ps = PlayerState::default();
        ps.stats[4] = (1 << 3) | (1 << 4) | (1 << 5);
        ps.ammo[2] = 100; // AMMO_BLASTER
        let mut input = LiveInput::default();
        input.select_weapon_slot(&ps, "2");
        assert_eq!(input.weapon_select, 4, "slot 2 is WP_BRYAR_PISTOL");
        input.select_weapon_slot(&ps, "3");
        assert_eq!(input.weapon_select, 5, "slot 3 is WP_BLASTER");
        input.select_weapon_slot(&ps, "1");
        assert_eq!(input.weapon_select, 3, "slot 1 selects the saber when not already wielded");
        input.cycle_weapon(&ps, true);
        assert_eq!(input.weapon_select, 4);
    }

    #[test]
    fn out_of_ammo_change_skips_the_dry_weapon_and_explosives() {
        let mut ps = PlayerState::default();
        ps.stats[4] = (1 << 4) | (1 << 5) | (1 << 12);
        ps.ammo[2] = 100;
        ps.ammo[8] = 100; // thermal ammo: safe autoswitch must still skip it
        let mut input = LiveInput::default();
        input.out_of_ammo_change(&ps, 5, 1);
        assert_eq!(input.weapon_select, 4, "blaster ran dry, pistol is next best");
        input.out_of_ammo_change(&ps, 4, 1);
        assert_eq!(input.weapon_select, 5);
    }

    #[test]
    fn pickup_autoswitch_respects_level_saber_and_safety() {
        let with_weapon = |weapon: u32| {
            let mut ps = PlayerState::default();
            ps.set_field_bits("weapon", weapon);
            ps
        };
        let mut input = LiveInput::default();
        input.pickup_autoswitch(&with_weapon(4), 5, 1);
        assert_eq!(input.weapon_select, 5, "a better safe weapon is selected");
        input.pickup_autoswitch(&with_weapon(4), 11, 1);
        assert_eq!(input.weapon_select, 5, "rockets are unsafe at level 1");
        input.pickup_autoswitch(&with_weapon(4), 11, 2);
        assert_eq!(input.weapon_select, 11);
        input.pickup_autoswitch(&with_weapon(3), 7, 2);
        assert_eq!(input.weapon_select, 11, "never away from the saber");
        input.pickup_autoswitch(&with_weapon(6), 5, 2);
        assert_eq!(input.weapon_select, 11, "worse weapons are ignored");
        input.pickup_autoswitch(&with_weapon(4), 8, 0);
        assert_eq!(input.weapon_select, 11, "level 0 never switches");
    }
}
