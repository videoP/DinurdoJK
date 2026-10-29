pub mod cvars;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Command,
    Cvar,
    /// A game-module command (OpenJK cgame `gcmds[]`): registered only while
    /// connected, so it completes, and is forwarded to the server.
    ServerCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatchScope {
    None,
    VidRestart,
    MapLoadOrVidRestart,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub kind: EntryKind,
    pub default: &'static str,
    pub range: &'static str,
    pub description: &'static str,
    pub latch: LatchScope,
    pub usage: &'static str,
}

#[cfg(test)]
const fn cvar(
    name: &'static str,
    default: &'static str,
    range: &'static str,
    description: &'static str,
) -> Entry {
    Entry {
        name,
        kind: EntryKind::Cvar,
        default,
        range,
        description,
        latch: LatchScope::None,
        usage: "",
    }
}



/// Return the cvar's declared discrete integer choices. Pipe-delimited ranges
/// are explicit choice lists (`0|1|2`, `30|60|120|240`). A plain `0..1` is
/// also a two-choice boolean declaration so legacy `toggle <bool-cvar>` binds
/// remain useful. Wider numeric ranges are scalar domains, not option lists.
pub fn declared_integer_options(entry: &Entry) -> Option<Vec<String>> {
    let range = entry.range.trim();
    if range == "0..1" {
        return Some(vec!["0".to_owned(), "1".to_owned()]);
    }
    if !range.contains('|') {
        return None;
    }
    let mut values = Vec::new();
    for part in range.split('|') {
        let token = part.trim().split_whitespace().next().unwrap_or("");
        if token.parse::<i64>().is_err() {
            return None;
        }
        values.push(token.to_owned());
    }
    (values.len() >= 2).then_some(values)
}



const fn server_command(name: &'static str, description: &'static str) -> Entry {
    Entry {
        name,
        kind: EntryKind::ServerCommand,
        default: "",
        range: "",
        description,
        latch: LatchScope::None,
        usage: "",
    }
}

/// OpenJK cg_consolecmds.c `gcmds[]` (vanilla): interpreted by the server's
/// game module. CG_InitConsoleCommands registers them for completion only.
pub const SERVER_COMMANDS: &[Entry] = &[
    // TaystJK gcmds[] jaPRO controls. These are reliable server strings.
    server_command("amTele", "jaPRO: teleport to mark, player, or coordinates: amTele [x y z [yaw]]."),
    server_command("amTeleMark", "jaPRO: save the current teleport mark."),
    server_command("engage_fullforceduel", "jaPRO: challenge/accept a full-force duel."),
    server_command("engage_gunduel", "jaPRO: challenge/accept a gun duel."),
    server_command("throwflag", "jaPRO: throw the carried team flag when the server permits it."),
    server_command("addbot", "Add a bot: addbot <name> [skill] [team] [delay] [altname]."),
    server_command("callteamvote", "Call a team vote: callteamvote <leader> <client>."),
    server_command("callvote", "Call a vote: callvote <map|map_restart|g_gametype|kick|clientkick|g_doWarmup|timelimit|fraglimit|nextmap> [value]."),
    server_command("duelteam", "Choose a Power Duel team: duelteam <free|single|double>."),
    server_command("follow", "Spectate a player: follow <name|number>."),
    server_command("follownext", "Spectate the next player."),
    server_command("followprev", "Spectate the previous player."),
    server_command("forcechanged", "Apply a new Force power configuration."),
    server_command("give", "Cheat: give <all|health|armor|ammo|weapons|force|item name>."),
    server_command("god", "Cheat: toggle invulnerability."),
    server_command("kill", "Suicide."),
    server_command("levelshot", "Cheat: move to the intermission point for a levelshot."),
    server_command("loaddefered", "Load deferred player models."),
    server_command("noclip", "Cheat: toggle noclip movement."),
    server_command("notarget", "Cheat: toggle NPC notarget."),
    server_command("NPC", "Cheat: spawn or control NPCs: NPC spawn <type>."),
    server_command("say", "Send a global chat message: say <message>."),
    server_command("say_team", "Send a team chat message: say_team <message>."),
    server_command("setviewpos", "Cheat: teleport: setviewpos <x> <y> <z> <yaw>."),
    server_command("siegeclass", "Choose a Siege class: siegeclass <class name>."),
    server_command("stats", "Show team statistics."),
    server_command("team", "Show or change team: team [free|f|red|r|blue|b|spectator|spectate|spec|s|follow1|follow2|scoreboard]."),
    server_command("teamtask", "Choose a team task."),
    server_command("teamvote", "Vote in a team vote: teamvote <yes|no>."),
    server_command("t_use", "Cheat: fire every entity with a targetname: t_use <targetname>."),
    server_command("tell", "Private message: tell <client number|name> <message>."),
    server_command("voice_cmd", "Send a voice command."),
    server_command("vgs_cmd", "Send a jaPRO VGS canned voice command."),
    server_command("vote", "Vote in a vote: vote <yes|no>."),
    server_command("where", "Print your current origin."),
    server_command("zoom", "Toggle binocular zoom."),
];

