// Shared SH compose dispatch decisions and row-gather packing.
// See: context/lib/rendering_pipeline.md §4

use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::{
    ComposeGridParams, DYNAMIC_COMPOSE_GRID_DIMS_SIZE, DYNAMIC_COMPOSE_ROW_CAPACITY,
    DynamicComposeGridParams, build_dynamic_compose_grid_bytes,
};

pub(super) fn should_dispatch(
    active: bool,
    pending_copy_through: bool,
    was_active: bool,
    frame_light_term_mask: LightTermMask,
    last_composed_mask: LightTermMask,
) -> bool {
    active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask
}

pub(super) fn checked_affinity_range_count(dimensions: [u32; 3]) -> Option<u32> {
    dimensions
        .into_iter()
        .try_fold(1u32, |count, dimension| count.checked_mul(dimension))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DynamicComposeDispatch {
    pub(super) row_offset: u32,
    pub(super) row_count: u32,
    pub(super) dynamic_offset: u32,
    pub(super) workgroup_count: u32,
}

pub(super) struct DynamicComposeGridUpload {
    pub(super) bytes: Vec<u8>,
    pub(super) dispatches: Vec<DynamicComposeDispatch>,
}

pub(super) fn gather_chunk_capacity(max_workgroups_x: u32) -> Option<u32> {
    let capacity = u32::try_from(DYNAMIC_COMPOSE_ROW_CAPACITY)
        .ok()?
        .min(max_workgroups_x);
    (capacity > 0).then_some(capacity)
}

fn dynamic_compose_dispatches(
    row_count: usize,
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
) -> Option<Vec<DynamicComposeDispatch>> {
    if dynamic_offset_alignment == 0 {
        return None;
    }
    if row_count == 0 {
        return Some(Vec::new());
    }
    let chunk_capacity = usize::try_from(gather_chunk_capacity(max_workgroups_x)?).ok()?;
    let record_size = u32::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE).ok()?;
    let record_stride = record_size
        .checked_add(dynamic_offset_alignment.checked_sub(1)?)?
        .checked_div(dynamic_offset_alignment)?
        .checked_mul(dynamic_offset_alignment)?;
    let mut dispatches = Vec::with_capacity(row_count.div_ceil(chunk_capacity));
    for row_offset in (0..row_count).step_by(chunk_capacity) {
        let row_count = (row_count - row_offset).min(chunk_capacity);
        let chunk_index = u32::try_from(dispatches.len()).ok()?;
        let row_count = u32::try_from(row_count).ok()?;
        dispatches.push(DynamicComposeDispatch {
            row_offset: u32::try_from(row_offset).ok()?,
            row_count,
            dynamic_offset: chunk_index.checked_mul(record_stride)?,
            workgroup_count: row_count,
        });
    }
    Some(dispatches)
}

pub(super) fn build_dynamic_compose_grid_upload_for_rows(
    grid: ComposeGridParams,
    physical_tile_stride: u32,
    rows: &[u32],
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
    max_buffer_size: u64,
) -> Option<DynamicComposeGridUpload> {
    let total_rows = checked_affinity_range_count(grid.affinity_dims)?;
    if rows.windows(2).any(|pair| pair[0] >= pair[1])
        || rows.last().is_some_and(|row| *row >= total_rows)
    {
        return None;
    }
    let dispatches =
        dynamic_compose_dispatches(rows.len(), max_workgroups_x, dynamic_offset_alignment)?;
    if dispatches.is_empty() {
        return Some(DynamicComposeGridUpload {
            bytes: Vec::new(),
            dispatches,
        });
    }
    let last = dispatches.last()?;
    let byte_len = u64::from(last.dynamic_offset)
        .checked_add(u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE).ok()?)?;
    if byte_len > max_buffer_size {
        return None;
    }
    let mut bytes = vec![0; usize::try_from(byte_len).ok()?];
    for dispatch in &dispatches {
        let start = usize::try_from(dispatch.row_offset).ok()?;
        let end = start.checked_add(usize::try_from(dispatch.row_count).ok()?)?;
        let record = build_dynamic_compose_grid_bytes(
            DynamicComposeGridParams {
                grid,
                physical_tile_stride,
            },
            rows.get(start..end)?,
        )?;
        let offset = usize::try_from(dispatch.dynamic_offset).ok()?;
        bytes[offset..offset + DYNAMIC_COMPOSE_GRID_DIMS_SIZE].copy_from_slice(&record);
    }
    Some(DynamicComposeGridUpload { bytes, dispatches })
}

