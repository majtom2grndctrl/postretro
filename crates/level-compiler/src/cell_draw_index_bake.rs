// CellDrawIndex (id 37) bake: derive each cell's owned BVH-leaf spans as CSR.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::bsp::BspLeafRecord;
use postretro_level_format::bvh::BvhLeaf;
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};

/// Bake the per-cell draw index from the already-sorted flat BVH leaf array and
/// the encoded BSP leaf records.
///
/// `bvh_leaves` is the flattened, **sorted** leaf array from `bvh_build::build_bvh`
/// (stable-sorted by `(material_bucket_id, cell_id, index_offset)`), so each cell's
/// leaves within one material bucket form one contiguous run. `bsp_leaves` is the
/// per-BSP-leaf record array (`vis_result.leaves_section.leaves`), indexed directly
/// by `BvhLeaf.cell_id` because `cell_id == BSP leaf index`.
///
/// A span is a maximal contiguous run of leaf slots owned by a cell **within a
/// single material bucket** — the `CellDrawIndex` wire invariant (each span lies in
/// one bucket). A run is broken both where the global slot order is non-contiguous
/// and where the material bucket changes, so a cell that touches K buckets owns at
/// least K disjoint spans. The bucket boundary never affects the runtime gather
/// (it slices contiguous global slots), but keeping spans bucket-coherent matches
/// the loader's validation contract.
///
/// Returns `None` when `bvh_leaves` is empty (zero-leaf maps omit the section).
///
/// A leaf contributes to the index only when it is **drawable**: `index_count > 0`
/// and its cell satisfies `is_solid == 0 && face_count > 0`. Non-drawable cells get
/// an empty CSR row.
///
/// The CSR serializes cells in ascending `cell_id` (one row per BSP leaf), and
/// within each cell emits maximal contiguous spans in ascending global `leaf_start`
/// order. Runs through `cell_span_offset` as a prefix sum over `cell_count`.
pub fn bake_cell_draw_index(
    bvh_leaves: &[BvhLeaf],
    bsp_leaves: &[BspLeafRecord],
) -> Option<CellDrawIndexSection> {
    if bvh_leaves.is_empty() {
        return None;
    }

    let cell_count = bsp_leaves.len();

    // For each cell (BSP leaf index), the maximal contiguous runs of leaf slots it
    // owns. The sort key clusters a cell's leaves per bucket into contiguous runs;
    // a new span starts whenever the previous slot was not this cell's predecessor.
    let mut spans_by_cell: Vec<Vec<Span>> = vec![Vec::new(); cell_count];

    for (slot, leaf) in bvh_leaves.iter().enumerate() {
        let slot = slot as u32;
        let cell_id = leaf.cell_id as usize;

        // `cell_id == BSP leaf index`; a leaf out of range would be a compiler
        // invariant break upstream (BVH cell_ids are BSP leaf indices).
        debug_assert!(
            cell_id < cell_count,
            "BVH leaf cell_id {cell_id} out of BSP leaf range {cell_count}",
        );

        if !is_drawable(leaf, bsp_leaves) {
            continue;
        }

        // Extend the current run only when this slot abuts the previous one *and*
        // shares its material bucket. The contiguity test alone would coalesce a
        // cell's adjacent-bucket leaves where they happen to abut globally; the
        // bucket test keeps every span inside one bucket (the wire invariant the
        // loader validates). The previous slot is `slot - 1` here, since the run
        // abuts only when `last.leaf_start + last.leaf_count == slot`.
        let runs = &mut spans_by_cell[cell_id];
        let extend = match runs.last() {
            Some(last) if last.leaf_start + last.leaf_count == slot => {
                bvh_leaves[(slot - 1) as usize].material_bucket_id == leaf.material_bucket_id
            }
            _ => false,
        };
        if extend {
            runs.last_mut()
                .expect("extend implies a last run")
                .leaf_count += 1;
        } else {
            runs.push(Span {
                leaf_start: slot,
                leaf_count: 1,
            });
        }
    }

    // Flatten per-cell runs into CSR: prefix-sum offsets + flat span payload.
    let mut cell_span_offset: Vec<u32> = Vec::with_capacity(cell_count + 1);
    let mut spans: Vec<Span> = Vec::new();
    cell_span_offset.push(0);
    for runs in &spans_by_cell {
        spans.extend_from_slice(runs);
        cell_span_offset.push(spans.len() as u32);
    }

    let section = CellDrawIndexSection {
        cell_count: cell_count as u32,
        span_count: spans.len() as u32,
        cell_span_offset,
        spans,
    };

    debug_assert_invariants(&section, bvh_leaves, bsp_leaves);

    Some(section)
}

