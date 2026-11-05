//! Storage two clusters share: chunks agree on it, and the compiler counts it
//! in the units the runtime derives.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use postretro_level_format::SectionId;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection, ClusterRangeRecord, ClusterRecord,
    ClusterResourceDomain, ClusterResourceRecord,
};
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_LOGICAL_TILE_DIMENSION, ClusterShPayloadsBaseMetadata,
    ClusterShPayloadsSourceMetadata, ClusterShPayloadsValidationInputs,
    cluster_sh_isolated_atlas_array_layout, source_metadata_from_sections,
};
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, DeltaShVolumesSection,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::octahedral::{
    MAX_SH_ATLAS_DIMENSION, irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_reconstruct::Level;
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_level_format::{read_container, read_section_data};
use postretro_level_loader::{
    ShStreamBaseMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};
use postretro_render_cpu::sh_residency::{
    ShResidencyPlan, plan_sh_residency, plan_sh_residency_parts, sparse_row_demand,
};

use super::super::sh_mandatory_bound::{ClusterStorageDemand, sh_mandatory_bound};
use super::atlas::DenseAtlasGeometry;
use super::dense::{ChunkPlan, DenseNode};
use super::shared_bytes::SharedBytesCheck;
use super::sparse::SparseSources;
use super::wire::EncodedBlock;

const GRID: [u32; 3] = [4, 4, 8];
const NODE_PROBES: u32 = 64;
const PROBES: u32 = NODE_PROBES * 2;

/// Two L0 nodes, one tile per probe: node 0 is probes 0..64, node 1 64..128.
fn source_atlas() -> OctahedralShVolumeSection {
    let layout = irradiance_atlas_array_layout(
        [PROBES, 1, 1],
        CLUSTER_SH_LOGICAL_TILE_DIMENSION,
        MAX_SH_ATLAS_DIMENSION,
    )
    .unwrap();
    let byte_len = (layout.layer_count * layout.atlas_width * layout.atlas_height) as usize * 8;
    let mut compact_atlas = vec![0; byte_len];
    for (index, texel) in compact_atlas.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let value = u16::try_from(index % 0x3c00).unwrap().to_le_bytes();
        for channel in texel.as_chunks_mut::<2>().0 {
            channel.copy_from_slice(&value);
        }
    }
    OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: GRID,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
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
                density_level: Level::L0.to_u8(),
                node_scale: 0,
            };
            PROBES as usize
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas,
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

/// Stored tile `slot`'s first texel in the source atlas, for tampering.
fn tile_byte(atlas: &OctahedralShVolumeSection, slot: u32) -> usize {
    let [layer, x, y] = irradiance_array_tile_location(
        slot as usize,
        atlas.tiles_per_layer,
        atlas.atlas_tiles_per_row,
    );
    let width = atlas.atlas_dimensions[0] as usize;
    let height = atlas.atlas_dimensions[1] as usize;
    ((layer as usize * height + y as usize * 6) * width + x as usize * 6) * 8
}

/// Light tile `slot`'s first texel far past its neighbours, so even BC6H,
/// which quantizes small differences away, stores the tile differently.
fn brighten_tile(atlas: &mut OctahedralShVolumeSection, slot: u32) {
    let at = tile_byte(atlas, slot);
    for channel in atlas.compact_atlas[at..at + 8].as_chunks_mut::<2>().0 {
        channel.copy_from_slice(&0x4c00u16.to_le_bytes());
    }
}

