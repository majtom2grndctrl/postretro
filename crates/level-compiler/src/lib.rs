// Public compiler helpers used by workspace tools and cross-crate bake tests.
// The compiler pipeline itself remains owned by the `prl-build` binary target.

pub mod bc5;
pub mod cache;
pub mod geometry;
pub mod geometry_utils;
pub mod map_data;
pub mod map_format;
pub mod navmesh_bake;
pub mod partition;
pub mod texture_mips;

// Keep the binary target's co-located unit coverage available to `--lib`
// without exporting the compiler pipeline as a release-time library surface.
#[cfg(test)]
mod affinity_grid;
#[cfg(test)]
mod animated_direct_sh_bake;
#[cfg(test)]
mod animated_light_chunks;
#[cfg(test)]
mod animated_light_weight_maps;
#[cfg(test)]
mod bake_control;
#[cfg(test)]
mod bc6h;
#[cfg(test)]
mod bvh_build;
#[cfg(test)]
mod cell_draw_index_bake;
#[cfg(test)]
mod chart_raster;
#[cfg(test)]
mod chunk_light_list_bake;
#[cfg(test)]
mod delta_drop_policy;
#[cfg(test)]
mod delta_sections;
#[cfg(test)]
mod delta_sh_bake;
#[cfg(test)]
mod direct_sh_bake;
#[cfg(test)]
mod entity_shadow_select;
#[cfg(test)]
mod fixture_pipeline;
#[cfg(test)]
mod fog_cell_masks;
#[cfg(test)]
mod format;
#[cfg(test)]
mod governor;
#[cfg(test)]
mod kinematic_geometry;
#[cfg(test)]
mod light_namespaces;
#[cfg(test)]
mod lightmap_bake;
#[cfg(test)]
mod lightmap_layer;
#[cfg(test)]
mod logger;
#[cfg(test)]
mod pack;
#[cfg(test)]
mod parse;
#[cfg(test)]
mod portals;
#[cfg(test)]
mod reporter;
#[cfg(test)]
mod script_light_membership;
#[cfg(test)]
mod sdf_bake;
#[cfg(test)]
mod sh_bake;
#[cfg(test)]
mod sh_group;
#[cfg(test)]
mod shadowmask_bake;
#[cfg(test)]
mod size_options;
#[cfg(test)]
mod stage;
#[cfg(test)]
mod texture_validation;
#[cfg(test)]
mod trigger_volumes;
#[cfg(test)]
mod visibility;