/// jaPRO server game commands (`commands[]` in codemp/game/g_cmds.c) that stock JKA
/// servers do not have. They complete only while connected to a jaPRO server.
pub const JAPRO_COMMANDS: &[Entry] = &[
    server_command("amBan", "jaPRO: Admin: ban a player: amBan <player> [duration]."),
    server_command("amBeg", "jaPRO: Emote: beg."),
    server_command("amBeg2", "jaPRO: Emote: beg (variant)."),
    server_command("amBernie", "jaPRO: Emote: sit with mittens."),
    server_command("amBreakdance", "jaPRO: Emote: breakdance."),
    server_command("amBreakdance2", "jaPRO: Emote: breakdance 2."),
    server_command("amBreakdance3", "jaPRO: Emote: breakdance 3."),
    server_command("amBreakdance4", "jaPRO: Emote: breakdance 4."),
    server_command("amCheer", "jaPRO: Emote: cheer."),
    server_command("amCower", "jaPRO: Emote: cower."),
    server_command("amDance", "jaPRO: Emote: dance."),
    server_command("amFlip", "jaPRO: Emote: saber flip."),
    server_command("amForceTeam", "jaPRO: Admin: move a player to a team: amForceTeam <player> <team>."),
    server_command("amFreeze", "jaPRO: Admin: freeze or unfreeze a player: amFreeze <player>."),
    server_command("amGrantAdmin", "jaPRO: Admin: grant admin rights: amGrantAdmin <player> <level>."),
    server_command("amHug", "jaPRO: Emote: hug."),
    server_command("amInfo", "jaPRO: Show the admin commands you may use."),
    server_command("amKick", "jaPRO: Admin: kick a player: amKick <player> [reason]."),
    server_command("amKillVote", "jaPRO: Admin: cancel the vote in progress."),
    server_command("amListMaps", "jaPRO: List the maps on the server: amListMaps [filter]."),
    server_command("amLockTeam", "jaPRO: Admin: lock or unlock a team: amLockTeam <team>."),
    server_command("amLogin", "Log in as server/mod admin: amLogin <password>."),
    server_command("amLogout", "jaPRO: Log out of your admin session."),
    server_command("amLookup", "jaPRO: Admin: look up a player's account: amLookup <player>."),
    server_command("amMap", "jaPRO: Admin: change map immediately: amMap <map>."),
    server_command("amMOTD", "jaPRO: Show the message of the day."),
    server_command("amNoisy", "jaPRO: Emote: noisy."),
    server_command("amPoint", "jaPRO: Emote: point."),
    server_command("amPsay", "jaPRO: Admin: private message: amPsay <player> <message>."),
    server_command("amRage", "jaPRO: Emote: rage."),
    server_command("amRename", "jaPRO: Admin: rename a player: amRename <player> <name>."),
    server_command("amSay", "jaPRO: Say something to the admins."),
    server_command("amSignal", "jaPRO: Emote: signal."),
    server_command("amSignal2", "jaPRO: Emote: signal 2."),
    server_command("amSignal3", "jaPRO: Emote: signal 3."),
    server_command("amSignal4", "jaPRO: Emote: signal 4."),
    server_command("amSit", "jaPRO: Emote: sit."),
    server_command("amSit2", "jaPRO: Emote: sit 2."),
    server_command("amSit3", "jaPRO: Emote: sit 3."),
    server_command("amSit4", "jaPRO: Emote: sit 4."),
    server_command("amSit5", "jaPRO: Emote: sit 5."),
    server_command("amSlap", "jaPRO: Emote: slap."),
    server_command("amSleep", "jaPRO: Emote: sleep."),
    server_command("amSmack", "jaPRO: Emote: smack."),
    server_command("amSurrender", "jaPRO: Emote: surrender."),
    server_command("amTaunt", "jaPRO: Emote: taunt."),
    server_command("amTaunt2", "jaPRO: Emote: taunt 2."),
    server_command("amTele", "jaPRO: amTele [player] or amTele <x> <y> <z> [yaw (admin)]."),
    server_command("amTeleMark", "jaPRO: Save your position as the teleport mark."),
    server_command("amVictory", "jaPRO: Emote: victory pose."),
    server_command("amVstr", "jaPRO: Admin: run a server vstr: amVstr <name>."),
    server_command("blink", "jaPRO: Blink forward a short distance."),
    server_command("changePassword", "jaPRO: Change your account password: changePassword <old> <new>."),
    server_command("clanAdmin", "jaPRO: Clan: administrate your clan."),
    server_command("clanCreate", "jaPRO: Clan: create a clan: clanCreate <name>."),
    server_command("clanInfo", "jaPRO: Clan: show your clan's information."),
    server_command("clanInvite", "jaPRO: Clan: invite a player: clanInvite <player>."),
    server_command("clanJoin", "jaPRO: Clan: join a clan you were invited to."),
    server_command("clanLeave", "jaPRO: Clan: leave your clan."),
    server_command("clanList", "jaPRO: Clan: list the clan members."),
    server_command("clanPass", "jaPRO: Clan: set the clan password."),
    server_command("clanSay", "jaPRO: Clan: message your clan: clanSay <message>."),
    server_command("clanWhois", "jaPRO: Clan: who is in a clan: clanWhois <name>."),
    server_command("coop", "jaPRO: Co-op race: coop <player>."),
    server_command("crouchJump", "jaPRO: Toggle crouch jumping."),
    server_command("flagRecord", "jaPRO: Invalidate your current race record."),
    server_command("gc", "jaPRO: Send a game command to a player: gc <player> <command>."),
    server_command("giveOther", "jaPRO: Cheat: give items to another player."),
    server_command("haste", "jaPRO: Toggle haste."),
    server_command("hide", "jaPRO: Toggle hiding yourself (spectators and racers)."),
    server_command("ignore", "jaPRO: Ignore a player's chat: ignore <player>."),
    server_command("jetpack", "jaPRO: Toggle the jetpack."),
    server_command("jump", "jaPRO: Change your jump level: jump <1-3>."),
    server_command("killOther", "jaPRO: Cheat: kill another player."),
    server_command("launch", "jaPRO: Launch yourself (race practice)."),
    server_command("login", "Log in to a server account: login <name> <password>."),
    server_command("logout", "jaPRO: Log out of your account."),
    server_command("mapEnts", "jaPRO: Cheat: list or spawn map entities."),
    server_command("master", "jaPRO: Add a clan master."),
    server_command("masterList", "jaPRO: List the clan masters."),
    server_command("modversion", "jaPRO: Show the server mod version."),
    server_command("move", "jaPRO: Choose your movement style: move <jka|qw|cpm|q3|pjk|wsw|rjq3|rjcpm|swoop|jetpack|sp|...>."),
    server_command("nearby", "jaPRO: List nearby players and their distance."),
    server_command("nudge", "jaPRO: Cheat: nudge your position."),
    server_command("pack", "jaPRO: Tribes: choose your pack."),
    server_command("practice", "jaPRO: Toggle practice mode in race mode."),
    server_command("printStats", "jaPRO: Print your account and race statistics."),
    server_command("race", "jaPRO: Toggle race mode."),
    server_command("rCompare", "jaPRO: Race: compare your records with another player: rCompare <player>."),
    server_command("register", "jaPRO: Register an account: register <name> <password>."),
    server_command("rFind", "jaPRO: Race: find a course by name: rFind <text>."),
    server_command("rHardest", "jaPRO: Race: list the hardest courses."),
    server_command("rLatest", "jaPRO: Race: list the latest records."),
    server_command("rocketChange", "jaPRO: Toggle backwards rocket launching."),
    server_command("rPopular", "jaPRO: Race: list the most popular courses."),
    server_command("rRank", "jaPRO: Race: show your race rank."),
    server_command("rTop", "jaPRO: Race: show the top times for this course."),
    server_command("rWarp", "jaPRO: Race: warp to a course: rWarp <course>."),
    server_command("rWorst", "jaPRO: Race: list the courses you have not beaten."),
    server_command("saber", "jaPRO: Change your saber style: saber <name>."),
    server_command("say_team_mod", "jaPRO: Team message visible to moderators."),
    server_command("score", "jaPRO: Show the scoreboard text."),
    server_command("serverConfig", "jaPRO: Show the server's configuration."),
    server_command("showNet", "jaPRO: Show network information for a player."),
    server_command("spot", "jaPRO: Save your current position as a spot."),
    server_command("thedestroyer", "jaPRO: Cheat: summon the destroyer."),
    server_command("throwNade", "jaPRO: Throw a grenade."),
    server_command("top", "jaPRO: Show the duel ranking table."),
    server_command("trace", "jaPRO: Trace what you are looking at."),
    server_command("warp", "jaPRO: Warp to a saved course spot: warp <name>."),
    server_command("warpList", "jaPRO: List the available warps."),
    server_command("whois", "jaPRO: Look up a player's account and aliases: whois <player>."),
    server_command("ysal", "jaPRO: Toggle the ysalamiri effect."),
];

