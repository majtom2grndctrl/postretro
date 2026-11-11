// Mover probe channel: what a mover posed along its path changes in the
// baked probe data. The direct half bakes each gated light's surviving visibility
// (id 57's direct block); the bounce half re-bakes the SH base with each
// mover posed (id 57's bounce block), and the same posed gather bins its
// closed-pose depth tiles (id 57's depth block).
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod absent_bounces;
mod absent_memo;
mod across;
mod bake;
mod blocks;
mod bounce;
mod cost;
mod depth;
mod direct;
mod direct_memo;
mod domain;
mod gather;
mod lights;
mod mid_pose;
mod near_field;
mod near_field_bounce;
mod pose_bake;
mod pose_lookup;
mod pose_memo;
mod pose_payload;
mod posed_scene;
mod posed_solid;
mod section;
mod skip;
mod states;
mod visibility;

#[cfg(test)]
mod direct_tests;
#[cfg(test)]
mod fixture_cost_tests;
#[cfg(test)]
pub(crate) mod fixture_scene;
#[cfg(test)]
mod later_bounce_tests;
#[cfg(test)]
mod raised_room_tests;
#[cfg(test)]
pub(crate) mod reference_maps;
#[cfg(test)]
mod tests;

pub(crate) use absent_bounces::AbsentLaterBounces;
pub(crate) use absent_memo::AbsentBounceMemo;
#[cfg(test)]
pub(crate) use across::EnclosedMidPoses;
pub(crate) use bake::{
    MoverBounceBake, MoverBounceJob, MoverBounceRequest, StaticBounce, bake_mover_bounce,
    held_progress_units, sh_base_bounce_count,
};
pub(crate) use blocks::BounceBlocks;
pub(crate) use cost::MoverChannelCost;
pub(crate) use depth::{CarriedDepth, DepthReservation, MoverAbsentDepth, reserve_mover_tiles};
pub(crate) use direct::{DirectScene, MoverDirectBake, ProbeCells, bake_direct};
#[cfg(test)]
pub(crate) use domain::direct_display_levels;
#[cfg(test)]
pub(crate) use domain::{
    BOUNCE_FLOOR_LEVELS, compact_tile, display_levels, max_abs_difference, round_trip_error,
};
pub(crate) use gather::PosedGather;
#[cfg(test)]
pub(crate) use gather::{DomainProbe, compact_slots};
pub(crate) use lights::{ChannelLightSources, channel_lights};
pub(crate) use near_field::NearFieldDomain;
#[cfg(test)]
pub(crate) use pose_bake::PosedBounce;
pub(crate) use pose_lookup::{HitReduction, MoverPoseBounces, PoseLookup};
pub(crate) use pose_memo::PoseMemo;
pub(crate) use posed_scene::PosedScene;
pub(crate) use posed_solid::PosedMovers;
#[cfg(test)]
pub(crate) use posed_solid::enclosure_inset;
pub(crate) use section::{EmittedDirectEntries, MoverPayloadBars, MoverRecords, channel_section};
pub(crate) use skip::{SkipBars, SkipCounts, skip_redundant_mid_poses};
#[cfg(test)]
pub(crate) use states::BounceProbe;
pub(crate) use states::MoverBounceStates;
pub(crate) use visibility::{JointVisibility, PosedOccluder, joint_soft_visibility};
