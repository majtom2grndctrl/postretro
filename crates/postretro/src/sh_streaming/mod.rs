//! App-side SH cluster residency policy and loader-to-renderer handoff.
//! See: context/lib/rendering_pipeline.md §4

pub(crate) mod budget;
pub(crate) mod controller;
pub(crate) mod generation;
mod topology;
#[cfg(test)]
mod topology_test_fixtures;
mod warm_set;
