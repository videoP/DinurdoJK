//! Developer "riversource" patches on the render thread: spawn, per-frame
//! native solver encoding and the water body's per-frame inputs. See
//! river_runtime.rs and river_surface.rs.
use crate::river_runtime::LiveRiver;
use crate::river_surface::{RiverSurface, ViewUniform};

/// A presented player as a JKA player hull (bg_public.h: mins -15,-15,-24;
/// maxs 15,15,40, or 16 crouched) in map units. Actor origins and velocities
/// are render space.
fn river_body(actor: &crate::ocean::interaction::ActorSample) -> crate::river_runtime::MapBody {
    use jka_render_core::coordinates::jka_position;
    let origin = jka_position(actor.origin);
    // CROUCH_VIEWHEIGHT is 12, DEFAULT_VIEWHEIGHT 36 (remote records use 26).
    let top = if actor.head_height < 20.0 { 16.0 } else { 40.0 };
    crate::river_runtime::MapBody {
        mins: [origin[0] - 15.0, origin[1] - 15.0, origin[2] - 24.0],
        maxs: [origin[0] + 15.0, origin[1] + 15.0, origin[2] + top],
        velocity: actor.velocity.map_or([0.0; 3], jka_position),
    }
}

impl super::Renderer {
    pub(in crate::renderer) fn spawn_live_river(
        &mut self,
        definition: crate::river_authoring::Definition,
        collision: &[jka_assets::bsp::river::CollisionSurface],
    ) -> Result<(), String> {
        let source = definition.source;
        let spawn_started = (crate::river_runtime::debug_mode() > 0).then(std::time::Instant::now);
        if spawn_started.is_some() { eprintln!("[RIVER DBG] preparing source center={:?} collision_triangles={} (render-thread spawn)", source.center, collision.len()); }
        if self.river_surface.is_none() {
            let mut surface = match RiverSurface::new(&self.device, &self.queue) {
                Ok(surface) => surface,
                Err(error) => {
                    if let Some(started) = spawn_started { eprintln!("[RIVER DBG] first-use resources failed after {:.2}ms", started.elapsed().as_secs_f64() * 1000.0); }
                    eprintln!("[RIVER] water resources failed: {error}");
                    return Err(error);
                }
            };
            // An ordinary linear world texture: JKA decode, mip chain, r_picmip.
            match super::textures::try_load_texture_asset(
                &self.base,
                self.detail_texture_game.as_deref(),
                crate::river_surface::FOAM_DETAIL_TEXTURE,
                false,
                true,
                false,
                "River foam detail",
            ) {
                Some(data) => {
                    let image = super::textures::upload_texture_picmip(
                        &self.device,
                        &self.queue,
                        &data,
                        self.picmip,
                    );
                    surface.set_foam_detail(image.view.clone());
                }
                None => eprintln!(
                    "[RIVER] {} not found; water draws without foam",
                    crate::river_surface::FOAM_DETAIL_TEXTURE
                ),
            }
            self.river_surface = Some(surface);
        }
        if let Some(surface) = &mut self.river_surface {
            let faces = self.world.as_ref().and_then(|world| {
                world.primary_skybox.and_then(|indices| {
                    let [Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)] =
                        indices.map(|i| world.textures.get(i).map(|t| &t.view))
                    else {
                        return None;
                    };
                    Some([a, b, c, d, e, f])
                })
            });
            surface.set_map_sky(&self.device, faces);
        }
        let grid = self
            .world
            .as_ref()
            .and_then(|world| world.entity_light_grid.as_ref());
        let light = |point: [f32; 3]| grid.and_then(|grid| grid.sample_classic_entity_light(point));
        let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
        match LiveRiver::spawn(
            &self.device,
            &self.queue,
            collision,
            source,
            light,
            map_toward_sun,
        ) {
            Ok(mut river) => {
                if let Some(started) = spawn_started { eprintln!("[RIVER DBG] source ready; spawn_cpu={:.2}ms; GPU initialization submitted (not waited)", started.elapsed().as_secs_f64() * 1000.0); }
                river.id = definition.id;
                river.set_settings(&self.queue, definition.appearance);
                eprintln!(
                    "[RIVER] source at {:?}: {}x{} cells, {:.1} units/cell",
                    source.center,
                    river.grid.dimensions[0],
                    river.grid.dimensions[1],
                    river.grid.cell_size[0] / jka_water::river::CENTIMETRES_PER_MAP_UNIT
                );
                if let Some(existing) = self.live_rivers.iter_mut().find(|r| r.id == definition.id)
                {
                    *existing = river;
                } else {
                    self.live_rivers.push(river);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(started) = spawn_started { eprintln!("[RIVER DBG] source creation failed after {:.2}ms", started.elapsed().as_secs_f64() * 1000.0); }
                Err(error)
            }
        }
    }

    pub(in crate::renderer) fn clear_live_rivers(&mut self) {
        self.live_rivers.clear();
    }

