// Load-time proofs for the id-22/42 cell-block format: the reject list, the
// zero- and one-block levels, and the payload split.
// See: context/lib/testing_guide.md

use super::*;
use crate::prl_load_test_fixtures::{
    geometry_blob, write_prl_load_fixture, write_prl_load_fixture_with_geometry,
};
use crate::{LevelWorld, load_prl};
use postretro_level_format::SectionBlob;
use postretro_level_format::geometry::{GeometrySection, Vertex};
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_TEXEL_BYTES,
    LIGHTMAP_BLOCK_RECORD_BYTES, LIGHTMAP_HEADER_BYTES, LIGHTMAP_POOL_LAYER_EDGE, LightmapBlock,
    LightmapMode, LightmapSection,
};
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, group_plane_len};
use postretro_test_log_capture::LogCapture;

/// Id-22 cell blocks at these extents (multiples of 4), one per cell, with
/// distinct bytes per block.
fn lightmap_section(extents: &[(u16, u16)]) -> LightmapSection {
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: LightmapMode::Shadowed,
        blocks: extents
            .iter()
            .enumerate()
            .map(|(cell, &(width, height))| {
                let texels = usize::from(width) * usize::from(height);
                LightmapBlock {
                    cell_id: cell as u32,
                    width,
                    height,
                    irradiance: vec![cell as u8 + 1; texels * IRRADIANCE_TEXEL_BYTES],
                    direction: vec![200 - cell as u8; texels / 4 * DIRECTION_TEXEL_BYTES],
                }
            })
            .collect(),
    }
}

/// Id-42 groups pairing [`lightmap_section`] of the same extents.
fn shadowmask_section(extents: &[(u16, u16)]) -> ShadowmaskAtlasSection {
    ShadowmaskAtlasSection {
        channels: vec![0],
        blocks: extents
            .iter()
            .enumerate()
            .map(|(block, &(width, height))| {
                let len = group_plane_len(u32::from(width), u32::from(height)).unwrap() as usize;
                [vec![block as u8; len], vec![block as u8 + 100; len]]
            })
            .collect(),
    }
}

fn blob(section_id: SectionId, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version: 1,
        data,
    }
}

fn lightmap_blob(bytes: Vec<u8>) -> SectionBlob {
    blob(SectionId::Lightmap, bytes)
}

fn shadowmask_blob(bytes: Vec<u8>) -> SectionBlob {
    blob(SectionId::ShadowmaskAtlas, bytes)
}

/// One vertex naming cell block `lightmap_block` (id + 1, 0 = none).
fn geometry_naming_block(lightmap_block: u16) -> GeometrySection {
    GeometrySection {
        vertices: vec![Vertex::new(
            [0.5, 0.5, 0.5],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.25, 0.75],
            lightmap_block,
        )],
        indices: Vec::new(),
        faces: Vec::new(),
    }
}

/// Byte offset of `field` within id-22 block record `block`.
fn lightmap_record_at(block: usize, field: usize) -> usize {
    LIGHTMAP_HEADER_BYTES + block * LIGHTMAP_BLOCK_RECORD_BYTES + field
}

/// Byte offset of `field` within id-42 block record 0 of a one-channel
/// [`shadowmask_section`]: tag, count, one slot padded to 4, block count.
fn shadowmask_record_at(field: usize) -> usize {
    8 + 4 + 4 + field
}

fn load_error_with_geometry(
    geometry: SectionBlob,
    sections: Vec<SectionBlob>,
    name: &str,
) -> String {
    let path = write_prl_load_fixture_with_geometry(geometry, sections, name);
    let result = load_prl(path.to_str().unwrap());
    std::fs::remove_file(&path).ok();
    match result {
        Ok(_) => panic!("{name}: the load must fail"),
        Err(error) => error.to_string(),
    }
}

fn load_error(sections: Vec<SectionBlob>, name: &str) -> String {
    load_error_with_geometry(geometry_blob(empty_geometry()), sections, name)
}

fn empty_geometry() -> GeometrySection {
    GeometrySection {
        vertices: Vec::new(),
        indices: Vec::new(),
        faces: Vec::new(),
    }
}

/// Loads with empty geometry: a drawn vertex would need faces, cells and a
/// BVH leaf, which the vertex check below does not.
fn load_ok(sections: Vec<SectionBlob>, name: &str) -> LevelWorld {
    let path = write_prl_load_fixture(sections, name);
    let result = load_prl(path.to_str().unwrap());
    std::fs::remove_file(&path).ok();
    result.unwrap_or_else(|error| panic!("{name}: the load must succeed: {error}"))
}

// ---- AC 17: load rejects each malformed or stale cell-block input ----

#[test]
fn load_rejects_an_older_lightmap_section_version() {
    let mut bytes = lightmap_section(&[(4, 4)]).to_bytes();
    bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
    let message = load_error(
        vec![lightmap_blob(bytes)],
        "postretro_test_lm_blocks_old_version.prl",
    );
    assert!(message.contains("Lightmap validation error"), "{message}");
    assert!(message.contains("version: 2"), "{message}");
}

