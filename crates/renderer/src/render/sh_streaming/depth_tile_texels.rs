//! A streamed level's side of a probe depth-tile reference rewrite: which
//! probes the seam may name, and the queued texels the next drain repacks.
//! See: context/lib/rendering_pipeline.md §4 (Probe depth moments, Cluster SH residency)

use super::*;
use crate::render::sh_depth_tile_rewrite::ProbeDepthTileRewriteError;

impl ShResidencyState {
    /// A rewrite may name only a valid probe of this level's lattice.
    pub(in crate::render) fn check_depth_tile_probe(
        &self,
        probe: u32,
    ) -> Result<(), ProbeDepthTileRewriteError> {
        let probes = &self.base_metadata.probes;
        match probes.get(probe as usize) {
            None => Err(ProbeDepthTileRewriteError::ProbeOutsideLattice {
                probe,
                probe_count: probes.len() as u32,
            }),
            Some(record) if record.validity == 0 => {
                Err(ProbeDepthTileRewriteError::InvalidProbe { probe })
            }
            Some(_) => Ok(()),
        }
    }

    /// Queue `probe`'s texel for the next drain, after the table changed. A
    /// probe not sampleable then needs nothing: its texel holds the invalid
    /// word, and the promotion that publishes it packs from the table.
    pub(in crate::render) fn queue_depth_tile_texel(&mut self, probe: u32) {
        self.pending_depth_tile_texels.insert(probe);
    }

    /// Repack every queued probe that is still sampleable into `uploads`, the
    /// drain's promotion and eviction batch. R and `E[d²]` come from the
    /// retained id-34 base metadata (the loader refuses an id-50 patch whose
    /// moments disagree with it, so promotion wrote the same bits), B/A from
    /// the sampled word, G from `references`.
    pub(super) fn flush_depth_tile_texels(
        &mut self,
        uploads: &mut StagedUploads,
        references: &ProbeDepthTileReferences,
    ) -> Result<(), ShResidencyDrainError> {
        if self.pending_depth_tile_texels.is_empty() {
            return Ok(());
        }
        let queued = std::mem::take(&mut self.pending_depth_tile_texels);
        let mut updates = Vec::with_capacity(queued.len());
        for dense in queued {
            let index = dense as usize;
            let word = *self
                .sampled_words
                .get(index)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            if word & PROBE_INDIRECTION_VALID_BIT == 0 {
                continue;
            }
            let probe = self
                .base_metadata
                .probes
                .get(index)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            updates.push((dense, probe.mean_distance, probe.mean_sq_distance));
        }
        if let Some(gpu) = self.gpu.as_ref()
            && !updates.is_empty()
        {
            gpu.upload_moments(
                uploads,
                &self.sampled_words,
                &updates,
                self.grid_dimensions(),
                references,
            )?;
        }
        Ok(())
    }
}
