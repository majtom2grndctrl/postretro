//! Id 57's streamed payload in id-50 chunks: one row per covered listed cell
//! whose streamed blocks carry a byte. A cell several clusters cover is
//! carried by each, as sparse rows are.
//! See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::CLUSTER_SH_MOVER_ROWS_BLOCK_KIND;
use postretro_level_format::mover_probe_channel::{
    MoverCellSlices, MoverChannelBlock, MoverPayloadPlacement, MoverProbeChannelSection,
};

use super::EncodedBlock;
use super::sparse::affinity_indices;

/// Id 57 with a streamed payload, and each listed cell's payload slices.
pub(super) struct MoverRows<'a> {
    section: &'a MoverProbeChannelSection,
    slices: Vec<MoverCellSlices>,
}

impl<'a> MoverRows<'a> {
    /// `None` unless a block's payload streams. Fails by name when a listed
    /// cell's row for a streamed block passes `u32`, the row's length field.
    /// Level-format sizes the rows of a section it never decoded on trust,
    /// so the compiler refuses here, before any row is sized or packed.
    pub(super) fn of(
        section: Option<&'a MoverProbeChannelSection>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(section) = section.filter(|section| section.streams()) else {
            return Ok(None);
        };
        let slices = section.cell_slices();
        for block in MoverChannelBlock::BOTH {
            if section.placement(block) != MoverPayloadPlacement::Streamed {
                continue;
            }
            for (listed, cell) in section.cells.iter().zip(&slices) {
                check_row_len(listed.cell, block, streamed_row_bytes(cell, block))?;
            }
        }
        Ok(Some(Self { section, slices }))
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

/// The bytes one listed cell's id-50 row carries for `block` when it
/// streams: the direct block's visibility then its own tiles, or the bounce
/// block's tiles, each tile value a two-byte half.
fn streamed_row_bytes(cell: &MoverCellSlices, block: MoverChannelBlock) -> u64 {
    match block {
        MoverChannelBlock::Direct => cell.visibility.len() as u64 + 2 * cell.own_tiles.len() as u64,
        MoverChannelBlock::Bounce => 2 * cell.bounce_tiles.len() as u64,
    }
}

/// Refuse a row past `u32` bytes, naming the block and the dense affinity
/// cell.
fn check_row_len(cell: u32, block: MoverChannelBlock, bytes: u64) -> anyhow::Result<()> {
    if bytes > u64::from(u32::MAX) {
        anyhow::bail!(
            "MoverProbeChannel (id 57): the {block} block's streamed row for affinity cell \
             {cell} holds {bytes} bytes, past the {} one id-50 row can carry. Lower the \
             movers' reach into that cell (fewer movers, poses or lights over it)",
            u32::MAX
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_sums_its_block_payload_with_tiles_as_two_byte_halves() {
        let cell = MoverCellSlices {
            visibility: 4..7,
            own_tiles: 10..12,
            bounce_tiles: 20..25,
        };
        assert_eq!(
            streamed_row_bytes(&cell, MoverChannelBlock::Direct),
            3 + 2 * 2
        );
        assert_eq!(streamed_row_bytes(&cell, MoverChannelBlock::Bounce), 5 * 2);
    }

    // Regression: the compiler packed streamed rows of a section it
    // assembled and never decoded, so a row past `u32` reached level-format's
    // row sizing, which panics there instead of failing the build by name.
    #[test]
    fn a_streamed_row_past_u32_fails_by_block_and_cell() {
        check_row_len(7, MoverChannelBlock::Bounce, u64::from(u32::MAX))
            .expect("a row of exactly u32::MAX bytes fits its length field");
        // Each part fits alone; the visibility and its own tiles together
        // pass the field.
        let cell = MoverCellSlices {
            visibility: 0..u32::MAX as usize,
            own_tiles: 0..1,
            bounce_tiles: 0..0,
        };
        let bytes = streamed_row_bytes(&cell, MoverChannelBlock::Direct);
        assert_eq!(bytes, u64::from(u32::MAX) + 2);
        let error = check_row_len(7, MoverChannelBlock::Direct, bytes)
            .expect_err("past the row's length field")
            .to_string();
        let bytes_named = bytes.to_string();
        for named in [
            "MoverProbeChannel (id 57)",
            "direct block",
            "affinity cell 7",
            bytes_named.as_str(),
        ] {
            assert!(error.contains(named), "{named:?} in {error}");
        }
    }
}
