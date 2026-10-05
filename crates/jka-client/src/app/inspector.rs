//! Inspector.
use crate::app::{
    playerstate_vec3, rendering_third_person, scene, ui, App, CrosshairSeen, DemoViewMode,
    Duration, EntityPresentationKind, InspectorEntityHint, Instant, OverlayMode,
    PlayerViewPolicyState, PresentedEntity, RenderCommand, SessionPhase, SurfaceInspectorInfo,
    SurfaceInspectorSection,
};

/// Half-extent of the box drawn for a point entity (no brush model) by
/// `r_drawEntities` — shared by the mesh builder and the trace hit test so
/// what you can click always matches what you see.
pub(in crate::app) const ENTITY_MARKER_POINT_HALF_SIZE: f32 = 8.0;

/// World-space AABB `r_drawEntities` draws for one entity, given its already
/// live-corrected `origin` (see `App::live_entity_origin`).
pub(in crate::app) fn entity_marker_box(
    entity: &crate::entity_graph::GraphEntity,
    origin: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    match entity.bounds {
        Some((bounds_mins, bounds_maxs)) => {
            let half: [f32; 3] =
                std::array::from_fn(|axis| (bounds_maxs[axis] - bounds_mins[axis]) * 0.5);
            (
                std::array::from_fn(|axis| origin[axis] - half[axis]),
                std::array::from_fn(|axis| origin[axis] + half[axis]),
            )
        }
        None => (
            std::array::from_fn(|axis| origin[axis] - ENTITY_MARKER_POINT_HALF_SIZE),
            std::array::from_fn(|axis| origin[axis] + ENTITY_MARKER_POINT_HALF_SIZE),
        ),
    }
}

