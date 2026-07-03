// Renderer crate root: GPU-owned passes, uploads, culling, and presentation.
// See: context/lib/rendering_pipeline.md

mod candidate_cull;
mod compute_cull;
mod lighting;
mod render;
mod shadow_cull;

pub use candidate_cull::{GatherStatus, gather_candidate_leaves};
pub use render::*;
