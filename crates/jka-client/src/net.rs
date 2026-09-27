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
    UserCmd as MovementCmd,
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
    pub time_nudge: i32,
    pub saber1: String,
    pub saber2: String,
    pub color1: u8,
    pub color2: u8,
    pub forcepowers: String,
    pub sex: String,
    pub password: String,
    pub error_decay: f32,
    pub no_predict: bool,
    pub show_miss: bool,
    /// Legacy compatibility escape hatch; normal joins now use the explicit missing-map prompt.
    pub allow_missing_map: bool,
    /// Allow HTTP package autodownload when the server advertises a TaystJK mvhttp/mvhttpurl endpoint.
    pub allow_http_downloads: bool,
    /// Allow the stock protocol-26 svc_download transport.
    pub allow_legacy_downloads: bool,
    /// Usercmds per second (OpenJK: one per com_maxfps client frame).
    pub command_rate: u32,
    /// JAPRO userinfo bitfield, also consumed by the native predictor.
    pub plugin_disable: i32,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            name: "Padawan".into(),
            rate: 25000,
            snaps: 40,
            max_packets: 60,
            time_nudge: 0,
            saber1: "single_1".into(),
            saber2: "none".into(),
            color1: 4,
            color2: 4,
            forcepowers: "7-1-032330000000001333".into(),
            sex: "male".into(),
            password: String::new(),
            error_decay: 100.0,
            no_predict: false,
            show_miss: false,
            allow_missing_map: false,
            allow_http_downloads: true,
            allow_legacy_downloads: true,
            command_rate: 125,
            plugin_disable: 1536,
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
            "cl_timenudge" => self.time_nudge.to_string(),
            "saber1" => self.saber1.clone(),
            "saber2" => self.saber2.clone(),
            "color1" => self.color1.to_string(),
            "color2" => self.color2.to_string(),
            "forcepowers" => self.forcepowers.clone(),
            "sex" => self.sex.clone(),
            "password" => self.password.clone(),
            "cg_errordecay" => format!("{}", self.error_decay),
            "cg_nopredict" => u8::from(self.no_predict).to_string(),
            "cg_showmiss" => u8::from(self.show_miss).to_string(),
            "cl_allowmissingmap" => u8::from(self.allow_missing_map).to_string(),
            "cl_allowhttpdownload" => u8::from(self.allow_http_downloads).to_string(),
            "cl_allowdownload" => u8::from(self.allow_legacy_downloads).to_string(),
            "cl_commandrate" => self.command_rate.to_string(),
            "cp_plugindisable" => self.plugin_disable.to_string(),
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
            "cl_timenudge" => number(-900, 900).map(|v| { self.time_nudge = v as i32; false }),
            "saber1" => text().map(|v| { self.saber1 = v; true }),
            "saber2" => text().map(|v| { self.saber2 = v; true }),
            "color1" => number(0, 255).map(|v| { self.color1 = v as u8; true }),
            "color2" => number(0, 255).map(|v| { self.color2 = v as u8; true }),
            "forcepowers" => text().map(|v| { self.forcepowers = v; true }),
            "sex" => text().map(|v| { self.sex = v; true }),
            "password" => text().map(|v| { self.password = v; true }),
            "cg_errordecay" => value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| { self.error_decay = v.clamp(0.0, 500.0); false })
                .ok_or_else(|| format!("{name}: expected a number")),
            "cg_nopredict" => Ok({ self.no_predict = atoi(value.as_bytes()) != 0; false }),
            "cg_showmiss" => Ok({ self.show_miss = atoi(value.as_bytes()) != 0; false }),
            "cl_allowmissingmap" => Ok({ self.allow_missing_map = atoi(value.as_bytes()) != 0; false }),
            "cl_allowhttpdownload" => Ok({ self.allow_http_downloads = atoi(value.as_bytes()) != 0; false }),
            "cl_allowdownload" => Ok({ self.allow_legacy_downloads = atoi(value.as_bytes()) != 0; false }),
            "cl_commandrate" => number(15, 1000).map(|v| { self.command_rate = v as u32; false }),
            "cp_plugindisable" => number(0, i32::MAX as i64).map(|v| { self.plugin_disable = v as i32; true }),
            _ => return None,
        };
        Some(result)
    }

    /// Cvar_InfoString(CVAR_USERINFO) for the stock userinfo cvars.
    pub fn userinfo(&self, model: &str) -> Vec<u8> {
        let mut info = Vec::new();
        // Info_SetValueForKey prepends, so insert in reverse of the desired order.
        let pairs: [(&str, String); 16] = [
            ("teamtask", "0".into()),
            ("char_color_blue", "255".into()),
            ("char_color_green", "255".into()),
            ("char_color_red", "255".into()),
            ("saber2", self.saber2.clone()),
            ("saber1", self.saber1.clone()),
            ("cg_predictItems", "1".into()),
            ("sex", self.sex.clone()),
            ("handicap", "100".into()),
            ("color2", self.color2.to_string()),
            ("color1", self.color1.to_string()),
            ("forcepowers", self.forcepowers.clone()),
            ("model", model.to_owned()),
            ("snaps", self.snaps.to_string()),
            ("rate", self.rate.to_string()),
            ("name", self.name.clone()),
        ];
        for (key, value) in pairs {
            set_info_value(&mut info, key.as_bytes(), value.as_bytes());
        }
        if !self.password.is_empty() {
            set_info_value(&mut info, b"password", self.password.as_bytes());
        }
        info
    }

    pub fn userinfo_for_mod(&self, model: &str, server_mod: mod_support::ServerMod) -> Vec<u8> {
        let mut info = self.userinfo(model);
        if server_mod == mod_support::ServerMod::Japro {
            set_info_value(&mut info, b"cjp_client", b"1.4JAPRO");
            set_info_value(&mut info, b"cp_pluginDisable", self.plugin_disable.to_string().as_bytes());
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
        let _ = writeln!(out, "seta cl_timeNudge \"{}\"", self.time_nudge);
        let _ = writeln!(out, "seta saber1 \"{}\"", self.saber1);
        let _ = writeln!(out, "seta saber2 \"{}\"", self.saber2);
        let _ = writeln!(out, "seta color1 \"{}\"", self.color1);
        let _ = writeln!(out, "seta color2 \"{}\"", self.color2);
        let _ = writeln!(out, "seta forcepowers \"{}\"", self.forcepowers);
        let _ = writeln!(out, "seta sex \"{}\"", self.sex);
        let _ = writeln!(out, "seta cg_errorDecay \"{}\"", self.error_decay);
        let _ = writeln!(out, "seta cg_noPredict \"{}\"", u8::from(self.no_predict));
        let _ = writeln!(out, "seta cl_allowMissingMap \"{}\"", u8::from(self.allow_missing_map));
        let _ = writeln!(out, "seta cl_allowHttpDownload \"{}\"", u8::from(self.allow_http_downloads));
        let _ = writeln!(out, "seta cl_allowDownload \"{}\"", u8::from(self.allow_legacy_downloads));
        let _ = writeln!(out, "seta cl_commandRate \"{}\"", self.command_rate);
    }
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

pub struct NetClient {
    socket: UdpSocket,
    session: ClientSession,
    epoch: Instant,
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

impl NetClient {
    #[cfg(test)]
    pub fn connect(server_name: &str, userinfo: Vec<u8>) -> Result<Self, String> {
        Self::connect_inner(server_name, userinfo, false)
    }

    pub fn connect_preflight(server_name: &str, userinfo: Vec<u8>) -> Result<Self, String> {
        Self::connect_inner(server_name, userinfo, true)
    }

    fn connect_inner(server_name: &str, userinfo: Vec<u8>, preflight: bool) -> Result<Self, String> {
        let server = resolve_server(server_name)?;
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("UDP bind failed: {error}"))?;
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
        let mut client = Self { socket, session, epoch, server_name: server_name.to_owned() };
        client.flush();
        Ok(client)
    }

    /// cls.realtime.
    pub fn realtime(&self) -> i32 {
        self.epoch.elapsed().as_millis().min(i32::MAX as u128) as i32
    }

    pub fn session(&self) -> &ClientSession { &self.session }
    pub fn session_mut(&mut self) -> &mut ClientSession { &mut self.session }

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
            let _ = self.socket.send_to(&packet, server);
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
        let held = |name: &str| buttons.active.contains(name);
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
    let japro = mod_support::ServerMod::detect(server) == mod_support::ServerMod::Japro;
    let int = |info: &[u8], key: &[u8], default: i32| info_value(info, key).map_or(default, atoi);
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
        step_slide_fix: int(server, b"g_stepSlideFix", 1),
        no_spec_move: int(server, b"g_noSpecMove", 0),
        tracemask,
        no_footsteps: i32::from(int(server, b"dmflags", 0) & 32 != 0),
        server_mod: i32::from(japro),
        jcinfo: if japro { int(server, b"jcinfo", 0) } else { 0 },
        jcinfo2: if japro { int(server, b"jcinfo2", 0) } else { 0 },
        taystjk_info: if japro { int(server, b"taystJKinfo", 0) } else { 0 },
        dmflags: int(server, b"dmflags", 0),
        hook_pull: if japro { int(server, b"g_hookStrength", 0) } else { 0 },
        restricts: if japro { int(server, b"restricts", 0) } else { 0 },
        plugin_disable: 1536,
        legacy_fixes: if japro {
            configstrings.get(&36).and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| {
                    let value = value.trim();
                    if let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
                        u32::from_str_radix(hex, 16).ok()
                    } else if value.starts_with('0') && value.len() > 1 {
                        u32::from_str_radix(value, 8).ok()
                    } else { value.parse().ok() }
                }).unwrap_or(0)
        } else { 0 },
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

