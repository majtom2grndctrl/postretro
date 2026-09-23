// Shared SH compose dispatch decisions.
// See: context/lib/rendering_pipeline.md §7.1

use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, DYNAMIC_COMPOSE_GRID_DIMS_SIZE, DynamicComposeGridParams,
    build_dynamic_compose_grid_bytes,
};

/// The compose atlas must be refreshed for new animated input, its initial
/// base copy, the frame after animated input stops, or a changed light-term
/// mask. Keeping that decision independent of individual compose pipelines is
/// the seam where streamed dirty ranges will replace whole-grid dispatches.
pub(super) fn should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
    frame_light_term_mask: LightTermMask,
    last_composed_mask: LightTermMask,
) -> bool {
    active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask
}

/// Checked x-fastest flattened affinity-cell count. Dynamic compose records
/// and their dispatches use this same range representation.
pub(super) fn checked_affinity_range_count(dimensions: [u32; 3]) -> Option<u32> {
    dimensions
        .into_iter()
        .try_fold(1u32, |count, dimension| count.checked_mul(dimension))
}

/// One dynamic-uniform record and its matching bounded one-dimensional
/// dispatch. `dynamic_offset` is always aligned to the device limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DynamicComposeDispatch {
    pub(super) range_start: u32,
    pub(super) range_count: u32,
    pub(super) dynamic_offset: u32,
    pub(super) workgroup_count: u32,
}

/// Dynamic grid bytes, including alignment padding between records, and the
/// dispatches that select those records. Keeping the range plan next to the
/// packed records prevents a bind-group offset from drifting from the shader's
/// `range_start` / `range_count` pair.
pub(super) struct DynamicComposeGridUpload {
    pub(super) bytes: Vec<u8>,
    pub(super) dispatches: Vec<DynamicComposeDispatch>,
}

/// Splits a flattened affinity range into no-more-than-`max_workgroups_x`
/// chunks. The dummy zero-range path retains one workgroup: the shader returns
/// before touching shared memory, but the bind group remains valid.
pub(super) fn dynamic_compose_dispatches(
    range_count: u32,
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
) -> Option<Vec<DynamicComposeDispatch>> {
    dynamic_compose_dispatches_for_ranges(
        &[(0, range_count)],
        range_count,
        max_workgroups_x,
        dynamic_offset_alignment,
    )
}

/// Plans bounded dispatches for arbitrary dirty ranges. Each input pair is
/// `(range_start, range_count)` in the flattened affinity-row namespace. A
/// range is split further when it exceeds the adapter's X-workgroup cap.
///
/// Empty input (or only zero-count ranges) deliberately produces one dummy
/// record so bind-group validation remains valid while the shader exits before
/// doing any work.
pub(super) fn dynamic_compose_dispatches_for_ranges(
    dirty_ranges: &[(u32, u32)],
    total_range_count: u32,
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
) -> Option<Vec<DynamicComposeDispatch>> {
    if max_workgroups_x == 0 || dynamic_offset_alignment == 0 {
        return None;
    }
    let record_size = u32::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE).ok()?;
    let record_stride = record_size
        .checked_add(dynamic_offset_alignment.checked_sub(1)?)?
        .checked_div(dynamic_offset_alignment)?
        .checked_mul(dynamic_offset_alignment)?;

    let mut dispatches = Vec::new();
    for &(range_start, range_count) in dirty_ranges {
        let range_end = range_start.checked_add(range_count)?;
        if range_end > total_range_count {
            return None;
        }
        let mut chunk_start = range_start;
        let mut remaining = range_count;
        while remaining > 0 {
            let chunk_count = remaining.min(max_workgroups_x);
            let chunk_index = u32::try_from(dispatches.len()).ok()?;
            dispatches.push(DynamicComposeDispatch {
                range_start: chunk_start,
                range_count: chunk_count,
                dynamic_offset: chunk_index.checked_mul(record_stride)?,
                workgroup_count: chunk_count,
            });
            chunk_start = chunk_start.checked_add(chunk_count)?;
            remaining -= chunk_count;
        }
    }
    if dispatches.is_empty() {
        dispatches.push(DynamicComposeDispatch {
            range_start: 0,
            range_count: 0,
            dynamic_offset: 0,
            workgroup_count: 1,
        });
    }
    Some(dispatches)
}

/// Packs all adapter-bounded range records. `max_buffer_size` is checked
/// before allocation; binding 18 then selects one fixed 80-byte record with a
/// dynamic offset for each dispatch.
pub(super) fn build_dynamic_compose_grid_upload(
    grid: ComposeGridParams,
    physical_tile_stride: u32,
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
    max_buffer_size: u64,
) -> Option<DynamicComposeGridUpload> {
    let range_count = checked_affinity_range_count(grid.affinity_dims)?;
    build_dynamic_compose_grid_upload_for_ranges(
        grid,
        physical_tile_stride,
        &[(0, range_count)],
        max_workgroups_x,
        dynamic_offset_alignment,
        max_buffer_size,
    )
}

