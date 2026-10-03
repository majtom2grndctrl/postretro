//! App-side SH cluster residency policy and loader-to-renderer handoff.
//! See: context/lib/rendering_pipeline.md §4

pub(crate) mod budget;
pub(crate) mod controller;
pub(crate) mod generation;
#[cfg(test)]
pub(crate) mod sync_manifest_test_fixture;
mod topology;
#[cfg(test)]
mod topology_test_fixtures;
#[cfg(test)]
pub(crate) mod trace_fixture;
mod warm_set;
