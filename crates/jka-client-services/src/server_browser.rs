//! Asynchronous JKA and JK2 server discovery.
//!
//! The wire protocol intentionally follows the Quake 3/OpenJK/JK2MV browser:
//! * Internet: `getservers <protocol>` to a master server, then `getinfo` each address.
//! * LAN: broadcast `getinfo` across the selected game's server-port range.
//! * Details: `getstatus` for the selected server's player list.
//!
//! Unlike the legacy client this is isolated on a worker thread.  The UI/main
//! thread only sends commands and consumes immutable results.

use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jka_protocol::{parse_info_response, PROTOCOL_VERSION};

/// Default browser filters: hide bots/connecting, NOT empty/full, exclude MBII variants.
/// Existing archived cl_serverFilters values still take precedence.
pub const DEFAULT_MOD_FILTER: &str = "-MBII,-MBIIOpenBeta";
/// Fourth field is the selected gametype bitmask; 0 means show every mode.
/// A three-field value from older saved configs remains supported.
pub const DEFAULT_SERVER_FILTERS: &str = "12|0|-MBII,-MBIIOpenBeta|0";
/// The game/protocol is part of every query and every result. JK2's two
/// supported versions deliberately have independent browser lists and settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserProtocol {
    Jka,
    Jk2_102,
    Jk2_104,
}

impl BrowserProtocol {
    pub const ALL: [Self; 3] = [Self::Jka, Self::Jk2_102, Self::Jk2_104];

    pub fn label(self) -> &'static str {
        match self {
            Self::Jka => "JKA",
            Self::Jk2_102 => "JK2 1.02",
            Self::Jk2_104 => "JK2 1.04",
        }
    }
    pub fn protocol(self) -> u32 {
        match self {
            Self::Jka => PROTOCOL_VERSION,
            Self::Jk2_102 => jka_protocol::jk2::Version::V102.protocol(),
            Self::Jk2_104 => jka_protocol::jk2::Version::V104.protocol(),
        }
    }
    pub fn master_port(self) -> u16 {
        match self {
            Self::Jka => 29060,
            Self::Jk2_102 | Self::Jk2_104 => 28060,
        }
    }
    pub fn first_server_port(self) -> u16 {
        match self {
            Self::Jka => FIRST_SERVER_PORT,
            Self::Jk2_102 | Self::Jk2_104 => 28070,
        }
    }
    pub fn default_masters(self) -> [&'static str; MAX_MASTER_SLOTS] {
        match self {
            Self::Jka => DEFAULT_MASTER_CVARS,
            Self::Jk2_102 | Self::Jk2_104 => [
                "master.jkhub.org", "master.jk2mv.org", "master.ouned.de", "", "",
            ],
        }
    }
    fn cache_filename(self) -> &'static str {
        match self {
            Self::Jka => "jka.json",
            Self::Jk2_102 => "jk2-102.json",
            Self::Jk2_104 => "jk2-104.json",
        }
    }

    fn settings_suffix(self) -> &'static str {
        match self {
            Self::Jka => "",
            Self::Jk2_102 => "-jk2-102",
            Self::Jk2_104 => "-jk2-104",
        }
    }
}


/// Stock JKA gametypes plus a catch-all for unknown/custom gametype numbers.
/// Values are intentionally server protocol ids, not UI list positions.
pub const GAMETYPE_FILTER_OPTIONS: [(i32, &str); 11] = [
    (0, "FFA"),
    (1, "Holocron"),
    (2, "Jedi Master"),
    (3, "Duel"),
    (4, "Power Duel"),
    (5, "Single Player"),
    (6, "Team FFA"),
    (7, "Siege"),
    (8, "CTF"),
    (9, "CTY"),
    (-1, "Unknown / Custom"),
];
pub const JK2_GAMETYPE_FILTER_OPTIONS: [(i32, &str); 9] = [
    (0, "FFA"), (1, "Holocron"), (2, "Jedi Master"), (3, "Duel"),
    (4, "Single Player"), (5, "Team FFA"), (7, "CTF"),
    (8, "CTY"), (-1, "Unknown / Custom"),
];
const GAMETYPE_FILTER_MASK: u16 = (1 << GAMETYPE_FILTER_OPTIONS.len()) - 1;

/// Unknown values share the single catch-all bit at index 10.
pub fn gametype_filter_bit(gametype: i32) -> u16 {
    if (0..=9).contains(&gametype) { 1 << gametype } else { 1 << 10 }
}

pub const FIRST_SERVER_PORT: u16 = jka_protocol::DEFAULT_PORT;
pub const NUM_SERVER_PORTS: u16 = 4;
const MASTER_WAIT: Duration = Duration::from_millis(1200);
const INFO_WAIT: Duration = Duration::from_millis(2200);
const STATUS_WAIT: Duration = Duration::from_millis(900);
const MAX_MASTER_SERVERS: usize = 4096;
const INTERNET_CACHE_VERSION: u32 = 1;
const MAX_INTERNET_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PACKET: usize = 64 * 1024;

/// TaystJK/OpenJK-style master slots. TaystJK supports five `sv_masterN` cvars:
/// Raven, JKHub, and Ouned by default, with slots 4-5 empty for custom masters.
pub const MAX_MASTER_SLOTS: usize = 5;
pub const DEFAULT_MASTER_CVARS: [&str; MAX_MASTER_SLOTS] = [
    "masterjk3.ravensoft.com",
    "master.jkhub.org",
    "master.ouned.de",
    "",
    "",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerSource {
    Internet,
    Lan,
    Favorites,
    History,
}

impl ServerSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Internet => "INTERNET",
            Self::Lan => "LAN",
            Self::Favorites => "FAVORITES",
            Self::History => "HISTORY",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerEntry {
    pub address: SocketAddr,
    pub hostname: String,
    pub map: String,
    pub game: String,
    pub gametype: i32,
    pub clients: u32,
    pub max_clients: u32,
    pub humans: u32,
    pub bots: u32,
    pub ping_ms: u32,
    pub need_password: bool,
    pub protocol: u32,
}

impl ServerEntry {
    pub fn placeholder(address: SocketAddr) -> Self {
        Self {
            address,
            hostname: address.to_string(),
            map: String::new(),
            game: String::new(),
            gametype: -1,
            clients: 0,
            max_clients: 0,
            humans: 0,
            bots: 0,
            ping_ms: 0,
            need_password: false,
            protocol: PROTOCOL_VERSION,
        }
    }

    pub fn gametype_label(&self) -> &'static str {
        if self.protocol == 15 || self.protocol == 16 {
            return match self.gametype {
                0 => "FFA", 1 => "HOLOCRON", 2 => "JEDI MASTER",
                3 => "DUEL", 4 => "SINGLE PLAYER", 5 => "TEAM FFA",
                7 => "CTF", 8 => "CTY", _ => "UNKNOWN",
            };
        }
        match self.gametype {
            0 => "FFA",
            1 => "HOLOCRON",
            2 => "JEDI MASTER",
            3 => "DUEL",
            4 => "POWER DUEL",
            5 => "SINGLE PLAYER",
            6 => "TEAM FFA",
            7 => "SIEGE",
            8 => "CTF",
            9 => "CTY",
            _ => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerPlayer {
    pub name: String,
    pub score: i32,
    pub ping: i32,
}

#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub address: SocketAddr,
    pub fields: Vec<(String, String)>,
    pub players: Vec<ServerPlayer>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSort {
    Ping,
    Players,
    Name,
    Map,
    Gametype,
    Mod,
    LastVisited,
}

impl BrowserSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ping => "PING",
            Self::Players => "PLAYERS",
            Self::Name => "NAME",
            Self::Map => "MAP",
            Self::Gametype => "MODE",
            Self::Mod => "MOD",
            Self::LastVisited => "LAST VISITED",
        }
    }
}

/// Case-insensitive exact fs_game inclusion/exclusion lists. A leading '-' means
/// exclude; positive entries limit results when present. Exclusions always win.
pub struct ModFilters<'a> {
    included: Vec<&'a str>,
    excluded: Vec<&'a str>,
}

