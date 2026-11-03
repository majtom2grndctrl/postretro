//! Mover light gate: the baked lights each kinematic mover blocks at either end of its travel.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod pose;
mod report;
mod segment;
mod tally;
mod texels;

#[cfg(test)]
mod tests;

pub(crate) use pose::MoverPoses;
pub(crate) use report::log_static_rows;
pub(crate) use tally::GateTally;
pub(crate) use texels::{CandidateMover, MoverTexels, gate_partition, light_candidates};
