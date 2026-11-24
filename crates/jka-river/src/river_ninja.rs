//! River-side input adapter for Fluid Ninja Live. No dependency on jka-ninja:
//! the solver's API and selected graph parameters are owned by Phase 1.
//! Generic DinurdoJK input providers only (native mask, contacts, actor
//! history). Anything derived from Fluid Ninja assets or graphs (presets,
//! texture inventory, parameter mappings) lives in the private jka-ninja
//! implementation; its coupling notes are DinurdoJK-ninja/docs/RIVER_NINJA_COUPLING.md.
use jka_water::river::{control::Grid, foam_field::Placement, CENTIMETRES_PER_MAP_UNIT};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
/// Re-exported for the client so it needs no direct jka-ninja dependency.
pub use jka_ninja::{river_caustics_image, river_input_images, river_underwater_images, RiverImage, RiverVolumeImage, INCLUDED as FLUID_NINJA_INCLUDED};
use wgpu::util::DeviceExt;

#[path = "river_ninja_debug.rs"]
mod wake_debug;

/// The Fluid Ninja river presentation is always on when it is built in; there
/// is no native fallback and no runtime toggle. This extends the actor
/// collection gate, not the physical wake toggle.
pub const fn actor_capture_requested() -> bool { jka_ninja::INCLUDED }
/// Set while a promoted Ninja ocean is active: it consumes the same joint
/// wake points as rivers, so players' river bones must be evaluated.
static OCEAN_ACTORS: AtomicBool = AtomicBool::new(false);
pub fn set_ocean_requested(requested: bool) { OCEAN_ACTORS.store(requested, Ordering::Relaxed); }
/// cg_waterwake != 0 (Effects > Water wake): the shared actor wake feeds the
/// Ninja river and Ninja ocean fields. Off sends them no actor points.
static WAKE_ENABLED: AtomicBool = AtomicBool::new(true);
pub fn set_wake_enabled(enabled: bool) { WAKE_ENABLED.store(enabled, Ordering::Relaxed); }
pub fn wake_enabled() -> bool { WAKE_ENABLED.load(Ordering::Relaxed) }
/// Evaluate the shared wake joints (river and/or Ninja ocean consumers).
pub fn wake_joints_requested() -> bool {
    wake_enabled() && (actor_capture_requested() || OCEAN_ACTORS.load(Ordering::Relaxed))
}
/// The followed player (entity id), owner of the player-following Ninja fields
/// (Coast window, Source-lake wake window). u32::MAX = none (free camera).
static WAKE_OWNER: AtomicU32 = AtomicU32::new(u32::MAX);
pub fn set_wake_owner(id: Option<u16>) { WAKE_OWNER.store(id.map_or(u32::MAX, u32::from), Ordering::Relaxed); }
pub fn wake_owner() -> Option<u16> { u16::try_from(WAKE_OWNER.load(Ordering::Relaxed)).ok() }
/// Velocity (cm/s, JKA x/y/z) of the wake owner among `tracks`, i.e. the motion
/// of a field that follows the player. Zero without an owner.
pub fn wake_owner_velocity_cm_s(tracks: &[Track]) -> [f32; 3] {
    let Some(owner) = wake_owner() else { return [0.0; 3] };
    let Some(t) = tracks.iter().find(|t| t.current.id == owner && t.current.kind == ActorKind::Player) else { return [0.0; 3] };
    let c = t.current;
    let v = c.velocity_map_s.or_else(|| t.previous.and_then(|p| {
        let dt = (c.time_ms - p.time_ms) as f32 * 0.001;
        (dt > 0.0).then(|| std::array::from_fn(|i| (c.origin_map[i] - p.origin_map[i]) / dt))
    })).unwrap_or([0.0; 3]);
    v.map(|x| x * CENTIMETRES_PER_MAP_UNIT)
}
static WAKE_DEBUG: AtomicBool = AtomicBool::new(false);
pub fn wake_debug_enabled() -> bool { WAKE_DEBUG.load(Ordering::Relaxed) }
pub fn wake_debug_active() -> bool { actor_capture_requested() && wake_debug_enabled() }
pub fn set_wake_debug(enabled: bool) { WAKE_DEBUG.store(enabled, Ordering::Relaxed); }
static MATERIAL_DEBUG: AtomicU32 = AtomicU32::new(0);
pub fn material_debug() -> u32 { MATERIAL_DEBUG.load(Ordering::Relaxed) }
pub fn set_material_debug(mode: u32) { MATERIAL_DEBUG.store(mode.min(5), Ordering::Relaxed); }
static RESOLUTION: AtomicU32 = AtomicU32::new(512);
pub fn resolution() -> u32 { RESOLUTION.load(Ordering::Relaxed) }
pub fn set_resolution(value: u32) { RESOLUTION.store(value.max(1), Ordering::Relaxed); }