impl<'a> ModFilters<'a> {
    pub fn parse(input: &'a str) -> Self {
        let mut included = Vec::new();
        let mut excluded = Vec::new();
        for token in input.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            if let Some(mod_name) = token.strip_prefix('-') {
                let mod_name = mod_name.trim();
                if !mod_name.is_empty() {
                    excluded.push(mod_name);
                }
            } else {
                included.push(token);
            }
        }
        Self { included, excluded }
    }

    pub fn matches(&self, fs_game: &str) -> bool {
        let game = if fs_game.trim().is_empty() { "base" } else { fs_game.trim() };
        !self.excluded.iter().any(|name| name.eq_ignore_ascii_case(game))
            && (self.included.is_empty()
                || self.included.iter().any(|name| name.eq_ignore_ascii_case(game)))
    }
}

pub struct BrowserUiState {
    pub protocol: BrowserProtocol,
    pub source: ServerSource,
    pub servers: HashMap<SocketAddr, ServerEntry>,
    pub source_addresses: HashMap<ServerSource, HashSet<SocketAddr>>,
    pub selected: Option<SocketAddr>,
    pub search: String,
    /// Case-insensitive map-name substring filter from the MAP column.
    pub map_filter: String,
    /// LAST VISITED filter: 0 all, 1 visited, 2 never visited.
    pub visit_filter: u8,
    pub hide_empty: bool,
    pub hide_full: bool,
    pub hide_bots: bool,
    /// Hide getstatus player rows that report the classic 999 "connecting" ping.
    /// When enabled, cached status data is also used to make the displayed player
    /// counts reflect those hidden rows.
    pub hide_connecting: bool,
    pub mod_filter: String,
    /// Bitmask of allowed gametypes. 0 disables filtering (all modes).
    pub gametype_filter: u16,
    pub max_ping: u32,
    pub sort: BrowserSort,
    pub sort_ascending: bool,
    pub refreshing: HashSet<ServerSource>,
    /// Addresses that have answered during the refresh currently in flight.
    /// We deliberately keep the previous list visible until RefreshFinished,
    /// then prune entries that did not answer. This avoids the old empty-list
    /// flash every time Refresh is pressed.
    refresh_seen: HashMap<ServerSource, HashSet<SocketAddr>>,
    pub status_text: String,
    pub source_status: HashMap<ServerSource, String>,
    pub details: Option<ServerStatus>,
    /// Cached getstatus replies used by the details pane and player-row filters.
    /// Bot and connecting counts come from these per-player pings when available;
    /// getinfo's advertised counts are only a fallback.
    pub status_cache: HashMap<SocketAddr, ServerStatus>,
    pub status_pending: HashSet<SocketAddr>,
    pub player_search: String,
    pub player_exact_match: bool,
    pub player_search_last_query: Option<Instant>,
    pub favorites: Vec<SocketAddr>,
    pub history: Vec<SocketAddr>,
    /// Last successful visit time for history entries, in Unix milliseconds.
    /// Legacy plain-address history rows simply have no timestamp here.
    pub history_last_visited: HashMap<SocketAddr, u64>,
    pub direct_connect: String,
    pub internet_requested_once: bool,
    /// `(source, address)` for a full server that should be polled until a
    /// slot opens. The application owns the eventual connect action.
    pub autojoin: Option<(ServerSource, SocketAddr)>,
    pub autojoin_last_query: Option<Instant>,
    /// Exact archived TaystJK-compatible `sv_master1`..`sv_master5` values.
    /// Empty slots are disabled.
    pub master_servers: [String; MAX_MASTER_SLOTS],
    /// UI edit buffer retained while a slot is temporarily unchecked.
    pub master_drafts: [String; MAX_MASTER_SLOTS],
    favorites_path: std::path::PathBuf,
    history_path: std::path::PathBuf,
    master_path: Option<std::path::PathBuf>,
    /// Persisted Internet discovery is shared across mods, but isolated by protocol.
    internet_cache_path: Option<std::path::PathBuf>,
}

impl BrowserUiState {
    /// `cl_serverFilters` value: `<flags>|<max ping>|<mod filter>|<gametype mask>`,
    /// where flags are bit 0 empty, 1 full, 2 bots, 3 connecting.
    /// The final mask is optional when reading old three-field values.
    /// Existing 3/4-field cl_serverFilters strings remain readable. The optional
    /// trailing fields persist the new per-column searches without changing the
    /// common/default value or silently discarding legacy settings.
    pub fn filters_string(&self) -> String {
        let flags = self.hide_empty as u8
            | (self.hide_full as u8) << 1
            | (self.hide_bots as u8) << 2
            | (self.hide_connecting as u8) << 3;
        let mut value = format!("{flags}|{}|{}|{}", self.max_ping, self.mod_filter.replace('|', ""), self.gametype_filter);
        if !self.search.is_empty() || !self.map_filter.is_empty()
            || !self.player_search.is_empty() || self.visit_filter != 0 || self.player_exact_match
        {
            value.push_str(&format!("|{}|{}|{}|{}|{}",
                self.search.replace('|', ""),
                self.map_filter.replace('|', ""),
                self.player_search.replace('|', ""),
                self.visit_filter,
                self.player_exact_match as u8,
            ));
        }
        value
    }

