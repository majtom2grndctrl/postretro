// Pure camera draw ranges from visible cells' load-validated CSR spans.
// See: context/lib/rendering_pipeline.md §5, §7.1–7.3

use std::collections::HashSet;

use postretro_level_format::cell_draw_index::Span;
use postretro_level_loader::CellDrawIndex;
use postretro_render_data::geometry::BucketRange;
use postretro_visibility::VisibleCells;

/// Reusable per-frame camera ranges, shared by depth and forward drawing.
#[derive(Default)]
pub struct VisibleSpanRanges {
    ranges: Vec<BucketRange>,
    spans: Vec<Span>,
    seen: HashSet<u32>,
    #[cfg(test)]
    spans_read: usize,
    #[cfg(test)]
    buckets_visited: usize,
}

impl VisibleSpanRanges {
    /// Inputs must come from the same loaded level. The loader guarantees
    /// disjoint spans, each wholly inside one ordered material bucket.
    /// Scratch grows on warmup; thereafter work reads only visible CSR rows.
    pub fn rebuild(
        &mut self,
        index: Option<&CellDrawIndex>,
        visible: &VisibleCells,
        buckets: &[BucketRange],
    ) {
        self.ranges.clear();
        self.spans.clear();
        self.seen.clear();
        #[cfg(test)]
        {
            self.spans_read = 0;
            self.buckets_visited = 0;
        }

        // Equal-sized cell sets can own very different numbers of spans.
        // The flat payload length bounds both scratch and maximal output runs
        // without scanning invisible rows, including after a level reinstall.
        let span_bound = index.map_or(0, |index| index.spans.len());
        self.spans.reserve(span_bound);
        self.ranges.reserve(span_bound.max(buckets.len()));

        let (Some(index), VisibleCells::Culled(cells)) = (index, visible) else {
            self.whole_buckets(buckets);
            return;
        };
        self.seen.reserve(cells.len());
        for &cell in cells {
            if cell >= index.cell_count {
                self.whole_buckets(buckets);
                return;
            }
            if !self.seen.insert(cell) {
                continue;
            }
            let cell = cell as usize;
            let start = index.cell_span_offset[cell] as usize;
            let end = index.cell_span_offset[cell + 1] as usize;
            #[cfg(test)]
            {
                self.spans_read += end - start;
            }
            self.spans.extend_from_slice(&index.spans[start..end]);
        }
        self.spans.sort_unstable_by_key(|span| span.leaf_start);

        let mut bucket_cursor = 0;
        let mut current_bucket = buckets.first();
        let mut previous_bucket = None;
        #[cfg(test)]
        if !self.spans.is_empty() && current_bucket.is_some() {
            self.buckets_visited = 1;
        }
        for span in &self.spans {
            while let Some(bucket) = current_bucket {
                if span.leaf_start < bucket.first_leaf + bucket.leaf_count {
                    break;
                }
                bucket_cursor += 1;
                current_bucket = buckets.get(bucket_cursor);
                #[cfg(test)]
                if current_bucket.is_some() {
                    self.buckets_visited += 1;
                }
            }
            let bucket = current_bucket.expect("load-validated span belongs to a bucket");
            let range = BucketRange {
                material_bucket_id: bucket.material_bucket_id,
                first_leaf: span.leaf_start,
                leaf_count: span.leaf_count,
            };
            if let Some(previous) = self.ranges.last_mut()
                && spans_abut_in_same_bucket(
                    previous,
                    &range,
                    previous_bucket == Some(bucket_cursor),
                )
            {
                previous.leaf_count += range.leaf_count;
            } else {
                self.ranges.push(range);
            }
            previous_bucket = Some(bucket_cursor);
        }
    }

    pub fn ranges(&self) -> &[BucketRange] {
        &self.ranges
    }

    fn whole_buckets(&mut self, buckets: &[BucketRange]) {
        self.ranges.clear();
        self.ranges.extend(
            buckets
                .iter()
                .copied()
                .filter(|bucket| bucket.leaf_count > 0),
        );
    }
}

