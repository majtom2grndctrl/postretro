// Bake-only surface radiance cache: patches on static faces that the SH probe gather reads.
// See: context/lib/build_pipeline.md §Compiler pipeline

mod gather;
mod layer;
mod patches;
mod sum;

pub(crate) use gather::{PatchGather, PatchLookup};
pub(crate) use layer::{SparseLayer, shade_light_layer};
pub(crate) use patches::{PATCH_SIZE_M, Patch, PatchSet, Side, patch_seed_id};
pub(crate) use sum::PatchRadiance;

#[cfg(test)]
mod tests;
