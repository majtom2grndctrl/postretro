// Bake-only surface radiance cache: patches on static faces that the SH probe gather reads.
// See: context/lib/build_pipeline.md §Compiler pipeline

mod bounce;
mod gather;
mod layer;
mod layer_set;
mod patches;
mod reach;
mod stage;
mod sum;
mod volume;
mod working_set;

#[cfg(test)]
pub(crate) use bounce::add_later_bounces;
pub(crate) use bounce::{
    BOUNCE_RAYS_PER_PATCH, DEFAULT_BOUNCE_FILL, LATER_BOUNCE_COUNT, cosine_directions,
    is_valid_bounce_fill, patch_frame,
};
pub(crate) use gather::{PatchGather, PatchLookup};
pub(crate) use layer::shade_patch_side;
#[cfg(test)]
pub(crate) use layer::{
    LayerKeys, LightDigest, SH_PATCH_LAYER_STAGE_ID, SH_PATCH_LAYER_STAGE_VERSION, SparseLayer,
    shade_light_layer,
};
pub(crate) use layer_set::LayerPlan;
#[cfg(test)]
pub(crate) use patches::{PATCH_SIZE_M, patch_seed_id};
pub(crate) use patches::{Patch, PatchCountExceeded, PatchSet, Side};
pub(crate) use reach::PatchReachIndex;
#[cfg(test)]
pub(crate) use stage::bake_sh_base;
pub(crate) use stage::{ShBase, bake_sh_base_with_movers, warn_bounce_only_emitters};
pub(crate) use sum::PatchRadiance;
#[cfg(test)]
pub(crate) use volume::{SH_BASE_VOLUME_STAGE_ID, SH_BASE_VOLUME_STAGE_VERSION, volume_key};
pub(crate) use working_set::{ShBaseWorkingSetProjection, plan_sh_base};

#[cfg(test)]
mod bounce_tests;
#[cfg(test)]
mod cost_tests;
#[cfg(test)]
mod determinism_tests;
#[cfg(test)]
mod layer_cache_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod test_scene;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod volume_cache_tests;
#[cfg(test)]
mod working_set_tests;
