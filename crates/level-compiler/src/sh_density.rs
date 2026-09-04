// Stored-set packaging for the base indirect and direct SH volumes.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::octahedral::{
    IrradianceAtlasArrayLayout, irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_reconstruct::{
    Level, StoredTile, corner_locals, reconstruct_l2_tile, stored_brick_prefix_sum, stored_tile_set,
};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};

use crate::sh_bake::MAX_SH_ATLAS_DIMENSION;

type PackedTile = Vec<OctahedralAtlasTexel>;

/// The one level assigned to each affinity brick before stored-set packing.
///
/// This interim task intentionally selects uniform L0 unless the measurement
/// flag requests another representable level. Task 6 replaces that selection
/// with the classifier plus delta ceilings and seam smoothing, while reusing the
/// packers below.
pub(crate) fn force_levels(
    grid_dimensions: [u32; 3],
    probe_validity: &[bool],
    forced_level: Option<Level>,
) -> Result<Vec<Level>, String> {
    let probe_count = checked_probe_count(grid_dimensions)?;
    if probe_validity.len() != probe_count {
        return Err(format!(
            "SH density force level received {} validity entries for grid {grid_dimensions:?}, expected {probe_count}",
            probe_validity.len()
        ));
    }

    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let brick_count = checked_probe_count(affinity_dimensions)?;
    let mut levels = vec![Level::L0; brick_count];
    let Some(forced_level) = forced_level else {
        return Ok(levels);
    };

    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let brick = brick_index(brick_x, brick_y, brick_z, affinity_dimensions);
                if brick_is_partial(brick_x, brick_y, brick_z, grid_dimensions) {
                    continue;
                }
                if forced_level == Level::L1
                    && !brick_has_valid_corner(
                        brick_x,
                        brick_y,
                        brick_z,
                        grid_dimensions,
                        probe_validity,
                    )
                {
                    continue;
                }
                levels[brick] = forced_level;
            }
        }
    }
    Ok(levels)
}

/// Repack the indirect bake's legacy valid-probe-order lossless intermediate
/// into the v10 brick-major stored set. The grouped bake cache remains upstream:
/// this is deliberately invoked only after cold and warm assembly converge.
pub(crate) fn pack_indirect_section(
    mut section: OctahedralShVolumeSection,
    forced_level: Option<Level>,
) -> Result<(OctahedralShVolumeSection, DensityPackStats), String> {
    require_rgba16f(section.irradiance_format, "indirect")?;
    let validity: Vec<bool> = section
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let levels = force_levels(section.grid_dimensions, &validity, forced_level)?;
    let source_tiles = decode_indirect_intermediate_tiles(&section, &validity)?;
    stamp_levels(&mut section, &levels)?;
    let (tiles, prefix) = stored_tiles(section.grid_dimensions, &validity, &levels, &source_tiles)?;
    let layout = stored_layout(prefix.total_stored_tiles, section.tile_dimension)?;
    section.atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    section.layer_count = layout.layer_count;
    section.tiles_per_layer = layout.tiles_per_layer;
    section.atlas_tiles_per_row = layout.atlas_tiles_per_row;
    section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    section.compact_atlas = pack_tiles_into_atlas(&tiles, layout, section.tile_dimension);

    Ok((
        section,
        DensityPackStats::from_levels(&levels, prefix.total_stored_tiles),
    ))
}

