// Proofs for id 42 plane absence and id 58 across both load modes.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §4

use postretro_level_format::lightmap_pool_bound::LightmapPoolBoundSection;

use super::*;

const EXTENTS: [(u16, u16); 4] = [(8, 4), (8, 4), (8, 4), (8, 4)];

/// Two blocks per fixture cell, so all four fit the two-cell residency set.
fn four_block_lightmap() -> LightmapSection {
    let mut lightmap = lightmap_section(&EXTENTS);
    for (block, record) in lightmap.blocks.iter_mut().enumerate() {
        record.cell_id = block as u32 / 2;
    }
    lightmap
}

/// Block 0 stores both planes, 1 only plane B, 2 only plane A, 3 neither.
fn mixed_shadowmask() -> ShadowmaskAtlasSection {
    let mut section = shadowmask_section(&EXTENTS);
    section.blocks[1][0].clear();
    section.blocks[2][1].clear();
    section.blocks[3] = [Vec::new(), Vec::new()];
    section
}

/// What a level whose planes were all stored would hand the pool: zeros
/// wherever `section` has no blob.
fn full_planes(section: &ShadowmaskAtlasSection, block: usize) -> [Vec<u8>; 2] {
    let len = group_plane_len(8, 4).unwrap() as usize;
    section.blocks[block].clone().map(|plane| {
        if plane.is_empty() {
            vec![0; len]
        } else {
            plane
        }
    })
}

fn mixed_fixture(name: &str) -> (Fixture, LightmapSection, ShadowmaskAtlasSection) {
    let (lightmap, shadowmask) = (four_block_lightmap(), mixed_shadowmask());
    let blobs = lighting_blobs(&lightmap, Some(&shadowmask), true);
    (Fixture::with_portals(name, blobs), lightmap, shadowmask)
}

#[test]
fn a_block_reads_only_the_planes_it_stores() {
    let (fixture, _, shadowmask) = mixed_fixture("postretro_test_lm_planes_ranges.prl");
    let manifest = fixture.manifest();
    let file = std::fs::read(&fixture.0).unwrap();
    let stored = |block: u32| {
        let range = manifest.block_file_ranges(block).unwrap().shadowmask;
        range.map(|range| file[range.start as usize..range.end as usize].to_vec())
    };
    let [a, b] = shadowmask.blocks[0].clone();
    assert_eq!(stored(0), Some([a, b].concat()));
    assert_eq!(stored(1), Some(shadowmask.blocks[1][1].clone()));
    assert_eq!(stored(2), Some(shadowmask.blocks[2][0].clone()));
    assert_eq!(stored(3), None);
}

#[test]
fn absent_planes_reach_the_pool_as_zero_planes_in_both_load_modes() {
    let (fixture, lightmap, shadowmask) = mixed_fixture("postretro_test_lm_planes_payloads.prl");
    let streamed = fixture.manifest();
    let resident = fixture.load(LightmapStreamingMode::AllResident);
    for block in 0..EXTENTS.len() {
        let expected = LightmapBlockPayload {
            shadowmask: Some(full_planes(&shadowmask, block)),
            ..expected_payload(&lightmap, None, block)
        };
        assert_eq!(
            streamed.read_block_pair(block as u32).unwrap(),
            expected,
            "streamed block {block}"
        );
        assert_eq!(
            resident.gpu_lighting_payloads.blocks[block], expected,
            "all-resident block {block}"
        );
    }
}

#[test]
fn a_block_with_no_stored_plane_reads_nothing_from_id_42() {
    let (fixture, _, _) = mixed_fixture("postretro_test_lm_planes_no_read.prl");
    let manifest = fixture.manifest();
    let before = manifest.read_counters().section_bytes(SHADOWMASK);
    manifest.read_block_pair(3).unwrap();
    assert_eq!(manifest.read_counters().section_bytes(SHADOWMASK), before);
}

