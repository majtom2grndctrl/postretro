//! End-to-end metadata-first fixture for the streamed SH loader.
//!
//! The fixture intentionally stays here, rather than in `prl_loader`'s broad
//! legacy suite: it constructs id 49/id 50 directly through `level-format` and
//! makes the streaming ownership contract apparent at the test boundary.

use std::ops::Range;
use std::path::PathBuf;

use postretro_level_format::alpha_lights::{
    ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightRecord, AlphaLightType,
    AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::{
    BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16, BillboardDirectScatterVolumeSection,
};
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cells::{CellRecord, CellsSection};
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectorySection, ClusterDirectoryShInventory,
    ClusterDirectoryValidationInputs, ClusterRecord, populate_canonical_resource_ranges,
};
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_PAYLOADS_CONTAINER_VERSION, CLUSTER_SH_PAYLOADS_VERSION, ClusterShPayloadsHeader,
    ClusterShPayloadsIndexRecord, ClusterShPayloadsSection, ClusterShPayloadsSourceRecord,
    ClusterShPayloadsValidationInputs, cluster_sh_isolated_atlas_array_layout,
    source_metadata_from_sections,
};
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{MAX_SH_ATLAS_DIMENSION, irradiance_atlas_array_layout};
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sh_volume::{
    AnimationDescriptor, OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{SectionBlob, SectionId, read_container, write_prl};

use crate::ShStorage;
use crate::prl_loader::load_prl_with_streaming_mode_for_test;
use crate::sh_stream::{ShStreamingMode, observe_positional_reads};

const GRID_DIMENSIONS: [u32; 3] = [16, 16, 4];
const AFFINITY_DIMENSIONS: [u32; 3] = [4, 4, 1];
const PROBE_COUNT: u32 = 16 * 16 * 4;
const AFFINITY_COUNT: u32 = 4 * 4;
const STORED_TILES: u32 = PROBE_COUNT;

struct StreamFixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    large_atlas_bytes: usize,
    id50_metadata_len: usize,
}

