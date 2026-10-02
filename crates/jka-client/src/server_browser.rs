//! Asynchronous Jedi Academy server discovery.
//!
//! The wire protocol intentionally follows OpenJK/JKA compatibility:
//! * Internet: `getservers <protocol>` to a master server, then `getinfo` each address.
//! * LAN: broadcast `getinfo` across the stock JKA server-port range.
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
    time::{Duration, Instant},
};

use jka_protocol::{parse_info_response, PROTOCOL_VERSION};

pub const FIRST_SERVER_PORT: u16 = jka_protocol::DEFAULT_PORT;
pub const NUM_SERVER_PORTS: u16 = 4;
const MASTER_WAIT: Duration = Duration::from_millis(1200);
const INFO_WAIT: Duration = Duration::from_millis(2200);
const STATUS_WAIT: Duration = Duration::from_millis(900);
const MAX_MASTER_SERVERS: usize = 4096;
const MAX_PACKET: usize = 64 * 1024;
const MASTER_PORT: u16 = 29060;

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

#[derive(Debug, Clone)]
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
}

impl BrowserSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ping => "PING",
            Self::Players => "PLAYERS",
            Self::Name => "NAME",
            Self::Map => "MAP",
            Self::Gametype => "MODE",
        }
    }
}

pub struct BrowserUiState {
    pub source: ServerSource,
    pub servers: HashMap<SocketAddr, ServerEntry>,
    pub source_addresses: HashMap<ServerSource, HashSet<SocketAddr>>,
    pub selected: Option<SocketAddr>,
    pub search: String,
    pub hide_empty: bool,
    pub hide_full: bool,
    pub hide_bots: bool,
    pub mod_filter: String,
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
    /// Cached getstatus replies used by both the details pane and player-name
    /// filtering. Entries are refreshed opportunistically while a player
    /// search is active.
    pub status_cache: HashMap<SocketAddr, ServerStatus>,
    pub status_pending: HashSet<SocketAddr>,
    pub player_search: String,
    pub player_exact_match: bool,
    pub player_search_last_query: Option<Instant>,
    pub favorites: Vec<SocketAddr>,
    pub history: Vec<SocketAddr>,
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
}

impl BrowserUiState {
    pub fn new(
        settings_dir: &Path,
        master_servers: [String; MAX_MASTER_SLOTS],
    ) -> Self {
        let favorites_path = settings_dir.join("server-favorites.txt");
        let history_path = settings_dir.join("server-history.txt");
        let mut favorites = load_address_list(&favorites_path);
        favorites.sort_unstable();
        let mut history = load_address_list(&history_path);
        // Keep the most recent history entries bounded.  The file is intentionally
        // human-editable and stores plain host:port lines.
        if history.len() > 64 {
            history = history.split_off(history.len() - 64);
        }
        let mut master_drafts = master_servers.clone();
        for (slot, default) in DEFAULT_MASTER_CVARS.iter().enumerate() {
            if master_drafts[slot].trim().is_empty() && !default.is_empty() {
                master_drafts[slot] = (*default).to_owned();
            }
        }
        Self {
            source: ServerSource::Internet,
            servers: HashMap::new(),
            source_addresses: HashMap::new(),
            selected: None,
            search: String::new(),
            hide_empty: false,
            hide_full: false,
            hide_bots: false,
            mod_filter: String::new(),
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
            direct_connect: String::new(),
            internet_requested_once: false,
            autojoin: None,
            autojoin_last_query: None,
            master_servers,
            master_drafts,
            favorites_path,
            history_path,
        }
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
                if matches!(source, ServerSource::Internet | ServerSource::Lan) {
                    if let Some(seen) = self.refresh_seen.remove(&source) {
                        if let Some(addresses) = self.source_addresses.get_mut(&source) {
                            addresses.retain(|address| seen.contains(address));
                        }
                    }
                } else {
                    self.refresh_seen.remove(&source);
                }
                let text = if discovered == 0 {
                    format!("{}: no servers found", source.label())
                } else {
                    format!("{}: {responded}/{discovered} servers responded", source.label())
                };
                self.source_status.insert(source, text.clone());
                self.status_text = text;
            }
            BrowserEvent::Status(status) => {
                self.status_pending.remove(&status.address);
                if self.selected == Some(status.address) {
                    self.details = Some(status.clone());
                }
                self.status_cache.insert(status.address, status);
            }
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
                    .unwrap_or_else(|| ServerEntry::placeholder(address))
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
        self.history.retain(|item| *item != address);
        self.history.push(address);
        if self.history.len() > 64 {
            let excess = self.history.len() - 64;
            self.history.drain(..excess);
        }
        save_address_list(&self.history_path, &self.history)
    }
}

