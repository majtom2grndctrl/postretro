// Plain-data live view of streamed lightmap residency, and the dev-tools levers.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Nothing here is feature-gated: the application's periodic log line and the
// capture report read these values in every build; the dev-tools Streaming
// tab only renders them and edits the levers.

use crate::lighting::lightmap::LightmapStreamCounters;

/// One frame's view of lightmap block streaming. The application fills the
/// controller, route and file-read fields; the renderer's pool fields come
/// from [`LightmapStreamCounters`] via [`Self::record_renderer_counters`].
///
/// Bytes are per section: "lightmap" is id 22 (irradiance plus direction),
/// "shadowmask" is id 42 (both groups). Gauges describe the current frame;
/// every other count is cumulative since the level's streaming began.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LightmapStreamingLiveDiagnostics {
    // Levers in force.
    pub pool_cap_layers: u32,
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
    /// Texels free under the cap for band prefetch, from the latest outcome.
    pub band_headroom_texels: u64,
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
    /// Visible misses, two disjoint buckets of drawn block-frames: drawn but
    /// outside the baked set at lead L, and drawn in it but not yet resident.
    pub drawn_outside_baked_set: u64,
    pub drawn_not_resident: u64,
    pub last_frame_drawn_outside_baked_set: u32,
    pub last_frame_drawn_not_resident: u32,
    // File reads.
    /// Positional reads the issuer performed for lightmap pairs.
    pub physical_reads: u64,
    /// Bytes read from id 22 and id 42 by every reader of the level's file:
    /// the loader's index read, preloads, and streaming reads.
    pub lightmap_bytes_read: u64,
    pub shadowmask_bytes_read: u64,
    // Renderer pool.
    pub pool_layers: u32,
    /// Most layers the active pool has held this level.
    pub peak_pool_layers: u32,
    /// Requested bytes of the active pool textures, spare layer included.
    pub pool_bytes: u64,
    pub retiring_pool_bytes: u64,
    /// Largest active-plus-retiring bytes any growth held at once.
    pub growth_transient_peak_bytes: u64,
    pub repacks: u64,
    pub growths: u64,
    pub last_drain_install_micros: u64,
    pub max_drain_install_micros: u64,
}

impl LightmapStreamingLiveDiagnostics {
    /// Copy the renderer-owned pool counters and raise the peak layer gauge.
    /// Controller, route and read fields are left untouched.
    pub fn record_renderer_counters(&mut self, counters: &LightmapStreamCounters) {
        self.pool_layers = counters.pool_layers;
        self.peak_pool_layers = self.peak_pool_layers.max(counters.pool_layers);
        self.pool_bytes = counters.active_pool_bytes;
        self.retiring_pool_bytes = counters.retiring_pool_bytes;
        self.growth_transient_peak_bytes = counters.growth_transient_peak_bytes;
        self.repacks = counters.repacks;
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

/// The two lightmap residency levers as the dev-tools Streaming tab edits
/// them. Developer levers, never a player setting
/// (`context/lib/experimental_spikes.md` §Tuning levers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightmapStreamingLevers {
    /// Pool cap in layers, `1..=`[`MAX_LIGHTMAP_POOL_CAP_LAYERS`]. Rides the
    /// next drain batch.
    pub pool_cap_layers: u32,
    /// Lead L in metres, `0..=max_lead_metres`. Takes effect at the next
    /// demand update.
    pub lead_metres: f32,
    /// The level's baked maximum lead; read-only.
    pub max_lead_metres: f32,
}

/// Largest pool cap the slider offers: a 256-layer texture array (WebGPU's
/// default `max_texture_array_layers`) less the reserved spare layer.
pub const MAX_LIGHTMAP_POOL_CAP_LAYERS: u32 = 255;

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
            pool_layers: 9,
            active_pool_bytes: 1000,
            repacks: 2,
            growths: 1,
            growth_transient_peak_bytes: 3000,
            last_drain_install_micros: 40,
            max_drain_install_micros: 900,
            ..LightmapStreamCounters::default()
        });
        live.record_renderer_counters(&LightmapStreamCounters {
            pool_layers: 6,
            ..LightmapStreamCounters::default()
        });
        assert_eq!(live.pool_layers, 6);
        assert_eq!(live.peak_pool_layers, 9, "the peak survives a smaller pool");
        assert_eq!(live.installs, 7, "controller fields are untouched");
    }
}
