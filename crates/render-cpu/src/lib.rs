//! CPU-only renderer data logic and shader-facing byte packing.
//! See: context/lib/rendering_pipeline.md

pub mod animated_lightmap;
pub mod chunk_list;
pub mod fog_mask;
pub mod frame_uniforms;
pub mod loaded_texture;
pub mod material_plan;
pub mod mesh_instances;
pub mod mesh_pass;
pub mod screen_effects;
pub mod sdf_atlas;
pub mod sdf_shadow;
pub mod sh_compose;
pub mod sh_volume;

pub mod fx {
    pub mod fog_volume;
    pub mod smoke;
}

pub use fx::{fog_volume, smoke};
