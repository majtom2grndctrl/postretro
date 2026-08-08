// Level-compiler library target. Exposes the compiler's modules so
// workspace-internal tools and cross-crate tests can call internals — the
// bc5/texture_mips helpers and the navmesh bake with its geometry inputs. The
// module set mirrors `main.rs` except the CLI orchestration layer (`pipeline`,
// `tui`), which references items defined in the binary root scope and stays
// main-only. Both crate roots share the source files, so the lib compiles the
// same graph the binary does (minus that orchestration).
// See: context/lib/build_pipeline.md §PRL Compilation, §Navigation bake

pub mod affinity_grid;
pub mod animated_direct_sh_bake;
pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bake_control;
pub mod bc5;
pub mod bc6h;
pub mod bvh_build;
pub mod cache;
pub mod cell_draw_index_bake;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_drop_policy;
pub mod delta_sections;
pub mod delta_sh_bake;
pub mod direct_sh_bake;
pub mod entity_shadow_select;
#[cfg(test)]
pub mod fixture_pipeline;
pub mod fog_cell_masks;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod governor;
pub mod kinematic_geometry;
pub mod light_namespaces;
pub mod lightmap_bake;
pub mod lightmap_layer;
pub mod logger;
pub mod map_data;
pub mod map_format;
pub mod navmesh_bake;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod reporter;
pub mod script_light_membership;
pub mod sdf_bake;
pub mod sh_bake;
pub mod sh_group;
pub mod shadowmask_bake;
pub mod size_options;
pub mod stage;
pub mod texture_mips;
pub mod texture_validation;
pub mod trigger_volumes;
pub mod visibility;
