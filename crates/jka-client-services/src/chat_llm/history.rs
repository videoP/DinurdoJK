//! What `chatQRD` works from: the chat of this session with wall-clock
//! times, what the filter did to each line, and when the player last touched
//! the game.

use super::SummaryPass;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Messages kept for summaries; a busy server writes a few hundred an hour.
const KEEP: usize = 4000;
/// Transcript budget in characters, sized for a 4096-token context.
pub const TRANSCRIPT_CHARS: usize = 7000;
/// No input for this long counts as being away.
pub const AWAY_AFTER: Duration = Duration::from_secs(60);
/// How long after coming back the absence is still "the last time I was away".
const AWAY_REMEMBERED: Duration = Duration::from_secs(15 * 60);
/// Longest stretch a summary may cover.
pub const MAX_MINUTES: f32 = 24.0 * 60.0;
/// Used when no time is given and the player was not away.
pub const FALLBACK_MINUTES: f32 = 10.0;

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    pub at: Instant,
    /// Local wall clock, hour and minute.
    pub clock: [u8; 2],
    pub sender: String,
    pub team: bool,
    /// The message as written.
    pub text: String,
    /// The filter's version, when it changed the message.
    pub filtered: Option<String>,
}

#[derive(Default)]
pub struct History {
    entries: VecDeque<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcript {
    pub text: String,
    pub messages: usize,
    /// Older messages left out to fit the model's context.
    pub omitted: usize,
}

fn hhmm(clock: [u8; 2]) -> String {
    format!("{:02}:{:02}", clock[0], clock[1])
}

impl History {
    pub fn push(&mut self, entry: Entry) {
        if self.entries.len() >= KEEP {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record what the filter made of message `id`.
    pub fn set_filtered(&mut self, id: u64, filtered: String) {
        // Verdicts arrive within seconds: the message is near the end.
        if let Some(entry) = self.entries.iter_mut().rev().take(256).find(|entry| entry.id == id) {
            entry.filtered = Some(filtered);
        }
    }

    fn since(&self, since: Instant) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |entry| entry.at >= since)
    }

    /// The last `count` messages before `id`, as "name: text" with filtered
    /// lines in their filtered form (context for the filter itself).
    pub fn before(&self, id: u64, count: usize) -> Vec<String> {
        let mut lines: Vec<String> = self
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.id < id)
            .take(count)
            .map(|entry| format!("{}: {}", entry.sender, entry.filtered.as_deref().unwrap_or(&entry.text)))
            .collect();
        lines.reverse();
        lines
    }

    /// The chat since `since` for the model. `Truth` shows messages as
    /// written; `Cleanse` shows the filter's version, marked `[filtered]`.
    /// The newest messages win when the budget is too small for all of them.
    pub fn transcript(&self, since: Instant, pass: SummaryPass, max_chars: usize) -> Transcript {
        let mut lines = Vec::new();
        let mut used = 0usize;
        let mut total = 0usize;
        for entry in self.since(since).collect::<Vec<_>>().into_iter().rev() {
            total += 1;
            if used > max_chars {
                continue;
            }
            let channel = if entry.team { " (team)" } else { "" };
            let body = match (pass, &entry.filtered) {
                (SummaryPass::Cleanse, Some(filtered)) => format!("[filtered] {filtered}"),
                _ => entry.text.clone(),
            };
            let line = format!("[{}] {}{channel}: {body}", hhmm(entry.clock), entry.sender);
            used += line.chars().count() + 1;
            if used <= max_chars || lines.is_empty() {
                lines.push(line);
            }
        }
        lines.reverse();
        Transcript { messages: lines.len(), omitted: total - lines.len(), text: lines.join("\n") }
    }