// Explicit seeks include forward seeks, which cannot be distinguished from
// dropped frames by inspecting time alone. The render thread consumes this.
static TIMELINE_EPOCH: AtomicU64 = AtomicU64::new(0);
pub fn timeline_epoch() -> u64 { TIMELINE_EPOCH.load(Ordering::Acquire) }
pub fn invalidate_timeline() { TIMELINE_EPOCH.fetch_add(1, Ordering::AcqRel); }

/// Same centre and world extent as the live material/foam field. Do not run
/// Grid::max_axis on the FNL resolution: that pads the water domain itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Domain {
    pub origin_cm: [f64; 3],
    pub extent_cm: [f32; 2],
    pub resolution: [u32; 2],
    pub hydraulic_cells: [u32; 2],
    pub placement: Placement,
}
impl Domain {
    pub fn new(origin_cm: [f64; 3], grid: Grid, resolution: [u32; 2]) -> Result<Self, String> {
        if resolution.contains(&0) || grid.dimensions.contains(&0)
            || grid.world_size.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            return Err("invalid river Ninja domain".into());
        }
        Ok(Self { origin_cm, extent_cm: grid.world_size, resolution, hydraulic_cells: grid.dimensions,
            placement: Placement::unrotated(origin_cm, &grid)? })
    }
    /// Map XY -> unit XY. Do not clamp: the caller rejects out-of-domain points.
    pub fn map_to_unit(&self, xy: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|a| ((f64::from(xy[a]) * f64::from(CENTIMETRES_PER_MAP_UNIT)
            - self.origin_cm[a]) / f64::from(self.extent_cm[a]) + 0.5) as f32)
    }
    /// Physical displacement measured in FNL texels per supplied step, prior
    /// to the graph's SimSpeedCoefficient and chosen input multiplier.
    pub fn displacement_texels(&self, velocity_cm_s: [f32; 2], dt: f32) -> [f32; 2] {
        std::array::from_fn(|a| velocity_cm_s[a] * dt * self.resolution[a] as f32 / self.extent_cm[a])
    }
}

/// Exact native geometry, not the half precision material output. The sparse
/// pool has the same vec4 geometry layout in all three Storage modes.
pub enum Geometry<'a> {
    Dense(&'a wgpu::Buffer),
    Sparse {
        geometry: &'a wgpu::Buffer,
        slots: &'a wgpu::Buffer,
        window_origin: [i32; 2],
        lattice_cells: [u32; 2],
        tiles: [u32; 2],
        tile: u32,
        generation: u64,
    },
}
#[derive(Clone, Copy)]
pub struct Blocks<'a> {
    pub flags: &'a wgpu::Buffer,
    pub grid: [u32; 2],
    /// The render block lattice, not the FNL resolution.
    pub cells: [u32; 2],
}
/// Borrow immediately after the river's foam pass, after gather/material and
/// occupancy flags have been encoded. Do not retain across pool growth or shift.
pub struct Inputs<'a> {
    pub domain: Domain,
    /// R32Float texture array: height relative to origin, depth, vx, vy (cm, cm/s).
    pub state: &'a wgpu::Texture,
    pub geometry: Geometry<'a>,
    pub blocks: Option<Blocks<'a>>,
}

/// Native-resolution dry/solid mask, 1=blocked, 0=open. The FNL compositing
/// graph (Phase 1) owns polarity conversion, filtering, weakening and density
/// killing. It must not mistake this for an already-composited FNL mask.
/// Created on demand; the regular river path never constructs this object.
pub struct MaskField {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    sparse: bool,
}

// Native wgpu-core resolves popped validation scopes synchronously (see
// the retired volume renderer). Unlike that path, pending is an error, never a clean scope.
pub fn checked_scope(scope: wgpu::ErrorScopeGuard) -> Result<(), String> {
    use std::future::Future;
    let mut future = std::pin::pin!(scope.pop());
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(None) => Ok(()),
        std::task::Poll::Ready(Some(error)) => Err(error.to_string()),
        std::task::Poll::Pending => Err("river Ninja validation scope pending".into()),
    }
}