/// Row 0 holds one light and row 1 two, so a row's cursor depends on whether
/// its chunk also carries row 0.
fn delta(value: u16) -> DeltaShVolumesSection {
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 2],
        tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
        tile_border: 1,
        animation_descriptor_indices: vec![0; 3],
        valid_probe_masks: vec![u64::MAX; 2],
        cell_levels: vec![Level::L0.to_u8(); 2],
        affinity_offsets: vec![0, 1, 3],
        affinity_lights: vec![0, 1, 2],
        delta_subblocks: vec![value; 3 * 64 * DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

fn direct_delta() -> DirectShDeltaVolumesSection {
    let delta = delta(0x3555);
    DirectShDeltaVolumesSection {
        affinity_factor: delta.affinity_factor,
        affinity_dims: delta.affinity_dims,
        tile_dimension: delta.tile_dimension,
        tile_border: delta.tile_border,
        valid_probe_masks: delta.valid_probe_masks,
        cell_levels: delta.cell_levels,
        affinity_offsets: vec![0, 0, 2],
        affinity_lights: vec![4, 5],
        delta_subblocks: vec![0x3555; 2 * 64 * DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

const DELTA_RESOURCE: u32 = 0;
const BASE_RESOURCE: u32 = 1;
const DIRECT_DELTA_RESOURCE: u32 = 2;

/// Cluster 0 carries node 1 and row 1 alone. Cluster 1 carries both nodes and
/// both rows, so it holds node 1 at another local rank, in an atlas of another
/// size, and row 1 at another cursor. Cluster 2 carries node 0.
fn directory() -> ClusterDirectorySection {
    let dense = |start, count| ClusterRangeRecord {
        resource_index: BASE_RESOURCE,
        start,
        count,
    };
    let rows = |resource_index, start, count| ClusterRangeRecord {
        resource_index,
        start,
        count,
    };
    let per_cluster = [
        vec![
            rows(DELTA_RESOURCE, 1, 1),
            dense(NODE_PROBES, NODE_PROBES),
            rows(DIRECT_DELTA_RESOURCE, 1, 1),
        ],
        vec![
            rows(DELTA_RESOURCE, 0, 2),
            dense(0, PROBES),
            rows(DIRECT_DELTA_RESOURCE, 0, 2),
        ],
        vec![dense(0, NODE_PROBES)],
    ];
    let mut clusters = Vec::new();
    let mut ranges = Vec::new();
    for (cluster, cluster_ranges) in per_cluster.into_iter().enumerate() {
        clusters.push(ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: cluster as u32,
            member_count: 1,
            range_start: ranges.len() as u32,
            range_count: cluster_ranges.len() as u32,
            primitive_count: 0,
            flags: 0,
        });
        ranges.extend(cluster_ranges);
    }
    ClusterDirectorySection {
        runtime_cell_count: 3,
        primitive_limit: 64,
        cell_limit: 32,
        clusters,
        resources: vec![
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 2],
            },
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: GRID,
            },
            ClusterResourceRecord {
                section_id: SectionId::DirectShDeltaVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 2],
            },
        ],
        members: vec![0, 1, 2],
        ranges,
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
        mandatory_bound: Vec::new(),
    }
}

