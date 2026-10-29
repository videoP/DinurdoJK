//! Console completion.
pub(in crate::app) mod arguments;
use arguments::{Context, Source};
use crate::app::{ui, App, OverlayMode};

#[derive(Debug, Clone)]
pub(in crate::app) enum ConsoleHit {
    Registered(crate::console::Suggestion),
    Map { name: String, tier: crate::console::MatchTier, mask: u64, source: bool },
    Argument { value: String, label: String, tier: crate::console::MatchTier, mask: u64,
               description: &'static str },
}

impl ConsoleHit {
    fn name(&self) -> &str {
        match self {
            Self::Registered(hit) => hit.entry.name,
            Self::Map { name, .. } => name,
            Self::Argument { value, .. } => value,
        }
    }

    fn tier(&self) -> crate::console::MatchTier {
        match self {
            Self::Registered(hit) => hit.tier,
            Self::Map { tier, .. } => *tier,
            Self::Argument { tier, .. } => *tier,
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Registered(hit) => hit.entry.description,
            Self::Map { source: true, .. } => "Source .map asset in the active game; loads source geometry directly.",
            Self::Map { source: false, .. } => "Compiled .bsp map in the active game; loads as a local map.",
            Self::Argument { description, .. } => description,
        }
    }
}


/// Common byte prefix; actual map qpaths validated by the loader are ASCII.
fn common_map_prefix(names: &[&str]) -> String {
    let Some(first) = names.first() else { return String::new() };
    let mut len = first.len();
    for name in names.iter().skip(1) {
        len = first.bytes().zip(name.bytes()).take(len)
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
    }
    // Avoid splitting a UTF-8 character in malformed/unloadable asset names.
    while !first.is_char_boundary(len) { len -= 1; }
    first[..len].to_owned()
}

impl App {
    pub(in crate::app) fn format_console_entry(&self, entry: &crate::console::Entry) -> String {
        match entry.kind {
            crate::console::EntryKind::Command => {
                format!("^3{}^7 [command] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::ServerCommand => {
                format!("^3{}^7 [server] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::Cvar => {
                let value = self
                    .console_cvar_value(entry.name)
                    .unwrap_or_else(|| "?".into());
                let latched = self
                    .latched_console_cvars
                    .get(&entry.name.to_ascii_lowercase())
                    .map(|value| format!("  latched: \"{}\"", value))
                    .unwrap_or_default();
                if entry.range.is_empty() {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  - {}",
                        entry.name, value, latched, entry.default, entry.description
                    )
                } else {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  range: {}  - {}",
                        entry.name, value, latched, entry.default, entry.range, entry.description
                    )
                }
            }
        }
    }

    pub(in crate::app) fn console_first_token_span(input: &str) -> Option<(usize, usize)> {
        let start = input.find(|ch: char| !ch.is_whitespace())?;
        let end = input[start..]
            .find(char::is_whitespace)
            .map_or(input.len(), |offset| start + offset);
        Some((start, end))
    }

    pub(in crate::app) fn replace_console_first_token(
        &mut self,
        replacement: &str,
        append_space: bool,
    ) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        self.console_input.replace_range(start..end, replacement);
        if append_space && self.console_input.len() == start + replacement.len() {
            self.console_input.push(' ');
        }
        self.console_cursor = self.console_input.len();
        self.console_history_index = None;
        self.console_scroll = 0;
    }

    /// Return the editable map argument, but only while the caret is in the
    /// first argument of /map or /devmap (not in the command or later arguments).
    pub(in crate::app) fn console_map_argument_span(input: &str, cursor: usize) -> Option<(usize, usize)> {
        let (verb_start, verb_end) = Self::console_first_token_span(input)?;
        let verb = input[verb_start..verb_end].trim_start_matches(['/', '\\']);
        if !verb.eq_ignore_ascii_case("map") && !verb.eq_ignore_ascii_case("devmap") {
            return None;
        }
        let rest = input.get(verb_end..)?;
        if !rest.starts_with(char::is_whitespace) || cursor <= verb_end {
            return None;
        }
        let arg_start = verb_end + rest.len() - rest.trim_start_matches(char::is_whitespace).len();
        if cursor < arg_start {
            return None;
        }
        let arg_end = input[arg_start..]
            .find(char::is_whitespace)
            .map_or(input.len(), |offset| arg_start + offset);
        (cursor <= arg_end).then_some((arg_start, arg_end))
    }