impl MaskField {
    pub fn new(device: &wgpu::Device, inputs: &Inputs<'_>) -> Result<Self, String> {
        let dims = inputs.domain.hydraulic_cells;
        let limits = device.limits();
        if dims.contains(&0) || dims.iter().any(|n| *n > limits.max_texture_dimension_2d
            || n.div_ceil(8) > limits.max_compute_workgroups_per_dimension) {
            return Err("river Ninja mask exceeds GPU limits".into());
        }
        let sparse = matches!(&inputs.geometry, Geometry::Sparse { .. });
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("river Ninja native solid or dry mask"),
            size: wgpu::Extent3d { width: dims[0], height: dims[1], depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river Ninja mask controls"), size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river Ninja native collision reader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("river_ninja_mask.wgsl").into()),
        });
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None,
        };
        let mut entries = vec![
            buffer(0, wgpu::BufferBindingType::Uniform),
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            buffer(2, wgpu::BufferBindingType::Storage { read_only: true }),
            wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::R32Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
        ];
        if sparse { entries.push(buffer(3, wgpu::BufferBindingType::Storage { read_only: true })); }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river Ninja native collision reader"), entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river Ninja native collision reader"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("river Ninja native collision reader"), layout: Some(&pipeline_layout), module: &module,
            entry_point: Some(if sparse { "sparse_mask" } else { "dense_mask" }),
            compilation_options: Default::default(), cache: None,
        });
        checked_scope(scope)?;
        Ok(Self { texture, view, uniform, pipeline, sparse })
    }
    pub fn texture(&self) -> &wgpu::Texture { &self.texture }
    /// Record only with a coherent state/geometry pair. In sparse mode call
    /// on window gather (or initial enable after one); do not sample pool
    /// geometry from a later incomplete hydraulic frame against an old window.
    /// Any error is feature-local: caller latches/logs and retains Unreal foam.
    pub fn encode(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, inputs: &Inputs<'_>) -> Result<(), String> {
        let dims = inputs.domain.hydraulic_cells;
        if [self.texture.width(), self.texture.height()] != dims
            || [inputs.state.width(), inputs.state.height()] != dims
            || inputs.state.depth_or_array_layers() != 4 || inputs.state.format() != crate::river_gpu::WATER_FORMAT
            || self.sparse != matches!(&inputs.geometry, Geometry::Sparse { .. }) {
            return Err("river Ninja mask inputs changed or invalid".into());
        }
        // 4 vec4 words, layout matches the WGSL uniform. Geometry is read-only.
        let (geo, slots, origin, lattice, table) = match &inputs.geometry {
            Geometry::Dense(geo) => (*geo, None, [0; 2], dims, [0; 4]),
            Geometry::Sparse { geometry, slots, window_origin, lattice_cells, tiles, tile, .. } =>
                (*geometry, Some(*slots), *window_origin, *lattice_cells, [tiles[0], tiles[1], *tile, tile.trailing_zeros()]),
        };
        let words = [dims[0], dims[1], 0, 0,
            origin[0] as u32, origin[1] as u32, lattice[0], lattice[1],
            table[0], table[1], table[2], table[3], 0, 0, 0, 0];
        // Keep updates in the same encoder order as hydraulic writes. queue
        // writes would move ahead of all command buffers submitted together.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river Ninja mask controls upload"), contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        let state = inputs.state.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
        });
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&state) },
            wgpu::BindGroupEntry { binding: 2, resource: geo.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.view) },
        ];
        if let Some(slots) = slots { entries.push(wgpu::BindGroupEntry { binding: 3, resource: slots.as_entire_binding() }); }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river Ninja mask inputs"), layout: &self.pipeline.get_bind_group_layout(0), entries: &entries,
        });
        checked_scope(scope)?;
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 0, 64);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("river Ninja solid or dry mask"), timestamp_writes: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(dims[0].div_ceil(8), dims[1].div_ceil(8), 1);
        Ok(())
    }
}