const EF_TELEPORT_BIT: i32 = 1 << 3;

/// One entry of cg_solidEntities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidEntity {
    pub number: i32,
    pub generic_enemy_index: i32,
    pub skip_movement: bool,
    pub clip: EntityClip,
}

/// CG_BuildSolidList over this frame's lerped entities: triggers and items
/// are not solid; `solid` is either SOLID_BMODEL or an encoded bbox.
pub fn solid_entities(entities: &[crate::cgame::PresentedEntity]) -> Vec<SolidEntity> {
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
                    origin: entity.origin,
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
            })
        })
        .collect()
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
    predicted_error: [f32; 3],
    predicted_error_time: i32,
    old_time: i32,
    last_snapshot: Option<(i32, i32, i32)>,
    this_frame_teleport: bool,
    pub misses: Vec<String>,
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
    }

    /// The committed `cg.predictedPlayerState`, excluding this frame's
    /// provisional display-only command. Predictable event transitions must
    /// use this state so the same local event is not emitted once per render
    /// frame before its usercmd is actually committed.
    pub fn committed_predicted(&self) -> Option<&PlayerState> {
        self.predicted.as_ref()
    }

    /// The playerstate to present this frame (see `display`).
    pub fn predicted(&self) -> Option<&PlayerState> {
        self.display.as_ref().or(self.predicted.as_ref())
    }

    /// usercmd angles behind [`Self::predicted`].
    pub fn predicted_command_angles(&self) -> Option<[i32; 3]> {
        self.display_angles
    }

    /// CG_CalcViewValues: the decaying prediction error added to the view.
    pub fn view_error(&mut self, time: i32, error_decay: f32) -> [f32; 3] {
        if error_decay <= 0.0 {
            return [0.0; 3];
        }
        let f = (error_decay - (time - self.predicted_error_time) as f32) / error_decay;
        if f > 0.0 && f < 1.0 {
            self.predicted_error.map(|e| e * f)
        } else {
            self.predicted_error_time = 0;
            [0.0; 3]
        }
    }

    /// CG_PredictPlayerState (vanilla, no vehicles/movers yet).
    pub fn predict(&mut self, input: PredictionInput<'_>) -> Result<(), String> {
        let PredictionInput { session, snap, next, time, movement, world, entities, settings, provisional } = input;
        let mut solids = solid_entities(entities);
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
        let next_frame_teleport = next.is_some_and(|next| {
            (next.player_state.field_i32("eFlags").unwrap_or(0) ^ snap.player_state.field_i32("eFlags").unwrap_or(0))
                & EF_TELEPORT_BIT
                != 0
        });
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
        let native = match &mut self.native {
            Some(native) => {
                native.configure(&prediction_settings)?;
                native.set_network(&to_native(base_ps))?;
                native
            }
            slot => slot.insert(NativePlayerState::from_network(&to_native(base_ps))?),
        };

        native.configure(&prediction_settings)?;
        if prediction_settings.server_mod == 1 {
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
        let mut world = PredictionWorld { world, solids: &solids, client_num };

        let mut moved = false;
        let mut command_time = base_ps.field_i32("commandTime").unwrap_or(0);
        for number in first..=current {
            let mut cmd = command(number);
            if prediction_settings.pmove_fixed != 0 {
                movement.update_view_angles(native, movement_cmd(&cmd));
            }
            if cmd.server_time <= command_time || cmd.server_time > latest.server_time {
                continue;
            }
            if command_time == old.field_i32("commandTime").unwrap_or(0) {
                if self.this_frame_teleport {
                    self.predicted_error = [0.0; 3];
                    self.this_frame_teleport = false;
                } else {
                    // CG_AdjustPositionForMover is not applied yet (no mover pushes).
                    let replayed = from_native(native.network());
                    let now = origin(&replayed);
                    let before = origin(&old);
                    let delta = [before[0] - now[0], before[1] - now[1], before[2] - now[2]];
                    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                    if length > 0.1 {
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
            if prediction_settings.pmove_fixed != 0 {
                let msec = prediction_settings.pmove_msec;
                cmd.server_time = ((cmd.server_time + msec - 1) / msec) * msec;
            }
            movement.predict(native, movement_cmd(&cmd), &prediction_settings, &mut world)?;
            command_time = native.view().command_time;
            moved = true;
        }
        // OpenJK leaves cg.predictedPlayerState at the base snapshot's state
        // when no command was replayed.
        self.predicted = Some(if moved { from_native(native.network()) } else { base_ps.clone() });
        self.display = self.predicted.clone();
        self.display_angles = session.command(current).map(|cmd| cmd.angles);
        if let Some(cmd) = provisional {
            let reached = native.view().command_time;
            if prediction_settings.pmove_fixed == 0 && cmd.server_time > reached && cmd.server_time > latest.server_time {
                movement.predict(native, movement_cmd(&cmd), &prediction_settings, &mut world)?;
                self.display = Some(from_native(native.network()));
                self.display_angles = Some(cmd.angles);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let solids = [player];
        let mut prediction = PredictionWorld { world: &mut world, solids: &solids, client_num: 4 };
        let hit = prediction.trace(query);
        assert_eq!(hit.entity, 7);
        assert!((hit.fraction - 18.0 / 64.0).abs() < 0.02, "{}", hit.fraction);
        // The passed entity (ourselves) and owned objects are ignored.
        let ignored = PredictionWorld { world: &mut world, solids: &solids, client_num: 4 }.trace(TraceQuery { pass_entity: 7, ..query });
        assert_eq!(ignored.fraction, 1.0);
        let owned = [SolidEntity { number: 300, generic_enemy_index: 1024 + 4, skip_movement: false, clip: player.clip }];
        assert_eq!(PredictionWorld { world: &mut world, solids: &owned, client_num: 4 }.trace(query).fraction, 1.0);

        // An inline model translated onto the path blocks like CM_TransformedBoxTrace.
        let mover = [SolidEntity {
            number: 90,
            generic_enemy_index: 0,
            skip_movement: false,
            clip: EntityClip::InlineModel { index: 1, origin: [0.0; 3], angles: [0.0; 3] },
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
        let mut net = NetClient::connect(&server, settings.userinfo("kyle/default")).unwrap();
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
                let buttons = CommandButtons { active: &held, any_key: true, talking: false };
                if last_command.is_none_or(|last| last.elapsed().as_millis() >= command_ms) {
                    last_command = Some(std::time::Instant::now());
                    let cmd = input.create_cmd(&buttons);
                    net.session_mut().create_command(cmd);
                }
                let mut preview = input.preview_cmd(&buttons);
                preview.server_time = net.session().server_time();
                provisional = Some(preview);
            }
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
    fn userinfo_contains_stock_keys_in_openjk_order() {
        let info = NetworkSettings::default().userinfo("kyle/default");
        let text = String::from_utf8(info).unwrap();
        assert!(text.starts_with("\\name\\Padawan\\rate\\25000\\snaps\\40\\model\\kyle/default"), "{text}");
        assert!(text.ends_with("\\teamtask\\0"), "{text}");
        assert!(jka_protocol::netchan::connect_packet(text.as_bytes()).is_ok());
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
        assert_eq!((settings.server_mod, settings.jcinfo, settings.jcinfo2), (1, 123, 8));
        assert_eq!((settings.pmove_msec, settings.hook_pull, settings.legacy_fixes), (1, 900, 7));
        config.insert(0, br"\gamename\basejka\jcinfo\123".to_vec());
        let settings = predict_settings(&config, &ps);
        assert_eq!((settings.server_mod, settings.jcinfo, settings.jcinfo2, settings.legacy_fixes), (0, 0, 0, 0));
        assert_eq!(settings.pmove_msec, 8);
        assert_eq!(settings.hook_pull, 0);
        config.clear();
        assert_eq!(predict_settings(&config, &ps).server_mod, 0);
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
        let info = settings.userinfo_for_mod("kyle/default", ServerMod::Base);
        assert_eq!(info_value(&info, b"cjp_client"), None);
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
        let cmd = input.create_cmd(&CommandButtons { active: &active, any_key: true, talking: false });
        assert_eq!(cmd.forward_move, 64);
        assert_eq!(cmd.right_move, -64);
        assert_eq!(cmd.buttons & (1 | 128 | BUTTON_WALKING | BUTTON_ANY), 1 | 128 | BUTTON_WALKING | BUTTON_ANY);
        assert_eq!(cmd.angles[1], jka_movement::angle_to_short(270.0));
        // wasPressed is consumed by one command.
        let cmd = input.create_cmd(&CommandButtons { active: &HashSet::new(), any_key: false, talking: false });
        assert_eq!(cmd.buttons, 0);
        assert_eq!(cmd.forward_move, 0);
    }

    #[test]
    fn generic_commands_are_one_shot() {
        let mut input = LiveInput::default();
        input.queue_generic_command(generic_command("force_throw").unwrap());
        let none = HashSet::new();
        let buttons = CommandButtons { active: &none, any_key: false, talking: false };
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
}
