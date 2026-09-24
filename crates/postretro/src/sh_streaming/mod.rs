//! App-side SH cluster residency policy and loader-to-renderer handoff.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

pub(crate) mod budget;
pub(crate) mod controller;
pub(crate) mod generation;
mod topology;
#[cfg(test)]
mod topology_test_fixtures;