/// The per-river prepared input mask; Phase 1 owns the simulation history.
pub(crate) struct Prepared {
    pub domain: Domain,
    pub mask: MaskField,
    contacts: Option<ContactField>,
    pub game_seconds: Option<f64>,
    pub scene_revision: Option<u64>,
    /// The Fluid Ninja presentation field (jka-ninja facade). Lives and dies
    /// with this bridge, so every bridge reset (disable, domain/window change,
    /// seek, river removal) also resets the simulation.
    field: Option<jka_ninja::RiverField>,
    /// Game time of the field's last encode (its own clock, not hydraulic).
    field_time: Option<f64>,
    tracks: Vec<jka_ninja::WakeTrack>,
    wake_debug: Option<Result<wake_debug::Probe, String>>,
}
impl Prepared {
    pub fn refresh(slot: &mut Option<Self>, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        inputs: &Inputs<'_>, changed: bool) -> Result<(), String> {
        if slot.as_ref().is_none_or(|p| p.domain != inputs.domain) {
            let mask = MaskField::new(device, inputs)?;
            mask.encode(device, encoder, inputs)?;
            *slot = Some(Self { domain: inputs.domain, mask, contacts: None, game_seconds: None, scene_revision: None,
                field: None, field_time: None, tracks: Vec::new(), wake_debug: None });
        } else if let Some(p) = slot.as_mut().filter(|_| changed) {
            p.mask.encode(device, encoder, inputs)?;
        }
        Ok(())
    }
    pub fn prepare_contacts(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, state: &wgpu::Texture, source: FrameSource<'_>) -> Result<(), String> {
        self.game_seconds = source.game_seconds.filter(|t| t.is_finite());
        self.scene_revision = source.scene_revision;
        if source.debug.is_none() { self.wake_debug = None; }
        self.tracks.clear();
        if source.tracks.is_empty() { if let Some(f) = &mut self.contacts { f.count = 0; } return Ok(()); }
        if self.contacts.is_none() { self.contacts = Some(ContactField::new(device)?); }
        self.contacts.as_mut().expect("created above").encode(device, source.queue, encoder, self.domain, state, source.tracks, &mut self.tracks)
    }
    pub fn clear_contacts(&mut self) {
        self.game_seconds = None;
        self.scene_revision = None;
        self.tracks.clear();
        if let Some(f) = &mut self.contacts { f.count = 0; }
    }
    pub fn contacts(&self) -> Option<Contacts<'_>> { self.contacts.as_ref().map(ContactField::outputs) }

    /// Create (lazily) and encode the Fluid Ninja field after the mask and
    /// contact passes of this encoder. Needs the input images; without them
    /// (not loaded, missing, or a build without the private implementation)
    /// this does nothing. Errors are feature-local: the caller latches them.
    pub fn encode_field(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        state: &wgpu::Texture, source: FrameSource<'_>) -> Result<Option<(f64, bool)>, String> {
        let Some(images) = source.images else { return Ok(None) };
        let desc = jka_ninja::RiverDesc { resolution: self.domain.resolution, extent_cm: self.domain.extent_cm };
        if self.field.as_ref().is_none_or(|f| *f.desc() != desc) {
            self.field = Some(jka_ninja::RiverField::new(device, &desc)?);
            self.field_time = None;
            eprintln!("[RIVER NINJA] field {}x{} over {:.0}x{:.0} cm", desc.resolution[0], desc.resolution[1], desc.extent_cm[0], desc.extent_cm[1]);
        }
        // Advance only on the authoritative timeline; no time -> no step.
        let game_delta = match (self.field_time, source.game_seconds) {
            (Some(old), Some(new)) if new >= old => new - old,
            _ => 0.0,
        };
        if source.game_seconds.is_some() { self.field_time = source.game_seconds; }
        // The contact encoder emits the ordered list and flags together.
        let contact_flags = self.contacts.as_ref()
            .filter(|c| c.count > 0 && c.count as usize == self.tracks.len())
            .map(|c| &c.flags);
        if !self.tracks.is_empty() && contact_flags.is_none() {
            return Err("river Ninja: missing contact flags for tracked points".into());
        }
        let field = self.field.as_mut().expect("created above");
        let stepped = field.encode(device, source.queue, encoder, &jka_ninja::RiverFrame {
            state, solid_or_dry: &self.mask.texture, contact_flags, tracks: &self.tracks, game_delta,
            images: [images[0], images[1], images[2], images[3], images[4]],
        })?;
        Ok(Some((game_delta, stepped)))
    }