    pub fn apply_filters_string(&mut self, value: &str) {
        // Migrate ONLY the old shipped default, not user-selected settings.
        // That default mistakenly hid Empty and left Bots visible.
        let value = if value.trim().is_empty()
            || matches!(value.trim(), "9|0|-MBII,-MBIIOpenBeta|0" | "9|0|-MBII,-MBIIOpenBeta")
        {
            DEFAULT_SERVER_FILTERS
        } else {
            value
        };
        let mut parts = value.splitn(9, '|');
        let flags = parts.next().and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(0);
        self.hide_empty = flags & 1 != 0;
        self.hide_full = flags & 2 != 0;
        self.hide_bots = flags & 4 != 0;
        self.hide_connecting = flags & 8 != 0;
        self.max_ping = parts.next().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0).min(999);
        self.mod_filter = parts.next().unwrap_or_default().trim().to_owned();
        self.gametype_filter = parts.next()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .unwrap_or(0) & GAMETYPE_FILTER_MASK;
        self.search = parts.next().unwrap_or_default().to_owned();
        self.map_filter = parts.next().unwrap_or_default().to_owned();
        self.player_search = parts.next().unwrap_or_default().to_owned();
        self.visit_filter = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(0).min(2);
        self.player_exact_match = parts.next() == Some("1");
        self.player_search_last_query = None;
    }

    pub fn matches_gametype(&self, gametype: i32) -> bool {
        self.gametype_filter == 0 || self.gametype_filter & gametype_filter_bit(gametype) != 0
    }

    /// A concise label for the multi-select gametype dropdown.
    pub fn gametype_filter_label(&self) -> String {
        let selected = self.gametype_filter.count_ones();
        match selected {
            0 => "All gametypes".to_owned(),
            1 => self.gametype_options()
                .iter()
                .find(|(gametype, _)| self.gametype_filter & gametype_filter_bit(*gametype) != 0)
                .map(|(_, label)| (*label).to_owned())
                .unwrap_or_else(|| "All gametypes".to_owned()),
            n => format!("{n} gametypes"),
        }
    }

    pub fn gametype_options(&self) -> &'static [(i32, &'static str)] {
        match self.protocol {
            BrowserProtocol::Jka => &GAMETYPE_FILTER_OPTIONS,
            BrowserProtocol::Jk2_102 | BrowserProtocol::Jk2_104 => &JK2_GAMETYPE_FILTER_OPTIONS,
        }
    }

    pub fn new(
        settings_dir: &Path,
        master_servers: [String; MAX_MASTER_SLOTS],
    ) -> Self {
        Self::new_for_protocol(settings_dir, master_servers, BrowserProtocol::Jka)
    }

    pub fn new_for_protocol(
        settings_dir: &Path,
        master_servers: [String; MAX_MASTER_SLOTS],
        protocol: BrowserProtocol,
    ) -> Self {
        let suffix = protocol.settings_suffix();
        let favorites_path = settings_dir.join(format!("server-favorites{suffix}.txt"));
        let history_path = settings_dir.join(format!("server-history{suffix}.txt"));
        let master_path = (protocol != BrowserProtocol::Jka)
            .then(|| settings_dir.join(format!("server-masters{suffix}.txt")));
        let master_servers = master_path.as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| {
                let rows: Vec<_> = text.lines().map(str::to_owned).collect();
                (rows.len() == MAX_MASTER_SLOTS).then(|| std::array::from_fn(|i| rows[i].clone()))
            })
            .unwrap_or(master_servers);
        let mut favorites = load_address_list(&favorites_path);
        favorites.sort_unstable();
        let mut history_rows = load_history_list(&history_path);
        // Keep the most recent history entries bounded. Legacy files containing
        // only plain host:port rows remain valid; newly written rows append a
        // Unix-millisecond last-visited timestamp.
        if history_rows.len() > 64 {
            history_rows = history_rows.split_off(history_rows.len() - 64);
        }
        let history: Vec<_> = history_rows.iter().map(|(address, _)| *address).collect();
        let history_last_visited = history_rows
            .into_iter()
            .filter_map(|(address, visited)| visited.map(|visited| (address, visited)))
            .collect();
        let mut master_drafts = master_servers.clone();
        for (slot, default) in protocol.default_masters().iter().enumerate() {
            if master_drafts[slot].trim().is_empty() && !default.is_empty() {
                master_drafts[slot] = (*default).to_owned();
            }
        }
        Self {
            protocol,
            source: ServerSource::Internet,
            servers: HashMap::new(),
            source_addresses: HashMap::new(),
            selected: None,
            search: String::new(),
            map_filter: String::new(),
            visit_filter: 0,
            hide_empty: false,
            hide_full: false,
            hide_bots: true,
            hide_connecting: true,
            mod_filter: if protocol == BrowserProtocol::Jka { DEFAULT_MOD_FILTER } else { "" }.to_owned(),
            gametype_filter: 0,
            max_ping: 0,
            sort: BrowserSort::Ping,
            sort_ascending: true,
            refreshing: HashSet::new(),
            refresh_seen: HashMap::new(),
            status_text: "Ready".to_owned(),
            source_status: HashMap::new(),
            details: None,
            status_cache: HashMap::new(),
            status_pending: HashSet::new(),
            player_search: String::new(),
            player_exact_match: false,
            player_search_last_query: None,
            favorites,
            history,
            history_last_visited,
            direct_connect: String::new(),
            internet_requested_once: false,
            autojoin: None,
            autojoin_last_query: None,
            master_servers,
            master_drafts,
            favorites_path,
            history_path,
            master_path,
            internet_cache_path: None,
        }
    }

    /// Use <GameData>/DinurdoJK/cache/serverbrowser for all game tabs.
    /// Cached rows are displayed immediately, but their old pings are not reused.
    /// Discovery is still started only when the Join page opens for this protocol.
    pub fn with_internet_cache(mut self, base_game_directory: &Path) -> Self {
        let path = jka_assets::pk3::dinurdojk_cache_directory(base_game_directory).join("serverbrowser")
            .join(self.protocol.cache_filename());
        if let Some(servers) = load_internet_cache(&path, self.protocol) {
            let count = servers.len();
            let addresses = self.source_addresses.entry(ServerSource::Internet).or_default();
            for server in servers {
                addresses.insert(server.address);
                self.servers.insert(server.address, server);
            }
            if count > 0 {
                let text = format!("{count} cached servers (refresh pending)");
                self.source_status.insert(ServerSource::Internet, text.clone());
                self.status_text = text;
            }
        }
        self.internet_cache_path = Some(path);
        self
    }

    pub fn internet_cache_path(&self) -> Option<&Path> {
        self.internet_cache_path.as_deref()
    }

    /// JK2 master configuration is independent of archived JKA sv_masterN cvars.
    pub fn set_jk2_masters(&mut self, updates: &[(usize, String)]) -> Result<(), String> {
        let Some(path) = &self.master_path else { return Ok(()); };
        let mut next = self.master_servers.clone();
        for (slot, value) in updates {
            if *slot < MAX_MASTER_SLOTS { next[*slot] = value.clone(); }
        }
        std::fs::write(path, next.join("\n") + "\n")
            .map_err(|error| format!("Could not save JK2 masters: {error}"))?;
        self.master_servers = next;
        Ok(())
    }

    pub fn handle_event(&mut self, event: BrowserEvent) {
        match event {
            BrowserEvent::RefreshStarted(source) => {
                self.refreshing.insert(source);
                let text = format!("Refreshing {}…", source.label());
                self.source_status.insert(source, text.clone());
                self.status_text = text;
                self.refresh_seen.insert(source, HashSet::new());
            }
            BrowserEvent::Server { source, server } => {
                let address = server.address;
                // A fresh getinfo means any previous per-player breakdown can
                // be stale. The next status batch rechecks it.
                self.status_cache.remove(&address);
                self.servers.insert(address, server);
                self.source_addresses.entry(source).or_default().insert(address);
                if self.refreshing.contains(&source) {
                    self.refresh_seen.entry(source).or_default().insert(address);
                }
                let text = format!("Receiving {} servers…", source.label());
                self.source_status.insert(source, text.clone());
                self.status_text = text;
            }
            BrowserEvent::ServerUpdate { source, server, quiet } => {
                let address = server.address;
                if !quiet {
                    self.status_cache.remove(&address);
                }
                self.servers.insert(address, server);
                self.source_addresses.entry(source).or_default().insert(address);
                if !quiet {
                    let text = format!("Updated {address}");
                    self.source_status.insert(source, text.clone());
                    self.status_text = text;
                }
            }
            BrowserEvent::RefreshFinished { source, discovered, responded } => {
                self.refreshing.remove(&source);
                // Internet/LAN are discovery sources, so a completed refresh is
                // the authoritative new set. Favorites/history deliberately keep
                // offline entries so the user can still retry them later.
                // A master outage / total query timeout must not erase the last
                // known-good Internet list or its on-disk cache.
                let preserve_internet = source == ServerSource::Internet && responded == 0;
                if matches!(source, ServerSource::Internet | ServerSource::Lan) && !preserve_internet {
                    if let Some(seen) = self.refresh_seen.remove(&source) {
                        if let Some(addresses) = self.source_addresses.get_mut(&source) {
                            addresses.retain(|address| seen.contains(address));
                        }
                    }
                } else {
                    self.refresh_seen.remove(&source);
                }
                let text = if preserve_internet && !self.addresses_for_source(source).is_empty() {
                    format!("INTERNET: no servers responded; showing cached results")
                } else if discovered == 0 {
                    format!("{}: no servers found", source.label())
                } else {
                    format!("{}: {responded}/{discovered} servers responded", source.label())
                };
                self.source_status.insert(source, text.clone());
                self.status_text = text;
            }
            BrowserEvent::Status(status) | BrowserEvent::ServerInfo(status) => {
                self.status_pending.remove(&status.address);
                if self.selected == Some(status.address) {
                    self.details = Some(status.clone());
                }
                self.status_cache.insert(status.address, status);
            }
            // Handled by App's console event receiver, not browser UI state.
            BrowserEvent::ConsoleServerStatus { .. } => {}
            BrowserEvent::StatusBatchFinished { requested } => {
                for address in requested {
                    if self.status_pending.remove(&address) {
                        // Record a completed no-response/empty result so player
                        // filtering does not leave that row permanently in the
                        // optimistic "still querying" state.
                        self.status_cache.insert(address, ServerStatus {
                            address,
                            fields: Vec::new(),
                            players: Vec::new(),
                        });
                    }
                }
            }
            BrowserEvent::Error { source, message } => {
                if let Some(source) = source {
                    self.refreshing.remove(&source);
                    self.refresh_seen.remove(&source);
                    self.source_status.insert(source, message.clone());
                }
                self.status_text = message;
            }
        }
    }

    pub fn addresses_for_source(&self, source: ServerSource) -> Vec<SocketAddr> {
        match source {
            ServerSource::Favorites => self.favorites.clone(),
            ServerSource::History => self.history.clone(),
            _ => self
                .source_addresses
                .get(&source)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default(),
        }
    }

    pub fn servers_for_current_source(&self) -> Vec<ServerEntry> {
        self.addresses_for_source(self.source)
            .into_iter()
            .map(|address| {
                self.servers
                    .get(&address)
                    .cloned()
                    .unwrap_or_else(|| {
                        let mut entry = ServerEntry::placeholder(address);
                        entry.protocol = self.protocol.protocol();
                        entry
                    })
            })
            .collect()
    }

    pub fn is_favorite(&self, address: SocketAddr) -> bool {
        self.favorites.contains(&address)
    }

    pub fn toggle_favorite(&mut self, address: SocketAddr) -> Result<bool, String> {
        let added = if let Some(index) = self.favorites.iter().position(|item| *item == address) {
            self.favorites.remove(index);
            false
        } else {
            self.favorites.push(address);
            self.favorites.sort_unstable();
            self.favorites.dedup();
            true
        };
        save_address_list(&self.favorites_path, &self.favorites)?;
        Ok(added)
    }

    pub fn remember_history(&mut self, address: SocketAddr) -> Result<(), String> {
        let visited = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.history.retain(|item| *item != address);
        self.history.push(address);
        self.history_last_visited.insert(address, visited);
        if self.history.len() > 64 {
            let excess = self.history.len() - 64;
            for stale in self.history.drain(..excess) {
                self.history_last_visited.remove(&stale);
            }
        }
        save_history_list(&self.history_path, &self.history, &self.history_last_visited)
    }

    pub fn history_last_visited(&self, address: SocketAddr) -> Option<u64> {
        self.history_last_visited.get(&address).copied()
    }

    /// (total, humans, bots, connecting) using the EternalJK zero-ping rule.
    /// Valid getstatus replies take priority over untrusted getinfo bot counters.
    /// No-response/invalid status entries retain the getinfo fallback.
    pub fn player_breakdown(&self, server: &ServerEntry) -> (u32, u32, u32, u32) {
        if let Some(status) = self.status_cache.get(&server.address)
            .filter(|status| !status.fields.is_empty())
        {
            let total = status.players.len() as u32;
            let bots = status.players.iter().filter(|player| player.ping == 0).count() as u32;
            let connecting = status.players.iter().filter(|player| player.ping == 999).count() as u32;
            return (total, total - bots, bots, connecting);
        }
        let humans = server.humans.min(server.clients);
        (server.clients, humans, server.bots.min(server.clients), 0)
    }

    /// Player count after bot and connecting filters, consistent with the
    /// detailed player list. Until getstatus responds, use getinfo estimates.
    pub fn visible_client_count(&self, server: &ServerEntry) -> u32 {
        let (total, humans, _, connecting) = self.player_breakdown(server);
        let without_bots = if self.hide_bots { humans } else { total };
        if self.hide_connecting { without_bots.saturating_sub(connecting) } else { without_bots }
    }
}

