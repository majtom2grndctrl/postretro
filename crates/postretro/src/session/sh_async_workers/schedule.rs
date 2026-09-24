//! Pure read scheduling for the SH I/O issuer: tier choice, offset order,
//! and coalescing. No threads or I/O, so every rule is unit-testable.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::ops::Range;

/// Largest run of unrequested bytes one physical read may cover and discard.
pub(super) const COALESCE_MAX_GAP_BYTES: u64 = 256 * 1024;
/// Largest span one coalesced read may cover. A single chunk above it is
/// read alone.
pub(super) const COALESCE_MAX_SPAN_BYTES: u64 = 16 * 1024 * 1024;

/// One pending chunk as the planner sees it: its tier and absolute file range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChunkSlot {
    pub(super) mandatory: bool,
    pub(super) range: Range<u64>,
}

/// One physical read: the file span and the slots it serves, by index into the
/// planner's input, in ascending offset order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadPlan {
    pub(super) span: Range<u64>,
    pub(super) members: Vec<usize>,
}

impl ReadPlan {
    /// Span bytes that belong to no member chunk; they are read and discarded.
    pub(super) fn gap_bytes(&self, slots: &[ChunkSlot]) -> u64 {
        let chunk_bytes: u64 = self
            .members
            .iter()
            .map(|&index| range_len(&slots[index].range))
            .sum();
        range_len(&self.span).saturating_sub(chunk_bytes)
    }
}

/// Plans the next physical read. The mandatory tier is served first whenever
/// it has any pending chunk, so optional work never delays it; within the
/// chosen tier the lowest offset starts the read, and following chunks of the
/// same tier merge while the gap and span caps hold.
pub(super) fn plan_next_read(slots: &[ChunkSlot]) -> Option<ReadPlan> {
    let mandatory = slots.iter().any(|slot| slot.mandatory);
    let mut tier: Vec<usize> = (0..slots.len())
        .filter(|&index| slots[index].mandatory == mandatory)
        .collect();
    tier.sort_by_key(|&index| (slots[index].range.start, slots[index].range.end));
    let (&first, rest) = tier.split_first()?;
    let mut span = slots[first].range.clone();
    let mut members = vec![first];
    for &index in rest {
        let range = &slots[index].range;
        let gap = range.start.saturating_sub(span.end);
        let merged_end = span.end.max(range.end);
        if gap > COALESCE_MAX_GAP_BYTES || merged_end - span.start > COALESCE_MAX_SPAN_BYTES {
            break;
        }
        span.end = merged_end;
        members.push(index);
    }
    Some(ReadPlan { span, members })
}

/// Splits one span read into per-chunk buffers, in the order of `ranges`.
/// A read serving one chunk that fills the span moves the buffer unchanged.
pub(super) fn split_span(span: &Range<u64>, bytes: Vec<u8>, ranges: &[Range<u64>]) -> Vec<Vec<u8>> {
    if let [only] = ranges
        && *only == *span
    {
        return vec![bytes];
    }
    ranges
        .iter()
        .map(|range| {
            let start = usize::try_from(range.start - span.start).expect("span fits memory");
            let end = usize::try_from(range.end - span.start).expect("span fits memory");
            bytes[start..end].to_vec()
        })
        .collect()
}

