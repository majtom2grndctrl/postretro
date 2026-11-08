// Mover probe channel: what a mover posed at a terminus changes in the baked
// probe data. Today: the direct half's joint soft visibility, and the bounce
// and closed-pose depth re-bake over the SH base's patches.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod bake;
mod bounce;
pub(crate) mod dev_bounce;
pub(crate) mod dev_pose;
mod domain;
mod gather;
mod posed_scene;
mod visibility;

#[cfg(test)]
mod tests;

pub(crate) use bake::{MoverBounceRequest, PosedBounce, StaticBounce, bake_mover_bounce};
pub(crate) use gather::PosedGather;
pub(crate) use posed_scene::{PosedScene, PosedSurfaces};
pub(crate) use visibility::{
    INSIDE_BRUSH_RULE, InsideBrushRule, JointVisibility, PosedOccluder, joint_soft_visibility,
};