#[derive(Debug)]
pub struct BrowserRequest {
    pub protocol: BrowserProtocol,
    pub command: BrowserCommand,
}

#[derive(Debug)]
pub struct TaggedBrowserEvent {
    pub protocol: BrowserProtocol,
    pub event: BrowserEvent,
}

#[derive(Debug)]
pub enum BrowserCommand {
    RefreshInternet { masters: Vec<String>, cache_path: Option<std::path::PathBuf> },
    RefreshLan,
    RefreshAddresses {
        source: ServerSource,
        addresses: Vec<SocketAddr>,
    },
    /// Refresh one row in place without starting a source-wide refresh.
    RefreshServer {
        source: ServerSource,
        address: SocketAddr,
        quiet: bool,
    },
    QueryStatus(SocketAddr),
    /// Fresh full getstatus request initiated explicitly by the user.
    GetServerInfo(SocketAddr),
    /// Console /serverstatus, deliberately separate from browser row queries.
    ConsoleServerStatus(SocketAddr),
    QueryStatusBatch(Vec<SocketAddr>),
}

#[derive(Debug)]
pub enum BrowserEvent {
    RefreshStarted(ServerSource),
    Server {
        source: ServerSource,
        server: ServerEntry,
    },
    ServerUpdate {
        source: ServerSource,
        server: ServerEntry,
        quiet: bool,
    },
    RefreshFinished {
        source: ServerSource,
        discovered: usize,
        responded: usize,
    },
    Status(ServerStatus),
    /// Full getstatus result requested from the server-row context menu.
    ServerInfo(ServerStatus),
    /// Console-only result, including timeouts. No selection/clipboard effects.
    ConsoleServerStatus { address: SocketAddr, result: Result<ServerStatus, String> },
    StatusBatchFinished {
        requested: Vec<SocketAddr>,
    },
    Error {
        source: Option<ServerSource>,
        message: String,
    },
}

pub fn spawn() -> Result<(Sender<BrowserRequest>, Receiver<TaggedBrowserEvent>), String> {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::Builder::new()
        .name("server-browser".into())
        .spawn(move || worker(command_rx, event_tx))
        .map_err(|error| format!("Could not start server browser worker: {error}"))?;
    Ok((command_tx, event_rx))
}

fn worker(commands: Receiver<BrowserRequest>, event_tx: Sender<TaggedBrowserEvent>) {
    while let Ok(BrowserRequest { protocol, command }) = commands.recv() {
        let events = ScopedEventSender { sender: &event_tx, protocol };
        let result = match command {
            BrowserCommand::RefreshInternet { masters, cache_path } => refresh_internet(&events, masters, cache_path.as_deref()),
            BrowserCommand::RefreshLan => refresh_lan(&events),
            BrowserCommand::RefreshAddresses { source, addresses } => {
                refresh_addresses(&events, source, addresses)
            }
            BrowserCommand::RefreshServer { source, address, quiet } => {
                refresh_server(&events, source, address, quiet)
            }
            BrowserCommand::QueryStatus(address) => query_status(&events, address),
            BrowserCommand::GetServerInfo(address) => get_server_info(&events, address),
            BrowserCommand::ConsoleServerStatus(address) => {
                let result = fetch_status(address).map_err(|(_, message)| message);
                events.send(BrowserEvent::ConsoleServerStatus { address, result })
                    .map_err(|_| (None, "browser UI disconnected".to_owned()))
            }
            BrowserCommand::QueryStatusBatch(addresses) => {
                let requested = addresses.clone();
                let result = query_status_batch(&events, addresses);
                if result.is_err() {
                    let _ = events.send(BrowserEvent::StatusBatchFinished { requested });
                }
                result
            }
        };
        if let Err((source, message)) = result {
            if events.send(BrowserEvent::Error { source, message }).is_err() {
                break;
            }
        }
    }
}

/// Retain the simple event API inside worker jobs while tagging each event
/// with its originating protocol, even after a different UI tab is selected.
struct ScopedEventSender<'a> {
    sender: &'a Sender<TaggedBrowserEvent>,
    protocol: BrowserProtocol,
}
impl ScopedEventSender<'_> {
    fn send(&self, event: BrowserEvent) -> Result<(), ()> {
        self.sender.send(TaggedBrowserEvent { protocol: self.protocol, event }).map_err(|_| ())
    }
}

type WorkerResult = Result<(), (Option<ServerSource>, String)>;

fn refresh_internet(
    events: &ScopedEventSender<'_>,
    masters: Vec<String>,
    cache_path: Option<&Path>,
) -> WorkerResult {
    let source = ServerSource::Internet;
    events
        .send(BrowserEvent::RefreshStarted(source))
        .map_err(|_| (Some(source), "browser UI disconnected".to_owned()))?;
    let addresses = discover_master_servers(&masters, events.protocol).map_err(|message| (Some(source), message))?;
    query_info_batch(events, source, addresses, cache_path)
}

fn refresh_lan(events: &ScopedEventSender<'_>) -> WorkerResult {
    let source = ServerSource::Lan;
    events
        .send(BrowserEvent::RefreshStarted(source))
        .map_err(|_| (Some(source), "browser UI disconnected".to_owned()))?;

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| (Some(source), format!("LAN browser socket: {e}")))?;
    socket
        .set_broadcast(true)
        .map_err(|e| (Some(source), format!("LAN broadcast: {e}")))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(75)))
        .map_err(|e| (Some(source), format!("LAN browser timeout: {e}")))?;

    let challenge = challenge_token();
    let request = jka_protocol::getinfo_request(&challenge)
        .map_err(|e| (Some(source), format!("LAN getinfo: {e}")))?;
    let started = Instant::now();
    for _ in 0..2 {
        for offset in 0..NUM_SERVER_PORTS {
            let address = SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::BROADCAST,
                events.protocol.first_server_port() + offset,
            ));
            socket
                .send_to(&request, address)
                .map_err(|e| (Some(source), format!("LAN getinfo send: {e}")))?;
        }
    }

    let mut buf = vec![0u8; MAX_PACKET];
    let mut seen = HashSet::new();
    let mut responded = 0usize;
    while started.elapsed() < INFO_WAIT {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                if seen.contains(&from) {
                    continue;
                }
                let Ok(info) = parse_info_response(&buf[..len], &challenge) else {
                    continue;
                };
                if info.protocol != events.protocol.protocol() {
                    continue;
                }
                seen.insert(from);
                let server = server_from_info(from, &info, started.elapsed());
                responded += 1;
                if events.send(BrowserEvent::Server { source, server }).is_err() {
                    return Ok(());
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                return Err((Some(source), format!("LAN browser receive: {error}")));
            }
        }
    }

    let _ = events.send(BrowserEvent::RefreshFinished {
        source,
        discovered: responded,
        responded,
    });
    Ok(())
}