static JAPRO_SERVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the console whether the current server is jaPRO (enables its commands).
pub fn set_japro_server(active: bool) {
    JAPRO_SERVER.store(active, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `name` is a vanilla server game command (forwarded while connected).
pub fn is_server_command(name: &str) -> bool {
    SERVER_COMMANDS.iter().any(|entry| entry.name.eq_ignore_ascii_case(name))
}

fn local_entries() -> &'static [Entry] {
    static ENTRIES: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = Vec::new();
        entries.extend(crate::app::cvar_registry().iter().map(|def| def.entry));
        entries.extend(crate::app::command_registry().iter()
            .filter(|def| def.scope == crate::app::CommandScope::Always)
            .flat_map(|def| def.entries()));
        let mut names = std::collections::HashSet::new();
        for entry in &entries { assert!(names.insert(entry.name.to_ascii_lowercase()), "duplicate console name: {}", entry.name); }
        entries
    })
}
fn scoped_commands() -> &'static [Entry] {
    static ENTRIES: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| crate::app::command_registry().iter()
        .filter(|def| def.scope != crate::app::CommandScope::Always)
        .flat_map(|def| def.entries()).collect())
}

pub fn cvars() -> impl Iterator<Item = &'static Entry> {
    local_entries().iter().filter(|entry| entry.kind == EntryKind::Cvar)
}

