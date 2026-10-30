//! Personal match statistics for the scoreboard: head-to-head against every
//! player and the end-of-game breakdown.
//!
//! Only what this client can measure reliably is kept. Kills and deaths come
//! from EV_OBITUARY (seen for every player, so team death totals are exact);
//! damage taken, Force spent, airborne kills and time-to-kill come from our
//! own playerState and our own view, never from guesses about other players.
//! Everything resets on `map_restart` and on a new map.
use crate::app::session::sampling::playerstate_vec3;
use crate::app::{App, Instant};
use crate::ui::{UiMatchStats, UiMatchup};

const MAX_CLIENTS: usize = 32;
const ENTITYNUM_NONE: i32 = 1023;
const ENTITYNUM_WORLD: i32 = 1022;
const ET_PLAYER: i32 = 1;
const EF_DEAD: i32 = 1 << 1;
/// Lose sight of someone for this long and their "first seen" clock restarts.
const SIGHT_MEMORY_SECS: f32 = 3.0;

#[derive(Clone, Default)]
struct Opponent {
    name: String,
    team: i32,
    kills: u16,
    deaths: u16,
    damage_taken: i32,
    /// When they came into view (and stayed there, give or take a blink).
    seen_since: Option<Instant>,
    last_seen: Option<Instant>,
}

#[derive(Default)]
pub(in crate::app) struct MatchStats {
    map: String,
    opponents: Vec<Opponent>,
    kills: u32,
    deaths: u32,
    suicides: u32,
    aerial_kills: u32,
    saber_kills: u32,
    streak: u32,
    best_streak: u32,
    hits: u32,
    damage_taken: i32,
    force_spent: i64,
    force_at_last_kill: i64,
    force_per_kill: Vec<i32>,
    ttk_ms: Vec<u32>,
    team_kills: [u32; 4],
    team_deaths: [u32; 4],
    last_health: Option<(i32, i32, i32)>,
    last_force: Option<i32>,
    last_hits: Option<i32>,
    last_scan: Option<Instant>,
}

impl MatchStats {
    fn opponent(&mut self, client: i32) -> Option<&mut Opponent> {
        let index = usize::try_from(client).ok().filter(|&c| c < MAX_CLIENTS)?;
        if self.opponents.len() < MAX_CLIENTS {
            self.opponents.resize(MAX_CLIENTS, Opponent::default());
        }
        self.opponents.get_mut(index)
    }

    fn reset(&mut self, map: String) {
        *self = Self { map, ..Self::default() };
    }

    /// `kills`/`deaths` against `client` as (you killed them, they killed you).
    pub(in crate::app) fn head_to_head(&self, client: i32) -> Option<(u16, u16)> {
        let opponent = self.opponents.get(usize::try_from(client).ok()?)?;
        (opponent.kills > 0 || opponent.deaths > 0).then_some((opponent.kills, opponent.deaths))
    }
}

fn average(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0u32), |(sum, count), value| (sum + value, count + 1));
    (count > 0).then(|| sum / f64::from(count))
}

impl App {
    fn stats_viewer(&self) -> Option<i32> {
        self.game_session.as_ref()?.client_game.presentation_client_num()
    }

    fn stats_team(&self, client: i32) -> i32 {
        self.game_session
            .as_ref()
            .and_then(|session| {
                session.client_game.client_info(usize::try_from(client).ok()?, &session.siege_classes)
            })
            .map_or(0, |info| info.team)
    }

    /// `map_restart`: every count starts again from zero.
    pub(in crate::app) fn reset_match_stats(&mut self) {
        let map = std::mem::take(&mut self.match_stats.map);
        self.match_stats.reset(map);
    }