pub(super) fn build_dynamic_compose_grid_upload(
    grid: ComposeGridParams,
    physical_tile_stride: u32,
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
    max_buffer_size: u64,
) -> Option<DynamicComposeGridUpload> {
    let row_count = checked_affinity_range_count(grid.affinity_dims)?;
    let rows = (0..row_count).collect::<Vec<_>>();
    build_dynamic_compose_grid_upload_for_rows(
        grid,
        physical_tile_stride,
        &rows,
        max_workgroups_x,
        dynamic_offset_alignment,
        max_buffer_size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(row_count: u32) -> ComposeGridParams {
        ComposeGridParams {
            grid_dimensions: [1, 1, 1],
            atlas_dimensions: [1, 1],
            tile_dimension: 6,
            tile_border: 1,
            atlas_tiles_per_row: 1,
            tiles_per_layer: 1,
            atlas_layer_count: 1,
            affinity_dims: [row_count, 1, 1],
            compact_atlas_tiles_per_row: 1,
            compact_atlas_tiles_per_layer: 1,
        }
    }

    fn word(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn dispatches_for_initial_copy_activity_transition_or_mask_change() {
        assert!(should_dispatch(
            false,
            true,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL
        ));
        assert!(should_dispatch(
            true,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL
        ));
        assert!(should_dispatch(
            false,
            false,
            true,
            LightTermMask::ALL,
            LightTermMask::ALL
        ));
        assert!(should_dispatch(
            false,
            false,
            false,
            LightTermMask::AMBIENT_FLOOR,
            LightTermMask::ALL
        ));
        assert!(!should_dispatch(
            false,
            false,
            false,
            LightTermMask::ALL,
            LightTermMask::ALL
        ));
    }

    #[test]
    fn flattened_ranges_are_checked() {
        assert_eq!(checked_affinity_range_count([0, 0, 0]), Some(0));
        assert_eq!(checked_affinity_range_count([2, 3, 4]), Some(24));
        assert_eq!(checked_affinity_range_count([u32::MAX, 2, 1]), None);
    }

    #[test]
    fn gather_plan_chunks_fragmented_rows_without_duplication() {
        let rows = [1, 4, 9, 12, 18];
        let upload =
            build_dynamic_compose_grid_upload_for_rows(grid(20), 8, &rows, 3, 256, u64::MAX)
                .unwrap();
        assert_eq!(upload.dispatches.len(), 2);
        let first = (0..3)
            .map(|index| word(&upload.bytes, 80 + index * 4))
            .collect::<Vec<_>>();
        let second_base = upload.dispatches[1].dynamic_offset as usize;
        let second = (0..2)
            .map(|index| word(&upload.bytes, second_base + 80 + index * 4))
            .collect::<Vec<_>>();
        assert_eq!(first, [1, 4, 9]);
        assert_eq!(second, [12, 18]);
    }

    #[test]
    fn gather_plan_splits_only_above_capacity() {
        let capacity = DYNAMIC_COMPOSE_ROW_CAPACITY;
        let rows = (0..u32::try_from(capacity + 1).unwrap()).collect::<Vec<_>>();
        let exact = build_dynamic_compose_grid_upload_for_rows(
            grid(rows.len() as u32),
            8,
            &rows[..capacity],
            u32::MAX,
            256,
            u64::MAX,
        )
        .unwrap();
        let overflow = build_dynamic_compose_grid_upload_for_rows(
            grid(rows.len() as u32),
            8,
            &rows,
            u32::MAX,
            256,
            u64::MAX,
        )
        .unwrap();
        assert_eq!(exact.dispatches.len(), 1);
        assert_eq!(overflow.dispatches.len(), 2);
    }

    #[test]
    fn empty_gather_plan_has_no_dispatch() {
        let upload =
            build_dynamic_compose_grid_upload_for_rows(grid(1), 8, &[], 65_535, 256, u64::MAX)
                .unwrap();
        assert!(upload.bytes.is_empty());
        assert!(upload.dispatches.is_empty());
    }

    #[test]
    fn gather_rejects_duplicates_unsorted_and_out_of_range_rows() {
        for rows in [&[1, 1][..], &[2, 1][..], &[4][..]] {
            assert!(build_dynamic_compose_grid_upload_for_rows(
                grid(4), 8, rows, 65_535, 256, u64::MAX,
            ).is_none());
        }
    }

    #[test]
    fn legacy_gather_compose_covers_every_affinity_row_once() {
        let upload = build_dynamic_compose_grid_upload(grid(5), 8, 65_535, 256, u64::MAX).unwrap();
        assert_eq!(upload.dispatches.len(), 1);
        assert_eq!(
            (0..5)
                .map(|i| word(&upload.bytes, 80 + i * 4))
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
    }
}
