//! Static ao runtime.
use crate::renderer::{
    create_fast_world_bind_group, create_world_bind_group, detail_texture_for_source, mpsc,
    run_static_ao_worker, sync_auto4_bind_groups, thread, upload_texture, GpuImage, Renderer,
    StaticAoBakeData, StaticAoBakeMode, StaticAoBakedLightmap, StaticAoJobKey, StaticAoPendingJob,
    StaticAoWorkerUpdate, TextureData, TryRecvError, WorldGpu,
};

impl Renderer {
    pub(in crate::renderer) fn static_ao_mode(&self) -> Option<StaticAoBakeMode> {
        self.static_bsp_ao_enabled
            .then_some(StaticAoBakeMode::Lightmap)
    }

    pub(in crate::renderer) fn desired_static_ao_key(&self) -> Option<StaticAoJobKey> {
        let mode = self.static_ao_mode()?;
        let source = self.world.as_ref()?.static_ao_source.as_ref()?;
        Some(StaticAoJobKey {
            map_hash: source.cache.map_hash,
            mode,
            samples: self.static_bsp_ao_samples,
            scale: self.static_bsp_ao_resolution,
            strength: self.static_bsp_ao_strength,
            range: self.static_bsp_ao_range,
            current_cell_only: self.static_bsp_ao_current_cell,
        })
    }