/// Every name the console knows. Server commands exist only while connected,
/// matching OpenJK's cgame registration lifetime; a server command that
/// shares a name with a local command is listed once (the local entry).
fn registry(connected: bool) -> impl Iterator<Item = &'static Entry> {
    let server: &'static [Entry] = if connected { SERVER_COMMANDS } else { &[] };
    let japro: &'static [Entry] =
        if connected && JAPRO_SERVER.load(std::sync::atomic::Ordering::Relaxed) { JAPRO_COMMANDS } else { &[] };
    local_entries().iter()
        .chain(server.iter().filter(|entry| !local_entries().iter().any(|local| local.name.eq_ignore_ascii_case(entry.name))).map(|entry| scoped_commands().iter()
            .find(|local| local.name.eq_ignore_ascii_case(entry.name)).unwrap_or(entry)))
        .chain(scoped_commands().iter().filter(move |entry| connected &&
            !SERVER_COMMANDS.iter().any(|known| known.name.eq_ignore_ascii_case(entry.name)) &&
            crate::app::command_registry().find(entry.name).is_some_and(|def|
                def.scope == crate::app::CommandScope::Connected || JAPRO_SERVER.load(std::sync::atomic::Ordering::Relaxed))))
        .chain(japro.iter().filter(|entry| {
            !local_entries().iter().chain(scoped_commands()).chain(SERVER_COMMANDS)
                .any(|known| known.name.eq_ignore_ascii_case(entry.name))
        }))
}

/// Local commands and cvars only (used for cvar get/set).
pub fn find(name: &str) -> Option<&'static Entry> {
    local_entries()
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
}

