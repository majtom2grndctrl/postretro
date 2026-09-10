// Optional and derived PRL section packing helpers.
// See: context/lib/build_pipeline.md

use postretro_level_format::bvh::BvhSection;

/// Clone a `BvhSection` with per-leaf animated-light chunk ranges stamped into
/// the output `BvhLeaf` records.
///
/// `chunk_ranges` is the parallel `(chunk_range_start, chunk_range_count)`
/// table returned by `animated_light_chunks::build_animated_light_chunks`,
/// indexed by BVH leaf slot. Pass an empty slice when no animated-light chunk
/// section is being emitted — every leaf then carries `(0, 0)` (the default).
///
/// This is the only sanctioned site that stamps the chunk-range fields, keeping
/// the animated-light chunk dependency explicit before payload serialization.
pub(crate) fn bvh_with_chunk_ranges(bvh: &BvhSection, chunk_ranges: &[(u32, u32)]) -> BvhSection {
    if chunk_ranges.is_empty() {
        // No chunk ranges to stamp — leaves keep their (0, 0) default.
        return bvh.clone();
    }
    debug_assert_eq!(
        chunk_ranges.len(),
        bvh.leaves.len(),
        "chunk_ranges must be parallel to bvh.leaves",
    );
    let mut stamped = bvh.clone();
    for (leaf, &(start, count)) in stamped.leaves.iter_mut().zip(chunk_ranges.iter()) {
        leaf.chunk_range_start = start;
        leaf.chunk_range_count = count;
    }
    stamped
}
