//! Frame.
use crate::app::{
    rendering_third_person, ui, App, Arc, ClientFramePerf, FrontendPage, InputLatencySample,
    DemoViewMode, Duration, Instant, LocalServer, MapEditor, OverlayMode, PlayerViewPolicyState, RenderCommand,
    RenderSnapshot, SessionPhase, SpectatorHighlight, SpectatorHighlightTeam, ViewLatchMode,
};

/// Build the menu's presentation view without mutating the camera that owns
/// the user's archived gameplay FOV. This also keeps preview crossfades and
/// live menu frames on the exact same projection when a zoom is held or a
/// gameplay camera option changes in the settings UI.
fn frontend_scene_camera(mut camera: crate::camera::Camera) -> crate::camera::Camera {
    camera.set_presentation_fov(super::constants::FRONTEND_SCENE_FOV);
    camera.shake = Default::default();
    camera.clear_first_person_bob();
    camera
}

impl App {
    pub(in crate::app) fn subframe_input_view_rotation(&self) -> Option<[f32; 2]> {
        if !self.live_remote_view_forced() {
            if let Some(angles) = self.live_view_angles() {
                return Some([angles[1].to_radians(), -angles[0].to_radians()]);
            }
        }
        self.local_server.as_ref().and_then(|server| {
            if server.view().view_forced != 0 {
                return None;
            }
            let angles = server.subframe_view_angles();
            Some([angles[1].to_radians(), -angles[0].to_radians()])
        })
    }

