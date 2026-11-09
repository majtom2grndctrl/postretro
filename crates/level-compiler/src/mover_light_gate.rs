//! Mover light gate: the baked lights each kinematic mover blocks at either end of its travel.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod animated;
mod cache;
mod lights;
mod pose;
mod probes;
mod report;
mod rows;
mod segment;
mod side_count;
mod summary;
mod tally;
mod texels;

#[cfg(test)]
mod tests;

pub(crate) use animated::{AnimatedScene, gate_animated};
pub(crate) use cache::{
    ANIMATED_MEMO, GateMemo, PROBE_MEMO, TEXEL_MEMO, fold_gate_light, gate_key_hasher,
};
pub(crate) use lights::{GateLight, GateLights};
pub(crate) use pose::{AnalysedMover, MoverPoses, Pose, Terminus};
pub(crate) use probes::{ProbeScene, gate_probes};
pub(crate) use report::{log_rows, log_side_counts};
pub(crate) use rows::MoverTexels;
pub(crate) use segment::{
    BOX_SLACK_M, boxes_may_touch, point_touches_brush, segment_may_touch_box,
    segment_touches_brush_within, sphere_touches_box,
};
pub(crate) use side_count::side_counts;
pub(crate) use summary::{LIGHTMAP_FAMILY, MoverLightSummary};
pub(crate) use tally::GateTally;
pub(crate) use texels::{CandidateMover, gate_partition, light_candidates};
