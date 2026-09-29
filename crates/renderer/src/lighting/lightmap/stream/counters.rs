// Always-on counters of streamed lightmap pool work, read by diagnostics.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

/// Cumulative and last-drain figures of the renderer's streamed lightmap
/// pool. Plain values: callers never see a pool slot or a GPU handle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LightmapStreamCounters {
    /// Drains accepted (validated and planned), no-op drains included.
    pub drains: u64,
    /// Drains that recorded and submitted GPU work.
    pub submissions: u64,
    /// Pairs uploaded, cumulative.
    pub installs: u64,
    /// Pairs whose payload did not match their block, failed whole.
    pub failed_installs: u64,
    /// Never-refused pairs deferred because growth waited on a retiring pool.
    pub deferred_pairs: u64,
    /// Blocks released: untargeted, over the cap, or evicted for room.
    pub evictions: u64,
    /// Drains that compacted the pool in place through the spare layer.
    pub repacks: u64,
    /// Same-texture block moves those repacks recorded.
    pub repack_copies: u64,
    /// Drains that grew a new pool generation.
    pub growths: u64,
    /// Pool texture sets created for this level: the first plus one per
    /// growth. A repack never adds one.
    pub pool_texture_sets: u32,
    /// Usable layers of the active generation (its texture has one more).
    pub pool_layers: u32,
    /// Requested bytes of the active pool textures, spare layer included.
    pub active_pool_bytes: u64,
    /// Requested bytes of the retiring generation, zero when none.
    pub retiring_pool_bytes: u64,
    /// Largest active-plus-retiring bytes any growth held at once.
    pub growth_transient_peak_bytes: u64,
    /// Block-table entries written, the install's whole table included.
    pub table_entries_written: u64,
    /// Entries the last drain wrote.
    pub last_drain_table_writes: u64,
    /// Pairs the last drain uploaded.
    pub last_drain_uploads: u64,
    /// Payload bytes the last drain staged for upload.
    pub last_drain_install_bytes: u64,
    /// CPU time of the last drain that submitted work, in microseconds:
    /// planning, staging, recording and submission.
    pub last_drain_install_micros: u64,
    /// Largest such drain time, in microseconds.
    pub max_drain_install_micros: u64,
}