    /// One line per stretch of filtered messages since `since`: these are
    /// facts the client knows, stated whatever the model writes.
    pub fn disclaimers(&self, since: Instant) -> Vec<String> {
        const SAME_STRETCH: Duration = Duration::from_secs(120);
        struct Stretch<'a> {
            first: &'a Entry,
            last: &'a Entry,
            count: usize,
            senders: Vec<&'a str>,
        }
        let mut stretches: Vec<Stretch<'_>> = Vec::new();
        for entry in self.since(since).filter(|entry| entry.filtered.is_some()) {
            match stretches.last_mut() {
                Some(stretch) if entry.at.saturating_duration_since(stretch.last.at) <= SAME_STRETCH => {
                    stretch.last = entry;
                    stretch.count += 1;
                    if !stretch.senders.contains(&entry.sender.as_str()) {
                        stretch.senders.push(&entry.sender);
                    }
                }
                _ => stretches.push(Stretch { first: entry, last: entry, count: 1, senders: vec![&entry.sender] }),
            }
        }
        stretches
            .iter()
            .map(|stretch| {
                let mut names = stretch.senders.iter().take(4).copied().collect::<Vec<_>>().join(", ");
                if stretch.senders.len() > 4 {
                    names.push_str(&format!(" and {} more", stretch.senders.len() - 4));
                }
                format!(
                    "DISCLAIMER {}-{}: {} message{} from {names} contained offensive language and {} filtered.",
                    hhmm(stretch.first.clock),
                    hhmm(stretch.last.clock),
                    stretch.count,
                    if stretch.count == 1 { "" } else { "s" },
                    if stretch.count == 1 { "was" } else { "were" },
                )
            })
            .collect()
    }
}

/// When the player last touched the game. Any key, button, wheel or mouse
/// movement the game window receives counts; an unfocused window gets none.
#[derive(Default)]
pub struct Away {
    last_input: Option<Instant>,
    /// The most recent absence: last input before it, first input after it.
    gap: Option<(Instant, Instant)>,
}

impl Away {
    pub fn input(&mut self, now: Instant) {
        if let Some(last) = self.last_input {
            if now.saturating_duration_since(last) >= AWAY_AFTER {
                self.gap = Some((last, now));
            }
        }
        self.last_input = Some(now);
    }

    /// Start of the absence a summary should cover: the one still going on,
    /// or the one the player just came back from.
    pub fn since(&self, now: Instant) -> Option<Instant> {
        if let Some(last) = self.last_input.filter(|last| now.saturating_duration_since(*last) >= AWAY_AFTER) {
            return Some(last);
        }
        self.gap
            .filter(|(_, returned)| now.saturating_duration_since(*returned) <= AWAY_REMEMBERED)
            .map(|(left, _)| left)
    }
}

