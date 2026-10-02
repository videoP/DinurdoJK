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
        vote_string.len() >= prefix.len() && vote_string[..prefix.len()].eq_ignore_ascii_case(prefix)
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
    fn hud_line_includes_the_argument() {
        let vote = VoteUi { seconds_left: 9, label: "New Map".into(), param: Some("mp/ffa3".into()), yes: 0, no: 2 };
        assert_eq!(vote.hud_line(), "^7Vote(9):<New Map mp/ffa3^7> Yes:0 No:2");
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
