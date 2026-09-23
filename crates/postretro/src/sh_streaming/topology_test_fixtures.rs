//! Minimal manifest-view fixtures for topology boundary tests.
//! See: context/lib/testing_guide.md §4

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
use postretro_level_loader::ShStreamBaseMetadata;

use super::controller::ShResidencyControllerError;
use super::topology::{ManifestTopologyView, PlannerTopology};

pub(super) struct ManifestFixture {
    directory: ClusterDirectorySection,
    payloads: ClusterShPayloadsSection,
    base: ShStreamBaseMetadata,
    adjacency: Vec<Vec<u32>>,
}

impl ManifestFixture {
    pub(super) fn two_clusters() -> Self {
        let dense_resource = ClusterResourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [2, 1, 1],
        };
        Self {
            directory: ClusterDirectorySection {
                runtime_cell_count: 2,
                primitive_limit: 64,
                cell_limit: 32,
                clusters: vec![cluster(0, 0), cluster(1, 1)],
                resources: vec![dense_resource],
                members: vec![0, 1],
                ranges: vec![dense_range(0), dense_range(1)],
            },
            payloads: ClusterShPayloadsSection {
                header: ClusterShPayloadsHeader {
                    cluster_count: 2,
                    source_count: 1,
                    grid_dimensions: [2, 1, 1],
                    affinity_dimensions: [1, 1, 1],
                    payload_bytes: 0,
                },
                sources: vec![ClusterShPayloadsSourceRecord {
                    section_id: SectionId::OctahedralShVolume as u32,
                    internal_version: SH_VOLUME_VERSION,
                    kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
                }],
                index: vec![payload_index(7), payload_index(11)],
            },
            base: ShStreamBaseMetadata {
                grid_origin: [0.0; 3],
                cell_size: [1.0; 3],
                grid_dimensions: [2, 1, 1],
                probe_stride: OCTAHEDRAL_PROBE_STRIDE,
                tile_dimension: 6,
                tile_border: 1,
                atlas_dimensions: [12, 6],
                layer_count: 1,
                tiles_per_layer: 2,
                atlas_tiles_per_row: 2,
                irradiance_format: 0,
                probes: vec![
                    OctahedralShProbe {
                        validity: 1,
                        ..OctahedralShProbe::default()
                    };
                    2
                ],
                animation_descriptors: Vec::new(),
                slot_for_map_light: Vec::new(),
            },
            adjacency: vec![vec![1], vec![0]],
        }
    }

    pub(super) fn duplicate_cell_assignment(&mut self) {
        self.directory.members[1] = 0;
    }

    pub(super) fn leave_cell_unassigned(&mut self) {
        self.directory.runtime_cell_count = 3;
    }

    pub(super) fn malformed_dense_ownership(&mut self) {
        self.directory.ranges[0].role = ClusterRangeRole::Halo;
        self.directory.ranges[0].owner_cluster_id = 1;
    }

    pub(super) fn out_of_range_dense_index(&mut self) {
        self.directory.ranges[1].start = 2;
    }

    pub(super) fn add_sparse_halo_with_owner(&mut self, halo_owner: u32) {
        self.directory.resources.push(ClusterResourceRecord {
            section_id: SectionId::DeltaShVolumes as u32,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [1, 1, 1],
        });
        self.directory.ranges = vec![
            dense_range(0),
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            dense_range(1),
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 1,
                owner_cluster_id: halo_owner,
                role: ClusterRangeRole::Halo,
            },
        ];
        self.directory.clusters[0].range_start = 0;
        self.directory.clusters[0].range_count = 2;
        self.directory.clusters[1].range_start = 2;
        self.directory.clusters[1].range_count = 2;
        self.payloads.sources.insert(
            0,
            ClusterShPayloadsSourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                internal_version: 6,
                kind: ClusterShPayloadsSourceKind::SparseAffinity,
            },
        );
        self.payloads.header.source_count = 2;
    }

    pub(super) fn out_of_range_adjacency(&mut self) {
        self.adjacency[0] = vec![2];
    }

    pub(super) fn build_topology(&self) -> Result<PlannerTopology, ShResidencyControllerError> {
        PlannerTopology::from_manifest_view(ManifestTopologyView {
            directory: &self.directory,
            payloads: &self.payloads,
            base: &self.base,
            adjacency: &self.adjacency,
        })
    }
}

pub(super) fn assert_invalid_topology(
    result: Result<PlannerTopology, ShResidencyControllerError>,
    expected: &str,
) {
    match result {
        Err(ShResidencyControllerError::InvalidTopology(message)) => {
            assert!(
                message.contains(expected),
                "expected topology rejection containing {expected:?}, got {message:?}"
            );
        }
        Err(error) => panic!("expected named InvalidTopology rejection, got {error}"),
        Ok(_) => panic!("malformed manifest unexpectedly constructed planner topology"),
    }
}

fn cluster(member_start: u32, range_start: u32) -> ClusterRecord {
    ClusterRecord {
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        member_start,
        member_count: 1,
        range_start,
        range_count: 1,
        primitive_count: 0,
        flags: 0,
    }
}

fn dense_range(start: u32) -> ClusterRangeRecord {
    ClusterRangeRecord {
        resource_index: 0,
        start,
        count: 1,
        owner_cluster_id: DENSE_OWNER_SENTINEL,
        role: ClusterRangeRole::Dense,
    }
}

fn payload_index(requested_resident_bytes: u64) -> ClusterShPayloadsIndexRecord {
    ClusterShPayloadsIndexRecord {
        payload_offset: 0,
        payload_len: 0,
        decoded_bytes: 0,
        requested_resident_bytes,
        stored_tile_count: 0,
        dense_patch_count: 0,
        affinity_patch_count: 0,
        hash: [requested_resident_bytes as u8; 32],
    }
}