    pub fn debug_wake(
        &mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        state: &wgpu::Texture, source: FrameSource<'_>, river_id: u64,
        step: Option<(f64, bool)>, material_bound: bool,
    ) {
        let Some(subject) = source.debug else { return };
        let probe = self.wake_debug.get_or_insert_with(|| {
            wake_debug::Probe::new(device).inspect_err(|error| {
                eprintln!("[RIVER NINJA WAKE] river={river_id} probe unavailable: {error}");
            })
        });
        let Ok(probe_state) = probe else { return };
        if let Err(error) = probe_state.frame(device, encoder, state, source, subject, river_id, self.domain,
            step, material_bound, &self.tracks, self.contacts.as_ref(), self.field.as_ref())
        {
            eprintln!("[RIVER NINJA WAKE] river={river_id} probe disabled: {error}");
            *probe = Err(error);
        }
    }
    /// The field's exported textures (Phase 3 consumer).
    pub fn field_outputs(&self) -> Option<jka_ninja::RiverOutputs<'_>> { self.field.as_ref().map(jka_ninja::RiverField::outputs) }
}

/// Prepared inputs, exposed only after the native mask has been encoded. The
/// two textures have the same hydraulic resolution, not the FNL resolution.
pub struct Snapshot<'a> {
    pub domain: Domain,
    pub state: &'a wgpu::Texture,
    pub solid_or_dry: &'a wgpu::Texture,
    pub blocks: Option<Blocks<'a>>,
    pub timeline_epoch: u64,
    pub game_seconds: Option<f64>,
    pub scene_revision: Option<u64>,
    pub contacts: Option<Contacts<'a>>,
}

/// Actual source categories; never treat a missile or mover as a skeletal player.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind { Player = 0, Mover = 1, Missile = 2, FogMover = 3, FogMissile = 4 }

impl ActorKind {
    pub fn has_river_contact_shape(self) -> bool {
        matches!(self, Self::Player | Self::Mover | Self::Missile)
    }
}
/// JKA's protocol entity namespace (MAX_GENTITIES in net.rs).
pub const MAX_TRACKS: usize = 1024;
/// Stable source slots for the humanoid joints selected by the river reference.
const RIVER_TRACKED_BONE_COUNT: usize = 12;
pub const RIVER_BONE_COUNT: usize = RIVER_TRACKED_BONE_COUNT + 2;
/// Calves and first index-finger joints used by the Fog_Mist overlap filter.
pub const FOG_BONE_SLOTS: [usize; 4] = [2, 3, 12, 13];
#[derive(Clone, Copy, Debug)]
pub struct RiverBone {
    pub position_map: [f32; 3],
    pub parent_distance_map: f32,
}

/// A source record, NOT an FNL brush. Radius/density/noise/height mapping
/// and GPU water-column contact belong to the actual TrackPoint adapter.
#[derive(Clone, Copy, Debug)]
pub struct ActorSample {
    pub id: u16,
    pub kind: ActorKind,
    pub continuity: i32,
    pub time_ms: i32,
    pub origin_map: [f32; 3],
    pub mins_map: [f32; 3],
    pub maxs_map: [f32; 3],
    pub velocity_map_s: Option<[f32; 3]>,
    /// Existing evaluated Ghoul2 joints; absent for unavailable models/movers.
    pub river_bones: Option<[Option<RiverBone>; RIVER_BONE_COUNT]>,
}
#[derive(Clone, Copy, Debug)]
pub struct Track {
    pub current: ActorSample,
    /// None on first observation, discontinuity or absence in preceding update.
    pub previous: Option<ActorSample>,
}
/// Bounded by the protocol entity namespace. One instance per
/// timeline, shared across rivers. Do not use render-frame extrapolation to
/// invent prior positions or connect across teleport/respawn.
#[derive(Default)]
pub struct ActorHistory {
    tracks: Vec<Track>,
    scratch: Vec<Track>,
    epoch: u64,
}
impl ActorHistory {
    pub fn reset(&mut self) { self.tracks.clear(); self.scratch.clear(); }
    pub fn tracks(&self) -> &[Track] { &self.tracks }
    pub fn update(&mut self, samples: impl IntoIterator<Item = ActorSample>) -> &[Track] {
        let epoch = timeline_epoch();
        if self.epoch != epoch { self.reset(); self.epoch = epoch; }
        self.scratch.clear();
        // Index by actual entity number, not a guessed brush priority or the
        // order of an unrelated rendering pass. Preserve incoming list order.
        let mut old = [None; MAX_TRACKS];
        let mut seen = [false; MAX_TRACKS];
        for (i, t) in self.tracks.iter().enumerate() { old[usize::from(t.current.id)] = Some(i); }
        for mut sample in samples.into_iter() {
            let id = usize::from(sample.id);
            if id >= MAX_TRACKS || seen[id] { continue; }
            if sample.velocity_map_s.is_some_and(|v| v.iter().any(|x| !x.is_finite())) { sample.velocity_map_s = None; }
            if sample.origin_map.iter().chain(&sample.mins_map).chain(&sample.maxs_map).any(|v| !v.is_finite())
                || (0..3).any(|i| sample.mins_map[i] > sample.maxs_map[i]) { continue; }
            seen[id] = true;
            let previous = old[id].map(|i| &self.tracks[i]).filter(|t| t.current.kind == sample.kind && t.current.continuity == sample.continuity)
                .and_then(|t| {
                    if t.current.time_ms < sample.time_ms { Some(t.current) }
                    else if t.current.time_ms == sample.time_ms { t.previous }
                    else { None }
                });
            self.scratch.push(Track { current: sample, previous });
        }
        std::mem::swap(&mut self.tracks, &mut self.scratch);
        &self.tracks
    }
}