/// A leaf draws when it carries geometry and its cell is an empty, face-bearing
/// BSP leaf. `cell_id == BSP leaf index` makes the join a direct index.
fn is_drawable(leaf: &BvhLeaf, bsp_leaves: &[BspLeafRecord]) -> bool {
    if leaf.index_count == 0 {
        return false;
    }
    match bsp_leaves.get(leaf.cell_id as usize) {
        Some(record) => record.is_solid == 0 && record.face_count > 0,
        None => false,
    }
}

/// Internal validation (debug builds only): every drawable leaf is covered exactly
/// once by a contiguous, maximal, ascending, in-bounds span; non-drawable cells are
/// empty rows; the CSR offsets are a correct prefix sum.
#[cfg(debug_assertions)]
fn debug_assert_invariants(
    section: &CellDrawIndexSection,
    bvh_leaves: &[BvhLeaf],
    bsp_leaves: &[BspLeafRecord],
) {
    let cell_count = section.cell_count as usize;
    debug_assert_eq!(
        section.cell_span_offset.len(),
        cell_count + 1,
        "offset table must be cell_count + 1",
    );
    debug_assert_eq!(section.cell_span_offset[0], 0, "offset[0] must be 0");
    debug_assert_eq!(
        section.cell_span_offset[cell_count], section.span_count,
        "final offset must equal span_count",
    );

    // Per-leaf coverage tally: drawable leaves must be covered exactly once,
    // non-drawable leaves never.
    let mut coverage = vec![0u32; bvh_leaves.len()];

    for cell in 0..cell_count {
        let start = section.cell_span_offset[cell] as usize;
        let end = section.cell_span_offset[cell + 1] as usize;
        debug_assert!(start <= end, "offset table not non-decreasing");

        let record = &bsp_leaves[cell];
        let cell_drawable = record.is_solid == 0 && record.face_count > 0;
        if !cell_drawable {
            debug_assert_eq!(
                start, end,
                "non-drawable cell {cell} must have an empty CSR row",
            );
        }

        let mut prev_end: Option<u32> = None;
        let mut prev_bucket: Option<u32> = None;
        for span in &section.spans[start..end] {
            debug_assert!(span.leaf_count >= 1, "spans must be non-empty");
            let span_end = span.leaf_start + span.leaf_count;
            debug_assert!(
                span_end as usize <= bvh_leaves.len(),
                "span out of bvh leaf bounds",
            );

            let span_bucket = bvh_leaves[span.leaf_start as usize].material_bucket_id;

            // Ascending and non-overlapping within the cell. Touching spans
            // (leaf_start == prev_end) are allowed, but only across a bucket
            // boundary — two abutting same-bucket spans would mean the coalescing
            // pass failed to merge a maximal run.
            if let Some(prev) = prev_end {
                debug_assert!(
                    span.leaf_start >= prev,
                    "spans within a cell must be ascending and non-overlapping",
                );
                if span.leaf_start == prev {
                    debug_assert_ne!(
                        prev_bucket,
                        Some(span_bucket),
                        "abutting same-bucket spans in cell {cell} must be one maximal span",
                    );
                }
            }
            prev_end = Some(span_end);
            prev_bucket = Some(span_bucket);

            for slot in span.leaf_start..span_end {
                let leaf = &bvh_leaves[slot as usize];
                debug_assert_eq!(
                    leaf.cell_id as usize, cell,
                    "span slot {slot} belongs to cell {} not {cell}",
                    leaf.cell_id,
                );
                debug_assert_eq!(
                    leaf.material_bucket_id, span_bucket,
                    "span slot {slot} crosses material bucket boundary",
                );
                debug_assert!(
                    is_drawable(leaf, bsp_leaves),
                    "span covers a non-drawable leaf at slot {slot}",
                );
                coverage[slot as usize] += 1;
            }
        }
    }

    for (slot, &count) in coverage.iter().enumerate() {
        let expected = if is_drawable(&bvh_leaves[slot], bsp_leaves) {
            1
        } else {
            0
        };
        debug_assert_eq!(
            count, expected,
            "leaf slot {slot} covered {count} times, expected {expected}",
        );
    }
}

