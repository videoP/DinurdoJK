//! Kill feed state. Obituaries arrive through `CgameNotice::Obituary`; each
//! one is kept until its `cg_killfeed*Time` runs out and aged against demo
//! time during playback, like chat, so pausing a demo freezes the feed.
use crate::app::{App, Duration, Instant};
use crate::ui::{UiKillfeed, UiKillfeedEntry};
use std::collections::VecDeque;

pub(in crate::app) struct Record {
    created: Instant,
    demo_elapsed_ms: Option<f64>,
    attacker: Option<(i32, String, i32)>,
    target: (i32, String, i32),
    mod_: i32,
}

#[derive(Default)]
pub(in crate::app) struct Feed {
    records: VecDeque<Record>,
    last_refresh: Option<Instant>,
}

/// More lines than `cg_killfeedLines` can ever show; older ones are dropped.
const KEEP: usize = 24;

impl App {
    pub(in crate::app) fn push_killfeed(
        &mut self,
        target: (i32, String),
        attacker: Option<(i32, String)>,
        mod_: i32,
    ) {
        let now = Instant::now();
        let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
        let team_of = |app: &App, client: i32| {
            let session = app.game_session.as_ref()?;
            let info = session.client_game.client_info(usize::try_from(client).ok()?, &session.siege_classes)?;
            Some(info.team)
        };
        let target_team = team_of(self, target.0).unwrap_or(0);
        let attacker = attacker.map(|(client, name)| (client, name, team_of(self, client).unwrap_or(0)));
        let feed = &mut self.killfeed;
        feed.records.push_back(Record {
            created: now,
            demo_elapsed_ms,
            attacker,
            target: (target.0, target.1, target_team),
            mod_,
        });
        while feed.records.len() > KEEP {
            feed.records.pop_front();
        }
        self.publish_transient_ui();
    }

    /// Three representative lines so HUD Edit can place and size the feed
    /// before anyone has died.
    pub(in crate::app) fn killfeed_sample_ui(&self) -> Option<UiKillfeed> {
        let settings = self.killfeed_settings;
        let viewer = self.network.name.clone();
        let line = |attacker: Option<&str>, target: &str, mod_, own_kill, own_death, team| UiKillfeedEntry {
            attacker: attacker.map(str::to_owned),
            attacker_team: team,
            target: target.to_owned(),
            target_team: 3 - team,
            mod_,
            own_kill,
            own_death,
            alpha: settings.timing(own_kill, own_death).1,
            enter: 1.0,
        };
        Some(UiKillfeed {
            style: settings.style,
            highlight_own: settings.highlight_own,
            entries: vec![
                line(Some(&viewer), "Tavion", 3, true, false, 1),
                line(Some("Rosh"), "Kyle", 19, false, false, 2),
                line(Some("Alora"), &viewer, 8, false, true, 2),
                line(None, "Desann", 38, false, false, 1),
            ],
        })
    }

    fn killfeed_age(&self, record: &Record, now: Instant, demo_now: Option<f64>) -> f32 {
        match (demo_now, record.demo_elapsed_ms) {
            (Some(now_ms), Some(created_ms)) => ((now_ms - created_ms).max(0.0) / 1000.0) as f32,
            _ => now.saturating_duration_since(record.created).as_secs_f32(),
        }
    }

    pub(in crate::app) fn killfeed_ui(&self) -> Option<UiKillfeed> {
        let settings = self.killfeed_settings;
        if !settings.enabled || self.killfeed.records.is_empty() {
            return None;
        }
        let now = Instant::now();
        let demo_now = self.current_demo_elapsed_ms(now);
        let viewer = self.scoreboard_focus_client();
        let entries: Vec<UiKillfeedEntry> = self
            .killfeed
            .records
            .iter()
            .rev()
            .filter_map(|record| {
                let own_kill = record.attacker.as_ref().is_some_and(|(client, ..)| Some(*client) == viewer)
                    && Some(record.target.0) != viewer;
                let own_death = Some(record.target.0) == viewer;
                let age = self.killfeed_age(record, now, demo_now);
                let alpha = settings.alpha_at(age, own_kill, own_death)?;
                Some(UiKillfeedEntry {
                    attacker: record.attacker.as_ref().map(|(_, name, _)| name.clone()),
                    attacker_team: record.attacker.as_ref().map_or(0, |(_, _, team)| *team),
                    target: record.target.1.clone(),
                    target_team: record.target.2,
                    mod_: record.mod_,
                    own_kill,
                    own_death,
                    alpha,
                    enter: (age / 0.18).clamp(0.0, 1.0),
                })
            })
            .take(usize::from(settings.max_lines.max(1)))
            .collect();
        (!entries.is_empty()).then_some(UiKillfeed {
            style: settings.style,
            highlight_own: settings.highlight_own,
            entries,
        })
    }

    /// Drop expired lines and report whether the feed needs a repaint: every
    /// 33 ms while any line is fading or sliding, otherwise only on expiry.
    pub(in crate::app) fn tick_killfeed(&mut self, now: Instant) -> bool {
        if self.killfeed.records.is_empty() {
            return false;
        }
        let demo_now = self.current_demo_elapsed_ms(now);
        let settings = self.killfeed_settings;
        let longest = settings.own_kill_time.max(settings.own_death_time).max(settings.other_time);
        let before = self.killfeed.records.len();
        let ages: Vec<f32> = self.killfeed.records.iter().map(|r| self.killfeed_age(r, now, demo_now)).collect();
        let mut index = 0;
        self.killfeed.records.retain(|_| {
            let keep = ages[index] < longest;
            index += 1;
            keep
        });
        let lifetimes = [settings.own_kill_time, settings.own_death_time, settings.other_time];
        let animating = ages
            .iter()
            .any(|&age| age < 0.2 || lifetimes.iter().any(|&life| age > life - 0.6 && age < life));
        let due = self
            .killfeed
            .last_refresh
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_millis(33));
        let changed = self.killfeed.records.len() != before;
        if (animating && due) || changed {
            self.killfeed.last_refresh = Some(now);
            return true;
        }
        false
    }
}