    /// One EV_OBITUARY, already resolved to client numbers.
    pub(in crate::app) fn note_match_obituary(&mut self, target: i32, attacker: Option<i32>, mod_: i32) {
        let Some(me) = self.stats_viewer() else { return };
        let target_team = self.stats_team(target);
        let attacker_team = attacker.map(|client| self.stats_team(client));
        let target_name = self.game_session.as_ref()
            .and_then(|session| session.client_game.client_name(usize::try_from(target).ok()?))
            .unwrap_or_default();
        let airborne = self.current_player_state()
            .and_then(|ps| ps.field_i32("groundEntityNum"))
            .is_some_and(|ground| ground == ENTITYNUM_NONE);
        let stats = &mut self.match_stats;
        stats.team_deaths[target_team.clamp(0, 3) as usize] += 1;
        if let (Some(attacker), Some(team)) = (attacker, attacker_team) {
            if attacker != target {
                stats.team_kills[team.clamp(0, 3) as usize] += 1;
            }
        }
        let now = Instant::now();
        if attacker == Some(me) && target != me {
            stats.kills += 1;
            stats.streak += 1;
            stats.best_streak = stats.best_streak.max(stats.streak);
            if airborne {
                stats.aerial_kills += 1;
            }
            if mod_ == 3 {
                stats.saber_kills += 1;
            }
            let spent = stats.force_spent - stats.force_at_last_kill;
            stats.force_at_last_kill = stats.force_spent;
            stats.force_per_kill.push(spent.clamp(0, i64::from(i32::MAX)) as i32);
            let mut seen_since = None;
            if let Some(opponent) = stats.opponent(target) {
                opponent.kills += 1;
                opponent.name = target_name;
                opponent.team = target_team;
                seen_since = opponent.seen_since.take();
            }
            if let Some(since) = seen_since {
                let ms = now.saturating_duration_since(since).as_millis();
                stats.ttk_ms.push(ms.min(u128::from(u32::MAX)) as u32);
            }
        } else if target == me {
            stats.deaths += 1;
            stats.streak = 0;
            match attacker {
                Some(attacker) if attacker != me => {
                    let name = self.game_session.as_ref()
                        .and_then(|session| session.client_game.client_name(usize::try_from(attacker).ok()?))
                        .unwrap_or_default();
                    if let Some(opponent) = self.match_stats.opponent(attacker) {
                        opponent.deaths += 1;
                        opponent.name = name;
                        opponent.team = attacker_team.unwrap_or(0);
                    }
                }
                _ => self.match_stats.suicides += 1,
            }
        }
    }

