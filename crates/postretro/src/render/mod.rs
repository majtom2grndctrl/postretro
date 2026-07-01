// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md
pub mod animated_lightmap;
#[cfg(feature = "dev-tools")]
pub mod debug_lines;
#[cfg(feature = "dev-tools")]
pub mod debug_ui;
pub mod fog_pass;
pub mod frame_timing;
pub mod loaded_texture;
pub mod mesh_instances;
pub mod mesh_pass;
#[cfg(feature = "dev-tools")]
pub mod nav_diagnostics;
pub mod screen_effects;
pub mod sdf_atlas;
pub mod sdf_shadow;
pub mod sh_compose;
#[cfg(feature = "dev-tools")]
pub mod sh_diagnostics;
pub mod sh_volume;
pub mod smoke;
pub mod splash;
pub mod splash_pass;
pub mod ui;

#[cfg(test)]
mod curve_eval_test;
#[cfg(test)]
mod sdf_light_select_test;

// --- Extracted submodules (module root is slim; impls split by concern) ---
mod fog_mask;
mod frame_uniforms;
mod material_plan;
mod pipeline_layout;
mod renderer_debug_ui;
mod renderer_diagnostics;
mod renderer_frame;
mod renderer_full_init;
mod renderer_geometry;
mod renderer_init;
mod renderer_init_pipelines;
mod renderer_init_resources;
mod renderer_light_slots;
mod renderer_lighting;
mod renderer_models;
mod renderer_render_frame;
mod renderer_resources;
mod renderer_shadow_passes;
mod renderer_splash;
mod renderer_state;
mod renderer_types;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::lighting::chunk_list::ChunkGrid;
use crate::lighting::influence;
use crate::lighting::lightmap::LightmapResources;
use crate::lighting::spec_buffer::{SPEC_LIGHT_SIZE, pack_spec_lights};
use crate::lighting::spot_shadow::SpotShadowPool;
use crate::lighting::{GPU_LIGHT_SIZE, pack_lights, pack_lights_with_slots_into};
use crate::render::loaded_texture::{
    LoadedTexture, load_model_diffuse_texture, load_textures, placeholder_loaded_texture,
};
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::fog_cell_masks::union_active_mask;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_loader::MapLight;
use postretro_render_data::geometry::BvhTree;
use postretro_render_data::influence::LightInfluence;
use postretro_render_data::material::Material;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use fog_pass::FogPass;
use frame_timing::FrameTiming;
use screen_effects::ScreenEffectsPass;
use sdf_atlas::SdfAtlasResources;
use sdf_shadow::{SdfShadowFrameInputs, SdfShadowPass, SdfShadowShGrid};
use sh_compose::ShComposeResources;
use sh_volume::ShVolumeResources;
use smoke::SmokePass;

use crate::fx::smoke::SpriteFrame;

// Re-export the moved free items so they stay reachable at their original
// `render::*` paths (external callers and sibling render modules depend on these).
pub(crate) use fog_mask::*;
pub(crate) use frame_uniforms::*;
pub(crate) use material_plan::*;
pub(crate) use pipeline_layout::*;
pub(crate) use renderer_geometry::*;
pub(crate) use renderer_lighting::*;
pub(crate) use renderer_types::*;

// Internal init/render helpers used by the `impl Renderer` files via `use super::*`.
use renderer_full_init::*;
use renderer_init_pipelines::*;
use renderer_init_resources::*;
