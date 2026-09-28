//! Load-time affinity-row indexes for the sampled-row gate, and the per-frame
//! resolver that reads them.
//! See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"
//!
//! Two things the gate needs never change after load: each row's scaled-node
//! writer (a function of the static dense layout) and the rows a runtime
//! cell's dilated bounds cover. Both are cached here, before residency
//! filtering, so a frame pays only for the rows it lists plus the bricks of
//! its dynamic (mover and mesh) regions. Dropped with `ShResidencyState`.

use postretro_visibility::VisibleCells;

use super::row_bitset::RowBitSet;
use super::sample_regions::{
    SampleRegionGrid, affinity_row, brick_rows_in, region_brick_bounds, scaled_writer_row,
};
use super::*;
use crate::render::ShSampleRegion;

const NO_WRITER: u32 = u32::MAX;
/// `scaled_writer_row` rejected the row's layout as having several writers.
const MULTIPLE_WRITERS: u32 = u32::MAX - 1;
/// `scaled_writer_row` rejected the row's address arithmetic.
const WRITER_OVERFLOW: u32 = u32::MAX - 2;

/// One frame's view-dependent gate inputs.
#[derive(Clone, Copy)]
pub(super) struct GateInputs<'a> {
    pub(super) visible_cells: &'a VisibleCells,
    pub(super) fog_cells: &'a [u32],
    /// Mover and forward-mesh bounds; these move, so they walk bricks.
    pub(super) dynamic_regions: &'a [ShSampleRegion],
    /// The empty-fog-reach sentinel: every resident row is gated.
    pub(super) fog_draw_all: bool,
}

pub(super) struct SampleRegionIndex {
    origin: [f32; 3],
    cell_size: [f32; 3],
    dimensions: [u32; 3],
    /// Per affinity row: its canonical scaled-node writer, `NO_WRITER`, or
    /// the error `scaled_writer_row` returns for it.
    writer_rows: Vec<u32>,
    /// CSR over runtime cells: the rows each cell's dilated bounds cover,
    /// regardless of residency.
    cell_row_offsets: Vec<u32>,
    cell_rows: Vec<u32>,
    /// Dedup scratch: an entry equals `stamp` once seen this frame.
    row_stamps: Vec<u32>,
    cell_stamps: Vec<u32>,
    stamp: u32,
}

impl SampleRegionIndex {
    /// Cannot fail for a valid dense layout: bricks clamp to the grid, and
    /// row counts derive from the same layout as the writer lookups below.
    /// An error here now surfaces at level install (`install_level_geometry`
    /// panics) rather than on the first frame that touches the row.
    pub(super) fn build(
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
            row_stamps: vec![0; row_count as usize],
            cell_stamps: vec![0; cell_count],
            stamp: 0,
        })
    }

    /// The canonical scaled-node writer of `row`, exactly as
    /// `scaled_writer_row` would compute it from the static dense layout.
    pub(super) fn writer_row(&self, row: u32) -> Result<Option<u32>, ShResidencyDrainError> {
        match self.writer_rows.get(row as usize).copied() {
            None | Some(WRITER_OVERFLOW) => Err(ShResidencyDrainError::SlotOverflow),
            Some(MULTIPLE_WRITERS) => Err(ShResidencyDrainError::GpuCapacity {
                reason: "one affinity row resolves to multiple scaled-node writers",
            }),
            Some(NO_WRITER) => Ok(None),
            Some(writer) => Ok(Some(writer)),
        }
    }

    /// Gate rows for one frame: resident rows under any input region, closed
    /// once over resident scaled-node writers. Each row appears once; the
    /// order is unspecified (the planner orders its own output).
    pub(super) fn resolve(
        &mut self,
        inputs: GateInputs<'_>,
        resident: [&RowBitSet; 3],
        rows: &mut Vec<u32>,
    ) -> Result<(), ShResidencyDrainError> {
        rows.clear();
        let is_resident = |row: u32| resident.iter().any(|set| set.contains(&row));
        if inputs.fog_draw_all {
            rows.extend(RowBitSet::iter_union(resident));
            // Every writer this could add is resident, so already listed; the
            // lookups remain only to surface a malformed layout as before.
            for &row in rows.iter() {
                self.writer_row(row)?;
            }
            return Ok(());
        }

        let stamp = self.next_stamp();
        let visible = match inputs.visible_cells {
            VisibleCells::DrawAll => CellIds::All(0..self.cell_stamps.len() as u32),
            VisibleCells::Culled(cells) => CellIds::Listed(cells.iter()),
        };
        // Fog reach normally contains the visible set; stamping cells makes a
        // shared cell cost once.
        for cell in visible.chain(inputs.fog_cells.iter().copied()) {
            let Some(cell_stamp) = self.cell_stamps.get_mut(cell as usize) else {
                // Out-of-range ids have no bounds to sample, as at the
                // application boundary.
                continue;
            };
            if *cell_stamp == stamp {
                continue;
            }
            *cell_stamp = stamp;
            let start = self.cell_row_offsets[cell as usize] as usize;
            let end = self.cell_row_offsets[cell as usize + 1] as usize;
            for &row in &self.cell_rows[start..end] {
                if is_resident(row) && first_sight(&mut self.row_stamps, stamp, row)? {
                    rows.push(row);
                }
            }
        }

        for &region in inputs.dynamic_regions {
            let Some(bricks) =
                region_brick_bounds(self.origin, self.cell_size, self.dimensions, region)
            else {
                continue;
            };
            for brick in brick_rows_in(bricks) {
                let row = affinity_row(brick, self.dimensions)?;
                if is_resident(row) && first_sight(&mut self.row_stamps, stamp, row)? {
                    rows.push(row);
                }
            }
        }

        // Do not recursively close writer rows: each sampled row adds at most one.
        let sampled_len = rows.len();
        for index in 0..sampled_len {
            if let Some(writer) = self.writer_row(rows[index])?
                && is_resident(writer)
                && first_sight(&mut self.row_stamps, stamp, writer)?
            {
                rows.push(writer);
            }
        }
        Ok(())
    }

    fn next_stamp(&mut self) -> u32 {
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.row_stamps.fill(0);
            self.cell_stamps.fill(0);
            self.stamp = 1;
        }
        self.stamp
    }

    #[cfg(test)]
    fn set_stamp_for_test(&mut self, stamp: u32) {
        self.stamp = stamp;
    }
}