#[test]
fn stream_mode_keeps_large_sh_bodies_in_the_prl_and_retains_only_metadata() {
    let fixture = write_stream_fixture(Id50Body::Valid);
    assert!(
        fixture.large_atlas_bytes >= 256 * 1024,
        "the fixture must make an eager id-34 body read observable"
    );

    let (streamed, report) = observe_positional_reads(streamed_body_ranges(&fixture), || {
        load_prl_with_streaming_mode_for_test(
            fixture.path.to_str().unwrap(),
            ShStreamingMode::Async,
        )
    });
    assert!(
        report.forbidden_reads.is_empty(),
        "streaming load read legacy SH body range(s): {:?}",
        report.forbidden_reads
    );
    let streamed = streamed.unwrap();
    let manifest = streamed.sh_stream_manifest().unwrap();

    assert!(matches!(streamed.sh_storage(), ShStorage::Streaming(_)));
    assert_eq!(manifest.cluster_count(), 2);
    assert_eq!(manifest.base().grid_dimensions, GRID_DIMENSIONS);
    assert_eq!(manifest.base().probes.len(), PROBE_COUNT as usize);
    assert_eq!(streamed.animation_descriptors().len(), 1);
    assert_eq!(streamed.animated_direct_descriptor_indices(), &[0]);
    assert_eq!(streamed.animated_direct_affinity_lights(), &[0]);
    assert_eq!(streamed.entity_shadow_lights(), &[0]);
    assert!(manifest.sources().indirect_delta.is_some());
    assert!(manifest.sources().direct.is_some());
    assert!(manifest.sources().direct_delta.is_some());
    assert!(manifest.sources().animated_direct_delta.is_some());
    assert_eq!(
        manifest
            .sources()
            .direct_delta
            .as_ref()
            .unwrap()
            .affinity_lights,
        &[0]
    );
    assert_eq!(
        manifest.cluster_adjacency(),
        &[Vec::<u32>::new(), Vec::new()]
    );
    assert_ne!(manifest.content_tag(), [0; 32]);

    // The raw legacy storage fields are deliberately empty in stream mode.
    // Retaining any of these values would retain the corresponding large
    // payload body instead of the projection held by `ShStreamManifest`.
    assert!(streamed.sh_volume.is_none());
    assert!(streamed.delta_sh_volumes.is_none());
    assert!(streamed.direct_sh_volume.is_none());
    assert!(streamed.direct_sh_delta_volumes.is_none());
    assert!(streamed.animated_direct_sh_delta_volumes.is_none());
    assert!(streamed.sh_volume().is_none());
    assert!(streamed.direct_sh_volume().is_none());

    // Scatter keeps its established global-probe addressing and therefore
    // remains whole-resident. This also proves the streamed id-34 projection
    // still cross-validates both id-34+47 and id-45+48 pairs.
    assert_eq!(
        streamed
            .billboard_direct_scatter_volume()
            .unwrap()
            .scatter_rgba
            .len(),
        PROBE_COUNT as usize * 4
    );
    assert!(
        streamed
            .animated_billboard_direct_scatter_delta_volumes()
            .is_some()
    );

    // The `off` escape is selected only after the same id-49/id-50 pair has
    // validated, then retains legacy bodies as its compatibility contract.
    let legacy =
        load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), ShStreamingMode::Off)
            .unwrap();
    assert!(matches!(legacy.sh_storage(), ShStorage::Legacy));
    assert_eq!(
        legacy.sh_volume().unwrap().compact_atlas.len(),
        fixture.large_atlas_bytes
    );
    assert_eq!(
        legacy.direct_sh_volume().unwrap().atlas.len(),
        fixture.large_atlas_bytes
    );
    assert!(legacy.delta_sh_volumes().is_some());
    assert!(legacy.direct_sh_delta_volumes().is_some());
    assert!(legacy.animated_direct_sh_delta_volumes().is_some());
    assert_eq!(legacy.entity_shadow_lights(), &[0]);
    assert_eq!(legacy.animation_descriptors().len(), 1);
    assert_eq!(legacy.animated_direct_descriptor_indices(), &[0]);
    assert_eq!(legacy.animated_direct_affinity_lights(), &[0]);
}

#[test]
fn malformed_id50_with_a_valid_directory_is_rejected_before_off_mode() {
    let fixture = write_stream_fixture(Id50Body::Malformed);
    let error =
        load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), ShStreamingMode::Off)
            .unwrap_err();

    assert!(
        error.to_string().contains("SH streaming validation error"),
        "id-50 validation must fail before the mode can choose legacy loading: {error}"
    );
}

#[cfg(unix)]
#[test]
fn manifest_reads_the_opened_file_after_its_diagnostic_path_is_replaced() {
    let fixture = write_stream_fixture(Id50Body::Valid);
    let streamed = load_prl_with_streaming_mode_for_test(
        fixture.path.to_str().unwrap(),
        ShStreamingMode::Async,
    )
    .unwrap();
    let manifest = streamed.sh_stream_manifest().unwrap().clone();
    let replacement = fixture.path.with_file_name("replacement.prl");

    std::fs::write(&replacement, b"not the retained PRL").unwrap();
    std::fs::rename(&replacement, &fixture.path).unwrap();

    assert_eq!(
        std::fs::read(&fixture.path).unwrap(),
        b"not the retained PRL"
    );
    let decoded = manifest.read_and_decode_cluster(0).unwrap();
    assert_eq!(decoded.cluster_id, 0);
    assert_eq!(decoded.blocks.len(), 6);
}

