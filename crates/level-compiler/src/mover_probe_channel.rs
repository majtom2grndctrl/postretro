// Mover probe channel: what a mover posed at a terminus changes in the baked
// probe data. Today: the direct half's joint soft visibility.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

pub(crate) mod dev_pose;
mod visibility;

#[cfg(test)]
mod tests;

pub(crate) use visibility::{
    INSIDE_BRUSH_RULE, InsideBrushRule, JointVisibility, PosedOccluder, joint_soft_visibility,
};
