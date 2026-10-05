//! Session events.
use crate::app::{
    event_debug, event_workers, DemoEventStat, Duration, EntityEvent, EventDispatchClass,
    GameSession, Instant,
};

impl GameSession {
    pub(in crate::app) fn ridden_vehicle_definition(
        &self,
        vehicle_num: i32,
    ) -> Option<jka_assets::vehicle::VehicleDefinition> {
        if vehicle_num <= 0 {
            return None;
        }
        let entity = self
            .presented_entities
            .iter()
            .find(|entity| i32::from(entity.number) == vehicle_num)?;
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        let requested = self.client_game.model_qpath(model_index)?;
        self.player_presenter
            .vehicle_definition_for_model_request(&requested)
            .cloned()
    }

    pub(in crate::app) fn take_event_debug_lines(&mut self) -> Vec<String> {
        self.event_debug_lines.drain(..).collect()
    }

    pub(in crate::app) fn record_event_stat(
        &mut self,
        event: EntityEvent,
        server_time: i32,
        class: EventDispatchClass,
    ) {
        let stat = self.event_stats.entry(event).or_insert(DemoEventStat {
            count: 0,
            handled: 0,
            partial: 0,
            unhandled: 0,
            first_server_time: server_time,
            last_server_time: server_time,
        });
        stat.count = stat.count.saturating_add(1);
        match class {
            EventDispatchClass::Handled => stat.handled = stat.handled.saturating_add(1),
            EventDispatchClass::Partial => stat.partial = stat.partial.saturating_add(1),
            EventDispatchClass::Unhandled => stat.unhandled = stat.unhandled.saturating_add(1),
        }
        stat.last_server_time = server_time;
    }

    /// CG_EntityEvent EV_NOAMMO for the viewer: the follow-up weapon change is
    /// applied by the owner of the local input (see `take_out_of_ammo`).
    pub(in crate::app) fn note_out_of_ammo(&mut self, event: &crate::cgame::PresentationEvent) {
        if event.event != EntityEvent::EV_NOAMMO {
            return;
        }
        let Some(ps) = self.client_game.presentation_player_state() else {
            return;
        };
        let local = ps.field_i32("clientNum").unwrap_or(-1);
        let duel = ps.field_i32("duelInProgress").unwrap_or(0) != 0;
        let duel_index = ps.field_i32("duelIndex").unwrap_or(-1);
        let number = i32::from(event.entity_num);
        if number != local
            || (duel && local != number && duel_index != number)
            || ps.field_i32("m_iVehicleNum").unwrap_or(0) != 0
        {
            return;
        }
        const WP_NONE: i32 = 0;
        const WP_SABER: i32 = 3;
        const WP_NUM_WEAPONS: i32 = 19;
        let weapon = ps.field_i32("weapon").unwrap_or(WP_NONE);
        if weapon == WP_NONE || weapon == WP_SABER {
            return; // vehicle/force-HUD flash cases: no weapon change
        }
        // eventParm < WP_NUM_WEAPONS names the weapon that ran dry (its
        // STAT_WEAPONS bit is dropped) and the change leaves the current one.
        let (old, dry) = match event.parm {
            0 => (0, 0),
            parm if parm < WP_NUM_WEAPONS => (weapon, parm),
            parm => (parm - WP_NUM_WEAPONS, 0),
        };
        if self.pending_out_of_ammo.len() < 8 {
            self.pending_out_of_ammo.push((old, dry));
        }
    }

