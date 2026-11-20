// Per-face index ranges recovered from the BVH leaves, for streams the loader
// stamps per world vertex (static specular owners, emissive volume ids).
// See: context/lib/build_pipeline.md §PRL section IDs

use std::ops::Range;

use postretro_render_data::geometry::BvhLeaf;

/// Face `i`'s index range is the `i`-th BVH leaf range by `index_offset`.
/// The ranges must tile `0..index_count` exactly, one non-empty leaf per face.
///
/// Id 17 keeps no face index ranges, but the compiler emits each face's
/// indices as one contiguous range, ranges in face order, and each face with
/// a triangle owns exactly one BVH leaf covering its range. The BVH flattens
/// leaves in material-bucket order, so they arrive shuffled; sorting by
/// `index_offset` restores face order, and face `i`'s range is the `i`-th. A
/// face with no triangle has no leaf, so the leaf count then falls short of
/// the face count and recovery fails by name.
pub(crate) fn face_index_ranges(
    leaves: &[BvhLeaf],
    face_count: usize,
    index_count: usize,
) -> Result<Vec<Range<usize>>, String> {
    if leaves.len() != face_count {
        return Err(format!(
            "{} BVH leaves for {face_count} Geometry faces",
            leaves.len()
        ));
    }
    let mut ranges: Vec<Range<usize>> = leaves
        .iter()
        .map(|leaf| {
            let start = leaf.index_offset as usize;
            start..start + leaf.index_count as usize
        })
        .collect();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut expected = 0;
    for (face, range) in ranges.iter().enumerate() {
        if range.start != expected || range.is_empty() {
            return Err(format!(
                "face {face}'s leaf covers indices {range:?}, expected a non-empty range \
                 starting at {expected}"
            ));
        }
        expected = range.end;
    }
    if expected != index_count {
        return Err(format!(
            "BVH leaves cover {expected} of the {index_count} Geometry indices"
        ));
    }
    Ok(ranges)
}
