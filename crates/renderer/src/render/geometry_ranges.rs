// Install-boundary validation for baked indirect draw index ranges.
// See: context/lib/rendering_pipeline.md §5

use postretro_render_data::geometry::BvhLeaf;

/// A baked leaf cannot be drawn against the index array being installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelGeometryRangeError {
    pub leaf_index: usize,
    pub index_offset: u32,
    pub index_count: u32,
    pub geometry_index_count: usize,
}

impl std::fmt::Display for LevelGeometryRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.index_offset.checked_add(self.index_count) {
            Some(end) => write!(
                f,
                "BVH leaf {} index range [{}..{}) exceeds Geometry index count {}",
                self.leaf_index, self.index_offset, end, self.geometry_index_count,
            ),
            None => write!(
                f,
                "BVH leaf {} index_offset {} + index_count {} overflows u32",
                self.leaf_index, self.index_offset, self.index_count,
            ),
        }
    }
}

impl std::error::Error for LevelGeometryRangeError {}

/// Validate before any level install step, including textures and CPU state.
/// Release builds disable wgpu's indirect-call validation and rely on this
/// check plus the same-leaf writer invariant. Empty ranges still need an
/// offset at or before the checked index-buffer boundary.
pub fn validate_level_geometry_ranges(
    leaves: &[BvhLeaf],
    geometry_index_count: usize,
) -> std::result::Result<(), LevelGeometryRangeError> {
    for (leaf_index, leaf) in leaves.iter().enumerate() {
        if leaf
            .index_offset
            .checked_add(leaf.index_count)
            .is_none_or(|end| end as usize > geometry_index_count)
        {
            return Err(LevelGeometryRangeError {
                leaf_index,
                index_offset: leaf.index_offset,
                index_count: leaf.index_count,
                geometry_index_count,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(index_offset: u32, index_count: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
            material_bucket_id: 0,
            index_offset,
            index_count,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    #[test]
    fn install_ranges_reject_aligned_past_end_and_u32_overflow() {
        let err = validate_level_geometry_ranges(&[leaf(0, 3), leaf(6, 3)], 6).unwrap_err();
        assert_eq!(err.leaf_index, 1);
        assert_eq!(
            err.to_string(),
            "BVH leaf 1 index range [6..9) exceeds Geometry index count 6"
        );
        let err = validate_level_geometry_ranges(&[leaf(u32::MAX, 3)], usize::MAX).unwrap_err();
        assert!(err.to_string().contains("overflows u32"));
    }

    #[test]
    fn install_ranges_accept_exact_end_and_zero_leaves() {
        assert!(validate_level_geometry_ranges(&[leaf(0, 3), leaf(3, 3)], 6).is_ok());
        assert!(validate_level_geometry_ranges(&[], 0).is_ok());
    }

    #[test]
    fn install_ranges_accept_empty_leaf_at_end_but_reject_offset_past_end() {
        assert!(validate_level_geometry_ranges(&[leaf(6, 0)], 6).is_ok());
        assert!(validate_level_geometry_ranges(&[leaf(9, 0)], 6).is_err());
    }
}