    /// The text a CG_EntityEvent prints or centre-prints through StringEd:
    /// duel start, item use failures and CTF flag messages.
    pub(in crate::app) fn note_stringed_messages(
        &mut self,
        event: &crate::cgame::PresentationEvent,
    ) {
        use crate::cgame::CgameNotice;
        use EntityEvent as E;
        let local = self.client_game.presentation_client_num().unwrap_or(-1);
        let state = &event.state;
        let notice = match event.event {
            E::EV_GLOBAL_DUEL => {
                let involved = ["otherEntityNum", "otherEntityNum2", "groundEntityNum"]
                    .iter()
                    .any(|field| state.field_i32(field) == Some(local));
                involved.then_some(CgameNotice::StringEd {
                    key: "MP_SVGAME_BEGIN_DUEL",
                    player: None,
                    team: None,
                    center: true,
                })
            }
            E::EV_PRIVATE_DUEL if !self.client_game.japro_racemode() => {
                // jaPRO cg_duelSounds: 2 is sound only, 0 disables the start cue.
                let option = self
                    .sound_presenter
                    .as_ref()
                    .map_or(1, |sound| sound.game_sounds().duel);
                (i32::from(event.entity_num) == local && event.parm == 2 && matches!(option, 1 | 3))
                    .then_some(CgameNotice::StringEd {
                        key: "MP_SVGAME_BEGIN_DUEL",
                        player: None,
                        team: None,
                        center: true,
                    })
            }
            E::EV_ITEMUSEFAIL if i32::from(event.entity_num) == local => {
                let key = match event.parm {
                    1 => "MP_INGAME_SENTRY_NOROOM",
                    2 => "MP_INGAME_SENTRY_ALREADYPLACED",
                    3 => "MP_INGAME_SHIELD_NOROOM",
                    4 => "MP_INGAME_SEEKER_ALREADYDEPLOYED",
                    _ => return,
                };
                Some(CgameNotice::StringEd {
                    key,
                    player: None,
                    team: None,
                    center: false,
                })
            }
            E::EV_CTFMESSAGE => {
                let key = match event.parm {
                    0 => "MP_INGAME_FRAGGED_FLAG_CARRIER",
                    1 => "MP_INGAME_FLAG_RETURNED",
                    2 => "MP_INGAME_PLAYER_RETURNED_FLAG",
                    3 => "MP_INGAME_PLAYER_CAPTURED_FLAG",
                    4 => "MP_INGAME_PLAYER_GOT_FLAG",
                    _ => return,
                };
                let client = state.field_i32("trickedentindex").unwrap_or(-1);
                let team = state.field_i32("trickedentindex2").unwrap_or(-1);
                let Some(info) = usize::try_from(client)
                    .ok()
                    .filter(|&client| client < 32)
                    .and_then(|client| self.client_game.client_info(client, &self.siege_classes))
                else {
                    return;
                };
                let team = match team {
                    1 => Some("RED"),
                    2 => Some("BLUE"),
                    3 => Some("SPECTATOR"),
                    0 => Some("FREE"),
                    _ => None,
                };
                Some(CgameNotice::StringEd {
                    key,
                    player: Some(info.name),
                    team,
                    center: false,
                })
            }
            _ => None,
        };
        if let Some(notice) = notice {
            self.client_game.push_notice(notice);
        }
    }

    /// OpenJK CG_Obituary. The obituary line is a client-side consequence of
    /// EV_OBITUARY, which is why demo playback otherwise had kill effects/markers
    /// but no console death text.
    pub(in crate::app) fn note_obituary(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        if event.event != E::EV_OBITUARY {
            return;
        }
        let target = event.state.field_i32("otherEntityNum").unwrap_or(-1);
        let attacker = event.state.field_i32("otherEntityNum2").unwrap_or(-1);
        let mod_ = event.parm;
        let Some(target_client) = usize::try_from(target).ok().filter(|&client| client < 32) else {
            return;
        };
        self.client_deaths[target_client] = self.client_deaths[target_client].saturating_add(1);
        let Some(target_name) = self.client_game.client_name(target_client) else {
            return;
        };

        let attacker_client = usize::try_from(attacker).ok().filter(|&client| client < 32);
        let attacker_name = attacker_client.and_then(|client| self.client_game.client_name(client));
        let gender = self.client_game.client_gender(target_client);
        let gender_key =
            |male: &'static str, female: &'static str, neuter: &'static str| match gender {
                1 => female,
                2 => neuter,
                _ => male,
            };

