// Proofs for lightmap streaming I/O: mode selection, index-only load, counted pair and span reads.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §4

use std::path::PathBuf;
use std::sync::Arc;

use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_TEXEL_BYTES, LightmapBlock,
    LightmapBlockPayload, LightmapMode, LightmapSection,
};
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, group_plane_len};
use postretro_level_format::{SectionBlob, SectionId};

use super::storage::{LightmapResidencyReason, select_lightmap_residency};
use super::*;
use crate::LevelWorld;
use crate::prl_container::PrlContainer;
use crate::prl_load_test_fixtures::{write_portal_prl_load_fixture, write_prl_load_fixture};
use crate::prl_streaming::load_prl_with_modes_for_test;
use crate::sh_stream::{ShStreamingMode, read_container_positionally};

const LIGHTMAP: u32 = SectionId::Lightmap as u32;
const SHADOWMASK: u32 = SectionId::ShadowmaskAtlas as u32;

// ---- Fixtures ----

/// Blocks at these extents, one per cell, with distinct bytes per block.
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

fn shadowmask_section(extents: &[(u16, u16)]) -> ShadowmaskAtlasSection {
    ShadowmaskAtlasSection {
        channels: vec![0],
        blocks: extents
            .iter()
            .enumerate()
            .map(|(block, &(width, height))| {
                let len = group_plane_len(u32::from(width), u32::from(height)).unwrap() as usize;
                [vec![block as u8 + 10; len], vec![block as u8 + 110; len]]
            })
            .collect(),
    }
}

/// Every cell mandatory from every camera cell of the two-cell fixtures.
fn residency_set() -> CellResidencySetSection {
    let entry = |cell_id| ResidencyEntry { lead: 0, cell_id };
    CellResidencySetSection {
        max_lead: 32 * 1024,
        offsets: vec![0, 2, 4],
        entries: vec![entry(0), entry(1), entry(0), entry(1)],
    }
}

fn blob(section_id: SectionId, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version: 1,
        data,
    }
}

/// Id 51, then id 22 immediately followed by id 42, so a coalesced span can
/// join the tail of one to the head of the other.
fn lighting_blobs(
    lightmap: &LightmapSection,
    shadowmask: Option<&ShadowmaskAtlasSection>,
    with_residency_set: bool,
) -> Vec<SectionBlob> {
    let mut blobs = Vec::new();
    if with_residency_set {
        blobs.push(blob(
            SectionId::CellResidencySet,
            residency_set().to_bytes(),
        ));
    }
    blobs.push(blob(SectionId::Lightmap, lightmap.to_bytes()));
    if let Some(shadowmask) = shadowmask {
        blobs.push(blob(SectionId::ShadowmaskAtlas, shadowmask.to_bytes()));
    }
    blobs
}

/// A temp PRL removed on drop. The name keeps parallel tests apart.
struct Fixture(PathBuf);

impl Fixture {
    fn with_portals(name: &str, blobs: Vec<SectionBlob>) -> Self {
        Self(write_portal_prl_load_fixture(blobs, name))
    }

    fn without_portals(name: &str, blobs: Vec<SectionBlob>) -> Self {
        Self(write_prl_load_fixture(blobs, name))
    }

    fn load(&self, lightmap: LightmapStreamingMode) -> LevelWorld {
        load_prl_with_modes_for_test(self.0.to_str().unwrap(), ShStreamingMode::Off, lightmap)
            .unwrap_or_else(|error| panic!("{}: load failed: {error}", self.0.display()))
    }