fn range_len(range: &Range<u64>) -> u64 {
    range.end.saturating_sub(range.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(mandatory: bool, start: u64, len: u64) -> ChunkSlot {
        ChunkSlot {
            mandatory,
            range: start..start + len,
        }
    }

    /// Drains `slots` through the planner the way the issuer does, returning
    /// the member indices of each physical read in issue order.
    fn drain_order(slots: &[ChunkSlot]) -> Vec<Vec<usize>> {
        let mut remaining: Vec<usize> = (0..slots.len()).collect();
        let mut reads = Vec::new();
        while !remaining.is_empty() {
            let view: Vec<ChunkSlot> = remaining.iter().map(|&i| slots[i].clone()).collect();
            let plan = plan_next_read(&view).expect("nonempty pending set");
            let issued: Vec<usize> = plan.members.iter().map(|&i| remaining[i]).collect();
            remaining.retain(|index| !issued.contains(index));
            reads.push(issued);
        }
        reads
    }

    #[test]
    fn empty_pending_set_plans_nothing() {
        assert_eq!(plan_next_read(&[]), None);
    }

    #[test]
    fn shuffled_tiers_drain_mandatory_first_then_optional_each_ascending() {
        let far = COALESCE_MAX_GAP_BYTES * 4;
        let slots = [
            slot(false, 2 * far, 10),
            slot(true, 5 * far, 10),
            slot(false, 0, 10),
            slot(true, far, 10),
            slot(false, 6 * far, 10),
            slot(true, 3 * far, 10),
            slot(false, 4 * far, 10),
        ];
        assert_eq!(
            drain_order(&slots),
            vec![
                vec![3],
                vec![5],
                vec![1],
                vec![2],
                vec![0],
                vec![6],
                vec![4]
            ]
        );
    }

    #[test]
    fn a_lower_offset_optional_chunk_never_precedes_pending_mandatory_work() {
        let slots = [slot(false, 0, 10), slot(true, 1 << 30, 10)];
        let plan = plan_next_read(&slots).unwrap();
        assert_eq!(plan.members, vec![1]);
    }

    #[test]
    fn coalescing_never_pulls_the_other_tier_into_a_read() {
        // The optional chunk sits in the gap between two mandatory chunks.
        let slots = [slot(true, 0, 10), slot(false, 10, 10), slot(true, 20, 10)];
        let plan = plan_next_read(&slots).unwrap();
        assert_eq!(plan.members, vec![0, 2]);
        assert_eq!(plan.span, 0..30);
        assert_eq!(plan.gap_bytes(&slots), 10);
    }

    #[test]
    fn chunks_within_the_gap_cap_merge_and_one_byte_more_splits() {
        let at_cap = [
            slot(true, 0, 100),
            slot(true, 100 + COALESCE_MAX_GAP_BYTES, 50),
        ];
        let plan = plan_next_read(&at_cap).unwrap();
        assert_eq!(plan.members, vec![0, 1]);
        assert_eq!(plan.span, 0..150 + COALESCE_MAX_GAP_BYTES);
        assert_eq!(plan.gap_bytes(&at_cap), COALESCE_MAX_GAP_BYTES);

        let over_cap = [
            slot(true, 0, 100),
            slot(true, 101 + COALESCE_MAX_GAP_BYTES, 50),
        ];
        assert_eq!(plan_next_read(&over_cap).unwrap().members, vec![0]);
    }

    #[test]
    fn merged_span_stops_at_the_span_cap() {
        let half = COALESCE_MAX_SPAN_BYTES / 2;
        let exact = [slot(true, 0, half), slot(true, half, half)];
        assert_eq!(plan_next_read(&exact).unwrap().members, vec![0, 1]);

        let over = [
            slot(true, 0, half),
            slot(true, half, half),
            slot(true, 2 * half, 1),
        ];
        let plan = plan_next_read(&over).unwrap();
        assert_eq!(plan.members, vec![0, 1]);
        assert_eq!(plan.span, 0..COALESCE_MAX_SPAN_BYTES);
    }

    #[test]
    fn a_chunk_larger_than_the_span_cap_is_read_alone() {
        let slots = [
            slot(true, 0, COALESCE_MAX_SPAN_BYTES + 1),
            slot(true, COALESCE_MAX_SPAN_BYTES + 1, 10),
        ];
        assert_eq!(drain_order(&slots), vec![vec![0], vec![1]]);
        let plan = plan_next_read(&slots).unwrap();
        assert_eq!(plan.span, 0..COALESCE_MAX_SPAN_BYTES + 1);
    }

    #[test]
    fn split_span_returns_exact_per_chunk_bytes_and_drops_gaps() {
        let bytes: Vec<u8> = (100..130).collect();
        let parts = split_span(&(100..130), bytes, &[100..105, 110..130]);
        assert_eq!(parts[0], (100..105).collect::<Vec<u8>>());
        assert_eq!(parts[1], (110..130).collect::<Vec<u8>>());
    }

    #[test]
    fn split_span_moves_a_single_full_span_chunk() {
        let bytes = vec![1, 2, 3];
        let pointer = bytes.as_ptr();
        let span = 7..10;
        let parts = split_span(&span, bytes, std::slice::from_ref(&span));
        assert_eq!(parts[0].as_ptr(), pointer, "no copy for a single chunk");
    }
}