#[cfg(not(debug_assertions))]
fn debug_assert_invariants(
    _section: &CellDrawIndexSection,
    _bvh_leaves: &[BvhLeaf],
    _bsp_leaves: &[BspLeafRecord],
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `BvhLeaf` carrying only the fields the bake reads; geometry bounds
    /// and chunk ranges are irrelevant to span derivation.
    fn leaf(material_bucket_id: u32, cell_id: u32, index_offset: u32, index_count: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id,
            aabb_max: [0.0; 3],
            index_offset,
            index_count,
            cell_id,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    /// An empty (drawable) BSP leaf record with the given face count.
    fn empty_leaf(face_count: u32) -> BspLeafRecord {
        BspLeafRecord {
            face_start: 0,
            face_count,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            is_solid: 0,
        }
    }

    /// A solid BSP leaf record (never drawable).
    fn solid_leaf() -> BspLeafRecord {
        BspLeafRecord {
            face_start: 0,
            face_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            is_solid: 1,
        }
    }

    /// Sort a leaf array by the exact BVH flatten key so tests feed the bake the
    /// same ordering the compiler produces.
    fn sort_like_bvh(mut leaves: Vec<BvhLeaf>) -> Vec<BvhLeaf> {
        leaves.sort_by_key(|l| (l.material_bucket_id, l.cell_id, l.index_offset));
        leaves
    }

    /// Spans owned by a cell, read back out of the CSR.
    fn cell_spans(section: &CellDrawIndexSection, cell: usize) -> &[Span] {
        let start = section.cell_span_offset[cell] as usize;
        let end = section.cell_span_offset[cell + 1] as usize;
        &section.spans[start..end]
    }

    #[test]
    fn bake_returns_none_for_zero_leaf_map() {
        let bsp = vec![empty_leaf(1)];
        assert!(bake_cell_draw_index(&[], &bsp).is_none());
    }

    #[test]
    fn single_cell_single_bucket_is_one_span() {
        // Cell 0, one bucket, three contiguous leaves -> a single span [0,3).
        let leaves = sort_like_bvh(vec![leaf(0, 0, 0, 3), leaf(0, 0, 3, 3), leaf(0, 0, 6, 3)]);
        let bsp = vec![empty_leaf(3)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        assert_eq!(section.cell_count, 1);
        assert_eq!(section.cell_span_offset, vec![0, 1]);
        assert_eq!(
            cell_spans(&section, 0),
            &[Span {
                leaf_start: 0,
                leaf_count: 3
            }]
        );
    }

    #[test]
    fn cell_touching_two_buckets_splits_into_two_spans() {
        // One cell whose leaves span two material buckets. After the BVH sort the
        // cell's slots are [bucket0 leaves..., bucket1 leaves...], which are NOT a
        // single contiguous run from the cell's perspective only if another cell
        // interleaves — but here a second cell (cell 1, bucket 0) sits between the
        // two buckets of cell 0 after sorting, forcing two disjoint runs.
        //
        // Sorted order by (bucket, cell, offset):
        //   bucket 0: (cell 0, off 0), (cell 1, off 10)
        //   bucket 1: (cell 0, off 20)
        // => slots: 0=cell0, 1=cell1, 2=cell0. Cell 0 owns slots {0, 2} as two runs.
        let leaves = sort_like_bvh(vec![leaf(0, 0, 0, 3), leaf(0, 1, 10, 3), leaf(1, 0, 20, 3)]);
        let bsp = vec![empty_leaf(2), empty_leaf(1)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        // Cell 0 owns two disjoint single-leaf runs at slots 0 and 2.
        assert_eq!(
            cell_spans(&section, 0),
            &[
                Span {
                    leaf_start: 0,
                    leaf_count: 1
                },
                Span {
                    leaf_start: 2,
                    leaf_count: 1
                },
            ],
            "per-bucket split must produce two spans for the interleaved cell",
        );
        // Cell 1 owns the single middle slot.
        assert_eq!(
            cell_spans(&section, 1),
            &[Span {
                leaf_start: 1,
                leaf_count: 1
            }]
        );
    }

    #[test]
    fn cell_spanning_two_buckets_splits_at_bucket_boundary() {
        // Cell 0 owns two leaves in bucket 0 and two in bucket 1, with no other cell
        // interleaving. Sorted slots: [0,1]=bucket0, [2,3]=bucket1, all cell 0 and
        // globally contiguous (0,1,2,3). Even though the slots abut globally, each
        // span must lie in a single material bucket (the wire invariant the loader
        // validates), so the run breaks at the bucket boundary into [0,2) and [2,4).
        let leaves = sort_like_bvh(vec![
            leaf(0, 0, 0, 3),
            leaf(0, 0, 3, 3),
            leaf(1, 0, 6, 3),
            leaf(1, 0, 9, 3),
        ]);
        let bsp = vec![empty_leaf(4)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        assert_eq!(
            cell_spans(&section, 0),
            &[
                Span {
                    leaf_start: 0,
                    leaf_count: 2
                },
                Span {
                    leaf_start: 2,
                    leaf_count: 2
                },
            ],
            "spans must break at the material bucket boundary even when slots abut",
        );
    }

    #[test]
    fn solid_cell_gets_empty_row() {
        // Cell 1 is solid: its leaf must not appear in any span, and its CSR row is
        // empty (offset[1] == offset[2]).
        let leaves = sort_like_bvh(vec![leaf(0, 0, 0, 3), leaf(0, 1, 3, 3)]);
        let bsp = vec![empty_leaf(1), solid_leaf()];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        assert_eq!(
            cell_spans(&section, 0),
            &[Span {
                leaf_start: 0,
                leaf_count: 1
            }]
        );
        assert!(
            cell_spans(&section, 1).is_empty(),
            "solid cell must be an empty row"
        );
        assert_eq!(section.cell_span_offset[1], section.cell_span_offset[2]);
    }

    #[test]
    fn zero_face_count_cell_gets_empty_row() {
        // Cell 1 is empty but carries zero faces (e.g. an exterior-culled leaf):
        // non-drawable, so its leaf is excluded and its row stays empty.
        let leaves = sort_like_bvh(vec![leaf(0, 0, 0, 3), leaf(0, 1, 3, 3)]);
        let bsp = vec![empty_leaf(1), empty_leaf(0)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        assert_eq!(
            cell_spans(&section, 0),
            &[Span {
                leaf_start: 0,
                leaf_count: 1
            }]
        );
        assert!(
            cell_spans(&section, 1).is_empty(),
            "zero-face cell must be an empty row"
        );
    }

    #[test]
    fn zero_index_count_leaf_is_excluded() {
        // A drawable cell whose leaf carries no indices is not a draw candidate.
        // (Defensive: BVH primitive collection already drops index_count==0 faces,
        // but the predicate must independently exclude such a leaf.)
        let leaves = vec![leaf(0, 0, 0, 0)];
        let bsp = vec![empty_leaf(1)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        assert!(cell_spans(&section, 0).is_empty());
        assert_eq!(section.span_count, 0);
    }

    #[test]
    fn offsets_are_a_correct_prefix_sum_in_ascending_cell_order() {
        // Three cells, varying span counts. Offsets must be the running prefix sum
        // and serialize cells in ascending id.
        //   cell 0: 1 span (slot 0)
        //   cell 1: solid -> 0 spans
        //   cell 2: 2 spans (interleaved buckets, slots 1 and 3)
        let leaves = sort_like_bvh(vec![
            leaf(0, 0, 0, 3),  // bucket 0, cell 0
            leaf(0, 2, 10, 3), // bucket 0, cell 2
            leaf(1, 1, 20, 3), // bucket 1, cell 1 (solid -> dropped)
            leaf(1, 2, 30, 3), // bucket 1, cell 2
        ]);
        // Sorted slots: 0=(b0,c0), 1=(b0,c2), 2=(b1,c1), 3=(b1,c2).
        let bsp = vec![empty_leaf(1), solid_leaf(), empty_leaf(2)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        // cell 0: [0,1) span; cell 1: empty; cell 2: [1,3) two spans.
        assert_eq!(section.cell_span_offset, vec![0, 1, 1, 3]);
        assert_eq!(
            cell_spans(&section, 0),
            &[Span {
                leaf_start: 0,
                leaf_count: 1
            }]
        );
        assert!(cell_spans(&section, 1).is_empty());
        assert_eq!(
            cell_spans(&section, 2),
            &[
                Span {
                    leaf_start: 1,
                    leaf_count: 1
                },
                Span {
                    leaf_start: 3,
                    leaf_count: 1
                },
            ],
        );
        // Final offset equals span_count.
        assert_eq!(
            *section.cell_span_offset.last().unwrap(),
            section.span_count,
        );
    }

    #[test]
    fn every_drawable_leaf_covered_exactly_once() {
        // Mixed map: drawable and non-drawable cells, multiple buckets. Tally each
        // slot's coverage across the CSR and compare against the drawable predicate.
        let leaves = sort_like_bvh(vec![
            leaf(0, 0, 0, 3),
            leaf(0, 1, 3, 3), // cell 1 solid -> non-drawable
            leaf(0, 2, 6, 3),
            leaf(1, 0, 9, 3),
            leaf(1, 2, 12, 3),
            leaf(2, 3, 15, 0), // zero index_count -> non-drawable
        ]);
        let bsp = vec![empty_leaf(2), solid_leaf(), empty_leaf(2), empty_leaf(1)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        let mut coverage = vec![0u32; leaves.len()];
        for span in &section.spans {
            for slot in span.leaf_start..span.leaf_start + span.leaf_count {
                coverage[slot as usize] += 1;
            }
        }
        for (slot, leaf) in leaves.iter().enumerate() {
            let expected = if is_drawable(leaf, &bsp) { 1 } else { 0 };
            assert_eq!(
                coverage[slot], expected,
                "slot {slot} coverage mismatch (cell {})",
                leaf.cell_id,
            );
        }
    }

    #[test]
    fn round_trips_through_format_crate() {
        // The baked section must satisfy the format crate's structural validation.
        let leaves = sort_like_bvh(vec![leaf(0, 0, 0, 3), leaf(0, 2, 6, 3), leaf(1, 2, 12, 3)]);
        let bsp = vec![empty_leaf(1), solid_leaf(), empty_leaf(2)];
        let section = bake_cell_draw_index(&leaves, &bsp).unwrap();

        let bytes = section.to_bytes();
        let restored = CellDrawIndexSection::from_bytes(&bytes).expect("structural round-trip");
        assert_eq!(section, restored);
    }
}
