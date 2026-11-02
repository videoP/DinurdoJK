//! Typed argument completion declarations. Command dispatch is deliberately
//! separate from candidate discovery, matching and the console popup.
//!
//! Register new completions in `source_for`. Providers never perform IO here;
//! they consume catalog results built by the existing UI catalog worker.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Source {
    Maps, VoteMaps, Demos, GhostDemos, Configs, Shaders, Players, TrailPlayers, Trails, Models, Sabers,
    Cvars, Commands, Registered, Keys, Entities, Targets,
    Teams, Give, Vote, Callvote, Duelteam, LoadTrailSlots,
}

impl Source {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Maps | Self::VoteMaps => "Map", Self::Demos | Self::GhostDemos => "Demo", Self::Configs => "Config",
            Self::Shaders => "Shader", Self::Players | Self::TrailPlayers => "Player", Self::Trails => "Trail",
            Self::Models => "Model", Self::Sabers => "Saber", Self::Cvars => "Cvar",
            Self::Commands => "Command", Self::Registered => "Command / CVAR", Self::Keys => "Key", Self::Entities => "Entity",
            Self::Targets => "Target", Self::Teams => "Team", Self::Give => "Give",
            Self::Vote => "Vote", Self::Callvote => "Vote option", Self::Duelteam => "Duel team",
            Self::LoadTrailSlots => "Trail slot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct Context {
    pub(super) source: Source,
    pub(super) start: usize,
    pub(super) end: usize,
}

// Important: only complete the *argument containing the caret*; never mutate
// trailing arguments or the command name. Quoted free-form messages and cvar
// values are intentionally outside this system.
pub(in crate::app) fn context(input: &str, cursor: usize) -> Option<Context> {
    let (verb_start, verb_end) = super::App::console_first_token_span(input)?;
    if cursor <= verb_end || !input[verb_end..].starts_with(char::is_whitespace) {
        return None;
    }
    let verb = input[verb_start..verb_end].trim_start_matches(['/', '\\']);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut pos = verb_end;
    while pos < input.len() {
        while pos < input.len() && input.as_bytes()[pos].is_ascii_whitespace() { pos += 1; }
        if pos == input.len() { break; }
        let start = pos;
        while pos < input.len() && !input.as_bytes()[pos].is_ascii_whitespace() { pos += 1; }
        spans.push((start, pos));
    }
    let (index, start, end) = if let Some((index, (start, end))) = spans.iter().copied()
        .enumerate().find(|(_, (start, end))| *start <= cursor && cursor <= *end)
    { (index, start, end) }
    else if cursor == input.len() && input.ends_with(char::is_whitespace) {
        (spans.len(), cursor, cursor)
    } else { return None };
    let previous = if index > 0 { spans.get(index - 1).map(|(s,e)| &input[*s..*e]) } else { None };
    let source = source_for(verb, index, previous)?;
    Some(Context {source, start, end})
}

/// Ordered by actual command argument index, not by value type.
/// Deliberately excludes generic cvar-value completion (0/1/2/3 etc.).
pub(super) fn source_for(command: &str, index: usize, previous: Option<&str>) -> Option<Source> {
    let command = command.to_ascii_lowercase();
    let previous = previous.unwrap_or("");
    Some(match (command.as_str(), index) {
        ("map" | "devmap", 0) => Source::Maps,
        ("ammap", 0) => Source::VoteMaps,
        ("demo", 0) => Source::Demos,
        ("rghost", 0) => Source::GhostDemos,
        ("exec" | "execq", 0) => Source::Configs,
        ("remapshader", 0 | 1) => Source::Shaders,
        ("follow" | "tell", 0) => Source::Players,
        ("strafetrail", 0) => Source::TrailPlayers,
        ("loadtrail", 0) => Source::Trails,
        ("loadtrail", 1) => Source::LoadTrailSlots,
        ("model" | "cg_forcemodel", 0) => Source::Models,
        ("saber1" | "saber2", 0) => Source::Sabers,
        ("set" | "seta" | "sets" | "setu" | "toggle" | "vstr", 0) => Source::Cvars,
        ("help", 0) => Source::Registered,
        ("bind" | "bindspec" | "binddemo" | "unbind" | "unbindspec" | "unbinddemo", 0) => Source::Keys,
        ("bind" | "bindspec" | "binddemo", 1) => Source::Commands,
        ("team", 0) => Source::Teams,
        ("give", 0) => Source::Give,
        ("vote", 0) => Source::Vote,
        ("duelteam", 0) => Source::Duelteam,
        ("callvote", 0) => Source::Callvote,
        ("callvote", 1) if previous.eq_ignore_ascii_case("map") => Source::VoteMaps,
        ("callvote", 1) if previous.eq_ignore_ascii_case("kick")
            || previous.eq_ignore_ascii_case("clientkick") => Source::Players,
        ("entities" | "entgraph", 0) => Source::Entities,
        ("t_use", 0) => Source::Targets,
        _ => return None,
    })
}

pub(super) fn static_values(source: Source) -> &'static [&'static str] {
    match source {
        Source::Teams => &["free", "f", "red", "r", "blue", "b", "spectator", "spectate", "spec", "s", "follow1", "follow2", "scoreboard"],
        Source::Give => &["all", "health", "armor", "ammo", "weapons", "force"],
        Source::Vote => &["yes", "no"],
        Source::Duelteam => &["free", "single", "double"],
        Source::Callvote => &["map", "map_restart", "g_gametype", "kick", "clientkick", "g_doWarmup", "timelimit", "fraglimit", "nextmap"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(input: &str, source: Source, fragment: &str) {
        let ctx = context(input, input.len()).expect(input);
        assert_eq!(ctx.source, source);
        assert_eq!(&input[ctx.start..ctx.end], fragment);
    }
    #[test]
    fn routes_arguments_and_nested_arguments() {
        check("/devmap co", Source::Maps, "co");
        check("/map ", Source::Maps, "");
        check("/callvote map mp/f", Source::VoteMaps, "mp/f");
        check("/bind MOUSE3 +att", Source::Commands, "+att");
        check("/remapShader old textures/", Source::Shaders, "textures/");
        check("/tell 3", Source::Players, "3");
        check("/execq auto", Source::Configs, "auto");
        check("/loadTrail mytrail 2", Source::LoadTrailSlots, "2");
        assert!(context("/set r_gamma 1", 14).is_none());
        assert!(context("/mapname test", 13).is_none());
    }
}
