//! Copy planning for id-50 isolated-atlas tile uploads.
//!
//! A cluster's node closure arrives in its own compact atlas, and each
//! canonical node lands on a contiguous live slot range. One copy region is a
//! run of consecutive live slots inside one live atlas row. Source tiles are
//! gathered per tile, so the closure-local layout never splits a region; only
//! the live allocation does.

use super::{PHYSICAL_TILE_DIMENSION, ShResidencyDrainError, SlotRun};

/// One copy region: `tiles` consecutive live slots starting at `live_slot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IsolatedUploadSpan {
    pub(super) live_slot: u32,
    pub(super) tiles: u32,
    /// Index of this span's first source slot in [`IsolatedUploadPlan::locals`].
    first: usize,
}

/// Copy regions for a node closure, in ascending live-slot order.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct IsolatedUploadPlan {
    pub(super) spans: Vec<IsolatedUploadSpan>,
    /// Closure-local source slot of every planned tile, in live-slot order.
    locals: Vec<u32>,
}

pub(super) fn plan_isolated_uploads(
    runs: &[SlotRun],
    live_tiles_per_row: u32,
) -> IsolatedUploadPlan {
    let mut ordered = runs.to_vec();
    ordered.sort_unstable_by_key(|run| run.live);
    let mut spans: Vec<IsolatedUploadSpan> = Vec::new();
    let mut locals = Vec::with_capacity(ordered.iter().map(|run| run.len as usize).sum());
    for run in ordered {
        for offset in 0..run.len {
            let live = run.live + offset;
            match spans.last_mut() {
                Some(span)
                    if span.live_slot + span.tiles == live
                        && span.live_slot / live_tiles_per_row == live / live_tiles_per_row =>
                {
                    span.tiles += 1;
                }
                _ => spans.push(IsolatedUploadSpan {
                    live_slot: live,
                    tiles: 1,
                    first: locals.len(),
                }),
            }
            locals.push(run.local + offset);
        }
    }
    IsolatedUploadPlan { spans, locals }
}

/// Byte geometry of one isolated atlas block's payload.
struct SourceGeometry {
    tiles_per_row: u32,
    tiles_per_layer: u32,
    /// Bytes in one block row of the whole source atlas.
    row_bytes: usize,
    /// Block rows in one source layer.
    layer_rows: usize,
    /// Block rows and bytes per block row of one tile.
    tile_rows: u32,
    tile_row_bytes: usize,
}