    /// Per-tick sampling of our own playerState: damage taken (attributed to
    /// PERS_ATTACKER), Force spent, hits landed, and who is in view.
    pub(in crate::app) fn tick_match_stats(&mut self, now: Instant) {
        let Some(session) = self.game_session.as_ref() else { return };
        let map = session.map_name.clone().unwrap_or_default();
        if map != self.match_stats.map {
            self.match_stats.reset(map);
        }
        let Some(ps) = self.current_player_state() else { return };
        let health = ps.stats[0];
        let armor = ps.stats[5];
        let spawn = ps.persistant[4];
        let attacker = ps.persistant[6];
        let force = ps.field_i32("fd.forcePower").unwrap_or(0);
        let hits = ps.persistant[1];
        let origin = playerstate_vec3(ps, "origin");
        let angles = playerstate_vec3(ps, "viewangles");
        let me = self.stats_viewer();

        let stats = &mut self.match_stats;
        if let Some((last_health, last_armor, last_spawn)) = stats.last_health {
            let lost = (last_health - health).max(0) + (last_armor - armor).max(0);
            if spawn == last_spawn && health > 0 && lost > 0 {
                stats.damage_taken += lost;
                if Some(attacker) != me {
                    if let Some(opponent) = stats.opponent(attacker) {
                        opponent.damage_taken += lost;
                    }
                }
            }
        }
        stats.last_health = Some((health, armor, spawn));
        if let Some(last) = stats.last_force {
            stats.force_spent += i64::from((last - force).max(0));
        }
        stats.last_force = Some(force);
        if let Some(last) = stats.last_hits {
            stats.hits += (hits - last).max(0) as u32;
        }
        stats.last_hits = Some(hits);

        // Sight: 10 Hz is plenty for a "time from first seen" clock.
        if stats.last_scan.is_some_and(|last| now.saturating_duration_since(last).as_secs_f32() < 0.1) {
            return;
        }
        stats.last_scan = Some(now);
        let (Some(origin), Some(angles), Some(me)) = (origin, angles, me) else { return };
        let eye = [origin[0], origin[1], origin[2] + 36.0];
        let (yaw, pitch) = (angles[1].to_radians(), angles[0].to_radians());
        let forward = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()];
        let candidates: Vec<(i32, [f32; 3])> = session
            .presented_entities
            .iter()
            .filter(|entity| {
                entity.entity_type == ET_PLAYER
                    && i32::from(entity.number) != me
                    && usize::from(entity.number) < MAX_CLIENTS
                    && entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0
            })
            .filter_map(|entity| {
                let target = [entity.origin[0], entity.origin[1], entity.origin[2] + 24.0];
                let delta = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
                let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                let facing = (delta[0] * forward[0] + delta[1] * forward[1] + delta[2] * forward[2]) / length.max(1.0);
                // Inside a ~110 degree cone and within a sensible range.
                (length < 6000.0 && facing > 0.57).then_some((i32::from(entity.number), target))
            })
            .collect();
        let solids = crate::net::solid_entities(&session.presented_entities, 0);
        let mut visible = [false; MAX_CLIENTS];
        if let Some(world) = self.map_collision.as_mut() {
            use jka_movement::TraceWorld;
            let mut prediction_world = crate::net::PredictionWorld { world, solids: &solids, client_num: me };
            for (client, target) in candidates {
                let trace = prediction_world.trace(jka_movement::TraceQuery {
                    start: eye,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    end: target,
                    pass_entity: me,
                    mask: 0x1,
                });
                visible[client as usize] = trace.entity != ENTITYNUM_WORLD;
            }
        }
        let stats = &mut self.match_stats;
        for client in 0..MAX_CLIENTS as i32 {
            let Some(opponent) = stats.opponent(client) else { continue };
            if visible[client as usize] {
                if opponent.seen_since.is_none() {
                    opponent.seen_since = Some(now);
                }
                opponent.last_seen = Some(now);
            } else if opponent
                .last_seen
                .is_none_or(|last| now.saturating_duration_since(last).as_secs_f32() > SIGHT_MEMORY_SECS)
            {
                opponent.seen_since = None;
            }
        }
    }

    pub(in crate::app) fn match_stats_ui(&self) -> UiMatchStats {
        let stats = &self.match_stats;
        let mut matchups: Vec<UiMatchup> = stats
            .opponents
            .iter()
            .filter(|o| o.kills > 0 || o.deaths > 0 || o.damage_taken > 0)
            .map(|o| UiMatchup {
                name: o.name.clone(),
                team: o.team,
                kills: o.kills,
                deaths: o.deaths,
                damage_taken: o.damage_taken.max(0) as u32,
            })
            .collect();
        matchups.sort_by_key(|m| std::cmp::Reverse((i32::from(m.kills) + i32::from(m.deaths), m.kills)));
        let net = |m: &UiMatchup| i32::from(m.kills) - i32::from(m.deaths);
        let best = matchups.iter().filter(|m| m.kills > 0).max_by_key(|m| (net(m), m.kills)).map(|m| m.name.clone());
        let nemesis = matchups.iter().filter(|m| m.deaths > 0).max_by_key(|m| (m.deaths, -net(m))).map(|m| m.name.clone());
        UiMatchStats {
            kills: stats.kills,
            deaths: stats.deaths,
            suicides: stats.suicides,
            aerial_kills: stats.aerial_kills,
            saber_kills: stats.saber_kills,
            best_streak: stats.best_streak,
            hits: stats.hits,
            damage_taken: stats.damage_taken.max(0) as u32,
            avg_force_per_kill: average(stats.force_per_kill.iter().map(|&v| f64::from(v))).map(|v| v.round() as u32),
            avg_ttk_ms: average(stats.ttk_ms.iter().map(|&v| f64::from(v))).map(|v| v.round() as u32),
            avg_damage_per_death: (stats.deaths > 0).then(|| stats.damage_taken.max(0) as u32 / stats.deaths),
            team_kills: [stats.team_kills[1], stats.team_kills[2]],
            team_deaths: [stats.team_deaths[1], stats.team_deaths[2]],
            best,
            nemesis,
            matchups,
            // Filled from our own `scores` row by the scoreboard.
            accuracy: None,
            excellent: 0,
            gauntlet: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_to_head_counts_both_directions() {
        let mut stats = MatchStats::default();
        stats.opponent(4).unwrap().kills = 3;
        stats.opponent(4).unwrap().deaths = 2;
        assert_eq!(stats.head_to_head(4), Some((3, 2)));
        assert_eq!(stats.head_to_head(5), None);
        stats.reset("mp/ffa3".into());
        assert_eq!(stats.head_to_head(4), None);
        assert_eq!(stats.map, "mp/ffa3");
    }

    #[test]
    fn average_of_nothing_is_none() {
        assert_eq!(average(std::iter::empty()), None);
        assert_eq!(average([2.0, 4.0].into_iter()), Some(3.0));
    }
}
