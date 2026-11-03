//! Mover light gate: the baked lights each kinematic mover blocks at either end of its travel.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

mod animated;
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
pub(crate) use lights::GateLights;
pub(crate) use pose::MoverPoses;
pub(crate) use probes::{LightProbes, ProbeScene, gate_probes};
pub(crate) use report::{log_rows, log_side_counts};
pub(crate) use rows::MoverTexels;
pub(crate) use side_count::side_counts;
pub(crate) use summary::{LIGHTMAP_FAMILY, MoverLightSummary};
pub(crate) use tally::GateTally;
pub(crate) use texels::{CandidateMover, gate_partition, light_candidates};