    /// Ids 22/42 through the production load reads and storage, bypassing
    /// the rest of the loader, so id 42 survives without the EntityShadowLights
    /// selection the full load reconciles it against.
    fn manifest(&self) -> Arc<LightmapStreamManifest> {
        let (file, meta) = read_container_positionally(self.0.to_str().unwrap()).unwrap();
        let container = PrlContainer::from_positional(file, meta, false);
        let (lightmap, mut read) =
            read_lightmap_for_residency(&container, stream_inputs()).unwrap();
        let lightmap = lightmap.expect("fixture carries id 22").index;
        let shadowmask = read
            .read_shadowmask(&container, Some(&lightmap))
            .unwrap()
            .map(|loaded| loaded.index);
        match read
            .into_storage(&container, Some(&lightmap), shadowmask.as_ref(), None)
            .unwrap()
        {
            LightmapStorage::Streaming(manifest) => manifest,
            LightmapStorage::AllResident => panic!("fixture must stream"),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn stream_inputs() -> LightmapResidencyInputs {
    LightmapResidencyInputs {
        requested: LightmapStreamingMode::Stream,
        has_residency_set: true,
        has_usable_portals: true,
        has_retained_file: true,
    }
}

fn expected_payload(
    lightmap: &LightmapSection,
    shadowmask: Option<&ShadowmaskAtlasSection>,
    block: usize,
) -> LightmapBlockPayload {
    LightmapBlockPayload {
        irradiance: lightmap.blocks[block].irradiance.clone(),
        direction: lightmap.blocks[block].direction.clone(),
        shadowmask: shadowmask.map(|section| section.blocks[block].clone()),
    }
}

fn range_len(range: &std::ops::Range<u64>) -> u64 {
    range.end - range.start
}

// ---- Mode selection (AC 14, no-portals half) ----

#[test]
fn residency_streams_only_with_request_residency_set_portals_file_and_blocks() {
    assert_eq!(
        select_lightmap_residency(stream_inputs(), 3),
        (
            LightmapStreamingMode::Stream,
            LightmapResidencyReason::Streamed
        )
    );
    let blocked = [
        (
            LightmapResidencyInputs {
                requested: LightmapStreamingMode::AllResident,
                ..stream_inputs()
            },
            LightmapResidencyReason::RequestedAllResident,
        ),
        (
            LightmapResidencyInputs {
                has_usable_portals: false,
                ..stream_inputs()
            },
            LightmapResidencyReason::NoUsablePortals,
        ),
        (
            LightmapResidencyInputs {
                has_residency_set: false,
                ..stream_inputs()
            },
            LightmapResidencyReason::NoResidencySet,
        ),
        (
            LightmapResidencyInputs {
                has_retained_file: false,
                ..stream_inputs()
            },
            LightmapResidencyReason::NoRetainedFile,
        ),
    ];
    for (inputs, reason) in blocked {
        assert_eq!(
            select_lightmap_residency(inputs, 3),
            (LightmapStreamingMode::AllResident, reason)
        );
    }
    assert_eq!(
        select_lightmap_residency(stream_inputs(), 0),
        (
            LightmapStreamingMode::AllResident,
            LightmapResidencyReason::NoLightmapBlocks
        ),
        "a zero-block level stays in placeholder mode"
    );
}

#[test]
fn level_without_portals_loads_all_resident_when_streaming_is_requested() {
    let lightmap = lightmap_section(&[(8, 4), (4, 4)]);
    let fixture = Fixture::without_portals(
        "postretro_test_lm_stream_no_portals.prl",
        lighting_blobs(&lightmap, None, true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    assert!(!world.has_portals);
    assert!(world.cell_residency_set.is_some());
    assert!(!world.lightmap_storage().is_streaming());
    assert_eq!(world.gpu_lighting_payloads.blocks.len(), 2);
    assert_eq!(
        world.gpu_lighting_payloads.blocks[0],
        expected_payload(&lightmap, None, 0)
    );
}

#[test]
fn level_without_a_residency_set_loads_all_resident_when_streaming_is_requested() {
    let lightmap = lightmap_section(&[(8, 4), (4, 4)]);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_no_id51.prl",
        lighting_blobs(&lightmap, None, false),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    assert!(world.has_portals);
    assert!(world.cell_residency_set.is_none());
    assert!(!world.lightmap_storage().is_streaming());
    assert_eq!(world.gpu_lighting_payloads.blocks.len(), 2);
}

#[test]
fn requested_all_resident_reads_the_whole_sections_even_when_the_level_could_stream() {
    let lightmap = lightmap_section(&[(8, 4), (4, 4)]);
    let shadowmask = shadowmask_section(&[(8, 4), (4, 4)]);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_requested_all_resident.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let world = fixture.load(LightmapStreamingMode::AllResident);
    assert!(!world.lightmap_storage().is_streaming());
    assert_eq!(world.gpu_lighting_payloads.blocks.len(), 2);
    let reads = world
        .prl_read_counters()
        .expect("loaded through the reader");
    assert_eq!(reads.section_bytes(LIGHTMAP), lightmap.byte_len() as u64);
    assert_eq!(
        reads.section_bytes(SHADOWMASK),
        shadowmask.byte_len() as u64
    );
}

#[test]
fn zero_block_level_stays_in_placeholder_mode_when_streaming_is_requested() {
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_zero_blocks.prl",
        lighting_blobs(&LightmapSection::empty(2), None, true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    assert!(!world.lightmap_storage().is_streaming());
    assert_eq!(world.lightmap.as_ref().map(|i| i.records.len()), Some(0));
    assert!(world.gpu_lighting_payloads.blocks.is_empty());
}

// ---- Streaming load holds only the indexes (AC 12, loader half) ----

#[test]
fn streaming_load_reads_exactly_the_id22_and_id42_index_prefixes() {
    let extents = [(8, 4), (4, 4), (4, 8)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_index_only.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);

    let manifest = world
        .lightmap_stream_manifest()
        .expect("portals, id 51 and blocks select streaming");
    assert!(
        world.gpu_lighting_payloads.blocks.is_empty(),
        "no block payload is resident after a streaming load"
    );
    assert_eq!(world.lightmap, Some(lightmap.index()));
    assert_eq!(manifest.lightmap_index(), &lightmap.index());
    assert_eq!(manifest.block_count(), 3);

    let reads = world
        .prl_read_counters()
        .expect("loaded through the reader");
    assert!(Arc::ptr_eq(reads, manifest.read_counters()));
    assert_eq!(
        reads.section_bytes(LIGHTMAP),
        lightmap.index().header.index_byte_len().unwrap()
    );
    assert_eq!(
        reads.section_bytes(SHADOWMASK),
        shadowmask.index().index_byte_len() as u64,
        "id 42's prefix is read even though the load later drops the section \
         for want of EntityShadowLights"
    );
}

#[test]
fn a_pair_read_adds_exactly_its_two_ranges_to_the_section_counters() {
    let extents = [(8, 4), (4, 4), (4, 8)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_pair_read.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let manifest = fixture.manifest();
    let reads = manifest.read_counters().clone();
    let before = (
        reads.section_bytes(LIGHTMAP),
        reads.section_bytes(SHADOWMASK),
    );

    let ranges = manifest.block_file_ranges(1).unwrap();
    let shadowmask_range = ranges.shadowmask.clone().expect("id 42 kept");
    let payload = manifest.read_block_pair(1).unwrap();

    assert_eq!(payload, expected_payload(&lightmap, Some(&shadowmask), 1));
    assert_eq!(
        reads.section_bytes(LIGHTMAP) - before.0,
        range_len(&ranges.lightmap)
    );
    assert_eq!(
        reads.section_bytes(SHADOWMASK) - before.1,
        range_len(&shadowmask_range)
    );
    let record = lightmap.index().records[1];
    assert_eq!(
        range_len(&ranges.lightmap),
        u64::from(record.irradiance.len + record.direction.len)
    );
}

#[test]
fn pair_ranges_are_absolute_file_offsets_of_each_blob_pair() {
    let extents = [(8, 4), (4, 4)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_pair_ranges.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let manifest = fixture.manifest();
    let file = std::fs::read(&fixture.0).unwrap();
    for block in 0..2u32 {
        let ranges = manifest.block_file_ranges(block).unwrap();
        let expected = expected_payload(&lightmap, Some(&shadowmask), block as usize);
        let lm = &file[ranges.lightmap.start as usize..ranges.lightmap.end as usize];
        assert_eq!(lm, [expected.irradiance, expected.direction].concat());
        let sm = ranges.shadowmask.unwrap();
        let [group_a, group_b] = expected.shadowmask.unwrap();
        assert_eq!(
            &file[sm.start as usize..sm.end as usize],
            [group_a, group_b].concat()
        );
    }
    assert!(manifest.block_file_ranges(2).is_err());
}

#[test]
fn span_read_accepts_a_coalesced_span_across_the_id22_id42_boundary() {
    let extents = [(8, 4), (4, 4)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_coalesced_span.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let manifest = fixture.manifest();
    let reads = manifest.read_counters().clone();
    let before = (
        reads.section_bytes(LIGHTMAP),
        reads.section_bytes(SHADOWMASK),
    );

    // The last id-22 block's range through the first id-42 block's range:
    // the tail of id 22, the head of id 42 (its index), and block 0's groups.
    let tail = manifest.block_file_ranges(1).unwrap().lightmap;
    let head = manifest.block_file_ranges(0).unwrap().shadowmask.unwrap();
    let lightmap_section = manifest.lightmap_section_range();
    let shadowmask_section = manifest.shadowmask_section_range().unwrap();
    assert_eq!(tail.end, lightmap_section.end, "block 1 ends id 22");
    assert_eq!(lightmap_section.end, shadowmask_section.start, "adjacent");

    let span = tail.start..head.end;
    let bytes = manifest.read_file_span(span.clone()).unwrap();
    let file = std::fs::read(&fixture.0).unwrap();
    assert_eq!(bytes, file[span.start as usize..span.end as usize]);
    assert_eq!(
        reads.section_bytes(LIGHTMAP) - before.0,
        tail.end - tail.start
    );
    assert_eq!(
        reads.section_bytes(SHADOWMASK) - before.1,
        head.end - shadowmask_section.start
    );
}

#[test]
fn span_read_rejects_ranges_outside_the_id22_id42_region_before_reading() {
    let extents = [(8, 4)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_span_reject.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let manifest = fixture.manifest();
    let region = manifest.span_region();
    assert_eq!(region.start, manifest.lightmap_section_range().start);
    assert_eq!(region.end, manifest.shadowmask_section_range().unwrap().end);
    let total = manifest.read_counters().total_bytes();

    for span in [
        region.start - 1..region.start + 4,
        region.end - 4..region.end + 1,
        region.start + 8..region.start + 4,
    ] {
        let error = manifest.read_file_span(span.clone()).unwrap_err();
        assert!(
            error.to_string().contains("outside the id-22/42 region"),
            "{span:?}: {error}"
        );
    }
    assert_eq!(manifest.read_counters().total_bytes(), total, "no read");
    assert_eq!(
        manifest.read_file_span(region.clone()).unwrap().len() as u64,
        range_len(&region)
    );
}

#[test]
fn pair_bytes_of_the_wrong_length_are_rejected() {
    let extents = [(8, 4)];
    let lightmap = lightmap_section(&extents);
    let shadowmask = shadowmask_section(&extents);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_pair_len.prl",
        lighting_blobs(&lightmap, Some(&shadowmask), true),
    );
    let manifest = fixture.manifest();
    let ranges = manifest.block_file_ranges(0).unwrap();
    let lightmap_bytes = manifest.read_file_span(ranges.lightmap.clone()).unwrap();
    let shadowmask_bytes = manifest
        .read_file_span(ranges.shadowmask.clone().unwrap())
        .unwrap();
    let mut short = shadowmask_bytes.clone();
    short.pop();
    assert!(
        manifest
            .payload_from_pair_bytes(0, lightmap_bytes.clone(), Some(short))
            .is_err()
    );
    assert!(
        manifest
            .payload_from_pair_bytes(0, lightmap_bytes.clone(), None)
            .is_err(),
        "a level keeping id 42 needs both halves"
    );
    assert_eq!(
        manifest
            .payload_from_pair_bytes(0, lightmap_bytes, Some(shadowmask_bytes))
            .unwrap(),
        expected_payload(&lightmap, Some(&shadowmask), 0)
    );
}

// ---- One-block level streams (AC 16, streaming half) ----

#[test]
fn one_block_level_streams_an_index_of_one_record_and_reads_its_pair_on_demand() {
    let lightmap = lightmap_section(&[(8, 4)]);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_one_block.prl",
        lighting_blobs(&lightmap, None, true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    let manifest = world.lightmap_stream_manifest().expect("one block streams");
    assert_eq!(manifest.lightmap_index().records.len(), 1);
    assert!(manifest.shadowmask_index().is_none());
    assert!(world.gpu_lighting_payloads.blocks.is_empty());

    let reads = manifest.read_counters();
    let index_len = lightmap.index().header.index_byte_len().unwrap();
    assert_eq!(reads.section_bytes(LIGHTMAP), index_len);
    let ranges = manifest.block_file_ranges(0).unwrap();
    assert_eq!(ranges.shadowmask, None);
    assert_eq!(
        manifest.read_block_pair(0).unwrap(),
        expected_payload(&lightmap, None, 0)
    );
    assert_eq!(
        reads.section_bytes(LIGHTMAP),
        index_len + range_len(&ranges.lightmap),
        "the whole section: its index plus its one block"
    );
    assert_eq!(
        index_len + range_len(&ranges.lightmap),
        lightmap.byte_len() as u64
    );
}

// ---- Lifetime ----

#[test]
fn dropping_the_world_releases_the_manifest_and_the_retained_file() {
    let lightmap = lightmap_section(&[(8, 4), (4, 4)]);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_release.prl",
        lighting_blobs(&lightmap, None, true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    let manifest = Arc::downgrade(world.lightmap_stream_manifest().unwrap());
    let file = Arc::downgrade(manifest.upgrade().unwrap().retained_file());
    drop(world);
    assert!(manifest.upgrade().is_none());
    assert!(file.upgrade().is_none(), "no other owner holds the file");
}

#[test]
fn drain_batches_validate_against_the_manifest_block_count_and_tag() {
    let lightmap = lightmap_section(&[(8, 4), (4, 4)]);
    let fixture = Fixture::with_portals(
        "postretro_test_lm_stream_drain_identity.prl",
        lighting_blobs(&lightmap, None, true),
    );
    let world = fixture.load(LightmapStreamingMode::Stream);
    let manifest = world.lightmap_stream_manifest().unwrap();
    let mut batch = LightmapDrainBatch {
        generation: 1,
        content_tag: manifest.content_tag(),
        target_remove: vec![1],
        ..Default::default()
    };
    manifest.validate_drain_batch(&batch).unwrap();
    batch.target_remove = vec![2];
    assert!(manifest.validate_drain_batch(&batch).is_err());
    batch.target_remove.clear();
    batch.content_tag = [0; 32];
    assert!(manifest.validate_drain_batch(&batch).is_err());
}
