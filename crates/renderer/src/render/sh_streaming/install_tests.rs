//! CPU-only install-transaction tests over synthetic streamed maps.
//!
//! Every probe is a valid L0 probe and every cluster owns a box of whole
//! bricks, so each chunk is canonical: one owner per probe, node, and sparse
//! row. Failures are injected into otherwise valid chunks.

use super::*;
use postretro_level_format::cluster_directory::{
    ClusterRangeRecord, ClusterRecord, ClusterResourceRecord,
};
use postretro_level_format::cluster_sh_payloads::DecodedClusterShBlock;
use postretro_level_format::sh_volume::OctahedralShProbe;

const SPARSE_SECTIONS: [u32; 3] = [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID];
/// f16 halves each synthetic sparse entry carries.
const ENTRY_TILE_F16: u32 = 2;

pub(super) struct SyntheticMap {
    grid: [u32; 3],
    bricks: [u32; 3],
    cluster_bricks: [u32; 3],
    clusters: [u32; 3],
}

impl SyntheticMap {
    /// `grid` is in probes and must be a multiple of four per axis;
    /// `cluster_bricks` must divide the brick grid.
    pub(super) fn new(grid: [u32; 3], cluster_bricks: [u32; 3]) -> Self {
        let bricks = grid.map(|axis| axis / 4);
        let clusters = [0, 1, 2].map(|axis| bricks[axis] / cluster_bricks[axis]);
        assert!((0..3).all(|axis| grid[axis] % 4 == 0 && bricks[axis] % cluster_bricks[axis] == 0));
        Self {
            grid,
            bricks,
            cluster_bricks,
            clusters,
        }
    }

    pub(super) fn cluster_count(&self) -> u32 {
        self.clusters.iter().product()
    }

    fn cluster_origin_brick(&self, cluster_id: u32) -> [u32; 3] {
        let [cx, cy, _] = self.clusters;
        [
            cluster_id % cx * self.cluster_bricks[0],
            cluster_id / cx % cy * self.cluster_bricks[1],
            cluster_id / (cx * cy) * self.cluster_bricks[2],
        ]
    }

    fn dense(&self, [x, y, z]: [u32; 3]) -> u32 {
        x + y * self.grid[0] + z * self.grid[0] * self.grid[1]
    }

    fn brick_row(&self, [x, y, z]: [u32; 3]) -> u32 {
        x + y * self.bricks[0] + z * self.bricks[0] * self.bricks[1]
    }

    fn row_count(&self) -> u32 {
        self.bricks.iter().product()
    }

    /// Sparse rows owned by a cluster, one per brick, in chunk order.
    fn cluster_rows(&self, cluster_id: u32) -> Vec<u32> {
        let origin = self.cluster_origin_brick(cluster_id);
        let mut rows = Vec::new();
        for z in 0..self.cluster_bricks[2] {
            for y in 0..self.cluster_bricks[1] {
                for x in 0..self.cluster_bricks[0] {
                    rows.push(self.brick_row([origin[0] + x, origin[1] + y, origin[2] + z]));
                }
            }
        }
        rows
    }

    /// Dense probes owned by a cluster, in chunk order.
    fn cluster_probes(&self, cluster_id: u32) -> Vec<u32> {
        let origin = self.cluster_origin_brick(cluster_id).map(|axis| axis * 4);
        let extent = self.cluster_bricks.map(|axis| axis * 4);
        let mut probes = Vec::new();
        for z in 0..extent[2] {
            for y in 0..extent[1] {
                for x in 0..extent[0] {
                    probes.push(self.dense([origin[0] + x, origin[1] + y, origin[2] + z]));
                }
            }
        }
        probes
    }