/// Whether `row` is first seen this frame; marks it seen.
fn first_sight(stamps: &mut [u32], stamp: u32, row: u32) -> Result<bool, ShResidencyDrainError> {
    let seen = stamps
        .get_mut(row as usize)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let first = *seen != stamp;
    *seen = stamp;
    Ok(first)
}

enum CellIds<'a> {
    All(std::ops::Range<u32>),
    Listed(std::slice::Iter<'a, u32>),
}

impl Iterator for CellIds<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        match self {
            Self::All(cells) => cells.next(),
            Self::Listed(cells) => cells.next().copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::super::sample_regions::resolve_regions;
    use super::*;

    const DIMS: [u32; 3] = [13, 6, 9];

    fn grid(nodes: &[Option<StoredNode>]) -> SampleRegionGrid<'_> {
        SampleRegionGrid {
            origin: [-2.0, 1.0, 0.5],
            cell_size: [1.5, 1.0, 2.0],
            dimensions: DIMS,
            dense_nodes: nodes,
        }
    }

    fn row_count() -> u32 {
        DIMS.iter().map(|dimension| dimension.div_ceil(4)).product()
    }

    fn probe_count() -> usize {
        DIMS.iter().product::<u32>() as usize
    }

    fn region(min: [f32; 3], max: [f32; 3]) -> ShSampleRegion {
        ShSampleRegion::new(glam::Vec3::from_array(min), glam::Vec3::from_array(max))
    }

    /// Mark every probe of brick `sampled` as a scaled node written by the
    /// brick `writer`.
    fn scale_brick(nodes: &mut [Option<StoredNode>], sampled: [u32; 3], writer: [u32; 3]) {
        let node = StoredNode {
            brick_origin: writer,
            scale: 1,
            level: 1,
        };
        for z in sampled[2] * 4..(sampled[2] * 4 + 4).min(DIMS[2]) {
            for y in sampled[1] * 4..(sampled[1] * 4 + 4).min(DIMS[1]) {
                for x in sampled[0] * 4..(sampled[0] * 4 + 4).min(DIMS[0]) {
                    nodes[(x + y * DIMS[0] + z * DIMS[0] * DIMS[1]) as usize] = Some(node);
                }
            }
        }
    }

    fn scaled_layout() -> Vec<Option<StoredNode>> {
        let mut nodes = vec![None; probe_count()];
        scale_brick(&mut nodes, [1, 0, 0], [0, 0, 0]);
        scale_brick(&mut nodes, [3, 1, 2], [2, 1, 2]);
        scale_brick(&mut nodes, [0, 1, 1], [3, 0, 0]);
        nodes
    }

    fn cells() -> Vec<ShSampleRegion> {
        vec![
            region([-2.0, 1.0, 0.5], [0.0, 2.0, 2.0]),
            region([3.0, 2.0, 4.0], [9.0, 4.0, 9.0]),
            // Straddles the low grid bounds.
            region([-40.0, -9.0, -30.0], [-1.0, 1.5, 1.0]),
            // Straddles the high grid bounds.
            region([12.0, 5.0, 14.0], [99.0, 70.0, 88.0]),
            region([1.0, 3.0, 5.0], [2.0, 3.5, 6.0]),
            // Entirely outside the grid.
            region([200.0, 200.0, 200.0], [201.0, 201.0, 201.0]),
            // Inverted bounds are normalized, as for any region.
            region([6.0, 4.0, 10.0], [4.0, 2.0, 8.0]),
            region([f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0]),
        ]
    }

    struct Frame {
        visible: VisibleCells,
        fog: Vec<u32>,
        dynamic: Vec<ShSampleRegion>,
        fog_draw_all: bool,
    }

    impl Frame {
        fn cells(visible: &[u32], fog: &[u32]) -> Self {
            Self {
                visible: VisibleCells::Culled(visible.to_vec()),
                fog: fog.to_vec(),
                dynamic: Vec::new(),
                fog_draw_all: fog.is_empty(),
            }
        }

        fn inputs(&self) -> GateInputs<'_> {
            GateInputs {
                visible_cells: &self.visible,
                fog_cells: &self.fog,
                dynamic_regions: &self.dynamic,
                fog_draw_all: self.fog_draw_all,
            }
        }

        /// The region list the application built before the cell index:
        /// visible bounds, fog bounds, then dynamic bounds.
        fn oracle_regions(&self, cells: &[ShSampleRegion]) -> Vec<ShSampleRegion> {
            let visible: Vec<ShSampleRegion> = match &self.visible {
                VisibleCells::DrawAll => cells.to_vec(),
                VisibleCells::Culled(ids) => ids
                    .iter()
                    .filter_map(|&cell| cells.get(cell as usize).copied())
                    .collect(),
            };
            let fog = self
                .fog
                .iter()
                .filter_map(|&cell| cells.get(cell as usize).copied());
            visible
                .into_iter()
                .chain(fog)
                .chain(self.dynamic.iter().copied())
                .collect()
        }
    }

    fn resident_sets(rows: [&[u32]; 3]) -> [BTreeSet<u32>; 3] {
        rows.map(|rows| rows.iter().copied().collect())
    }

    /// Resolve `frame` through both paths and require identical results.
    fn assert_equivalent(
        index: &mut SampleRegionIndex,
        nodes: &[Option<StoredNode>],
        cells: &[ShSampleRegion],
        frame: &Frame,
        resident: &[BTreeSet<u32>; 3],
    ) -> Vec<u32> {
        let mut expected = Vec::new();
        let expected_result = resolve_regions(
            grid(nodes),
            &frame.oracle_regions(cells),
            frame.fog_draw_all,
            &[&resident[0], &resident[1], &resident[2]],
            &mut expected,
        );
        let bits = resident
            .clone()
            .map(|rows| rows.into_iter().collect::<RowBitSet>());
        let mut actual = Vec::new();
        let actual_result =
            index.resolve(frame.inputs(), [&bits[0], &bits[1], &bits[2]], &mut actual);
        assert_eq!(actual_result, expected_result);
        let unsorted_len = actual.len();
        actual.sort_unstable();
        actual.dedup();
        assert_eq!(actual.len(), unsorted_len, "resolved rows repeat");
        if expected_result.is_ok() {
            assert_eq!(actual, expected);
        }
        actual
    }

    fn fixture() -> (
        Vec<Option<StoredNode>>,
        Vec<ShSampleRegion>,
        SampleRegionIndex,
    ) {
        let nodes = scaled_layout();
        let cells = cells();
        let index = SampleRegionIndex::build(&grid(&nodes), cells.iter().copied()).unwrap();
        (nodes, cells, index)
    }

    fn every_row() -> Vec<u32> {
        (0..row_count()).collect()
    }

    #[test]
    fn cached_writer_rows_match_scaled_writer_scan() {
        let mut nodes = scaled_layout();
        // Two writers inside one brick is a malformed layout; the cache must
        // report the same error the scan does.
        nodes[(8 + 4 * DIMS[0] + 4 * DIMS[0] * DIMS[1]) as usize] = Some(StoredNode {
            brick_origin: [0, 0, 0],
            scale: 1,
            level: 1,
        });
        nodes[(9 + 4 * DIMS[0] + 4 * DIMS[0] * DIMS[1]) as usize] = Some(StoredNode {
            brick_origin: [1, 0, 0],
            scale: 1,
            level: 1,
        });
        let index = SampleRegionIndex::build(&grid(&nodes), std::iter::empty()).unwrap();
        let mut saw_error = false;
        for row in 0..row_count() + 3 {
            let expected = scaled_writer_row(&grid(&nodes), row);
            saw_error |= expected.is_err();
            assert_eq!(index.writer_row(row), expected, "row {row}");
        }
        assert!(saw_error);
    }

    #[test]
    fn cell_only_regions_match_oracle() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&every_row(), &[], &[]]);
        for visible in [&[0][..], &[1], &[1, 4], &[0, 1, 2, 3, 4, 5, 6, 7]] {
            let rows = assert_equivalent(
                &mut index,
                &nodes,
                &cells,
                &Frame::cells(visible, visible),
                &resident,
            );
            assert!(!rows.is_empty());
        }
    }

    #[test]
    fn overlapping_visible_and_fog_cells_match_oracle() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&every_row(), &[2, 5, 9], &[30]]);
        let frames = [
            Frame::cells(&[1, 4], &[4, 1, 0, 3]),
            Frame::cells(&[], &[4, 4, 1]),
            Frame::cells(&[2, 2, 2], &[2]),
            // Out-of-range ids have no bounds.
            Frame::cells(&[1, 99], &[1, 1000]),
        ];
        for frame in &frames {
            assert_equivalent(&mut index, &nodes, &cells, frame, &resident);
        }
    }

    #[test]
    fn mover_and_mesh_regions_match_oracle() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&every_row(), &[], &[]]);
        let mut frame = Frame::cells(&[0], &[0]);
        frame.dynamic = vec![
            region([4.0, 2.0, 6.0], [4.0, 2.0, 6.0]),
            region([-100.0, -100.0, -100.0], [100.0, 100.0, 100.0]),
            region([f32::INFINITY, 0.0, 0.0], [0.0, 0.0, 0.0]),
            region([10.0, 5.0, 12.0], [7.0, 3.0, 9.0]),
        ];
        assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        frame.visible = VisibleCells::Culled(Vec::new());
        frame.fog = vec![5];
        assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
    }

    #[test]
    fn draw_all_and_visible_draw_all_match_oracle() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&[0, 7], &[3, 7, 11], &[1, row_count() - 1]]);
        let mut frame = Frame::cells(&[0], &[]);
        frame.dynamic = vec![region([0.0; 3], [1.0; 3])];
        assert!(frame.fog_draw_all);
        assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);

        frame.visible = VisibleCells::DrawAll;
        frame.fog = vec![3];
        frame.fog_draw_all = false;
        assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
    }

    #[test]
    fn empty_input_resolves_no_rows() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&every_row(), &every_row(), &[]]);
        let mut frame = Frame::cells(&[], &[]);
        frame.fog_draw_all = false;
        let rows = assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        assert!(rows.is_empty());

        let empty = resident_sets([&[], &[], &[]]);
        let frame = Frame::cells(&[0, 1, 2, 3], &[0, 1, 2, 3]);
        let rows = assert_equivalent(&mut index, &nodes, &cells, &frame, &empty);
        assert!(rows.is_empty());
    }

    #[test]
    fn partially_resident_rows_and_foreign_writers_match_oracle() {
        let (nodes, cells, mut index) = fixture();
        let every = every_row();
        // Odd rows resident only, spread across the three passes; the
        // scaled bricks' writers are resident in some sets and not others.
        let odd: Vec<u32> = every.iter().copied().filter(|row| row % 2 == 1).collect();
        let resident = resident_sets([&odd[..odd.len() / 2], &odd[odd.len() / 2..], &[0]]);
        for visible in [&[0][..], &[1], &[0, 1, 2, 3, 4, 6]] {
            let mut frame = Frame::cells(visible, visible);
            frame.dynamic = vec![region([9.0, 3.0, 8.0], [10.0, 4.0, 9.0])];
            assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        }
    }

    #[test]
    fn a_row_evicted_then_reinstalled_between_frames_resolves_like_oracle() {
        let (nodes, cells, mut index) = fixture();
        let frame = Frame::cells(&[0, 1], &[0, 1, 4]);
        let mut resident = resident_sets([&every_row(), &[], &[]]);
        let before = assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        let row = before[before.len() / 2];

        resident[0].remove(&row);
        let evicted = assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        assert!(!evicted.contains(&row));

        resident[1].insert(row);
        let reinstalled = assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
        assert_eq!(reinstalled, before);
    }

    #[test]
    fn dedup_stamps_survive_wraparound() {
        let (nodes, cells, mut index) = fixture();
        let resident = resident_sets([&every_row(), &[], &[]]);

        // Frame A's dynamic region sits at the grid's low corner, dilating
        // to brick_x == 0 only. Frame B's sits at the high corner, whose
        // dilated bricks all have brick_x >= 2. The frames must be disjoint:
        // if B touched any row A had touched, B's resolve at stamp MAX would
        // re-stamp it, erasing the stale mark this test needs to survive
        // long enough for the wraparound reset to matter.
        let mut frame_a = Frame::cells(&[], &[]);
        frame_a.fog_draw_all = false;
        frame_a.dynamic = vec![region([-2.0, 1.0, 0.5], [-2.0, 1.0, 0.5])];
        let mut frame_b = Frame::cells(&[], &[]);
        frame_b.fog_draw_all = false;
        frame_b.dynamic = vec![region([16.0, 6.0, 16.5], [16.0, 6.0, 16.5])];

        let expected = assert_equivalent(&mut index, &nodes, &cells, &frame_a, &resident);
        assert!(!expected.is_empty());

        // Land on the stamp before wraparound: frame B's resolve consumes
        // `u32::MAX`, and frame A's next resolve is the one that wraps to 0.
        index.set_stamp_for_test(u32::MAX - 1);
        let b_rows = assert_equivalent(&mut index, &nodes, &cells, &frame_b, &resident);
        assert!(!b_rows.is_empty());
        assert!(b_rows.iter().all(|row| !expected.contains(row)));

        // Without the wraparound reset, A's rows still hold stamp 1 from the
        // first resolve above; this call's stamp is corrected back to 1 too,
        // so a stale mark would collide and A's rows would be wrongly
        // deduped away instead of re-added.
        assert_eq!(
            assert_equivalent(&mut index, &nodes, &cells, &frame_a, &resident),
            expected
        );
    }

    #[test]
    fn resolve_reuses_warmed_scratch() {
        let (_, _, mut index) = fixture();
        let bits: RowBitSet = every_row().into_iter().collect();
        let empty = RowBitSet::default();
        let frame = Frame::cells(&[0, 1, 2, 3, 4], &[0, 1, 2, 3, 4, 5, 6]);
        let mut rows = Vec::new();
        index
            .resolve(frame.inputs(), [&bits, &empty, &empty], &mut rows)
            .unwrap();
        let warmed = rows.capacity();
        for _ in 0..32 {
            index
                .resolve(frame.inputs(), [&bits, &empty, &empty], &mut rows)
                .unwrap();
            assert_eq!(rows.capacity(), warmed);
        }
    }

    fn arb_region() -> impl Strategy<Value = ShSampleRegion> {
        let coordinate = -8.0f32..24.0;
        (
            [coordinate.clone(), coordinate.clone(), coordinate.clone()],
            [coordinate.clone(), coordinate.clone(), coordinate],
        )
            .prop_map(|(min, max)| region(min, max))
    }

    proptest! {
        #[test]
        fn random_layouts_and_frames_match_oracle(
            scaled in prop::collection::vec(
                ((0u32..4, 0u32..2, 0u32..3), (0u32..4, 0u32..2, 0u32..3)),
                0..6,
            ),
            cells in prop::collection::vec(arb_region(), 0..12),
            frames in prop::collection::vec(
                (
                    prop::option::of(prop::collection::vec(0u32..14, 0..10)),
                    prop::collection::vec(0u32..14, 0..12),
                    prop::collection::vec(arb_region(), 0..3),
                    any::<bool>(),
                    [
                        prop::collection::btree_set(0u32..24, 0..24),
                        prop::collection::btree_set(0u32..24, 0..8),
                        prop::collection::btree_set(0u32..24, 0..8),
                    ],
                ),
                1..5,
            ),
        ) {
            let mut nodes = vec![None; probe_count()];
            for ((sx, sy, sz), (wx, wy, wz)) in scaled {
                scale_brick(&mut nodes, [sx, sy, sz], [wx, wy, wz]);
            }
            let mut index =
                SampleRegionIndex::build(&grid(&nodes), cells.iter().copied()).unwrap();
            // One index across frames: residency and view change between them.
            for (visible, fog, dynamic, draw_all, resident) in frames {
                let frame = Frame {
                    visible: visible.map_or(VisibleCells::DrawAll, VisibleCells::Culled),
                    fog_draw_all: draw_all || fog.is_empty(),
                    fog,
                    dynamic,
                };
                assert_equivalent(&mut index, &nodes, &cells, &frame, &resident);
            }
        }
    }
}