/// Snapshot clock and source tracks from the existing render presentation
/// update, rather than hydraulic solver rate or wall time. The solver owns
/// the fixed-step accumulator and decides when to consume these records.
#[derive(Clone, Copy)]
pub struct FrameSource<'a> {
    pub game_seconds: Option<f64>,
    pub scene_revision: Option<u64>,
    pub tracks: &'a [Track],
    /// Views of `jka_ninja::river_input_images()` (5 simulation inputs, then
    /// the material flow map and roughness map), once the renderer loaded them.
    pub images: Option<[&'a wgpu::TextureView; 7]>,
    /// Uploaded caustics atlas with wrapped borders for each volume slice.
    pub caustics: Option<&'a wgpu::TextureView>,
    pub queue: &'a wgpu::Queue,
    pub debug: Option<WakeDebugSubject>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WakeDebugSubject {
    pub client: Option<u16>,
    pub position_map: Option<[f32; 3]>,
}

/// Raw GPU painter source record, in domain-relative JKA centimetres. This
/// is deliberately NOT a port of Transient.Pos/Size/Velo: their mapping
/// belongs to the selected upstream tracker route and Phase 1.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuTrack {
    origin: [f32; 4], previous: [f32; 4], mins: [f32; 4], maxs: [f32; 4],
    velocity: [f32; 4], previous_velocity: [f32; 4], meta: [u32; 4],
}
fn relative_position(domain: Domain, p: [f32; 3], w: f32) -> [f32; 4] {
    let p: [f32; 3] = std::array::from_fn(|i| (f64::from(p[i]) * f64::from(CENTIMETRES_PER_MAP_UNIT) - domain.origin_cm[i]) as f32);
    [p[0], p[1], p[2], w]
}
impl GpuTrack {
    fn from_track(domain: Domain, t: &Track) -> Self {
        let cur = t.current;
        let old = t.previous.unwrap_or(cur);
        let v = cur.velocity_map_s.unwrap_or([0.; 3]).map(|x| x * CENTIMETRES_PER_MAP_UNIT);
        let pv = old.velocity_map_s.unwrap_or([0.; 3]).map(|x| x * CENTIMETRES_PER_MAP_UNIT);
        Self { origin: relative_position(domain, cur.origin_map, cur.kind as u32 as f32),
            previous: relative_position(domain, old.origin_map, if t.previous.is_some() { 1. } else { 0. }),
            mins: relative_position(domain, cur.mins_map, 0.), maxs: relative_position(domain, cur.maxs_map, 0.),
            velocity: [v[0], v[1], v[2], if cur.velocity_map_s.is_some() { 1. } else { 0. }],
            previous_velocity: [pv[0], pv[1], pv[2], if t.previous.is_some() && old.velocity_map_s.is_some() { 1. } else { 0. }],
            meta: [u32::from(cur.id), cur.continuity as u32, cur.time_ms as u32, old.time_ms as u32] }
    }
}

/// One wake point of an actor in JKA map units, before any field frame.
/// The single actor-to-wake source for river, Ocean2 and Coast fields.
#[derive(Clone, Copy, Debug)]
pub struct WakePoint {
    pub actor_id: u16,
    /// 0 = hull centre, 1.. = tracked joint.
    pub slot: u8,
    pub continuity: i32,
    pub time_ms: i32,
    pub position_map: [f32; 3],
    pub previous_map: Option<[f32; 3]>,
    /// Contact box in map units: a joint's +-half-height band, or the hull.
    pub mins_map: [f32; 3],
    pub maxs_map: [f32; 3],
    pub source: jka_ninja::WakeTrackSource,
}
impl WakePoint {
    pub fn joint(&self) -> bool { self.slot != 0 }
    /// Field-relative track; `rel` maps a JKA map position to field cm.
    pub fn track(&self, rel: impl Fn([f32; 3]) -> [f32; 3]) -> jka_ninja::WakeTrack {
        jka_ninja::WakeTrack { actor_id: self.actor_id, slot: self.slot, continuity: self.continuity,
            time_ms: self.time_ms, origin_cm: rel(self.position_map), previous_cm: self.previous_map.map(&rel),
            source: self.source }
    }
}