#[test]
fn load_rejects_the_retired_smb5_shadowmask_tag() {
    let extents = [(4, 4)];
    let mut shadowmask = shadowmask_section(&extents).to_bytes();
    shadowmask[0..4].copy_from_slice(b"SMB5");
    let message = load_error(
        vec![
            lightmap_blob(lightmap_section(&extents).to_bytes()),
            shadowmask_blob(shadowmask),
        ],
        "postretro_test_lm_blocks_smb5_tag.prl",
    );
    assert!(
        message.contains("ShadowmaskAtlas validation error"),
        "{message}"
    );
    assert!(message.contains("SMB5"), "{message}");
}

#[test]
fn load_rejects_a_shadowmask_block_count_that_differs_from_the_lightmap() {
    let message = load_error(
        vec![
            lightmap_blob(lightmap_section(&[(4, 4)]).to_bytes()),
            shadowmask_blob(shadowmask_section(&[(4, 4), (8, 4)]).to_bytes()),
        ],
        "postretro_test_lm_blocks_count_mismatch.prl",
    );
    assert!(
        message.contains("shadowmask block count 2 does not match the lightmap's 1"),
        "{message}"
    );
}

#[test]
fn load_rejects_a_vertex_lightmap_block_past_the_table() {
    let message = load_error_with_geometry(
        geometry_blob(geometry_naming_block(2)),
        vec![lightmap_blob(lightmap_section(&[(4, 4)]).to_bytes())],
        "postretro_test_lm_blocks_vertex_past_table.prl",
    );
    assert!(
        message.contains("vertex 0 names lightmap block 1 past the 1-block Lightmap"),
        "{message}"
    );

    // Without id 22 the table is empty, so any nonzero block is past it.
    let message = load_error_with_geometry(
        geometry_blob(geometry_naming_block(1)),
        Vec::new(),
        "postretro_test_lm_blocks_vertex_without_lightmap.prl",
    );
    assert!(
        message.contains("vertex 0 names lightmap block 0 past the 0-block Lightmap"),
        "{message}"
    );
}

#[test]
fn load_rejects_a_block_blob_range_outside_its_section() {
    let extents = [(4, 4)];
    let mut lightmap = lightmap_section(&extents).to_bytes();
    let at = lightmap_record_at(0, 20);
    let past_end = lightmap.len() as u64;
    lightmap[at..at + 8].copy_from_slice(&past_end.to_le_bytes());
    let message = load_error(
        vec![lightmap_blob(lightmap)],
        "postretro_test_lm_blocks_range_outside.prl",
    );
    assert!(message.contains("Lightmap validation error"), "{message}");
    assert!(message.contains("outside the section"), "{message}");

    let mut shadowmask = shadowmask_section(&extents).to_bytes();
    let at = shadowmask_record_at(12);
    let past_end = shadowmask.len() as u64;
    shadowmask[at..at + 8].copy_from_slice(&past_end.to_le_bytes());
    let message = load_error(
        vec![
            lightmap_blob(lightmap_section(&extents).to_bytes()),
            shadowmask_blob(shadowmask),
        ],
        "postretro_test_lm_blocks_shadowmask_range_outside.prl",
    );
    assert!(
        message.contains("ShadowmaskAtlas validation error"),
        "{message}"
    );
    assert!(message.contains("outside the section"), "{message}");
}

#[test]
fn load_rejects_a_block_larger_than_a_pool_layer() {
    let edge = LIGHTMAP_POOL_LAYER_EDGE as u16 + 4;
    let message = load_error(
        vec![lightmap_blob(lightmap_section(&[(edge, 4)]).to_bytes())],
        "postretro_test_lm_blocks_oversize.prl",
    );
    assert!(
        message.contains("exceeds the 2048² pool layer"),
        "{message}"
    );
}