    /// Map names come from the existing worker-populated VFS catalogs, so no
    /// PK3 scan or filesystem walk happens on a keystroke or a rendered frame.
    fn console_map_hits(&self, query: &str, limit: usize) -> (Vec<ConsoleHit>, usize) {
        if let Some((previous, bsp_count, map_count, cached_limit, hits, total)) =
            self.console_map_suggest_cache.borrow().as_ref()
        {
            if previous == query && *bsp_count == self.solo_maps.len()
                && *map_count == self.source_maps.len() && *cached_limit == limit
            {
                return (hits.clone(), *total);
            }
        }
        let mut hits = Vec::new();
        let mut add = |name: String, source: bool| {
            let ranked = if query.is_empty() {
                Some((crate::console::MatchTier::Prefix, 0))
            } else {
                crate::console::rank_name_match(&name, query)
            };
            if let Some((tier, mask)) = ranked {
                hits.push(ConsoleHit::Map { name, tier, mask, source });
            }
        };
        // The parser also accepts 'maps/' paths and explicit .bsp extensions.
        // Offer those spellings only when the user actually starts typing them,
        // keeping the default list clean and extensionless for compiled maps.
        let query_lower = query.to_ascii_lowercase();
        let maps_prefix = query_lower.starts_with("maps/");
        let bsp_extension = query_lower.contains(".b");
        for entry in &self.solo_maps {
            if crate::scene::MapSource::from_map_argument(&entry.map_name).is_ok() {
                let name = if maps_prefix {
                    format!("maps/{}", entry.map_name)
                } else {
                    entry.map_name.clone()
                };
                if bsp_extension {
                    add(format!("{name}.bsp"), false);
                } else {
                    add(name, false);
                }
            }
        }
        for entry in &self.source_maps {
            // Explicit .map is required here; an extensionless name resolves to BSP.
            let name = if maps_prefix {
                format!("maps/{}.map", entry.map_name)
            } else {
                format!("{}.map", entry.map_name)
            };
            if crate::scene::MapSource::from_map_argument(&name).is_ok() {
                add(name, true);
            }
        }
        hits.sort_by(|a, b| {
            (a.tier(), a.name().len(), a.name().to_ascii_lowercase())
                .cmp(&(b.tier(), b.name().len(), b.name().to_ascii_lowercase()))
        });
        let total = hits.len();
        hits.truncate(limit);
        *self.console_map_suggest_cache.borrow_mut() = Some((
            query.to_owned(), self.solo_maps.len(), self.source_maps.len(), limit,
            hits.clone(), total,
        ));
        (hits, total)
    }

