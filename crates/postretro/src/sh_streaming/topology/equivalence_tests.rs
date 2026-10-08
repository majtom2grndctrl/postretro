//! The array-backed owner derivation against the tree-keyed code it replaced.
//! `oracle_owners` is that code, kept verbatim. The build also runs over a
//! `BTreeMap` table through `from_manifest_view_with`, so a table fault and a
//! derivation fault show up separately. Errors are compared, not just results.
//! See: context/lib/testing_guide.md §4

use std::collections::BTreeMap;

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ClusterRangeRecord, ClusterRangeRole, ClusterRecord,
    ClusterResourceDomain, ClusterResourceRecord, DENSE_OWNER_SENTINEL,
};
use postretro_level_format::cluster_sh_payloads::{
    ClusterShPayloadsHeader, ClusterShPayloadsIndexRecord, ClusterShPayloadsSection,
    ClusterShPayloadsSourceKind, ClusterShPayloadsSourceRecord,
};
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, SH_VOLUME_VERSION,
};

use super::super::topology_nodes::{DenseNode, DenseNodeOwners, NodeOwners};
use super::*;
use crate::streaming::cluster_hints::ClusterHints;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u32) -> u32 {
        ((self.next() >> 33) % u64::from(bound)) as u32
    }

    fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
}

const GRID_SHAPES: [[u32; 3]; 8] = [
    [4, 4, 4],
    [8, 8, 8],
    [16, 16, 16],
    [5, 7, 9],
    [11, 6, 13],
    [23, 17, 14],
    [32, 8, 4],
    [1, 1, 1],
];

struct SyntheticManifest {
    directory: ClusterDirectorySection,
    payloads: ClusterShPayloadsSection,
    base: ShStreamBaseMetadata,
    adjacency: Vec<Vec<u32>>,
}

/// Knobs that decide which rejection paths a manifest can reach.
#[derive(Clone, Copy)]
struct Shape {
    /// Probe scales are drawn per probe, not per brick, so several scales
    /// share an origin brick.
    mixed_scales: bool,
    /// Largest scale drawn; 32 and above overflow the node span.
    max_scale: u32,
    /// Leave some valid probes outside every dense range.
    leave_gaps: bool,
    /// Add sparse halo ranges that may name any cluster as owner.
    sparse_halos: bool,
}

fn synthetic(rng: &mut Rng, dims: [u32; 3], cluster_count: u32, shape: Shape) -> SyntheticManifest {
    let probe_count = dims.iter().product::<u32>();
    let bricks = dims.map(|axis| axis.div_ceil(4));
    let brick_scale: Vec<u8> = (0..bricks.iter().product::<u32>())
        .map(|_| {
            if rng.chance(60) {
                0
            } else {
                rng.below(shape.max_scale + 1) as u8
            }
        })
        .collect();
    let mut probes = Vec::with_capacity(probe_count as usize);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let brick = (x / 4 + bricks[0] * (y / 4 + bricks[1] * (z / 4))) as usize;
                let node_scale = if shape.mixed_scales && rng.chance(30) {
                    rng.below(shape.max_scale + 1) as u8
                } else {
                    brick_scale[brick]
                };
                probes.push(OctahedralShProbe {
                    validity: u8::from(rng.chance(70)),
                    node_scale,
                    ..OctahedralShProbe::default()
                });
            }
        }
    }

    let mut ranges = Vec::new();
    let mut clusters = Vec::new();
    for cluster_id in 0..cluster_count {
        let first = ranges.len() as u32;
        if !shape.leave_gaps && cluster_id + 1 == cluster_count {
            ranges.push(dense_range(0, probe_count));
        }
        for _ in 0..1 + rng.below(3) {
            let start = rng.below(probe_count);
            ranges.push(dense_range(start, 1 + rng.below(probe_count - start)));
        }
        if shape.sparse_halos {
            for _ in 0..rng.below(3) {
                ranges.push(ClusterRangeRecord {
                    resource_index: 1,
                    start: 0,
                    count: 1,
                    owner_cluster_id: rng.below(cluster_count),
                    role: ClusterRangeRole::Halo,
                });
            }
        }
        clusters.push(ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 0,
            range_start: first,
            range_count: ranges.len() as u32 - first,
            primitive_count: 0,
            flags: 0,
        });
    }

    let mut sources = vec![ClusterShPayloadsSourceRecord {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
    }];
    if shape.sparse_halos {
        sources.push(ClusterShPayloadsSourceRecord {
            section_id: SectionId::DeltaShVolumes as u32,
            internal_version: 6,
            kind: ClusterShPayloadsSourceKind::SparseAffinity,
        });
    }
    SyntheticManifest {
        directory: ClusterDirectorySection {
            runtime_cell_count: 0,
            primitive_limit: 64,
            cell_limit: 32,
            clusters,
            resources: vec![
                ClusterResourceRecord {
                    section_id: SectionId::OctahedralShVolume as u32,
                    domain: ClusterResourceDomain::DenseProbe,
                    dimensions: dims,
                },
                ClusterResourceRecord {
                    section_id: SectionId::DeltaShVolumes as u32,
                    domain: ClusterResourceDomain::AffinityCell,
                    dimensions: [1, 1, 1],
                },
            ],
            members: Vec::new(),
            ranges,
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        },
        payloads: ClusterShPayloadsSection {
            header: ClusterShPayloadsHeader {
                cluster_count,
                source_count: sources.len() as u32,
                grid_dimensions: dims,
                affinity_dimensions: bricks,
                payload_bytes: 0,
            },
            sources,
            index: (0..cluster_count)
                .map(|cluster_id| ClusterShPayloadsIndexRecord {
                    payload_offset: 0,
                    payload_len: 0,
                    decoded_bytes: 0,
                    requested_resident_bytes: u64::from(cluster_id) + 1,
                    stored_tile_count: 0,
                    dense_patch_count: 0,
                    affinity_patch_count: 0,
                    hash: [cluster_id as u8; 32],
                })
                .collect(),
        },
        base: ShStreamBaseMetadata {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: dims,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: 6,
            tile_border: 1,
            atlas_dimensions: [12, 6],
            layer_count: 1,
            tiles_per_layer: 2,
            atlas_tiles_per_row: 2,
            irradiance_format: 0,
            probes,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        },
        adjacency: vec![Vec::new(); cluster_count as usize],
    }
}