/// Execution ignores optional leading console `/` or `\`. Completion must
/// normalize those too, so `/remapShader` appears before Enter is pressed.
fn completion_query(name: &str) -> &str {
    name.trim_start_matches(['/', '\\'])
}

/// Exact lookup for completion, including server commands while connected.
pub fn find_command(name: &str, connected: bool) -> Option<&'static Entry> {
    let name = completion_query(name);
    registry(connected).find(|entry| entry.name.eq_ignore_ascii_case(name))
}

pub fn prefix(prefix: &str, connected: bool) -> impl Iterator<Item = &'static Entry> {
    let prefix = completion_query(prefix).to_ascii_lowercase();
    registry(connected).filter(move |entry| entry.name.to_ascii_lowercase().starts_with(&prefix))
}

/// Registered executable commands (including cgame/server commands only while
/// connected), excluding cvars just like OpenJK's `cmdlist`.
pub fn commands(connected: bool) -> impl Iterator<Item = &'static Entry> {
    registry(connected).filter(|entry| entry.kind != EntryKind::Cvar)
}

/// Small case-insensitive `Com_Filter` subset used by `cmdlist`: `*` matches
/// any run and `?` matches one character. Ordinary patterns are exact matches.
pub fn filter_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase().into_bytes();
    let text = text.to_ascii_lowercase().into_bytes();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star = None;
    let mut star_text = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            star_text = t;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            star_text += 1;
            t = star_text;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// How a registered name matched the live console query. Lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    Exact,
    Prefix,
    Substring,
    /// Query letters appear in order (`cgfov` finds `cg_fov`).
    Subsequence,
    /// Only the description mentions the query.
    Description,
}