/// Expand one actor into its wake points: the evaluated Ghoul2 joints when
/// available, else one hull-centre point. Never fabricates joints.
pub fn wake_points(t: &Track, out: &mut Vec<WakePoint>) {
    use jka_ninja::WakeTrackSource;
    let cur = t.current;
    if !wake_enabled() || !cur.kind.has_river_contact_shape() { return; }
    let valid = |b: RiverBone| b.parent_distance_map.is_finite() && b.parent_distance_map > 0.0
        && b.position_map.iter().all(|v| v.is_finite());
    let base = WakePoint { actor_id: cur.id, slot: 0, continuity: cur.continuity, time_ms: cur.time_ms,
        position_map: cur.origin_map, previous_map: t.previous.map(|p| p.origin_map),
        mins_map: cur.mins_map, maxs_map: cur.maxs_map, source: WakeTrackSource::Primitive { bounds_radius_cm: 0.0 } };
    if let Some(bones) = cur.river_bones {
        let start = out.len();
        let half = jka_ninja::wake_contact_half_height_cm() / CENTIMETRES_PER_MAP_UNIT;
        for (i, bone) in bones.into_iter().take(RIVER_TRACKED_BONE_COUNT).enumerate() {
            let Some(bone) = bone.filter(|b| valid(*b)) else { continue };
            let previous = t.previous.and_then(|p| p.river_bones).and_then(|b| b[i]).filter(|b| valid(*b));
            let p = bone.position_map;
            out.push(WakePoint { slot: (i + 1) as u8, position_map: p, previous_map: previous.map(|b| b.position_map),
                mins_map: [p[0], p[1], p[2] - half], maxs_map: [p[0], p[1], p[2] + half],
                source: WakeTrackSource::Skeletal { parent_distance_cm: bone.parent_distance_map * CENTIMETRES_PER_MAP_UNIT },
                ..base });
        }
        if out.len() != start { return; }
    }
    let half: [f32; 3] = std::array::from_fn(|i| 0.5 * (cur.maxs_map[i] - cur.mins_map[i]));
    let radius = half[0].hypot(half[1]).hypot(half[2]) * CENTIMETRES_PER_MAP_UNIT;
    if !radius.is_finite() || radius <= 0.0 { return; }
    out.push(WakePoint { source: WakeTrackSource::Primitive { bounds_radius_cm: radius }, ..base });
}

/// River records: keep GPU contact records and private tracker records in
/// lockstep, including rejected/dry records.
fn append_river_tracks(domain: Domain, t: &Track, points: &mut Vec<WakePoint>, tracks: &mut Vec<jka_ninja::WakeTrack>, records: &mut Vec<GpuTrack>) {
    points.clear();
    wake_points(t, points);
    let rel = |p: [f32; 3]| { let a = relative_position(domain, p, 0.0); [a[0], a[1], a[2]] };
    for point in points.iter() {
        let mut record = GpuTrack::from_track(domain, t);
        if point.joint() {
            let pos = rel(point.position_map);
            let old = point.previous_map.map(&rel);
            let o = old.unwrap_or(pos);
            let (lo, hi) = (rel(point.mins_map), rel(point.maxs_map));
            record.origin = [pos[0], pos[1], pos[2], t.current.kind as u32 as f32];
            record.previous = [o[0], o[1], o[2], if old.is_some() { 1.0 } else { 0.0 }];
            record.mins = [lo[0], lo[1], lo[2], 1.0]; // point sample marker
            record.maxs = [hi[0], hi[1], hi[2], 0.0];
            record.velocity = [0.0; 4];
            record.previous_velocity = [0.0; 4];
        }
        records.push(record);
        tracks.push(point.track(rel));
    }
}