#[test]
fn load_rejects_a_nonzero_reserved_field() {
    let extents = [(4, 4)];
    let mut lightmap = lightmap_section(&extents).to_bytes();
    let at = lightmap_record_at(0, 32);
    lightmap[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
    let message = load_error(
        vec![lightmap_blob(lightmap)],
        "postretro_test_lm_blocks_reserved.prl",
    );
    assert!(
        message.contains("lightmap block 0 has nonzero reserved field"),
        "{message}"
    );

    let mut shadowmask = shadowmask_section(&extents).to_bytes();
    let at = shadowmask_record_at(24);
    shadowmask[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
    let message = load_error(
        vec![
            lightmap_blob(lightmap_section(&extents).to_bytes()),
            shadowmask_blob(shadowmask),
        ],
        "postretro_test_lm_blocks_shadowmask_reserved.prl",
    );
    assert!(
        message.contains("shadowmask block 0 has nonzero reserved field"),
        "{message}"
    );
}

#[test]
fn vertex_check_accepts_every_block_in_the_table_and_no_block() {
    let index = lightmap_section(&[(4, 4), (4, 4)]).index();
    let vertices: Vec<WorldVertex> = [0u16, 1, 2]
        .into_iter()
        .map(|lightmap_block| WorldVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            normal_oct: [0; 2],
            tangent_packed: [0; 2],
            lightmap_uv: [0; 2],
            lightmap_block,
            animated_block: 0,
        })
        .collect();
    validate_vertex_lightmap_blocks(&vertices, Some(&index))
        .expect("ids 0 (none) through the last block + 1 are in range");
    validate_vertex_lightmap_blocks(&vertices[..1], None)
        .expect("a vertex with no block needs no table");
}

// The vertex lightmap fields changed frame with the same bytes: only the
// container entry version tells a stale geometry section apart.
#[test]
fn load_rejects_a_stale_geometry_container_version() {
    let mut geometry = geometry_blob(empty_geometry());
    geometry.version = 1;
    let message = load_error_with_geometry(
        geometry,
        Vec::new(),
        "postretro_test_lm_blocks_geometry_v1.prl",
    );
    assert!(
        message.contains("Geometry validation error: container version 1 (expected 2)"),
        "{message}"
    );
}

// ---- AC 16: zero- and one-block levels ----

#[test]
fn level_without_a_lightmap_loads_in_placeholder_mode_and_warns_once() {
    let path = write_prl_load_fixture(Vec::new(), "postretro_test_lm_blocks_absent.prl");
    let capture = LogCapture::start();
    let world = load_prl(path.to_str().unwrap()).expect("a level without id 22 loads");
    std::fs::remove_file(&path).ok();
    capture.assert_logged_once(log::Level::Warn, "Lightmap section missing");
    assert!(world.lightmap.is_none());
    assert!(world.shadowmask_atlas.is_none());
    assert_eq!(world.gpu_lighting_payloads, GpuLightingPayloads::default());
}

#[test]
fn level_with_zero_lightmap_blocks_loads_with_no_payloads() {
    let world = load_ok(
        vec![lightmap_blob(LightmapSection::empty(2).to_bytes())],
        "postretro_test_lm_blocks_zero.prl",
    );
    let index = world.lightmap.as_ref().expect("a zero-block id 22 is kept");
    assert!(index.records.is_empty());
    assert_eq!(index.header.block_count, 0);
    assert!(world.gpu_lighting_payloads.blocks.is_empty());
}

#[test]
fn level_with_one_lightmap_block_loads_a_one_entry_payload() {
    let section = lightmap_section(&[(8, 4)]);
    let world = load_ok(
        vec![lightmap_blob(section.to_bytes())],
        "postretro_test_lm_blocks_one.prl",
    );
    assert_eq!(world.lightmap, Some(section.index()));
    let payloads = &world.gpu_lighting_payloads.blocks;
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0].irradiance, section.blocks[0].irradiance);
    assert_eq!(payloads[0].direction, section.blocks[0].direction);
    assert_eq!(payloads[0].shadowmask, None, "no id 42 in this level");
}

// ---- Payload split ----

fn loaded(extents: &[(u16, u16)]) -> (LoadedLightmap, LoadedShadowmask) {
    let lightmap = lightmap_section(extents);
    let shadowmask = shadowmask_section(extents);
    let blocks = lightmap
        .blocks
        .iter()
        .map(|block| LightmapBlockPayload {
            irradiance: block.irradiance.clone(),
            direction: block.direction.clone(),
            shadowmask: None,
        })
        .collect();
    (
        LoadedLightmap {
            index: lightmap.index(),
            blocks,
        },
        LoadedShadowmask {
            index: shadowmask.index(),
            groups: shadowmask.blocks,
        },
    )
}

#[test]
fn splitting_moves_each_blocks_shadowmask_groups_beside_its_texels() {
    let extents = [(4, 4), (8, 4)];
    let (lightmap, shadowmask) = loaded(&extents);
    let expected_groups = shadowmask.groups.clone();
    let (lightmap_index, shadowmask_index, payloads) =
        split_gpu_lighting(Some(lightmap), Some(shadowmask));
    assert_eq!(lightmap_index.map(|index| index.records.len()), Some(2));
    assert_eq!(shadowmask_index.map(|index| index.records.len()), Some(2));
    for (block, groups) in expected_groups.into_iter().enumerate() {
        assert_eq!(
            payloads.blocks[block].shadowmask,
            Some(groups),
            "block {block}"
        );
    }
}

// Pin: partial-lighting-install. Whatever is present moves; absence stays absent.
#[test]
fn splitting_partial_lighting_moves_only_the_present_payloads() {
    let (lightmap, _) = loaded(&[(4, 4)]);
    let (lightmap_index, shadowmask_index, payloads) = split_gpu_lighting(Some(lightmap), None);
    assert!(lightmap_index.is_some() && shadowmask_index.is_none());
    assert_eq!(payloads.blocks.len(), 1);
    assert!(payloads.blocks[0].shadowmask.is_none());

    let (lightmap_index, shadowmask_index, payloads) = split_gpu_lighting(None, None);
    assert!(lightmap_index.is_none() && shadowmask_index.is_none());
    assert_eq!(payloads, GpuLightingPayloads::default());
}
