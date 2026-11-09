// Mover probe channel (PRL id 57): the renderer-owned direct table both
// direct-SH compose passes read, the bounce table the indirect SH compose
// reads, the per-frame mover t′ block inside each, each streamed block's
// per-cell residency, and the per-mover rows the streamed planner recomposes
// when a mover travels. Mover state and residency drive every write here; no
// light change ever does.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

mod bounce;
mod gpu;
mod layout;
mod pool;
mod state;

#[cfg(test)]
mod bounce_compose_tests;
#[cfg(test)]
mod compose_tests;
#[cfg(test)]
mod tests;

pub(in crate::render) use gpu::{
    ClusterChannelWrites, ClusterSparseRow, ClusterTermRows, MoverChannelResources,
};
#[cfg(test)]
pub(in crate::render) use layout::NO_MOVER_ROWS;
pub(in crate::render) use layout::{
    MoverChannelError, MoverChannelInputs, MoverChannelRows, TermFamily, WholeTermRows,
};

/// The WGSL reads both direct compose passes concatenate after their own
/// source; each binds `mover_channel` at binding 25 of its compose group.
pub(in crate::render) const MOVER_CHANNEL_WGSL: &str =
    include_str!("../../shaders/mover_channel.wgsl");

/// The binding both direct compose passes give the table.
pub(in crate::render) const BIND_MOVER_CHANNEL: u32 = 25;

/// The WGSL reads the indirect SH compose concatenates after its own source;
/// it binds `mover_bounce` at binding 25 of its compose group.
pub(in crate::render) const MOVER_BOUNCE_WGSL: &str =
    include_str!("../../shaders/mover_bounce.wgsl");
