//! Server votes: the `CS_VOTE_*` configstrings, the `CG_DrawVote` readout and
//! the `callvote` command lines the Vote menu builds.

/// `VOTE_TIME`: how long a vote stays open (bg_public.h).
pub const VOTE_TIME_MS: i32 = 30_000;

pub const CS_VOTE_TIME: u16 = 8;
pub const CS_VOTE_STRING: u16 = 9;
pub const CS_VOTE_YES: u16 = 10;
pub const CS_VOTE_NO: u16 = 11;

/// A vote that is currently open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteUi {
    pub seconds_left: i32,
    /// Human name of the vote ("Restart Map", "New Map", "Kick Player", ...).
    pub label: String,
    /// The vote's argument ("mp/ffa3", a player name, "Duel"), if it has one.
    pub param: Option<String>,
    pub yes: i32,
    pub no: i32,
}

impl VoteUi {
    /// `CG_DrawVote`'s first line.
    pub fn hud_line(&self) -> String {
        match self.param.as_deref().filter(|param| !param.is_empty()) {
            Some(param) => format!(
                "^7Vote({}):<{} {}^7> Yes:{} No:{}",
                self.seconds_left, self.label, param, self.yes, self.no
            ),
            None => format!(
                "^7Vote({}):<{}^7> Yes:{} No:{}",
                self.seconds_left, self.label, self.yes, self.no
            ),
        }
    }
}

