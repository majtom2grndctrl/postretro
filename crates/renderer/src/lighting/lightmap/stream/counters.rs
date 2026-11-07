// Always-on counters of streamed lightmap pool work, read by diagnostics.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_render_cpu::lightmap_pool::PerPool;

/// Cumulative and last-drain figures of the renderer's streamed lightmap and
/// mask pools. Plain values: callers never see a pool slot or a GPU handle.
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
    /// Never-refused pairs deferred because growth waited on a retiring pool
    /// or would pass the device's array-layer limit.
    pub deferred_pairs: u64,
    /// Blocks released: untargeted, or evicted for room.
    pub evictions: u64,
    /// Drains that repacked each pool in place through its spare layer, by
    /// either pass.
    pub repacks: PerPool<u64>,
    /// Of those, drains that escalated to compaction.
    pub compactions: PerPool<u64>,
    /// Compactions that found no copy order and fell through to growth.
    pub compaction_rollbacks: PerPool<u64>,
    /// Same-texture moves those repacks recorded. A move within one layer
    /// stages through the spare layer and takes two; a move to another layer
    /// takes one, or more when the item parks on the way.
    pub repack_copy_commands: PerPool<u64>,
    /// Drains that grew a new generation of each pool.
    pub growths: PerPool<u64>,
    /// Texture sets created for each pool on this level: the first plus one
    /// per growth, none for a pool with nothing to hold. A repack never adds
    /// one.
    pub pool_texture_sets: PerPool<u32>,
    /// Usable layers of each pool's live generation (its textures have one
    /// more).
    pub pool_layers: PerPool<u32>,
    /// Requested bytes of each pool's live textures, spare layer included.
    pub active_pool_bytes: PerPool<u64>,
    /// Requested bytes of each pool's retiring generation, zero when none.
    pub retiring_pool_bytes: PerPool<u64>,
    /// Largest live-plus-retiring bytes both pools held at once after a growth.
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

impl LightmapStreamCounters {
    /// Live and retiring bytes of both pools.
    pub fn held_bytes(&self) -> u64 {
        self.active_pool_bytes.total() + self.retiring_pool_bytes.total()
    }
}