fn refresh_addresses(
    events: &ScopedEventSender<'_>,
    source: ServerSource,
    addresses: Vec<SocketAddr>,
) -> WorkerResult {
    events
        .send(BrowserEvent::RefreshStarted(source))
        .map_err(|_| (Some(source), "browser UI disconnected".to_owned()))?;
    query_info_batch(events, source, addresses, None)
}

fn refresh_server(
    events: &ScopedEventSender<'_>,
    source: ServerSource,
    address: SocketAddr,
    quiet: bool,
) -> WorkerResult {
    if !address.is_ipv4() {
        return Ok(());
    }

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| (Some(source), format!("server query socket: {e}")))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(80)))
        .map_err(|e| (Some(source), format!("server query timeout: {e}")))?;
    let challenge = challenge_token();
    let request = jka_protocol::getinfo_request(&challenge)
        .map_err(|e| (Some(source), format!("getinfo: {e}")))?;
    let sent = Instant::now();
    socket
        .send_to(&request, address)
        .map_err(|e| (Some(source), format!("getinfo {address}: {e}")))?;

    let mut buf = vec![0u8; MAX_PACKET];
    while sent.elapsed() < STATUS_WAIT {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) if from == address => {
                let Ok(info) = parse_info_response(&buf[..len], &challenge) else {
                    continue;
                };
                if info.protocol != events.protocol.protocol() {
                    continue;
                }
                let server = server_from_info(address, &info, sent.elapsed());
                let _ = events.send(BrowserEvent::ServerUpdate { source, server, quiet });
                // A user-requested one-row refresh should also keep the selected
                // details/player cache current. Autojoin uses quiet refreshes and
                // only needs getinfo's slot count, so do not double its polling
                // cost with a getstatus round-trip every cycle.
                if !quiet {
                    let _ = query_status(events, address);
                }
                return Ok(());
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err((Some(source), format!("getinfo receive: {error}"))),
        }
    }

    // Preserve the old row if a single-server refresh times out. Source-wide
    // refreshes are responsible for pruning stale discovery results.
    Ok(())
}

fn discover_master_servers(masters: &[String], protocol: BrowserProtocol) -> Result<Vec<SocketAddr>, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| format!("master browser socket: {e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| format!("master browser timeout: {e}"))?;

    // Ask for the complete candidate set; empty/full filtering is instant and local in the UI.
    let mut request = b"\xff\xff\xff\xffgetservers ".to_vec();
    request.extend_from_slice(protocol.protocol().to_string().as_bytes());
    request.extend_from_slice(b" empty full");
    let enabled: Vec<_> = masters
        .iter()
        .map(|master| master.trim())
        .filter(|master| !master.is_empty())
        .collect();
    if enabled.is_empty() {
        return Err("No master servers enabled; sv_master1..sv_master5 are empty".to_owned());
    }

    let mut resolved = Vec::new();
    for master in enabled {
        let addresses = if master.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) {
            master.to_socket_addrs()
        } else {
            (master, protocol.master_port()).to_socket_addrs()
        };
        match addresses {
            Ok(addresses) => {
                for address in addresses.filter(SocketAddr::is_ipv4) {
                    resolved.push(address);
                }
            }
            Err(error) => eprintln!("SERVER BROWSER: could not resolve {master}: {error}"),
        }
    }
    resolved.sort_unstable();
    resolved.dedup();
    if resolved.is_empty() {
        return Err("No enabled master server could be resolved".to_owned());
    }

    for address in &resolved {
        let _ = socket.send_to(&request, address);
    }

    let started = Instant::now();
    let mut buf = vec![0u8; MAX_PACKET];
    let mut servers = HashSet::new();
    while started.elapsed() < MASTER_WAIT && servers.len() < MAX_MASTER_SERVERS {
        match socket.recv_from(&mut buf) {
            Ok((len, _)) => {
                for address in parse_getservers_response(&buf[..len]) {
                    servers.insert(address);
                    if servers.len() >= MAX_MASTER_SERVERS {
                        break;
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(format!("master server receive: {error}")),
        }
    }

    let mut servers: Vec<_> = servers.into_iter().collect();
    servers.sort_unstable();
    Ok(servers)
}

fn query_info_batch(
    events: &ScopedEventSender<'_>,
    source: ServerSource,
    mut addresses: Vec<SocketAddr>,
    cache_path: Option<&Path>,
) -> WorkerResult {
    addresses.retain(SocketAddr::is_ipv4);
    addresses.sort_unstable();
    addresses.dedup();
    let discovered = addresses.len();
    if addresses.is_empty() {
        let _ = events.send(BrowserEvent::RefreshFinished {
            source,
            discovered: 0,
            responded: 0,
        });
        return Ok(());
    }

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| (Some(source), format!("server query socket: {e}")))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|e| (Some(source), format!("server query timeout: {e}")))?;
    let challenge = challenge_token();
    let request = jka_protocol::getinfo_request(&challenge)
        .map_err(|e| (Some(source), format!("getinfo: {e}")))?;

    let mut sent_at = HashMap::with_capacity(addresses.len());
    for (index, address) in addresses.iter().copied().enumerate() {
        if socket.send_to(&request, address).is_ok() {
            sent_at.insert(address, Instant::now());
        }
        // Avoid dumping a full public master list into the socket queue in one burst.
        if index % 64 == 63 {
            thread::sleep(Duration::from_millis(4));
        }
    }

    let started = Instant::now();
    let mut buf = vec![0u8; MAX_PACKET];
    let mut responded_addresses = HashSet::new();
    let mut responded = 0usize;
    let mut cached_servers = Vec::new();
    while started.elapsed() < INFO_WAIT && responded_addresses.len() < sent_at.len() {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let Some(sent) = sent_at.get(&from).copied() else {
                    continue;
                };
                if responded_addresses.contains(&from) {
                    continue;
                }
                let Ok(info) = parse_info_response(&buf[..len], &challenge) else {
                    continue;
                };
                if info.protocol != events.protocol.protocol() {
                    continue;
                }
                responded_addresses.insert(from);
                let server = server_from_info(from, &info, sent.elapsed());
                responded += 1;
                if cache_path.is_some() {
                    cached_servers.push(server.clone());
                }
                if events.send(BrowserEvent::Server { source, server }).is_err() {
                    return Ok(());
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                return Err((Some(source), format!("server browser receive: {error}")));
            }
        }
    }

    // Disk I/O stays on the browser worker; the UI/render thread never writes
    // server caches. Replace the file only after a completed, nonempty scan.
    if responded > 0 {
        if let Some(path) = cache_path {
            if let Err(error) = save_internet_cache(path, events.protocol, &cached_servers) {
                eprintln!("SERVER BROWSER: cache write failed: {error}");
            }
        }
    }
    let _ = events.send(BrowserEvent::RefreshFinished {
        source,
        discovered,
        responded,
    });
    Ok(())
}

fn fetch_status(address: SocketAddr) -> Result<ServerStatus, (Option<ServerSource>, String)> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| (None, format!("server status socket: {e}")))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(80)))
        .map_err(|e| (None, format!("server status timeout: {e}")))?;
    socket
        .send_to(b"\xff\xff\xff\xffgetstatus", address)
        .map_err(|e| (None, format!("getstatus {address}: {e}")))?;

    let started = Instant::now();
    let mut buf = vec![0u8; MAX_PACKET];
    while started.elapsed() < STATUS_WAIT {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) if from == address => {
                return parse_status_response(address, &buf[..len])
                    .ok_or_else(|| (None, format!("Malformed getstatus response from {address}")));
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err((None, format!("getstatus receive: {error}"))),
        }
    }
    Err((None, format!("Server status timed out: {address}")))
}

