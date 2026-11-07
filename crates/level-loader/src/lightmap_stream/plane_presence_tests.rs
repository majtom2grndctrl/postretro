// Proofs for id 42 plane absence and id 58 across both load modes.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §4

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

// The pool places what a payload carries, so an absent plane must stay empty
// here in both load modes: no placement, no upload, and no bytes.
#[test]
fn an_absent_plane_stays_empty_in_both_load_modes() {
    let (fixture, lightmap, shadowmask) = mixed_fixture("postretro_test_lm_planes_payloads.prl");
    let streamed = fixture.manifest();
    let resident = fixture.load(LightmapStreamingMode::AllResident);
    for block in 0..EXTENTS.len() {
        let expected = LightmapBlockPayload {
            shadowmask: Some(shadowmask.blocks[block].clone()),
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
fn streamed_block_reads_exactly_its_present_plane() {
    let (fixture, _, shadowmask) = mixed_fixture("postretro_test_lm_planes_exact.prl");
    let manifest = fixture.manifest();
    let read = |block: u32| {
        let before = manifest.read_counters().section_bytes(SHADOWMASK);
        let payload = manifest.read_block_pair(block).unwrap();
        let bytes = manifest.read_counters().section_bytes(SHADOWMASK) - before;
        (payload.shadowmask.unwrap(), bytes)
    };

    // Block 1 stores only plane B, and block 2 only plane A.
    let plane_b = shadowmask.blocks[1][1].clone();
    let plane_a = shadowmask.blocks[2][0].clone();
    assert!(!plane_a.is_empty() && !plane_b.is_empty());
    let (only_b, bytes) = read(1);
    assert_eq!(only_b, [Vec::new(), plane_b.clone()]);
    assert_eq!(
        bytes,
        plane_b.len() as u64,
        "reads plane B and nothing else"
    );
    let (only_a, bytes) = read(2);
    assert_eq!(only_a, [plane_a.clone(), Vec::new()]);
    assert_eq!(
        bytes,
        plane_a.len() as u64,
        "reads plane A and nothing else"
    );
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
    for mode in BOTH_MODES {
        let message = load_error(&fixture.0, mode);
        assert!(
            message.contains("SMB7") && message.contains("re-bake"),
            "{mode:?}: {message}"
        );
    }
}

const BOTH_MODES: [LightmapStreamingMode; 2] = [
    LightmapStreamingMode::AllResident,
    LightmapStreamingMode::Stream,
];

/// A portal level with a block, id 51, and whatever `edit` leaves of its id 58.
fn level_with_bound(name: &str, edit: impl FnOnce(&mut Vec<SectionBlob>)) -> Fixture {
    let lightmap = lightmap_section(&[(8, 4)]);
    let mut blobs = lighting_blobs(&lightmap, None, true);
    edit(&mut blobs);
    Fixture::with_portals(name, blobs)
}

fn is_bound(blob: &SectionBlob) -> bool {
    blob.section_id == SectionId::LightmapPoolBound as u32
}

#[test]
fn level_needing_the_bound_refuses_absent_or_unknown_id_58() {
    let absent = level_with_bound("postretro_test_lm_bound_absent.prl", |blobs| {
        blobs.retain(|blob| !is_bound(blob));
    });
    let unknown = level_with_bound("postretro_test_lm_bound_version.prl", |blobs| {
        let bound = blobs.iter_mut().find(|blob| is_bound(blob)).unwrap();
        bound.data[0..4].copy_from_slice(&2u32.to_le_bytes());
    });
    for mode in BOTH_MODES {
        let message = load_error(&absent.0, mode);
        assert!(
            message.contains("LightmapPoolBound") && message.contains("re-bake"),
            "{mode:?}: {message}"
        );
        let message = load_error(&unknown.0, mode);
        assert!(
            message.contains("LightmapPoolBound version 2 is unknown"),
            "{mode:?}: {message}"
        );
    }
}

// The requirement is the compiler's emission rule, read from the table: id 22
// blocks and id 51, not whether this load streams.
#[test]
fn a_level_the_compiler_stores_no_bound_for_loads_without_id_58() {
    let no_id_51 = Fixture::with_portals(
        "postretro_test_lm_bound_no_id_51.prl",
        lighting_blobs(&lightmap_section(&[(8, 4)]), None, false),
    );
    let no_blocks = Fixture::with_portals(
        "postretro_test_lm_bound_no_blocks.prl",
        lighting_blobs(&lightmap_section(&[]), None, true),
    );
    for mode in BOTH_MODES {
        no_id_51.load(mode);
        no_blocks.load(mode);
    }
}

#[test]
fn a_streamed_level_keeps_its_id_58_on_the_manifest() {
    let fixture = level_with_bound("postretro_test_lm_bound_kept.prl", |_| {});
    let world = fixture.load(LightmapStreamingMode::Stream);
    let manifest = world.lightmap_stream_manifest().expect("the level streams");
    assert_eq!(manifest.pool_bound(), Some(FIXTURE_POOL_BOUND));
}