/// Build a plan over `delta` at `format` and hand it to `with`. Id 34's format
/// names what chunks encode to; the atlas each chunk slices stays RGBA16F.
fn with_plan<T>(
    directory: &ClusterDirectorySection,
    base: &OctahedralShVolumeSection,
    delta: &DeltaShVolumesSection,
    format: u32,
    with: impl FnOnce(&ChunkPlan<'_>) -> T,
) -> T {
    let direct_delta = direct_delta();
    let sources: Vec<ClusterShPayloadsSourceMetadata<'_>> =
        source_metadata_from_sections(base, None, Some(delta), Some(&direct_delta), None);
    let inputs = ClusterShPayloadsValidationInputs {
        directory,
        base: ClusterShPayloadsBaseMetadata {
            irradiance_format: format,
            ..ClusterShPayloadsBaseMetadata::from(base)
        },
        sources: &sources,
    };
    let sparse = SparseSources::from_sections(Some(delta), Some(&direct_delta), None).unwrap();
    with(&ChunkPlan::new(inputs, sparse).unwrap())
}

fn encode(
    plan: &ChunkPlan<'_>,
    cluster: u32,
    atlas: &OctahedralShVolumeSection,
    check: &mut SharedBytesCheck,
) -> anyhow::Result<()> {
    plan.encode_chunk(cluster, atlas, None, check).map(|_| ())
}

#[test]
fn agreeing_chunks_pass_at_other_ranks_cursors_and_atlas_sizes() {
    let directory = directory();
    let base = source_atlas();
    for format in [IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_FORMAT_BC6H] {
        with_plan(&directory, &base, &delta(0x3c00), format, |plan| {
            let mut check = SharedBytesCheck::default();
            for cluster in 0..3 {
                encode(plan, cluster, &base, &mut check).unwrap();
            }
        });
    }
}

#[test]
fn disagreeing_chunks_fail_the_build_naming_both() {
    let directory = directory();
    let base = source_atlas();
    let mut other = base.clone();
    // A texel of node 1, which clusters 0 and 1 both carry.
    brighten_tile(&mut other, NODE_PROBES + 3);

    for format in [IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_FORMAT_BC6H] {
        with_plan(&directory, &base, &delta(0x3c00), format, |plan| {
            let mut check = SharedBytesCheck::default();
            encode(plan, 0, &base, &mut check).unwrap();
            let error = encode(plan, 1, &other, &mut check).unwrap_err().to_string();
            assert!(error.contains("clusters 0 and 1"), "{error}");
            assert!(error.contains("dense node 1"), "{error}");
        });
    }
}

#[test]
fn a_tile_only_one_chunk_carries_cannot_disagree() {
    let directory = directory();
    let base = source_atlas();
    let mut other = base.clone();
    brighten_tile(&mut other, 3);

    with_plan(
        &directory,
        &base,
        &delta(0x3c00),
        IRRADIANCE_FORMAT_RGBA16F,
        |plan| {
            let mut check = SharedBytesCheck::default();
            encode(plan, 0, &base, &mut check).unwrap();
            encode(plan, 1, &other, &mut check).unwrap();
        },
    );
}

#[test]
fn chunks_that_disagree_on_a_sparse_row_fail_naming_both() {
    let directory = directory();
    let base = source_atlas();
    let mut check = SharedBytesCheck::default();
    with_plan(
        &directory,
        &base,
        &delta(0x3c00),
        IRRADIANCE_FORMAT_RGBA16F,
        |plan| {
            encode(plan, 0, &base, &mut check).unwrap();
        },
    );
    // Same CSR shape, other tile values, so the second chunk disagrees on the
    // row and on nothing else.
    with_plan(
        &directory,
        &base,
        &delta(0x3555),
        IRRADIANCE_FORMAT_RGBA16F,
        |plan| {
            let error = encode(plan, 1, &base, &mut check).unwrap_err().to_string();
            assert!(error.contains("clusters 0 and 1"), "{error}");
            assert!(
                error.contains(&format!(
                    "sparse row 1 of section {}",
                    SectionId::DeltaShVolumes as u32
                )),
                "{error}"
            );
        },
    );
}

#[test]
fn a_chunks_atlas_padding_never_reaches_a_node_hash() {
    // Three tiles in a layout with room for more: everything past them is
    // padding, which only this chunk's slot count put there.
    let slots = 3;
    let layout = cluster_sh_isolated_atlas_array_layout(slots).unwrap();
    assert!(layout.tiles_per_layer * layout.layer_count > slots);
    let base = source_atlas();
    let node = DenseNode {
        index: 0,
        global_base: 0,
        local_base: 0,
        stored_tile_count: slots,
    };
    for format in [IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_FORMAT_BC6H] {
        let block = EncodedBlock::isolated_atlas(
            SectionId::OctahedralShVolume as u32,
            format,
            slots,
            &base.compact_atlas,
            DenseAtlasGeometry::from_octahedral(&base),
            std::slice::from_ref(&node),
        )
        .unwrap();
        let hash = |block: &EncodedBlock| {
            let mut hasher = blake3::Hasher::new();
            block.hash_node_tiles(&node, &mut hasher).unwrap();
            hasher.finalize()
        };
        let original = hash(&block);

        let mut padded = EncodedBlock {
            body: block.body.clone(),
            ..block
        };
        *padded.body.last_mut().unwrap() ^= 0xff;
        assert_eq!(hash(&padded), original, "format {format}: padding hashed");

        // The first payload byte is tile 0, past the 20-byte header.
        padded.body[20] ^= 0xff;
        assert_ne!(hash(&padded), original, "format {format}: tile not hashed");
    }
}

/// One cluster's demand in the pools' units.
#[derive(Debug, PartialEq, Eq)]
struct ClusterFigures {
    dense_slots: u64,
    /// `(entries, tile_f16)` by section id.
    sparse: BTreeMap<u32, (u64, u64)>,
}

/// The compiler's view of `cluster`.
fn compiler_figures(
    directory: &ClusterDirectorySection,
    demand: &ClusterStorageDemand,
    cluster: u32,
) -> ClusterFigures {
    ClusterFigures {
        dense_slots: demand
            .cluster_nodes
            .of(cluster)
            .iter()
            .map(|&node| u64::from(demand.node_slots[node as usize]))
            .sum(),
        sparse: demand
            .families
            .iter()
            .filter_map(|family| {
                let rows = family.cluster_rows.of(cluster);
                (!rows.is_empty()).then(|| {
                    let section = directory.resources[family.resource_index as usize].section_id;
                    let units = rows.iter().fold((0, 0), |total, &row| {
                        let (entries, tile_f16) = family.row_units[row as usize];
                        (total.0 + u64::from(entries), total.1 + u64::from(tile_f16))
                    });
                    (section, units)
                })
            })
            .collect(),
    }
}

/// The runtime's view of `clusters`' union, from the residency plan alone:
/// shared storage counts once because a stored node or row has one live copy.
fn runtime_union_figures(
    plan: &ShResidencyPlan,
    sources: &ShStreamSourceMetadata,
    clusters: impl IntoIterator<Item = u32>,
) -> ClusterFigures {
    let mut nodes = BTreeSet::new();
    let mut rows = BTreeSet::new();
    for cluster in clusters {
        let storage = &plan.cluster_storage[cluster as usize];
        for range in &storage.dense {
            nodes.extend(
                plan.dense_node[range.start as usize..range.end as usize]
                    .iter()
                    .flatten()
                    .copied(),
            );
        }
        for (section, range) in &storage.sparse {
            rows.extend(range.clone().map(|row| (*section, row)));
        }
    }
    let mut sparse = BTreeMap::<u32, (u64, u64)>::new();
    for (section, row) in rows {
        let (entries, tile_f16) = sparse_row_demand(sources, 0, section, row).unwrap();
        let total = sparse.entry(section).or_default();
        total.0 += u64::from(entries);
        total.1 += u64::from(tile_f16);
    }
    ClusterFigures {
        dense_slots: nodes
            .iter()
            .map(|node| u64::from(plan.node_layouts.get(node).unwrap().tile_count))
            .sum(),
        sparse,
    }
}

fn sparse_metadata(
    section_id: u32,
    affinity_dims: [u32; 3],
    masks: &[u64],
    levels: &[u8],
    offsets: &[u32],
    lights: &[u32],
) -> ShStreamSparseMetadata {
    ShStreamSparseMetadata {
        section_id,
        internal_version: 1,
        affinity_dims,
        tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
        tile_border: 1,
        valid_probe_masks: masks.to_vec(),
        cell_levels: levels.to_vec(),
        affinity_offsets: offsets.to_vec(),
        affinity_lights: lights.to_vec(),
        animation_descriptor_indices: Vec::new(),
    }
}

// The bound's contract is "physical slots and sparse units": a compiler figure
// in any other unit would size the pool wrong with every test above still green.
#[test]
fn compiler_cluster_demand_equals_what_the_runtime_derives() {
    let directory = directory();
    let base = source_atlas();
    let delta = delta(0x3c00);
    let direct_delta = direct_delta();
    let demand = with_plan(
        &directory,
        &base,
        &delta,
        IRRADIANCE_FORMAT_RGBA16F,
        |plan| plan.storage_demand().unwrap(),
    );

    let runtime_base = ShStreamBaseMetadata {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        probe_stride: base.probe_stride,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_dimensions: base.atlas_dimensions,
        layer_count: base.layer_count,
        tiles_per_layer: base.tiles_per_layer,
        atlas_tiles_per_row: base.atlas_tiles_per_row,
        irradiance_format: base.irradiance_format,
        probes: base.probes.clone(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    };
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_metadata(
            SectionId::DeltaShVolumes as u32,
            delta.affinity_dims,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            &delta.affinity_offsets,
            &delta.affinity_lights,
        )),
        direct: None,
        direct_delta: Some(sparse_metadata(
            SectionId::DirectShDeltaVolumes as u32,
            direct_delta.affinity_dims,
            &direct_delta.valid_probe_masks,
            &direct_delta.cell_levels,
            &direct_delta.affinity_offsets,
            &direct_delta.affinity_lights,
        )),
        animated_direct_delta: None,
    };
    let plan = plan_sh_residency_parts([0; 32], &directory, &runtime_base, &sources, &[]).unwrap();

    for cluster in 0..3 {
        assert_eq!(
            compiler_figures(&directory, &demand, cluster),
            runtime_union_figures(&plan, &sources, [cluster]),
            "cluster {cluster}"
        );
    }
    // A nonzero fixture: 64 tiles per node, and rows of real size.
    assert_eq!(compiler_figures(&directory, &demand, 1).dense_slots, 128);
    assert_eq!(
        compiler_figures(&directory, &demand, 0).sparse[&(SectionId::DeltaShVolumes as u32)],
        (2, 2 * 64 * DEFAULT_DELTA_PROBE_F16_STRIDE as u64)
    );
}

