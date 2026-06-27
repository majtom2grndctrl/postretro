// Optional and derived PRL section packing helpers.
// See: context/lib/build_pipeline.md

use postretro_level_format::SectionBlob;
use postretro_level_format::bvh::BvhSection;

/// Serialize a `BvhSection` with per-leaf animated-light chunk ranges stamped
/// into the on-disk `BvhLeaf` records.
///
/// `chunk_ranges` is the parallel `(chunk_range_start, chunk_range_count)`
/// table returned by `animated_light_chunks::build_animated_light_chunks`,
/// indexed by BVH leaf slot. Pass an empty slice when no animated-light chunk
/// section is being emitted — every leaf then carries `(0, 0)` (the default).
///
/// This is the only sanctioned site that writes the chunk-range fields of
/// `BvhLeaf` to disk: keeping the application here, immediately adjacent to
/// `to_bytes()`, makes the "animated-light chunks must run before BVH
/// serialization" ordering an explicit data dependency rather than a hidden
/// side effect on `BvhSection`.
pub(crate) fn serialize_bvh_with_chunk_ranges(
    bvh: &BvhSection,
    chunk_ranges: &[(u32, u32)],
) -> Vec<u8> {
    if chunk_ranges.is_empty() {
        // No chunk ranges to stamp — leaves keep their (0, 0) default.
        return bvh.to_bytes();
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
    stamped.to_bytes()
}

/// Append an optional section blob when its data is present.
///
/// Generic over `section_id` so any optional PRL section can route through the
/// same append point. Absent data (`None`) is a no-op — the section is simply
/// omitted from the container, which is how the runtime distinguishes optional
/// sections.
pub(crate) fn append_optional_section(
    sections: &mut Vec<SectionBlob>,
    section_id: u32,
    data: Option<Vec<u8>>,
) {
    if let Some(bytes) = data {
        sections.push(SectionBlob {
            section_id,
            version: 1,
            data: bytes,
        });
    }
}
