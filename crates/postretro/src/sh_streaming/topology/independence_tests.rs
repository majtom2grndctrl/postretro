//! The planner reads no per-probe or ownership data: a cluster's table rows come
//! from the id-50 index alone, whatever the probes and halo ranges say, so no
//! cluster's topology depends on another's. Random manifests, because scales
//! and halos are drawn per probe and per range.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

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

use super::*;
use crate::sh_streaming::topology_test_fixtures::assert_invalid_topology;
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

/// What a manifest may vary without the planner noticing.
#[derive(Clone, Copy)]
struct Shape {
    /// Probe scales are drawn per probe, not per brick, so several scales
    /// share an origin brick.
    mixed_scales: bool,
    /// Largest scale drawn; past `MAX_NODE_SCALE` only the format refuses it.
    max_scale: u32,
    /// Leave some valid probes outside every dense range.
    leave_gaps: bool,
    /// Add sparse halo ranges that may name any cluster.
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

fn topology_of(
    manifest: &SyntheticManifest,
) -> Result<PlannerTopology, ShResidencyControllerError> {
    let hints = Arc::new(ClusterHints {
        cell_to_cluster: Vec::new(),
        pinned: Default::default(),
        priority: vec![0; manifest.directory.clusters.len()],
    });
    PlannerTopology::from_manifest_view(
        ManifestTopologyView {
            directory: &manifest.directory,
            payloads: &manifest.payloads,
            base: &manifest.base,
            adjacency: &manifest.adjacency,
            seam_portals: &[],
        },
        hints,
    )
}

#[test]
fn cluster_tables_come_from_the_id50_index_whatever_probes_and_halos_say() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut manifests = 0;
    for round in 0..12 {
        for dims in GRID_SHAPES {
            for cluster_count in [1, 2, 5, 9] {
                for (max_scale, leave_gaps, sparse_halos) in
                    [(3, false, false), (3, true, false), (40, true, true)]
                {
                    let shape = Shape {
                        mixed_scales: round % 2 == 0,
                        max_scale,
                        leave_gaps,
                        sparse_halos,
                    };
                    let manifest = synthetic(&mut rng, dims, cluster_count, shape);
                    let topology = topology_of(&manifest)
                        .unwrap_or_else(|error| panic!("{dims:?} {cluster_count}: {error}"));
                    let index = &manifest.payloads.index;
                    assert_eq!(topology.cluster_count(), cluster_count as usize);
                    assert_eq!(
                        topology.requested_resident_bytes,
                        index
                            .iter()
                            .map(|entry| entry.requested_resident_bytes)
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(
                        topology.chunk_hashes,
                        index.iter().map(|entry| entry.hash).collect::<Vec<_>>()
                    );
                    manifests += 1;
                }
            }
        }
    }
    assert!(manifests > 1000, "only {manifests} manifests were checked");
}

#[test]
fn a_dense_range_past_the_id34_metadata_is_refused_by_name() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for dims in GRID_SHAPES {
        let shape = Shape {
            mixed_scales: false,
            max_scale: 3,
            leave_gaps: false,
            sparse_halos: false,
        };
        let mut manifest = synthetic(&mut rng, dims, 2, shape);
        let last = manifest.directory.ranges.len() - 1;
        manifest.directory.ranges[last] = dense_range(dims.iter().product::<u32>(), 1);
        assert_invalid_topology(topology_of(&manifest), "dense index exceeds id-34 metadata");
    }
}