// Id 48 is affinity-domain, but stays whole-resident outside the sparse pools.
#[test]
fn billboard_scatter_is_not_a_pool_family() {
    let mut directory = directory();
    directory.resources.push(ClusterResourceRecord {
        section_id: SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
        domain: ClusterResourceDomain::AffinityCell,
        dimensions: [1, 1, 2],
    });
    let base = source_atlas();

    let demand = with_plan(
        &directory,
        &base,
        &delta(0x3c00),
        IRRADIANCE_FORMAT_RGBA16F,
        |plan| plan.storage_demand().unwrap(),
    );

    let families: Vec<u32> = demand.families.iter().map(|f| f.resource_index).collect();
    assert_eq!(families, [DELTA_RESOURCE, DIRECT_DELTA_RESOURCE]);
}

fn committed_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl")
}

// The compiled level end to end: what id 49 stores against what the loader and
// the residency plan derive, cell by cell, with the fixture's real pin.
#[test]
fn stored_bound_equals_the_runtime_union_on_the_compiled_fixture() {
    let path = committed_fixture();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = world.sh_stream_manifest().expect("the fixture streams SH");
    let plan = plan_sh_residency(manifest, &world.cells).unwrap();
    let residency = world
        .cell_residency_set
        .as_ref()
        .expect("the fixture has id 51");
    let directory = world.cluster_directory().expect("the fixture has id 49");
    let stored = &directory.mandatory_bound;

    let mut cell_to_cluster = vec![0; directory.runtime_cell_count as usize];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let start = cluster.member_start as usize;
        for &cell in &directory.members[start..start + cluster.member_count as usize] {
            cell_to_cluster[cell as usize] = cluster_id as u32;
        }
    }
    let pinned: Vec<u32> = directory
        .cluster_hints
        .iter()
        .filter(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0)
        .map(|hint| hint.cluster_id)
        .collect();
    assert!(!pinned.is_empty(), "the fixture must exercise a pin");
    assert_eq!(stored.len(), 2, "default lead and baked maximum");

    for entry in stored {
        let mut worst = 0;
        let mut worst_cell = None;
        for cell in 0..world.cells.len() {
            let data = &world.cells[cell];
            if data.is_solid || data.is_exterior {
                continue;
            }
            let reached = residency
                .entries_for(cell)
                .iter()
                .filter(|reached| reached.lead <= entry.lead)
                .map(|reached| cell_to_cluster[reached.cell_id as usize]);
            let clusters: BTreeSet<u32> = reached.chain(pinned.iter().copied()).collect();
            let figures = runtime_union_figures(&plan, manifest.sources(), clusters);
            if worst_cell.is_none() || figures.dense_slots > worst {
                worst = figures.dense_slots;
                worst_cell = Some(cell as u32);
            }
        }
        assert_eq!(
            u64::from(entry.worst_dense_slots),
            worst,
            "lead {}",
            entry.lead
        );
        assert_eq!(Some(entry.worst_cell), worst_cell, "lead {}", entry.lead);
        assert!(worst > 0);
    }
}

