//! Metadata-only id-35 and sparse-source projection parsing.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::fs::File;

use postretro_level_format::SectionEntry;
use postretro_level_format::SectionId;
use postretro_level_format::delta_sh_volumes::DELTA_SH_VOLUMES_VERSION;
use postretro_level_format::direct_sh_delta_volumes::DIRECT_SH_DELTA_VOLUMES_VERSION;
use postretro_level_format::direct_sh_volume::DIRECT_SH_VOLUME_VERSION;

use super::manifest::ensure_section_floor;
use super::metadata_base::{checked_product, read_f32, read_u32, read_u32_vec, read_u64};
use super::positional_io::read_vec_at;
use super::projection::{ShStreamBaseMetadata, ShStreamDirectMetadata, ShStreamSparseMetadata};
use super::{PrlLoadError, stream_error};
pub(super) fn read_optional_direct_metadata(
    file: &File,
    entry: Option<&SectionEntry>,
) -> Result<Option<ShStreamDirectMetadata>, PrlLoadError> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    ensure_section_floor(entry, 76, "id-35 header")?;
    let header = read_vec_at(file, entry.offset, 76, "id-35 header")?;
    if read_u32(&header, 0) != DIRECT_SH_VOLUME_VERSION {
        return Err(stream_error("id 35 has an unsupported internal version"));
    }
    let grid_origin = [
        read_f32(&header, 4),
        read_f32(&header, 8),
        read_f32(&header, 12),
    ];
    let cell_size = [
        read_f32(&header, 16),
        read_f32(&header, 20),
        read_f32(&header, 24),
    ];
    let grid_dimensions = [
        read_u32(&header, 28),
        read_u32(&header, 32),
        read_u32(&header, 36),
    ];
    let tile_dimension = read_u32(&header, 40);
    let tile_border = read_u32(&header, 44);
    let atlas_dimensions = [read_u32(&header, 48), read_u32(&header, 52)];
    let atlas_tiles_per_row = read_u32(&header, 56);
    let layer_count = read_u32(&header, 60);
    let tiles_per_layer = read_u32(&header, 64);
    let irradiance_format = read_u32(&header, 68);
    let atlas_len_header = read_u32(&header, 72);
    let atlas_len = u64::from(atlas_len_header);
    postretro_level_format::direct_sh_volume::validate_metadata_projection(
        grid_dimensions,
        tile_dimension,
        tile_border,
        atlas_dimensions,
        layer_count,
        tiles_per_layer,
        atlas_tiles_per_row,
        irradiance_format,
        atlas_len_header,
    )
    .map_err(PrlLoadError::FormatError)?;
    let expected_section_size = 76u64
        .checked_add(atlas_len)
        .ok_or_else(|| stream_error("id-35 section size overflows"))?;
    if entry.size != expected_section_size {
        return Err(stream_error(
            "id 35 header atlas length disagrees with section size",
        ));
    }
    Ok(Some(ShStreamDirectMetadata {
        grid_origin,
        cell_size,
        grid_dimensions,
        tile_dimension,
        tile_border,
        atlas_dimensions,
        layer_count,
        tiles_per_layer,
        atlas_tiles_per_row,
        irradiance_format,
    }))
}