/// Turn a `voteString` into the label and argument `CG_DrawVote` shows.
pub fn describe(vote_string: &str) -> (String, Option<String>) {
    let starts = |prefix: &str| {
        vote_string
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    if starts("map_restart") {
        ("Restart Map".into(), None)
    } else if starts("vstr nextmap") {
        ("Next Map".into(), None)
    } else if starts("g_doWarmup") {
        ("Warmup".into(), None)
    } else if starts("g_gametype") {
        ("Game Type".into(), Some(vote_string.get(11..).unwrap_or("").to_owned()))
    } else if starts("map") {
        ("New Map".into(), Some(vote_string.get(4..).unwrap_or("").to_owned()))
    } else if starts("kick") {
        ("Kick Player".into(), Some(vote_string.get(5..).unwrap_or("").to_owned()))
    } else {
        // Custom votes such as polls.
        (vote_string.to_owned(), None)
    }
}

/// Read the vote configstrings at cgame time `now_ms`. `None` while no vote is
/// running (`cgs.voteTime == 0`).
pub fn read(get: impl Fn(u16) -> Option<String>, now_ms: i32) -> Option<VoteUi> {
    let number = |index: u16| get(index).and_then(|value| value.trim().parse::<i32>().ok()).unwrap_or(0);
    let started = number(CS_VOTE_TIME);
    if started == 0 {
        return None;
    }
    let string = get(CS_VOTE_STRING).unwrap_or_default();
    let (label, param) = describe(&string);
    Some(VoteUi {
        seconds_left: ((VOTE_TIME_MS - (now_ms - started)) / 1000).max(0),
        label,
        param,
        yes: number(CS_VOTE_YES),
        no: number(CS_VOTE_NO),
    })
}

/// Only literal `map <name>` votes can be prepared without the server's
/// command interpreter. Display names like `map mp/ffa3 (Death Star)` are not
/// part of the BSP qpath. In particular never guess what `vstr nextmap` runs.
pub fn proposed_map(vote_string: &str) -> Option<String> {
    // jaPRO's stock map callvote includes an appended nextmap reset:
    // `map racepack8; set nextmap map_restart 0`. The semicolon belongs
    // to the console command separator, not the BSP qpath.
    let (first_command, suffix) = match vote_string.split_once(';') {
        Some((map, suffix)) => (map.trim(), Some(suffix.trim())),
        None => (vote_string.trim(), None),
    };
    // A jaPRO map vote appends exactly this harmless nextmap reset. Never
    // extrapolate its destination from an arbitrary compound server command.
    if let Some(suffix) = suffix {
        if !suffix.eq_ignore_ascii_case("set nextmap map_restart 0")
            && !suffix.trim_end_matches([')', ',', '"', ' ', '\n']).eq_ignore_ascii_case("set nextmap map_restart 0")
        {
            return None;
        }
    }
    let mut words = first_command.split_ascii_whitespace();
    if !words.next()?.eq_ignore_ascii_case("map") { return None; }
    let name = words.next()?;
    // Ignore only the known optional display name `(Map Title)`; reject
    // additional commands/arguments in the first segment.
    if let Some(tail) = words.next() {
        if !tail.starts_with('(') { return None; }
    }
    let name = name.strip_prefix("maps/").unwrap_or(name);
    let name = name.strip_suffix(".bsp").unwrap_or(name);
    if name.is_empty() || name.len() >= 60 || name.starts_with('/')
        || name.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/'))
    { return None; }
    Some(name.to_owned())
}

/// Observe an incremental CS_VOTE_STRING (index 9) reliable command.
/// The protocol gamestate decoder's configstring map is only the initial
/// snapshot; later `cs` commands reach CGame separately. Return None for
/// unrelated commands, but Some((text, None)) when an unrelated vote begins,
/// so a previous map-vote candidate can be invalidated.
pub fn vote_map_cs9_update(command: &[u8]) -> Option<(String, Option<String>)> {
    let args = jka_protocol::commands::tokenize(command);
    if args.len() < 3 || !args[0].eq_ignore_ascii_case(b"cs")
        || args[1].as_slice() != b"9" { return None; }
    let text = String::from_utf8_lossy(&args[2]).into_owned();
    let target = proposed_map(&text);
    Some((text, target))
}

/// Match the *server* `print` command, never a player `chat`, so an ordinary
/// player cannot manufacture a successful map vote by saying "Vote passed".
/// Quake 3, OpenJK and TaystJK announce this before delayed execution.
pub fn is_passed_vote_print(command: &[u8]) -> bool {
    server_print_text(command).is_some_and(|text| {
        let lowercase = text.to_ascii_lowercase();
        // Don't interpret arbitrary server prints that merely quote the phrase.
        lowercase.starts_with("vote passed") || lowercase.starts_with("vote succeeded")
    })
}

/// A trusted, successful vote print often contains the command itself:
/// `print "Vote passed. (map racepack8; set nextmap map_restart 0), ..."`.
/// Unlike CS_VOTE_TIME, this survives even when the server clears the vote
/// configstring in the same packet. Never evaluate the command, only extract
/// its first, explicitly named map target.
pub fn passed_vote_map(command: &[u8]) -> Option<String> {
    let text = server_print_text(command)?;
    if !is_passed_vote_print(command) { return None; }

    // TaystJK g_main.c VotePassed prints:
    //   Vote passed. (map racepack6; set nextmap map_restart 0^7),
    //   command will be executed in 7 seconds.
    // The ^7 is the server's appended color reset, not part of the command.
    // Isolate the balanced (...) before checking the vote itself.
    let start = text.find('(')?;
    let mut depth = 0usize;
    let mut close = None;
    for (offset, ch) in text[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 { close = Some(start + offset); break; }
            }
            _ => {}
        }
    }
    let mut command_text = text.get(start + 1..close?)?.trim_end();
    if let Some(without_color_reset) = command_text.strip_suffix("^7") {
        command_text = without_color_reset.trim_end();
    }
    proposed_map(command_text)
}

fn server_print_text(command: &[u8]) -> Option<String> {
    // Match only a reliable server print, not chat or local console text.
    let raw = command.trim_ascii_start();
    let boundary = raw.iter().position(|b| b.is_ascii_whitespace())?;
    if !raw[..boundary].eq_ignore_ascii_case(b"print") { return None; }
    let payload = raw[boundary..].trim_ascii_start();
    let payload = payload.strip_prefix(b"\"").unwrap_or(payload);
    Some(String::from_utf8_lossy(payload).into_owned())
}

/// Game types the menu offers, `(number, label)`. Single player is not votable.
pub const GAME_TYPES: &[(u8, &str)] = &[
    (0, "Free For All"),
    (1, "Holocron FFA"),
    (2, "Jedi Master"),
    (3, "Duel"),
    (4, "Power Duel"),
    (6, "Team FFA"),
    (7, "Siege"),
    (8, "Capture the Flag"),
    (9, "Capture the Ysalamiri"),
];

/// The `callvote` line for a map, or `None` when the name is empty or unsafe.
pub fn map_vote(name: &str) -> Option<String> {
    let name = name.trim().trim_start_matches("maps/").trim_end_matches(".bsp");
    safe_argument(name).then(|| format!("callvote map {name}"))
}

