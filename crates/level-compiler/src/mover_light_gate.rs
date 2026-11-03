//! Mover light gate: the baked lights each kinematic mover blocks at either end of its travel.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod pose;
mod report;
mod rows;
mod segment;
mod side_count;
mod tally;
mod texels;

#[cfg(test)]
mod tests;

pub(crate) use pose::MoverPoses;
pub(crate) use report::{log_side_counts, log_static_rows};
pub(crate) use rows::MoverTexels;
pub(crate) use side_count::side_counts;
pub(crate) use tally::GateTally;
pub(crate) use texels::{CandidateMover, gate_partition, light_candidates};