    pub(in crate::app) fn render_snapshot(&self) -> RenderSnapshot {
        let demo_player_position = self
            .game_session
            .as_ref()
            .and_then(|playback| playback.player_position);
        let studio_demo = self.studio_demo_scene_active();
        let dynamic_models = if self.profile_preview_active && !studio_demo {
            Arc::clone(&self.profile_dynamic_models)
        } else {
            self.game_session
                .as_ref()
                .map(|playback| {
                    if playback.fx_surfaces.is_empty() {
                        Arc::clone(&playback.dynamic_models)
                    } else {
                        let mut merged = Vec::with_capacity(
                            playback.dynamic_models.len() + playback.fx_surfaces.len(),
                        );
                        merged.extend(playback.dynamic_models.iter().cloned());
                        merged.extend(playback.fx_surfaces.iter().cloned());
                        Arc::new(merged)
                    }
                })
                .unwrap_or_else(|| Arc::clone(&self.solo_dynamic_models))
        };
        let dynamic_models = if self.overlay == OverlayMode::MapEdit {
            if let Some(preview) = self
                .map_editor
                .as_ref()
                .and_then(MapEditor::preview_surface)
            {
                let mut merged = Vec::with_capacity(dynamic_models.len() + 1);
                merged.extend(dynamic_models.iter().cloned());
                merged.push(preview);
                Arc::new(merged)
            } else {
                Arc::clone(&dynamic_models)
            }
        } else {
            dynamic_models
        };
        let dynamic_models = match self.emote_gallery_models() {
            Some(gallery) if !gallery.is_empty() => {
                let mut merged = Vec::with_capacity(dynamic_models.len() + gallery.len());
                merged.extend(dynamic_models.iter().cloned());
                merged.extend(gallery.iter().cloned());
                Arc::new(merged)
            }
            _ => dynamic_models,
        };
        let dynamic_models = if !self.model_placement.placed_draws.is_empty()
            || !self.model_placement.preview_draws.is_empty()
            || !self.model_placement.picker_draws.is_empty()
            || !self.model_placement.fx_draws.is_empty() {
            let mut merged = Vec::with_capacity(dynamic_models.len()
                + self.model_placement.placed_draws.len()
                + self.model_placement.preview_draws.len()
                + self.model_placement.picker_draws.len()
                + self.model_placement.fx_draws.len());
            merged.extend(dynamic_models.iter().cloned());
            merged.extend(self.model_placement.placed_draws.iter().cloned());
            merged.extend(self.model_placement.preview_draws.iter().cloned());
            merged.extend(self.model_placement.picker_draws.iter().cloned());
            merged.extend(self.model_placement.fx_draws.iter().cloned());
            Arc::new(merged)
        } else {
            dynamic_models
        };
        // CGame owns mover state once it has a snapshot (demo, live, or the
        // solo shim, which spawns brush entities as ET_MOVERs); before that
        // inline models keep their compiled pose.
        let inline_models = self
            .game_session
            .as_ref()
            .filter(|playback| playback.current_snapshot.is_some())
            .map(|playback| Arc::clone(&playback.inline_models));
        let inline_models = if !self.model_placement.bmodel_placed.is_empty()
            || (self.model_placement.active
                && self.model_placement.kind == super::egui_model_placement::PlacementKind::Bmodel)
        {
            let mut instances = if let Some(authoritative) = inline_models.as_ref() {
                authoritative.as_ref().clone()
            } else {
                // Compiled inline models must remain visible before CGame has
                // supplied its first authoritative snapshot. Passing Some([])
                // without these base poses would hide every existing bmodel.
                let mut originals = std::collections::BTreeMap::new();
                if let Some(cache) = self.prepared_map_cache.as_ref() {
                    for model in &cache.map.inline_models {
                        originals.entry(model.model).or_insert(crate::renderer::InlineModelInstance {
                            model: model.model,
                            origin: [0.0; 3],
                            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                            is_portal_entity: false,
                        });
                    }
                }
                originals.into_values().collect()
            };
            instances.extend(self.model_placement.bmodel_placed.iter().map(|p| p.instance()));
            if self.model_placement.active
                && self.model_placement.kind == super::egui_model_placement::PlacementKind::Bmodel
                && self.model_placement.hit_valid
            {
                if let (Some(model), Some(bounds)) = (self.model_placement.bmodel_selection,
                    self.model_placement.bmodel_bounds)
                {
                    instances.push(super::egui_model_placement::BmodelPlacement {
                        model, bounds, pivot: self.model_placement.bmodel_pivot,
                        hit: self.model_placement.origin,
                        angles: self.model_placement.angles,
                    }.instance());
                }
            }
            Some(Arc::new(instances))
        } else {
            inline_models
        };
        // Profile/asset previews are isolated UI scenes, equivalent to the
        // RDF_NOWORLDMODEL model widgets in the JKA UI. They must never inherit
        // gameplay view rotation (including the render-thread subframe latch).
        let isolated_preview = (self.profile_preview_active && !studio_demo)
            || (self.front_end && self.frontend_page == FrontendPage::AssetViewer);
        let input_view_rotation = if isolated_preview || self.front_end {
            // Menu cameras never consume live mouse/third-person late-latch data.
            None
        } else {
            self.subframe_input_view_rotation()
        };
        let spectator_highlights = if !isolated_preview
            && self.japro_cg.spec_hud
            && (self.demo_playback_active() || self.live_spec_hud_eligible())
        {
            self.game_session.as_ref().map_or_else(
                || Arc::new(Vec::new()),
                |session| {
                    const EF_DEAD: i32 = 1 << 1;
                    // Exclude exactly the player whose camera we are presenting.
                    // Do not infer this from the demo recorder's playerState: fake
                    // follow owns another client and freecam owns no client at all.
                    let viewed_client = if session.live {
                        match session.spectate_info() {
                            Some(info) => info.followed.map(|(client, _)| client),
                            None => session.client_game.presentation_client_num(),
                        }
                    } else if session.local {
                        session.client_game.presentation_client_num()
                    } else {
                        match self.demo_view_mode {
                            DemoViewMode::Authoritative => session.demo_followed_client(),
                            DemoViewMode::Follow(client) => Some(client),
                            DemoViewMode::Free => None,
                        }
                    };
                    Arc::new(
                        session
                            .presented_entities
                            .iter()
                            // The snapshot/predicted player is deliberately
                            // absent from presented_entities and submitted via
                            // a synthesized entity instead. Include it here too:
                            // once freecam/fake-follow detaches from that player,
                            // they become an ordinary outline candidate.
                            .chain(session.audio_followed_entity.iter())
                            .filter(|entity| {
                                entity.entity_type == crate::cgame::ET_PLAYER
                                    && viewed_client != Some(i32::from(entity.number))
                                    && entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0
                                    // TaystJK cg_specHud deliberately excludes racers.
                                    && entity.state.field_i32("bolt1").unwrap_or(0) != 2
                            })
                            .map(|entity| {
                                let team = session
                                    .client_game
                                    .client_info(usize::from(entity.number), &session.siege_classes)
                                    .map_or(0, |info| info.team);
                                SpectatorHighlight {
                                    entity_num: entity.number,
                                    team: match team {
                                        crate::cgame::TEAM_RED => SpectatorHighlightTeam::Red,
                                        crate::cgame::TEAM_BLUE => SpectatorHighlightTeam::Blue,
                                        _ => SpectatorHighlightTeam::Yellow,
                                    },
                                }
                            })
                            .collect(),
                    )
                },
            )
        } else {
            Arc::new(Vec::new())
        };
        RenderSnapshot {
            studio_lens: (self.front_end&&self.frontend_page==FrontendPage::RenderStudio)
                .then(|| {
                    let key=self.render_studio.camera_rig_runtime.camera.filter(|_|self.render_studio.timeline.camera_rig.active())
                        .unwrap_or_else(||self.studio_camera_key_at(self.render_studio.timeline_time()));
                    let mut lens=key.lens;
                    // The Scene view has its own editor camera; otherwise show
                    // the authored lens during playback just as in the export.
                    if self.render_studio.spatial.scene_camera.is_some()
                        && self.render_studio.export.is_none() && self.render_studio.still.is_none(){lens.aperture=None;}
                    lens
                }),
            studio_scene: self.render_studio.scene_frame.clone().or_else(||{
                if !self.studio_bsp_active() {return None;}
                let world=self.render_studio.bsp_world.as_ref()?;
                let id=world.id;
                let camera=self.asset_preview_camera();
                let time=self.render_studio.timeline_time();
                let timeline=&self.render_studio.timeline;
                let environment=timeline.scene_lighting.sample(time,
                    timeline.interpolation!=super::render_studio_timeline::StudioInterpolation::Linear).render_sample();
                Some(Arc::new(crate::renderer::StudioSceneFrame {
                    outgoing:crate::renderer::StudioSourceFrame {
                        id,revision:0,shot_camera:camera,camera,
                        lens: self.studio_camera_key_at(time).lens,
                        environment:Some(environment),time:time as f32,
                        player_position:None,area_mask:None,
                        dynamic_models:Arc::clone(&dynamic_models),
                        inline_models:Arc::clone(&world.inline_models),
                        lights:Arc::new(timeline.lights_at(time)),
                        screen_fx:Arc::new(Vec::new()),cloud_foreground:Arc::new(Vec::new()),
                        ocean_actors:Arc::new(Vec::new()),hud:None,
                    },
                    incoming:None,effect:0,progress:0.0,video:self.video,
                    offline_capture:self.render_studio.export.is_some() || self.render_studio.still.is_some(),
                }))
            }),
            studio_environment: (self.profile_preview_active && !studio_demo
                && self.frontend_page==FrontendPage::RenderStudio).then(|| {
                let timeline=&self.render_studio.timeline;
                timeline.scene_lighting.sample(self.render_studio.timeline_time(),
                    timeline.interpolation!=super::render_studio_timeline::StudioInterpolation::Linear).render_sample()
            }),
            model_frame_source: self.network.model_frame_debug.then(|| {
                let playback = self.game_session.as_ref();
                crate::model_frame_log::FrameSource::new(
                    playback
                        .and_then(|s| s.audio_followed_entity.as_ref())
                        .map_or(1023, |e| e.number),
                    playback.and_then(|s| s.player_presenter.viewer_anim_debug()),
                )
            }),
            camera: if isolated_preview {
                self.asset_preview_camera()
            } else if self.front_end {
                frontend_scene_camera(self.camera)
            } else {
                let mut camera = self.camera;
                camera.set_presentation_fov(self.japro_effective_fov(Instant::now()));
                if let Some(playback) = self.game_session.as_ref() {
                    // Render space is Y-up: landing dip and stair smoothing move a
                    // player-owned eye vertically. Demo freecam has no player owner;
                    // its stored camera position is already the exact render position.
                    if playback.live || playback.local || self.demo_view_mode != DemoViewMode::Free {
                        camera.position.y += playback.view_offset_z;
                    }
                }
                camera
            },
            view_latch: if isolated_preview || self.front_end || input_view_rotation.is_none() {
                ViewLatchMode::Disabled
            } else {
                self.render_view_latch
            },
            input_view_rotation,
            dynamic_crosshair_world: self
                .crosshair_target
                .dynamic
                .then_some(self.crosshair_hit_world)
                .flatten(),
            player_position: demo_player_position
                .or_else(|| self.local_server.as_ref().map(LocalServer::world_position)),
            ocean_actors: self.game_session.as_ref().map_or_else(
                || Arc::new(Vec::new()), |s| Arc::clone(&s.ocean_actors)),
            area_mask: self
                .game_session
                .as_ref()
                .and_then(|playback| playback.current_snapshot.as_ref())
                .map(|snapshot| snapshot.area_mask),
            dynamic_models,
            player_water: if isolated_preview {
                Arc::new(Default::default())
            } else {
                Arc::new(self.water_wetness.profiles(Instant::now()))
            },
            spectator_highlights,
            transient_lights: if self.profile_preview_active && !studio_demo {
                Arc::new(if self.frontend_page==FrontendPage::RenderStudio {
                    self.render_studio.timeline.lights_at(self.render_studio.timeline_time())
                } else {Vec::new()})
            } else if self.front_end && self.frontend_cinematic.is_some() {
                self.menu_props.as_ref().map_or_else(
                    || Arc::new(Vec::new()), |props| Arc::clone(&props.lights))
            } else {
                self.game_session.as_ref().map_or_else(
                    || Arc::new(Vec::new()),
                    |playback| Arc::clone(&playback.fx_lights),
                )
            },
            screen_fx: self.game_session.as_ref().map_or_else(
                || Arc::new(Vec::new()),
                |playback| Arc::clone(&playback.screen_fx),
            ),
            cloud_foreground: if self.profile_preview_active && !studio_demo {
                Arc::new(Vec::new())
            } else {
                self.game_session.as_ref().map_or_else(
                    || Arc::new(Vec::new()),
                    |playback| Arc::clone(&playback.fx_blades),
                )
            },
            inline_models,
            companion_scene: self.companion_scene_view(),
            dof_focus_target: self.dof_focus_target,
            input_latency: self.last_simulated_mouse_input,
            client_perf: self
                .game_session
                .as_ref()
                .map_or_else(ClientFramePerf::default, |playback| playback.client_perf),
            hud: self.current_hud_state(),
            movement_hud: self.current_movement_hud_state(),
            player_names: self.world_player_names(),
            jump_shade: self.current_jump_shade(),
            camera_shake: if isolated_preview || self.front_end {
                None
            } else {
                self.game_session
                    .as_ref()
                    .and_then(|playback| playback.event_presenter.camera_shake())
            },
        }
    }