#[test]
fn sync_proof_manifest_reads_every_streamed_family_from_one_real_chunk() {
    let fixture = write_stream_fixture(Id50Body::Valid);
    let world = load_prl_with_streaming_mode_for_test(
        fixture.path.to_str().unwrap(),
        ShStreamingMode::SyncProof,
    )
    .unwrap();
    let manifest = world.sh_stream_manifest().unwrap();
    let decoded = manifest.read_and_decode_cluster(0).unwrap();
    let source_ids: Vec<_> = decoded
        .blocks
        .iter()
        .map(|block| block.section_id)
        .collect();
    assert_eq!(
        source_ids,
        vec![
            SectionId::DeltaShVolumes as u32,
            SectionId::OctahedralShVolume as u32,
            SectionId::OctahedralShVolume as u32,
            SectionId::DirectShVolume as u32,
            SectionId::DirectShDeltaVolumes as u32,
            SectionId::AnimatedDirectShDeltaVolumes as u32,
        ]
    );
    assert!(
        decoded
            .blocks
            .iter()
            .all(|block| !decoded.block_bytes(block).is_empty())
    );

    let empty = manifest.read_and_decode_cluster(1).unwrap();
    assert!(empty.blocks.is_empty());
    assert!(empty.bytes.is_empty());
}

#[cfg(unix)]
#[test]
fn delayed_positional_reader_decodes_the_same_verified_chunk() {
    use std::os::unix::fs::FileExt;
    use std::time::Duration;

    let fixture = write_stream_fixture(Id50Body::Valid);
    let world = load_prl_with_streaming_mode_for_test(
        fixture.path.to_str().unwrap(),
        ShStreamingMode::Async,
    )
    .unwrap();
    let manifest = world.sh_stream_manifest().unwrap();
    let bytes = manifest
        .read_encoded_cluster_with(0, |file, offset, len| {
            std::thread::sleep(Duration::from_millis(250));
            let mut bytes = vec![0; len as usize];
            let mut cursor = 0;
            while cursor < bytes.len() {
                let read = file.read_at(&mut bytes[cursor..], offset + cursor as u64)?;
                assert_ne!(read, 0);
                cursor += read;
            }
            Ok(bytes)
        })
        .unwrap();
    let decoded = manifest.decode_encoded_cluster(0, bytes).unwrap();
    assert_eq!(decoded.cluster_id, 0);
    assert_eq!(decoded.blocks.len(), 6);
    assert_eq!(
        decoded.bytes,
        manifest.read_and_decode_cluster(0).unwrap().bytes
    );
}

#[test]
fn chunk_file_range_is_absolute_and_empty_for_a_canonical_empty_cluster() {
    let fixture = write_stream_fixture(Id50Body::Valid);
    let world = load_prl_with_streaming_mode_for_test(
        fixture.path.to_str().unwrap(),
        ShStreamingMode::Async,
    )
    .unwrap();
    let manifest = world.sh_stream_manifest().unwrap();
    let id50 = manifest
        .container()
        .find_section(SectionId::ClusterShPayloads as u32)
        .unwrap();
    let region_start = id50.offset + fixture.id50_metadata_len as u64;
    let chunk_len = manifest.payloads().index[0].payload_len;

    let chunk = manifest.chunk_file_range(0).unwrap();
    assert_eq!(chunk, region_start..region_start + chunk_len);
    let empty = manifest.chunk_file_range(1).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.start, chunk.end);
    assert!(manifest.chunk_file_range(2).is_err(), "unknown cluster");

    // The span read returns exactly the bytes the per-chunk decode verifies.
    let bytes = manifest.read_file_span(chunk).unwrap();
    let decoded = manifest.decode_encoded_cluster(0, bytes).unwrap();
    assert_eq!(decoded.blocks.len(), 6);
    assert!(manifest.read_file_span(empty).unwrap().is_empty());
}

#[test]
fn read_file_span_rejects_ranges_outside_the_id50_payload_region() {
    let fixture = write_stream_fixture(Id50Body::Valid);
    let world = load_prl_with_streaming_mode_for_test(
        fixture.path.to_str().unwrap(),
        ShStreamingMode::Async,
    )
    .unwrap();
    let manifest = world.sh_stream_manifest().unwrap();
    let id50 = manifest
        .container()
        .find_section(SectionId::ClusterShPayloads as u32)
        .unwrap();
    let region_start = id50.offset + fixture.id50_metadata_len as u64;
    let region_end = id50.offset + id50.size;

    let rejected = [
        // Starts inside the id-50 metadata prefix.
        region_start - 1..region_start + 4,
        // Runs past the end of the section.
        region_end - 4..region_end + 1,
        // Entirely before the section.
        0..8,
        // Reversed.
        region_start + 4..region_start,
    ];
    for range in rejected {
        let error = manifest.read_file_span(range.clone()).unwrap_err();
        assert!(
            format!("{error}").contains("outside the id-50 payload region"),
            "{range:?}: unexpected error {error}"
        );
    }
    assert_eq!(
        manifest
            .read_file_span(region_start..region_end)
            .unwrap()
            .len() as u64,
        region_end - region_start
    );
}