impl SourceGeometry {
    fn new(
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Result<Self, ShResidencyDrainError> {
        let tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let tiles_per_layer = tiles_per_row
            .checked_mul(height / PHYSICAL_TILE_DIMENSION)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let (row_bytes, layer_rows, tile_rows, tile_row_bytes) = match format {
            wgpu::TextureFormat::Bc6hRgbUfloat => (width / 4 * 16, height / 4, 2, 32),
            wgpu::TextureFormat::Rgba16Float => (width * 8, height, 8, 64),
            _ => unreachable!("streamed SH base formats are validated"),
        };
        Ok(Self {
            tiles_per_row,
            tiles_per_layer,
            row_bytes: row_bytes as usize,
            layer_rows: layer_rows as usize,
            tile_rows,
            tile_row_bytes,
        })
    }

    /// `(layer, tile row, tile column)` of a closure-local slot.
    fn position(&self, local: u32) -> (usize, u32, u32) {
        let in_layer = local % self.tiles_per_layer;
        (
            (local / self.tiles_per_layer) as usize,
            in_layer / self.tiles_per_row,
            in_layer % self.tiles_per_row,
        )
    }

    /// Byte offset of block row `row` of the tile at a local position.
    fn source_offset(
        &self,
        (layer, tile_row, column): (usize, u32, u32),
        row: u32,
    ) -> Option<usize> {
        let block_row = (tile_row * self.tile_rows + row) as usize;
        layer
            .checked_mul(self.layer_rows)?
            .checked_add(block_row)?
            .checked_mul(self.row_bytes)?
            .checked_add(column as usize * self.tile_row_bytes)
    }
}

/// Gather one span's tiles into `scratch` as tightly packed block rows and
/// return the packed row pitch. Source tiles adjacent in one local row are
/// copied together.
pub(super) fn pack_isolated_upload_span(
    payload: &[u8],
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    plan: &IsolatedUploadPlan,
    span: IsolatedUploadSpan,
    scratch: &mut Vec<u8>,
) -> Result<u32, ShResidencyDrainError> {
    let geometry = SourceGeometry::new(format, width, height)?;
    let tiles = span.tiles as usize;
    let packed_row_bytes = geometry
        .tile_row_bytes
        .checked_mul(tiles)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    scratch.clear();
    scratch.resize(
        packed_row_bytes
            .checked_mul(geometry.tile_rows as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
        0,
    );
    let locals = &plan.locals[span.first..span.first + tiles];
    let mut start = 0;
    while start < tiles {
        let position = geometry.position(locals[start]);
        // Extend over tiles that follow on the same local row.
        let mut end = start + 1;
        while end < tiles
            && locals[end] == locals[end - 1] + 1
            && geometry.position(locals[end]).1 == position.1
            && geometry.position(locals[end]).0 == position.0
        {
            end += 1;
        }
        let bytes = (end - start) * geometry.tile_row_bytes;
        for row in 0..geometry.tile_rows {
            let source = geometry
                .source_offset(position, row)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let target = row as usize * packed_row_bytes + start * geometry.tile_row_bytes;
            scratch[target..target + bytes].copy_from_slice(
                payload.get(source..source + bytes).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "isolated atlas tile span is truncated",
                    },
                )?,
            );
        }
        start = end;
    }
    u32::try_from(packed_row_bytes).map_err(|_| ShResidencyDrainError::SlotOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(local: u32, live: u32, len: u32) -> SlotRun {
        SlotRun { local, live, len }
    }

    #[test]
    fn spans_follow_live_rows_and_ignore_the_local_layout() {
        // Regression: splitting at every local-row edge made thousands of
        // copies for one real cluster.
        let plan = plan_isolated_uploads(&[run(5, 2, 3), run(0, 0, 2)], 4);
        assert_eq!(
            plan.spans
                .iter()
                .map(|span| (span.live_slot, span.tiles))
                .collect::<Vec<_>>(),
            [(0, 4), (4, 1)]
        );
        assert_eq!(plan.locals, [0, 1, 5, 6, 7]);
    }

    #[test]
    fn a_live_allocation_gap_starts_a_new_span() {
        let plan = plan_isolated_uploads(&[run(0, 0, 2), run(2, 3, 2)], 64);
        assert_eq!(
            plan.spans
                .iter()
                .map(|span| (span.live_slot, span.tiles))
                .collect::<Vec<_>>(),
            [(0, 2), (3, 2)]
        );
    }

    #[test]
    fn packed_span_gathers_bc6h_tiles_from_separate_local_rows() {
        // A 3x2-tile BC6H atlas: local 1 is row 0 column 1; local 3 is row 1
        // column 0. Each tile is two block rows of 32 bytes.
        let payload: Vec<u8> = (0..384).map(|index| index as u8).collect();
        let plan = plan_isolated_uploads(&[run(1, 7, 1), run(3, 8, 1)], 64);
        let mut scratch = Vec::new();
        let pitch = pack_isolated_upload_span(
            &payload,
            wgpu::TextureFormat::Bc6hRgbUfloat,
            24,
            16,
            &plan,
            plan.spans[0],
            &mut scratch,
        )
        .unwrap();
        assert_eq!(pitch, 64);
        let expected: Vec<u8> = [32..64, 192..224, 128..160, 288..320]
            .into_iter()
            .flat_map(|range| payload[range].to_vec())
            .collect();
        assert_eq!(scratch, expected);
    }

    #[test]
    fn packed_span_copies_adjacent_rgba16f_tiles_together() {
        // A 3x2-tile RGBA16F atlas: locals 1 and 2 share row 0; local 3
        // starts row 1. Each tile is eight block rows of 64 bytes.
        let payload: Vec<u8> = (0..3072).map(|index| (index % 251) as u8).collect();
        let plan = plan_isolated_uploads(&[run(1, 0, 3)], 64);
        let mut scratch = Vec::new();
        let pitch = pack_isolated_upload_span(
            &payload,
            wgpu::TextureFormat::Rgba16Float,
            24,
            16,
            &plan,
            plan.spans[0],
            &mut scratch,
        )
        .unwrap();
        assert_eq!(pitch, 192);
        for row in 0..8 {
            let packed = &scratch[row * 192..(row + 1) * 192];
            assert_eq!(&packed[..128], &payload[row * 192 + 64..row * 192 + 192]);
            assert_eq!(
                &packed[128..],
                &payload[(8 + row) * 192..(8 + row) * 192 + 64]
            );
        }
    }

    #[test]
    fn a_truncated_payload_is_rejected() {
        let plan = plan_isolated_uploads(&[run(3, 0, 1)], 64);
        let mut scratch = Vec::new();
        assert!(
            pack_isolated_upload_span(
                &[0; 100],
                wgpu::TextureFormat::Bc6hRgbUfloat,
                24,
                16,
                &plan,
                plan.spans[0],
                &mut scratch,
            )
            .is_err()
        );
    }
}