    /// Every provider emits a plain insertion value and (optionally) a richer
    /// popup label. A player, for example, inserts its stable client slot but
    /// displays the slot together with the live player name.
    fn console_argument_hits(&self, source: Source, query: &str, limit: usize) -> (Vec<ConsoleHit>, usize) {
        if source == Source::Maps { return self.console_map_hits(query, limit); }
        if !matches!(source, Source::Players | Source::TrailPlayers | Source::Entities | Source::Targets) {
            if let Some((cached_query, cached_source, hits, total)) =
                self.console_arg_suggest_cache.borrow().as_ref()
            {
                if cached_query == query && *cached_source == source {
                    return (hits.clone(), *total);
                }
            }
        }
        let mut hits = Vec::new();
        let mut add = |value: &str, label: &str, description: &'static str| {
            if value.is_empty() { return; }
            let ranked = if query.is_empty() {
                Some((crate::console::MatchTier::Prefix, 0))
            } else {
                crate::console::rank_name_match(label, query)
                    .or_else(|| crate::console::rank_name_match(value, query))
            };
            if let Some((tier, mask)) = ranked {
                hits.push(ConsoleHit::Argument {
                    value: value.to_owned(), label: label.to_owned(), tier, mask, description,
                });
            }
        };
        match source {
            Source::Maps => unreachable!(),
            Source::VoteMaps => for entry in &self.solo_maps {
                add(&entry.map_name, &entry.map_name,
                    "Locally installed BSP; the server must also support this map.");
            },
            Source::Demos | Source::GhostDemos => for name in &self.console_files.demos {
                add(name, name, "Available .dm_26 demo in the active game VFS.");
            },
            Source::Configs => for name in &self.console_files.configs {
                add(name, name, "Indexed .cfg file; accepts an optional .cfg extension.");
            },
            Source::Trails => for name in &self.console_files.trails {
                add(name, name, "Indexed strafetrails/<name>.cfg file.");
            },
            Source::Shaders => for entry in &self.asset_viewer_entries {
                if let Some(name) = &entry.shader_name {
                    add(name, name, "Shader definition from the asset viewer index.");
                }
            },
            Source::Models => for entry in &self.profile_models {
                add(&entry.value, &entry.value, "Installed model/skin preset from the profile index.");
            },
            Source::Sabers => for entry in &self.profile_sabers {
                add(&entry.name, &entry.name, "Saber hilt definition from the profile index.");
            },
            Source::Players | Source::TrailPlayers => {
                if source == Source::TrailPlayers {
                    add("-1", "-1  Stop tracing", "Disable live player trail tracing.");
                }
                if let Some(session) = self.game_session.as_ref() {
                    for slot in 0..32 {
                        if let Some(name) = session.client_game.client_name(slot).filter(|s| !s.is_empty()) {
                            let display = crate::logging::strip_jka_colors(&name);
                            let value = slot.to_string();
                            let label = format!("{value}  {display}");
                            add(&value, &label, "Live connected player; inserts the client slot.");
                        }
                    }
                }
            },
            Source::Cvars | Source::Commands | Source::Registered => {
                let connected = self.console_scope();
                for entry in crate::console::cvars().filter(|_| source != Source::Commands)
                    .chain(crate::console::commands(connected).filter(|_| source != Source::Cvars))
                {
                    add(entry.name, entry.name, entry.description);
                }
                if source == Source::Commands {
                    // Commands in binds include +movement actions, which aren't all
                    // part of the registered console command/cvar set.
                    for entry in crate::keybinds::CONTROL_ACTIONS {
                        add(entry.command, entry.command, "Built-in bindable control action.");
                    }
                }
            },
            Source::Keys => {
                for key in ["SPACE", "TAB", "ENTER", "ESCAPE", "BACKSPACE", "UPARROW", "DOWNARROW",
                    "LEFTARROW", "RIGHTARROW", "ALT", "CTRL", "SHIFT", "INS", "DEL", "PGUP", "PGDN",
                    "HOME", "END", "PAUSE", "SEMICOLON", "APOSTROPHE", "COMMA", "PERIOD", "SLASH",
                    "BACKSLASH", "MINUS", "EQUALS", "LBRACKET", "RBRACKET", "MWHEELUP", "MWHEELDOWN"] {
                    add(key, key, "Valid bind/unbind key name.");
                }
                for n in 1..=12 { let name = format!("F{n}"); add(&name, &name, "Function key."); }
                for n in 1..=5 { let name = format!("MOUSE{n}"); add(&name, &name, "Mouse button."); }
                for ch in b'A'..=b'Z' { let name = (ch as char).to_string(); add(&name, &name, "Letter key."); }
                for n in 0..=9 { let name = n.to_string(); add(&name, &name, "Digit key."); }
            },
            Source::Entities | Source::Targets => {
                if let Some(graph) = self.entity_graph.as_ref() {
                    for entity in &graph.entities {
                        if source == Source::Entities && !entity.classname.is_empty() {
                            add(&entity.classname, &entity.classname, "Classname from the current BSP entity lump.");
                        }
                        if !entity.targetname.is_empty() {
                            add(&entity.targetname, &entity.targetname, "Targetname from the current BSP entity lump.");
                        }
                    }
                }
            },
            Source::Teams | Source::Give | Source::Vote | Source::Callvote | Source::Duelteam => {
                for &name in arguments::static_values(source) {
                    add(name, name, "Accepted command argument from the command registry.");
                }
            },
            Source::LoadTrailSlots => {
                for n in 0..crate::strafe_trail::MAX_CLIENTS {
                    let name = n.to_string();
                    add(&name, &name, "Destination trail slot.");
                }
            },
        }
        if source == Source::GhostDemos {
            add("clear", "clear", "Remove all loaded race ghosts.");
        }
        // Candidate catalogs may include duplicate shader definitions or
        // repeated bind actions. Preserve the best score while deduplicating.
        hits.sort_by(|a,b| {
            (a.tier(), a.name().len(), a.name().to_ascii_lowercase())
                .cmp(&(b.tier(), b.name().len(), b.name().to_ascii_lowercase()))
        });
        let mut distinct = std::collections::HashSet::new();
        hits.retain(|hit| distinct.insert(hit.name().to_ascii_lowercase()));
        let total = hits.len();
        hits.truncate(limit);
        if !matches!(source, Source::Players | Source::TrailPlayers | Source::Entities | Source::Targets) {
            *self.console_arg_suggest_cache.borrow_mut() = Some((query.to_owned(), source, hits.clone(), total));
        }
        (hits, total)
    }

    fn argument_context(&self) -> Option<Context> {
        arguments::context(&self.console_input, self.console_cursor)
    }

    fn replace_console_argument(&mut self, start: usize, end: usize, name: &str) {
        self.console_input.replace_range(start..end, name);
        self.console_cursor = start + name.len();
        self.console_history_index = None;
        self.console_scroll = 0;
        self.console_suggest_reset();
    }

    fn ensure_console_argument_catalog(&mut self) {
        let Some(context) = self.argument_context() else { return; };
        match context.source {
            Source::Maps => { self.ensure_solo_map_catalog(); self.ensure_source_map_catalog(); },
            Source::VoteMaps => self.ensure_solo_map_catalog(),
            Source::Demos | Source::GhostDemos | Source::Configs | Source::Trails => self.ensure_console_file_catalog(),
            Source::Shaders => self.ensure_asset_viewer_catalog(),
            Source::Models | Source::Sabers => self.ensure_profile_catalog(),
            _ => {},
        }
    }

    pub(in crate::app) fn complete_console_input(&mut self) {
        if let Some(Context { source, start, end }) = self.argument_context() {
            let query = self.console_input[start..end].to_owned();
            let (hits, total) = self.console_argument_hits(source, &query, 24);
            if total == 1 {
                self.replace_console_argument(start, end, hits[0].name());
            } else if hits.is_empty() {
                self.push_console_line(format!("^5No {} matches for '{query}' (catalog may still be loading).", source.label()));
            } else {
                self.push_console_line(format!("^5{total} {} match(es) for '{query}':^7", source.label()));
                for hit in hits {
                    self.push_console_line(format!("^3{}^7", hit.name()));
                }
            }
            self.publish_ui();
            return;
        }
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            self.push_console_line("^7Type a command/cvar prefix, then press Tab to complete.");
            self.publish_ui();
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        if prefix.is_empty() {
            return;
        }

        // An exact registered name always wins, even if it is itself a prefix of
        // another command. Otherwise a single prefix match is unambiguous.
        let connected = self.console_scope();
        if let Some(entry) = crate::console::find_command(&prefix, connected)
            .or_else(|| crate::console::unique_prefix(&prefix, connected))
        {
            self.replace_console_first_token(entry.name, true);
            self.publish_ui();
            return;
        }

        let mut matches: Vec<_> = crate::console::prefix(&prefix, connected)
            .copied()
            .collect();
        if matches.is_empty() {
            self.push_console_line(format!("^1No commands or cvars start with '{prefix}'."));
            self.publish_ui();
            return;
        }

        if let Some(common) = crate::console::common_prefix(&prefix, connected) {
            if common.len() > prefix.len() || common != prefix {
                self.replace_console_first_token(&common, false);
            }
        }

        matches.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        self.push_console_line(format!("^5{} match(es) for '{}':^7", matches.len(), prefix));
        for entry in matches {
            let line = self.format_console_entry(&entry);
            self.push_console_line(line);
        }
        self.publish_ui();
    }

    /// Interactive argument suggestions reuse the exact same popup as cvars.
    /// For known command arguments, show candidates from the typed provider;
    /// otherwise preserve the original registered command/cvar completion.
    pub(in crate::app) fn console_suggest_state(
        &self,
    ) -> Option<(Vec<ConsoleHit>, usize, bool)> {
        if !self.console_suggest || self.overlay != OverlayMode::Console || self.console_search_open {
            return None;
        }
        if let Some(Context { source, start, end }) = self.argument_context() {
            if self.console_suggest_hidden { return None; }
            let (hits, total) = self.console_argument_hits(source, &self.console_input[start..end], 24);
            return (!hits.is_empty()).then_some((hits, total, false));
        }
        let (start, end) = Self::console_first_token_span(&self.console_input)?;
        let token = &self.console_input[start..end];
        let connected = self.console_scope();
        if self.console_cursor > end {
            let entry = crate::console::find_command(token, connected)?;
            let hit = crate::console::Suggestion {
                entry,
                tier: crate::console::MatchTier::Exact,
                mask: 0,
            };
            return Some((vec![ConsoleHit::Registered(hit)], 1, true));
        }
        if self.console_suggest_hidden {
            return None;
        }
        let (hits, total) = crate::console::suggest(token, connected, 24);
        (!hits.is_empty()).then_some((hits.into_iter().map(ConsoleHit::Registered).collect(), total, false))
    }

    pub(in crate::app) fn console_suggest_list_open(&self) -> bool {
        matches!(self.console_suggest_state(), Some((_, _, false)))
    }

    pub(in crate::app) fn console_suggest_snapshot(
        &self,
    ) -> (Vec<ui::ConsoleSuggestion>, usize, bool) {
        let Some((hits, total, hint)) = self.console_suggest_state() else {
            return (Vec::new(), 0, false);
        };
        let rows = hits
            .iter()
            .map(|hit| match hit {
                ConsoleHit::Registered(hit) => {
                    let entry = hit.entry;
                    let is_cvar = entry.kind == crate::console::EntryKind::Cvar;
                    let value = if is_cvar {
                        self.console_cvar_value(entry.name).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let same = |a: &str, b: &str| {
                        a.trim() == b.trim()
                            || matches!((a.trim().parse::<f64>(), b.trim().parse::<f64>()), (Ok(a), Ok(b)) if a == b)
                    };
                    ui::ConsoleSuggestion {
                        name: entry.name.to_owned(),
                        kind: match entry.kind {
                            crate::console::EntryKind::Cvar => ui::ConsoleSuggestKind::Cvar,
                            crate::console::EntryKind::Command => ui::ConsoleSuggestKind::Command,
                            crate::console::EntryKind::ServerCommand => ui::ConsoleSuggestKind::Server,
                        },
                        mask: hit.mask,
                        modified: is_cvar && !value.is_empty() && !same(&value, entry.default),
                        value,
                        default_value: entry.default,
                        range: entry.range,
                        description: entry.description,
                    }
                }
                ConsoleHit::Map { name, mask, source, .. } => ui::ConsoleSuggestion {
                    name: name.clone(),
                    kind: ui::ConsoleSuggestKind::Map,
                    mask: *mask,
                    modified: false,
                    value: String::new(),
                    default_value: "",
                    range: "",
                    description: if *source {
                        "Source .map asset in the active game; loads source geometry directly."
                    } else {
                        "Compiled .bsp map in the active game; loads as a local map."
                    },
                },
                ConsoleHit::Argument { label, mask, description, .. } => ui::ConsoleSuggestion {
                    name: label.clone(),
                    kind: ui::ConsoleSuggestKind::Argument,
                    mask: *mask,
                    modified: false,
                    value: String::new(),
                    default_value: "",
                    range: "",
                    description,
                },
            })
            .collect();
        (rows, total, hint)
    }

    /// The typed text changed: the popup starts over at the best match.
    pub(in crate::app) fn console_suggest_reset(&mut self) {
        self.console_suggest_index = 0;
        self.console_suggest_picked = false;
        self.console_suggest_hidden = false;
        *self.console_arg_suggest_cache.borrow_mut() = None;
        self.ensure_console_argument_catalog();
    }

    /// Put `name` in place of the first token and leave the caret one space
    /// past it, ready for arguments.
    pub(in crate::app) fn apply_console_suggestion(&mut self, name: &str) {
        if let Some(Context { start, end, .. }) = self.argument_context() {
            self.replace_console_argument(start, end, name);
            return;
        }
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        let had_space = self.console_input[end..].starts_with(char::is_whitespace);
        self.console_input.replace_range(start..end, name);
        let mut cursor = start + name.len();
        if !had_space {
            self.console_input.insert(cursor, ' ');
        }
        cursor += 1;
        self.console_cursor = cursor.min(self.console_input.len());
        self.console_history_index = None;
        self.console_scroll = 0;
        self.console_suggest_reset();
    }

    /// Tab with the popup open: extend to the shared prefix when the best
    /// matches all start with what was typed, otherwise take the highlighted row.
    pub(in crate::app) fn tab_console_suggestion(&mut self) {
        let Some((hits, total, false)) = self.console_suggest_state() else {
            return;
        };
        if let Some(Context { start, end, .. }) = self.argument_context() {
            if !self.console_suggest_picked && hits[0].tier() == crate::console::MatchTier::Prefix {
                let query = &self.console_input[start..end];
                let prefix: Vec<_> = hits.iter().filter(|hit| hit.name().len() >= query.len()
                    && hit.name().as_bytes()[..query.len()].eq_ignore_ascii_case(query.as_bytes())).collect();
                if prefix.len() == 1 && total == 1 {
                    let name = prefix[0].name().to_owned();
                    self.replace_console_argument(start, end, &name);
                    return;
                }
                if !prefix.is_empty() && total == hits.len() {
                    let names = prefix.iter().map(|hit| hit.name()).collect::<Vec<_>>();
                    let common = common_map_prefix(&names);
                    if common.len() > query.len() {
                        self.replace_console_argument(start, end, &common);
                        return;
                    }
                }
            }
            let index = if self.console_suggest_picked {
                self.console_suggest_index.min(hits.len() - 1)
            } else {
                0
            };
            let name = hits[index].name().to_owned();
            self.replace_console_argument(start, end, &name);
            return;
        }
        if !self.console_suggest_picked && hits[0].tier() == crate::console::MatchTier::Prefix {
            if let Some((start, end)) = Self::console_first_token_span(&self.console_input) {
                let token = self.console_input[start..end].to_owned();
                // Match TaystJK/legacy console ergonomics: if this prefix can
                // only resolve to one registered command/cvar, Tab completes
                // the whole token *and* leaves a trailing space ready for its
                // first argument. Previously the live-suggestion path stopped
                // after extending the common prefix ("conn" -> "connect") and
                // required a second keypress before arguments could be typed.
                if let Some(entry) = crate::console::unique_prefix(&token, self.console_scope()) {
                    self.apply_console_suggestion(entry.name);
                    return;
                }
                if let Some(common) = crate::console::common_prefix(&token, self.console_scope()) {
                    if common.len() > token.len() {
                        self.console_input.replace_range(start..end, &common);
                        self.console_cursor = start + common.len();
                        self.console_suggest_reset();
                        return;
                    }
                }
            }
        }
        let index = if self.console_suggest_picked {
            self.console_suggest_index.min(hits.len() - 1)
        } else {
            0
        };
        self.apply_console_suggestion(hits[index].name());
    }

    /// Enter on an arrowed-to row that is not what was typed fills it in
    /// instead of running the half-typed text.
    pub(in crate::app) fn accept_picked_console_suggestion(&mut self) -> bool {
        if !self.console_suggest_picked {
            return false;
        }
        let Some((hits, _, false)) = self.console_suggest_state() else {
            return false;
        };
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return false;
        };
        let name = hits[self.console_suggest_index.min(hits.len() - 1)].name();
        let (start, end) = self.argument_context().map(|ctx| (ctx.start, ctx.end))
            .unwrap_or((start, end));
        if self.console_input[start..end].eq_ignore_ascii_case(name) {
            return false;
        }
        self.apply_console_suggestion(name);
        true
    }

    pub(in crate::app) fn step_console_suggestion(&mut self, delta: i32) {
        let Some((hits, _, false)) = self.console_suggest_state() else {
            return;
        };
        let current = if self.console_suggest_picked {
            self.console_suggest_index
        } else {
            0
        };
        self.console_suggest_index =
            (current as i32 + delta).rem_euclid(hits.len() as i32) as usize;
        self.console_suggest_picked = true;
    }

    /// Left click on the popup or the header switch. Returns whether it was consumed.
    pub(in crate::app) fn click_console_suggest(
        &mut self,
        width: u32,
        height: u32,
        x: f64,
        y: f64,
    ) -> bool {
        if ui::console_suggest_chip_hit(width, x, y) {
            let next = if self.console_suggest { "0" } else { "1" };
            let _ = self.set_console_cvar("con_suggest", next);
            return true;
        }
        let Some((hits, _, hint)) = self.console_suggest_state() else {
            return false;
        };
        if hint {
            return false;
        }
        let selected = if self.console_suggest_picked {
            self.console_suggest_index.min(hits.len() - 1)
        } else {
            0
        };
        let detail_lines =
            ui::console_suggest_detail_line_count(width, height, hits[selected].description());
        let Some(index) = ui::console_suggest_hit(
            width,
            height,
            self.console_size,
            self.console_scale,
            hits.len(),
            selected,
            detail_lines,
            x,
            y,
        ) else {
            return false;
        };
        self.apply_console_suggestion(hits[index].name());
        true
    }

    pub(in crate::app) fn complete_unique_console_command(&mut self) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        let connected = self.console_scope();
        if crate::console::find_command(&prefix, connected).is_some() {
            return;
        }
        if let Some(entry) = crate::console::unique_prefix(&prefix, connected) {
            self.replace_console_first_token(entry.name, false);
        }
    }

    pub(in crate::app) fn browse_console_history(&mut self, direction: i32) {
        if self.console_history.is_empty() {
            return;
        }
        let len = self.console_history.len();
        let next = match (self.console_history_index, direction.signum()) {
            (None, -1) => Some(len - 1),
            (Some(index), -1) => Some(index.saturating_sub(1)),
            (Some(index), 1) if index + 1 < len => Some(index + 1),
            (Some(_), 1) => None,
            (index, _) => index,
        };
        self.console_history_index = next;
        self.console_input = next
            .map(|index| self.console_history[index].clone())
            .unwrap_or_default();
        if let Some((start, end)) = Self::console_first_token_span(&self.console_input) {
            if end == self.console_input.len()
                && crate::console::find_command(
                    &self.console_input[start..end],
                    self.console_scope(),
                )
                .is_some_and(|entry| entry.kind == crate::console::EntryKind::Cvar)
            {
                self.console_input.push(' ');
            }
        }
        self.console_cursor = self.console_input.len();
        self.console_scroll = 0;
        // A recalled command is not a search: keep Up/Down on history.
        self.console_suggest_reset();
        self.console_suggest_hidden = true;
        self.publish_ui();
    }
}

#[cfg(test)]
mod map_completion_tests {
    use super::{common_map_prefix, App};

    #[test]
    fn console_only_completes_the_map_argument() {
        for (input, expected) in [
            ("/map ", Some((5, 5))),
            ("/devmap mp/ffa", Some((8, 14))),
            ("devmap    duel", Some((10, 14))),
            ("\\map test.map", Some((5, 13))),
            ("MAP mp/ffa3", Some((4, 11))),
            ("/connect mp/ffa3", None),
            ("/map", None),
            ("mapname test", None),
        ] {
            assert_eq!(App::console_map_argument_span(input, input.len()), expected, "{input}");
        }
        assert_eq!(App::console_map_argument_span("/map mp/ffa3", 2), None);
        assert_eq!(App::console_map_argument_span("/map mp/ffa3 more", 17), None);
    }

    #[test]
    fn map_completion_common_prefix_preserves_paths() {
        assert_eq!(common_map_prefix(&["mp/ffa3", "mp/ffa4", "mp/ffa5"]), "mp/ffa");
        assert_eq!(common_map_prefix(&["mp/ffa3", "mp/FFA3"]), "mp/ffa3");
    }
}
