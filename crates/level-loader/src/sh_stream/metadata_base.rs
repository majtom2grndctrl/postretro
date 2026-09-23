//! Metadata-only id-34 projection parsing, excluding the compact atlas body.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::fs::File;

use postretro_level_format::SectionEntry;
use postretro_level_format::sh_volume::{
    AnimationDescriptor, OctahedralShProbe, SH_VOLUME_VERSION,
};

use super::manifest::ensure_section_floor;
use super::positional_io::read_vec_at;
use super::projection::ShStreamBaseMetadata;
use super::{PrlLoadError, stream_error};
pub(super) fn read_base_metadata(
    file: &File,
    entry: &SectionEntry,
) -> Result<ShStreamBaseMetadata, PrlLoadError> {
    const HEADER: u64 = 84;
    ensure_section_floor(entry, HEADER, "id-34 header")?;
    let header = read_vec_at(file, entry.offset, HEADER, "id-34 header")?;
    if read_u32(&header, 0) != SH_VOLUME_VERSION {
        return Err(stream_error("id 34 has an unsupported internal version"));
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
    let probe_stride = read_u32(&header, 40);
    let animation_count = read_u32(&header, 44);
    let tile_dimension = read_u32(&header, 48);
    let tile_border = read_u32(&header, 52);
    let atlas_dimensions = [read_u32(&header, 56), read_u32(&header, 60)];
    let atlas_tiles_per_row = read_u32(&header, 64);
    let layer_count = read_u32(&header, 68);
    let tiles_per_layer = read_u32(&header, 72);
    let irradiance_format = read_u32(&header, 76);
    let atlas_len = u64::from(read_u32(&header, 80));
    if probe_stride != 8 {
        return Err(stream_error(
            "id 34 probe stride is not v11's 8-byte record",
        ));
    }
    let probe_count = checked_product(grid_dimensions, "id-34 probe count")?;
    let probe_bytes = probe_count
        .checked_mul(u64::from(probe_stride))
        .ok_or_else(|| stream_error("id-34 probe metadata size overflows"))?;
    let prefix_len = HEADER
        .checked_add(probe_bytes)
        .ok_or_else(|| stream_error("id-34 metadata prefix size overflows"))?;
    ensure_section_floor(entry, prefix_len, "id-34 metadata prefix")?;
    let atlas_start = entry
        .offset
        .checked_add(prefix_len)
        .ok_or_else(|| stream_error("id-34 atlas offset overflows"))?;
    let tail_start = atlas_start
        .checked_add(atlas_len)
        .ok_or_else(|| stream_error("id-34 atlas end overflows"))?;
    let section_end = entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| stream_error("id-34 section end overflows"))?;
    if tail_start > section_end {
        return Err(stream_error(
            "id-34 metadata and declared atlas exceed section bounds",
        ));
    }
    let prefix = read_vec_at(file, entry.offset, prefix_len, "id-34 metadata")?;
    let probe_count = usize::try_from(probe_count)
        .map_err(|_| stream_error("id-34 probe count exceeds usize"))?;
    let mut probes = Vec::new();
    probes
        .try_reserve_exact(probe_count)
        .map_err(|_| stream_error("id-34 probe metadata allocation failed"))?;
    for index in 0..probe_count {
        let offset = 84usize
            .checked_add(
                index
                    .checked_mul(8)
                    .ok_or_else(|| stream_error("id-34 probe offset overflows"))?,
            )
            .ok_or_else(|| stream_error("id-34 probe offset overflows"))?;
        probes.push(OctahedralShProbe {
            validity: prefix[offset],
            mean_distance: read_u16(&prefix, offset + 1),
            mean_sq_distance: read_u16(&prefix, offset + 3),
            density_level: prefix[offset + 5],
            node_scale: prefix[offset + 6],
        });
    }
    postretro_level_format::sh_volume::validate_probe_metadata(grid_dimensions, &probes)
        .map_err(PrlLoadError::FormatError)?;
    postretro_level_format::sh_volume::validate_metadata_projection(
        grid_dimensions,
        probe_stride,
        tile_dimension,
        tile_border,
        atlas_dimensions,
        layer_count,
        tiles_per_layer,
        atlas_tiles_per_row,
        irradiance_format,
        u32::try_from(atlas_len).expect("id-34 header atlas length is u32"),
        &probes,
    )
    .map_err(PrlLoadError::FormatError)?;
    let tail = read_vec_at(
        file,
        tail_start,
        section_end - tail_start,
        "id-34 trailing metadata",
    )?;
    let (animation_descriptors, slot_for_map_light) = parse_animation_tail(&tail, animation_count)?;
    Ok(ShStreamBaseMetadata {
        grid_origin,
        cell_size,
        grid_dimensions,
        probe_stride,
        tile_dimension,
        tile_border,
        atlas_dimensions,
        layer_count,
        tiles_per_layer,
        atlas_tiles_per_row,
        irradiance_format,
        probes,
        animation_descriptors,
        slot_for_map_light,
    })
}

pub(super) fn checked_product(values: [u32; 3], name: &'static str) -> Result<u64, PrlLoadError> {
    values.into_iter().try_fold(1u64, |total, value| {
        total
            .checked_mul(u64::from(value))
            .ok_or_else(|| stream_error(format!("{name} overflows")))
    })
}