fn query_status(events: &ScopedEventSender<'_>, address: SocketAddr) -> WorkerResult {
    let status = fetch_status(address)?;
    let _ = events.send(BrowserEvent::Status(status));
    Ok(())
}

fn get_server_info(events: &ScopedEventSender<'_>, address: SocketAddr) -> WorkerResult {
    let status = fetch_status(address)?;
    let _ = events.send(BrowserEvent::ServerInfo(status));
    Ok(())
}

fn query_status_batch(events: &ScopedEventSender<'_>, mut addresses: Vec<SocketAddr>) -> WorkerResult {
    addresses.retain(SocketAddr::is_ipv4);
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        let _ = events.send(BrowserEvent::StatusBatchFinished { requested: addresses });
        return Ok(());
    }

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| (None, format!("server status socket: {e}")))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|e| (None, format!("server status timeout: {e}")))?;

    let request = b"\xff\xff\xff\xffgetstatus";
    let requested = addresses.clone();
    let wanted: HashSet<_> = addresses.iter().copied().collect();
    for (index, address) in addresses.iter().copied().enumerate() {
        let _ = socket.send_to(request, address);
        if index % 64 == 63 {
            thread::sleep(Duration::from_millis(3));
        }
    }

    let started = Instant::now();
    let mut buf = vec![0u8; MAX_PACKET];
    let mut responded = HashSet::new();
    while started.elapsed() < STATUS_WAIT && responded.len() < wanted.len() {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                if !wanted.contains(&from) || responded.contains(&from) {
                    continue;
                }
                if let Some(status) = parse_status_response(from, &buf[..len]) {
                    responded.insert(from);
                    if events.send(BrowserEvent::Status(status)).is_err() {
                        return Ok(());
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err((None, format!("getstatus receive: {error}"))),
        }
    }

    let _ = events.send(BrowserEvent::StatusBatchFinished { requested });
    Ok(())
}

fn challenge_token() -> String {
    // ASCII alphanumeric so it can be parsed by jka_protocol's strict getinfo helper.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("djk{nanos:08x}")
}

fn server_from_info(
    address: SocketAddr,
    info: &jka_protocol::ServerInfo,
    elapsed: Duration,
) -> ServerEntry {
    let text = |key: &[u8]| -> String {
        info.get(key)
            .map(|value| String::from_utf8_lossy(value).into_owned())
            .unwrap_or_default()
    };
    let number = |key: &[u8]| -> u32 { text(key).parse().unwrap_or(0) };
    let signed = |key: &[u8]| -> i32 { text(key).parse().unwrap_or(-1) };
    let clients = number(b"clients");
    let bots = number(b"bots");
    let humans = info
        .get(b"g_humanplayers")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| clients.saturating_sub(bots));
    // `game` is the traditional idTech3 server-browser key for the active
    // game directory. Some compatible servers expose the literal `fs_game`
    // key instead, so accept that as a fallback for filtering/display.
    let game = {
        let game = text(b"game");
        if game.is_empty() { text(b"fs_game") } else { game }
    };
    ServerEntry {
        address,
        hostname: text(b"hostname"),
        map: text(b"mapname"),
        game,
        gametype: signed(b"gametype"),
        clients,
        max_clients: number(b"sv_maxclients"),
        humans,
        bots,
        ping_ms: elapsed.as_millis().min(u32::MAX as u128) as u32,
        need_password: number(b"needpass") != 0,
        protocol: info.protocol,
    }
}

/// Parse the classic JKA master reply: `getserversResponse` followed by a
/// sequence of `\\ + IPv4[4] + port_be[2]`, normally terminated by `\\EOT`.
pub fn parse_getservers_response(packet: &[u8]) -> Vec<SocketAddr> {
    const PREFIX: &[u8] = b"\xff\xff\xff\xffgetserversResponse";
    let Some(mut cursor) = packet.strip_prefix(PREFIX) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    while !cursor.is_empty() {
        if cursor.starts_with(b"\\EOT") {
            break;
        }
        if cursor.first() != Some(&b'\\') || cursor.len() < 7 {
            break;
        }
        let ip = Ipv4Addr::new(cursor[1], cursor[2], cursor[3], cursor[4]);
        let port = u16::from_be_bytes([cursor[5], cursor[6]]);
        if !ip.is_unspecified() && port != 0 {
            result.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
        }
        cursor = &cursor[7..];
    }
    result
}

/// Render the complete getstatus payload in a readable, copyable form.
/// Every server-info key and every returned player row is included.
pub fn format_server_status(status: &ServerStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!("getstatus {}\n", status.address));
    out.push_str("Server settings:\n");
    if status.fields.is_empty() {
        out.push_str("  <no server-info fields returned>\n");
    } else {
        let key_width = status
            .fields
            .iter()
            .map(|(key, _)| key.len())
            .max()
            .unwrap_or(0)
            .min(32);
        for (key, value) in &status.fields {
            out.push_str(&format!("  {key:<key_width$}  {value}\n"));
        }
    }

    out.push_str("Players:\n");
    if status.players.is_empty() {
        out.push_str("  <no players returned>\n");
    } else {
        out.push_str("  #   score  ping  name\n");
        for (index, player) in status.players.iter().enumerate() {
            out.push_str(&format!(
                "  {index:<3} {score:>5}  {ping:>4}  {name}\n",
                score = player.score,
                ping = player.ping,
                name = player.name,
            ));
        }
    }
    out.trim_end().to_owned()
}

/// Match OpenJK CL_ServerStatusResponse console layout, rather than the
/// separate copy-friendly layout used by the browser's Get Server Info menu.
pub fn format_openjk_server_status(status: &ServerStatus) -> String {
    let mut out = format!("Server ({})\nServer settings:\n", status.address);
    for (key, value) in &status.fields {
        out.push_str(&format!("{key:<24}{value}\n"));
    }
    out.push_str("\nPlayers:\nnum: score: ping: name:\n");
    for (index, player) in status.players.iter().enumerate() {
        out.push_str(&format!(
            "{index:<2}   {score:<3}    {ping:<3}   \"{name}\"\n",
            score = player.score,
            ping = player.ping,
            name = player.name,
        ));
    }
    out.trim_end_matches('\n').to_owned()
}

fn parse_status_response(address: SocketAddr, packet: &[u8]) -> Option<ServerStatus> {
    const PREFIX: &[u8] = b"\xff\xff\xff\xffstatusResponse\n";
    let body = packet.strip_prefix(PREFIX)?;
    let text = String::from_utf8_lossy(body);
    let mut lines = text.lines();
    let info = lines.next()?;
    let fields = parse_info_string(info);
    let players = lines.filter_map(parse_status_player).collect();
    Some(ServerStatus {
        address,
        fields,
        players,
    })
}

fn parse_info_string(info: &str) -> Vec<(String, String)> {
    let mut parts = info.trim_start_matches('\\').split('\\');
    let mut fields = Vec::new();
    while let Some(key) = parts.next() {
        let Some(value) = parts.next() else { break };
        if !key.is_empty() {
            fields.push((key.to_owned(), value.to_owned()));
        }
    }
    fields
}

fn parse_status_player(line: &str) -> Option<ServerPlayer> {
    // Whitespace between numeric fields isn't guaranteed to be a single space.
    // Preserve spaces in quoted player names rather than splitting the whole row.
    let (score, rest) = line.trim().split_once(char::is_whitespace)?;
    let (ping, name) = rest.trim_start().split_once(char::is_whitespace)?;
    let score = score.parse().ok()?;
    let ping = ping.parse().ok()?;
    let name = name.trim().trim_matches('"').to_owned();
    Some(ServerPlayer { name, score, ping })
}

fn parse_history_list(text: &str) -> Vec<(SocketAddr, Option<u64>)> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(address_text) = fields.next() else { continue };
        let Ok(address) = address_text.parse::<SocketAddr>() else { continue };
        if !seen.insert(address) {
            continue;
        }
        let visited = fields.next().and_then(|value| value.parse::<u64>().ok());
        rows.push((address, visited));
    }
    rows
}

fn load_history_list(path: &Path) -> Vec<(SocketAddr, Option<u64>)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_history_list(&text)
}