// The same level through the compiler's own sets: the node sets lifted from
// `ChunkPlan` and the bound they give must reproduce what the bake stored.
#[test]
fn recomputed_bound_equals_the_stored_bound_on_the_compiled_fixture() {
    let mut reader = BufReader::new(File::open(committed_fixture()).unwrap());
    let meta = read_container(&mut reader).unwrap();
    let mut section = |id: SectionId| read_section_data(&mut reader, &meta, id as u32).unwrap();
    let directory =
        ClusterDirectorySection::from_bytes(&section(SectionId::ClusterDirectory).unwrap())
            .unwrap();
    let base =
        OctahedralShVolumeSection::from_bytes(&section(SectionId::OctahedralShVolume).unwrap())
            .unwrap();
    let delta = section(SectionId::DeltaShVolumes)
        .map(|bytes| DeltaShVolumesSection::from_bytes(&bytes).unwrap());
    let direct_delta = section(SectionId::DirectShDeltaVolumes)
        .map(|bytes| DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap());
    let animated = section(SectionId::AnimatedDirectShDeltaVolumes)
        .map(|bytes| AnimatedDirectShDeltaVolumesSection::from_bytes(&bytes).unwrap());
    let cells = CellsSection::from_bytes(&section(SectionId::Cells).unwrap()).unwrap();
    let residency = CellResidencySetSection::from_bytes(
        &section(SectionId::CellResidencySet).unwrap(),
        cells.cells.len(),
    )
    .unwrap();
    let sources = source_metadata_from_sections(
        &base,
        None,
        delta.as_ref(),
        direct_delta.as_ref(),
        animated.as_ref(),
    );
    let inputs = ClusterShPayloadsValidationInputs {
        directory: &directory,
        base: ClusterShPayloadsBaseMetadata::from(&base),
        sources: &sources,
    };
    let sparse =
        SparseSources::from_sections(delta.as_ref(), direct_delta.as_ref(), animated.as_ref())
            .unwrap();
    let demand = ChunkPlan::new(inputs, sparse)
        .unwrap()
        .storage_demand()
        .unwrap();

    let bound = sh_mandatory_bound(&directory, &cells, Some(&residency), &demand).unwrap();

    assert!(!bound.is_empty());
    assert_eq!(bound, directory.mandatory_bound);
}
