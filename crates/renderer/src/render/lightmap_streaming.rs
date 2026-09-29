// Renderer entry points for streamed lightmap cell blocks: the per-frame
// drain and the pool counters.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome};

use super::Renderer;
use crate::lighting::lightmap::{LightmapResidencyDrainError, LightmapStreamCounters};

/// Default pool cap in layers (15 × 14 MiB = 210 MiB of BC pool at the
/// default lead). The first generation holds this many layers, or fewer
/// when every block fits in less.
pub const DEFAULT_LIGHTMAP_POOL_CAP_LAYERS: u32 = 15;

impl Renderer {
    /// Execute the controller's lightmap drain batch. Call once per frame,
    /// before `render_frame_indirect` or a capture frame records the forward
    /// pass, so the frame samples what this drain made resident.
    ///
    /// The batch is validated before anything changes. A batch from an
    /// earlier generation (including a previous level's, after a reload) is
    /// rejected. Growth, repack moves, pair uploads and table writes go to
    /// the GPU in one submission; a pair becomes sampleable in the drain that
    /// uploads all of its planes, never with half of them.
    pub fn drain_lightmap_residency(
        &mut self,
        batch: LightmapDrainBatch,
    ) -> Result<LightmapDrainOutcome, LightmapResidencyDrainError> {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let (outcome, meter_changed) = full
            .lightmap_resources
            .drain_streaming(device, queue, batch)?;
        if meter_changed {
            full.lightmap_residency_report = full.lightmap_residency_report.with_static_rows(
                full.lightmap_resources.residency.clone(),
                full.lightmap_resources.retiring_bytes(),
            );
        }
        Ok(outcome)
    }

    /// Counters of the installed level's streamed lightmap pool, `None` when
    /// the level does not stream its blocks.
    pub fn lightmap_stream_counters(&self) -> Option<LightmapStreamCounters> {
        self.full.as_ref()?.lightmap_resources.stream_counters()
    }
}