    /// CG_OutOfAmmoChange for EV_NOAMMO events the session queued.
    pub(in crate::app) fn apply_out_of_ammo_changes(&mut self) {
        let auto_switch = self.audio.game.auto_switch;
        let Some(session) = self.game_session.as_mut() else {
            return;
        };
        if session.pending_out_of_ammo.is_empty() && session.pending_weapon_select.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut session.pending_out_of_ammo);
        let picked = std::mem::take(&mut session.pending_weapon_select);
        let Some(snapshot) = session.current_snapshot.as_ref() else {
            return;
        };
        if auto_switch > 0 {
            for (old, dry) in pending {
                let mut ps = snapshot.player_state.clone();
                if dry > 0 {
                    ps.stats[4] &= !(1 << dry);
                }
                self.live_input.out_of_ammo_change(&ps, old, auto_switch);
            }
        }
        for tag in picked {
            self.live_input
                .pickup_autoswitch(&snapshot.player_state, tag, auto_switch);
        }
    }

    pub(in crate::app) fn publish_snapshot(&self) {
        if let Some(render) = &self.render {
            render.publish(self.render_snapshot());
        }
    }

    /// Feed the render-only latest-view mailbox directly from live mouse input.
    /// The mailbox always contains raw local viewangles; RenderSnapshot::view_latch
    /// decides whether they map directly to first person or through the cheap
    /// no-trace portion of the existing OpenJK third-person camera.
    pub(in crate::app) fn publish_live_subframe_view(&self, input: InputLatencySample) {
        if self.live_remote_view_forced() {
            return;
        }
        let Some(view_angles) = self.live_view_angles() else {
            return;
        };
        if let Some(render) = &self.render {
            render.publish_subframe_view_rotation(
                view_angles[1].to_radians(),
                -view_angles[0].to_radians(),
                self.crosshair_target
                    .dynamic
                    .then_some(self.crosshair_hit_world)
                    .flatten(),
                input,
            );
        }
    }

    pub(in crate::app) fn publish_local_subframe_view(&self, input: InputLatencySample) -> bool {
        let Some(server) = self.local_server.as_ref() else {
            return false;
        };
        let player_view = server.view();
        if player_view.view_forced != 0 {
            return false;
        }
        let entity_view = server.entity_view();
        let policy = PlayerViewPolicyState::from_entity_view(entity_view, player_view.legs_timer);
        if rendering_third_person(self.third_person, self.first_person_lightsaber, policy)
            && !matches!(self.render_view_latch, ViewLatchMode::ThirdPerson(_))
        {
            // Around a wall, during camera damping, or on the first camera frame,
            // keep the pre-existing local fallback that reruns the authoritative
            // CGame camera instead of using an untraced third-person orbit.
            return false;
        }
        let view_angles = server.subframe_view_angles();
        if let Some(render) = &self.render {
            render.publish_subframe_view_rotation(
                view_angles[1].to_radians(),
                -view_angles[0].to_radians(),
                self.crosshair_target
                    .dynamic
                    .then_some(self.crosshair_hit_world)
                    .flatten(),
                input,
            );
            return true;
        }
        false
    }

    pub(in crate::app) fn publish_ui(&self) {
        if self.settings_batch_open {
            self.ui_publish_pending.set(true);
            return;
        }
        self.render_command(RenderCommand::SetUi(self.ui_snapshot()));
        self.render_command(RenderCommand::SetStreamerSettings(self.streamer));
    }

    /// Start a batch of settings changes. The renderer defers pipeline-variant
    /// activation and this side coalesces UI snapshots until `end_settings_batch`.
    pub(in crate::app) fn begin_settings_batch(&mut self, title: &str) {
        self.settings_batch_open = true;
        self.render_command(RenderCommand::BatchBegin(title.to_owned()));
    }

    pub(in crate::app) fn end_settings_batch(&mut self) {
        self.settings_batch_open = false;
        if self.ui_publish_pending.take() {
            self.publish_ui();
        }
        self.render_command(RenderCommand::BatchEnd);
    }

    /// Both invnext/invprev and TaystJK's force->inventory cycling use this
    /// selector. Like the weapon/Force strips, only the most recently used
    /// selection occupies the bottom-center HUD area.
    pub(in crate::app) fn show_inventory_selection(&mut self, ps: &jka_protocol::server::PlayerState) {
        if ps.stats[0] <= 0 || ps.stats[1] == 0 || ps.stats[2] == 0
            || ps.persistant[3] == 3
            || ps.field_i32("pm_flags").unwrap_or(0) & 4096 != 0
            || ps.field_i32("pm_type").unwrap_or(0) == 4
            || !(1..12).contains(&self.live_input.inventory_select)
            || matches!(self.live_input.inventory_select, 7 | 8 | 9)
        {
            return;
        }
        self.inventory_select_until = Some(Instant::now() + Duration::from_millis(1400));
        self.weapon_select_until = None;
        self.force_select_until = None;
        self.publish_transient_ui();
    }

    pub(in crate::app) fn inventory_select_ui(&self, now: Instant) -> Option<ui::UiInventorySelect> {
        if now >= self.inventory_select_until? { return None; }
        let ps = self.live_player_state()?;
        let selected = self.live_input.inventory_select;
        if ps.stats[0] <= 0 || ps.stats[1] == 0 || ps.stats[2] == 0
            || ps.persistant[3] == 3
            || ps.field_i32("pm_flags").unwrap_or(0) & 4096 != 0
            || ps.field_i32("pm_type").unwrap_or(0) == 4
            || !(1..12).contains(&selected)
            || ps.stats[2] & (1 << selected) == 0
        { return None; }
        Some(ui::UiInventorySelect {
            selected,
            owned_bits: ps.stats[2] as u32,
        })
    }

    pub(in crate::app) fn force_select_ui(&self, now: Instant) -> Option<ui::UiForceSelect> {
        let until = self.force_select_until?;
        if now >= until {
            return None;
        }
        let ps = self.live_player_state()?;
        if ps.stats[0] <= 0 {
            return None;
        }
        let known_bits = ps.field_i32("fd.forcePowersKnown").unwrap_or(0) as u32;
        Some(ui::UiForceSelect {
            selected: self.live_input.force_select,
            known_bits,
        })
    }

    pub(in crate::app) fn weapon_select_ui(&self, now: Instant) -> Option<ui::UiWeaponSelect> {
        if now >= self.weapon_select_until? { return None; }
        let ps = self.live_player_state()?;
        const PMF_FOLLOW: i32 = 4096;
        const PM_SPECTATOR: i32 = 4;
        if ps.stats[0] <= 0 || ps.persistant[3] == 3
            || ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0
            || ps.field_i32("pm_type").unwrap_or(0) == PM_SPECTATOR
            || ps.field_i32("emplacedIndex").unwrap_or(0) != 0
        { return None; }
        let mut selectable_bits = 0_u32;
        let mut ammo_available_bits = 0_u32;
        for weapon in 1..=16 {
            let Some((ammo_index, primary_cost, alt_cost)) = jka_movement::weapon_info(weapon) else {
                continue;
            };
            let ammo = ps.ammo.get(ammo_index.max(0) as usize).copied().unwrap_or(0);
            let has_ammo = ammo >= primary_cost || ammo >= alt_cost;
            let detpack_valid = weapon != 14 || ammo > 0
                || ps.field_i32("hasDetPackPlanted").unwrap_or(0) != 0;
            if has_ammo { ammo_available_bits |= 1 << weapon; }
            if ps.stats[4] & (1 << weapon) != 0 && has_ammo && detpack_valid {
                selectable_bits |= 1 << weapon;
            }
        }
        Some(ui::UiWeaponSelect {
            selected: self.live_input.weapon_select,
            owned_bits: ps.stats[4] as u32,
            selectable_bits,
            ammo_available_bits,
            saber_style: ps.field_i32("fd.saberDrawAnimLevel").unwrap_or(0),
        })
    }

    pub(in crate::app) fn publish_transient_ui(&self) {
        let (mut chat_lines, mut center_print) = self.transient_ui_snapshot();
        let mut console_notify_lines = self.console_notify_ui_snapshot();
        // Studio owns the transport and IN/OUT selection when editing a demo;
        // don't paint the native viewer's timeline over its preview rectangle.
        let demo_timeline = if self.studio_demo_scene_active() {None}
            else {self.demo_timeline_ui(Instant::now())};
        let prediction_debug = self.prediction_debug_ui();
        let mut crosshair_target = self.crosshair_target.clone();
        let mut follow_name = self.current_follow_name();
        let mut race_timer = self
            .game_session
            .as_ref()
            .and_then(|session| session.race_timer_ui.clone());
        let mut vote_line = self.vote_hud_line();
        let mut speedometer = self.speedometer_ui.clone();
        let team_overlay = self.team_overlay_ui();
        let scoreboard = self.visible_scoreboard();
        let scoreboard_focus_client = self.scoreboard_focus_client();
        let killfeed = if self.overlay == OverlayMode::HudEdit {
            self.killfeed_ui().or_else(|| self.killfeed_sample_ui())
        } else {
            self.killfeed_ui()
        };
        let force_select = self.force_select_ui(Instant::now());
        let weapon_select = self.weapon_select_ui(Instant::now());
        let inventory_select = self.inventory_select_ui(Instant::now());
        if self.overlay == OverlayMode::HudEdit {
            self.hud_edit_transient_samples(
                &mut chat_lines,
                &mut center_print,
                &mut follow_name,
                &mut vote_line,
                &mut crosshair_target,
                &mut race_timer,
                &mut speedometer,
            );
        }
        if self.streamer_preview_local {
            crate::streamer::redact_stream_transients(&mut chat_lines, &mut console_notify_lines, self.streamer);
        }
        self.render_command(RenderCommand::SetTransientUi {
            chat_lines,
            console_notify_lines,
            center_print,
            demo_timeline,
            prediction_debug,
            crosshair_target,
            force_select,
            weapon_select,
            inventory_select,
            follow_name,
            game_timer: self.game_timer_text(),
            mini_scores: self.mini_scores_ui(),
            hud_score: self.hud_score_ui(),
            enemy_info: self.enemy_info_ui(),
            race_timer,
            vote_line,
            scoreboard,
            scoreboard_focus_client,
            killfeed,
            chat_anim: self.chat_anim_ui(Instant::now()),
            speedometer,
            lagometer: self.current_lagometer_ui(),
            team_overlay,
        });
    }

    pub(in crate::app) fn publish_telemetry_ui(&self) {
        self.render_command(RenderCommand::SetUiTelemetry {
            perf: self.perf,
            threads: self.threads.clone(),
        });
    }

    pub(in crate::app) fn render_command(&self, command: RenderCommand) {
        if let Some(render) = &self.render {
            render.command(command);
        }
    }

    pub(in crate::app) fn demo_playback_active(&self) -> bool {
        self.game_session.as_ref().is_some_and(|session| {
            !session.live && session.phase == SessionPhase::Playing && session.demo_index.is_some()
        })
    }
}