/// One row of the live console filter.
#[derive(Debug, Clone, Copy)]
pub struct Suggestion {
    pub entry: &'static Entry,
    pub tier: MatchTier,
    /// Bit `i` set = byte `i` of `entry.name` matched the query (first 64 bytes).
    pub mask: u64,
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let (hay, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || hay.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

fn name_mask(start: usize, len: usize) -> u64 {
    (start..start + len).filter(|bit| *bit < 64).fold(0, |mask, bit| mask | 1u64 << bit)
}

/// Shared name matcher for registered commands/cvars and argument completion.
/// The mask highlights matched bytes in the console popup (up to 64 bytes).
/// Argument completion intentionally does not match command descriptions.
pub(crate) fn rank_name_match(name: &str, query: &str) -> Option<(MatchTier, u64)> {
    if query.is_empty() {
        return None;
    }
    let name = name.as_bytes();
    let q = query.as_bytes();
    if name.eq_ignore_ascii_case(q) {
        return Some((MatchTier::Exact, name_mask(0, name.len())));
    }
    if name.len() >= q.len() && name[..q.len()].eq_ignore_ascii_case(q) {
        return Some((MatchTier::Prefix, name_mask(0, q.len())));
    }
    if let Some(start) = name.windows(q.len()).position(|w| w.eq_ignore_ascii_case(q)) {
        return Some((MatchTier::Substring, name_mask(start, q.len())));
    }
    let mut mask = 0u64;
    let mut wanted = q.iter().map(u8::to_ascii_lowercase);
    let mut next = wanted.next();
    for (index, byte) in name.iter().enumerate() {
        if next == Some(byte.to_ascii_lowercase()) {
            mask |= name_mask(index, 1);
            next = wanted.next();
        }
    }
    (next.is_none() && q.len() >= 2).then_some((MatchTier::Subsequence, mask))
}

fn classify(entry: &'static Entry, query: &str) -> Option<Suggestion> {
    if let Some((tier, mask)) = rank_name_match(entry.name, query) {
        return Some(Suggestion { entry, tier, mask });
    }
    (query.len() >= 3 && contains_ignore_case(entry.description, query))
        .then_some(Suggestion { entry, tier: MatchTier::Description, mask: 0 })
}

/// Live-filter the registry for the console popup: best matches first (exact,
/// prefix, substring, in-order letters, then description hits), shorter names
/// ahead of longer ones inside a tier. Returns at most `limit` rows plus the
/// total number of matches.
pub fn suggest(query: &str, connected: bool, limit: usize) -> (Vec<Suggestion>, usize) {
    let query = completion_query(query.trim());
    if query.is_empty() {
        return (Vec::new(), 0);
    }
    let mut hits: Vec<Suggestion> = registry(connected).filter_map(|entry| classify(entry, query)).collect();
    let total = hits.len();
    hits.sort_by(|a, b| {
        (a.tier, a.entry.name.len(), a.entry.name.to_ascii_lowercase())
            .cmp(&(b.tier, b.entry.name.len(), b.entry.name.to_ascii_lowercase()))
    });
    hits.truncate(limit);
    (hits, total)
}

/// Return the only registered command/cvar matching `prefix`, if exactly one exists.
/// Exact names should normally be checked with [`find_command`] first so an exact
/// command wins even when it is also a prefix of another registered name.
pub fn unique_prefix(prefix_text: &str, connected: bool) -> Option<&'static Entry> {
    let mut matches = prefix(prefix_text, connected);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Longest common command/cvar prefix for every registered name matching `prefix_text`.
/// Command names are ASCII, so byte indexing is intentional here. The spelling/case of
/// the first registered entry is preserved in the returned completion.
pub fn common_prefix(prefix_text: &str, connected: bool) -> Option<String> {
    let matches: Vec<_> = prefix(prefix_text, connected).collect();
    let first = matches.first()?.name;
    let first_lower = first.to_ascii_lowercase();
    let mut common_len = first_lower.len();

    for entry in matches.iter().skip(1) {
        let other = entry.name.to_ascii_lowercase();
        common_len = first_lower
            .as_bytes()
            .iter()
            .zip(other.as_bytes())
            .take(common_len)
            .take_while(|(a, b)| a == b)
            .count();
    }

    Some(first[..common_len].to_owned())
}

/// Result of completing a word against the connected players' names.
#[derive(Debug, PartialEq, Eq)]
pub enum PlayerNameCompletion {
    None,
    /// The full name with its colour codes, as in the player's `n` userinfo.
    One(String),
    /// Multiple matches, already ordered from closest to loosest match.
    Many(Vec<String>),
}

/// JAPP `CG_ChatboxTabComplete`: `word` is matched against connected player
/// names with colour codes stripped and case ignored. Ambiguous hits are ranked
/// so repeated Tab can cycle in a useful order:
/// exact -> prefix -> word/separator boundary -> interior substring.
pub fn complete_player_name(word: &str, names: &[String]) -> PlayerNameCompletion {
    let needle = crate::logging::strip_jka_colors(word).to_lowercase();
    if needle.is_empty() {
        return PlayerNameCompletion::None;
    }

    let mut hits: Vec<(usize, usize, usize, usize, String)> = names
        .iter()
        .enumerate()
        .filter_map(|(slot_order, name)| {
            let clean = crate::logging::strip_jka_colors(name).to_lowercase();
            let position = clean.find(&needle)?;
            let tier = if clean == needle {
                0
            } else if position == 0 {
                1
            } else {
                let at_boundary = clean[..position]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| !ch.is_alphanumeric());
                if at_boundary { 2 } else { 3 }
            };
            // Within a tier, earlier occurrences and shorter names are closer.
            // Preserve server/client slot order as the final deterministic tie-break.
            Some((tier, position, clean.chars().count(), slot_order, name.clone()))
        })
        .collect();
    hits.sort_by_key(|(tier, position, clean_len, slot_order, _)| {
        (*tier, *position, *clean_len, *slot_order)
    });
    let mut hits: Vec<String> = hits.into_iter().map(|(_, _, _, _, name)| name).collect();

    match hits.len() {
        0 => PlayerNameCompletion::None,
        1 => PlayerNameCompletion::One(hits.remove(0)),
        _ => PlayerNameCompletion::Many(hits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_names_match_anywhere_ignoring_colour_and_case() {
        let names = vec!["^1Red^7Jawa".to_owned(), "Padawan".to_owned(), "".to_owned()];
        assert_eq!(complete_player_name("^2jaw", &names), PlayerNameCompletion::One("^1Red^7Jawa".to_owned()));
        assert_eq!(complete_player_name("zzz", &names), PlayerNameCompletion::None);
        assert_eq!(complete_player_name("", &names), PlayerNameCompletion::None);
        assert_eq!(complete_player_name("^7", &names), PlayerNameCompletion::None);
        // "a" is in both "RedJawa" and "Padawan".
        assert!(matches!(complete_player_name("A", &names), PlayerNameCompletion::Many(hits) if hits.len() == 2));
    }

    #[test]
    fn player_name_completion_ranks_exact_prefix_boundary_then_substring() {
        let names = vec![
            "SuperJawaGuy".to_owned(),
            "RedJawa".to_owned(),
            "JawaBob".to_owned(),
            "Foo_Jawa".to_owned(),
            "^2Jawa".to_owned(),
        ];
        assert_eq!(
            complete_player_name("^7jaw", &names),
            PlayerNameCompletion::Many(vec![
                "^2Jawa".to_owned(),
                "JawaBob".to_owned(),
                "Foo_Jawa".to_owned(),
                "RedJawa".to_owned(),
                "SuperJawaGuy".to_owned(),
            ])
        );
    }

    #[test]
    fn server_commands_complete_only_while_connected() {
        assert!(find_command("team", false).is_none());
        assert_eq!(find_command("TEAM", true).map(|entry| entry.kind), Some(EntryKind::ServerCommand));
        // "tea" also matches teamtask/teamvote: Tab extends to the common "team".
        assert_eq!(common_prefix("tea", true).as_deref(), Some("team"));
        assert_eq!(unique_prefix("callv", true).map(|entry| entry.name), Some("callvote"));
        assert!(unique_prefix("callv", false).is_none());
        // Local commands keep priority and are not listed twice.
        assert_eq!(prefix("say", true).filter(|entry| entry.name == "say").count(), 1);
        let names: Vec<_> = prefix("follow", true).map(|entry| entry.name).collect();
        assert_eq!(
            names,
            ["followFastest", "followRedFlag", "followBlueFlag", "follow", "follownext", "followprev"]
        );
    }

    #[test]
    fn japro_commands_complete_only_on_japro_servers() {
        set_japro_server(false);
        assert!(find_command("whois", true).is_none());
        set_japro_server(true);
        assert_eq!(find_command("whois", true).map(|entry| entry.kind), Some(EntryKind::ServerCommand));
        assert!(find_command("whois", false).is_none(), "not connected: nothing server-side completes");
        assert_eq!(unique_prefix("whoi", true).map(|entry| entry.name), Some("whois"));
        // Vanilla names are listed once even though jaPRO has them too.
        assert_eq!(prefix("callvote", true).count(), 1);
        assert_eq!(prefix("amtele", true).count(), 2, "amTele and amTeleMark");
        set_japro_server(false);
        assert!(find_command("whois", true).is_none());
    }

    #[test]
    fn map_name_ranking_matches_console_substrings_and_subsequences() {
        assert_eq!(rank_name_match("mp/ffa3", "mp/ffa3").unwrap().0, MatchTier::Exact);
        assert_eq!(rank_name_match("mp/ffa3", "mp/f").unwrap().0, MatchTier::Prefix);
        assert_eq!(rank_name_match("mp/ffa3", "ffa").unwrap().0, MatchTier::Substring);
        assert_eq!(rank_name_match("mp/ffa3", "mfa").unwrap().0, MatchTier::Subsequence);
        assert_eq!(rank_name_match("mp/ffa3", "zzz"), None);
    }

    #[test]
    fn suggestions_rank_exact_prefix_substring_then_fuzzy() {
        let (hits, total) = suggest("cg_fov", false, 8);
        assert_eq!(hits[0].entry.name, "cg_fov");
        assert_eq!(hits[0].tier, MatchTier::Exact);
        assert!(total >= 1);

        let (hits, _) = suggest("fps", false, 24);
        assert!(hits.iter().any(|hit| hit.entry.name == "com_maxfps" && hit.tier == MatchTier::Substring));
        let first_substring = hits.iter().position(|hit| hit.tier == MatchTier::Substring).unwrap();
        assert!(hits[..first_substring].iter().all(|hit| hit.tier < MatchTier::Substring));

        let (hits, _) = suggest("cgfov", false, 8);
        let fuzzy = hits.iter().find(|hit| hit.entry.name == "cg_fov").expect("cg_fov");
        assert_eq!(fuzzy.tier, MatchTier::Subsequence);
        assert_eq!(fuzzy.mask.count_ones(), 5);
    }

    #[test]
    fn suggestions_ignore_blank_queries_and_hide_server_commands_offline() {
        assert!(suggest("   ", false, 8).0.is_empty());
        assert!(suggest("callvot", false, 8).0.is_empty());
        assert_eq!(suggest("callvot", true, 8).0[0].entry.name, "callvote");
    }

    #[test]
    fn local_shader_remap_commands_complete_with_or_without_console_slash() {
        for cmd in ["remapShader", "listRemaps"] {
            assert_eq!(find_command(cmd, false).map(|e| e.name), Some(cmd));
            assert_eq!(find_command(&format!("/{cmd}"), false).map(|e| e.name), Some(cmd));
            assert_eq!(find_command(&format!("\\{cmd}"), false).map(|e| e.name), Some(cmd));
            assert!(prefix(&format!("/{cmd}"), false).any(|e| e.name == cmd));
            assert_eq!(suggest(&format!("/{cmd}"), false, 10).0[0].entry.name, cmd);
        }
        assert_eq!(unique_prefix("/remapS", false).map(|e| e.name), Some("remapShader"));
    }

    #[test]
    fn every_vanilla_gcmd_is_registered() {
        for name in [
            "addbot", "callteamvote", "callvote", "duelteam", "follow", "follownext", "followprev",
            "forcechanged", "give", "god", "kill", "levelshot", "loaddefered", "noclip", "notarget",
            "NPC", "say", "say_team", "setviewpos", "siegeclass", "stats", "t_use", "team", "teamtask",
            "teamvote", "tell", "voice_cmd", "vote", "where", "zoom",
        ] {
            assert!(is_server_command(name), "{name}");
        }
        assert!(is_server_command("vgs_cmd"));
        assert_eq!(SERVER_COMMANDS.len(), 36);
    }

    #[test]
    fn unique_prefix_resolves_a_single_entry() {
        assert_eq!(
            unique_prefix("cl_commandr", false).map(|entry| entry.name),
            Some("cl_commandRate")
        );
    }

    #[test]
    fn common_prefix_extends_ambiguous_names() {
        // The current registry has many r_p* entries. Completion should extend as far
        // as every matching name allows, but no farther.
        assert_eq!(common_prefix("r_p", false).as_deref(), Some("r_p"));
        assert_eq!(common_prefix("r_plan", false).as_deref(), Some("r_planardebug"));
    }

    #[test]
    fn fx_ab_cvars_are_registered_for_direct_console_use() {
        for name in [
            "r_flares",
            "r_worldFlares",
            "r_saberFlares",
            "r_saberImpactFx",
            "r_fxGeometry",
            "r_fxZeroAlphaDiscard",
        ] {
            let entry = find(name).unwrap_or_else(|| panic!("missing console cvar {name}"));
            assert_eq!(entry.kind, EntryKind::Cvar, "{name}");
        }
    }

    #[test]
    fn command_filter_matches_openjk_style_wildcards() {
        assert!(filter_match("demo*", "demo_speed"));
        assert!(filter_match("?elp", "help"));
        assert!(filter_match("VID_RESTART", "vid_restart"));
        assert!(!filter_match("demo", "demo_speed"));
    }

    #[test]
    fn restart_sensitive_cvars_are_registered_as_latched() {
        assert_eq!(find("r_backend").unwrap().latch, LatchScope::VidRestart);
        assert_eq!(find("r_pbr").unwrap().latch, LatchScope::MapLoadOrVidRestart);
        assert_eq!(
            find("fs_allowAssetOverrides").unwrap().latch,
            LatchScope::MapLoadOrVidRestart
        );
        assert_eq!(find("r_swapInterval").unwrap().latch, LatchScope::None);
    }
}

#[cfg(test)]
mod discrete_option_tests {
    use super::*;

    #[test]
    fn declared_integer_options_are_explicit_only() {
        let discrete = cvar("x", "0", "0|1|2", "");
        assert_eq!(declared_integer_options(&discrete), Some(vec!["0".into(), "1".into(), "2".into()]));
        let annotated = cvar("x", "20", "5|10|20|30|60 seconds", "");
        assert_eq!(declared_integer_options(&annotated), Some(vec!["5".into(), "10".into(), "20".into(), "30".into(), "60".into()]));
        let boolean = cvar("x", "0", "0..1", "");
        assert_eq!(declared_integer_options(&boolean), Some(vec!["0".into(), "1".into()]));
        let slider = cvar("x", "0", "0..10000", "");
        assert_eq!(declared_integer_options(&slider), None);
    }
}
