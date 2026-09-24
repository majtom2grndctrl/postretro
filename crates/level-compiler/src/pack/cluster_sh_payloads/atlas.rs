//! Isolated 8×8 dense-cell gathering and encoding for id-50 chunks.
//! See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_LOGICAL_TILE_DIMENSION, cluster_sh_isolated_atlas_array_layout,
};
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::octahedral::{
    IrradianceAtlasArrayLayout, irradiance_array_tile_location,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::{BLOCK_KIND_ISOLATED_ATLAS, DenseNode, EncodedBlock, push_u32};

#[derive(Clone, Copy)]
pub(super) struct DenseAtlasGeometry {
    dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    tiles_per_row: u32,
    tile_dimension: u32,
}

impl DenseAtlasGeometry {
    pub(super) fn from_octahedral(section: &OctahedralShVolumeSection) -> Self {
        Self {
            dimensions: section.atlas_dimensions,
            layer_count: section.layer_count,
            tiles_per_layer: section.tiles_per_layer,
            tiles_per_row: section.atlas_tiles_per_row,
            tile_dimension: section.tile_dimension,
        }
    }

    pub(super) fn from_direct(section: &DirectShVolumeSection) -> Self {
        Self {
            dimensions: section.atlas_dimensions,
            layer_count: section.layer_count,
            tiles_per_layer: section.tiles_per_layer,
            tiles_per_row: section.atlas_tiles_per_row,
            tile_dimension: section.tile_dimension,
        }
    }
}

impl EncodedBlock {
    pub(super) fn isolated_atlas(
        section_id: u32,
        format: u32,
        slot_count: u32,
        source_atlas: &[u8],
        source_geometry: DenseAtlasGeometry,
        nodes: &[DenseNode],
    ) -> anyhow::Result<Self> {
        let layout = cluster_sh_isolated_atlas_array_layout(slot_count)
            .ok_or_else(|| anyhow::anyhow!("id-50 isolated atlas layout exceeds format cap"))?;
        let rgba = isolated_rgba16f_atlas(source_atlas, source_geometry, nodes, &layout)?;
        let payload = encode_isolated_atlas_payload(&rgba, &layout, format)?;
        let mut body = Vec::with_capacity(20 + payload.len());
        push_u32(&mut body, format);
        push_u32(&mut body, slot_count);
        push_u32(&mut body, layout.atlas_width);
        push_u32(&mut body, layout.atlas_height);
        push_u32(&mut body, layout.layer_count);
        body.extend_from_slice(&payload);
        Ok(Self {
            section_id,
            kind: BLOCK_KIND_ISOLATED_ATLAS,
            element_count: slot_count,
            body,
        })
    }
}

fn isolated_rgba16f_atlas(
    source: &[u8],
    source_geometry: DenseAtlasGeometry,
    nodes: &[DenseNode],
    layout: &IrradianceAtlasArrayLayout,
) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        source_geometry.tile_dimension == CLUSTER_SH_LOGICAL_TILE_DIMENSION,
        "id-50 source tile dimension is not the logical six-texel tile"
    );
    let total_bytes = usize::try_from(
        u64::from(layout.layer_count)
            .checked_mul(u64::from(layout.atlas_width))
            .and_then(|value| value.checked_mul(u64::from(layout.atlas_height)))
            .and_then(|value| value.checked_mul(8))
            .ok_or_else(|| anyhow::anyhow!("id-50 isolated RGBA atlas length overflow"))?,
    )?;
    let mut result = vec![0; total_bytes];
    for node in nodes {
        for node_offset in 0..node.stored_tile_count {
            let global_slot = node
                .global_base
                .checked_add(node_offset)
                .ok_or_else(|| anyhow::anyhow!("id-50 global stored slot overflow"))?;
            let local_slot = node
                .local_base
                .checked_add(node_offset)
                .ok_or_else(|| anyhow::anyhow!("id-50 local stored slot overflow"))?;
            copy_dilated_tile(
                source,
                source_geometry,
                global_slot,
                &mut result,
                layout,
                local_slot,
            )?;
        }
    }
    Ok(result)
}