    fn directory(&self) -> ClusterDirectorySection {
        let mut resources = vec![ClusterResourceRecord {
            section_id: INDIRECT_BASE_ID,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: self.grid,
        }];
        for section_id in SPARSE_SECTIONS {
            resources.push(ClusterResourceRecord {
                section_id,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: self.bricks,
            });
        }
        let mut clusters = Vec::new();
        let mut ranges = Vec::new();
        for cluster_id in 0..self.cluster_count() {
            let range_start = ranges.len() as u32;
            let origin = self.cluster_origin_brick(cluster_id);
            let probe_origin = origin.map(|axis| axis * 4);
            let probe_extent = self.cluster_bricks.map(|axis| axis * 4);
            for z in 0..probe_extent[2] {
                for y in 0..probe_extent[1] {
                    ranges.push(ClusterRangeRecord {
                        resource_index: 0,
                        start: self.dense([
                            probe_origin[0],
                            probe_origin[1] + y,
                            probe_origin[2] + z,
                        ]),
                        count: probe_extent[0],
                        owner_cluster_id: cluster_id,
                        role: ClusterRangeRole::Owned,
                    });
                }
            }
            for resource_index in 1..=SPARSE_SECTIONS.len() as u32 {
                for z in 0..self.cluster_bricks[2] {
                    for y in 0..self.cluster_bricks[1] {
                        ranges.push(ClusterRangeRecord {
                            resource_index,
                            start: self.brick_row([origin[0], origin[1] + y, origin[2] + z]),
                            count: self.cluster_bricks[0],
                            owner_cluster_id: cluster_id,
                            role: ClusterRangeRole::Owned,
                        });
                    }
                }
            }
            clusters.push(ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 0,
                range_start,
                range_count: ranges.len() as u32 - range_start,
                primitive_count: 0,
                flags: 0,
            });
        }
        ClusterDirectorySection {
            runtime_cell_count: 0,
            primitive_limit: 0,
            cell_limit: 0,
            clusters,
            resources,
            members: Vec::new(),
            ranges,
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        }
    }

    fn base(&self) -> postretro_level_loader::ShStreamBaseMetadata {
        postretro_level_loader::ShStreamBaseMetadata {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: self.grid,
            probe_stride: 8,
            tile_dimension: 4,
            tile_border: 1,
            atlas_dimensions: [8, 8],
            layer_count: 1,
            tiles_per_layer: 1,
            atlas_tiles_per_row: 1,
            irradiance_format: 0,
            probes: vec![
                OctahedralShProbe {
                    validity: 1,
                    density_level: 0,
                    node_scale: 0,
                    ..Default::default()
                };
                self.grid.iter().product::<u32>() as usize
            ],
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn sources(&self) -> postretro_level_loader::ShStreamSourceMetadata {
        let rows = self.row_count() as usize;
        let sparse = |section_id, animation_descriptor_indices| {
            postretro_level_loader::ShStreamSparseMetadata {
                section_id,
                internal_version: 1,
                affinity_dims: self.bricks,
                tile_dimension: 4,
                tile_border: 1,
                valid_probe_masks: vec![u64::MAX; rows],
                cell_levels: vec![2; rows],
                affinity_offsets: (0..=rows as u32).collect(),
                affinity_lights: vec![0; rows],
                animation_descriptor_indices,
            }
        };
        postretro_level_loader::ShStreamSourceMetadata {
            indirect_delta: Some(sparse(INDIRECT_DELTA_ID, vec![0])),
            direct: Some(postretro_level_loader::ShStreamDirectMetadata {
                grid_origin: [0.0; 3],
                cell_size: [1.0; 3],
                grid_dimensions: self.grid,
                tile_dimension: 4,
                tile_border: 1,
                atlas_dimensions: [8, 8],
                layer_count: 1,
                tiles_per_layer: 1,
                atlas_tiles_per_row: 1,
                irradiance_format: 0,
            }),
            direct_delta: Some(sparse(DIRECT_DELTA_ID, Vec::new())),
            animated_direct_delta: Some(sparse(ANIMATED_DIRECT_DELTA_ID, vec![0])),
        }
    }

    /// A live generation that targets every cluster.
    pub(super) fn state(&self) -> ShResidencyState {
        let mut state =
            ShResidencyState::from_parts([5; 32], &self.directory(), &self.base(), &self.sources())
                .expect("synthetic streamed map is canonical");
        state.generation = 1;
        state.generation_has_reset = true;
        state.targets = (0..self.cluster_count()).collect();
        state
    }

    /// A canonical ready chunk: id-34/id-35 atlases, one probe patch per
    /// owned probe, and one owned row per brick for each sparse family, in
    /// id-27, id-41, id-45 order.
    pub(super) fn prepared(&self, state: &ShResidencyState, cluster_id: u32) -> PreparedShCluster {
        let mut nodes = state.nodes_by_owner[&cluster_id].clone();
        nodes.sort_by_key(|node| state.node_layouts[node].global_base_slot);
        let mut closure_base = BTreeMap::new();
        let mut closure_tiles = 0u32;
        for node in nodes {
            closure_base.insert(node, closure_tiles);
            closure_tiles += state.node_layouts[&node].tile_count;
        }

        let mut bytes = Vec::new();
        let mut blocks = Vec::new();
        let mut push_block = |section_id, kind, element_count, words: &[u32]| {
            let start = bytes.len();
            for word in words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            blocks.push(DecodedClusterShBlock {
                section_id,
                kind,
                element_count,
                body: start..bytes.len(),
            });
        };
        let atlas_header = [0, closure_tiles, 1, 1, 1];
        push_block(
            INDIRECT_BASE_ID,
            ISOLATED_ATLAS_BLOCK,
            closure_tiles,
            &atlas_header,
        );
        push_block(
            DIRECT_BASE_ID,
            ISOLATED_ATLAS_BLOCK,
            closure_tiles,
            &atlas_header,
        );

        let probes = self.cluster_probes(cluster_id);
        let mut patch_words = Vec::with_capacity(probes.len() * 4);
        for &dense in &probes {
            let node = state.dense_node[dense as usize].expect("synthetic probes are valid");
            let local = state.dense_node_local_slot[dense as usize].expect("valid probe rank");
            let rank = closure_base[&node] + local;
            let word = PROBE_INDIRECTION_VALID_BIT
                | u32::from(node.level)
                | (u32::from(node.scale) << PROBE_INDIRECTION_SCALE_SHIFT)
                | (rank << PROBE_INDIRECTION_SLOT_SHIFT);
            // Mean distance and mean-square distance share the third word.
            patch_words.extend_from_slice(&[dense, word, 0x3c00_3c00, 0]);
        }
        push_block(
            INDIRECT_BASE_ID,
            PROBE_PATCH_BLOCK,
            probes.len() as u32,
            &patch_words,
        );

        let rows = self.cluster_rows(cluster_id);
        let row_count = rows.len() as u32;
        for section_id in SPARSE_SECTIONS {
            let mut words = vec![row_count, row_count, row_count * ENTRY_TILE_F16, 0];
            for (index, &row) in rows.iter().enumerate() {
                words.extend_from_slice(&[row, index as u32, 1, 1]);
            }
            for index in 0..row_count {
                words.extend_from_slice(&[0, index * ENTRY_TILE_F16, ENTRY_TILE_F16, 0]);
            }
            // Two f16 halves per u32 word.
            words.extend(std::iter::repeat_n(
                0,
                (row_count * ENTRY_TILE_F16 / 2) as usize,
            ));
            push_block(section_id, SPARSE_ROWS_BLOCK, row_count, &words);
        }

        PreparedShCluster {
            generation: state.generation,
            content_tag: state.content_tag,
            chunk: DecodedClusterShPayload {
                cluster_id,
                bytes,
                blocks,
            },
        }
    }
}

/// Rewrite the level bits of a chunk's last probe patch so that patch fails
/// validation after every earlier patch has rewritten its compose word.
fn corrupt_last_probe_patch(prepared: &mut PreparedShCluster) {
    let block = prepared
        .chunk
        .blocks
        .iter()
        .find(|block| block.kind == PROBE_PATCH_BLOCK)
        .expect("synthetic chunk carries probe patches")
        .clone();
    let word_offset = block.body.end - 16 + 4;
    prepared.chunk.bytes[word_offset] ^= 0x1;
}

/// Every residency mirror an install may touch, compared field by field.
#[derive(Debug, PartialEq)]
struct ResidencyMirror {
    node_slots: BTreeMap<StoredNode, PoolRange>,
    dense_slots: FirstFitRanges,
    compose_words: Vec<u32>,
    sampled_words: Vec<u32>,
    sparse_pools: BTreeMap<u32, SparsePool>,
    dirty_rows: BTreeSet<(u32, u32)>,
    indirect_dirty_rows: BTreeSet<u32>,
    indirect_resident_rows: BTreeSet<u32>,
    indirect_base_row_refs: BTreeMap<u32, u32>,
    indirect_delta_row_refs: BTreeMap<u32, u32>,
    direct_promotion_dirty_rows: BTreeSet<u32>,
    direct_animated_dirty_rows: BTreeSet<u32>,
    direct_promotion_resident_rows: BTreeSet<u32>,
    direct_animated_resident_rows: BTreeSet<u32>,
    direct_base_row_refs: BTreeMap<u32, u32>,
    direct_promotion_row_refs: BTreeMap<u32, u32>,
    direct_animated_row_refs: BTreeMap<u32, u32>,
    installed: Vec<InstalledMirror>,
    pending_promotion: BTreeSet<u32>,
    sampleable: BTreeSet<u32>,
}

#[derive(Debug, PartialEq)]
struct InstalledMirror {
    cluster_id: u32,
    patched_dense: Vec<u32>,
    owned_nodes: Vec<StoredNode>,
    sparse_rows: Vec<(u32, u32)>,
    required_indirect_epoch: u64,
    required_direct_epoch: u64,
}

fn mirror(state: &ShResidencyState) -> ResidencyMirror {
    ResidencyMirror {
        node_slots: state.node_slots.clone(),
        dense_slots: state.dense_slots.clone(),
        compose_words: state.compose_words.clone(),
        sampled_words: state.sampled_words.clone(),
        sparse_pools: state.sparse_pools.clone(),
        dirty_rows: state.dirty_rows.clone(),
        indirect_dirty_rows: state.indirect_dirty_rows.clone(),
        indirect_resident_rows: state.indirect_resident_rows.clone(),
        indirect_base_row_refs: state.indirect_base_row_refs.clone(),
        indirect_delta_row_refs: state.indirect_delta_row_refs.clone(),
        direct_promotion_dirty_rows: state.direct_promotion_dirty_rows.clone(),
        direct_animated_dirty_rows: state.direct_animated_dirty_rows.clone(),
        direct_promotion_resident_rows: state.direct_promotion_resident_rows.clone(),
        direct_animated_resident_rows: state.direct_animated_resident_rows.clone(),
        direct_base_row_refs: state.direct_base_row_refs.clone(),
        direct_promotion_row_refs: state.direct_promotion_row_refs.clone(),
        direct_animated_row_refs: state.direct_animated_row_refs.clone(),
        installed: state
            .installed
            .iter()
            .map(|(&cluster_id, installed)| InstalledMirror {
                cluster_id,
                patched_dense: installed.patches.iter().map(|patch| patch.dense).collect(),
                owned_nodes: installed.owned_nodes.clone(),
                sparse_rows: installed.sparse_rows.clone(),
                required_indirect_epoch: installed.required_indirect_epoch,
                required_direct_epoch: installed.required_direct_epoch,
            })
            .collect(),
        pending_promotion: state.pending_promotion.clone(),
        sampleable: state.sampleable.clone(),
    }
}

/// Two clusters of two bricks each: small enough to read, large enough that a
/// second install reuses nothing by accident.
fn two_cluster_map() -> SyntheticMap {
    SyntheticMap::new([16, 4, 4], [2, 1, 1])
}

#[test]
fn synthetic_install_publishes_dense_and_every_sparse_family() {
    let map = two_cluster_map();
    let mut state = map.state();
    let prepared = map.prepared(&state, 0);
    state.install(None, &prepared).unwrap();

    assert_eq!(state.installed.keys().copied().collect::<Vec<_>>(), [0]);
    assert_eq!(state.node_slots.len(), 2);
    assert!(state.compose_words[..4].iter().all(|word| *word != 0));
    for section_id in SPARSE_SECTIONS {
        assert_eq!(state.sparse_pools[&section_id].logical_occupancy(), (2, 4));
    }
    assert_eq!(state.indirect_resident_rows, BTreeSet::from([0, 1]));
    assert_eq!(state.direct_promotion_resident_rows, BTreeSet::from([0, 1]));
    assert_eq!(state.direct_animated_resident_rows, BTreeSet::from([0, 1]));
}

#[test]
fn incremental_resident_rows_match_a_full_rebuild_from_row_refs() {
    let map = two_cluster_map();
    let mut state = map.state();
    state.install(None, &map.prepared(&state, 0)).unwrap();
    state.install(None, &map.prepared(&state, 1)).unwrap();
    let incremental = (
        state.indirect_resident_rows.clone(),
        state.direct_promotion_resident_rows.clone(),
        state.direct_animated_resident_rows.clone(),
    );

    // Eviction still rebuilds these unions from the ref tables; install must
    // leave exactly what that rebuild would produce.
    state.refresh_indirect_resident_rows();
    state.refresh_direct_resident_rows();
    assert_eq!(
        incremental,
        (
            state.indirect_resident_rows.clone(),
            state.direct_promotion_resident_rows.clone(),
            state.direct_animated_resident_rows.clone(),
        )
    );
    assert_eq!(incremental.0, BTreeSet::from([0, 1, 2, 3]));
}

#[test]
fn failed_install_after_partial_allocation_restores_every_residency_mirror() {
    let map = two_cluster_map();

    // Failure 1: the last probe patch is malformed. Cluster 1's dense nodes
    // are already allocated and every earlier patch has rewritten its compose
    // word when the error surfaces.
    let mut state = map.state();
    state.install(None, &map.prepared(&state, 0)).unwrap();
    let before = mirror(&state);
    let mut bad_patch = map.prepared(&state, 1);
    corrupt_last_probe_patch(&mut bad_patch);
    assert_eq!(
        state.install(None, &bad_patch),
        Err(malformed(
            1,
            "probe patch flags disagree with retained id-34 node metadata"
        ))
    );
    assert_eq!(mirror(&state), before);

    // Failure 2: the id-45 pool refuses cluster 1's first row. A stale live
    // row stands in for the allocator refusal. Dense nodes, compose words,
    // dirty and resident rows, row refs, and the id-27 and id-41 rows are all
    // provisionally allocated first, because id-45 is the chunk's last block.
    let mut state = map.state();
    state.install(None, &map.prepared(&state, 0)).unwrap();
    let blocked_row = map.cluster_rows(1)[0];
    state
        .sparse_pools
        .get_mut(&ANIMATED_DIRECT_DELTA_ID)
        .unwrap()
        .install(blocked_row, 1, ENTRY_TILE_F16)
        .unwrap();
    let before = mirror(&state);
    assert_eq!(
        state.install(None, &map.prepared(&state, 1)),
        Err(malformed(1, "sparse row cannot allocate pool range"))
    );
    assert_eq!(mirror(&state), before);
}

#[test]
fn rolled_back_install_strands_no_address_for_the_next_drain() {
    let map = two_cluster_map();
    let mut recovered = map.state();
    recovered
        .install(None, &map.prepared(&recovered, 0))
        .unwrap();
    let mut bad_patch = map.prepared(&recovered, 1);
    corrupt_last_probe_patch(&mut bad_patch);
    assert!(recovered.install(None, &bad_patch).is_err());
    recovered
        .install(None, &map.prepared(&recovered, 1))
        .unwrap();

    let mut clean = map.state();
    clean.install(None, &map.prepared(&clean, 0)).unwrap();
    clean.install(None, &map.prepared(&clean, 1)).unwrap();

    // A retry after rollback lands on exactly the addresses a never-failed
    // install would, so no slot or row range was leaked or double-owned.
    assert_eq!(mirror(&recovered), mirror(&clean));
}
