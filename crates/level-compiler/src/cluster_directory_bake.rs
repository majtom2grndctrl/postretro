//! Deterministic compiler-only cell clustering and SH directory construction.
//! See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_FLAG_INDIVISIBLE_OVERSIZE, CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection,
    ClusterDirectoryValidationInputs, ClusterHintRecord, ClusterRecord, canonical_cell_partition,
    populate_canonical_resource_ranges,
};
use postretro_level_format::portals::PortalsSection;

use crate::pack::FinalizedShEmissionView;
use crate::streaming_hints::ResolvedStreamingHints;

/// Defaults selected by the bounded Slice-2 threshold exercise.
pub const DEFAULT_PRIMITIVE_LIMIT: u32 = 64;
pub const DEFAULT_CELL_LIMIT: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterDirectoryBakeStats {
    pub elapsed: Duration,
    pub directory_bytes: usize,
    pub construction_metadata_bytes: usize,
    pub cluster_count: usize,
    pub range_count: usize,
    pub oversize_singletons: usize,
    pub active_affinity_cells: usize,
    pub covering_references: usize,
    pub maximum_visited_nodes_per_cluster: usize,
}

pub struct ClusterDirectoryBake {
    pub directory: ClusterDirectorySection,
    pub stats: ClusterDirectoryBakeStats,
}

pub(crate) fn bake_cluster_directory(
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    cell_locator: &CellLocatorSection,
    sh: FinalizedShEmissionView<'_>,
    streaming_hints: &ResolvedStreamingHints,
) -> anyhow::Result<ClusterDirectoryBake> {
    bake_cluster_directory_with_hints_and_limits(
        cells,
        portals,
        bvh,
        cell_locator,
        sh,
        streaming_hints,
        (DEFAULT_PRIMITIVE_LIMIT, DEFAULT_CELL_LIMIT),
    )
}

#[cfg(test)]
pub(crate) fn bake_cluster_directory_with_limits(
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    cell_locator: &CellLocatorSection,
    sh: FinalizedShEmissionView<'_>,
    primitive_limit: u32,
    cell_limit: u32,
) -> anyhow::Result<ClusterDirectoryBake> {
    let empty_streaming_hints = ResolvedStreamingHints::default();
    bake_cluster_directory_with_hints_and_limits(
        cells,
        portals,
        bvh,
        cell_locator,
        sh,
        &empty_streaming_hints,
        (primitive_limit, cell_limit),
    )
}

fn bake_cluster_directory_with_hints_and_limits(
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    cell_locator: &CellLocatorSection,
    sh: FinalizedShEmissionView<'_>,
    streaming_hints: &ResolvedStreamingHints,
    (primitive_limit, cell_limit): (u32, u32),
) -> anyhow::Result<ClusterDirectoryBake> {
    anyhow::ensure!(
        primitive_limit > 0,
        "cluster primitive limit must be positive"
    );
    anyhow::ensure!(cell_limit > 0, "cluster cell limit must be positive");
    let started = Instant::now();
    let partition = canonical_cell_partition(
        cells,
        portals,
        bvh,
        primitive_limit,
        cell_limit,
        &streaming_hints.seam_portal_ids,
    )?;
    let mut oversize_singletons = 0usize;
    for cluster in &partition.clusters {
        if cluster.flags != CLUSTER_FLAG_INDIVISIBLE_OVERSIZE {
            continue;
        }
        oversize_singletons += 1;
        let cell = partition.members[cluster.member_start as usize];
        log::warn!(
            "[Compiler] SH cluster cell {cell} is an indivisible singleton with {} primitives, exceeding limit {primitive_limit} by {}",
            cluster.primitive_count,
            cluster.primitive_count - primitive_limit,
        );
    }

    let cluster_hints = cluster_hints_for_partition(&partition, cells, streaming_hints)?;
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: u32::try_from(cells.cells.len())?,
        primitive_limit,
        cell_limit,
        clusters: partition.clusters,
        resources: Vec::new(),
        members: partition.members,
        ranges: Vec::new(),
        seam_portal_ids: streaming_hints.seam_portal_ids.clone(),
        cluster_hints,
    };
    let inputs = ClusterDirectoryValidationInputs {
        cells,
        portals,
        bvh,
        cell_locator,
        sh: sh.inventory(),
    };
    let coverage_stats = populate_canonical_resource_ranges(&mut directory, inputs)?;
    directory.validate_semantics(inputs)?;
    let directory_bytes = directory.byte_len()?;
    let construction_metadata_bytes = construction_metadata_bytes(
        directory.clusters.len(),
        directory.members.len(),
        directory.ranges.len(),
        coverage_stats.affinity_cell_count,
        coverage_stats.covering_references,
        coverage_stats.maximum_visited_nodes_per_cluster,
    )?;
    Ok(ClusterDirectoryBake {
        stats: ClusterDirectoryBakeStats {
            elapsed: started.elapsed(),
            directory_bytes,
            construction_metadata_bytes,
            cluster_count: directory.clusters.len(),
            range_count: directory.ranges.len(),
            oversize_singletons,
            active_affinity_cells: coverage_stats.active_affinity_cell_count,
            covering_references: coverage_stats.covering_references,
            maximum_visited_nodes_per_cluster: coverage_stats.maximum_visited_nodes_per_cluster,
        },
        directory,
    })
}