pub(super) fn read_optional_sparse_metadata(
    file: &File,
    entry: Option<&SectionEntry>,
    section: SectionId,
    base: &ShStreamBaseMetadata,
) -> Result<Option<ShStreamSparseMetadata>, PrlLoadError> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let has_descriptor_map = matches!(
        section,
        SectionId::DeltaShVolumes | SectionId::AnimatedDirectShDeltaVolumes
    );
    let fixed_len = if has_descriptor_map { 26u64 } else { 22u64 };
    ensure_section_floor(entry, fixed_len, "streamed sparse header")?;
    let header = read_vec_at(file, entry.offset, fixed_len, "streamed sparse header")?;
    let expected_version = match section {
        SectionId::DeltaShVolumes => DELTA_SH_VOLUMES_VERSION,
        SectionId::DirectShDeltaVolumes => DIRECT_SH_DELTA_VOLUMES_VERSION,
        SectionId::AnimatedDirectShDeltaVolumes => {
            postretro_level_format::animated_direct_sh_delta_volumes::ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION
        }
        _ => return Err(stream_error("not a streamed sparse section")),
    };
    if header[0] != expected_version {
        return Err(stream_error(format!(
            "section {} has an unsupported internal version",
            section as u32
        )));
    }
    if header[1] != postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR {
        return Err(stream_error(format!(
            "section {} has a mismatched affinity factor",
            section as u32
        )));
    }
    let affinity_dims = [
        read_u32(&header, 2),
        read_u32(&header, 6),
        read_u32(&header, 10),
    ];
    let descriptor_count = if has_descriptor_map {
        read_u32(&header, 14)
    } else {
        0
    };
    let tile_dimension_offset = if has_descriptor_map { 18 } else { 14 };
    let tile_dimension = read_u32(&header, tile_dimension_offset);
    let tile_border = read_u32(&header, tile_dimension_offset + 4);
    let expected_affinity_dims = base.grid_dimensions.map(|dimension| dimension.div_ceil(4));
    if affinity_dims != expected_affinity_dims
        || tile_dimension != base.tile_dimension
        || tile_border != base.tile_border
    {
        return Err(stream_error(format!(
            "streamed sparse section {} metadata does not match id-34",
            section as u32
        )));
    }
    let cell_count = checked_product(affinity_dims, "streamed sparse affinity cell count")?;
    let descriptor_bytes = u64::from(descriptor_count)
        .checked_mul(4)
        .ok_or_else(|| stream_error("streamed sparse descriptor table overflows"))?;
    let masks_bytes = cell_count
        .checked_mul(8)
        .ok_or_else(|| stream_error("streamed sparse mask table overflows"))?;
    let offsets_bytes = cell_count
        .checked_add(1)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| stream_error("streamed sparse offset table overflows"))?;
    let before_entries = fixed_len
        .checked_add(descriptor_bytes)
        .and_then(|value| value.checked_add(masks_bytes))
        .and_then(|value| value.checked_add(cell_count))
        .and_then(|value| value.checked_add(offsets_bytes))
        .ok_or_else(|| stream_error("streamed sparse metadata length overflows"))?;
    if before_entries > entry.size {
        return Err(stream_error(
            "streamed sparse metadata exceeds section bounds",
        ));
    }
    let prefix = read_vec_at(
        file,
        entry.offset,
        before_entries,
        "streamed sparse metadata",
    )?;
    let cell_count_usize = usize::try_from(cell_count)
        .map_err(|_| stream_error("streamed sparse affinity cell count exceeds usize"))?;
    let mut cursor = usize::try_from(fixed_len)
        .map_err(|_| stream_error("fixed sparse header exceeds usize"))?;
    let descriptor_count_usize = usize::try_from(descriptor_count)
        .map_err(|_| stream_error("sparse descriptor count exceeds usize"))?;
    let mut animation_descriptor_indices = read_u32_vec(
        &prefix,
        &mut cursor,
        descriptor_count_usize,
        "sparse descriptor map",
    )?;
    let mut valid_probe_masks = Vec::new();
    valid_probe_masks
        .try_reserve_exact(cell_count_usize)
        .map_err(|_| stream_error("sparse mask allocation failed"))?;
    for _ in 0..cell_count_usize {
        valid_probe_masks.push(read_u64(&prefix, cursor));
        cursor += 8;
    }
    let cell_levels = prefix[cursor..cursor + cell_count_usize].to_vec();
    cursor += cell_count_usize;
    if cell_levels.iter().any(|&level| level > 2) {
        return Err(stream_error(
            "streamed sparse section has invalid coarsening level",
        ));
    }
    let affinity_offsets = read_u32_vec(
        &prefix,
        &mut cursor,
        cell_count_usize
            .checked_add(1)
            .ok_or_else(|| stream_error("sparse offset count overflows"))?,
        "sparse offsets",
    )?;
    if affinity_offsets.first() != Some(&0)
        || affinity_offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(stream_error(
            "streamed sparse section has invalid CSR offsets",
        ));
    }
    let entry_count = *affinity_offsets
        .last()
        .ok_or_else(|| stream_error("streamed sparse offset table is empty"))?;
    let entries_bytes = u64::from(entry_count)
        .checked_mul(4)
        .ok_or_else(|| stream_error("streamed sparse entry table overflows"))?;
    let metadata_len = before_entries
        .checked_add(entries_bytes)
        .ok_or_else(|| stream_error("streamed sparse metadata end overflows"))?;
    if metadata_len > entry.size {
        return Err(stream_error(
            "streamed sparse entry table exceeds section bounds",
        ));
    }
    let entry_bytes = read_vec_at(
        file,
        entry
            .offset
            .checked_add(before_entries)
            .ok_or_else(|| stream_error("streamed sparse entry offset overflows"))?,
        entries_bytes,
        "streamed sparse light indices",
    )?;
    let mut entry_cursor = 0;
    let entry_count_usize = usize::try_from(entry_count)
        .map_err(|_| stream_error("sparse entry count exceeds usize"))?;
    let affinity_lights = read_u32_vec(
        &entry_bytes,
        &mut entry_cursor,
        entry_count_usize,
        "sparse light indices",
    )?;
    let expected_payload_bytes = sparse_payload_bytes(
        &valid_probe_masks,
        &cell_levels,
        &affinity_offsets,
        tile_dimension,
    )?;
    let expected_section_size = metadata_len
        .checked_add(expected_payload_bytes)
        .ok_or_else(|| stream_error("streamed sparse section size overflows"))?;
    if entry.size != expected_section_size {
        return Err(stream_error(
            "streamed sparse payload length disagrees with metadata",
        ));
    }
    if has_descriptor_map
        && affinity_lights.iter().any(|&index| {
            usize::try_from(index).map_or(true, |index| index >= animation_descriptor_indices.len())
        })
    {
        return Err(stream_error(
            "streamed sparse light index exceeds descriptor map",
        ));
    }
    Ok(Some(ShStreamSparseMetadata {
        section_id: section as u32,
        internal_version: u32::from(expected_version),
        affinity_dims,
        tile_dimension,
        tile_border,
        valid_probe_masks,
        cell_levels,
        affinity_offsets,
        affinity_lights,
        animation_descriptor_indices: std::mem::take(&mut animation_descriptor_indices),
    }))
}
fn sparse_payload_bytes(
    masks: &[u64],
    levels: &[u8],
    offsets: &[u32],
    tile_dimension: u32,
) -> Result<u64, PrlLoadError> {
    if masks.len().checked_add(1) != Some(offsets.len()) || levels.len() != masks.len() {
        return Err(stream_error("sparse metadata table lengths disagree"));
    }
    let tile_texels = u64::from(tile_dimension)
        .checked_mul(u64::from(tile_dimension))
        .and_then(|value| value.checked_mul(3))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| stream_error("sparse tile byte stride overflows"))?;
    let f16_count = masks.iter().zip(levels).zip(offsets.windows(2)).try_fold(
        0u64,
        |total, ((&mask, &level), pair)| {
            let entries = pair[1]
                .checked_sub(pair[0])
                .ok_or_else(|| stream_error("sparse CSR offsets are not monotonic"))?;
            let stored = match level {
                0 => u64::from(mask.count_ones()),
                1 => {
                    const CORNERS: u64 = (1 << 0)
                        | (1 << 3)
                        | (1 << 12)
                        | (1 << 15)
                        | (1 << 48)
                        | (1 << 51)
                        | (1 << 60)
                        | (1 << 63);
                    u64::from((mask & CORNERS).count_ones())
                }
                2 => u64::from((mask != 0) as u8),
                _ => return Err(stream_error("sparse section has invalid coarsening level")),
            };
            total
                .checked_add(
                    u64::from(entries)
                        .checked_mul(stored)
                        .ok_or_else(|| stream_error("sparse tile count overflows"))?,
                )
                .ok_or_else(|| stream_error("sparse total tile count overflows"))
        },
    )?;
    f16_count
        .checked_mul(tile_texels)
        .ok_or_else(|| stream_error("sparse payload byte size overflows"))
}