/// Repack direct SH from its dense cacheable intermediate using the exact stored
/// slots and levels already emitted by id 34. Direct has no validity metadata of
/// its own, so id 34 remains the sole membership source.
pub(crate) fn pack_direct_section(
    mut section: DirectShVolumeSection,
    base: &OctahedralShVolumeSection,
) -> Result<DirectShVolumeSection, String> {
    require_rgba16f(section.irradiance_format, "direct")?;
    if section.grid_dimensions != base.grid_dimensions
        || section.tile_dimension != base.tile_dimension
        || section.tile_border != base.tile_border
    {
        return Err(format!(
            "direct SH dense intermediate does not match id 34 grid/tile geometry: direct grid {:?}, tile {}/{}; base grid {:?}, tile {}/{}",
            section.grid_dimensions,
            section.tile_dimension,
            section.tile_border,
            base.grid_dimensions,
            base.tile_dimension,
            base.tile_border,
        ));
    }

    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let levels = levels_from_base(base)?;
    let source_tiles = decode_dense_tiles(&section, &validity)?;
    let (tiles, prefix) = stored_tiles(section.grid_dimensions, &validity, &levels, &source_tiles)?;
    let layout = stored_layout(prefix.total_stored_tiles, section.tile_dimension)?;
    section.atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    section.layer_count = layout.layer_count;
    section.tiles_per_layer = layout.tiles_per_layer;
    section.atlas_tiles_per_row = layout.atlas_tiles_per_row;
    section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
    section.atlas = pack_tiles_into_atlas(&tiles, layout, section.tile_dimension);
    Ok(section)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DensityPackStats {
    pub(crate) brick_levels: [u32; 3],
    pub(crate) stored_tiles: u32,
}

impl DensityPackStats {
    fn from_levels(levels: &[Level], stored_tiles: u32) -> Self {
        let mut brick_levels = [0; 3];
        for level in levels {
            brick_levels[level.to_u8() as usize] += 1;
        }
        Self {
            brick_levels,
            stored_tiles,
        }
    }
}

fn require_rgba16f(format: u32, label: &str) -> Result<(), String> {
    if format == IRRADIANCE_FORMAT_RGBA16F {
        Ok(())
    } else {
        Err(format!(
            "cannot density-pack {label} SH after its at-rest encoding (format tag {format})"
        ))
    }
}

fn checked_probe_count(dimensions: [u32; 3]) -> Result<usize, String> {
    dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })
        .ok_or_else(|| format!("SH density grid dimensions {dimensions:?} overflow usize"))
}

fn brick_index(x: usize, y: usize, z: usize, affinity_dimensions: [u32; 3]) -> usize {
    x + y * affinity_dimensions[0] as usize
        + z * affinity_dimensions[0] as usize * affinity_dimensions[1] as usize
}

fn brick_is_partial(x: usize, y: usize, z: usize, dimensions: [u32; 3]) -> bool {
    (x + 1) * 4 > dimensions[0] as usize
        || (y + 1) * 4 > dimensions[1] as usize
        || (z + 1) * 4 > dimensions[2] as usize
}

fn probe_index(x: usize, y: usize, z: usize, dimensions: [u32; 3]) -> usize {
    x + y * dimensions[0] as usize + z * dimensions[0] as usize * dimensions[1] as usize
}

fn brick_has_valid_corner(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    validity: &[bool],
) -> bool {
    corner_locals().into_iter().any(|local| {
        let local_x = local % 4;
        let local_y = (local / 4) % 4;
        let local_z = local / 16;
        let index = probe_index(
            brick_x * 4 + local_x,
            brick_y * 4 + local_y,
            brick_z * 4 + local_z,
            dimensions,
        );
        validity[index]
    })
}

fn stamp_levels(section: &mut OctahedralShVolumeSection, levels: &[Level]) -> Result<(), String> {
    let affinity_dimensions = section
        .grid_dimensions
        .map(|dimension| dimension.div_ceil(4));
    if levels.len() != checked_probe_count(affinity_dimensions)? {
        return Err("SH density level count does not match the affinity grid".to_string());
    }
    for z in 0..section.grid_dimensions[2] as usize {
        for y in 0..section.grid_dimensions[1] as usize {
            for x in 0..section.grid_dimensions[0] as usize {
                let brick = brick_index(x / 4, y / 4, z / 4, affinity_dimensions);
                section.probes[probe_index(x, y, z, section.grid_dimensions)].density_level =
                    levels[brick].to_u8();
            }
        }
    }
    Ok(())
}

fn levels_from_base(base: &OctahedralShVolumeSection) -> Result<Vec<Level>, String> {
    let affinity_dimensions = base.grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let mut levels = Vec::with_capacity(checked_probe_count(affinity_dimensions)?);
    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let index =
                    probe_index(brick_x * 4, brick_y * 4, brick_z * 4, base.grid_dimensions);
                let level = Level::from_u8(base.probes[index].density_level).ok_or_else(|| {
                    format!(
                        "id 34 base metadata has invalid density level {} in brick {brick_x},{brick_y},{brick_z}",
                        base.probes[index].density_level
                    )
                })?;
                levels.push(level);
            }
        }
    }
    Ok(levels)
}

fn decode_indirect_intermediate_tiles(
    section: &OctahedralShVolumeSection,
    validity: &[bool],
) -> Result<Vec<Option<PackedTile>>, String> {
    let mut tiles = vec![None; section.probes.len()];
    let mut valid_rank = 0usize;
    for (probe, &is_valid) in validity.iter().enumerate() {
        if !is_valid {
            continue;
        }
        tiles[probe] = Some(read_tile(
            &section.compact_atlas,
            section.atlas_dimensions,
            section.layer_count,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
            section.tile_dimension,
            valid_rank,
        )?);
        valid_rank += 1;
    }
    Ok(tiles)
}