fn cluster_hints_for_partition(
    partition: &postretro_level_format::cluster_directory::CanonicalCellPartition,
    cells: &CellsSection,
    streaming_hints: &ResolvedStreamingHints,
) -> anyhow::Result<Vec<ClusterHintRecord>> {
    let cell_count = cells.cells.len();
    let mut cluster_by_cell = vec![None; cell_count];
    for (cluster_index, cluster) in partition.clusters.iter().enumerate() {
        let cluster_id = u32::try_from(cluster_index)?;
        let member_end = cluster
            .member_start
            .checked_add(cluster.member_count)
            .ok_or_else(|| {
                anyhow::anyhow!("canonical cluster {cluster_id} member range overflows")
            })?;
        let members = partition
            .members
            .get(usize::try_from(cluster.member_start)?..usize::try_from(member_end)?)
            .ok_or_else(|| {
                anyhow::anyhow!("canonical cluster {cluster_id} member range exceeds member list")
            })?;
        for &cell_id in members {
            let slot = cluster_by_cell
                .get_mut(usize::try_from(cell_id)?)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "canonical cluster {cluster_id} names runtime cell {cell_id} out of range"
                    )
                })?;
            anyhow::ensure!(
                slot.replace(cluster_id).is_none(),
                "canonical partition assigns runtime cell {cell_id} to more than one cluster",
            );
        }
    }

    let mut policy_by_cluster = BTreeMap::<u32, (bool, u32)>::new();
    let mut previous_pin = None;
    for &cell_id in &streaming_hints.pinned_cell_ids {
        anyhow::ensure!(
            previous_pin.is_none_or(|previous| previous < cell_id),
            "resolved streaming pinned-cell IDs must be strictly ascending",
        );
        let cluster_id = cluster_id_for_cell(cell_id, &cluster_by_cell)?;
        policy_by_cluster.entry(cluster_id).or_default().0 = true;
        previous_pin = Some(cell_id);
    }

    let mut previous_priority_cell = None;
    for &(cell_id, priority) in &streaming_hints.cell_priorities {
        anyhow::ensure!(
            previous_priority_cell.is_none_or(|previous| previous < cell_id),
            "resolved streaming priority cell IDs must be strictly ascending",
        );
        anyhow::ensure!(
            (1..=3).contains(&priority),
            "resolved streaming priority for cell {cell_id} must be in 1..=3",
        );
        let cluster_id = cluster_id_for_cell(cell_id, &cluster_by_cell)?;
        let policy = policy_by_cluster.entry(cluster_id).or_default();
        policy.1 = policy.1.max(priority);
        previous_priority_cell = Some(cell_id);
    }

    Ok(policy_by_cluster
        .into_iter()
        .map(|(cluster_id, (pinned, priority))| ClusterHintRecord {
            cluster_id,
            flags: if pinned { CLUSTER_HINT_FLAG_PINNED } else { 0 },
            priority,
        })
        .collect())
}

