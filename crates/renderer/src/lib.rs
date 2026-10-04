// Renderer crate root: GPU-owned passes, uploads, culling, and presentation.
// See: context/lib/rendering_pipeline.md

mod candidate_cull;
mod compute_cull;
mod lighting;
mod render;
mod shadow_cull;
mod visible_span_draws;

pub use visible_span_draws::VisibleSpanRanges;

#[cfg(test)]
#[global_allocator]
static UPLOAD_TEST_ALLOCATOR: postretro_sim::alloc_probe::CountingAllocator =
    postretro_sim::alloc_probe::CountingAllocator;

pub use candidate_cull::{
    GatherStatus, gather_candidate_leaves, visibility_path_uses_candidate_cull,
};
pub use render::*;