fn copy_dilated_tile(
    source: &[u8],
    source_geometry: DenseAtlasGeometry,
    source_slot: u32,
    destination: &mut [u8],
    destination_layout: &IrradianceAtlasArrayLayout,
    destination_slot: u32,
) -> anyhow::Result<()> {
    let [source_layer, source_x, source_y] = irradiance_array_tile_location(
        usize::try_from(source_slot)?,
        source_geometry.tiles_per_layer,
        source_geometry.tiles_per_row,
    );
    anyhow::ensure!(
        source_layer < source_geometry.layer_count,
        "id-50 source slot exceeds atlas layers"
    );
    let [destination_layer, destination_x, destination_y] = irradiance_array_tile_location(
        usize::try_from(destination_slot)?,
        destination_layout.tiles_per_layer,
        destination_layout.atlas_tiles_per_row,
    );
    anyhow::ensure!(
        destination_layer < destination_layout.layer_count,
        "id-50 destination slot exceeds isolated atlas layers"
    );
    let source_width = usize::try_from(source_geometry.dimensions[0])?;
    let source_height = usize::try_from(source_geometry.dimensions[1])?;
    let source_layer_bytes = source_width
        .checked_mul(source_height)
        .and_then(|texels| texels.checked_mul(8))
        .ok_or_else(|| anyhow::anyhow!("id-50 source atlas layer bytes overflow"))?;
    let destination_width = usize::try_from(destination_layout.atlas_width)?;
    let destination_height = usize::try_from(destination_layout.atlas_height)?;
    let destination_layer_bytes = destination_width
        .checked_mul(destination_height)
        .and_then(|texels| texels.checked_mul(8))
        .ok_or_else(|| anyhow::anyhow!("id-50 destination atlas layer bytes overflow"))?;
    for y in 0..8usize {
        for x in 0..8usize {
            let source_texel_x = usize::try_from(source_x)? * 6 + x.min(5);
            let source_texel_y = usize::try_from(source_y)? * 6 + y.min(5);
            let destination_texel_x = usize::try_from(destination_x)? * 8 + x;
            let destination_texel_y = usize::try_from(destination_y)? * 8 + y;
            let source_offset = usize::try_from(source_layer)?
                .checked_mul(source_layer_bytes)
                .and_then(|base| {
                    base.checked_add((source_texel_y * source_width + source_texel_x) * 8)
                })
                .ok_or_else(|| anyhow::anyhow!("id-50 source tile offset overflow"))?;
            let destination_offset = usize::try_from(destination_layer)?
                .checked_mul(destination_layer_bytes)
                .and_then(|base| {
                    base.checked_add(
                        (destination_texel_y * destination_width + destination_texel_x) * 8,
                    )
                })
                .ok_or_else(|| anyhow::anyhow!("id-50 destination tile offset overflow"))?;
            let source_texel = source
                .get(source_offset..source_offset + 8)
                .ok_or_else(|| anyhow::anyhow!("id-50 source tile exceeds packed atlas"))?;
            let destination_texel = destination
                .get_mut(destination_offset..destination_offset + 8)
                .ok_or_else(|| anyhow::anyhow!("id-50 destination tile exceeds isolated atlas"))?;
            destination_texel.copy_from_slice(source_texel);
        }
    }
    Ok(())
}

fn encode_isolated_atlas_payload(
    rgba: &[u8],
    layout: &IrradianceAtlasArrayLayout,
    format: u32,
) -> anyhow::Result<Vec<u8>> {
    match format {
        IRRADIANCE_FORMAT_RGBA16F => Ok(rgba.to_vec()),
        IRRADIANCE_FORMAT_BC6H => {
            let layer_bytes = usize::try_from(
                u64::from(layout.atlas_width)
                    .checked_mul(u64::from(layout.atlas_height))
                    .and_then(|value| value.checked_mul(8))
                    .ok_or_else(|| anyhow::anyhow!("id-50 BC6H layer size overflow"))?,
            )?;
            let mut encoded = Vec::new();
            for layer in rgba.chunks_exact(layer_bytes) {
                let mut f32_rgba = Vec::with_capacity(layer.len() / 2);
                for channels in layer.chunks_exact(8) {
                    for channel in channels.chunks_exact(2) {
                        f32_rgba.push(crate::sh_bake::f16_bits_to_f32(u16::from_le_bytes([
                            channel[0], channel[1],
                        ])));
                    }
                }
                encoded.extend_from_slice(&crate::bc6h::encode_bc6h_rgb_from_f32_rgba(
                    &f32_rgba,
                    layout.atlas_width,
                    layout.atlas_height,
                ));
            }
            anyhow::ensure!(
                rgba.len() == usize::try_from(layout.layer_count)? * layer_bytes,
                "id-50 RGBA isolated atlas has an incomplete layer"
            );
            Ok(encoded)
        }
        _ => anyhow::bail!("id-50 dense source uses unknown irradiance format {format}"),
    }
}