#[derive(Clone, Copy)]
enum Id50Body {
    Valid,
    Malformed,
}

fn write_stream_fixture(id50_body: Id50Body) -> StreamFixture {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cluster-stream.prl");
    let cells = fixture_cells();
    let portals = PortalsSection {
        vertices: Vec::new(),
        portals: Vec::new(),
    };
    let bvh = BvhSection {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    let locator = fixture_locator();
    let base = large_base_section();
    let direct = direct_section(&base);
    let indirect_delta = indirect_delta_section();
    let direct_delta = direct_delta_section();
    let animated_direct_delta = animated_direct_delta_section();
    let billboard = billboard_section();
    let animated_billboard = animated_billboard_section();
    let alpha_lights = fixture_alpha_lights();
    let shadow_selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let cluster_directory = cluster_directory(
        &cells,
        &portals,
        &bvh,
        &locator,
        &base,
        &direct,
        &indirect_delta,
        &shadow_selection,
        &direct_delta,
        &animated_direct_delta,
        &billboard,
        &animated_billboard,
    );
    let id50 = match id50_body {
        Id50Body::Valid => valid_id50(
            &cluster_directory,
            &base,
            &direct,
            &indirect_delta,
            &direct_delta,
            &animated_direct_delta,
        ),
        Id50Body::Malformed => vec![0],
    };
    let id50_metadata_len = ClusterShPayloadsSection::metadata_len_from_header(&id50).unwrap_or(0);

    let sections = vec![
        blob(
            SectionId::Geometry,
            1,
            GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            }
            .to_bytes(),
        ),
        blob(SectionId::Bvh, 1, bvh.to_bytes()),
        blob(SectionId::Cells, 1, cells.to_bytes()),
        blob(SectionId::CellLocator, 1, locator.to_bytes()),
        blob(SectionId::DeltaShVolumes, 1, indirect_delta.to_bytes()),
        blob(SectionId::OctahedralShVolume, 1, base.to_bytes()),
        blob(SectionId::DirectShVolume, 1, direct.to_bytes()),
        blob(SectionId::AlphaLights, 1, alpha_lights.to_bytes()),
        blob(
            SectionId::EntityShadowLights,
            1,
            shadow_selection.to_bytes(),
        ),
        blob(SectionId::DirectShDeltaVolumes, 1, direct_delta.to_bytes()),
        blob(
            SectionId::AnimatedDirectShDeltaVolumes,
            1,
            animated_direct_delta.to_bytes(),
        ),
        blob(
            SectionId::BillboardDirectScatterVolume,
            1,
            billboard.to_bytes(),
        ),
        blob(
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            1,
            animated_billboard.to_bytes(),
        ),
        blob(
            SectionId::ClusterDirectory,
            CLUSTER_DIRECTORY_CONTAINER_VERSION,
            cluster_directory.try_to_bytes().unwrap(),
        ),
        blob(
            SectionId::ClusterShPayloads,
            CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            id50,
        ),
        blob(
            SectionId::TextureCacheKeys,
            1,
            TextureCacheKeysSection::default().to_bytes(),
        ),
        blob(
            SectionId::FogVolumes,
            1,
            FogVolumesSection::default().to_bytes(),
        ),
    ];

    let mut file = std::fs::File::create(&path).unwrap();
    write_prl(&mut file, &sections).unwrap();
    StreamFixture {
        _directory: directory,
        path,
        large_atlas_bytes: base.compact_atlas.len(),
        id50_metadata_len,
    }
}

fn blob(section_id: SectionId, version: u16, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version,
        data,
    }
}