fn save_history_list(
    path: &Path,
    addresses: &[SocketAddr],
    last_visited: &HashMap<SocketAddr, u64>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut text = String::from("# address\tlast_visited_unix_ms\n");
    for address in addresses {
        text.push_str(&address.to_string());
        if let Some(visited) = last_visited.get(address) {
            text.push('\t');
            text.push_str(&visited.to_string());
        }
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load_address_list(path: &Path) -> Vec<SocketAddr> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut addresses = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(address) = line.parse::<SocketAddr>() {
            if seen.insert(address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

pub fn save_address_list(path: &Path, addresses: &[SocketAddr]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut text = String::new();
    for address in addresses {
        text.push_str(&address.to_string());
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A versioned snapshot of the last successfully answered Internet servers.
/// Favorites/history, player status and LAN discoveries are deliberately excluded.
#[derive(serde::Serialize, serde::Deserialize)]
struct InternetServerCache {
    version: u32,
    protocol: u32,
    servers: Vec<ServerEntry>,
}

fn load_internet_cache(path: &Path, protocol: BrowserProtocol) -> Option<Vec<ServerEntry>> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > MAX_INTERNET_CACHE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let cached: InternetServerCache = serde_json::from_slice(&bytes).ok()?;
    if cached.version != INTERNET_CACHE_VERSION || cached.protocol != protocol.protocol()
        || cached.servers.len() > MAX_MASTER_SERVERS
    {
        return None;
    }
    let mut seen = HashSet::new();
    Some(cached.servers.into_iter().filter_map(|mut server| {
        if !server.address.is_ipv4() || server.protocol != protocol.protocol()
            || !seen.insert(server.address)
        {
            return None;
        }
        // Old network latency must never appear as a current ping.
        server.ping_ms = 0;
        Some(server)
    }).collect())
}

fn save_internet_cache(
    path: &Path,
    protocol: BrowserProtocol,
    servers: &[ServerEntry],
) -> Result<(), String> {
    let cached = InternetServerCache {
        version: INTERNET_CACHE_VERSION,
        protocol: protocol.protocol(),
        servers: servers.to_vec(),
    };
    let bytes = serde_json::to_vec(&cached)
        .map_err(|error| format!("serialize Internet server cache: {error}"))?;
    if bytes.len() as u64 > MAX_INTERNET_CACHE_BYTES {
        return Err("Internet server cache exceeds size limit".to_owned());
    }
    let parent = path.parent().ok_or("Internet server cache path has no parent")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("{}: {error}", parent.display()))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internet_cache_round_trips_per_protocol_without_reusing_ping() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        // The cache lives in <GameData>/DinurdoJK, a sibling of base/.
        let root = std::env::temp_dir().join(format!("dinurdo-browser-cache-{}-{nonce}", std::process::id()));
        let base = root.join("base");
        for protocol in BrowserProtocol::ALL {
            let address: SocketAddr = "127.0.0.1:29070".parse().unwrap();
            let path = jka_assets::pk3::dinurdojk_cache_directory(&base).join("serverbrowser").join(protocol.cache_filename());
            let mut entry = ServerEntry::placeholder(address);
            entry.protocol = protocol.protocol();
            entry.hostname = protocol.label().to_owned();
            entry.ping_ms = 123;
            save_internet_cache(&path, protocol, &[entry]).unwrap();
            let restored = BrowserUiState::new_for_protocol(
                &base, protocol.default_masters().map(str::to_owned), protocol,
            ).with_internet_cache(&base);
            assert_eq!(restored.servers_for_current_source().len(), 1);
            assert_eq!(restored.servers[&address].hostname, protocol.label());
            assert_eq!(restored.servers[&address].ping_ms, 0);
            assert!(!restored.internet_requested_once);
            assert_eq!(restored.internet_cache_path(), Some(path.as_path()));
            // Another protocol can never mistake this snapshot for its own.
            let other = if protocol == BrowserProtocol::Jka { BrowserProtocol::Jk2_102 } else { BrowserProtocol::Jka };
            assert!(load_internet_cache(&path, other).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn internet_timeout_preserves_cached_rows() {
        let path = Path::new("nonexistent-browser-cache-timeout-test-path");
        let mut browser = BrowserUiState::new(path, DEFAULT_MASTER_CVARS.map(str::to_owned));
        let address: SocketAddr = "127.0.0.1:29070".parse().unwrap();
        browser.handle_event(BrowserEvent::Server {
            source: ServerSource::Internet,
            server: ServerEntry::placeholder(address),
        });
        browser.handle_event(BrowserEvent::RefreshStarted(ServerSource::Internet));
        browser.handle_event(BrowserEvent::RefreshFinished {
            source: ServerSource::Internet, discovered: 0, responded: 0,
        });
        assert_eq!(browser.addresses_for_source(ServerSource::Internet), vec![address]);
        assert!(browser.status_text.contains("cached"));
    }

    #[test]
    fn game_tabs_use_their_own_wire_protocols_and_ports() {
        assert_eq!(BrowserProtocol::Jka.protocol(), 26);
        assert_eq!(BrowserProtocol::Jk2_102.protocol(), 15);
        assert_eq!(BrowserProtocol::Jk2_104.protocol(), 16);
        assert_eq!(BrowserProtocol::Jka.master_port(), 29060);
        assert_eq!(BrowserProtocol::Jk2_102.master_port(), 28060);
        assert_eq!(BrowserProtocol::Jk2_104.first_server_port(), 28070);
        assert_eq!(BrowserProtocol::Jka.first_server_port(), 29070);
    }

    #[test]
    fn jk2_browsers_are_isolated_from_jka_and_each_other() {
        let dir = Path::new("nonexistent-browser-game-tab-test-path");
        let mut jka = BrowserUiState::new(dir, DEFAULT_MASTER_CVARS.map(str::to_owned));
        let mut jk2 = BrowserUiState::new_for_protocol(
            dir, BrowserProtocol::Jk2_102.default_masters().map(str::to_owned),
            BrowserProtocol::Jk2_102,
        );
        let jk2_104 = BrowserUiState::new_for_protocol(
            dir, BrowserProtocol::Jk2_104.default_masters().map(str::to_owned),
            BrowserProtocol::Jk2_104,
        );
        assert_eq!(jka.protocol, BrowserProtocol::Jka);
        assert_eq!(jk2.protocol, BrowserProtocol::Jk2_102);
        assert_eq!(jk2_104.protocol, BrowserProtocol::Jk2_104);
        assert!(jka.mod_filter.contains("MBII"));
        assert!(jk2.mod_filter.is_empty());
        assert_ne!(jka.favorites_path, jk2.favorites_path);
        assert_ne!(jk2.history_path, jk2_104.history_path);
        assert_ne!(jka.master_servers, jk2.master_servers);
        let address: SocketAddr = "127.0.0.1:28070".parse().unwrap();
        jk2.handle_event(BrowserEvent::Server {
            source: ServerSource::Internet,
            server: ServerEntry { protocol: 15, ..ServerEntry::placeholder(address) },
        });
        assert_eq!(jk2.servers_for_current_source().len(), 1);
        assert!(jka.servers_for_current_source().is_empty());
        assert!(jk2_104.servers_for_current_source().is_empty());
    }

    #[test]
    fn mod_filter_negative_and_positive_terms() {
        let exclusions = ModFilters::parse(" -MBII, -MBIIOpenBeta ");
        assert!(exclusions.matches("base"));
        assert!(exclusions.matches("jaPRO"));
        assert!(!exclusions.matches("mbii"));
        assert!(!exclusions.matches("MBIIOPENBETA"));
        let mixed = ModFilters::parse("base,JAPro,-jaPRO");
        assert!(mixed.matches(""));
        assert!(!mixed.matches("japro"));
        assert!(!mixed.matches("japlus"));
        assert!(ModFilters::parse("-").matches("base"));
    }

    #[test]
    fn bot_and_connecting_counts_use_status_when_available() {
        let mut browser = BrowserUiState::new(Path::new("."), DEFAULT_MASTER_CVARS.map(str::to_owned));
        let address = "127.0.0.1:29070".parse().unwrap();
        let mut server = ServerEntry::placeholder(address);
        server.clients = 4;
        server.humans = 4; // Server-advertised bot count is incorrect.
        browser.hide_bots = true;
        browser.hide_connecting = true;
        assert_eq!(browser.visible_client_count(&server), 4); // Fallback until status.
        browser.status_cache.insert(address, ServerStatus {
            address,
            fields: vec![("sv_hostname".into(), "Test".into())],
            players: vec![
                ServerPlayer { name: "human".into(), score: 0, ping: 44 },
                ServerPlayer { name: "bot".into(), score: 0, ping: 0 },
                ServerPlayer { name: "connecting".into(), score: 0, ping: 999 },
                ServerPlayer { name: "bot2".into(), score: 0, ping: 0 },
            ],
        });
        assert_eq!(browser.player_breakdown(&server), (4, 2, 2, 1));
        assert_eq!(browser.visible_client_count(&server), 1);
        browser.hide_bots = false;
        assert_eq!(browser.visible_client_count(&server), 3);
        browser.hide_connecting = false;
        assert_eq!(browser.visible_client_count(&server), 4);
        browser.hide_bots = true;
        assert_eq!(browser.visible_client_count(&server), 2);
    }

    #[test]
    fn filter_defaults_and_persistence() {
        assert_eq!(DEFAULT_SERVER_FILTERS.split('|').nth(2), Some(DEFAULT_MOD_FILTER));
        let mut browser = BrowserUiState::new(Path::new("."), DEFAULT_MASTER_CVARS.map(str::to_owned));
        assert_eq!(browser.mod_filter, DEFAULT_MOD_FILTER);
        assert!(browser.hide_bots);
        assert!(browser.hide_connecting);
        assert!(!browser.hide_empty);
        assert!(!browser.hide_full);
        assert_eq!(browser.filters_string(), DEFAULT_SERVER_FILTERS);
        browser.apply_filters_string("4|120|japro,-MBII");
        assert!(browser.hide_bots);
        assert!(!browser.hide_empty);
        assert!(!browser.hide_connecting);
        assert_eq!(browser.filters_string(), "4|120|japro,-MBII|0");
        browser.apply_filters_string("");
        assert_eq!(browser.filters_string(), DEFAULT_SERVER_FILTERS);
    }

    #[test]
    fn gametype_filter_matches_and_persists() {
        let mut browser = BrowserUiState::new(Path::new("."), DEFAULT_MASTER_CVARS.map(str::to_owned));
        for gametype in -5..=15 {
            assert!(browser.matches_gametype(gametype));
        }
        browser.gametype_filter = gametype_filter_bit(3) | gametype_filter_bit(8);
        assert!(browser.matches_gametype(3));
        assert!(browser.matches_gametype(8));
        assert!(!browser.matches_gametype(0));
        assert!(!browser.matches_gametype(-1));
        assert_eq!(browser.gametype_filter_label(), "2 gametypes");
        let saved = browser.filters_string();
        browser.gametype_filter = 0;
        browser.apply_filters_string(&saved);
        assert_eq!(browser.gametype_filter, gametype_filter_bit(3) | gametype_filter_bit(8));
        browser.gametype_filter = gametype_filter_bit(8);
        assert_eq!(browser.gametype_filter_label(), "CTF");
        // Unknown/custom modes all share a single selectable bucket.
        browser.gametype_filter = gametype_filter_bit(-1);
        assert!(browser.matches_gametype(99));
        assert!(browser.matches_gametype(-1));
        assert!(!browser.matches_gametype(3));
        // Legacy saved values have no fourth field: treat as no gametype filter.
        browser.apply_filters_string("9|0|-MBII,-MBIIOpenBeta");
        assert_eq!(browser.gametype_filter, 0);
        assert_eq!(browser.filters_string(), DEFAULT_SERVER_FILTERS);
        // Reject unknown bit positions and corrupted values.
        browser.apply_filters_string("9|0|base|65535");
        assert_eq!(browser.gametype_filter, GAMETYPE_FILTER_MASK);
        browser.apply_filters_string("9|0|base|garbage");
        assert_eq!(browser.gametype_filter, 0);
    }

    #[test]
    fn column_filters_persist_and_old_default_migrates() {
        let mut browser = BrowserUiState::new(Path::new("."), DEFAULT_MASTER_CVARS.map(str::to_owned));
        browser.search = "duel".into();
        browser.map_filter = "ffa".into();
        browser.player_search = "kyle".into();
        browser.player_exact_match = true;
        browser.visit_filter = 2;
        let saved = browser.filters_string();
        browser.apply_filters_string(DEFAULT_SERVER_FILTERS);
        assert!(browser.search.is_empty());
        assert!(browser.map_filter.is_empty());
        assert!(browser.player_search.is_empty());
        assert_eq!(browser.visit_filter, 0);
        browser.apply_filters_string(&saved);
        assert_eq!(browser.search, "duel");
        assert_eq!(browser.map_filter, "ffa");
        assert_eq!(browser.player_search, "kyle");
        assert!(browser.player_exact_match);
        assert_eq!(browser.visit_filter, 2);
        browser.apply_filters_string("9|0|-MBII,-MBIIOpenBeta|0");
        assert_eq!(browser.filters_string(), DEFAULT_SERVER_FILTERS);
        assert!(!browser.hide_empty);
        assert!(browser.hide_bots);
        assert!(browser.hide_connecting);
    }

    #[test]
    fn parses_classic_master_packet() {
        let mut packet = b"\xff\xff\xff\xffgetserversResponse".to_vec();
        packet.extend_from_slice(&[b'\\', 127, 0, 0, 1, 0x71, 0x8e]); // 29070
        packet.extend_from_slice(&[b'\\', 10, 20, 30, 40, 0x71, 0x8f]);
        packet.extend_from_slice(b"\\EOT");
        assert_eq!(
            parse_getservers_response(&packet),
            vec![
                "127.0.0.1:29070".parse().unwrap(),
                "10.20.30.40:29071".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn parses_status_player_lines() {
        let player = parse_status_player("12 54 \"^2Kyle Katarn\"").unwrap();
        assert_eq!(player.score, 12);
        assert_eq!(player.ping, 54);
        assert_eq!(player.name, "^2Kyle Katarn");
    }

    #[test]
    fn parses_status_player_extra_whitespace() {
        let player = parse_status_player("  12  \t 0   \"Bot Name\" ").unwrap();
        assert_eq!(player.score, 12);
        assert_eq!(player.ping, 0);
        assert_eq!(player.name, "Bot Name");
    }

    #[test]
    fn formats_full_getstatus_dump() {
        let status = ServerStatus {
            address: "127.0.0.1:29070".parse().unwrap(),
            fields: vec![
                ("sv_hostname".to_owned(), "^2Test Server".to_owned()),
                ("mapname".to_owned(), "mp/ffa3".to_owned()),
            ],
            players: vec![ServerPlayer {
                name: "^3Kyle Katarn".to_owned(),
                score: 12,
                ping: 54,
            }],
        };
        let dump = format_server_status(&status);
        assert!(dump.contains("getstatus 127.0.0.1:29070"));
        assert!(dump.contains("sv_hostname"));
        assert!(dump.contains("^2Test Server"));
        assert!(dump.contains("12"));
        assert!(dump.contains("54"));
        assert!(dump.contains("^3Kyle Katarn"));
    }

    #[test]
    fn formats_openjk_console_serverstatus_with_all_fields_and_players() {
        let status = ServerStatus {
            address: "127.0.0.1:29070".parse().unwrap(),
            fields: vec![
                ("sv_hostname".to_owned(), "^2Test Server".to_owned()),
                ("mapname".to_owned(), "mp/ffa3".to_owned()),
            ],
            players: vec![ServerPlayer {
                name: "^3Kyle Katarn".to_owned(),
                score: 12,
                ping: 54,
            }],
        };
        let output = format_openjk_server_status(&status);
        assert!(output.starts_with("Server (127.0.0.1:29070)\nServer settings:\n"));
        assert!(output.contains("sv_hostname             ^2Test Server\n"));
        assert!(output.contains("mapname                 mp/ffa3\n"));
        assert!(output.contains("\nPlayers:\nnum: score: ping: name:\n"));
        assert!(output.contains("0    12     54    \"^3Kyle Katarn\""));
    }

    #[test]
    fn history_parser_accepts_legacy_and_timestamped_rows() {
        let rows = parse_history_list(
            "127.0.0.1:29070\n10.20.30.40:29071\t1760000123456\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("127.0.0.1:29070".parse().unwrap(), None));
        assert_eq!(
            rows[1],
            ("10.20.30.40:29071".parse().unwrap(), Some(1_760_000_123_456))
        );
    }

}
