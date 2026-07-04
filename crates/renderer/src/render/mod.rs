// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md
mod animated_lightmap;
#[cfg(feature = "dev-tools")]
mod debug_lines;
#[cfg(feature = "dev-tools")]
mod debug_ui;
mod direct_sh_compose;
mod fog_pass;
mod frame_timing;
mod loaded_texture;
mod mesh_pass;
mod promoted_depth_cache;
mod screen_effects;
mod sdf_atlas;
mod sdf_shadow;
mod sh_compose;
#[cfg(feature = "dev-tools")]
mod sh_diagnostics;
mod sh_volume;
mod smoke;
mod splash_pass;
mod ui;

#[cfg(test)]
mod curve_eval_test;
#[cfg(test)]
mod sdf_light_select_test;

// --- Extracted submodules (module root is slim; impls split by concern) ---
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
use crate::lighting::lightmap::LightmapResources;
use crate::lighting::spot_shadow::SpotShadowPool;
use crate::render::loaded_texture::{
    LoadedTexture, load_model_diffuse_texture, load_textures, placeholder_loaded_texture,
};
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_loader::MapLight;
use postretro_lighting::influence;
use postretro_lighting::spec_buffer::{SPEC_LIGHT_SIZE, pack_spec_lights};
use postretro_lighting::{GPU_LIGHT_SIZE, pack_lights, pack_lights_with_slots_into};
use postretro_render_cpu::chunk_list::ChunkGrid;
use postretro_render_data::geometry::BvhTree;
use postretro_render_data::influence::LightInfluence;
use postretro_render_data::material::Material;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use direct_sh_compose::{DirectShComposeResources, DirectShDebugOverride};
use fog_pass::FogPass;
use frame_timing::FrameTiming;
use promoted_depth_cache::{PromotedDepthCache, PromotedDepthCacheFramePlan};
use screen_effects::ScreenEffectsPass;
use sdf_atlas::SdfAtlasResources;
use sdf_shadow::{SdfShadowFrameInputs, SdfShadowPass, SdfShadowShGrid};
use sh_compose::ShComposeResources;
use sh_volume::ShVolumeResources;
use smoke::SmokePass;

use postretro_render_cpu::smoke::SpriteFrame;

// Cross-crate re-export: these items now live in `postretro_render_cpu`, kept
// reachable here at their original `render::*` paths.
pub(crate) use postretro_render_cpu::fog_mask::*;
pub use postretro_render_cpu::frame_uniforms::{
    DynamicDirectIsolation, LightingIsolation, SdfShadowMode,
};
pub(crate) use postretro_render_cpu::frame_uniforms::{
    FrameUniforms, SDF_SHADOW_FLAG_ATLAS_PRESENT, UNIFORM_SIZE, build_uniform_data,
};
pub(crate) use postretro_render_cpu::material_plan::{
    parse_blake3_key, plan_submesh_materials, resolve_model_open_path_and_handle,
};
pub(crate) use postretro_render_cpu::mesh_instances;

// Re-export the moved free items so they stay reachable at their original
// `render::*` paths (external callers and sibling render modules depend on these).
pub(crate) use material_plan::*;
pub(crate) use pipeline_layout::*;
pub use renderer_geometry::level_world_to_geometry;
pub(crate) use renderer_geometry::{
    build_default_view_projection, build_line_indices_from_triangles, bytemuck_cast_slice_u32,
    cast_world_vertices_to_bytes,
};
pub(crate) use renderer_lighting::*;
pub use renderer_types::{
    BvhOverlayBudget, BvhOverlayColorMode, BvhOverlayDepthMode, BvhOverlayState,
    CameraCullDiagnostics, CameraCullPath, CellOverlayState, ClearColor, DEFAULT_AMBIENT_FLOOR,
    DEFAULT_DYNAMIC_DIRECT_SCALE, DEFAULT_INDIRECT_SCALE, LevelGeometry, LocatorDiagnostics,
    PortalOverlayState, PresentHandle, Renderer, SpatialCellSetDiagnostics, SpatialDiagnostics,
    WorldWireframeMode,
};
pub(crate) use renderer_types::{GpuTexture, POST_RETRO_ANISO_CLAMP};

#[cfg(feature = "dev-tools")]
pub use debug_ui::{DebugUi, draw_diagnostics_panel};
#[cfg(feature = "dev-tools")]
pub use frame_timing::FrameTimingSnapshot;
#[cfg(feature = "dev-tools")]
pub use sh_diagnostics::{MarkerMode, ShDiagnosticsState};
#[cfg(feature = "dev-tools")]
pub use sh_volume::DeltaVolumeMeta;

// Internal init/render helpers used by the `impl Renderer` files via `use super::*`.
use renderer_full_init::*;
use renderer_init_pipelines::*;
use renderer_init_resources::*;