fn fixture_cells() -> CellsSection {
    CellsSection {
        cells: vec![
            CellRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [16.0, 16.0, 4.0],
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            },
            CellRecord {
                bounds_min: [20.0, 0.0, 0.0],
                bounds_max: [21.0, 1.0, 1.0],
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            },
        ],
        portal_refs: Vec::new(),
    }
}

fn fixture_locator() -> CellLocatorSection {
    CellLocatorSection {
        root: CellLocatorChild::Node(0),
        nodes: vec![CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: 17.0,
            front: CellLocatorChild::Cell(1),
            back: CellLocatorChild::Cell(0),
        }],
    }
}

fn fixture_alpha_lights() -> AlphaLightsSection {
    AlphaLightsSection {
        lights: vec![AlphaLightRecord {
            origin: [0.0, 0.0, 0.0],
            light_type: AlphaLightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: AlphaFalloffModel::Linear,
            falloff_range: 8.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
            shadow_type: AlphaShadowType::StaticLightMap,
        }],
    }
}

fn large_base_section() -> OctahedralShVolumeSection {
    let layout =
        irradiance_atlas_array_layout([STORED_TILES, 1, 1], 6, MAX_SH_ATLAS_DIMENSION).unwrap();
    let atlas_len = layout.layer_count as usize
        * layout.atlas_width as usize
        * layout.atlas_height as usize
        * 8;
    OctahedralShVolumeSection {
        grid_origin: [0.5, 0.5, 0.5],
        cell_size: [1.0, 1.0, 1.0],
        grid_dimensions: GRID_DIMENSIONS,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [layout.atlas_width, layout.atlas_height],
        layer_count: layout.layer_count,
        tiles_per_layer: layout.tiles_per_layer,
        atlas_tiles_per_row: layout.atlas_tiles_per_row,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: 0x3c00,
                mean_sq_distance: 0x4000,
                density_level: 0,
                node_scale: 0,
            };
            PROBE_COUNT as usize
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: vec![0xa5; atlas_len],
        animation_descriptors: vec![AnimationDescriptor::default()],
        slot_for_map_light: Vec::new(),
    }
}

fn direct_section(base: &OctahedralShVolumeSection) -> DirectShVolumeSection {
    DirectShVolumeSection {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_dimensions: base.atlas_dimensions,
        layer_count: base.layer_count,
        tiles_per_layer: base.tiles_per_layer,
        atlas_tiles_per_row: base.atlas_tiles_per_row,
        irradiance_format: base.irradiance_format,
        atlas: vec![0x5a; base.compact_atlas.len()],
    }
}

fn indirect_delta_section() -> DeltaShVolumesSection {
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: AFFINITY_DIMENSIONS,
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: vec![u64::MAX; AFFINITY_COUNT as usize],
        cell_levels: vec![0; AFFINITY_COUNT as usize],
        affinity_offsets: vec![0; AFFINITY_COUNT as usize + 1],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
}

fn direct_delta_section() -> DirectShDeltaVolumesSection {
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: AFFINITY_DIMENSIONS,
        tile_dimension: 6,
        tile_border: 1,
        valid_probe_masks: vec![u64::MAX; AFFINITY_COUNT as usize],
        cell_levels: vec![0; AFFINITY_COUNT as usize],
        affinity_offsets: one_entry_offsets(),
        affinity_lights: vec![0],
        delta_subblocks: vec![0; sparse_tile_f16_count()],
    }
}

fn animated_direct_delta_section() -> AnimatedDirectShDeltaVolumesSection {
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: AFFINITY_DIMENSIONS,
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: vec![0],
        valid_probe_masks: vec![u64::MAX; AFFINITY_COUNT as usize],
        cell_levels: vec![0; AFFINITY_COUNT as usize],
        affinity_offsets: one_entry_offsets(),
        affinity_lights: vec![0],
        delta_subblocks: vec![0; sparse_tile_f16_count()],
    }
}