#[derive(Debug)]
pub enum BrowserCommand {
    RefreshInternet { masters: Vec<String> },
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
    StatusBatchFinished {
        requested: Vec<SocketAddr>,
    },
    Error {
        source: Option<ServerSource>,
        message: String,
    },
}

pub fn spawn() -> Result<(Sender<BrowserCommand>, Receiver<BrowserEvent>), String> {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::Builder::new()
        .name("server-browser".into())
        .spawn(move || worker(command_rx, event_tx))
        .map_err(|error| format!("Could not start server browser worker: {error}"))?;
    Ok((command_tx, event_rx))
}

fn worker(commands: Receiver<BrowserCommand>, events: Sender<BrowserEvent>) {
    while let Ok(command) = commands.recv() {
        let result = match command {
            BrowserCommand::RefreshInternet { masters } => refresh_internet(&events, masters),
            BrowserCommand::RefreshLan => refresh_lan(&events),
            BrowserCommand::RefreshAddresses { source, addresses } => {
                refresh_addresses(&events, source, addresses)
            }
            BrowserCommand::RefreshServer { source, address, quiet } => {
                refresh_server(&events, source, address, quiet)
            }
            BrowserCommand::QueryStatus(address) => query_status(&events, address),
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

type WorkerResult = Result<(), (Option<ServerSource>, String)>;

fn refresh_internet(events: &Sender<BrowserEvent>, masters: Vec<String>) -> WorkerResult {
    let source = ServerSource::Internet;
    events
        .send(BrowserEvent::RefreshStarted(source))
        .map_err(|_| (Some(source), "browser UI disconnected".to_owned()))?;
    let addresses = discover_master_servers(&masters).map_err(|message| (Some(source), message))?;
    query_info_batch(events, source, addresses)
}

fn refresh_lan(events: &Sender<BrowserEvent>) -> WorkerResult {
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
                FIRST_SERVER_PORT + offset,
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
                if info.protocol != PROTOCOL_VERSION {
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
    events: &Sender<BrowserEvent>,
    source: ServerSource,
    addresses: Vec<SocketAddr>,
) -> WorkerResult {
    events
        .send(BrowserEvent::RefreshStarted(source))
        .map_err(|_| (Some(source), "browser UI disconnected".to_owned()))?;
    query_info_batch(events, source, addresses)
}

fn refresh_server(
    events: &Sender<BrowserEvent>,
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
                if info.protocol != PROTOCOL_VERSION {
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

fn discover_master_servers(masters: &[String]) -> Result<Vec<SocketAddr>, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| format!("master browser socket: {e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| format!("master browser timeout: {e}"))?;

    // Ask for the complete candidate set; empty/full filtering is instant and local in the UI.
    let mut request = b"\xff\xff\xff\xffgetservers ".to_vec();
    request.extend_from_slice(PROTOCOL_VERSION.to_string().as_bytes());
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
            (master, MASTER_PORT).to_socket_addrs()
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
        return Err("No enabled Jedi Academy master server could be resolved".to_owned());
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
    events: &Sender<BrowserEvent>,
    source: ServerSource,
    mut addresses: Vec<SocketAddr>,
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
                if info.protocol != PROTOCOL_VERSION {
                    continue;
                }
                responded_addresses.insert(from);
                let server = server_from_info(from, &info, sent.elapsed());
                responded += 1;
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

    let _ = events.send(BrowserEvent::RefreshFinished {
        source,
        discovered,
        responded,
    });
    Ok(())
}

fn query_status(events: &Sender<BrowserEvent>, address: SocketAddr) -> WorkerResult {
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
                let status = parse_status_response(address, &buf[..len])
                    .ok_or_else(|| (None, format!("Malformed getstatus response from {address}")))?;
                let _ = events.send(BrowserEvent::Status(status));
                return Ok(());
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err((None, format!("getstatus receive: {error}"))),
        }
    }
    Err((None, format!("Server status timed out: {address}")))
}

fn query_status_batch(events: &Sender<BrowserEvent>, mut addresses: Vec<SocketAddr>) -> WorkerResult {
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
                if !wanted.contains(&from) || !responded.insert(from) {
                    continue;
                }
                if let Some(status) = parse_status_response(from, &buf[..len]) {
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
    let mut parts = line.trim().splitn(3, ' ');
    let score = parts.next()?.parse().ok()?;
    let ping = parts.next()?.parse().ok()?;
    let name = parts.next()?.trim().trim_matches('"').to_owned();
    Some(ServerPlayer { name, score, ping })
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

#[cfg(test)]
mod tests {
    use super::*;

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

}
