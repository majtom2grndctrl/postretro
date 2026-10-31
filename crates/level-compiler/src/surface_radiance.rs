// Bake-only surface radiance cache: patches on static faces that the SH probe gather reads.
// See: context/lib/build_pipeline.md §Compiler pipeline

mod gather;
mod layer;
mod layer_set;
mod patches;
mod reach;
mod stage;
mod sum;
mod working_set;

pub(crate) use gather::PatchGather;
#[cfg(test)]
pub(crate) use gather::PatchLookup;
#[cfg(test)]
pub(crate) use layer::{SparseLayer, shade_light_layer};
#[cfg(test)]
pub(crate) use patches::{PATCH_SIZE_M, Patch, PatchSet, Side, patch_seed_id};
#[cfg(test)]
pub(crate) use reach::PatchReachIndex;
pub(crate) use stage::{bake_sh_base, warn_bounce_only_emitters};
#[cfg(test)]
pub(crate) use sum::PatchRadiance;
pub(crate) use working_set::{ShBaseWorkingSetProjection, plan_sh_base};

#[cfg(test)]
mod cost_tests;
#[cfg(test)]
mod layer_cache_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod test_scene;
#[cfg(test)]
mod tests;