/// `chatQRD [minutes] [truth|cleanse]`, in either order.
pub fn parse_args(args: &[&str]) -> Result<(Option<f32>, SummaryPass), String> {
    const USAGE: &str = "usage: chatQRD [minutes] [truth|cleanse]";
    let mut minutes = None;
    let mut pass = None;
    for word in args {
        if let Some(found) = SummaryPass::from_word(word) {
            if pass.replace(found).is_some() {
                return Err(USAGE.to_owned());
            }
        } else if let Ok(value) = word.trim_end_matches(['m', 'M']).parse::<f32>() {
            if !value.is_finite() || value <= 0.0 || minutes.replace(value.min(MAX_MINUTES)).is_some() {
                return Err(USAGE.to_owned());
            }
        } else {
            return Err(USAGE.to_owned());
        }
    }
    Ok((minutes, pass.unwrap_or(SummaryPass::Truth)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, at: Instant, minute: u8, sender: &str, text: &str) -> Entry {
        Entry { id, at, clock: [14, minute], sender: sender.to_owned(), team: false, text: text.to_owned(), filtered: None }
    }

    #[test]
    fn arguments_come_in_either_order_and_default_to_truth() {
        assert_eq!(parse_args(&[]).unwrap(), (None, SummaryPass::Truth));
        assert_eq!(parse_args(&["15"]).unwrap(), (Some(15.0), SummaryPass::Truth));
        assert_eq!(parse_args(&["cleanse", "2.5"]).unwrap(), (Some(2.5), SummaryPass::Cleanse));
        assert_eq!(parse_args(&["30m", "truth"]).unwrap(), (Some(30.0), SummaryPass::Truth));
        assert_eq!(parse_args(&["999999"]).unwrap().0, Some(MAX_MINUTES));
        assert!(parse_args(&["0"]).is_err());
        assert!(parse_args(&["-3"]).is_err());
        assert!(parse_args(&["5", "6"]).is_err());
        assert!(parse_args(&["soon"]).is_err());
    }

    #[test]
    fn an_absence_is_remembered_after_coming_back() {
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let mut away = Away::default();
        assert_eq!(away.since(at(0)), None);
        away.input(at(0));
        away.input(at(20));
        // Playing: not away.
        assert_eq!(away.since(at(30)), None);
        // Still away, asked by a script or bind from elsewhere.
        assert_eq!(away.since(at(20 + 300)), Some(at(20)));
        // Back after five minutes; typing the command counts as input.
        away.input(at(320));
        away.input(at(325));
        assert_eq!(away.since(at(330)), Some(at(20)));
        // After a long stretch of play, that absence is no longer what
        // "while I was away" means.
        let mut seconds = 325;
        while seconds < 325 + 16 * 60 {
            seconds += 30;
            away.input(at(seconds));
        }
        assert_eq!(away.since(at(seconds + 1)), None);
    }

    #[test]
    fn transcript_shows_truth_or_the_filtered_version() {
        let start = Instant::now();
        let mut history = History::default();
        history.push(entry(1, start, 1, "Luke", "hello there"));
        history.push(entry(2, start + Duration::from_secs(60), 2, "Vader", "you absolute muppet"));
        history.push(Entry { team: true, ..entry(3, start + Duration::from_secs(90), 2, "Leia", "regroup") });
        history.set_filtered(2, "you absolute ***".to_owned());
        let truth = history.transcript(start, SummaryPass::Truth, 1000);
        assert_eq!(truth.text, "[14:01] Luke: hello there\n[14:02] Vader: you absolute muppet\n[14:02] Leia (team): regroup");
        assert_eq!((truth.messages, truth.omitted), (3, 0));
        let cleanse = history.transcript(start + Duration::from_secs(30), SummaryPass::Cleanse, 1000);
        assert_eq!(cleanse.text, "[14:02] Vader: [filtered] you absolute ***\n[14:02] Leia (team): regroup");
        // A tight budget keeps the newest lines.
        let tight = history.transcript(start, SummaryPass::Truth, 30);
        assert_eq!(tight.text, "[14:02] Leia (team): regroup");
        assert_eq!((tight.messages, tight.omitted), (1, 2));
        assert_eq!(history.before(3, 5), ["Luke: hello there", "Vader: you absolute ***"]);
    }

    #[test]
    fn disclaimers_group_filtered_messages_into_stretches() {
        let start = Instant::now();
        let mut history = History::default();
        for (id, seconds, minute, sender) in [(1, 0, 2, "Vader"), (2, 50, 2, "Maul"), (3, 100, 3, "Vader"), (4, 900, 17, "Dooku")] {
            history.push(entry(id, start + Duration::from_secs(seconds), minute, sender, "x"));
            history.set_filtered(id, "***".to_owned());
        }
        history.push(entry(5, start + Duration::from_secs(950), 18, "Yoda", "calm"));
        assert_eq!(
            history.disclaimers(start),
            [
                "DISCLAIMER 14:02-14:03: 3 messages from Vader, Maul contained offensive language and were filtered.",
                "DISCLAIMER 14:17-14:17: 1 message from Dooku contained offensive language and was filtered.",
            ]
        );
        assert!(history.disclaimers(start + Duration::from_secs(920)).is_empty());
    }
}
