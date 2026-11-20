// Plain-data live view of streamed lightmap residency, and the dev-tools lead lever.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Nothing here is feature-gated: the application's periodic log line and the
// capture report read these values in every build; the dev-tools Streaming
// tab only renders them and edits the lead.

use postretro_render_cpu::lightmap_pool::PerPool;

use crate::lighting::lightmap::LightmapStreamCounters;

/// Visible lightmap misses by cause, in drawn block-frames: a block drawn
/// while not installed counts once per counted frame, in exactly one bucket,
/// so the buckets sum to `drawn_not_resident`. A bucket takes SH's name
/// ([`crate::ShMissBuckets`], counted in episodes) where the cause is the same.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LightmapMissBuckets {
    /// The frame may not read it: untargeted, or a non-portal frame holds
    /// reads to the camera cell's baked set.
    pub outside_reach: u64,
    /// Its read is in flight, or it waits for the issuer.
    pub read_in_flight: u64,
    /// The frame's request pass stopped before it with every permit held, or
    /// the band's half of them.
    pub waiting_on_permit: u64,
    /// The frame's request pass stopped before it at the in-hand pair bytes,
    /// or the band's half of them.
    pub waiting_on_in_hand_cap: u64,
    /// Ready, offered to this frame's drain, and past the shared budget's cut.
    pub held_by_drain_budget: u64,
    /// Ready or drained, but not offered to this frame's drain: the renderer
    /// had not returned the previous lightmap batch.
    pub awaiting_renderer_drain: u64,
    /// This frame's latest drain to carry it handed it back unplaced: a pool
    /// it must grow was retiring, or growth would pass the device limit.
    pub deferred_by_pool: u64,
    /// A band pair the pool refused, or whose read waits for band headroom.
    pub refused: u64,
    /// Its read, decode or install failed, or its drain rolled back, this
    /// frame or (a failed pair) earlier.
    pub failed: u64,
}

impl LightmapMissBuckets {
    pub fn total(&self) -> u64 {
        self.outside_reach
            + self.read_in_flight
            + self.waiting_on_permit
            + self.waiting_on_in_hand_cap
            + self.held_by_drain_budget
            + self.awaiting_renderer_drain
            + self.deferred_by_pool
            + self.refused
            + self.failed
    }
}

/// One frame's view of lightmap block streaming. The application fills the
/// controller, route and file-read fields; the renderer's pool fields come
/// from [`LightmapStreamCounters`] via [`Self::record_renderer_counters`].
///
/// Bytes are per section: "lightmap" is id 22 (irradiance plus direction),
/// "shadowmask" is id 42 (the present planes). Gauges describe the current frame;
/// every other count is cumulative since the level's streaming began.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LightmapStreamingLiveDiagnostics {
    // Lead in force.
    pub lead_metres: f32,
    pub max_lead_metres: f32,
    // Residency gauges (controller).
    pub block_count: u64,
    pub resident_blocks: u64,
    pub resident_lightmap_bytes: u64,
    pub resident_shadowmask_bytes: u64,
    /// The current camera cell's baked set within lead L, plus the pins.
    pub mandatory_blocks: u64,
    pub mandatory_lightmap_bytes: u64,
    pub mandatory_shadowmask_bytes: u64,
    /// Texels free in each pool for band prefetch, from the latest outcome.
    pub band_headroom_texels: PerPool<u64>,
    /// Read bytes delivered by the issuer and not yet taken by the controller.
    pub in_flight_read_bytes: u64,
    // Controller counters.
    pub reads_requested: u64,
    pub installs: u64,
    pub evictions: u64,
    pub refusals: u64,
    pub deferrals: u64,
    pub failed_reads: u64,
    pub failed_installs: u64,
    pub cancelled_reads: u64,
    /// Visible misses in drawn block-frames, two buckets that may overlap:
    /// drawn but outside the baked set at lead L, and drawn but not resident.
    pub drawn_outside_baked_set: u64,
    pub drawn_not_resident: u64,
    pub last_frame_drawn_outside_baked_set: u32,
    pub last_frame_drawn_not_resident: u32,
    /// `drawn_not_resident` by cause, in block-frames; the buckets sum to it.
    pub miss_buckets: LightmapMissBuckets,
    // File reads.
    /// Positional reads the issuer performed for lightmap pairs.
    pub physical_reads: u64,
    /// Bytes read from id 22 and id 42 by every reader of the level's file:
    /// the loader's index read, preloads, and streaming reads.
    pub lightmap_bytes_read: u64,
    pub shadowmask_bytes_read: u64,
    // Renderer pools.
    pub pool_layers: PerPool<u32>,
    /// Most layers each pool's live generation has held this level.
    pub peak_pool_layers: PerPool<u32>,
    /// Requested bytes of each pool's live textures, spare layer included.
    pub pool_bytes: PerPool<u64>,
    pub retiring_pool_bytes: PerPool<u64>,
    /// Largest live-plus-retiring bytes both pools held at once after a growth.
    pub growth_transient_peak_bytes: u64,
    pub repacks: PerPool<u64>,
    /// Of those repacks, the ones that escalated to compaction.
    pub compactions: PerPool<u64>,
    /// Compactions that found no copy order: the layout stood, and the drain
    /// fell back to the downward pass, then growth.
    pub compaction_rollbacks: PerPool<u64>,
    pub growths: PerPool<u64>,
    pub last_drain_install_micros: u64,
    pub max_drain_install_micros: u64,
}

