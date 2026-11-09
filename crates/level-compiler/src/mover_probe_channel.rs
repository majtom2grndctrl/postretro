// Mover probe channel: what a mover posed at a terminus changes in the baked
// probe data. The direct half bakes each gated light's surviving visibility
// (id 57's direct block); the bounce half re-bakes the SH base with each
// mover posed (id 57's bounce block), and the same posed gather bins its
// closed-pose depth tiles (id 57's depth block).
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod bake;
mod blocks;
mod bounce;
mod bounce_memo;
mod cost;
mod depth;
mod direct;
mod direct_memo;
mod domain;
mod gather;
mod lights;
mod posed_scene;
mod posed_solid;
mod section;
mod states;
mod visibility;

#[cfg(test)]
mod direct_tests;
#[cfg(test)]
mod fixture_cost_tests;
#[cfg(test)]
mod fixture_scene;
#[cfg(test)]
mod raised_room_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use bake::PosedBounce;
pub(crate) use bake::{
    MoverBounceJob, MoverBounceRequest, StaticBounce, StaticTail, bake_mover_bounce,
    sh_base_bounce_count,
};
pub(crate) use blocks::BounceBlocks;
pub(crate) use bounce_memo::BounceMemo;
pub(crate) use cost::MoverChannelCost;
pub(crate) use depth::{DepthReservation, reserve_mover_tiles};
pub(crate) use direct::{DirectScene, MoverDirectBake, ProbeCells, bake_direct};
#[cfg(test)]
pub(crate) use domain::{
    BOUNCE_FLOOR_LEVELS, compact_tile, display_levels, max_abs_difference, round_trip_error,
};
pub(crate) use gather::PosedGather;
#[cfg(test)]
pub(crate) use gather::compact_slots;
pub(crate) use lights::{ChannelLightSources, channel_lights};
pub(crate) use posed_scene::PosedScene;
pub(crate) use posed_solid::PosedMovers;
pub(crate) use section::{EmittedDirectEntries, MoverPayloadBars, MoverRamps, channel_section};
pub(crate) use states::MoverBounceStates;
pub(crate) use visibility::{JointVisibility, PosedOccluder, joint_soft_visibility};