        let notice = if attacker == target {
            let key = match mod_ {
                4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 17 => gender_key(
                    "SUICIDE_SHOT_MALE",
                    "SUICIDE_SHOT_FEMALE",
                    "SUICIDE_SHOT_GENDERLESS",
                ),
                14 | 18 | 19 | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 => gender_key(
                    "SUICIDE_EXPLOSIVES_MALE",
                    "SUICIDE_EXPLOSIVES_FEMALE",
                    "SUICIDE_EXPLOSIVES_GENDERLESS",
                ),
                15 => gender_key(
                    "SUICIDE_ELECTROCUTED_MALE",
                    "SUICIDE_ELECTROCUTED_FEMALE",
                    "SUICIDE_ELECTROCUTED_GENDERLESS",
                ),
                38 => gender_key(
                    "SUICIDE_FALLDEATH_MALE",
                    "SUICIDE_FALLDEATH_FEMALE",
                    "SUICIDE_FALLDEATH_GENDERLESS",
                ),
                _ => gender_key(
                    "SUICIDE_GENERICDEATH_MALE",
                    "SUICIDE_GENERICDEATH_FEMALE",
                    "SUICIDE_GENERICDEATH_GENDERLESS",
                ),
            };
            crate::cgame::CgameNotice::Obituary {
                target: target_name,
                attacker: None,
                key,
            }
        } else if let Some(attacker_name) = attacker_name {
            let key = match mod_ {
                1 => "KILLED_STUN",
                2 => "KILLED_MELEE",
                3 => "KILLED_SABER",
                4 | 5 => "KILLED_BRYAR",
                6 | 7 => "KILLED_BLASTER",
                8 | 9 => "KILLED_DISRUPTOR",
                10 => "KILLED_DISRUPTORSNIPE",
                11 => "KILLED_BOWCASTER",
                12 => "KILLED_REPEATER",
                13 | 14 => "KILLED_REPEATERALT",
                15 | 16 => "KILLED_DEMP2",
                17 => "KILLED_FLECHETTE",
                18 => "KILLED_FLECHETTE_MINE",
                19 | 20 => "KILLED_ROCKET",
                21 | 22 => "KILLED_ROCKET_HOMING",
                23 | 24 => "KILLED_THERMAL",
                25 => "KILLED_TRIPMINE",
                26 => "KILLED_TRIPMINE_TIMED",
                27 => "KILLED_DETPACK",
                28 | 29 | 30 | 36 | 41 => "KILLED_GENERIC",
                31 => "KILLED_DARKFORCE",
                32 => "KILLED_SENTRY",
                37 => "KILLED_TELEFRAG",
                38 => "KILLED_FORCETOSS",
                _ => "KILLED_GENERIC",
            };
            crate::cgame::CgameNotice::Obituary {
                target: target_name,
                attacker: Some(attacker_name),
                key,
            }
        } else {
            let key = match mod_ {
                40 => "DIED_LASER",
                _ => "DIED_GENERIC",
            };
            crate::cgame::CgameNotice::Obituary {
                target: target_name,
                attacker: None,
                key,
            }
        };
        self.client_game.push_notice(notice);
    }

    /// TaystJK/jaPRO `cg_killMessage`: EV_OBITUARY is the authoritative kill
    /// event, so this shares exactly the same attacker/target semantics as
    /// `cg_killSounds` and works for live play, demos, and follow views.
    pub(in crate::app) fn note_kill_message(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        if event.event != E::EV_OBITUARY {
            return;
        }
        let option = self
            .sound_presenter
            .as_ref()
            .map_or(1, |sound| sound.game_sounds().kill_message);
        if option == 0 {
            return;
        }
        let Some(local) = self.client_game.presentation_client_num() else {
            return;
        };
        let target = event.state.field_i32("otherEntityNum").unwrap_or(-1);
        let attacker = event.state.field_i32("otherEntityNum2").unwrap_or(-1);
        if attacker != local || attacker == target {
            return;
        }
        let Some(info) = usize::try_from(target)
            .ok()
            .filter(|&client| client < 32)
            .and_then(|client| self.client_game.client_info(client, &self.siege_classes))
        else {
            return;
        };
        let (rank, score) = self
            .client_game
            .presentation_player_state()
            .map_or((None, None), |ps| {
                (Some(ps.persistant[2]), Some(ps.persistant[0]))
            });
        self.client_game
            .push_notice(crate::cgame::CgameNotice::KillMessage {
                target: info.name,
                rank,
                score,
                gametype: self.client_game.gametype(),
            });
    }

    /// EV_LOCALTIMER, EV_SCOREPLUM and the saber "no force" flash of EV_NOAMMO.
    pub(in crate::app) fn note_hud_events(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        let Some(local) = self.client_game.presentation_client_num() else {
            return;
        };
        let viewer_weapon = self
            .client_game
            .presentation_player_state()
            .and_then(|ps| ps.field_i32("weapon"));
        match event.event {
            E::EV_LOCALTIMER if event.state.field_i32("owner") == Some(local) => {
                let duration = event.state.field_i32("time2").unwrap_or(0);
                if duration > 0 {
                    self.timer_bar = Some((Instant::now(), duration));
                }
            }
            E::EV_SCOREPLUM if event.state.field_i32("otherEntityNum") == Some(local) => {
                // jaPRO spot icons (eventParm 1) are not drawn.
                if event.parm != 1 {
                    let score = event.state.field_i32("time").unwrap_or(0);
                    self.weapon_fx.add_score_plum(event.position, score);
                }
            }
            E::EV_NOAMMO if i32::from(event.entity_num) == local => {
                // Remote demo POVs have no complete playerState weapon/force data.
                if viewer_weapon == Some(3) {
                    self.force_flash_until = Some(Instant::now() + Duration::from_millis(1000));
                }
            }
            _ => {}
        }
    }

    /// EV_ITEM_PICKUP / EV_GLOBAL_ITEM_PICKUP for the viewer: HUD icon, "Picked
    /// up" line and weapon auto-select (`CG_ItemPickup`).
    pub(in crate::app) fn note_item_pickup(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        if !matches!(event.event, E::EV_ITEM_PICKUP | E::EV_GLOBAL_ITEM_PICKUP) {
            return;
        }
        let Some(local) = self.client_game.presentation_client_num() else {
            return;
        };
        if i32::from(event.entity_num) != local {
            return;
        }
        if self
            .client_game
            .presentation_player_state()
            .is_some_and(|ps| ps.field_i32("duelInProgress").unwrap_or(0) != 0)
        {
            return;
        }
        let index = if event.event == E::EV_ITEM_PICKUP {
            // The item entity sits in eventParm; its modelindex is the bg_itemlist index.
            let Ok(item_entity) = u16::try_from(event.parm) else {
                return;
            };
            if self
                .item_pickup_until
                .get(&event.parm)
                .is_some_and(|&until| until >= event.server_time)
            {
                return; // rww's double-pickup guard
            }
            self.item_pickup_until
                .insert(event.parm, event.server_time + 500);
            self.client_game
                .entity_state(item_entity)
                .and_then(|state| state.field_i32("modelindex"))
                .unwrap_or(0)
        } else {
            event.parm
        };
        let Some(item) = jka_movement::bg_item(index).filter(|_| index >= 1) else {
            return;
        };
        if let Some(icon) = jka_movement::bg_item_icon(index) {
            self.item_pickup = Some((icon, Instant::now()));
        }
        const IT_WEAPON: i32 = 1;
        const IT_TEAM: i32 = 8;
        if item.item_type == IT_WEAPON && self.pending_weapon_select.len() < 8 {
            self.pending_weapon_select.push(item.tag);
        }
        if item.item_type != IT_TEAM && !item.classname.is_empty() {
            self.client_game
                .push_notice(crate::cgame::CgameNotice::ItemPickupLine {
                    classname: item.classname,
                });
        }
    }

    /// The kick after the viewer fires (`CG_FireWeapon`, cg_screenShake 2).
    pub(in crate::app) fn note_weapon_shake(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        if self.screen_shake_level < 2 || !matches!(event.event, E::EV_FIRE_WEAPON | E::EV_ALT_FIRE)
        {
            return;
        }
        if self.client_game.presentation_client_num() != Some(i32::from(event.entity_num)) {
            return;
        }
        let alt = event.event == E::EV_ALT_FIRE;
        let weapon = event.state.field_i32("weapon").unwrap_or(0);
        // The charge start time rides in constantLight.
        let charged = ((event.server_time - event.state.field_i32("constantLight").unwrap_or(0))
            as f32
            * 0.001)
            .clamp(0.2, 3.0)
            * 2.0;
        let rand = |low: f32, high: f32| {
            low + (high - low) * ((event.receive_sequence % 100) as f32 / 100.0)
        };
        const WP_BRYAR_PISTOL: i32 = 4;
        const WP_BOWCASTER: i32 = 7;
        const WP_REPEATER: i32 = 8;
        const WP_DEMP2: i32 = 9;
        const WP_FLECHETTE: i32 = 10;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_BRYAR_OLD: i32 = 16;
        let (intensity, ms) = match (weapon, alt) {
            (WP_BRYAR_PISTOL | WP_BRYAR_OLD | WP_DEMP2, true) | (WP_BOWCASTER, false) => {
                (charged, 250)
            }
            (WP_ROCKET_LAUNCHER, _) | (WP_REPEATER, true) | (WP_FLECHETTE, true) => {
                (rand(2.0, 3.0), 350)
            }
            (WP_FLECHETTE, false) => (1.5, 250),
            _ => return,
        };
        self.event_presenter.add_camera_shake(intensity, ms);
    }

    /// EV_FALL / EV_ROLL landings and EV_STEP_* smoothing for the viewer.
    pub(in crate::app) fn note_view_kick(&mut self, event: &crate::cgame::PresentationEvent) {
        use EntityEvent as E;
        let Some(local) = self.client_game.presentation_client_num() else {
            return;
        };
        if event.state.field_i32("clientNum").unwrap_or(-2) != local {
            return;
        }
        let (falling_to_death, pm_flags) =
            self.client_game
                .presentation_player_state()
                .map_or((false, 0), |ps| {
                    (
                        ps.field_i32("fallingToDeath").unwrap_or(0) != 0,
                        ps.field_i32("pm_flags").unwrap_or(0),
                    )
                });
        let time = event.server_time;
        match event.event {
            E::EV_FALL | E::EV_ROLL => {
                // EV_ROLL only runs DoFall for a fall-roll-in-one (eventParm set).
                if !falling_to_death && (event.event == E::EV_FALL || event.parm != 0) {
                    self.event_presenter.view_kick_mut().land(event.parm, time);
                }
            }
            E::EV_STEP_4 | E::EV_STEP_8 | E::EV_STEP_12 | E::EV_STEP_16 => {
                // Demos and followed players interpolate, so they need no smoothing.
                const PMF_FOLLOW: i32 = 4096;
                if (self.live || self.local) && pm_flags & PMF_FOLLOW == 0 {
                    let steps = event.event.as_i32() - E::EV_STEP_4.as_i32() + 1;
                    self.event_presenter.view_kick_mut().step(steps, time);
                }
            }
            _ => {}
        }
    }

    pub(in crate::app) fn dispatch_prepared_event(
        &mut self,
        prepared: event_workers::PreparedPresentationEvent,
        target_server_time: i32,
        debug_events: u8,
    ) {
        let event_workers::PreparedPresentationEvent {
            event,
            sound,
            fx,
            visual,
        } = prepared;
        // OpenJK EV_DESTROY_WEAPON_MODEL mutates the target Ghoul2 model before
        // this frame's player/body rendering. Event-worker preparation never
        // owns or mutates presenter state; ordered application stays here.
        self.player_presenter.apply_entity_event(&event);
        self.note_out_of_ammo(&event);
        self.note_view_kick(&event);
        self.note_weapon_shake(&event);
        self.note_item_pickup(&event);
        self.note_hud_events(&event);
        self.note_obituary(&event);
        self.note_kill_message(&event);
        self.note_stringed_messages(&event);
        let fx_dispatch = self.weapon_fx.entity_event_prepared(&event, &fx);
        let sound_dispatch_started = Instant::now();
        let sound_dispatch = if self.suppress_audio {
            None
        } else {
            match (self.sound_presenter.as_mut(), sound) {
                (Some(presenter), Some(sound)) => presenter.dispatch_prepared(
                    &event,
                    &self.client_game,
                    &self.siege_classes,
                    target_server_time,
                    sound,
                ),
                _ => None,
            }
        };
        self.client_perf.audio_ms += sound_dispatch_started.elapsed().as_secs_f64() * 1000.0;
        let dispatch = sound_dispatch
            .or(fx_dispatch)
            .unwrap_or_else(|| self.event_presenter.dispatch_prepared(&event, visual));
        self.record_event_stat(event.event, event.server_time, dispatch.class());
        if debug_events >= 1 {
            self.push_event_debug_line(event_debug::accepted_line(&event, &dispatch.status()));
        }
        if debug_events >= 3 {
            for line in event_debug::verbose_lines(&event, &self.client_game) {
                self.push_event_debug_line(line);
            }
        }
    }

    pub(in crate::app) fn event_stats_lines(&self) -> Vec<String> {
        if self.event_stats.is_empty() {
            return vec!["^3CG EVENT STATS:^7 no accepted events observed yet".into()];
        }
        let total: u64 = self.event_stats.values().map(|stat| stat.count).sum();
        let mut rows: Vec<_> = self.event_stats.iter().collect();
        rows.sort_by(|(event_a, stat_a), (event_b, stat_b)| {
            stat_b
                .count
                .cmp(&stat_a.count)
                .then_with(|| event_a.cmp(event_b))
        });
        let mut lines = vec![format!(
            "^3CG EVENT STATS:^7 accepted={} unique={} suppressedDuplicate={} suppressedZero={}  H/P/U columns show presentation result",
            total,
            rows.len(),
            self.event_suppressed_duplicate,
            self.event_suppressed_zero,
        )];
        lines.extend(rows.into_iter().map(|(&event, stat)| {
            format!(
                "  {:>5}  {:<28} id={:<3} cat={:<10} H/P/U={}/{}/{} first={} last={}",
                stat.count,
                event_debug::event_name(event),
                event.as_i32(),
                event_debug::event_category(event),
                stat.handled,
                stat.partial,
                stat.unhandled,
                stat.first_server_time,
                stat.last_server_time,
            )
        }));
        lines
    }

    pub(in crate::app) fn clear_event_stats(&mut self) {
        self.event_stats.clear();
        self.event_suppressed_duplicate = 0;
        self.event_suppressed_zero = 0;
    }
}