impl LightmapStreamingLiveDiagnostics {
    /// Copy the renderer-owned pool counters and raise each peak layer gauge.
    /// Controller, route and read fields are left untouched.
    pub fn record_renderer_counters(&mut self, counters: &LightmapStreamCounters) {
        self.pool_layers = counters.pool_layers;
        self.peak_pool_layers =
            PerPool::from_fn(|kind| self.peak_pool_layers[kind].max(counters.pool_layers[kind]));
        self.pool_bytes = counters.active_pool_bytes;
        self.retiring_pool_bytes = counters.retiring_pool_bytes;
        self.growth_transient_peak_bytes = counters.growth_transient_peak_bytes;
        self.repacks = counters.repacks;
        self.compactions = counters.compactions;
        self.compaction_rollbacks = counters.compaction_rollbacks;
        self.growths = counters.growths;
        self.last_drain_install_micros = counters.last_drain_install_micros;
        self.max_drain_install_micros = counters.max_drain_install_micros;
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident_lightmap_bytes + self.resident_shadowmask_bytes
    }

    pub fn mandatory_bytes(&self) -> u64 {
        self.mandatory_lightmap_bytes + self.mandatory_shadowmask_bytes
    }
}

/// The level's lead L as the dev-tools Streaming tab edits it: one reach for
/// every streamed resource, shown whenever the level has one. A developer
/// lever, never a player setting
/// (`context/lib/experimental_spikes.md` §Tuning levers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamingReachLever {
    /// Lead L in metres, `0..=max_lead_metres`. Takes effect at the next
    /// frame.
    pub lead_metres: f32,
    /// The level's baked maximum lead; read-only.
    pub max_lead_metres: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_counters_fill_pool_fields_and_keep_the_peak_layer_count() {
        let mut live = LightmapStreamingLiveDiagnostics {
            installs: 7,
            ..LightmapStreamingLiveDiagnostics::default()
        };
        live.record_renderer_counters(&LightmapStreamCounters {
            pool_layers: PerPool {
                lightmap: 9,
                mask: 4,
            },
            active_pool_bytes: PerPool {
                lightmap: 1000,
                mask: 500,
            },
            repacks: PerPool {
                lightmap: 2,
                mask: 1,
            },
            compactions: PerPool {
                lightmap: 1,
                mask: 1,
            },
            compaction_rollbacks: PerPool {
                lightmap: 0,
                mask: 1,
            },
            growths: PerPool {
                lightmap: 1,
                mask: 0,
            },
            growth_transient_peak_bytes: 3000,
            last_drain_install_micros: 40,
            max_drain_install_micros: 900,
            ..LightmapStreamCounters::default()
        });
        assert_eq!(
            (live.repacks, live.compactions, live.compaction_rollbacks),
            (
                PerPool {
                    lightmap: 2,
                    mask: 1
                },
                PerPool {
                    lightmap: 1,
                    mask: 1
                },
                PerPool {
                    lightmap: 0,
                    mask: 1
                },
            ),
            "each pool's repacks, compactions and rollbacks"
        );
        live.record_renderer_counters(&LightmapStreamCounters {
            pool_layers: PerPool {
                lightmap: 6,
                mask: 5,
            },
            ..LightmapStreamCounters::default()
        });
        assert_eq!(live.pool_layers.lightmap, 6);
        assert_eq!(
            live.peak_pool_layers,
            PerPool {
                lightmap: 9,
                mask: 5
            },
            "each pool keeps its own peak, through a smaller pool"
        );
        assert_eq!(live.installs, 7, "controller fields are untouched");
    }
}