fn billboard_section() -> BillboardDirectScatterVolumeSection {
    let mut scatter_rgba = Vec::with_capacity(PROBE_COUNT as usize * 4);
    for _ in 0..PROBE_COUNT {
        scatter_rgba.extend([0, 0, 0, BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16]);
    }
    BillboardDirectScatterVolumeSection {
        grid_origin: [0.5, 0.5, 0.5],
        cell_size: [1.0, 1.0, 1.0],
        grid_dimensions: GRID_DIMENSIONS,
        scatter_rgba,
    }
}

fn animated_billboard_section() -> AnimatedBillboardDirectScatterDeltaVolumesSection {
    AnimatedBillboardDirectScatterDeltaVolumesSection {
        animation_descriptor_indices: vec![0],
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: AFFINITY_DIMENSIONS,
        affinity_offsets: one_entry_offsets(),
        affinity_lights: vec![0],
        delta_rgba: vec![0; 64 * 4],
    }
}

#[allow(clippy::too_many_arguments)]
fn cluster_directory(
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    locator: &CellLocatorSection,
    base: &OctahedralShVolumeSection,
    direct: &DirectShVolumeSection,
    indirect_delta: &DeltaShVolumesSection,
    shadow_selection: &EntityShadowLightsSection,
    direct_delta: &DirectShDeltaVolumesSection,
    animated_direct_delta: &AnimatedDirectShDeltaVolumesSection,
    billboard: &BillboardDirectScatterVolumeSection,
    animated_billboard: &AnimatedBillboardDirectScatterDeltaVolumesSection,
) -> ClusterDirectorySection {
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [16.0, 16.0, 4.0],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [20.0, 0.0, 0.0],
                bounds_max: [21.0, 1.0, 1.0],
                member_start: 1,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            },
        ],
        resources: Vec::new(),
        members: vec![0, 1],
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let inputs = ClusterDirectoryValidationInputs {
        cells,
        portals,
        bvh,
        cell_locator: locator,
        sh: ClusterDirectoryShInventory {
            octahedral: Some(base),
            direct: Some(direct),
            delta: Some(indirect_delta),
            shadow_selection: Some(shadow_selection),
            direct_delta: Some(direct_delta),
            animated_direct_delta: Some(animated_direct_delta),
            billboard: Some(billboard),
            animated_billboard_delta: Some(animated_billboard),
        },
    };
    populate_canonical_resource_ranges(&mut directory, inputs).unwrap();
    directory.validate_semantics(inputs).unwrap();
    assert_eq!(directory.clusters[0].range_count, 7);
    assert_eq!(directory.clusters[1].range_count, 0);
    directory
}

fn valid_id50(
    directory: &ClusterDirectorySection,
    base: &OctahedralShVolumeSection,
    direct: &DirectShVolumeSection,
    indirect_delta: &DeltaShVolumesSection,
    direct_delta: &DirectShDeltaVolumesSection,
    animated_direct_delta: &AnimatedDirectShDeltaVolumesSection,
) -> Vec<u8> {
    let sources = source_metadata_from_sections(
        base,
        Some(direct),
        Some(indirect_delta),
        Some(direct_delta),
        Some(animated_direct_delta),
    );
    let chunk = dense_and_empty_sparse_chunk();
    let payload = chunk.clone();
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 2,
            source_count: sources.len() as u32,
            grid_dimensions: GRID_DIMENSIONS,
            affinity_dimensions: AFFINITY_DIMENSIONS,
            payload_bytes: payload.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![
            ClusterShPayloadsIndexRecord {
                payload_offset: 0,
                payload_len: chunk.len() as u64,
                decoded_bytes: decoded_chunk_bytes(),
                requested_resident_bytes: u64::from(STORED_TILES) * 2 * 8 * 8 * 8
                    + 2 * (16 + sparse_tile_f16_count() as u64 * 2),
                stored_tile_count: STORED_TILES,
                dense_patch_count: PROBE_COUNT,
                affinity_patch_count: AFFINITY_COUNT * 3,
                hash: *blake3::hash(&chunk).as_bytes(),
            },
            ClusterShPayloadsIndexRecord {
                payload_offset: chunk.len() as u64,
                payload_len: 0,
                decoded_bytes: 0,
                requested_resident_bytes: 0,
                stored_tile_count: 0,
                dense_patch_count: 0,
                affinity_patch_count: 0,
                hash: *blake3::hash(&[]).as_bytes(),
            },
        ],
    };
    section
        .validate_against(ClusterShPayloadsValidationInputs {
            directory,
            base: base.into(),
            sources: &sources,
        })
        .unwrap();
    section.try_to_bytes(&payload).unwrap()
}