#[cfg(test)]
mod frontend_scene_camera_tests {
    use super::frontend_scene_camera;
    use crate::camera::{Camera, CameraShakeOffset};

    #[test]
    fn frontend_projection_ignores_gameplay_fov_without_overwriting_it() {
        let reference = frontend_scene_camera(Camera::new_with_fov(
            [100.0, 35.0, -50.0], 0.2, 90.0,
        ));
        for gameplay_fov in [1.0, 50.0, 90.0, 120.0, 140.0] {
            let gameplay = Camera::new_with_fov([100.0, 35.0, -50.0], 0.2, gameplay_fov);
            let menu = frontend_scene_camera(gameplay);
            assert_eq!(gameplay.cg_fov(), gameplay_fov, "gameplay FOV must survive");
            assert_eq!(menu.cg_fov(), super::super::constants::FRONTEND_SCENE_FOV);
            for (width, height) in [(1280, 720), (2560, 1440), (1920, 1200)] {
                let matrix = menu.view_projection(width, height).to_cols_array();
                assert_eq!(
                    matrix,
                    reference.view_projection(width, height).to_cols_array(),
                    "FOV must not change menu framing",
                );
            }
        }
    }

    #[test]
    fn frontend_camera_preserves_cinematic_pose_but_discards_game_shake() {
        let mut gameplay = Camera::new_with_fov([10.0, 20.0, 30.0], 1.25, 118.0);
        gameplay.pitch = -0.12;
        gameplay.shake = CameraShakeOffset {
            position: glam::Vec3::new(1.0, 2.0, 3.0),
            yaw: 0.7,
            pitch: 0.2,
        };
        let menu = frontend_scene_camera(gameplay);
        assert_eq!(menu.position, gameplay.position);
        assert_eq!(menu.yaw, gameplay.yaw);
        assert_eq!(menu.pitch, gameplay.pitch);
        assert_eq!(menu.shake, CameraShakeOffset::default());
        assert_eq!(gameplay.shake.position, glam::Vec3::new(1.0, 2.0, 3.0));
    }
}
