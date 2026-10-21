//! lighting implementation modules.
pub(in crate::renderer) mod cascades;
pub(in crate::renderer) mod entity_shadows;
pub(in crate::renderer) mod glass_transmission;
pub(in crate::renderer) mod legacy_dlight_lists;
pub(in crate::renderer) mod legacy_dlights;
pub(in crate::renderer) mod local_shadows;
pub(in crate::renderer) mod original_sun;
pub(in crate::renderer) mod sun_bake;
mod sun_bake_cache;
mod sun_bake_upload;
pub(in crate::renderer) mod baked_sun_cache;
pub(in crate::renderer) mod ray_tracing;
pub(in crate::renderer) mod rt_diagnostics;
pub(in crate::renderer) mod settings;
pub(in crate::renderer) mod shadow_resources;
pub(in crate::renderer) mod state;
pub(in crate::renderer) mod sun;
pub(in crate::renderer) mod upload;
