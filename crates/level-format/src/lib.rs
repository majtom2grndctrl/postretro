// Shared PRL container format and optional glTF asset-path resolution.
// See: context/lib/build_pipeline.md §PRL Compilation, §Baked texture mips

pub mod alpha_lights;
pub mod animated_billboard_direct_scatter_delta_volumes;
pub mod animated_direct_sh_delta_volumes;
pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod animated_lightmap_atlas;
pub mod billboard_direct_scatter_volume;
pub mod bsp;
pub mod bvh;
pub mod cell_draw_index;
pub mod cell_locator;
pub mod cell_visibility;
pub mod cells;
pub mod chunk_light_list;
pub mod cluster_directory;
pub mod data_script;
pub mod delta_sh_volumes;
pub mod direct_sh_delta_volumes;
pub mod direct_sh_volume;
pub mod entity_shadow_lights;
pub mod fog_cell_masks;
pub mod fog_volumes;
pub mod geometry;
#[cfg(feature = "gltf-resolve")]
pub mod gltf_resolve;
pub mod kinematic_geometry;
pub mod light_influence;
pub mod light_membership;
pub mod light_tags;
pub mod lightmap;
pub mod map_entity;
pub mod navmesh;
pub mod octahedral;
pub mod portals;
pub mod prm;
pub mod sdf_atlas;
pub mod sh_reconstruct;
pub mod sh_volume;
pub mod shadowmask_atlas;
pub mod sprite_collection;
pub mod texture_cache_keys;
pub mod texture_names;
pub mod trigger_volumes;

#[cfg(test)]
mod prm_test_fixtures;

mod container;
mod section_registry;

pub use container::{
    CURRENT_VERSION, ContainerMeta, FormatError, Header, MAGIC, Result, SectionBlob,
    SectionDescriptor, SectionEntry, read_container, read_section_data, section_data_from_bytes,
    write_prl, write_prl_header_and_table,
};
pub use section_registry::SectionId;