/// Same-index source record and GPU contact flag (1 = native wet surface
/// intersects actor hull or tracked-point band). Both are STORAGE buffers; no CPU readback.
/// Read only after the contact pass in the current command encoder.
pub struct Contacts<'a> {
    pub tracks: &'a wgpu::Buffer,
    pub flags: &'a wgpu::Buffer,
    pub count: u32,
}
struct ContactField {
    pipeline: wgpu::ComputePipeline,
    controls: wgpu::Buffer,
    tracks: wgpu::Buffer,
    flags: wgpu::Buffer,
    records: Vec<GpuTrack>,
    points: Vec<WakePoint>,
    count: u32,
    /// Bind group for the current hydraulic state texture (identity-cached).
    group: Option<(wgpu::Texture, wgpu::BindGroup)>,
}
impl ContactField {
    const MAX: u64 = jka_ninja::WAKE_MAX_TRACKS as u64;
    fn new(device: &wgpu::Device) -> Result<Self, String> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("river Ninja hydraulic contacts"),
            source: wgpu::ShaderSource::Wgsl(include_str!("river_ninja_contact.wgsl").into()) });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("river Ninja contact layout"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            storage(2, true), storage(3, false),
        ] });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("river Ninja contact pipeline layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("river Ninja contact pipeline"), layout: Some(&pl), module: &shader,
            entry_point: Some("contacts"), compilation_options: Default::default(), cache: None });
        let buffer = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let controls = buffer("river Ninja contact controls", 32, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let tracks = buffer("river Ninja contact tracks", Self::MAX * std::mem::size_of::<GpuTrack>() as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let flags = buffer("river Ninja contact flags", Self::MAX * 4, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        checked_scope(scope)?;
        Ok(Self { pipeline, controls, tracks, flags, records: Vec::new(), points: Vec::new(), count: 0, group: None })
    }
    /// Persistent buffers updated with queue writes: each river owns its
    /// ContactField and encodes it at most once per frame, so one write per
    /// buffer lands before the submit that reads it.
    fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, domain: Domain,
        state: &wgpu::Texture, source: &[Track], tracks: &mut Vec<jka_ninja::WakeTrack>) -> Result<(), String> {
        self.count = 0;
        self.records.clear();
        tracks.clear();
        for t in source { append_river_tracks(domain, t, &mut self.points, tracks, &mut self.records); }
        if tracks.len() > Self::MAX as usize { return Err("river Ninja contact source exceeds tracked point capacity".into()); }
        if tracks.is_empty() { return Ok(()); }
        if self.group.as_ref().is_none_or(|(t, _)| t != state) {
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let view = state.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("river Ninja contact inputs"), layout: &self.pipeline.get_bind_group_layout(0), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.controls.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: self.tracks.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.flags.as_entire_binding() },
            ] });
            checked_scope(scope)?;
            self.group = Some((state.clone(), group));
        }
        let count = tracks.len() as u32;
        let words = [domain.extent_cm[0].to_bits(), domain.extent_cm[1].to_bits(), 0, 0, count, 0, 0, 0];
        queue.write_buffer(&self.controls, 0, bytemuck::cast_slice(&words));
        queue.write_buffer(&self.tracks, 0, bytemuck::cast_slice(&self.records));
        encoder.clear_buffer(&self.flags, 0, None);
        let (_, group) = self.group.as_ref().expect("built above");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("river Ninja surface contacts"), timestamp_writes: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(count, 1, 1);
        self.count = count;
        Ok(())
    }
    fn outputs(&self) -> Contacts<'_> { Contacts { tracks: &self.tracks, flags: &self.flags, count: self.count } }
}

/// WGSL input conversion and conservative native wet-block rectangle mapping.
/// The solver must supply its actual sampled footprint (including history,
/// backtrace, filtering, and pressure support); use no culling when unknown.
pub const HYDRAULIC_INPUT_WGSL: &str = include_str!("river_ninja_inputs.wgsl");

/// The selected solver can append this to its own shader module using a free
/// storage binding. Do not call for stages whose dry outputs are not zero or
/// whose read footprint cannot be bounded by the caller.
pub fn block_test_source(group: u32, binding: u32) -> String {
    format!(r#"
@group({group}) @binding({binding}) var<storage, read> ninja_native_flags: array<u32>;
fn ninja_has_wet_block(rect: vec4<u32>, grid: vec2<u32>) -> bool {{
    for (var y = rect.y; y < rect.w; y++) {{
        for (var x = rect.x; x < rect.z; x++) {{
            if ninja_native_flags[y * grid.x + x] != 0u {{ return true; }}
        }}
    }}
    return false;
}}
"#)
}