    /// Native frame for each patch and the shared FFT detail, before any scene
    /// pass, plus the water body's pipeline/copy/view for this frame's target.
    /// `scene_format` is the scene pass's colour format; `copyable` is false
    /// when that target is the swapchain (no post, no MSAA).
    pub(in crate::renderer) fn encode_live_rivers(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame_delta: f32,
        camera: &super::Camera,
        scene_format: wgpu::TextureFormat,
        copyable: bool,
    ) {
        let Some(water) = self
            .river_surface
            .as_mut()
            .filter(|_| !self.live_rivers.is_empty())
        else {
            return;
        };
        let delta = if frame_delta.is_finite() {
            frame_delta.max(0.0)
        } else {
            0.0
        };
        water.clock += f64::from(delta);
        let clock = water.clock;
        if let Err(error) = water.encode_ocean(&self.device, encoder, clock as f32) {
            eprintln!("[RIVER] FFT detail failed, patches removed: {error}");
            self.live_rivers.clear();
            return;
        }
        water.ensure_pipeline(
            &self.device,
            &self.camera_layout,
            &self.shadow_receiver_layout,
            scene_format,
            self.msaa_samples,
            super::depth_format(),
            super::camera_depth_compare(),
        );
        let size = [
            self.targets._scene_texture.width(),
            self.targets._scene_texture.height(),
        ];
        let blue_water_ambient = self.sky_backend == 1 && self.blue_water_sky.is_some();
        let foam = water.has_foam_detail();
        {
            let sun = self.river_scene_sun();
            let view = ViewUniform::new(
                camera,
                size,
                sun,
                blue_water_ambient,
                clock as f32,
                delta.min(0.1),
                foam,
            );
            let water = self.river_surface.as_mut().expect("checked above");
            water.prepare(&self.device, &self.queue, size, self.msaa_samples, &view);
        }
        let water = self.river_surface.as_mut().expect("checked above");
        if !copyable && !water.warned_unsampleable {
            water.warned_unsampleable = true;
            eprintln!("[RIVER] the water look needs post-processing or MSAA (the scene must be copyable); not drawn");
        }
        // Sparse (long-distance) rivers keep their presentation window around
        // the camera; dense patches ignore this.
        {
            let eye = jka_render_core::coordinates::jka_position(camera.position.to_array());
            let grid = self.world.as_ref().and_then(|world| world.entity_light_grid.as_ref());
            let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
            for river in &mut self.live_rivers {
                river.update_view(
                    &self.device,
                    &self.queue,
                    eye,
                    |point| grid.and_then(|grid| grid.sample_classic_entity_light(point)),
                    map_toward_sun,
                );
            }
        }
        // Player wake: the Water menu's "Player water deformation" toggle and
        // "Deformation strength" (1 = native body velocity). Players are the
        // presented records the ocean reads (filled while cg_waterwake or
        // ocean interaction is on); each is a JKA player hull.
        {
            let (enabled, strength) = self.river_interaction;
            let bodies: Vec<crate::river_runtime::MapBody> = if enabled {
                self.river_actors.iter().map(river_body).collect()
            } else {
                Vec::new()
            };
            for river in &mut self.live_rivers {
                river.body_source = (enabled, self.river_actors.len());
                river.set_bodies(&self.device, &self.queue, &bodies, strength);
            }
        }
        let water = self.river_surface.as_ref();
        let device = &self.device;
        let profiler = &mut self.gpu_profiler;
        let mut solve_timestamp = |encoder: &mut wgpu::CommandEncoder, end: bool| {
            profiler.write_encoder_timestamp(encoder, super::GpuPass::RiverSolve, end);
        };
        self.live_rivers.retain_mut(|river| {
            match river.encode_frame(device, encoder, frame_delta, clock, water, &mut solve_timestamp) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("[RIVER] patch removed: {error}");
                    false
                }
            }
        });
        // Water mesh quadtree beyond the camera window: per-view traversal
        // (frustum side planes of this camera; depth planes are not tested).
        if let Some(water) = self.river_surface.as_ref() {
            let view_proj = camera.view_projection(size[0], size[1]);
            let planes = [
                view_proj.row(3) + view_proj.row(0),
                view_proj.row(3) - view_proj.row(0),
                view_proj.row(3) + view_proj.row(1),
                view_proj.row(3) - view_proj.row(1),
            ];
            let cm = f64::from(jka_water::river::CENTIMETRES_PER_MAP_UNIT);
            let frustum = |centre: [f64; 3], extent: [f64; 3]| {
                // River cm (JKA axes) -> render space map units: (x, z, -y).
                let c = glam::Vec3::new((centre[0] / cm) as f32, (centre[2] / cm) as f32, (-centre[1] / cm) as f32);
                let e = glam::Vec3::new((extent[0] / cm) as f32, (extent[2] / cm) as f32, (extent[1] / cm) as f32) * 1.05;
                planes.iter().all(|p| p.truncate().dot(c) + p.w + p.truncate().abs().dot(e) >= 0.0)
            };
            let eye = jka_render_core::coordinates::jka_position(camera.position.to_array()).map(|v| f64::from(v) * cm);
            let view = jka_river::river_far::FarView { eye_cm: eye, frustum: &frustum, lod_morphing: jka_river::river_far::lod_morph() };
            let grid = self.world.as_ref().and_then(|world| world.entity_light_grid.as_ref());
            let map_toward_sun = grid.and_then(|grid| grid.map_toward_sun());
            let mut light = |point: [f32; 3]| grid.and_then(|grid| grid.sample_classic_entity_light(point));
            for river in &mut self.live_rivers {
                river.prepare_far(&self.device, &self.queue, water, &view, &mut light, map_toward_sun);
            }
        }
        let debug = crate::river_runtime::debug_mode();
        if debug > 0 {
            let pipeline_ready = self.river_surface.as_ref().is_some_and(|surface| surface.ready_for(scene_format, self.msaa_samples));
            for (index, river) in self.live_rivers.iter_mut().enumerate() {
                river.debug_report(index, debug, copyable, pipeline_ready);
            }
        }
    }
}