fn decode_dense_tiles(
    section: &DirectShVolumeSection,
    validity: &[bool],
) -> Result<Vec<Option<PackedTile>>, String> {
    let mut tiles = vec![None; validity.len()];
    for (probe, &is_valid) in validity.iter().enumerate() {
        if is_valid {
            tiles[probe] = Some(read_tile(
                &section.atlas,
                section.atlas_dimensions,
                section.layer_count,
                section.tiles_per_layer,
                section.atlas_tiles_per_row,
                section.tile_dimension,
                probe,
            )?);
        }
    }
    Ok(tiles)
}

fn read_tile(
    bytes: &[u8],
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    atlas_tiles_per_row: u32,
    tile_dimension: u32,
    slot: usize,
) -> Result<PackedTile, String> {
    let width = atlas_dimensions[0] as usize;
    let height = atlas_dimensions[1] as usize;
    let layer_texels = width
        .checked_mul(height)
        .ok_or_else(|| "SH density atlas dimensions overflow".to_string())?;
    let expected_len = layer_texels
        .checked_mul(layer_count as usize)
        .and_then(|texels| texels.checked_mul(8))
        .ok_or_else(|| "SH density atlas byte length overflows".to_string())?;
    if bytes.len() != expected_len {
        return Err(format!(
            "SH density source atlas has {} bytes, expected {expected_len} from its declared geometry",
            bytes.len()
        ));
    }
    let [layer, tile_x, tile_y] =
        irradiance_array_tile_location(slot, tiles_per_layer, atlas_tiles_per_row);
    if layer >= layer_count {
        return Err(format!(
            "SH density source slot {slot} is outside its atlas"
        ));
    }
    let mut tile = Vec::with_capacity((tile_dimension * tile_dimension) as usize);
    for y in 0..tile_dimension as usize {
        for x in 0..tile_dimension as usize {
            let texel = layer as usize * layer_texels
                + (tile_y as usize * tile_dimension as usize + y) * width
                + tile_x as usize * tile_dimension as usize
                + x;
            let byte = texel * 8;
            tile.push(OctahedralAtlasTexel {
                rgba: [
                    u16::from_le_bytes([bytes[byte], bytes[byte + 1]]),
                    u16::from_le_bytes([bytes[byte + 2], bytes[byte + 3]]),
                    u16::from_le_bytes([bytes[byte + 4], bytes[byte + 5]]),
                    u16::from_le_bytes([bytes[byte + 6], bytes[byte + 7]]),
                ],
            });
        }
    }
    Ok(tile)
}

fn stored_tiles(
    dimensions: [u32; 3],
    validity: &[bool],
    levels: &[Level],
    source_tiles: &[Option<PackedTile>],
) -> Result<
    (
        Vec<PackedTile>,
        postretro_level_format::sh_reconstruct::StoredBrickPrefixSum,
    ),
    String,
> {
    let prefix = stored_brick_prefix_sum(dimensions, levels, validity).ok_or_else(|| {
        "SH density stored-set prefix sum rejected its metadata shape".to_string()
    })?;
    if source_tiles.len() != validity.len() {
        return Err("SH density source tile count does not match validity metadata".to_string());
    }
    let tile_texels = source_tiles
        .iter()
        .flatten()
        .next()
        .map(Vec::len)
        .unwrap_or_else(|| (6 * 6) as usize);
    let mut output = Vec::with_capacity(prefix.total_stored_tiles as usize);
    for brick_z in 0..prefix.affinity_dimensions[2] as usize {
        for brick_y in 0..prefix.affinity_dimensions[1] as usize {
            for brick_x in 0..prefix.affinity_dimensions[0] as usize {
                let brick = brick_index(brick_x, brick_y, brick_z, prefix.affinity_dimensions);
                let (mask, brick_tiles) = brick_tiles(
                    brick_x,
                    brick_y,
                    brick_z,
                    dimensions,
                    validity,
                    source_tiles,
                );
                for stored in stored_tile_set(levels[brick], mask) {
                    match stored {
                        StoredTile::Probe(local) => {
                            output.push(brick_tiles[local].clone().unwrap_or_else(|| {
                                vec![OctahedralAtlasTexel::default(); tile_texels]
                            }));
                        }
                        StoredTile::BrickMean => {
                            output.push(l2_mean_tile(&brick_tiles, tile_texels)?)
                        }
                    }
                }
            }
        }
    }
    if output.len() != prefix.total_stored_tiles as usize {
        return Err("SH density packing disagreed with the stored-set prefix sum".to_string());
    }
    Ok((output, prefix))
}