fn dense_range(start: u32, count: u32) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index: 0,
        start,
        count,
        owner_cluster_id: DENSE_OWNER_SENTINEL,
        role: ClusterRangeRole::Dense,
    }
}

// ---- The pre-array owner derivation, kept verbatim as the oracle. ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OracleDenseNode {
    origin: [u32; 3],
    scale: u8,
}

fn oracle_dense_node(
    base: &ShStreamBaseMetadata,
    dense_index: usize,
) -> Result<OracleDenseNode, ShResidencyControllerError> {
    let probe = base.probes.get(dense_index).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("dense index exceeds id-34 probes".into())
    })?;
    let width = usize::try_from(base.grid_dimensions[0]).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("grid width exceeds usize".into())
    })?;
    let height = usize::try_from(base.grid_dimensions[1]).map_err(|_| {
        ShResidencyControllerError::InvalidTopology("grid height exceeds usize".into())
    })?;
    let xy = width.checked_mul(height).ok_or_else(|| {
        ShResidencyControllerError::InvalidTopology("grid xy dimensions overflow".into())
    })?;
    if xy == 0
        || dense_index
            >= xy
                .checked_mul(usize::try_from(base.grid_dimensions[2]).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("grid depth exceeds usize".into())
                })?)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("grid dimensions overflow".into())
                })?
    {
        return Err(ShResidencyControllerError::InvalidTopology(
            "dense index is outside id-34 grid".into(),
        ));
    }
    let coords = [
        u32::try_from(dense_index % width).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe x exceeds u32".into())
        })?,
        u32::try_from((dense_index / width) % height).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe y exceeds u32".into())
        })?,
        u32::try_from(dense_index / xy).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("probe z exceeds u32".into())
        })?,
    ];
    let scale_edge = 1u32
        .checked_shl(u32::from(probe.node_scale))
        .ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology("id-34 node scale overflows".into())
        })?;
    let brick = coords.map(|coordinate| coordinate / 4);
    Ok(OracleDenseNode {
        origin: brick.map(|coordinate| coordinate / scale_edge * scale_edge),
        scale: probe.node_scale,
    })
}