pub(super) fn parse_animation_tail(
    tail: &[u8],
    animation_count: u32,
) -> Result<(Vec<AnimationDescriptor>, Vec<u32>), PrlLoadError> {
    let animation_count = usize::try_from(animation_count)
        .map_err(|_| stream_error("id-34 animation count exceeds usize"))?;
    let minimum_tail_len = animation_count
        .checked_mul(36)
        .and_then(|length| length.checked_add(4))
        .ok_or_else(|| stream_error("id-34 animation header table size overflows"))?;
    if minimum_tail_len > tail.len() {
        return Err(stream_error(
            "id-34 animation descriptor headers are truncated",
        ));
    }
    let mut descriptors = Vec::new();
    descriptors
        .try_reserve_exact(animation_count)
        .map_err(|_| stream_error("id-34 animation descriptor allocation failed"))?;
    let mut cursor = 0usize;
    for _ in 0..animation_count {
        let header_end = cursor
            .checked_add(36)
            .ok_or_else(|| stream_error("id-34 animation descriptor header overflows"))?;
        if header_end > tail.len() {
            return Err(stream_error(
                "id-34 animation descriptor header is truncated",
            ));
        }
        let period = read_f32(tail, cursor);
        let phase = read_f32(tail, cursor + 4);
        let base_color = [
            read_f32(tail, cursor + 8),
            read_f32(tail, cursor + 12),
            read_f32(tail, cursor + 16),
        ];
        let brightness_count = usize::try_from(read_u32(tail, cursor + 20))
            .map_err(|_| stream_error("id-34 brightness count exceeds usize"))?;
        let color_count = usize::try_from(read_u32(tail, cursor + 24))
            .map_err(|_| stream_error("id-34 color count exceeds usize"))?;
        let start_active = read_u32(tail, cursor + 28);
        let direction_count = usize::try_from(read_u32(tail, cursor + 32))
            .map_err(|_| stream_error("id-34 direction count exceeds usize"))?;
        cursor = header_end;
        let brightness_bytes = brightness_count
            .checked_mul(4)
            .ok_or_else(|| stream_error("id-34 brightness sample size overflows"))?;
        let color_bytes = color_count
            .checked_mul(12)
            .ok_or_else(|| stream_error("id-34 color sample size overflows"))?;
        let direction_bytes = direction_count
            .checked_mul(12)
            .ok_or_else(|| stream_error("id-34 direction sample size overflows"))?;
        let samples_end = cursor
            .checked_add(brightness_bytes)
            .and_then(|end| end.checked_add(color_bytes))
            .and_then(|end| end.checked_add(direction_bytes))
            .ok_or_else(|| stream_error("id-34 animation sample range overflows"))?;
        if samples_end > tail.len() {
            return Err(stream_error(
                "id-34 animation descriptor samples are truncated",
            ));
        }
        let mut brightness = Vec::new();
        brightness
            .try_reserve_exact(brightness_count)
            .map_err(|_| stream_error("id-34 brightness allocation failed"))?;
        for index in 0..brightness_count {
            brightness.push(read_f32(tail, cursor + index * 4));
        }
        cursor += brightness_bytes;
        let mut color = Vec::new();
        color
            .try_reserve_exact(color_count)
            .map_err(|_| stream_error("id-34 color allocation failed"))?;
        for index in 0..color_count {
            let offset = cursor + index * 12;
            color.push([
                read_f32(tail, offset),
                read_f32(tail, offset + 4),
                read_f32(tail, offset + 8),
            ]);
        }
        cursor += color_bytes;
        let mut direction = Vec::new();
        direction
            .try_reserve_exact(direction_count)
            .map_err(|_| stream_error("id-34 direction allocation failed"))?;
        for index in 0..direction_count {
            let offset = cursor + index * 12;
            direction.push([
                read_f32(tail, offset),
                read_f32(tail, offset + 4),
                read_f32(tail, offset + 8),
            ]);
        }
        cursor += direction_bytes;
        descriptors.push(AnimationDescriptor {
            period,
            phase,
            base_color,
            brightness,
            color,
            direction,
            start_active,
        });
    }
    let slot_count_end = cursor
        .checked_add(4)
        .ok_or_else(|| stream_error("id-34 slot table count offset overflows"))?;
    if slot_count_end > tail.len() {
        return Err(stream_error("id-34 map-light slot table is truncated"));
    }
    let slot_count = usize::try_from(read_u32(tail, cursor))
        .map_err(|_| stream_error("id-34 map-light slot count exceeds usize"))?;
    cursor = slot_count_end;
    let slots_bytes = slot_count
        .checked_mul(4)
        .ok_or_else(|| stream_error("id-34 map-light slot table size overflows"))?;
    let slots_end = cursor
        .checked_add(slots_bytes)
        .ok_or_else(|| stream_error("id-34 map-light slot table range overflows"))?;
    if slots_end != tail.len() {
        return Err(stream_error(
            "id-34 trailing bytes do not exactly match the map-light slot table",
        ));
    }
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(slot_count)
        .map_err(|_| stream_error("id-34 map-light slot allocation failed"))?;
    for index in 0..slot_count {
        slots.push(read_u32(tail, cursor + index * 4));
    }
    Ok((descriptors, slots))
}

pub(super) fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

pub(super) fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub(super) fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

pub(super) fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub(super) fn read_u32_vec(
    bytes: &[u8],
    cursor: &mut usize,
    count: usize,
    what: &'static str,
) -> Result<Vec<u32>, PrlLoadError> {
    let byte_len = count
        .checked_mul(4)
        .ok_or_else(|| stream_error(format!("{what} byte length overflows")))?;
    let end = cursor
        .checked_add(byte_len)
        .ok_or_else(|| stream_error(format!("{what} range overflows")))?;
    if end > bytes.len() {
        return Err(stream_error(format!("{what} is truncated")));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| stream_error(format!("{what} allocation failed")))?;
    for index in 0..count {
        values.push(read_u32(bytes, *cursor + index * 4));
    }
    *cursor = end;
    Ok(values)
}