fn brick_tiles(
    brick_x: usize,
    brick_y: usize,
    brick_z: usize,
    dimensions: [u32; 3],
    validity: &[bool],
    source_tiles: &[Option<PackedTile>],
) -> (u64, [Option<PackedTile>; 64]) {
    let mut mask = 0u64;
    let tiles = std::array::from_fn(|local| {
        let local_x = local % 4;
        let local_y = (local / 4) % 4;
        let local_z = local / 16;
        let x = brick_x * 4 + local_x;
        let y = brick_y * 4 + local_y;
        let z = brick_z * 4 + local_z;
        if x >= dimensions[0] as usize || y >= dimensions[1] as usize || z >= dimensions[2] as usize
        {
            return None;
        }
        let probe = probe_index(x, y, z, dimensions);
        if validity[probe] {
            mask |= 1u64 << local;
            source_tiles[probe].clone()
        } else {
            None
        }
    });
    (mask, tiles)
}

fn l2_mean_tile(
    tiles: &[Option<PackedTile>; 64],
    tile_texels: usize,
) -> Result<PackedTile, String> {
    let rgb_tiles = std::array::from_fn(|local| {
        tiles[local].as_ref().map(|tile| {
            tile.iter()
                .map(|texel| {
                    glam::Vec3::new(
                        f16_bits_to_f32(texel.rgba[0]),
                        f16_bits_to_f32(texel.rgba[1]),
                        f16_bits_to_f32(texel.rgba[2]),
                    )
                })
                .collect()
        })
    });
    let mean = reconstruct_l2_tile(&rgb_tiles, tile_texels)
        .ok_or_else(|| "L2 stored tile requested for an all-invalid brick".to_string())?;
    Ok(mean
        .into_iter()
        .map(|rgb| OctahedralAtlasTexel {
            rgba: [
                f32_to_f16_bits(rgb.x),
                f32_to_f16_bits(rgb.y),
                f32_to_f16_bits(rgb.z),
                f32_to_f16_bits(1.0),
            ],
        })
        .collect())
}

fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x3ff;
    let value = if exp == 0 {
        mantissa as f32 * 2.0f32.powi(-24)
    } else if exp == 0x1f {
        if mantissa == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + mantissa as f32 / 1024.0) * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -value } else { value }
}

fn stored_layout(
    tile_count: u32,
    tile_dimension: u32,
) -> Result<IrradianceAtlasArrayLayout, String> {
    irradiance_atlas_array_layout([tile_count, 1, 1], tile_dimension, MAX_SH_ATLAS_DIMENSION)
        .ok_or_else(|| format!("SH density stored atlas cannot fit {tile_count} tile(s)"))
}

