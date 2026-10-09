//! Resource-neutral streaming layer shared by SH clusters and lightmap blocks.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

pub(crate) mod cell_demand;
pub(crate) mod cluster_hints;
pub(crate) mod drain_budget;
pub(crate) mod issuer;
#[cfg(test)]
pub(crate) mod read_gate_test_fixture;
pub(crate) mod request;
pub(crate) mod schedule;
pub(crate) mod shared_drain;
pub(crate) mod target_bitset;