/// Slab-method ray/AABB intersection. Returns the entry distance along `dir`
/// (which need not be normalized-length-aware by the caller; callers here
/// only compare distances against each other, never against a world unit
/// budget), or `None` when the ray starts past the box or misses it entirely.
pub(in crate::app) fn ray_aabb_distance(
    origin: [f32; 3],
    dir: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
) -> Option<f32> {
    let mut tmin = 0.0f32;
    let mut tmax = f32::MAX;
    for axis in 0..3 {
        if dir[axis].abs() < 1e-6 {
            if origin[axis] < mins[axis] || origin[axis] > maxs[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir[axis];
        let (mut t1, mut t2) = (
            (mins[axis] - origin[axis]) * inv,
            (maxs[axis] - origin[axis]) * inv,
        );
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    }
    // `tmin` sticks at its 0.0 initial value exactly when the origin is
    // already inside the box on every axis (it never found a positive entry
    // time). Standing inside a room-sized trigger volume is normal; that
    // should not hijack every trace until the player walks back out of it.
    (tmin > 0.0).then_some(tmin)
}

impl App {
    /// Live snapshot entity backing a brush `graph_entity` (`inline_model`),
    /// identified the same SOLID_BMODEL/modelindex way `enrich_surface_inspector`
    /// identifies the entity under the crosshair. `None` for point entities
    /// (never networked) or when nothing in the current snapshot matches.
    pub(in crate::app) fn live_presented_entity(
        &self,
        graph_entity: &crate::entity_graph::GraphEntity,
    ) -> Option<&PresentedEntity> {
        let inline_model = graph_entity.inline_model?;
        let session = self.game_session.as_ref()?;
        const SOLID_BMODEL: i32 = 0x00ff_ffff;
        session.presented_entities.iter().find(|entity| {
            entity.state.field_i32("solid").unwrap_or(0) == SOLID_BMODEL
                && entity.state.field_i32("modelindex").unwrap_or(0) == inline_model as i32
        })
    }

    /// World position of a map entity, corrected for live motion when it's a
    /// currently-networked brush entity. idTech3 brush-model convention:
    /// `entity.origin` is the translation added to the model's BSP-baked
    /// geometry (see `net::solid_entities`'s `EntityClip::InlineModel`), so it
    /// is zero at rest and nonzero only while the mover is off its spawn spot.
    pub(in crate::app) fn live_entity_origin(
        &self,
        graph_entity: &crate::entity_graph::GraphEntity,
    ) -> [f32; 3] {
        let static_origin = graph_entity.origin;
        match self.live_presented_entity(graph_entity) {
            Some(entity) => std::array::from_fn(|axis| static_origin[axis] + entity.origin[axis]),
            None => static_origin,
        }
    }

    /// `r_drawEntities`: rebuilds the box + link-line overlay from the static
    /// `EntityGraph` every tick and ships it to the renderer. Entity counts
    /// are small (tens to a few hundred per map), so a full CPU rebuild +
    /// GPU reupload each tick is simpler than dirty-tracking and cheap enough
    /// for a debug toggle.
    pub(in crate::app) fn rebuild_entity_markers(&mut self) {
        let Some(graph) = self.entity_graph.as_deref() else {
            self.render_command(RenderCommand::SetEntityMarkers(None));
            return;
        };
        let mut mesh = jka_assets::bsp::DebugVolumeMesh::default();
        let mut origins = Vec::with_capacity(graph.entities.len());
        for entity in &graph.entities {
            if !entity.positioned {
                origins.push(None);
                continue;
            }
            let origin = self.live_entity_origin(entity);
            origins.push(Some(origin));
            let (mins, maxs) = entity_marker_box(entity, origin);
            mesh.push_box(mins, maxs, entity.category.color());
        }
        for link in &graph.links {
            if let (Some(from), Some(to)) = (origins[link.from], origins[link.to]) {
                mesh.push_line(from, to, [0xFF, 0xA9, 0x40, 0xFF]);
            }
        }
        self.render_command(RenderCommand::SetEntityMarkers(Some(mesh)));
    }

    /// Nearest `r_drawEntities` marker box under the center crosshair, as an
    /// index into `self.entity_graph`'s entities, if the overlay is on and the
    /// aim ray hits one. Takes priority over whatever a GPU surface pick would
    /// find behind it: if the box is what's visibly under the crosshair, trace
    /// should report the entity, not the material it happens to be drawn over.
    pub(in crate::app) fn trace_entity_marker_hit_center(&self) -> Option<usize> {
        // Matches `trace_entity_hint_center`'s TRACE_DISTANCE: far enough to
        // cover any map, not so unbounded that "looking roughly that way"
        // from across the level counts as aiming at it.
        const MAX_DISTANCE: f32 = 131_072.0;
        if !self.video.draw_entities {
            return None;
        }
        let graph = self.entity_graph.as_deref()?;
        let start = scene::jka_position(self.camera.position.to_array());
        let direction = scene::jka_position(self.camera.forward().to_array());
        let mut best: Option<(usize, f32)> = None;
        for (index, entity) in graph.entities.iter().enumerate() {
            if !entity.positioned {
                continue;
            }
            let origin = self.live_entity_origin(entity);
            let (mins, maxs) = entity_marker_box(entity, origin);
            let Some(distance) = ray_aabb_distance(start, direction, mins, maxs) else {
                continue;
            };
            if distance > MAX_DISTANCE {
                continue;
            }
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((index, distance));
            }
        }
        best.map(|(index, _)| index)
    }

    /// Builds the trace menu entry for a `trace_entity_marker_hit_center` hit,
    /// straight from the static `EntityGraph` data (no GPU round trip needed).
    pub(in crate::app) fn entity_marker_trace_info(&self, index: usize) -> SurfaceInspectorInfo {
        let graph = self
            .entity_graph
            .as_deref()
            .expect("hit test found this index in the loaded graph");
        let entity = &graph.entities[index];
        let hit_entity_num = self.live_presented_entity(entity).map(|live| live.number);
        let origin = self.live_entity_origin(entity);

        let mut summary = vec![
            ("CATEGORY".to_owned(), entity.category.label().to_owned()),
            (
                "ORIGIN".to_owned(),
                format!("{:.0} {:.0} {:.0}", origin[0], origin[1], origin[2]),
            ),
        ];
        if !entity.targetname.is_empty() {
            summary.push(("TARGETNAME".to_owned(), entity.targetname.clone()));
        }
        summary.push((
            "LIVE ENTITY".to_owned(),
            match hit_entity_num {
                Some(number) => format!("#{number}"),
                None => "not currently networked".to_owned(),
            },
        ));

        let mut sections = Vec::new();
        if !entity.properties.is_empty() {
            sections.push(SurfaceInspectorSection {
                title: "KEYS".to_owned(),
                lines: entity
                    .properties
                    .iter()
                    .map(|(key, value)| format!("{key} = {value}"))
                    .collect(),
            });
        }
        let targets: Vec<String> = graph.outgoing[index]
            .iter()
            .map(|&link| {
                let link = graph.links[link];
                let target = &graph.entities[link.to];
                format!(
                    "{}  ->  {} \"{}\"",
                    link.key, target.classname, target.targetname
                )
            })
            .chain(entity.dangling.iter().map(|(key, value)| {
                format!("{key}  ->  \"{value}\" (no entity has that targetname)")
            }))
            .collect();
        if !targets.is_empty() {
            sections.push(SurfaceInspectorSection {
                title: "TARGETS".to_owned(),
                lines: targets,
            });
        }
        let targeted_by: Vec<String> = graph.incoming[index]
            .iter()
            .map(|&link| {
                let link = graph.links[link];
                format!("{}  <-  {}", link.key, graph.entities[link.from].classname)
            })
            .collect();
        if !targeted_by.is_empty() {
            sections.push(SurfaceInspectorSection {
                title: "TARGETED BY".to_owned(),
                lines: targeted_by,
            });
        }

        let mut lines = vec![format!("classname {}", entity.classname)];
        lines.extend(
            entity
                .properties
                .iter()
                .map(|(key, value)| format!("{key} = {value}")),
        );

        SurfaceInspectorInfo {
            kind: "MAP ENTITY".to_owned(),
            title: entity.classname.clone(),
            summary,
            sections,
            lines,
            hit_entity_num,
            hit_inline_model: entity.inline_model,
        }
    }

    /// Appends a trace result to the `/trace` menu history and opens it,
    /// shared by the async GPU surface pick and the synchronous entity-marker
    /// hit test.
    pub(in crate::app) fn push_trace_entry(&mut self, info: SurfaceInspectorInfo) {
        self.surface_inspector = Some(info.clone());
        const TRACE_MENU_CAP: usize = 20;
        if self.trace_menu_entries.len() >= TRACE_MENU_CAP {
            self.trace_menu_entries.remove(0);
        }
        self.trace_menu_entries.push(info);
        self.trace_menu_selected = Some(self.trace_menu_entries.len() - 1);
        self.set_overlay(OverlayMode::Trace);
        self.publish_ui();
    }

    /// jaPRO CG_ScanForCrosshairEntity plus the colour half of CG_DrawCrosshair and
    /// CG_DrawCrosshairNames. The trace is CG_CrosshairTrace: world and solid
    /// bodies, own client ignored. Static target/name scans retain the old ~30 Hz
    /// cadence; dynamic placement/autofocus additionally follow fresh subframe input.
    pub(in crate::app) fn update_crosshair_target(&mut self, now: Instant) {
        use crate::cgame::crosshair::{self, CrosshairViewer};
        use jka_movement::TraceWorld;
        const SCAN_INTERVAL: Duration = Duration::from_millis(33);
        const ENTITYNUM_WORLD: i32 = 1022;
        const MAX_CLIENTS: i32 = 32;
        const TEAM_SPECTATOR: i32 = 3;
        const STAT_HEALTH: usize = 0;
        // WP_MuzzlePoint from the vendored jaPRO/OpenJK bg_weapons.c.
        const MUZZLE: [[f32; 3]; 17] = [
            [0.0, 0.0, 0.0],
            [0.0, 8.0, 0.0],
            [0.0, 8.0, 0.0],
            [8.0, 16.0, 0.0],
            [12.0, 6.0, -6.0],
            [12.0, 6.0, -6.0],
            [12.0, 6.0, -6.0],
            [12.0, 2.0, -6.0],
            [12.0, 4.5, -6.0],
            [12.0, 6.0, -6.0],
            [12.0, 6.0, -6.0],
            [12.0, 8.0, -4.0],
            [12.0, 0.0, -4.0],
            [12.0, 0.0, -10.0],
            [12.0, 0.0, -4.0],
            [12.0, 6.0, -6.0],
            [12.0, 6.0, -6.0],
        ];

        let settings = self.crosshair;
        let session_supports_crosshair = self.game_session.as_ref().is_some_and(|session| {
            session.live
                || (!session.live
                    && session.phase == SessionPhase::Playing
                    && session.demo_index.is_some())
        });
        // A dynamic crosshair needs its trace even when target colouring/names are disabled.
        // DOF autofocus also consumes this endpoint when available.
        let active = settings.style != 0
            && (settings.dynamic != 0
                || settings.identify_target
                || settings.names != 0.0
                || (self.video.depth_of_field_strength > 0.001 && self.video.dof_autofocus))
            && matches!(
                self.overlay,
                OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs
            )
            && session_supports_crosshair;
        if !active {
            self.crosshair_seen = None;
            self.crosshair_hit_world = None;
            if self.crosshair_target != ui::UiCrosshairTarget::default() {
                self.crosshair_target = ui::UiCrosshairTarget::default();
                self.publish_transient_ui();
            }
            return;
        }
        let dof_autofocus = self.video.depth_of_field_strength > 0.001 && self.video.dof_autofocus;
        let scan_interval = if settings.dynamic != 0 || dof_autofocus {
            Duration::ZERO
        } else {
            SCAN_INTERVAL
        };
        if now.saturating_duration_since(self.crosshair_scan_at) < scan_interval {
            return;
        }
        self.crosshair_scan_at = now;

        // TaystJK uses cg.predictedPlayerState for the live viewer. Keep local
        // in-process play on its snapshot path because it intentionally has no
        // client predictor; remote live play uses the predictor when available.
        let live_ps = self.live_player_state().cloned();
        let Some(session) = self.game_session.as_ref() else {
            return;
        };
        let Some(snapshot) = session.client_game.current_snapshot() else {
            return;
        };
        let ps = if session.live {
            live_ps.as_ref().unwrap_or(&snapshot.player_state)
        } else {
            &snapshot.player_state
        };
        let entities = &session.presented_entities;
        let find = |number: i32| {
            entities
                .iter()
                .find(|entity| i32::from(entity.number) == number)
        };
        let snapshot_client = ps.field_i32("clientNum").unwrap_or(0);
        let (viewer, alive) = if session.live || self.demo_view_mode == DemoViewMode::Authoritative
        {
            (
                CrosshairViewer {
                    client: snapshot_client,
                    team: ps.persistant[3],
                    duel_in_progress: ps.field_i32("duelInProgress").unwrap_or(0) != 0,
                    duel_index: ps.field_i32("duelIndex").unwrap_or(0),
                    weapon: ps.field_i32("weapon").unwrap_or(0),
                },
                ps.stats[STAT_HEALTH] > 0,
            )
        } else {
            match self.demo_view_mode {
                DemoViewMode::Follow(client) => {
                    let state = find(client).map(|entity| &entity.state);
                    let team = usize::try_from(client)
                        .ok()
                        .and_then(|index| {
                            session
                                .client_game
                                .client_info(index, &session.siege_classes)
                        })
                        .map_or(TEAM_SPECTATOR, |info| info.team);
                    (
                        CrosshairViewer {
                            client,
                            team,
                            duel_in_progress: false,
                            duel_index: 0,
                            weapon: state
                                .and_then(|state| state.field_i32("weapon"))
                                .unwrap_or(0),
                        },
                        state
                            .and_then(|state| state.field_i32("health"))
                            .unwrap_or(1)
                            > 0,
                    )
                }
                DemoViewMode::Free => (
                    CrosshairViewer {
                        client: -1,
                        team: TEAM_SPECTATOR,
                        ..CrosshairViewer::default()
                    },
                    true,
                ),
                DemoViewMode::Authoritative => unreachable!("handled above"),
            }
        };

        // TaystJK staticCrosshairOverride(): mode 1 is unconditionally dynamic;
        // mode 2 goes static for the authored weapon/racemode/strafehelper cases.
        // WSW/Weze strafehelper add-ons are intentionally absent from this client,
        // so there are no corresponding flags to test here.
        let static_override = if settings.dynamic == 2 {
            matches!(viewer.weapon, 0 | 1 | 2 | 3)
                || ps.stats[crate::japro_cg::STAT_RACEMODE] != 0
                || self.strafe_helper.flags & (ui::SHELPER_UPDATED | ui::SHELPER_CGAZ) != 0
        } else {
            false
        };
        let dynamic = settings.dynamic != 0 && !static_override && viewer.client >= 0;

        // CG_CrosshairTrace. TaystJK deliberately uses two different angle sources in
        // third person: predictedPlayerState.viewangles for the trace direction, but
        // cg_entities[clientNum].lerpAngles for CG_CalcMuzzlePoint's weapon offset.
        // Keep that split. DinurdoJK's subframe renderer can present newer viewangles
        // than the App camera, so use its newest input angles for the trace and leave
        // final framebuffer projection to the render thread's late-latched camera.
        let policy = PlayerViewPolicyState::from_player_state(ps);
        let third_person =
            rendering_third_person(self.third_person, self.first_person_lightsaber, policy);
        let viewer_entity = session
            .audio_followed_entity
            .as_ref()
            .filter(|entity| i32::from(entity.number) == viewer.client)
            .or_else(|| find(viewer.client));
        let ps_angles = playerstate_vec3(ps, "viewangles").unwrap_or([
            -self.camera.pitch.to_degrees(),
            self.camera.yaw.to_degrees(),
            0.0,
        ]);
        let entity_angles = viewer_entity
            .map(|entity| entity.angles)
            .unwrap_or(ps_angles);

        // OpenJK/TaystJK's first-person source is cg.refdef.viewangles; third-person
        // trace direction is cg.predictedPlayerState.viewangles. With subframe input,
        // the Rust equivalent of both is the newest cl.viewangles sample that the
        // renderer itself will late-latch. Do not substitute lerpAngles here: those
        // are intentionally only the third-person muzzle-offset basis.
        let newest_input_angles = if self.live_remote_view_forced() {
            None
        } else {
            self.live_view_angles().or_else(|| {
                self.local_server
                    .as_ref()
                    .filter(|server| server.view().view_forced == 0)
                    .map(|server| server.subframe_view_angles())
            })
        };
        let refdef_angles = newest_input_angles.unwrap_or([
            -self.camera.pitch.to_degrees(),
            self.camera.yaw.to_degrees(),
            0.0,
        ]);
        let trace_angles = if third_person {
            newest_input_angles.unwrap_or(ps_angles)
        } else {
            refdef_angles
        };

        let mut hit = None;
        self.crosshair_hit_world = None;
        if let Some(world) = self.map_collision.as_mut() {
            let camera_start = scene::jka_position(self.camera.position.to_array());
            // Static crosshair scans/autofocus still refer to the current refdef
            // centre ray. Under subframe input that means the newest input angles,
            // not the previous App-camera orientation.
            let (refdef_forward, _, _) = crate::camera::angle_vectors(refdef_angles);
            let (start, dir, distance) = if dynamic {
                // CG_ScanForCrosshairEntity traces from CG_CalcMuzzlePoint along
                // predicted/refdef viewangles. CG_CalcMuzzlePoint itself uses the
                // player's lerpAngles for the third-person weapon-offset basis.
                let mut direction_angles = trace_angles;
                let emplaced_index = ps.field_i32("emplacedIndex").unwrap_or(0);
                let emplaced = viewer.weapon == 17 && emplaced_index != 0;
                if emplaced && direction_angles[0] > 40.0 {
                    direction_angles[0] = 40.0;
                }
                let (aim_forward, _, _) = crate::camera::angle_vectors(direction_angles);

                let muzzle_basis = if third_person {
                    entity_angles
                } else {
                    refdef_angles
                };
                let (muzzle_forward, muzzle_right, _) = crate::camera::angle_vectors(muzzle_basis);
                let mut muzzle = if third_person {
                    viewer_entity
                        .map(|entity| entity.origin)
                        .or_else(|| playerstate_vec3(ps, "origin"))
                        .unwrap_or(camera_start)
                } else {
                    camera_start
                };

                // CG_CalcMuzzlePoint's emplaced-gun special case. Its muzzle basis
                // is pitch-constrained independently from the trace direction.
                let mut offset_forward = muzzle_forward;
                let mut offset_right = muzzle_right;
                if emplaced {
                    if let Some(gun) = find(emplaced_index) {
                        muzzle = gun.origin;
                        muzzle[2] += 46.0;
                        let mut pitch_constraint = muzzle_basis;
                        if pitch_constraint[0] > 40.0 {
                            pitch_constraint[0] = 40.0;
                        }
                        let (forward, right, _) = crate::camera::angle_vectors(pitch_constraint);
                        offset_forward = forward;
                        offset_right = right;
                    }
                }

                let weapon = usize::try_from(viewer.weapon)
                    .ok()
                    .filter(|&weapon| weapon < MUZZLE.len())
                    .unwrap_or(0);
                let offset = if matches!(viewer.weapon, 1 | 2 | 3 | 6 | 17 | 18) {
                    [0.0; 3]
                } else {
                    MUZZLE[weapon]
                };
                muzzle[0] += offset_forward[0] * offset[0] + offset_right[0] * offset[1];
                muzzle[1] += offset_forward[1] * offset[0] + offset_right[1] * offset[1];
                muzzle[2] += offset_forward[2] * offset[0] + offset_right[2] * offset[1];
                if !emplaced {
                    if third_person {
                        muzzle[2] += ps.field_i32("viewheight").unwrap_or(0) as f32 + offset[2];
                    } else {
                        muzzle[2] += offset[2];
                    }
                }
                (muzzle, aim_forward, self.map_distance_cull.max(1.0))
            } else {
                (camera_start, refdef_forward, 131_072.0)
            };
            let end = std::array::from_fn(|axis| start[axis] + dir[axis] * distance);
            let solids = crate::net::solid_entities(entities, snapshot.server_time);
            let result = crate::net::PredictionWorld {
                world,
                solids: &solids,
                client_num: viewer.client,
            }
            .trace(jka_movement::TraceQuery {
                start,
                mins: [0.0; 3],
                maxs: [0.0; 3],
                end,
                pass_entity: viewer.client,
                mask: 0x1 | 0x100,
            });
            let render_end = scene::render_position(result.end);
            self.crosshair_hit_world = Some(render_end);
            if result.fraction < 1.0 && result.entity >= 0 {
                hit = Some(result.entity);
            }
        }
        // Dynamic placement remains in world space. RenderSnapshot/LatestViewState
        // pair this endpoint with the same input angles and the render thread projects
        // it only after the final camera latch.

        let viewer_state = find(viewer.client).map(|entity| &entity.state);
        let hit_entity = hit
            .filter(|&number| number < ENTITYNUM_WORLD)
            .and_then(find);
        let tricked = hit_entity.is_some_and(|entity| {
            i32::from(entity.number) < MAX_CLIENTS
                && crosshair::is_mind_tricked(&entity.state, viewer.client, viewer_state)
        });

        let mut seen = self.crosshair_seen.map(|seen| CrosshairSeen {
            on_target: false,
            ..seen
        });
        let mut color = None;
        if tricked {
            if seen.is_some_and(|seen| seen.entity == hit.unwrap_or(-1)) {
                seen = None;
            }
        } else if let Some(entity) = hit_entity {
            let identified = crosshair::identify_client(entity);
            if viewer.team != TEAM_SPECTATOR {
                let (target, is_pilot) = identified.unwrap_or((i32::from(entity.number), false));
                seen = Some(CrosshairSeen {
                    entity: target,
                    is_pilot,
                    at: now,
                    on_target: true,
                });
                color = crosshair::crosshair_color(&session.client_game, &viewer, entity);
            } else if let Some((target, is_pilot)) =
                identified.filter(|_| i32::from(entity.number) < MAX_CLIENTS)
            {
                seen = Some(CrosshairSeen {
                    entity: target,
                    is_pilot,
                    at: now,
                    on_target: true,
                });
            }
        }

        let identify = if settings.identify_target {
            color
        } else {
            None
        };
        let name = seen
            .filter(|_| alive || viewer.team == TEAM_SPECTATOR)
            .filter(|_| !self.scores_showing)
            .and_then(|seen| {
                let age_ms = i32::try_from(now.saturating_duration_since(seen.at).as_millis())
                    .unwrap_or(i32::MAX);
                let alpha = crosshair::name_alpha(settings.names, age_ms, seen.on_target)?;
                let state = find(seen.entity).map(|entity| &entity.state);
                crosshair::crosshair_name(
                    &session.client_game,
                    &viewer,
                    seen.entity,
                    seen.is_pilot,
                    state,
                    alpha,
                    settings.names_colours,
                    settings.names_opacity,
                )
            })
            .map(|name| ui::UiCrosshairName {
                text: name.text,
                color: name.color,
                alpha: name.alpha,
            });

        self.crosshair_seen = seen;
        let target = ui::UiCrosshairTarget {
            color: identify,
            dynamic,
            name,
        };
        if target != self.crosshair_target {
            self.crosshair_target = target;
            self.publish_transient_ui();
        }
    }

    pub(in crate::app) fn trace_entity_hint_center(&mut self) -> Option<InspectorEntityHint> {
        use jka_movement::TraceWorld;
        const ENTITYNUM_WORLD: i32 = 1022;
        const TRACE_DISTANCE: f32 = 131_072.0;
        // OpenJK MASK_SHOT: SOLID | BODY | CORPSE | TERRAIN. This deliberately
        // does not make non-solid trigger volumes steal the ordinary trace.
        const MASK_SHOT: i32 = 0x1 | 0x100 | 0x200 | 0x1000;

        // Finish borrowing the live CGame state before taking map_collision
        // mutably. Besides being borrow-checker friendly, this makes it clear
        // that the trace consumes a point-in-time entity collision snapshot.
        let (client_num, solids) = {
            let session = self.game_session.as_ref()?;
            let snapshot = session.client_game.current_snapshot()?;
            let client_num = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
            let solids =
                crate::net::solid_entities(&session.presented_entities, snapshot.server_time);
            (client_num, solids)
        };
        let start = scene::jka_position(self.camera.position.to_array());
        let direction = scene::jka_position(self.camera.forward().to_array());
        let end = std::array::from_fn(|axis| start[axis] + direction[axis] * TRACE_DISTANCE);
        let world = self.map_collision.as_mut()?;
        let trace = crate::net::PredictionWorld {
            world,
            solids: &solids,
            client_num,
        }
        .trace(jka_movement::TraceQuery {
            start,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end,
            pass_entity: client_num,
            mask: MASK_SHOT,
        });
        if trace.fraction >= 1.0 || trace.entity < 0 || trace.entity >= ENTITYNUM_WORLD {
            return None;
        }
        Some(InspectorEntityHint {
            entity_num: u16::try_from(trace.entity).ok()?,
            distance: TRACE_DISTANCE * trace.fraction.clamp(0.0, 1.0),
            hit: scene::render_position(trace.end),
        })
    }

    pub(in crate::app) fn trace_surface_center(&mut self) {
        // The trace bind toggles: press again while the menu is open to
        // close it instead of sampling and adding another entry.
        if self.overlay == OverlayMode::Trace {
            self.close_trace_overlay();
            return;
        }
        let size = self
            .window
            .as_ref()
            .map(|window| window.inner_size())
            .unwrap_or_default();
        if size.width == 0 || size.height == 0 {
            self.console_status = "TRACE UNAVAILABLE: WINDOW HAS NO RENDER SIZE".into();
            self.publish_ui();
            return;
        }

        // An `r_drawEntities` marker box under the crosshair wins over
        // whatever the GPU surface pick would find behind it.
        if let Some(index) = self.trace_entity_marker_hit_center() {
            let info = self.entity_marker_trace_info(index);
            self.push_trace_entry(info);
            return;
        }

        let entity_hint = self.trace_entity_hint_center();
        self.render_command(RenderCommand::InspectSurface {
            x: size.width as f32 * 0.5,
            y: size.height as f32 * 0.5,
            width: size.width,
            height: size.height,
            entity_hint,
        });
    }

    pub(in crate::app) fn trace_entity_kind_label(kind: EntityPresentationKind) -> &'static str {
        match kind {
            EntityPresentationKind::General => "GENERAL",
            EntityPresentationKind::Player => "PLAYER",
            EntityPresentationKind::Item => "ITEM",
            EntityPresentationKind::Missile => "MISSILE",
            EntityPresentationKind::Special => "SPECIAL",
            EntityPresentationKind::Holocron => "HOLOCRON",
            EntityPresentationKind::Mover => "MOVER",
            EntityPresentationKind::Beam => "BEAM",
            EntityPresentationKind::Portal => "PORTAL",
            EntityPresentationKind::Speaker => "SPEAKER",
            EntityPresentationKind::PushTrigger => "PUSH TRIGGER",
            EntityPresentationKind::TeleportTrigger => "TELEPORT TRIGGER",
            EntityPresentationKind::Invisible => "INVISIBLE",
            EntityPresentationKind::Npc => "NPC",
            EntityPresentationKind::Team => "TEAM",
            EntityPresentationKind::Body => "BODY",
            EntityPresentationKind::Terrain => "TERRAIN",
            EntityPresentationKind::Fx => "FX",
            EntityPresentationKind::Event => "EVENT",
            EntityPresentationKind::Unknown => "UNKNOWN",
        }
    }

    pub(in crate::app) fn trace_item_type_label(item_type: i32) -> &'static str {
        // itemType_t ordering from bg_public.h / bg_itemlist.
        match item_type {
            1 => "WEAPON",
            2 => "AMMO",
            3 => "ARMOR",
            4 => "HEALTH",
            5 => "POWERUP",
            6 => "HOLDABLE",
            7 => "PERSISTANT POWERUP",
            8 => "TEAM",
            _ => "OTHER",
        }
    }

    /// Spawnflag names are class-specific. Keep this table tied to the same
    /// OpenJK-derived bits used by the local-server ports instead of pretending
    /// the numeric value has one global meaning.
    pub(in crate::app) fn trace_spawnflag_labels(classname: &str, flags: i32) -> Vec<String> {
        let known: &[(i32, &str)] = match classname.to_ascii_lowercase().as_str() {
            "func_door" | "func_plat" | "func_button" => &[
                (1, "START_OPEN"),
                (2, "FORCE_ACTIVATE"),
                (4, "CRUSHER"),
                (8, "TOGGLE"),
                (16, "LOCKED"),
                (64, "PLAYER_USE"),
                (128, "INACTIVE"),
            ],
            "trigger_multiple" => &[
                (1, "PLAYER_ONLY"),
                (2, "FACING"),
                (4, "USE_BUTTON"),
                (8, "FIRE_BUTTON"),
                (16, "NPC_ONLY"),
                (32, "LIMITED_PILOT"),
                (128, "INACTIVE"),
                (2048, "MULTIPLE"),
            ],
            "trigger_once" => &[
                (1, "PLAYER_ONLY"),
                (2, "FACING"),
                (4, "USE_BUTTON"),
                (8, "FIRE_BUTTON"),
                (16, "NPC_ONLY"),
                (128, "INACTIVE"),
                (2048, "MULTIPLE"),
            ],
            "func_static" => &[(1, "FORCE_PUSH"), (2, "FORCE_PULL"), (4, "SHADER_ANIM")],
            "func_rotating" => &[(2, "RADAR"), (4, "Z_AXIS"), (8, "X_AXIS")],
            "func_bobbing" => &[(1, "X_AXIS"), (2, "Y_AXIS")],
            "func_train" => &[(1, "START_ON")],
            "func_usable" | "func_wall" => &[(1, "START_OFF")],
            "target_delay" => &[(1, "NO_RETRIGGER")],
            _ => &[],
        };
        let mut labels = Vec::new();
        let mut described = 0_i32;
        for &(bit, label) in known {
            described |= bit;
            if flags & bit != 0 {
                labels.push(label.to_owned());
            }
        }
        let unknown = flags & !described;
        for bit in 0..31 {
            let mask = 1_i32 << bit;
            if unknown & mask != 0 {
                labels.push(format!("BIT_{bit}"));
            }
        }
        labels
    }

    pub(in crate::app) fn enrich_surface_inspector(&self, info: &mut SurfaceInspectorInfo) {
        let Some(entity_num) = info.hit_entity_num else {
            return;
        };
        let Some(session) = self.game_session.as_ref() else {
            return;
        };
        let Some(entity) = session
            .presented_entities
            .iter()
            .find(|entity| entity.number == entity_num)
        else {
            return;
        };

        const SOLID_BMODEL: i32 = 0x00ff_ffff;
        let state = &entity.state;
        let kind = entity.presentation_kind();
        let kind_label = Self::trace_entity_kind_label(kind);
        let model_index = state.field_i32("modelindex").unwrap_or(0);
        let model_index2 = state.field_i32("modelindex2").unwrap_or(0);
        let solid = state.field_i32("solid").unwrap_or(0);
        let is_bmodel = solid == SOLID_BMODEL && model_index > 0;
        if is_bmodel {
            info.hit_inline_model = u32::try_from(model_index).ok();
        }

        // modelindex is not globally a CS_MODELS index. In particular ET_ITEM
        // indexes bg_itemlist. Resolve the display model using the same semantic
        // source as the presenter so the inspector does not print plausible but
        // wrong qpaths.
        let item = (kind == EntityPresentationKind::Item)
            .then(|| jka_movement::bg_item(model_index))
            .flatten();
        let actor_info = match kind {
            EntityPresentationKind::Player => {
                let client_num = state
                    .field_i32("clientNum")
                    .unwrap_or(i32::from(entity_num));
                usize::try_from(client_num).ok().and_then(|client_num| {
                    session
                        .client_game
                        .client_info(client_num, &session.siege_classes)
                })
            }
            EntityPresentationKind::Npc => session.client_game.npc_client_info(state).ok(),
            _ => None,
        };
        let registered_model = match kind {
            EntityPresentationKind::General
            | EntityPresentationKind::Mover
            | EntityPresentationKind::Holocron => session.client_game.model_qpath(model_index),
            _ => None,
        };
        let model_display = if is_bmodel {
            Some(format!("*{model_index}  (inline BSP model)"))
        } else if let Some(item) = item.as_ref() {
            (!item.world_model.is_empty()).then(|| item.world_model.clone())
        } else if let Some(actor) = actor_info.as_ref() {
            let skin = if actor.skin_name.is_empty() {
                "default"
            } else {
                actor.skin_name.as_str()
            };
            Some(format!("{}  ·  skin {skin}", actor.model_qpath()))
        } else {
            registered_model.clone()
        };

        // Authored map entities are keyed most reliably by their inline model
        // number. For model entities, fall back to model/model2 qpath and use
        // origin only as the tie-breaker when a map reuses an asset.
        let graph_match = self.entity_graph.as_deref().and_then(|graph| {
            let inline_name = is_bmodel.then(|| format!("*{model_index}"));
            let model = registered_model.as_deref();
            graph
                .entities
                .iter()
                .enumerate()
                .filter(|(_, graph_entity)| {
                    if let Some(inline_name) = inline_name.as_deref() {
                        return graph_entity
                            .property("model")
                            .is_some_and(|value| value.eq_ignore_ascii_case(inline_name));
                    }
                    let Some(model) = model else { return false };
                    graph_entity
                        .property("model")
                        .is_some_and(|value| value.eq_ignore_ascii_case(model))
                        || graph_entity
                            .property("model2")
                            .is_some_and(|value| value.eq_ignore_ascii_case(model))
                })
                .min_by(|(_, a), (_, b)| {
                    let da = (0..3)
                        .map(|axis| (a.origin[axis] - entity.origin[axis]).powi(2))
                        .sum::<f32>();
                    let db = (0..3)
                        .map(|axis| (b.origin[axis] - entity.origin[axis]).powi(2))
                        .sum::<f32>();
                    da.total_cmp(&db)
                })
        });

        let rendered_material = info
            .summary
            .iter()
            .find(|(label, _)| label == "MATERIAL")
            .map(|(_, value)| value.clone());
        let distance = info
            .summary
            .iter()
            .find(|(label, _)| label == "DISTANCE")
            .map(|(_, value)| value.clone());

        let mut title = format!("{kind_label}  ·  ENTITY #{entity_num}");
        let mut classname = item.as_ref().map(|item| item.classname.clone());
        let mut target = None;
        let mut targetname = None;
        let mut spawnflag_summary = None;
        if let Some((_, graph_entity)) = graph_match {
            if !graph_entity.classname.is_empty() {
                classname = Some(graph_entity.classname.clone());
                title = format!("{}  ·  ENTITY #{entity_num}", graph_entity.classname);
            }
            target = graph_entity.property("target").map(str::to_owned);
            targetname = graph_entity.property("targetname").map(str::to_owned);
            let spawnflags = graph_entity
                .property("spawnflags")
                .and_then(|value| value.trim().parse::<i32>().ok())
                .unwrap_or(0);
            if spawnflags != 0 {
                let labels = Self::trace_spawnflag_labels(&graph_entity.classname, spawnflags);
                spawnflag_summary = Some(if labels.is_empty() {
                    format!("{spawnflags} (0x{spawnflags:x})")
                } else {
                    format!("{spawnflags}  ·  {}", labels.join(" | "))
                });
            }
        }

        if let Some(actor) = actor_info.as_ref() {
            if !actor.name.is_empty() && kind == EntityPresentationKind::Player {
                title = format!("{}  ·  PLAYER #{entity_num}", actor.name);
            }
        }

        let mut summary = vec![(
            "ENTITY".into(),
            format!(
                "#{entity_num} · {kind_label} (eType {})",
                entity.entity_type
            ),
        )];
        if let Some(classname) = classname.as_deref() {
            summary.push(("CLASSNAME".into(), classname.to_owned()));
        }
        if let Some(model) = model_display.as_deref() {
            summary.push(("MODEL".into(), model.to_owned()));
        }
        summary.push((
            "ORIGIN".into(),
            format!(
                "{:.1}, {:.1}, {:.1}",
                entity.origin[0], entity.origin[1], entity.origin[2]
            ),
        ));
        if let Some(spawnflags) = spawnflag_summary {
            summary.push(("SPAWNFLAGS".into(), spawnflags));
        }
        if let Some(target) = target.as_deref() {
            summary.push(("TARGET".into(), target.to_owned()));
        } else if let Some(targetname) = targetname.as_deref() {
            summary.push(("TARGETNAME".into(), targetname.to_owned()));
        }
        if let Some(distance) = distance {
            summary.push(("DISTANCE".into(), distance));
        }
        if let Some(material) = rendered_material {
            summary.push(("MATERIAL".into(), material));
        }
        info.title = title;
        info.kind = if is_bmodel {
            "BRUSH ENTITY".into()
        } else if model_display
            .as_deref()
            .is_some_and(|model| model.to_ascii_lowercase().contains(".md3"))
        {
            "ENTITY · MD3".into()
        } else if kind == EntityPresentationKind::Player {
            "PLAYER".into()
        } else if kind == EntityPresentationKind::Npc {
            "NPC".into()
        } else if kind == EntityPresentationKind::Item {
            "ITEM".into()
        } else {
            "ENTITY".into()
        };
        info.summary = summary;

        let solid_text = if solid == SOLID_BMODEL {
            format!("BMODEL (*{model_index})")
        } else if solid == 0 {
            "not solid".into()
        } else {
            // Same entityState solid unpack as net::solid_entities / OpenJK.
            let x = (solid & 255) as f32;
            let zd = ((solid >> 8) & 255) as f32;
            let zu = ((solid >> 16) & 255) as f32 - 32.0;
            format!("bbox mins -{x:.0},-{x:.0},-{zd:.0}  maxs {x:.0},{x:.0},{zu:.0}")
        };
        let mut entity_lines = vec![
            format!(
                "Origin: {:.1}, {:.1}, {:.1}",
                entity.origin[0], entity.origin[1], entity.origin[2]
            ),
            format!(
                "Angles: {:.1}, {:.1}, {:.1}",
                entity.angles[0], entity.angles[1], entity.angles[2]
            ),
            format!("Solid: {solid_text}"),
        ];
        if let Some(model) = model_display.as_deref() {
            entity_lines.push(format!("Model: {model} · modelindex {model_index}"));
        } else if model_index != 0 {
            entity_lines.push(format!("modelindex: {model_index}"));
        }
        if model_index2 != 0 {
            let second_model = session.client_game.model_qpath(model_index2).map_or_else(
                || model_index2.to_string(),
                |model| format!("{model_index2} · {model}"),
            );
            entity_lines.push(format!("modelindex2: {second_model}"));
        }
        if let Some(item) = item.as_ref() {
            entity_lines.push(format!(
                "Item: {} · {} (type {}) · tag {} · quantity {}",
                item.classname,
                Self::trace_item_type_label(item.item_type),
                item.item_type,
                item.tag,
                item.quantity
            ));
            if !item.world_model2.is_empty() {
                entity_lines.push(format!("Secondary item model: {}", item.world_model2));
            }
        }
        if let Some(actor) = actor_info.as_ref() {
            let skin = if actor.skin_name.is_empty() {
                "default"
            } else {
                actor.skin_name.as_str()
            };
            entity_lines.push(format!("Actor visual: {} / {skin}", actor.model_name));
            if !actor.saber_name.is_empty() || !actor.saber2_name.is_empty() {
                entity_lines.push(format!(
                    "Sabers: {} / {}",
                    actor.saber_name, actor.saber2_name
                ));
            }
        }
        let frame = state.field_i32("frame").unwrap_or(0);
        let scale = state.field_i32("iModelScale").unwrap_or(0);
        if frame != 0 || scale != 0 {
            entity_lines.push(format!(
                "Frame: {frame} · model scale: {}%",
                if scale == 0 { 100 } else { scale }
            ));
        }
        let health = state.field_i32("health").unwrap_or(0);
        let maxhealth = state.field_i32("maxhealth").unwrap_or(0);
        let weapon = state.field_i32("weapon").unwrap_or(0);
        if health != 0 || maxhealth != 0 || weapon != 0 {
            entity_lines.push(format!("Health: {health}/{maxhealth} · weapon: {weapon}"));
        }
        let owner = state.field_i32("owner").unwrap_or(0);
        let eflags = state.field_i32("eFlags").unwrap_or(0);
        if owner != 0 || eflags != 0 {
            entity_lines.push(format!("Owner: {owner} · eFlags: 0x{eflags:08x}"));
        }
        info.sections.insert(
            0,
            SurfaceInspectorSection {
                title: "ENTITY STATE".into(),
                lines: entity_lines.clone(),
            },
        );

        if let Some((graph_index, graph_entity)) = graph_match {
            let spawnflags = graph_entity
                .property("spawnflags")
                .and_then(|value| value.trim().parse::<i32>().ok())
                .unwrap_or(0);
            let flag_labels = Self::trace_spawnflag_labels(&graph_entity.classname, spawnflags);
            let mut spawn_lines = vec![format!(
                "BSP entity #{} · {}",
                graph_entity.bsp_index, graph_entity.classname
            )];
            spawn_lines.push(format!(
                "Authored origin: {:.1}, {:.1}, {:.1}",
                graph_entity.origin[0], graph_entity.origin[1], graph_entity.origin[2]
            ));
            if spawnflags != 0 {
                spawn_lines.push(format!(
                    "spawnflags: {spawnflags} (0x{spawnflags:x}){}",
                    if flag_labels.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", flag_labels.join(" | "))
                    }
                ));
            }
            if let Some((mins, maxs)) = graph_entity.bounds {
                spawn_lines.push(format!(
                    "Brush bounds: [{:.1}, {:.1}, {:.1}] → [{:.1}, {:.1}, {:.1}]  ·  size {:.1} × {:.1} × {:.1}",
                    mins[0], mins[1], mins[2], maxs[0], maxs[1], maxs[2],
                    maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]
                ));
            }
            for key in [
                "model",
                "model2",
                "targetname",
                "target",
                "target2",
                "killtarget",
                "team",
                "speed",
                "wait",
                "delay",
                "random",
                "lip",
                "height",
                "health",
                "dmg",
                "count",
                "radius",
                "message",
                "soundset",
            ] {
                if let Some(value) = graph_entity
                    .property(key)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    spawn_lines.push(format!("{key}: {value}"));
                }
            }
            if let Some(graph) = self.entity_graph.as_deref() {
                let outgoing = graph
                    .outgoing
                    .get(graph_index)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let incoming = graph
                    .incoming
                    .get(graph_index)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if !outgoing.is_empty() || !incoming.is_empty() {
                    spawn_lines.push(format!(
                        "Target links: {} outgoing · {} incoming",
                        outgoing.len(),
                        incoming.len()
                    ));
                    for &link_index in outgoing.iter().take(4) {
                        if let Some(link) = graph.links.get(link_index) {
                            if let Some(to) = graph.entities.get(link.to) {
                                let target_name = (!to.targetname.is_empty())
                                    .then(|| format!(" ({})", to.targetname))
                                    .unwrap_or_default();
                                spawn_lines.push(format!(
                                    "{} → BSP #{} {}{}",
                                    link.key, to.bsp_index, to.classname, target_name
                                ));
                            }
                        }
                    }
                }
            }
            info.sections.insert(
                1,
                SurfaceInspectorSection {
                    title: "MAP SPAWN".into(),
                    lines: spawn_lines.clone(),
                },
            );
            info.lines.push(format!(
                "MAP ENTITY: BSP #{} {}",
                graph_entity.bsp_index, graph_entity.classname
            ));
            info.lines
                .extend(spawn_lines.into_iter().map(|line| format!("MAP: {line}")));
            info.lines.push("MAP SPAWN VARS:".into());
            info.lines.extend(
                graph_entity
                    .properties
                    .iter()
                    .map(|(key, value)| format!("  {key} = {value}")),
            );
        }
        info.lines.extend(
            entity_lines
                .into_iter()
                .map(|line| format!("ENTITY: {line}")),
        );
    }

    /// Clears the current trace selection and the whole `/trace` menu history.
    pub(in crate::app) fn forget_trace(&mut self) {
        self.surface_inspector = None;
        self.trace_menu_entries.clear();
        self.trace_menu_selected = None;
    }

    pub(in crate::app) fn clear_surface_inspection(&mut self) {
        self.forget_trace();
        self.render_command(RenderCommand::ClearSurfaceInspection);
        if self.overlay == OverlayMode::Trace {
            self.set_overlay(OverlayMode::None);
        }
        self.publish_ui();
    }

    /// Closes the `/trace` popup (Q-to-toggle, Escape, or its window's close
    /// button) without wiping the trace history — just the selection, so
    /// reopening still shows everything traced this session. The highlighted
    /// face/surface in the 3D view (`inspector_vertex_range`) is also cleared;
    /// leaving it selected with no inspector open to explain it is confusing.
    pub(in crate::app) fn close_trace_overlay(&mut self) {
        self.set_overlay(OverlayMode::None);
        self.render_command(RenderCommand::ClearSurfaceInspection);
    }

    pub(in crate::app) fn copy_surface_inspector_text(&mut self) {
        let Some(info) = &self.surface_inspector else {
            return;
        };
        let mut text = format!("TRACE INSPECTOR [{}]\n{}\n", info.kind, info.title);
        if !info.summary.is_empty() {
            text.push_str("\nSUMMARY\n");
            for (label, value) in &info.summary {
                text.push_str(label);
                text.push_str(": ");
                text.push_str(value);
                text.push('\n');
            }
        }
        for section in &info.sections {
            text.push('\n');
            text.push_str(&section.title);
            text.push('\n');
            for line in &section.lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        if !info.lines.is_empty() {
            text.push_str("\nFULL DIAGNOSTICS\n");
            for line in &info.lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        match crate::clipboard::set_text(text.trim_end()) {
            Ok(()) => {
                self.console_status = "TRACE INSPECTOR COPIED TO CLIPBOARD".into();
            }
            Err(error) => {
                self.console_status = format!("CLIPBOARD ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
        self.publish_ui();
    }
}