fn pack_tiles_into_atlas(
    tiles: &[PackedTile],
    layout: IrradianceAtlasArrayLayout,
    tile_dimension: u32,
) -> Vec<u8> {
    let width = layout.atlas_width as usize;
    let layer_texels = width * layout.atlas_height as usize;
    let mut atlas =
        vec![OctahedralAtlasTexel::default(); layer_texels * layout.layer_count as usize];
    for (slot, tile) in tiles.iter().enumerate() {
        let [layer, tile_x, tile_y] = irradiance_array_tile_location(
            slot,
            layout.tiles_per_layer,
            layout.atlas_tiles_per_row,
        );
        for y in 0..tile_dimension as usize {
            for x in 0..tile_dimension as usize {
                let destination = layer as usize * layer_texels
                    + (tile_y as usize * tile_dimension as usize + y) * width
                    + tile_x as usize * tile_dimension as usize
                    + x;
                atlas[destination] = tile[y * tile_dimension as usize + x];
            }
        }
    }
    let mut bytes = Vec::with_capacity(atlas.len() * 8);
    for texel in atlas {
        for channel in texel.rgba {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, irradiance_atlas_array_layout,
    };
    use postretro_level_format::sh_volume::{OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe};

    const TILE_DIMENSION: u32 = 6;

    fn raw_indirect(
        dimensions: [u32; 3],
        valid: impl Fn(usize) -> bool,
    ) -> OctahedralShVolumeSection {
        let total = checked_probe_count(dimensions).unwrap();
        let probes: Vec<_> = (0..total)
            .map(|index| OctahedralShProbe {
                validity: u8::from(valid(index)),
                ..Default::default()
            })
            .collect();
        let valid_count = probes.iter().filter(|probe| probe.validity != 0).count() as u32;
        let layout =
            irradiance_atlas_array_layout([valid_count, 1, 1], TILE_DIMENSION, 8192).unwrap();
        let mut atlas = vec![
            OctahedralAtlasTexel::default();
            layout.layer_count as usize
                * layout.atlas_width as usize
                * layout.atlas_height as usize
        ];
        let mut rank = 0usize;
        for (probe, metadata) in probes.iter().enumerate() {
            if metadata.validity == 0 {
                continue;
            }
            let value = f32_to_f16_bits(probe as f32);
            let [layer, tx, ty] = irradiance_array_tile_location(
                rank,
                layout.tiles_per_layer,
                layout.atlas_tiles_per_row,
            );
            for y in 0..TILE_DIMENSION as usize {
                for x in 0..TILE_DIMENSION as usize {
                    let destination = layer as usize
                        * layout.atlas_width as usize
                        * layout.atlas_height as usize
                        + (ty as usize * TILE_DIMENSION as usize + y) * layout.atlas_width as usize
                        + tx as usize * TILE_DIMENSION as usize
                        + x;
                    atlas[destination] = OctahedralAtlasTexel {
                        rgba: [value, value, value, f32_to_f16_bits(1.0)],
                    };
                }
            }
            rank += 1;
        }
        let mut bytes = Vec::new();
        for texel in atlas {
            for channel in texel.rgba {
                bytes.extend_from_slice(&channel.to_le_bytes());
            }
        }
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: dimensions,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas: bytes,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn slot_value(section: &OctahedralShVolumeSection, slot: usize) -> f32 {
        let [layer, tx, ty] = irradiance_array_tile_location(
            slot,
            section.tiles_per_layer,
            section.atlas_tiles_per_row,
        );
        let texel = layer as usize
            * section.atlas_dimensions[0] as usize
            * section.atlas_dimensions[1] as usize
            + ty as usize * TILE_DIMENSION as usize * section.atlas_dimensions[0] as usize
            + tx as usize * TILE_DIMENSION as usize;
        let byte = texel * 8;
        f16_bits_to_f32(u16::from_le_bytes([
            section.compact_atlas[byte],
            section.compact_atlas[byte + 1],
        ]))
    }

    #[test]
    fn stored_set_packing_matches_each_level_on_a_constructed_brick() {
        for (level, expected_slots) in [(Level::L0, 64), (Level::L1, 8), (Level::L2, 1)] {
            let raw = raw_indirect([4, 4, 4], |_| true);
            let (packed, stats) = pack_indirect_section(raw, Some(level)).unwrap();
            assert_eq!(stats.stored_tiles, expected_slots);
            assert_eq!(
                packed
                    .probes
                    .iter()
                    .map(|probe| probe.density_level)
                    .collect::<Vec<_>>(),
                vec![level.to_u8(); 64]
            );
            for (slot, stored) in stored_tile_set(level, u64::MAX).into_iter().enumerate() {
                let expected = match stored {
                    StoredTile::Probe(local) => local as f32,
                    StoredTile::BrickMean => 31.5,
                };
                assert_eq!(slot_value(&packed, slot), expected);
            }
        }
    }

    #[test]
    fn l0_packing_is_brick_major_across_adjacent_bricks() {
        let raw = raw_indirect([8, 4, 4], |_| true);
        let (packed, stats) = pack_indirect_section(raw, None).unwrap();

        assert_eq!(stats.stored_tiles, 128);
        assert_eq!(slot_value(&packed, 0), 0.0);
        assert_eq!(slot_value(&packed, 63), 123.0);
        assert_eq!(slot_value(&packed, 64), 4.0);
        assert_eq!(slot_value(&packed, 127), 127.0);
    }

    #[test]
    fn l2_packing_synthesizes_the_mean_over_valid_tiles() {
        let raw = raw_indirect([4, 4, 4], |probe| probe != 0);
        let (packed, _) = pack_indirect_section(raw, Some(Level::L2)).unwrap();
        assert!((slot_value(&packed, 0) - 32.0).abs() < 0.01);
    }

    #[test]
    fn l1_packing_reserves_an_invalid_corner_as_a_zero_tile() {
        let raw = raw_indirect([4, 4, 4], |probe| probe != 0);
        let (packed, stats) = pack_indirect_section(raw, Some(Level::L1)).unwrap();
        assert_eq!(stats.stored_tiles, 8);
        assert_eq!(slot_value(&packed, 0), 0.0);
        assert_eq!(slot_value(&packed, 1), 3.0);
    }

    #[test]
    fn forced_level_keeps_partial_edge_bricks_at_l0() {
        let validity = vec![true; 5 * 4 * 4];
        let levels = force_levels([5, 4, 4], &validity, Some(Level::L2)).unwrap();
        assert_eq!(levels, vec![Level::L2, Level::L0]);
    }
}