    pub(in crate::renderer) fn request_static_ao(&mut self, force_rebuild: bool) {
        let Some(key) = self.desired_static_ao_key() else {
            return;
        };
        if let Some(pending) = &self.static_ao_pending {
            if pending.key != key || force_rebuild {
                self.static_ao_deferred_force_rebuild |= force_rebuild;
            }
            return;
        }
        let Some(source) = self
            .world
            .as_ref()
            .and_then(|world| world.static_ao_source.clone())
        else {
            println!("Static BSP AO: no BSP collision/cache source available for this map");
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (progress_tx, progress_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let camera_position = self.previous_camera_position;
        println!(
            "Static BSP AO: starting {} bake/cache lookup at {} samples / {}x / {}% strength / {:.2}x range{}",
            key.mode.label(),
            key.samples,
            key.scale,
            key.strength,
            key.range as f32 / 100.0,
            if force_rebuild { " (forced rebuild)" } else if key.current_cell_only { " (current cell only)" } else { "" }
        );
        match thread::Builder::new()
            .name("jka-static-ao".into())
            .spawn(move || {
                let result = run_static_ao_worker(
                    source,
                    key,
                    force_rebuild,
                    &progress_tx,
                    &update_tx,
                    camera_position,
                );
                if result.is_err() {
                    let _ = progress_tx.send((100, 100));
                }
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.static_ao_pending = Some(StaticAoPendingJob {
                    key,
                    rx,
                    progress_rx,
                    update_rx,
                });
            }
            Err(error) => {
                eprintln!("Static BSP AO: could not start worker thread: {error}");
            }
        }
    }

    pub(in crate::renderer) fn poll_static_ao_progress(&mut self) -> Option<(u32, u32)> {
        let pending = self.static_ao_pending.as_mut()?;
        let mut latest = None;
        while let Ok(progress) = pending.progress_rx.try_recv() {
            latest = Some(progress);
        }
        latest
    }

    pub(in crate::renderer) fn rebuild_world_surface_bind_groups(&self, world: &mut WorldGpu) {
        let WorldGpu {
            sky_average: _,
            primary_skybox,
            textures,
            footprint_mark_textures,
            lightmaps,
            deluxemaps,
            texture_clamp,
            detail_texture_by_base,
            reflection_probes,
            coarse_batches,
            full_batches,
            ..
        } = world;
        let footprint_marks: [&GpuImage; 2] = std::array::from_fn(|slot| {
            footprint_mark_textures[slot]
                .and_then(|index| textures.get(index))
                .unwrap_or(&self.white)
        });
        for batch in coarse_batches.iter_mut().chain(full_batches.iter_mut()) {
            let detail_texture = detail_texture_for_source(
                &batch.source,
                &self.detail_texture,
                self.detail_texture_auto,
                &self.detail_auto_textures,
                detail_texture_by_base,
            );
            batch.bind_group = create_world_bind_group(
                &self.device,
                &self.surface_layout,
                &self.white,
                &self.missing,
                &self.flat_normal,
                &self.repeat_sampler,
                &self.clamp_sampler,
                &self.pbr_repeat_sampler,
                &self.pbr_clamp_sampler,
                &self.lightmap_sampler,
                &self.sky_sampler,
                detail_texture,
                textures,
                lightmaps,
                deluxemaps,
                texture_clamp,
                *primary_skybox,
                &footprint_marks,
                reflection_probes,
                &batch.source,
                &batch._material_buffer,
                &batch._fog_buffer,
                self.surface_deformation.buffer(),
                self.surface_deformation.field_views(),
                self.surface_deformation.field_sampler(),
            );
            batch.fast_bind_group = create_fast_world_bind_group(
                &self.device,
                &self.fast_surface_layout,
                &self.white,
                &self.missing,
                &self.repeat_sampler,
                &self.clamp_sampler,
                &self.lightmap_sampler,
                &self.sky_sampler,
                textures,
                lightmaps,
                texture_clamp,
                *primary_skybox,
                &footprint_marks,
                &batch.source,
                &batch._material_buffer,
                self.surface_deformation.buffer(),
                self.surface_deformation.field_views(),
                self.surface_deformation.field_sampler(),
            );
        }
        sync_auto4_bind_groups(world);
    }

    pub(in crate::renderer) fn restore_static_ao_lightmaps(&mut self) {
        let Some(mut world) = self.world.take() else {
            return;
        };
        if !world.static_ao_lightmap_active {
            self.world = Some(world);
            self.sync_baked_brightness_assets();
            return;
        }
        world.lightmaps = world
            .lightmap_base_data
            .iter()
            .map(|base| upload_texture(&self.device, &self.queue, base))
            .collect();
        self.rebuild_world_surface_bind_groups(&mut world);
        world.static_ao_lightmap_active = false;
        self.world = Some(world);
        self.sync_baked_brightness_assets();
    }

    pub(in crate::renderer) fn apply_static_vertex_ao(
        &mut self,
        values: &[u8],
    ) -> Result<(), String> {
        let Some(world) = self.world.as_mut() else {
            return Err("world changed before AO result could be applied".into());
        };
        let Some(source) = world.static_ao_source.as_ref() else {
            return Err("map has no static AO source".into());
        };
        if values.len() != source.vertices.len()
            || world.static_ao_source_to_gpu.len() != source.vertices.len()
        {
            return Err("vertex AO cache does not match current world geometry".into());
        }
        for (source_index, &visibility) in values.iter().enumerate() {
            let gpu_index = world.static_ao_source_to_gpu[source_index];
            if gpu_index == u32::MAX {
                continue;
            }
            if let Some(vertex) = world.static_ao_gpu_vertices.get_mut(gpu_index as usize) {
                vertex.alpha_cutoff = f32::from(visibility) / 255.0;
            }
        }
        self.queue.write_buffer(
            &world.vertex_buffer,
            0,
            bytemuck::cast_slice(&world.static_ao_gpu_vertices),
        );
        Ok(())
    }

    pub(in crate::renderer) fn apply_static_lightmap_ao(
        &mut self,
        images: Vec<StaticAoBakedLightmap>,
    ) -> Result<(), String> {
        let Some(mut world) = self.world.take() else {
            return Err("world changed before AO result could be applied".into());
        };
        let result = (|| {
            if images.len() != world.lightmap_base_data.len() {
                return Err("lightmap AO cache does not match current lightmap set".into());
            }
            // The existing HQ lightmap-AO cache/compositor is intentionally an
            // 8-bit path. Replacing an HDR/FP16 lightmap with that cache would
            // silently destroy the extra lighting range that r_floatLightmap is
            // meant to preserve. Keep the rich lightmaps intact instead of
            // quantizing them; vertex AO remains available with float lightmaps.
            if world
                .lightmap_base_data
                .iter()
                .any(|lightmap| lightmap.rgba16f.is_some())
            {
                return Err(
                    "HQ static lightmap AO is not applied to FP16/HDR lightmaps; use vertex AO or disable r_floatLightmap"
                        .into(),
                );
            }
            let mut lightmaps = Vec::with_capacity(images.len());
            for image in images {
                let data = TextureData {
                    label: image.label,
                    source: None,
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba,
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: image.clamp,
                    srgb: image.srgb,
                };
                lightmaps.push(upload_texture(&self.device, &self.queue, &data));
            }
            world.lightmaps = lightmaps;
            self.rebuild_world_surface_bind_groups(&mut world);
            world.static_ao_lightmap_active = true;
            Ok(())
        })();
        self.world = Some(world);
        self.sync_baked_brightness_assets();
        result
    }

    pub(in crate::renderer) fn apply_static_lightmap_tile(
        &mut self,
        texture: usize,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), String> {
        let Some(world) = self.world.as_ref() else {
            return Err("world changed before AO tile could be applied".into());
        };
        if !world.static_ao_lightmap_active {
            return Err("progressive AO lightmap was not initialized".into());
        }
        let Some(lightmap) = world.lightmaps.get(texture) else {
            return Err("AO tile lightmap index is out of range".into());
        };
        if rgba.len() != width as usize * height as usize * 4 {
            return Err("AO tile byte count is invalid".into());
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &lightmap._texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub(in crate::renderer) fn poll_static_ao_updates(&mut self) {
        let mut updates = Vec::new();
        if let Some(pending) = self.static_ao_pending.as_mut() {
            while let Ok(update) = pending.update_rx.try_recv() {
                updates.push(update);
            }
        }
        let desired = self.desired_static_ao_key();
        for update in updates {
            let result = match update {
                StaticAoWorkerUpdate::InitLightmaps { key, images } if desired == Some(key) => {
                    self.apply_static_lightmap_ao(images)
                }
                StaticAoWorkerUpdate::VertexAo { key, values } if desired == Some(key) => {
                    self.apply_static_vertex_ao(&values)
                }
                StaticAoWorkerUpdate::LightmapTile {
                    key,
                    texture,
                    x,
                    y,
                    width,
                    height,
                    rgba,
                } if desired == Some(key) => {
                    self.apply_static_lightmap_tile(texture, x, y, width, height, &rgba)
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("Static BSP AO progressive update: {error}");
            }
        }
    }

    pub(in crate::renderer) fn poll_static_ao_worker(&mut self) {
        let outcome =
            self.static_ao_pending
                .as_ref()
                .and_then(|pending| match pending.rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        Some(Err("static AO worker disconnected".into()))
                    }
                });
        let Some(outcome) = outcome else {
            return;
        };
        let finished_key = self.static_ao_pending.take().map(|job| job.key);
        let desired = self.desired_static_ao_key();
        match outcome {
            Ok(result) if desired == Some(result.key) => {
                let StaticAoBakeData::Lightmap {
                    vertex_values,
                    images,
                } = result.data;
                let applied = self
                    .apply_static_vertex_ao(&vertex_values)
                    .and_then(|()| self.apply_static_lightmap_ao(images));
                match applied {
                    Ok(()) => {
                        println!(
                        "Static BSP AO: {} {} at {} samples / {}x / {}% / {:.2}x range in {:.1} ms",
                        if result.cache_hit { "cache hit" } else { "bake complete" },
                        result.key.mode.label(),
                        result.key.samples,
                        result.key.scale,
                        result.key.strength,
                        result.key.range as f32 / 100.0,
                        result.elapsed_ms
                    )
                    }
                    Err(error) => eprintln!("Static BSP AO: could not apply result: {error}"),
                }
            }
            Ok(result) => println!(
                "Static BSP AO: discarded stale {} {}-sample {}x / {}% / {:.2}x range result",
                result.key.mode.label(),
                result.key.samples,
                result.key.scale,
                result.key.strength,
                result.key.range as f32 / 100.0
            ),
            Err(error) => eprintln!("Static BSP AO worker: {error}"),
        }

        let force = std::mem::take(&mut self.static_ao_deferred_force_rebuild);
        if self.desired_static_ao_key().is_some() && (desired != finished_key || force) {
            self.request_static_ao(force);
        }
    }
}
