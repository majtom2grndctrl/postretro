// Mover probe channel: what a mover posed at a terminus changes in the baked
// probe data. The direct half bakes each gated light's surviving visibility
// (id 57's direct block); the bounce and closed-pose depth re-bake over the
// SH base's patches does not emit yet, so id 57's bounce and depth blocks
// stay empty.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod bake;
mod bounce;
pub(crate) mod dev_bounce;
pub(crate) mod dev_floor;
pub(crate) mod dev_pose;
pub(crate) mod dev_trace;
mod direct;
mod direct_memo;
mod domain;
mod gather;
mod lights;
mod posed_scene;
mod section;
mod visibility;

#[cfg(test)]
mod direct_tests;
#[cfg(test)]
mod tests;

pub(crate) use bake::{MoverBounceRequest, PosedBounce, StaticBounce, bake_mover_bounce};
pub(crate) use direct::{DirectScene, MoverDirectBake, ProbeCells, bake_direct};
pub(crate) use gather::PosedGather;
pub(crate) use lights::{ChannelLightSources, channel_lights};
pub(crate) use posed_scene::{PosedScene, PosedSurfaces};
pub(crate) use section::{EmittedDirectEntries, MoverRamps, channel_section};
pub(crate) use visibility::{
    INSIDE_BRUSH_RULE, InsideBrushRule, JointVisibility, PosedOccluder, joint_soft_visibility,
};