/// Equivalent to [`build_dynamic_compose_grid_upload`], but takes coalesced
/// dirty ranges for streamed residency. It validates every range against the
/// grid's flattened affinity-row count before allocating or packing bytes.
pub(super) fn build_dynamic_compose_grid_upload_for_ranges(
    grid: ComposeGridParams,
    physical_tile_stride: u32,
    dirty_ranges: &[(u32, u32)],
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
    max_buffer_size: u64,
) -> Option<DynamicComposeGridUpload> {
    let total_range_count = checked_affinity_range_count(grid.affinity_dims)?;
    let dispatches = dynamic_compose_dispatches_for_ranges(
        dirty_ranges,
        total_range_count,
        max_workgroups_x,
        dynamic_offset_alignment,
    )?;
    let last = dispatches.last()?;
    let byte_len = u64::from(last.dynamic_offset)
        .checked_add(u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE).ok()?)?;
    if byte_len > max_buffer_size {
        return None;
    }
    let mut bytes = vec![0; usize::try_from(byte_len).ok()?];
    for dispatch in &dispatches {
        let record = build_dynamic_compose_grid_bytes(DynamicComposeGridParams {
            grid,
            physical_tile_stride,
            range_start: dispatch.range_start,
            range_count: dispatch.range_count,
        });
        let offset = usize::try_from(dispatch.dynamic_offset).ok()?;
        bytes[offset..offset + DYNAMIC_COMPOSE_GRID_DIMS_SIZE].copy_from_slice(&record);
    }
    Some(DynamicComposeGridUpload { bytes, dispatches })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_for_initial_copy_activity_transition_or_mask_change() {
        assert!(should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
        assert!(should_dispatch(
            false,
            false,
            false,
            LightTermMask::AMBIENT_FLOOR,
            LightTermMask::ALL,
        ));
        assert!(!should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL,
        ));
    }

    #[test]
    fn flattened_ranges_are_checked() {
        assert_eq!(checked_affinity_range_count([0, 0, 0]), Some(0));
        assert_eq!(checked_affinity_range_count([2, 3, 4]), Some(24));
        assert_eq!(checked_affinity_range_count([u32::MAX, 2, 1]), None);
    }

    #[test]
    fn dynamic_ranges_are_adapter_bounded_and_offsets_aligned() {
        assert_eq!(
            dynamic_compose_dispatches(5, 2, 256),
            Some(vec![
                DynamicComposeDispatch {
                    range_start: 0,
                    range_count: 2,
                    dynamic_offset: 0,
                    workgroup_count: 2,
                },
                DynamicComposeDispatch {
                    range_start: 2,
                    range_count: 2,
                    dynamic_offset: 256,
                    workgroup_count: 2,
                },
                DynamicComposeDispatch {
                    range_start: 4,
                    range_count: 1,
                    dynamic_offset: 512,
                    workgroup_count: 1,
                },
            ])
        );
        assert_eq!(
            dynamic_compose_dispatches(0, 65_535, 256),
            Some(vec![DynamicComposeDispatch {
                range_start: 0,
                range_count: 0,
                dynamic_offset: 0,
                workgroup_count: 1,
            }])
        );
        assert_eq!(dynamic_compose_dispatches(1, 0, 256), None);
        assert_eq!(dynamic_compose_dispatches(1, 1, 0), None);
    }

    #[test]
    fn dirty_ranges_are_checked_split_and_keep_their_global_offsets() {
        assert_eq!(
            dynamic_compose_dispatches_for_ranges(&[(3, 5), (12, 1)], 16, 2, 256),
            Some(vec![
                DynamicComposeDispatch {
                    range_start: 3,
                    range_count: 2,
                    dynamic_offset: 0,
                    workgroup_count: 2,
                },
                DynamicComposeDispatch {
                    range_start: 5,
                    range_count: 2,
                    dynamic_offset: 256,
                    workgroup_count: 2,
                },
                DynamicComposeDispatch {
                    range_start: 7,
                    range_count: 1,
                    dynamic_offset: 512,
                    workgroup_count: 1,
                },
                DynamicComposeDispatch {
                    range_start: 12,
                    range_count: 1,
                    dynamic_offset: 768,
                    workgroup_count: 1,
                },
            ])
        );
        assert_eq!(
            dynamic_compose_dispatches_for_ranges(&[(15, 2)], 16, 2, 256),
            None
        );
        assert_eq!(
            dynamic_compose_dispatches_for_ranges(&[(u32::MAX, 1)], u32::MAX, 2, 256),
            None
        );
    }

    #[test]
    fn dynamic_grid_upload_bakes_each_chunk_range_into_its_record() {
        let grid = ComposeGridParams {
            grid_dimensions: [1, 1, 1],
            atlas_dimensions: [1, 1],
            tile_dimension: 6,
            tile_border: 1,
            atlas_tiles_per_row: 1,
            tiles_per_layer: 1,
            atlas_layer_count: 1,
            affinity_dims: [5, 1, 1],
            compact_atlas_tiles_per_row: 1,
            compact_atlas_tiles_per_layer: 1,
        };
        let upload = build_dynamic_compose_grid_upload(grid, 8, 2, 256, 4096).unwrap();
        assert_eq!(upload.bytes.len(), 592);
        assert_eq!(upload.dispatches.len(), 3);
        assert_eq!(
            u32::from_ne_bytes(upload.bytes[256 + 68..256 + 72].try_into().unwrap()),
            2
        );
        assert_eq!(
            u32::from_ne_bytes(upload.bytes[256 + 72..256 + 76].try_into().unwrap()),
            2
        );
        assert!(build_dynamic_compose_grid_upload(grid, 8, 2, 256, 591).is_none());
    }
}
