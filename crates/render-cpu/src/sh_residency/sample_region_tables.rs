// Load-time affinity-row tables for the sampled-row gate: each row's
// scaled-node writer and the rows each runtime cell's dilated bounds cover.
// See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"
//
// Both never change after load, so prep builds them once, before residency
// filtering; the renderer's per-frame resolver reads them.

use super::ShResidencyDrainError;
use super::sample_regions::{
    SampleRegionGrid, ShSampleRegion, affinity_row, brick_rows_in, region_brick_bounds,
    scaled_writer_row,
};

const NO_WRITER: u32 = u32::MAX;
/// `scaled_writer_row` rejected the row's layout as having several writers.
const MULTIPLE_WRITERS: u32 = u32::MAX - 1;
/// `scaled_writer_row` rejected the row's address arithmetic.
const WRITER_OVERFLOW: u32 = u32::MAX - 2;

#[derive(Debug, Clone, PartialEq)]
pub struct SampleRegionTables {
    pub origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub dimensions: [u32; 3],
    /// Per affinity row: its canonical scaled-node writer, `NO_WRITER`, or
    /// the error `scaled_writer_row` returns for it.
    writer_rows: Vec<u32>,
    /// CSR over runtime cells: the rows each cell's dilated bounds cover,
    /// regardless of residency.
    cell_row_offsets: Vec<u32>,
    cell_rows: Vec<u32>,
}

impl SampleRegionTables {
    /// Cannot fail for a valid dense layout: bricks clamp to the grid, and
    /// row counts derive from the same layout as the writer lookups below.
    /// An error here fails the residency plan, before any GPU work.
    pub fn build(
        grid: &SampleRegionGrid<'_>,
        cell_bounds: impl ExactSizeIterator<Item = ShSampleRegion>,
    ) -> Result<Self, ShResidencyDrainError> {
        let row_count = grid
            .dimensions
            .iter()
            .try_fold(1u32, |rows, dimension| {
                rows.checked_mul(dimension.div_ceil(4))
            })
            // An unaddressable row space makes every writer lookup fail, as
            // `scaled_writer_row` does; the empty cache reproduces that.
            .unwrap_or(0);
        if row_count > WRITER_OVERFLOW {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        let writer_rows = (0..row_count)
            .map(|row| match scaled_writer_row(grid, row) {
                Ok(Some(writer)) => writer,
                Ok(None) => NO_WRITER,
                Err(ShResidencyDrainError::SlotOverflow) => WRITER_OVERFLOW,
                Err(_) => MULTIPLE_WRITERS,
            })
            .collect();

        let cell_count = cell_bounds.len();
        let mut cell_row_offsets = Vec::with_capacity(cell_count + 1);
        let mut cell_rows = Vec::new();
        cell_row_offsets.push(0);
        for bounds in cell_bounds {
            if let Some(bricks) =
                region_brick_bounds(grid.origin, grid.cell_size, grid.dimensions, bounds)
            {
                for brick in brick_rows_in(bricks) {
                    cell_rows.push(affinity_row(brick, grid.dimensions)?);
                }
            }
            cell_row_offsets.push(
                u32::try_from(cell_rows.len()).map_err(|_| ShResidencyDrainError::SlotOverflow)?,
            );
        }

        Ok(Self {
            origin: grid.origin,
            cell_size: grid.cell_size,
            dimensions: grid.dimensions,
            writer_rows,
            cell_row_offsets,
            cell_rows,
        })
    }

    /// Affinity rows in the grid's row space (zero when it is unaddressable).
    pub fn row_count(&self) -> usize {
        self.writer_rows.len()
    }

    /// Runtime cells the tables index.
    pub fn cell_count(&self) -> usize {
        self.cell_row_offsets.len() - 1
    }

    /// The rows `cell`'s dilated bounds cover. `cell` must be below
    /// [`Self::cell_count`].
    pub fn cell_rows(&self, cell: usize) -> &[u32] {
        let start = self.cell_row_offsets[cell] as usize;
        let end = self.cell_row_offsets[cell + 1] as usize;
        &self.cell_rows[start..end]
    }

    /// The canonical scaled-node writer of `row`, exactly as
    /// `scaled_writer_row` would compute it from the static dense layout.
    pub fn writer_row(&self, row: u32) -> Result<Option<u32>, ShResidencyDrainError> {
        match self.writer_rows.get(row as usize).copied() {
            None | Some(WRITER_OVERFLOW) => Err(ShResidencyDrainError::SlotOverflow),
            Some(MULTIPLE_WRITERS) => Err(ShResidencyDrainError::GpuCapacity {
                reason: "one affinity row resolves to multiple scaled-node writers",
            }),
            Some(NO_WRITER) => Ok(None),
            Some(writer) => Ok(Some(writer)),
        }
    }
}