fn dense_and_empty_sparse_chunk() -> Vec<u8> {
    let isolated_layout = cluster_sh_isolated_atlas_array_layout(STORED_TILES).unwrap();
    let isolated_atlas_len = isolated_layout.layer_count as usize
        * isolated_layout.atlas_width as usize
        * isolated_layout.atlas_height as usize
        * 8;
    let blocks = vec![
        (
            SectionId::DeltaShVolumes,
            3,
            AFFINITY_COUNT,
            empty_sparse_rows(),
        ),
        (
            SectionId::OctahedralShVolume,
            0,
            PROBE_COUNT,
            probe_patches(),
        ),
        (
            SectionId::OctahedralShVolume,
            1,
            STORED_TILES,
            isolated_atlas_body(&isolated_layout, isolated_atlas_len),
        ),
        (
            SectionId::DirectShVolume,
            1,
            STORED_TILES,
            isolated_atlas_body(&isolated_layout, isolated_atlas_len),
        ),
        (
            SectionId::DirectShDeltaVolumes,
            3,
            AFFINITY_COUNT,
            one_entry_sparse_rows(),
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            3,
            AFFINITY_COUNT,
            one_entry_sparse_rows(),
        ),
    ];
    let table_len = 16 + blocks.len() * 32;
    let body_len: usize = blocks.iter().map(|(_, _, _, body)| body.len()).sum();
    let mut bytes = Vec::with_capacity(table_len + body_len);
    push_u32(&mut bytes, CLUSTER_SH_PAYLOADS_VERSION);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, blocks.len() as u32);
    push_u32(&mut bytes, 0);
    let mut offset = table_len as u64;
    for (section_id, kind, count, body) in &blocks {
        push_u32(&mut bytes, *section_id as u32);
        push_u32(&mut bytes, *kind);
        push_u32(&mut bytes, *count);
        push_u32(&mut bytes, 0);
        push_u64(&mut bytes, offset);
        push_u64(&mut bytes, body.len() as u64);
        offset += body.len() as u64;
    }
    for (_, _, _, body) in blocks {
        bytes.extend_from_slice(&body);
    }
    bytes
}

fn decoded_chunk_bytes() -> u64 {
    let isolated_layout = cluster_sh_isolated_atlas_array_layout(STORED_TILES).unwrap();
    let isolated_atlas_body_len = 20
        + isolated_layout.layer_count as u64
            * isolated_layout.atlas_width as u64
            * isolated_layout.atlas_height as u64
            * 8;
    u64::from(PROBE_COUNT) * 16
        + 2 * isolated_atlas_body_len
        + sparse_rows_len() as u64
        + 2 * one_entry_sparse_rows_len() as u64
}

fn probe_patches() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PROBE_COUNT as usize * 16);
    for z in 0..GRID_DIMENSIONS[2] {
        for y in 0..GRID_DIMENSIONS[1] {
            for x in 0..GRID_DIMENSIONS[0] {
                let dense_index = x + GRID_DIMENSIONS[0] * (y + GRID_DIMENSIONS[1] * z);
                let brick = x / 4 + AFFINITY_DIMENSIONS[0] * (y / 4);
                let local = x % 4 + 4 * (y % 4) + 16 * z;
                let rank = brick * 64 + local;
                push_u32(&mut bytes, dense_index);
                push_u32(&mut bytes, 0x0000_0004 | (rank << 5));
                bytes.extend_from_slice(&0x3c00u16.to_le_bytes());
                bytes.extend_from_slice(&0x4000u16.to_le_bytes());
                push_u32(&mut bytes, 0);
            }
        }
    }
    bytes
}