fn oracle_owners(
    manifest: ManifestTopologyView<'_>,
) -> Result<Vec<BTreeSet<u32>>, ShResidencyControllerError> {
    let directory = manifest.directory;
    let cluster_count = directory.clusters.len();
    let mut owners = vec![BTreeSet::new(); cluster_count];
    let streamed_sparse: BTreeSet<u32> = manifest
            .payloads
            .sources
            .iter()
            .filter(|source| {
                matches!(
                    source.kind,
                    postretro_level_format::cluster_sh_payloads::ClusterShPayloadsSourceKind::SparseAffinity
                )
            })
            .map(|source| source.section_id)
            .collect();
    let base_resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == SectionId::OctahedralShVolume as u32)
        .ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology(
                "id-49 does not contain the required id-34 resource".into(),
            )
        })?;

    let mut node_owner = BTreeMap::<OracleDenseNode, u32>::new();
    let mut patch_owner = vec![u32::MAX; manifest.base.probes.len()];
    for (cluster_id, _) in directory.clusters.iter().enumerate() {
        for range in cluster_ranges(directory, cluster_id)? {
            let resource = directory
                .resources
                .get(range.resource_index as usize)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology(
                        "range names an absent resource".into(),
                    )
                })?;
            if range.resource_index as usize != base_resource_index
                || resource.domain != ClusterResourceDomain::DenseProbe
            {
                continue;
            }
            if range.role != ClusterRangeRole::Dense
                || range.owner_cluster_id != DENSE_OWNER_SENTINEL
            {
                return Err(ShResidencyControllerError::InvalidTopology(
                    "id-34 range has non-dense ownership fields".into(),
                ));
            }
            for dense_index in checked_range(range.start, range.count, "dense range")? {
                let dense_index = usize::try_from(dense_index).map_err(|_| {
                    ShResidencyControllerError::InvalidTopology("dense index exceeds usize".into())
                })?;
                if manifest
                    .base
                    .probes
                    .get(dense_index)
                    .ok_or_else(|| {
                        ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds id-34 metadata".into(),
                        )
                    })?
                    .validity
                    == 0
                {
                    continue;
                }
                let patch = patch_owner.get_mut(dense_index).ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology(
                        "dense index exceeds id-34 metadata".into(),
                    )
                })?;
                *patch = (*patch).min(cluster_id as u32);
                let node = oracle_dense_node(manifest.base, dense_index)?;
                node_owner
                    .entry(node)
                    .and_modify(|owner| *owner = (*owner).min(cluster_id as u32))
                    .or_insert(cluster_id as u32);
            }
        }
    }

    for (cluster_id, _) in directory.clusters.iter().enumerate() {
        for range in cluster_ranges(directory, cluster_id)? {
            let resource = directory
                .resources
                .get(range.resource_index as usize)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology(
                        "range names an absent resource".into(),
                    )
                })?;
            if range.resource_index as usize == base_resource_index {
                for dense_index in checked_range(range.start, range.count, "dense range")? {
                    let dense_index = usize::try_from(dense_index).map_err(|_| {
                        ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds usize".into(),
                        )
                    })?;
                    if manifest
                        .base
                        .probes
                        .get(dense_index)
                        .ok_or_else(|| {
                            ShResidencyControllerError::InvalidTopology(
                                "dense index exceeds id-34 metadata".into(),
                            )
                        })?
                        .validity
                        == 0
                    {
                        continue;
                    }
                    let patch = *patch_owner.get(dense_index).ok_or_else(|| {
                        ShResidencyControllerError::InvalidTopology(
                            "dense index exceeds id-34 metadata".into(),
                        )
                    })?;
                    if patch == u32::MAX {
                        return Err(ShResidencyControllerError::InvalidTopology(
                            "dense patch has no canonical writer".into(),
                        ));
                    }
                    owners[cluster_id].insert(patch);
                    let node_owner = *node_owner
                        .get(&oracle_dense_node(manifest.base, dense_index)?)
                        .ok_or_else(|| {
                            ShResidencyControllerError::InvalidTopology(
                                "dense node has no canonical writer".into(),
                            )
                        })?;
                    owners[cluster_id].insert(node_owner);
                }
                continue;
            }
            if resource.domain == ClusterResourceDomain::AffinityCell
                && streamed_sparse.contains(&resource.section_id)
                && range.role == ClusterRangeRole::Halo
            {
                owners[cluster_id].insert(range.owner_cluster_id);
            }
        }
        owners[cluster_id].remove(&(cluster_id as u32));
    }
    validate_owner_graph(&owners)?;
    Ok(owners)
}

/// Build the topology over both tables and require byte-identical results.
/// Returns whether the manifest was accepted.
fn assert_tables_agree(manifest: &SyntheticManifest, label: &str) -> bool {
    let hints = Arc::new(ClusterHints {
        cell_to_cluster: Vec::new(),
        pinned: BTreeSet::new(),
        priority: vec![0; manifest.directory.clusters.len()],
    });
    let view = || ManifestTopologyView {
        directory: &manifest.directory,
        payloads: &manifest.payloads,
        base: &manifest.base,
        adjacency: &manifest.adjacency,
        seam_portals: &[],
    };
    let array_table =
        DenseNodeOwners::for_grid(manifest.base.grid_dimensions, manifest.base.probes.len());
    let array = PlannerTopology::from_manifest_view_with(view(), hints.clone(), array_table);
    let tree =
        PlannerTopology::from_manifest_view_with(view(), hints, BTreeMap::<DenseNode, u32>::new());
    assert_eq!(
        format!("{array:?}"),
        format!("{tree:?}"),
        "{label}: topology differs"
    );
    match (&array, oracle_owners(view())) {
        (Ok(topology), Ok(owners)) => {
            let expected: Vec<Vec<u32>> = owners
                .into_iter()
                .map(|set| set.into_iter().collect())
                .collect();
            assert_eq!(
                topology.owners, expected,
                "{label}: owners differ from oracle"
            );
        }
        (Err(error), Err(expected)) => assert_eq!(
            format!("{error:?}"),
            format!("{expected:?}"),
            "{label}: rejection differs from oracle"
        ),
        (array, oracle) => panic!(
            "{label}: production {:?} vs oracle {:?}",
            array.as_ref().map(|_| "accepted"),
            oracle.map(|_| "accepted")
        ),
    }
    array.is_ok()
}