fn spans_abut_in_same_bucket(
    previous: &BucketRange,
    next: &BucketRange,
    same_bucket: bool,
) -> bool {
    same_bucket && previous.first_leaf + previous.leaf_count == next.first_leaf
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_sim::alloc_probe::AllocSnapshot;

    fn bucket(material_bucket_id: u32, first_leaf: u32, leaf_count: u32) -> BucketRange {
        BucketRange {
            material_bucket_id,
            first_leaf,
            leaf_count,
        }
    }

    fn index(rows: &[&[(u32, u32)]]) -> CellDrawIndex {
        let mut offsets = vec![0];
        let mut spans = Vec::new();
        for row in rows {
            spans.extend(row.iter().map(|&(leaf_start, leaf_count)| Span {
                leaf_start,
                leaf_count,
            }));
            offsets.push(spans.len() as u32);
        }
        CellDrawIndex {
            cell_count: rows.len() as u32,
            span_count: spans.len() as u32,
            cell_span_offset: offsets,
            spans,
        }
    }

    fn fixture() -> (CellDrawIndex, Vec<BucketRange>) {
        // Several spans per (cell, bucket), unsorted CSR rows, and an empty cell.
        (
            index(&[
                &[(6, 2), (0, 2)],
                &[(2, 1), (8, 2)],
                &[(3, 3), (10, 2)],
                &[],
            ]),
            vec![bucket(4, 0, 6), bucket(9, 6, 6), bucket(12, 12, 0)],
        )
    }

    fn slots(ranges: &[BucketRange]) -> Vec<u32> {
        ranges
            .iter()
            .flat_map(|range| range.first_leaf..range.first_leaf + range.leaf_count)
            .collect()
    }

    #[test]
    fn visible_ranges_cover_distinct_cells_exactly_without_invisible_slots() {
        let (index, buckets) = fixture();
        let mut ranges = VisibleSpanRanges::default();
        for mask in 0..16u32 {
            let mut cells: Vec<_> = (0..4)
                .rev()
                .filter(|cell| mask & (1 << cell) != 0)
                .collect();
            cells.extend_from_within(..);
            ranges.rebuild(Some(&index), &VisibleCells::Culled(cells), &buckets);
            let mut expected = Vec::new();
            for cell in 0..4 {
                if mask & (1 << cell) != 0 {
                    for span in &index.spans[index.cell_span_offset[cell] as usize
                        ..index.cell_span_offset[cell + 1] as usize]
                    {
                        expected.extend(span.leaf_start..span.leaf_start + span.leaf_count);
                    }
                }
            }
            expected.sort_unstable();
            assert_eq!(slots(ranges.ranges()), expected, "visible mask {mask}");
            for pair in ranges.ranges().windows(2) {
                assert!(pair[0].first_leaf < pair[1].first_leaf);
                assert!(
                    pair[0].material_bucket_id != pair[1].material_bucket_id
                        || pair[0].first_leaf + pair[0].leaf_count < pair[1].first_leaf
                );
            }
        }
    }

    #[test]
    fn visible_ranges_merge_abutting_spans_only_within_one_bucket() {
        let (index, buckets) = fixture();
        let mut ranges = VisibleSpanRanges::default();
        for cells in [vec![0, 1], vec![1, 0], vec![1, 0, 1]] {
            ranges.rebuild(Some(&index), &VisibleCells::Culled(cells), &buckets);
            assert_eq!(ranges.ranges(), &[bucket(4, 0, 3), bucket(9, 6, 4)]);
        }
        ranges.rebuild(Some(&index), &VisibleCells::Culled(vec![0, 2]), &buckets);
        assert_eq!(
            ranges.ranges(),
            &[
                bucket(4, 0, 2),
                bucket(4, 3, 3),
                bucket(9, 6, 2),
                bucket(9, 10, 2)
            ]
        );
        ranges.rebuild(Some(&index), &VisibleCells::Culled(vec![2, 1, 0]), &buckets);
        assert_eq!(ranges.ranges(), &[bucket(4, 0, 6), bucket(9, 6, 6)]);
        assert_eq!(slots(ranges.ranges()), slots(&buckets));
        assert!(
            ranges.ranges().len()
                <= buckets
                    .iter()
                    .filter(|bucket| bucket.leaf_count > 0)
                    .count()
        );
    }

    #[test]
    fn visible_ranges_clear_previous_frame_for_empty_and_zero_span_sets() {
        let (index, buckets) = fixture();
        let mut ranges = VisibleSpanRanges::default();
        for cells in [vec![0], vec![], vec![1], vec![3], vec![2], vec![2]] {
            ranges.rebuild(Some(&index), &VisibleCells::Culled(cells.clone()), &buckets);
            if cells.is_empty() || cells == [3] {
                assert!(ranges.ranges().is_empty());
            } else {
                let cell = cells[0] as usize;
                let mut expected: Vec<_> = index.spans[index.cell_span_offset[cell] as usize
                    ..index.cell_span_offset[cell + 1] as usize]
                    .iter()
                    .flat_map(|span| span.leaf_start..span.leaf_start + span.leaf_count)
                    .collect();
                expected.sort_unstable();
                assert_eq!(slots(ranges.ranges()), expected);
            }
        }
    }

    #[test]
    fn visible_ranges_fallback_for_any_invalid_id_then_recover_next_frame() {
        let (index, buckets) = fixture();
        let whole = &buckets[..2];
        let mut ranges = VisibleSpanRanges::default();
        for cells in [vec![4, 0], vec![0, 4], vec![u32::MAX], vec![0, 1, 4]] {
            ranges.rebuild(Some(&index), &VisibleCells::Culled(cells), &buckets);
            assert_eq!(ranges.ranges(), whole);
            ranges.rebuild(Some(&index), &VisibleCells::Culled(vec![0]), &buckets);
            assert_eq!(ranges.ranges(), &[bucket(4, 0, 2), bucket(9, 6, 2)]);
        }
        ranges.rebuild(Some(&index), &VisibleCells::Culled(vec![3]), &buckets);
        assert!(ranges.ranges().is_empty(), "last valid id uses spans");
        ranges.rebuild(Some(&index), &VisibleCells::DrawAll, &buckets);
        assert_eq!(ranges.ranges(), whole);
        ranges.rebuild(None, &VisibleCells::Culled(vec![]), &buckets);
        assert_eq!(ranges.ranges(), whole);
    }

    #[test]
    fn visible_ranges_read_new_level_even_when_visible_set_is_unchanged() {
        let (old, buckets) = fixture();
        let new = index(&[&[(4, 2)], &[(0, 4)], &[(6, 6)], &[]]);
        let visible = VisibleCells::Culled(vec![0]);
        let mut ranges = VisibleSpanRanges::default();
        ranges.rebuild(Some(&old), &visible, &buckets);
        ranges.rebuild(Some(&new), &visible, &buckets);
        assert_eq!(ranges.ranges(), &[bucket(4, 4, 2)]);
        ranges.rebuild(None, &VisibleCells::DrawAll, &buckets);
        ranges.rebuild(Some(&new), &visible, &buckets);
        assert_eq!(ranges.ranges(), &[bucket(4, 4, 2)]);
    }

    #[test]
    fn visible_ranges_touch_only_visible_spans_and_walk_buckets_once() {
        let rows: Vec<_> = (0..4096).map(|cell| vec![(cell, 1)]).collect();
        let row_slices: Vec<_> = rows.iter().map(Vec::as_slice).collect();
        let index = index(&row_slices);
        let buckets = [
            bucket(0, 0, 1024),
            bucket(1, 1024, 1024),
            bucket(2, 2048, 1024),
            bucket(3, 3072, 1024),
        ];
        let mut ranges = VisibleSpanRanges::default();
        ranges.rebuild(
            Some(&index),
            &VisibleCells::Culled(vec![3500, 3500]),
            &buckets,
        );
        assert_eq!(ranges.spans_read, 1);
        assert_eq!(ranges.buckets_visited, buckets.len());
        assert_eq!(ranges.ranges(), &[bucket(3, 3500, 1)]);
        ranges.rebuild(
            Some(&index),
            &VisibleCells::Culled(vec![3500, 0, 2048, 1, 1024]),
            &buckets,
        );
        assert_eq!(ranges.spans_read, 5);
        assert_eq!(ranges.buckets_visited, buckets.len());
    }

    #[test]
    fn visible_ranges_allocate_nothing_after_warmup_despite_changed_span_fanout() {
        let index = index(&[&[(0, 1)], &[(1, 1), (3, 1), (5, 1)], &[(2, 1), (4, 1)], &[]]);
        let buckets = [bucket(0, 0, 6)];
        let sets = [
            VisibleCells::Culled(vec![0]),
            VisibleCells::Culled(vec![1]),
            VisibleCells::Culled(vec![2]),
            VisibleCells::Culled(vec![3]),
            VisibleCells::Culled(vec![]),
            VisibleCells::DrawAll,
            VisibleCells::Culled(vec![4]),
        ];
        let mut ranges = VisibleSpanRanges::default();
        ranges.rebuild(Some(&index), &sets[0], &buckets);
        let probe = AllocSnapshot::arm();
        for _ in 0..8 {
            for visible in &sets {
                ranges.rebuild(Some(&index), visible, &buckets);
            }
            ranges.rebuild(None, &sets[0], &buckets);
        }
        assert_eq!(probe.allocs_since(), 0);
        let bigger = VisibleCells::Culled(vec![0, 1, 2, 3]);
        ranges.rebuild(Some(&index), &bigger, &buckets);
        let probe = AllocSnapshot::arm();
        ranges.rebuild(Some(&index), &bigger, &buckets);
        for visible in &sets {
            ranges.rebuild(Some(&index), visible, &buckets);
        }
        assert_eq!(probe.allocs_since(), 0);
    }
}