pub fn gametype_vote(number: u8) -> String {
    format!("callvote g_gametype {number}")
}

pub fn clientkick_vote(client: usize) -> String {
    format!("callvote clientkick {client}")
}

pub fn forcespec_vote(client: usize) -> String {
    format!("callvote forcespec {client}")
}

pub fn limit_vote(kind: &str, value: u32) -> String {
    format!("callvote {kind} {value}")
}

/// jaPRO `poll`: free text, so quotes and command separators are rejected.
pub fn poll_vote(question: &str) -> Option<String> {
    let question = question.trim();
    (!question.is_empty() && !question.contains(['"', ';', '\n', '\r', '\\'])).then(|| format!("callvote poll {question}"))
}

/// Vote arguments are single console tokens on the wire.
fn safe_argument(value: &str) -> bool {
    !value.is_empty() && !value.contains(|c: char| c.is_whitespace() || matches!(c, '"' | ';' | '\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings<'a>(pairs: &'a [(u16, &'a str)]) -> impl Fn(u16) -> Option<String> + 'a {
        move |index| pairs.iter().find(|(i, _)| *i == index).map(|(_, v)| (*v).to_owned())
    }

    #[test]
    fn no_vote_when_time_is_zero_or_missing() {
        assert_eq!(read(strings(&[]), 5000), None);
        assert_eq!(read(strings(&[(CS_VOTE_TIME, "0"), (CS_VOTE_STRING, "map_restart")]), 5000), None);
    }

    #[test]
    fn countdown_and_counts() {
        let vote = read(
            strings(&[(8, "10000"), (9, "map_restart"), (10, "3"), (11, "1")]),
            22_500,
        )
        .unwrap();
        assert_eq!(vote.seconds_left, 17);
        assert_eq!((vote.yes, vote.no), (3, 1));
        assert_eq!(vote.label, "Restart Map");
        assert_eq!(vote.hud_line(), "^7Vote(17):<Restart Map^7> Yes:3 No:1");
    }

    #[test]
    fn countdown_never_goes_negative() {
        let vote = read(strings(&[(8, "1000"), (9, "map_restart")]), 999_999).unwrap();
        assert_eq!(vote.seconds_left, 0);
    }

    #[test]
    fn descriptions_match_cg_drawvote() {
        assert_eq!(describe("kick Padawan"), ("Kick Player".into(), Some("Padawan".into())));
        assert_eq!(describe("map mp/ffa3"), ("New Map".into(), Some("mp/ffa3".into())));
        assert_eq!(describe("g_gametype Duel"), ("Game Type".into(), Some("Duel".into())));
        assert_eq!(describe("vstr nextmap"), ("Next Map".into(), None));
        assert_eq!(describe("g_doWarmup 1"), ("Warmup".into(), None));
        assert_eq!(describe("poll Best map?"), ("poll Best map?".into(), None));
    }

    #[test]
    fn descriptions_preserve_unicode_arguments() {
        assert_eq!(describe("KiCk Padawän"), ("Kick Player".into(), Some("Padawän".into())));
        assert_eq!(describe("MaP mp/ärena"), ("New Map".into(), Some("mp/ärena".into())));
        assert_eq!(describe("G_GAMETYPE Duël"), ("Game Type".into(), Some("Duël".into())));
    }

    #[test]
    fn custom_votes_allow_unicode_at_every_prefix_boundary() {
        // Command checks must not slice through two-, three-, or four-byte characters.
        for character in ['ä', '界', '😀'] {
            for offset in 0..=11 {
                let text = format!("{}{character} vote?", "x".repeat(offset));
                assert_eq!(describe(&text), (text.clone(), None), "vote: {text:?}");
            }
        }
    }

    #[test]
    fn unicode_votes_reach_the_hud() {
        let vote = read(strings(&[(8, "1000"), (9, "kick Padawän"), (10, "2"), (11, "1")]), 2000).unwrap();
        assert_eq!(vote.hud_line(), "^7Vote(29):<Kick Player Padawän^7> Yes:2 No:1");

        let vote = read(strings(&[(8, "1000"), (9, "poll Ist ä gut?")]), 2000).unwrap();
        assert_eq!(vote.hud_line(), "^7Vote(29):<poll Ist ä gut?^7> Yes:0 No:0");
    }

    #[test]
    fn hud_line_includes_the_argument() {
        let vote = VoteUi { seconds_left: 9, label: "New Map".into(), param: Some("mp/ffa3".into()), yes: 0, no: 2 };
        assert_eq!(vote.hud_line(), "^7Vote(9):<New Map mp/ffa3^7> Yes:0 No:2");
    }

    #[test]
    fn reliable_cs9_vote_map_is_available_before_vote_passes() {
        let raw = b"cs 9 \"map Racepack8 (racepack8)\"\n";
        let (text, map) = vote_map_cs9_update(raw).expect("CS9 update");
        assert_eq!(text, "map Racepack8 (racepack8)");
        assert_eq!(map.as_deref(), Some("Racepack8"));
        assert_eq!(vote_map_cs9_update(b"cs 8 \"51225\"\n"), None);
        assert_eq!(vote_map_cs9_update(b"cs 8 \"\"\n"), None);
        assert_eq!(vote_map_cs9_update(b"print \"Vote passed.\""), None);
        assert_eq!(vote_map_cs9_update(b"cs 9 \"vstr nextmap\""), Some(("vstr nextmap".into(), None)));
    }

    #[test]
    fn prewarm_only_on_literal_map_vote_and_authoritative_print() {
        assert_eq!(proposed_map("map mp/ffa3 (The Pit)"), Some("mp/ffa3".into()));
        assert_eq!(proposed_map("MAP maps/mp/ffa3.bsp"), Some("mp/ffa3".into()));
        assert_eq!(proposed_map("map racepack8; set nextmap map_restart 0"), Some("racepack8".into()));
        assert_eq!(proposed_map("map racepack8 (Race Pack); set nextmap map_restart 0"), Some("racepack8".into()));
        for vote in ["map_restart", "vstr nextmap", "map ../bad", "map a;b", "map", "map foo\\bar"] {
            assert_eq!(proposed_map(vote), None, "{vote}");
        }
        assert!(is_passed_vote_print(b"print \"Vote passed.\n\""));
        assert!(!is_passed_vote_print(b"chat \"Vote passed.\""));
        assert!(!is_passed_vote_print(b"print \"Vote failed.\""));
        let passed = b"print \"Vote passed. (map racepack8; set nextmap map_restart 0), command will be executed in 60 seconds.\n\"";
        assert!(is_passed_vote_print(passed));
        assert_eq!(passed_vote_map(passed).as_deref(), Some("racepack8"));
        // Exact jaPRO `VotePassed()` reliable print, including ^7 inside the
        // vote parentheses and the delay sentence after the command.
        let japro = b"print \"Vote passed. (map racepack6; set nextmap map_restart 0^7), command will be executed in 7 seconds.\n\"";
        assert!(is_passed_vote_print(japro));
        assert_eq!(passed_vote_map(japro).as_deref(), Some("racepack6"));
        assert_eq!(passed_vote_map(b"print \"Vote passed. (map mp/ffa3 (The Pit)^7), command will be executed in 60 seconds.\"").as_deref(), Some("mp/ffa3"));
        assert_eq!(passed_vote_map(b"print \"Vote passed. (map racepack6; quit^7), command will be executed in 7 seconds.\""), None);
        assert_eq!(passed_vote_map(b"chat \"Vote passed. (map racepack6^7)\""), None);

        assert_eq!(passed_vote_map(b"print \"Vote passed. (vstr nextmap), command will be executed in 60 seconds.\""), None);
    }

    #[test]
    fn callvote_lines() {
        assert_eq!(map_vote(" maps/mp/ffa3.bsp ").as_deref(), Some("callvote map mp/ffa3"));
        assert_eq!(map_vote("bad name"), None);
        assert_eq!(map_vote("x;quit"), None);
        assert_eq!(map_vote(""), None);
        assert_eq!(gametype_vote(8), "callvote g_gametype 8");
        assert_eq!(clientkick_vote(12), "callvote clientkick 12");
        assert_eq!(forcespec_vote(3), "callvote forcespec 3");
        assert_eq!(limit_vote("timelimit", 20), "callvote timelimit 20");
        assert_eq!(poll_vote("Rematch?").as_deref(), Some("callvote poll Rematch?"));
        assert_eq!(poll_vote("a;b"), None);
        assert_eq!(poll_vote("   "), None);
    }
}