#[test]
fn array_table_builds_the_same_topology_as_the_tree_on_well_formed_manifests() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut accepted, mut with_scales) = (0, 0);
    for round in 0..12 {
        for dims in GRID_SHAPES {
            for cluster_count in [1, 2, 5, 9] {
                let shape = Shape {
                    mixed_scales: false,
                    max_scale: 3,
                    leave_gaps: false,
                    sparse_halos: false,
                };
                let manifest = synthetic(&mut rng, dims, cluster_count, shape);
                with_scales += usize::from(manifest.base.probes.iter().any(|p| p.node_scale > 0));
                let label = format!("round {round} dims {dims:?} clusters {cluster_count}");
                accepted += usize::from(assert_tables_agree(&manifest, &label));
            }
        }
    }
    assert!(accepted > 300, "only {accepted} manifests were accepted");
    assert!(with_scales > 100);
}

#[test]
fn array_table_agrees_when_scales_collide_at_one_origin_brick() {
    let mut rng = Rng(0xd1b5_4a32_d192_ed03);
    let mut accepted = 0;
    for round in 0..12 {
        for dims in GRID_SHAPES {
            for cluster_count in [2, 6] {
                let shape = Shape {
                    mixed_scales: true,
                    max_scale: 3,
                    leave_gaps: false,
                    sparse_halos: false,
                };
                let manifest = synthetic(&mut rng, dims, cluster_count, shape);
                let label = format!("collide round {round} dims {dims:?}");
                accepted += usize::from(assert_tables_agree(&manifest, &label));
            }
        }
    }
    assert!(accepted > 100, "only {accepted} manifests were accepted");
}

#[test]
fn array_table_agrees_on_rejected_manifests() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let (mut accepted, mut rejected) = (0, 0);
    for round in 0..20 {
        for dims in GRID_SHAPES {
            for (max_scale, leave_gaps, sparse_halos) in [
                (3, true, false),
                (40, false, false),
                (3, false, true),
                (40, true, true),
            ] {
                let shape = Shape {
                    mixed_scales: round % 2 == 0,
                    max_scale,
                    leave_gaps,
                    sparse_halos,
                };
                let manifest = synthetic(&mut rng, dims, 4, shape);
                let label = format!("reject round {round} dims {dims:?} scale {max_scale}");
                if assert_tables_agree(&manifest, &label) {
                    accepted += 1;
                } else {
                    rejected += 1;
                }
            }
        }
    }
    assert!(
        accepted > 20 && rejected > 20,
        "accepted {accepted}, rejected {rejected}"
    );
}

#[test]
fn array_table_answers_every_owner_query_like_the_tree() {
    let mut rng = Rng(42);
    for dims in GRID_SHAPES {
        let mut array = DenseNodeOwners::for_grid(dims, dims.iter().product::<u32>() as usize);
        let mut tree = BTreeMap::<DenseNode, u32>::new();
        let bricks = dims.map(|axis| axis.div_ceil(4));
        let node = |rng: &mut Rng| DenseNode {
            // Origins run past the grid so the overflow path takes some.
            origin: [
                rng.below(bricks[0] + 2),
                rng.below(bricks[1] + 2),
                rng.below(bricks[2] + 2),
            ],
            scale: rng.below(4) as u8,
        };
        for _ in 0..400 {
            let (key, cluster_id) = (node(&mut rng), rng.below(8));
            array.lower_owner(key, cluster_id);
            tree.lower_owner(key, cluster_id);
        }
        for _ in 0..800 {
            let key = node(&mut rng);
            assert_eq!(array.owner(&key), tree.owner(&key), "dims {dims:?} {key:?}");
        }
        for key in tree.keys() {
            assert_eq!(array.owner(key), tree.owner(key));
        }
    }
}

#[test]
fn an_oversized_grid_claim_falls_back_to_the_overflow_map() {
    // 4096^3 probes would need 2^27 bricks; two probes cannot justify that.
    let mut table = DenseNodeOwners::for_grid([4096, 4096, 4096], 2);
    let node = DenseNode {
        origin: [5, 5, 5],
        scale: 1,
    };
    table.lower_owner(node, 9);
    table.lower_owner(node, 3);
    assert_eq!(table.owner(&node), Some(3));
}