fn cluster_id_for_cell(cell_id: u32, cluster_by_cell: &[Option<u32>]) -> anyhow::Result<u32> {
    cluster_by_cell
        .get(usize::try_from(cell_id)?)
        .and_then(|cluster_id| *cluster_id)
        .ok_or_else(|| {
            anyhow::anyhow!("resolved streaming hint names runtime cell {cell_id} out of range")
        })
}

fn construction_metadata_bytes(
    cluster_count: usize,
    member_count: usize,
    range_count: usize,
    affinity_cell_count: usize,
    covering_references: usize,
    maximum_visited_nodes_per_cluster: usize,
) -> anyhow::Result<usize> {
    let cluster_bytes = cluster_count.checked_mul(size_of::<ClusterRecord>());
    let member_bytes = member_count.checked_mul(size_of::<u32>());
    let range_bytes = range_count.checked_mul(size_of::<
        postretro_level_format::cluster_directory::ClusterRangeRecord,
    >());
    let affinity_bytes = affinity_cell_count.checked_mul(2);
    // Conservative accounting for one BTreeSet node per covering reference:
    // key plus allocator/tree links and color/padding.
    let covering_reference_bytes = covering_references.checked_mul(40);
    let visited_node_bytes = maximum_visited_nodes_per_cluster.checked_mul(20);

    cluster_bytes
        .and_then(|value| member_bytes.and_then(|bytes| value.checked_add(bytes)))
        .and_then(|value| range_bytes.and_then(|bytes| value.checked_add(bytes)))
        .and_then(|value| affinity_bytes.and_then(|bytes| value.checked_add(bytes)))
        .and_then(|value| covering_reference_bytes.and_then(|bytes| value.checked_add(bytes)))
        .and_then(|value| visited_node_bytes.and_then(|bytes| value.checked_add(bytes)))
        .ok_or_else(|| anyhow::anyhow!("cluster metadata byte count overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bvh::{BvhLeaf, BvhSection};
    use postretro_level_format::cell_locator::{CellLocatorChild, CellLocatorSection};
    use postretro_level_format::cells::{CELL_FLAG_DRAWABLE, CellRecord, CellsSection};
    use postretro_level_format::portals::{PortalRecord, PortalsSection};
    use postretro_level_format::sh_volume::OctahedralShVolumeSection;

    fn cell(x: f32) -> CellRecord {
        CellRecord {
            bounds_min: [x, 0.0, 0.0],
            bounds_max: [x + 1.0, 1.0, 1.0],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }
    }

    fn portal(a: u32, b: u32) -> PortalRecord {
        PortalRecord {
            vertex_start: 0,
            vertex_count: 0,
            front_leaf: a,
            back_leaf: b,
        }
    }

    fn leaf(cell_id: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: 0,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count: 3,
            cell_id,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    fn bake(
        cells: CellsSection,
        portals: Vec<PortalRecord>,
        primitive_cells: &[u32],
        primitive_limit: u32,
        cell_limit: u32,
    ) -> ClusterDirectorySection {
        bake_result(cells, portals, primitive_cells, primitive_limit, cell_limit).unwrap()
    }

    fn bake_result(
        cells: CellsSection,
        portals: Vec<PortalRecord>,
        primitive_cells: &[u32],
        primitive_limit: u32,
        cell_limit: u32,
    ) -> anyhow::Result<ClusterDirectorySection> {
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals,
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: primitive_cells.iter().copied().map(leaf).collect(),
            root_node_index: 0,
        };
        let locator = CellLocatorSection {
            root: CellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        let sh = OctahedralShVolumeSection::placeholder();
        let view =
            FinalizedShEmissionView::new(&sh, None, None, None, None, None, None, None).unwrap();
        bake_cluster_directory_with_limits(
            &cells,
            &portals,
            &bvh,
            &locator,
            view,
            primitive_limit,
            cell_limit,
        )
        .map(|baked| baked.directory)
    }

    #[test]
    fn cluster_directory_partition_is_stable_under_portal_and_primitive_permutation() {
        let cells = CellsSection {
            cells: vec![cell(0.0), cell(1.0), cell(2.0), cell(3.0)],
            portal_refs: Vec::new(),
        };
        let first = bake(
            cells.clone(),
            vec![portal(2, 3), portal(0, 1), portal(1, 2)],
            &[0, 1, 2, 3],
            2,
            4,
        );
        let second = bake(
            cells,
            vec![portal(1, 2), portal(0, 1), portal(2, 3)],
            &[3, 1, 0, 2],
            2,
            4,
        );
        assert_eq!(
            first.try_to_bytes().unwrap(),
            second.try_to_bytes().unwrap()
        );
        assert_eq!(first.members, vec![0, 1, 2, 3]);
        assert_eq!(first.clusters.len(), 2);
    }

    #[test]
    fn cluster_directory_partition_reconsiders_frontier_after_nonfitting_candidate() {
        let cells = CellsSection {
            cells: vec![cell(0.0), cell(1.0), cell(2.0)],
            portal_refs: Vec::new(),
        };
        // Cell 1 is the first frontier key but has two primitives; after the
        // seed's one primitive it cannot fit. Cell 2 still fits and must join.
        let directory = bake(cells, vec![portal(0, 1), portal(0, 2)], &[0, 1, 1, 2], 2, 3);
        assert_eq!(directory.clusters[0].member_count, 2);
        assert_eq!(&directory.members[0..2], &[0, 2]);
    }

    #[test]
    fn compiler_cluster_directory_round_trips_shared_canonical_validation() {
        let cells = CellsSection {
            cells: vec![cell(0.0), cell(1.0), cell(2.0)],
            portal_refs: Vec::new(),
        };
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: vec![portal(0, 1), portal(1, 2)],
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: (0..3).map(leaf).collect(),
            root_node_index: 0,
        };
        let locator = CellLocatorSection {
            root: CellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        let sh = OctahedralShVolumeSection::placeholder();
        let view =
            FinalizedShEmissionView::new(&sh, None, None, None, None, None, None, None).unwrap();
        let baked =
            bake_cluster_directory_with_limits(&cells, &portals, &bvh, &locator, view, 2, 3)
                .unwrap();

        let parsed =
            ClusterDirectorySection::from_bytes(&baked.directory.try_to_bytes().unwrap()).unwrap();
        parsed
            .validate_semantics(ClusterDirectoryValidationInputs {
                cells: &cells,
                portals: &portals,
                bvh: &bvh,
                cell_locator: &locator,
                sh: view.inventory(),
            })
            .unwrap();
        assert_eq!(parsed, baked.directory);
    }

    #[test]
    fn resolved_hints_cut_partition_and_merge_cluster_policy_records() {
        let cells = CellsSection {
            cells: vec![cell(0.0), cell(1.0)],
            portal_refs: Vec::new(),
        };
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: vec![portal(0, 1)],
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: vec![leaf(0), leaf(1)],
            root_node_index: 0,
        };
        let locator = CellLocatorSection {
            root: CellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        let sh = OctahedralShVolumeSection::placeholder();
        let view =
            FinalizedShEmissionView::new(&sh, None, None, None, None, None, None, None).unwrap();
        let hints = ResolvedStreamingHints {
            seam_portal_ids: vec![0],
            pinned_cell_ids: vec![0],
            cell_priorities: vec![(1, 2), (1, 3)],
        };

        let error = match bake_cluster_directory(&cells, &portals, &bvh, &locator, view, &hints) {
            Ok(_) => panic!("duplicate priority cells must be rejected before directory emission"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("strictly ascending"));

        let hints = ResolvedStreamingHints {
            seam_portal_ids: vec![0],
            pinned_cell_ids: vec![0],
            cell_priorities: vec![(1, 3)],
        };
        let baked = bake_cluster_directory(&cells, &portals, &bvh, &locator, view, &hints).unwrap();
        assert_eq!(baked.directory.seam_portal_ids, vec![0]);
        assert_eq!(baked.directory.members, vec![0, 1]);
        assert_eq!(baked.directory.clusters.len(), 2);
        assert_eq!(
            baked.directory.cluster_hints,
            vec![
                ClusterHintRecord {
                    cluster_id: 0,
                    flags: CLUSTER_HINT_FLAG_PINNED,
                    priority: 0,
                },
                ClusterHintRecord {
                    cluster_id: 1,
                    flags: 0,
                    priority: 3,
                },
            ]
        );
        assert_eq!(baked.directory.clusters[0].flags, 0);
        assert_eq!(baked.directory.clusters[1].flags, 0);
    }

    #[test]
    fn cluster_directory_partition_keeps_disconnected_cells_and_flags_only_indivisible_overage() {
        let cells = CellsSection {
            cells: vec![cell(0.0), cell(10.0), cell(20.0)],
            portal_refs: Vec::new(),
        };
        let directory = bake(cells, Vec::new(), &[1, 1, 1, 1], 3, 8);
        assert_eq!(directory.clusters.len(), 3);
        assert_eq!(directory.clusters[0].flags, 0);
        assert_eq!(
            directory.clusters[1].flags,
            CLUSTER_FLAG_INDIVISIBLE_OVERSIZE
        );
        assert_eq!(directory.clusters[2].flags, 0);
    }

    #[test]
    fn cluster_directory_rejects_zero_limits() {
        let cells = CellsSection {
            cells: vec![cell(0.0)],
            portal_refs: Vec::new(),
        };
        assert!(bake_result(cells.clone(), Vec::new(), &[], 0, 1).is_err());
        assert!(bake_result(cells, Vec::new(), &[], 1, 0).is_err());
    }

    #[test]
    fn cluster_directory_rejects_portal_endpoint_outside_cells() {
        let cells = CellsSection {
            cells: vec![cell(0.0)],
            portal_refs: Vec::new(),
        };
        assert!(bake_result(cells, vec![portal(0, 1)], &[], 1, 1).is_err());
    }

    #[test]
    fn construction_metadata_bytes_reports_overflow_before_allocation() {
        // Regression: stats multiplication could wrap before checked addition.
        assert!(construction_metadata_bytes(usize::MAX, 0, 0, 0, 0, 0).is_err());
        assert!(construction_metadata_bytes(0, usize::MAX, 0, 0, 0, 0).is_err());
        assert!(construction_metadata_bytes(0, 0, usize::MAX, 0, 0, 0).is_err());
        assert!(construction_metadata_bytes(0, 0, 0, usize::MAX, 0, 0).is_err());
        assert!(construction_metadata_bytes(0, 0, 0, 0, usize::MAX, 0).is_err());
        assert!(construction_metadata_bytes(0, 0, 0, 0, 0, usize::MAX).is_err());
    }

    #[test]
    fn construction_metadata_bytes_preserves_normal_accounting() {
        let expected = 2 * size_of::<ClusterRecord>()
            + 3 * size_of::<u32>()
            + 5 * size_of::<postretro_level_format::cluster_directory::ClusterRangeRecord>()
            + 7 * 2
            + 11 * 40
            + 13 * 20;
        assert_eq!(
            construction_metadata_bytes(2, 3, 5, 7, 11, 13).unwrap(),
            expected
        );
    }

    #[test]
    #[ignore = "measurement helper; set POSTRETRO_CLUSTER_DRY_RUN_PRL"]
    fn cluster_directory_threshold_dry_run_from_prl() {
        use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
        use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
        use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
        use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
        use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
        use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
        use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
        use postretro_level_format::{SectionId, read_container, read_section_data};
        use std::io::Cursor;

        let path = std::env::var("POSTRETRO_CLUSTER_DRY_RUN_PRL")
            .expect("POSTRETRO_CLUSTER_DRY_RUN_PRL must name a compiler-produced PRL");
        let bytes = std::fs::read(path).unwrap();
        let mut cursor = Cursor::new(bytes);
        let meta = read_container(&mut cursor).unwrap();
        let mut section = |id: SectionId| {
            read_section_data(&mut cursor, &meta, id as u32)
                .unwrap()
                .unwrap_or_else(|| panic!("missing {id:?}"))
        };
        let cells = CellsSection::from_bytes(&section(SectionId::Cells)).unwrap();
        let portals = PortalsSection::from_bytes(&section(SectionId::Portals)).unwrap();
        let bvh = BvhSection::from_bytes(&section(SectionId::Bvh)).unwrap();
        let locator = CellLocatorSection::from_bytes(
            &section(SectionId::CellLocator),
            cells.cells.len() as u32,
        )
        .unwrap();
        let base =
            OctahedralShVolumeSection::from_bytes(&section(SectionId::OctahedralShVolume)).unwrap();
        macro_rules! optional {
            ($id:expr, $ty:ty) => {{
                read_section_data(&mut cursor, &meta, $id as u32)
                    .unwrap()
                    .map(|bytes| <$ty>::from_bytes(&bytes).unwrap())
            }};
        }
        let direct = optional!(SectionId::DirectShVolume, DirectShVolumeSection);
        let delta = optional!(SectionId::DeltaShVolumes, DeltaShVolumesSection);
        let selection = optional!(SectionId::EntityShadowLights, EntityShadowLightsSection);
        let direct_delta = optional!(SectionId::DirectShDeltaVolumes, DirectShDeltaVolumesSection);
        let animated = optional!(
            SectionId::AnimatedDirectShDeltaVolumes,
            AnimatedDirectShDeltaVolumesSection
        );
        let billboard = optional!(
            SectionId::BillboardDirectScatterVolume,
            BillboardDirectScatterVolumeSection
        );
        let animated_billboard = optional!(
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            AnimatedBillboardDirectScatterDeltaVolumesSection
        );
        let view = FinalizedShEmissionView::new(
            &base,
            direct.as_ref(),
            delta.as_ref(),
            selection.as_ref(),
            direct_delta.as_ref(),
            animated.as_ref(),
            billboard.as_ref(),
            animated_billboard.as_ref(),
        )
        .unwrap();
        for &(primitive_limit, cell_limit) in &[(32, 16), (64, 32), (128, 64)] {
            let baked = bake_cluster_directory_with_limits(
                &cells,
                &portals,
                &bvh,
                &locator,
                view,
                primitive_limit,
                cell_limit,
            )
            .unwrap();
            let mut primitive_counts: Vec<_> = baked
                .directory
                .clusters
                .iter()
                .map(|cluster| cluster.primitive_count)
                .collect();
            let mut cell_counts: Vec<_> = baked
                .directory
                .clusters
                .iter()
                .map(|cluster| cluster.member_count)
                .collect();
            primitive_counts.sort_unstable();
            cell_counts.sort_unstable();
            let percentile = |values: &[u32], percentile: usize| {
                values[(values.len().saturating_sub(1) * percentile) / 100]
            };
            let dense_addresses: u64 = baked
                .directory
                .ranges
                .iter()
                .filter(|range| {
                    range.role == postretro_level_format::cluster_directory::ClusterRangeRole::Dense
                })
                .map(|range| u64::from(range.count))
                .sum();
            let affinity_addresses: u64 = baked
                .directory
                .ranges
                .iter()
                .filter(|range| {
                    range.role != postretro_level_format::cluster_directory::ClusterRangeRole::Dense
                })
                .map(|range| u64::from(range.count))
                .sum();
            let owned_affinity_addresses: u64 = baked
                .directory
                .ranges
                .iter()
                .filter(|range| {
                    range.role == postretro_level_format::cluster_directory::ClusterRangeRole::Owned
                })
                .map(|range| u64::from(range.count))
                .sum();
            let mut per_cluster_dense = Vec::new();
            let mut per_cluster_affinity = Vec::new();
            for cluster in &baked.directory.clusters {
                let begin = cluster.range_start as usize;
                let end = begin + cluster.range_count as usize;
                per_cluster_dense.push(
                    baked.directory.ranges[begin..end]
                        .iter()
                        .filter(|range| range.role == postretro_level_format::cluster_directory::ClusterRangeRole::Dense)
                        .map(|range| u64::from(range.count))
                        .sum::<u64>(),
                );
                per_cluster_affinity.push(
                    baked.directory.ranges[begin..end]
                        .iter()
                        .filter(|range| range.role != postretro_level_format::cluster_directory::ClusterRangeRole::Dense)
                        .map(|range| u64::from(range.count))
                        .sum::<u64>(),
                );
            }
            println!(
                "candidate={primitive_limit}/{cell_limit} clusters={} primitive_p50/p95/max={}/{}/{} cell_p50/p95/max={}/{}/{} oversize={} directory_bytes={} dense_addresses={} dense_max_per_cluster={} affinity_addresses={} affinity_max_per_cluster={} affinity_owned/halo={}/{} covering_refs={} construction_metadata_upper_bytes={} serialization_peak_upper_bytes={} elapsed_ms={:.3}",
                baked.stats.cluster_count,
                percentile(&primitive_counts, 50),
                percentile(&primitive_counts, 95),
                primitive_counts.last().copied().unwrap_or(0),
                percentile(&cell_counts, 50),
                percentile(&cell_counts, 95),
                cell_counts.last().copied().unwrap_or(0),
                baked.stats.oversize_singletons,
                baked.stats.directory_bytes,
                dense_addresses,
                per_cluster_dense.iter().copied().max().unwrap_or(0),
                affinity_addresses,
                per_cluster_affinity.iter().copied().max().unwrap_or(0),
                owned_affinity_addresses,
                affinity_addresses - owned_affinity_addresses,
                baked.stats.covering_references,
                baked.stats.construction_metadata_bytes,
                baked.stats.construction_metadata_bytes + baked.stats.directory_bytes,
                baked.stats.elapsed.as_secs_f64() * 1000.0,
            );
        }

        let fixture_cells = CellsSection {
            cells: (0..12).map(|index| cell(index as f32)).collect(),
            portal_refs: Vec::new(),
        };
        let fixture_portals = vec![
            portal(0, 1),
            portal(0, 2),
            portal(1, 3),
            portal(2, 4),
            portal(3, 5),
            portal(4, 6),
            portal(5, 7),
            portal(6, 7),
        ];
        let mut fixture_primitives = Vec::new();
        for (cell_id, count) in [5usize, 30, 10, 20, 8, 2, 16, 1, 0, 0, 140, 0]
            .into_iter()
            .enumerate()
        {
            fixture_primitives.extend(std::iter::repeat_n(cell_id as u32, count));
        }
        for &(primitive_limit, cell_limit) in &[(32, 4), (64, 8), (128, 16)] {
            let directory = bake(
                fixture_cells.clone(),
                fixture_portals.clone(),
                &fixture_primitives,
                primitive_limit,
                cell_limit,
            );
            let primitive_max = directory
                .clusters
                .iter()
                .map(|cluster| cluster.primitive_count)
                .max()
                .unwrap_or(0);
            let cell_max = directory
                .clusters
                .iter()
                .map(|cluster| cluster.member_count)
                .max()
                .unwrap_or(0);
            let oversize = directory
                .clusters
                .iter()
                .filter(|cluster| cluster.flags == CLUSTER_FLAG_INDIVISIBLE_OVERSIZE)
                .count();
            println!(
                "fixture_candidate={primitive_limit}/{cell_limit} clusters={} primitive_max={primitive_max} cell_max={cell_max} oversize={oversize} directory_bytes={}",
                directory.clusters.len(),
                directory.byte_len().unwrap(),
            );
        }
    }
}