#[test]
fn pair_bytes_must_match_the_blocks_plane_presence() {
    let (fixture, _, _) = mixed_fixture("postretro_test_lm_planes_presence.prl");
    let manifest = fixture.manifest();
    let lightmap_of = |block| {
        let range = manifest.block_file_ranges(block).unwrap().lightmap;
        manifest.read_file_span(range).unwrap()
    };
    let message = |result: Result<LightmapBlockPayload, _>| {
        let error: crate::prl::PrlLoadError = result.expect_err("presence disagrees");
        error.to_string()
    };
    let needs_plane = message(manifest.payload_from_pair_bytes(1, lightmap_of(1), None));
    assert!(needs_plane.contains("plane presence"), "{needs_plane}");
    let extra = Some(vec![0; 32]);
    let has_none = message(manifest.payload_from_pair_bytes(3, lightmap_of(3), extra));
    assert!(has_none.contains("plane presence"), "{has_none}");
    let short = Some(vec![0; 31]);
    let short = message(manifest.payload_from_pair_bytes(1, lightmap_of(1), short));
    assert!(short.contains("read returned 31 bytes"), "{short}");
}

/// A load's error text, whichever mode it reads the section in.
fn load_error(path: &std::path::Path, mode: LightmapStreamingMode) -> String {
    load_prl_with_modes_for_test(path.to_str().unwrap(), ShStreamingMode::Off, mode)
        .map(|_| ())
        .expect_err("the level must be refused")
        .to_string()
}

#[test]
fn a_level_with_the_retired_tag_is_refused_by_name_in_both_load_modes() {
    let lightmap = lightmap_section(&[(8, 4)]);
    let shadowmask = shadowmask_section(&[(8, 4)]);
    let mut blobs = lighting_blobs(&lightmap, Some(&shadowmask), true);
    let id_42 = blobs
        .iter_mut()
        .find(|blob| blob.section_id == SHADOWMASK)
        .unwrap();
    id_42.data[0..4].copy_from_slice(b"SMB7");
    let fixture = Fixture::with_portals("postretro_test_lm_planes_smb7.prl", blobs);
    for mode in [
        LightmapStreamingMode::AllResident,
        LightmapStreamingMode::Stream,
    ] {
        let message = load_error(&fixture.0, mode);
        assert!(
            message.contains("SMB7") && message.contains("re-bake"),
            "{mode:?}: {message}"
        );
    }
}

fn bound_blob(section: LightmapPoolBoundSection) -> SectionBlob {
    blob(SectionId::LightmapPoolBound, section.to_bytes())
}

const BOUND: LightmapPoolBoundSection = LightmapPoolBoundSection {
    worst_lightmap_layers: 27,
    worst_mask_layers: 12,
};

#[test]
fn rejects_unknown_id_58_version_by_name_in_both_load_modes() {
    let lightmap = lightmap_section(&[(8, 4)]);
    let mut blobs = lighting_blobs(&lightmap, None, true);
    let mut unknown = bound_blob(BOUND);
    unknown.data[0..4].copy_from_slice(&2u32.to_le_bytes());
    blobs.push(unknown);
    let fixture = Fixture::with_portals("postretro_test_lm_bound_version.prl", blobs);
    for mode in [
        LightmapStreamingMode::AllResident,
        LightmapStreamingMode::Stream,
    ] {
        let message = load_error(&fixture.0, mode);
        assert!(
            message.contains("LightmapPoolBound version 2 is unknown"),
            "{mode:?}: {message}"
        );
    }
}

#[test]
fn a_streamed_level_keeps_its_id_58_on_the_manifest() {
    let lightmap = lightmap_section(&[(8, 4)]);
    let mut blobs = lighting_blobs(&lightmap, None, true);
    blobs.push(bound_blob(BOUND));
    let fixture = Fixture::with_portals("postretro_test_lm_bound_kept.prl", blobs);
    let world = fixture.load(LightmapStreamingMode::Stream);
    let manifest = world.lightmap_stream_manifest().expect("the level streams");
    assert_eq!(manifest.pool_bound(), Some(BOUND));
}
