//! Id 57's streamed payload in id-50 chunks: one row per covered listed cell
//! whose streamed blocks carry a byte. A cell several clusters cover is
//! carried by each, as sparse rows are.
//! See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::CLUSTER_SH_MOVER_ROWS_BLOCK_KIND;
use postretro_level_format::mover_probe_channel::{MoverCellSlices, MoverProbeChannelSection};

use super::EncodedBlock;
use super::sparse::affinity_indices;

/// Id 57 with a streamed payload, and each listed cell's payload slices.
pub(super) struct MoverRows<'a> {
    section: &'a MoverProbeChannelSection,
    slices: Vec<MoverCellSlices>,
}

impl<'a> MoverRows<'a> {
    /// `None` unless a block's payload streams.
    pub(super) fn of(section: Option<&'a MoverProbeChannelSection>) -> Option<Self> {
        let section = section.filter(|section| section.streams())?;
        Some(Self {
            section,
            slices: section.cell_slices(),
        })
    }

    /// Cluster `cluster_id`'s row block, or `None` when it covers no listed
    /// cell with streamed bytes.
    pub(super) fn block(
        &self,
        directory: &ClusterDirectorySection,
        cluster_id: u32,
    ) -> anyhow::Result<Option<EncodedBlock>> {
        let cells = affinity_indices(directory, cluster_id, SectionId::MoverProbeChannel as u32)?;
        let rows = self.section.streamed_rows_for(&self.slices, cells);
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(EncodedBlock {
            section_id: SectionId::MoverProbeChannel as u32,
            kind: CLUSTER_SH_MOVER_ROWS_BLOCK_KIND,
            element_count: u32::try_from(rows.len())?,
            body: self.section.encode_streamed_rows(&self.slices, &rows),
        }))
    }
}