fn isolated_atlas_body(
    layout: &postretro_level_format::octahedral::IrradianceAtlasArrayLayout,
    atlas_len: usize,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(20 + atlas_len);
    push_u32(&mut bytes, IRRADIANCE_FORMAT_RGBA16F);
    push_u32(&mut bytes, STORED_TILES);
    push_u32(&mut bytes, layout.atlas_width);
    push_u32(&mut bytes, layout.atlas_height);
    push_u32(&mut bytes, layout.layer_count);
    bytes.resize(20 + atlas_len, 0);
    bytes
}

fn empty_sparse_rows() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(sparse_rows_len());
    push_u32(&mut bytes, AFFINITY_COUNT);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    for row in 0..AFFINITY_COUNT {
        push_u32(&mut bytes, row);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 1);
    }
    bytes
}

fn sparse_rows_len() -> usize {
    16 + AFFINITY_COUNT as usize * 16
}

fn one_entry_sparse_rows() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(one_entry_sparse_rows_len());
    push_u32(&mut bytes, AFFINITY_COUNT);
    push_u32(&mut bytes, 1);
    push_u32(&mut bytes, sparse_tile_f16_count() as u32);
    push_u32(&mut bytes, 0);
    for row in 0..AFFINITY_COUNT {
        push_u32(&mut bytes, row);
        push_u32(&mut bytes, u32::from(row != 0));
        push_u32(&mut bytes, u32::from(row == 0));
        push_u32(&mut bytes, 1);
    }
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, sparse_tile_f16_count() as u32);
    push_u32(&mut bytes, 0);
    bytes.resize(bytes.len() + sparse_tile_f16_count() * 2, 0);
    bytes
}

fn sparse_tile_f16_count() -> usize {
    64 * 6 * 6 * 3
}

fn one_entry_offsets() -> Vec<u32> {
    let mut offsets = vec![1; AFFINITY_COUNT as usize + 1];
    offsets[0] = 0;
    offsets
}

fn one_entry_sparse_rows_len() -> usize {
    sparse_rows_len() + 16 + sparse_tile_f16_count() * 2
}

fn streamed_body_ranges(fixture: &StreamFixture) -> Vec<Range<u64>> {
    let mut file = std::fs::File::open(&fixture.path).unwrap();
    let table = read_container(&mut file).unwrap();
    let entry = |section| table.find_section(section as u32).unwrap();
    let base = entry(SectionId::OctahedralShVolume);
    let direct = entry(SectionId::DirectShVolume);
    let indirect_delta = entry(SectionId::DeltaShVolumes);
    let direct_delta = entry(SectionId::DirectShDeltaVolumes);
    let animated_direct_delta = entry(SectionId::AnimatedDirectShDeltaVolumes);
    let payloads = entry(SectionId::ClusterShPayloads);
    let base_body_start = base.offset + 84 + u64::from(PROBE_COUNT) * 8;
    let direct_body_start = direct.offset + 76;
    // Each affinity row has an eight-byte mask and a one-byte level. The
    // offsets are a separate (count + 1)-element table, rather than four
    // bytes per row.
    let sparse_metadata_len = u64::from(AFFINITY_COUNT) * 9 + 4 * 17;
    let indirect_delta_body_start = indirect_delta.offset + 26 + sparse_metadata_len;
    let direct_delta_body_start = direct_delta.offset + 22 + sparse_metadata_len + 4;
    let animated_direct_delta_body_start =
        animated_direct_delta.offset + 26 + 4 + sparse_metadata_len + 4;
    vec![
        base_body_start..base_body_start + fixture.large_atlas_bytes as u64,
        direct_body_start..direct.offset + direct.size,
        indirect_delta_body_start..indirect_delta.offset + indirect_delta.size,
        direct_delta_body_start..direct_delta.offset + direct_delta.size,
        animated_direct_delta_body_start..animated_direct_delta.offset + animated_direct_delta.size,
        payloads.offset + fixture.id50_metadata_len as u64..payloads.offset + payloads.size,
    ]
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
