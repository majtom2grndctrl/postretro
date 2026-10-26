// Skinned-mesh per-frame instance planning, mirror reach and cap occupancy under the shared budgets.
// See: context/lib/rendering_pipeline.md §9, §7.3a

mod frame_plan;
mod input;
mod planner;

pub use frame_plan::{
    MeshCapOccupancy, MeshFramePlan, MeshFramePlans, ModelDrawGroup, PlannedInstance,
    instance_casts_into_cone, planned_forward_sample_bounds,
};
pub use input::{MeshInstanceInput, MeshPaletteCacheKey, MeshPresentation, MirrorReach};
pub use planner::{
    JointCounts, MAX_INSTANCES, MAX_PALETTE_ENTRIES, plan_mesh_frame, plan_mesh_frame_plans,
};

#[cfg(test)]
#[path = "mesh_instances/tests.rs"]
mod tests;
