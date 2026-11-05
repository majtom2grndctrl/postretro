// Cell-cluster metadata and grid-relative SH resource addressing (PRL id 49).
// See: context/lib/build_pipeline.md §PRL section IDs

use thiserror::Error;

use crate::{
    SectionId,
    animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection,
    animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    billboard_direct_scatter_volume::{
        BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16, BillboardDirectScatterVolumeSection,
    },
    bvh::BvhSection,
    cell_locator::{CellLocatorChild, CellLocatorSection},
    cells::CellsSection,
    delta_sh_volumes::DeltaShVolumesSection,
    direct_sh_delta_volumes::DirectShDeltaVolumesSection,
    direct_sh_volume::DirectShVolumeSection,
    entity_shadow_lights::EntityShadowLightsSection,
    portals::PortalsSection,
    sh_volume::OctahedralShVolumeSection,
};

#[path = "cluster_directory/bound.rs"]
mod bound;
#[path = "cluster_directory/canonical_partition.rs"]
mod canonical_partition;
#[path = "cluster_directory/coverage.rs"]
mod coverage;
#[cfg(test)]
#[path = "cluster_directory/coverage_oracle.rs"]
mod coverage_oracle;
#[cfg(test)]
#[path = "cluster_directory/coverage_tests.rs"]
mod coverage_tests;
#[path = "cluster_directory/probe_locate.rs"]
mod probe_locate;
#[cfg(test)]
#[path = "cluster_directory/tests.rs"]
mod tests;
#[path = "cluster_directory/wire.rs"]
mod wire;

pub use bound::{
    BOUND_ENTRY_HEADER_SIZE, BOUND_FAMILY_RECORD_SIZE, MAX_BOUND_ENTRIES, ShMandatoryBoundEntry,
    ShMandatoryBoundFamily, WHOLE_MAP_WORST_CELL,
};
pub use canonical_partition::{CanonicalCellPartition, canonical_cell_partition};

pub const CLUSTER_DIRECTORY_VERSION: u32 = 3;
pub const CLUSTER_DIRECTORY_CONTAINER_VERSION: u16 = 3;
pub const HEADER_SIZE: usize = 40;
pub const CLUSTER_RECORD_SIZE: usize = 48;
pub const RESOURCE_RECORD_SIZE: usize = 24;
pub const MEMBER_RECORD_SIZE: usize = 4;
pub const RANGE_RECORD_SIZE: usize = 16;
pub const CLUSTER_HINT_RECORD_SIZE: usize = 16;

pub const CLUSTER_FLAG_INDIVISIBLE_OVERSIZE: u32 = 1;
pub const CLUSTER_HINT_FLAG_PINNED: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ClusterResourceDomain {
    DenseProbe = 0,
    AffinityCell = 1,
}

impl ClusterResourceDomain {
    fn parse(value: u32) -> Result<Self, ClusterDirectoryError> {
        match value {
            0 => Ok(Self::DenseProbe),
            1 => Ok(Self::AffinityCell),
            _ => Err(ClusterDirectoryError::InvalidData(format!(
                "resource domain {value} is unknown"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterRecord {
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub member_start: u32,
    pub member_count: u32,
    pub range_start: u32,
    pub range_count: u32,
    pub primitive_count: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterResourceRecord {
    pub section_id: u32,
    pub domain: ClusterResourceDomain,
    pub dimensions: [u32; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterRangeRecord {
    pub resource_index: u32,
    pub start: u32,
    pub count: u32,
}

/// Compiler-authored policy metadata for one canonical cluster.
///
/// The fourth on-wire word is reserved and deliberately omitted here: it is
/// required to be zero while decoding and is not a semantic input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterHintRecord {
    pub cluster_id: u32,
    pub flags: u32,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterDirectorySection {
    pub runtime_cell_count: u32,
    pub primitive_limit: u32,
    pub cell_limit: u32,
    pub clusters: Vec<ClusterRecord>,
    pub resources: Vec<ClusterResourceRecord>,
    pub members: Vec<u32>,
    pub ranges: Vec<ClusterRangeRecord>,
    /// Sorted, unique portal IDs whose endpoints must not share a cluster.
    pub seam_portal_ids: Vec<u32>,
    /// Sorted, non-no-op policy records keyed by canonical cluster ID.
    pub cluster_hints: Vec<ClusterHintRecord>,
    /// Worst mandatory SH demand of any camera cell, ascending by lead. Empty
    /// on a level with no id 34.
    pub mandatory_bound: Vec<ShMandatoryBoundEntry>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClusterDirectoryError {
    #[error(
        "ClusterDirectoryVersionMismatch: version {version}, expected {expected}; re-bake the level"
    )]
    VersionMismatch { version: u32, expected: u32 },
    #[error("ClusterDirectorySeamPortalOrder: portal ids must be strictly ascending")]
    SeamPortalOrder,
    #[error("ClusterDirectorySeamPortalOutOfRange: portal {portal}, count {count}")]
    SeamPortalOutOfRange { portal: u32, count: usize },
    #[error(
        "ClusterDirectorySeamPortalCellOutOfRange: portal {portal}, endpoints ({front}, {back}), cell count {cell_count}"
    )]
    SeamPortalCellOutOfRange {
        portal: u32,
        front: u32,
        back: u32,
        cell_count: u32,
    },
    #[error("ClusterDirectorySeamPortalSameCell: portal {portal}, cell {cell}")]
    SeamPortalSameCell { portal: u32, cell: u32 },
    #[error(
        "ClusterDirectorySeamPortalSameCluster: portal {portal}, endpoints ({front}, {back}), cluster {cluster}"
    )]
    SeamPortalSameCluster {
        portal: u32,
        front: u32,
        back: u32,
        cluster: u32,
    },
    #[error("ClusterDirectoryHintOrder: cluster ids must be strictly ascending")]
    ClusterHintOrder,
    #[error("ClusterDirectoryHintClusterOutOfRange: hint {hint}, cluster {cluster}, count {count}")]
    ClusterHintClusterOutOfRange {
        hint: usize,
        cluster: u32,
        count: u32,
    },
    #[error("ClusterDirectoryHintFlagsInvalid: hint {hint}, flags {flags:#x}")]
    ClusterHintFlagsInvalid { hint: usize, flags: u32 },
    #[error("ClusterDirectoryHintPriorityOutOfRange: hint {hint}, priority {priority}")]
    ClusterHintPriorityOutOfRange { hint: usize, priority: u32 },
    #[error("ClusterDirectoryHintNoOp: hint {hint}")]
    ClusterHintNoOp { hint: usize },
    #[error("ClusterDirectoryHintReserved: hint {hint}, reserved {reserved}")]
    ClusterHintReserved { hint: u32, reserved: u32 },
    #[error("ClusterDirectoryBoundTruncated: {needed} tail bytes past {available} available")]
    BoundTruncated { needed: usize, available: usize },
    #[error("ClusterDirectoryBoundTrailingBytes: {extra} bytes past the last bound entry")]
    BoundTrailingBytes { extra: usize },
    #[error("ClusterDirectoryBoundCount: {count} entries, at most {limit}")]
    BoundCount { count: u32, limit: u32 },
    #[error("ClusterDirectoryBoundLeadOrder: leads must be strictly ascending")]
    BoundLeadOrder,
    #[error("ClusterDirectoryBoundCellOutOfRange: entry {entry}, cell {cell}, cell count {count}")]
    BoundCellOutOfRange { entry: usize, cell: u32, count: u32 },
    #[error(
        "ClusterDirectoryBoundWholeMapShape: the whole-map marker needs one entry at lead 0; entry {entry}, lead {lead}, {entries} entries"
    )]
    BoundWholeMapShape {
        entry: usize,
        lead: u32,
        entries: usize,
    },
    #[error(
        "ClusterDirectoryBoundFamilyOrder: entry {entry} resource indices must be strictly ascending"
    )]
    BoundFamilyOrder { entry: usize },
    #[error(
        "ClusterDirectoryBoundFamilyResource: entry {entry}, resource {resource} is not an affinity-domain resource of {count}"
    )]
    BoundFamilyResource {
        entry: usize,
        resource: u32,
        count: usize,
    },
    #[error("ClusterDirectoryBoundWithoutProbes: a directory with no id 34 stores no bound")]
    BoundWithoutProbes,
    #[error("ClusterDirectoryCanonicalPartitionMismatch: {0}")]
    CanonicalPartitionMismatch(String),
    #[error("ClusterDirectoryInvalidData: {0}")]
    InvalidData(String),
    #[error("ClusterDirectoryCellOutOfRange: cluster {cluster}, cell {cell}, cell limit {limit}")]
    CellOutOfRange { cluster: u32, cell: u32, limit: u32 },
    #[error(
        "ClusterDirectoryGridRangeOutOfRange: cluster {cluster}, resource {resource}, start {start}, count {count}, limit {limit}"
    )]
    GridRangeOutOfRange {
        cluster: u32,
        resource: u32,
        start: u32,
        count: u32,
        limit: u32,
    },
    #[error("ClusterDirectorySizeOverflow: {0}")]
    SizeOverflow(&'static str),
    #[error("ClusterDirectoryAllocationFailed: {0}")]
    AllocationFailed(&'static str),
    #[error("ClusterDirectoryMissingResource: {0}")]
    MissingResource(String),
    #[error("ClusterDirectoryResourceMismatch: {0}")]
    ResourceMismatch(String),
}

/// Parsed-valid, policy-available SH sections supplied without copying payloads.
/// Presence in this inventory is the exact emitted/on-wire resource inventory.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterDirectoryShInventory<'a> {
    pub octahedral: Option<&'a OctahedralShVolumeSection>,
    pub direct: Option<&'a DirectShVolumeSection>,
    pub delta: Option<&'a DeltaShVolumesSection>,
    pub shadow_selection: Option<&'a EntityShadowLightsSection>,
    pub direct_delta: Option<&'a DirectShDeltaVolumesSection>,
    pub animated_direct_delta: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub billboard: Option<&'a BillboardDirectScatterVolumeSection>,
    pub animated_billboard_delta: Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
}

/// Borrowed cross-section evidence for the shared semantic validator.
#[derive(Debug, Clone, Copy)]
pub struct ClusterDirectoryValidationInputs<'a> {
    pub cells: &'a CellsSection,
    pub portals: &'a PortalsSection,
    pub bvh: &'a BvhSection,
    pub cell_locator: &'a CellLocatorSection,
    pub sh: ClusterDirectoryShInventory<'a>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClusterDirectoryCoverageStats {
    pub affinity_cell_count: usize,
    pub active_affinity_cell_count: usize,
    pub covering_references: usize,
    pub maximum_visited_nodes_per_cluster: usize,
}

/// Build the canonical resource table for an emitted SH inventory.
pub fn canonical_resource_records(
    inventory: ClusterDirectoryShInventory<'_>,
) -> Result<Vec<ClusterResourceRecord>, ClusterDirectoryError> {
    let base_dims = inventory
        .octahedral
        .map_or([0, 0, 0], |base| base.grid_dimensions);
    let affinity_dims = affinity_dimensions(base_dims)?;
    let candidates = [
        (
            SectionId::DeltaShVolumes,
            inventory.delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::OctahedralShVolume,
            inventory.octahedral.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::DirectShVolume,
            inventory.direct.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::DirectShDeltaVolumes,
            inventory.direct_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            inventory.animated_direct_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::BillboardDirectScatterVolume,
            inventory.billboard.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            inventory.animated_billboard_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
    ];
    Ok(candidates
        .into_iter()
        .filter(|(_, present, _, _)| *present)
        .map(|(section, _, domain, dimensions)| ClusterResourceRecord {
            section_id: section as u32,
            domain,
            dimensions,
        })
        .collect())
}

/// Populate canonical resource rows and grid-relative ranges for an already
/// partitioned directory. Sparse coverage derivation is shared with load-time
/// validation.
pub fn populate_canonical_resource_ranges(
    directory: &mut ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
) -> Result<ClusterDirectoryCoverageStats, ClusterDirectoryError> {
    directory.resources = canonical_resource_records(inputs.sh)?;
    let Some(base) = inputs.sh.octahedral else {
        directory.ranges.clear();
        for cluster in &mut directory.clusters {
            cluster.range_start = 0;
            cluster.range_count = 0;
        }
        return Ok(ClusterDirectoryCoverageStats::default());
    };
    let affinity_dims = affinity_dimensions(base.grid_dimensions)?;
    validate_companions(base, inputs.sh, affinity_dims)?;
    let (ranges, counts, stats) =
        coverage::derive_ranges_with_counts(directory, inputs, base, affinity_dims)?;
    let mut start = 0u32;
    for (cluster, count) in directory.clusters.iter_mut().zip(counts) {
        cluster.range_start = if count == 0 { 0 } else { start };
        cluster.range_count = count;
        start = start
            .checked_add(count)
            .ok_or(ClusterDirectoryError::SizeOverflow("cluster range total"))?;
    }
    directory.ranges = ranges;
    Ok(stats)
}

impl ClusterDirectorySection {
    pub fn validate_structure(&self) -> Result<(), ClusterDirectoryError> {
        if self.primitive_limit == 0 || self.cell_limit == 0 {
            return invalid("primitive_limit and cell_limit must be positive");
        }
        if self.members.len() != self.runtime_cell_count as usize {
            return invalid(format!(
                "member count {} does not equal runtime_cell_count {}",
                self.members.len(),
                self.runtime_cell_count
            ));
        }
        if self.runtime_cell_count == 0 {
            if !self.clusters.is_empty()
                || !self.members.is_empty()
                || !self.resources.is_empty()
                || !self.ranges.is_empty()
                || !self.seam_portal_ids.is_empty()
                || !self.cluster_hints.is_empty()
                || !self.mandatory_bound.is_empty()
            {
                return invalid(
                    "standalone empty directory must contain no clusters, members, resources, ranges, seams, hints, or bound entries",
                );
            }
            return Ok(());
        }
        if self.clusters.is_empty() {
            return invalid("nonzero runtime_cell_count requires at least one cluster");
        }

        let mut expected_member_start = 0u32;
        let mut expected_range_start = 0u32;
        let mut seen_cells = vec![false; self.runtime_cell_count as usize];
        for (cluster_id, cluster) in self.clusters.iter().enumerate() {
            validate_bounds(cluster_id, cluster.bounds_min, cluster.bounds_max)?;
            if cluster.flags & !CLUSTER_FLAG_INDIVISIBLE_OVERSIZE != 0 {
                return invalid(format!(
                    "cluster {cluster_id} has unknown flags {:#x}",
                    cluster.flags
                ));
            }
            if cluster.member_count == 0 {
                return invalid(format!("cluster {cluster_id} has no members"));
            }
            if cluster.member_count > self.cell_limit {
                return invalid(format!(
                    "cluster {cluster_id} member_count {} exceeds cell_limit {}",
                    cluster.member_count, self.cell_limit
                ));
            }
            let oversize = cluster.primitive_count > self.primitive_limit;
            if oversize != (cluster.flags == CLUSTER_FLAG_INDIVISIBLE_OVERSIZE)
                || (oversize && cluster.member_count != 1)
            {
                return invalid(format!(
                    "cluster {cluster_id} oversize flag does not match singleton primitive exception"
                ));
            }
            validate_slice(
                cluster_id,
                "member",
                cluster.member_start,
                cluster.member_count,
                expected_member_start,
                self.members.len(),
            )?;
            validate_slice(
                cluster_id,
                "range",
                cluster.range_start,
                cluster.range_count,
                expected_range_start,
                self.ranges.len(),
            )?;
            expected_member_start = expected_member_start
                .checked_add(cluster.member_count)
                .ok_or(ClusterDirectoryError::SizeOverflow("member slice end"))?;
            expected_range_start = expected_range_start
                .checked_add(cluster.range_count)
                .ok_or(ClusterDirectoryError::SizeOverflow("range slice end"))?;

            let members =
                &self.members[cluster.member_start as usize..expected_member_start as usize];
            for pair in members.windows(2) {
                if pair[0] >= pair[1] {
                    return invalid(format!(
                        "cluster {cluster_id} members are not strictly ascending"
                    ));
                }
            }
            for &cell in members {
                if cell >= self.runtime_cell_count {
                    return Err(ClusterDirectoryError::CellOutOfRange {
                        cluster: cluster_id as u32,
                        cell,
                        limit: self.runtime_cell_count,
                    });
                }
                if std::mem::replace(&mut seen_cells[cell as usize], true) {
                    return invalid(format!("cell {cell} occurs in more than one cluster"));
                }
            }
        }
        if expected_member_start as usize != self.members.len()
            || !seen_cells.into_iter().all(|seen| seen)
        {
            return invalid("cluster member slices do not consume every runtime cell exactly once");
        }
        if expected_range_start as usize != self.ranges.len() {
            return invalid("cluster range slices do not consume the range table");
        }

        for pair in self.seam_portal_ids.windows(2) {
            if pair[0] >= pair[1] {
                return Err(ClusterDirectoryError::SeamPortalOrder);
            }
        }
        let cluster_count = u32_len(self.clusters.len(), "cluster count")?;
        for (hint_index, hint) in self.cluster_hints.iter().enumerate() {
            if hint.cluster_id >= cluster_count {
                return Err(ClusterDirectoryError::ClusterHintClusterOutOfRange {
                    hint: hint_index,
                    cluster: hint.cluster_id,
                    count: cluster_count,
                });
            }
            if hint.flags & !CLUSTER_HINT_FLAG_PINNED != 0 {
                return Err(ClusterDirectoryError::ClusterHintFlagsInvalid {
                    hint: hint_index,
                    flags: hint.flags,
                });
            }
            if hint.priority > 3 {
                return Err(ClusterDirectoryError::ClusterHintPriorityOutOfRange {
                    hint: hint_index,
                    priority: hint.priority,
                });
            }
            if hint.flags == 0 && hint.priority == 0 {
                return Err(ClusterDirectoryError::ClusterHintNoOp { hint: hint_index });
            }
            if hint_index > 0 && self.cluster_hints[hint_index - 1].cluster_id >= hint.cluster_id {
                return Err(ClusterDirectoryError::ClusterHintOrder);
            }
        }

        for (index, resource) in self.resources.iter().enumerate() {
            let expected_domain = resource_domain(resource.section_id).ok_or_else(|| {
                ClusterDirectoryError::InvalidData(format!(
                    "resource {index} names unsupported section {}",
                    resource.section_id
                ))
            })?;
            if resource.domain != expected_domain {
                return invalid(format!(
                    "resource {index} domain disagrees with section {}",
                    resource.section_id
                ));
            }
            if index > 0 && self.resources[index - 1].section_id >= resource.section_id {
                return invalid("resource section ids must be strictly ascending");
            }
            let zero_axes = resource.dimensions.iter().filter(|&&d| d == 0).count();
            if zero_axes != 0 && zero_axes != 3 {
                return invalid(format!(
                    "resource {index} has mixed-zero dimensions {:?}",
                    resource.dimensions
                ));
            }
            checked_product(resource.dimensions)?;
        }

        for (cluster_id, cluster) in self.clusters.iter().enumerate() {
            let begin = cluster.range_start as usize;
            let end = begin + cluster.range_count as usize;
            let mut previous: Option<&ClusterRangeRecord> = None;
            for range in &self.ranges[begin..end] {
                let resource = self
                    .resources
                    .get(range.resource_index as usize)
                    .ok_or_else(|| {
                        ClusterDirectoryError::InvalidData(format!(
                            "cluster {cluster_id} range names resource {} outside {}",
                            range.resource_index,
                            self.resources.len()
                        ))
                    })?;
                if range.count == 0 {
                    return invalid(format!("cluster {cluster_id} range count must be positive"));
                }
                let limit = checked_product(resource.dimensions)?;
                let range_end = range
                    .start
                    .checked_add(range.count)
                    .ok_or(ClusterDirectoryError::SizeOverflow("grid range end"))?;
                if range_end > limit {
                    return Err(ClusterDirectoryError::GridRangeOutOfRange {
                        cluster: cluster_id as u32,
                        resource: range.resource_index,
                        start: range.start,
                        count: range.count,
                        limit,
                    });
                }
                if let Some(prev) = previous {
                    if (prev.resource_index, prev.start) >= (range.resource_index, range.start) {
                        return invalid(format!("cluster {cluster_id} ranges are not sorted"));
                    }
                    if prev.resource_index == range.resource_index {
                        let prev_end = prev
                            .start
                            .checked_add(prev.count)
                            .ok_or(ClusterDirectoryError::SizeOverflow("previous range end"))?;
                        if prev_end > range.start {
                            return invalid(format!("cluster {cluster_id} ranges overlap"));
                        }
                        if prev_end == range.start {
                            return invalid(format!(
                                "cluster {cluster_id} adjacent ranges were not coalesced"
                            ));
                        }
                    }
                }
                previous = Some(range);
            }
        }
        bound::validate_bound_entries(self)
    }

    pub fn validate_semantics(
        &self,
        inputs: ClusterDirectoryValidationInputs<'_>,
    ) -> Result<(), ClusterDirectoryError> {
        self.validate_structure()?;
        validate_cells(self, inputs.cells, inputs.portals, inputs.bvh)?;
        validate_resources(self, inputs)?;
        Ok(())
    }
}

fn validate_cells(
    directory: &ClusterDirectorySection,
    cells: &CellsSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
) -> Result<(), ClusterDirectoryError> {
    if cells.cells.len() != directory.runtime_cell_count as usize {
        return invalid(format!(
            "runtime_cell_count {} disagrees with Cells count {}",
            directory.runtime_cell_count,
            cells.cells.len()
        ));
    }
    validate_seam_portals(directory, portals)?;
    let canonical = canonical_cell_partition(
        cells,
        portals,
        bvh,
        directory.primitive_limit,
        directory.cell_limit,
        &directory.seam_portal_ids,
    )?;
    for cluster_id in 0..directory.clusters.len().max(canonical.clusters.len()) {
        let Some(cluster) = directory.clusters.get(cluster_id) else {
            return canonical_mismatch(format!(
                "directory is missing canonical greedy partition cluster {cluster_id}"
            ));
        };
        let Some(expected) = canonical.clusters.get(cluster_id) else {
            return canonical_mismatch(format!(
                "cluster {cluster_id} is not present in the canonical greedy partition"
            ));
        };
        let member_begin = cluster.member_start as usize;
        let member_end = member_begin + cluster.member_count as usize;
        let members = &directory.members[member_begin..member_end];
        let expected_begin = expected.member_start as usize;
        let expected_end = expected_begin + expected.member_count as usize;
        let expected_members = &canonical.members[expected_begin..expected_end];
        if members != expected_members {
            let first_difference = members
                .iter()
                .zip(expected_members)
                .position(|(actual, canonical)| actual != canonical)
                .unwrap_or_else(|| members.len().min(expected_members.len()));
            return canonical_mismatch(format!(
                "cluster {cluster_id} member position {first_difference} disagrees with canonical greedy partition: actual {:?}, canonical {:?}",
                members.get(first_difference),
                expected_members.get(first_difference)
            ));
        }
        if !float_array_bits_equal(expected.bounds_min, cluster.bounds_min)
            || !float_array_bits_equal(expected.bounds_max, cluster.bounds_max)
        {
            return canonical_mismatch(format!(
                "cluster {cluster_id} bounds do not equal the exact member union"
            ));
        }
        if expected.primitive_count != cluster.primitive_count {
            return canonical_mismatch(format!(
                "cluster {cluster_id} primitive_count {} disagrees with canonical BVH count {}",
                cluster.primitive_count, expected.primitive_count
            ));
        }
        if expected.flags != cluster.flags {
            return canonical_mismatch(format!(
                "cluster {cluster_id} flags {:#x} disagree with canonical flags {:#x}",
                cluster.flags, expected.flags
            ));
        }
    }
    Ok(())
}

fn validate_seam_portals(
    directory: &ClusterDirectorySection,
    portals: &PortalsSection,
) -> Result<(), ClusterDirectoryError> {
    let mut cell_clusters = try_vec(directory.runtime_cell_count, "seam cell clusters")?;
    cell_clusters.resize(directory.runtime_cell_count as usize, 0u32);
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let member_begin = cluster.member_start as usize;
        let member_end = member_begin + cluster.member_count as usize;
        for &cell in &directory.members[member_begin..member_end] {
            cell_clusters[cell as usize] = cluster_id as u32;
        }
    }
    for &portal_id in &directory.seam_portal_ids {
        let portal = portals.portals.get(portal_id as usize).ok_or(
            ClusterDirectoryError::SeamPortalOutOfRange {
                portal: portal_id,
                count: portals.portals.len(),
            },
        )?;
        if portal.front_leaf >= directory.runtime_cell_count
            || portal.back_leaf >= directory.runtime_cell_count
        {
            return Err(ClusterDirectoryError::SeamPortalCellOutOfRange {
                portal: portal_id,
                front: portal.front_leaf,
                back: portal.back_leaf,
                cell_count: directory.runtime_cell_count,
            });
        }
        if portal.front_leaf == portal.back_leaf {
            return Err(ClusterDirectoryError::SeamPortalSameCell {
                portal: portal_id,
                cell: portal.front_leaf,
            });
        }
        if cell_clusters[portal.front_leaf as usize] == cell_clusters[portal.back_leaf as usize] {
            return Err(ClusterDirectoryError::SeamPortalSameCluster {
                portal: portal_id,
                front: portal.front_leaf,
                back: portal.back_leaf,
                cluster: cell_clusters[portal.front_leaf as usize],
            });
        }
    }
    Ok(())
}

fn validate_resources(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
) -> Result<(), ClusterDirectoryError> {
    let inventory = inputs.sh;
    if inventory.octahedral.is_none()
        && (inventory.direct.is_some()
            || inventory.delta.is_some()
            || inventory.direct_delta.is_some()
            || inventory.animated_direct_delta.is_some()
            || inventory.billboard.is_some()
            || inventory.animated_billboard_delta.is_some())
    {
        return Err(ClusterDirectoryError::ResourceMismatch(
            "SH companion is present without id 34".into(),
        ));
    }

    let base_dims = inventory
        .octahedral
        .map_or([0, 0, 0], |base| base.grid_dimensions);
    let affinity_dims = affinity_dimensions(base_dims)?;
    let expected = [
        (
            SectionId::DeltaShVolumes,
            inventory.delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::OctahedralShVolume,
            inventory.octahedral.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::DirectShVolume,
            inventory.direct.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::DirectShDeltaVolumes,
            inventory.direct_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            inventory.animated_direct_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
        (
            SectionId::BillboardDirectScatterVolume,
            inventory.billboard.is_some(),
            ClusterResourceDomain::DenseProbe,
            base_dims,
        ),
        (
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            inventory.animated_billboard_delta.is_some(),
            ClusterResourceDomain::AffinityCell,
            affinity_dims,
        ),
    ];
    let expected_rows: Vec<_> = expected
        .iter()
        .filter(|(_, present, _, _)| *present)
        .collect();
    if directory.resources.len() != expected_rows.len() {
        return Err(ClusterDirectoryError::MissingResource(format!(
            "directory has {} resource rows, emitted inventory requires {}",
            directory.resources.len(),
            expected_rows.len()
        )));
    }
    for (row, (section, _, domain, dimensions)) in directory.resources.iter().zip(expected_rows) {
        if row.section_id != *section as u32 {
            return Err(ClusterDirectoryError::MissingResource(format!(
                "expected section {}, found {}",
                *section as u32, row.section_id
            )));
        }
        if row.domain != *domain || row.dimensions != *dimensions {
            return Err(ClusterDirectoryError::ResourceMismatch(format!(
                "section {} row has domain/dimensions {:?}/{:?}, expected {:?}/{:?}",
                row.section_id, row.domain, row.dimensions, domain, dimensions
            )));
        }
    }

    let Some(base) = inventory.octahedral else {
        if !directory.ranges.is_empty() {
            return Err(ClusterDirectoryError::ResourceMismatch(
                "directory without id 34 must not contain ranges".into(),
            ));
        }
        if !directory.mandatory_bound.is_empty() {
            return Err(ClusterDirectoryError::BoundWithoutProbes);
        }
        return Ok(());
    };
    let probe_count = checked_product(base.grid_dimensions)? as usize;
    if base.probes.len() != probe_count {
        return resource_mismatch(format!(
            "id 34 probe metadata count {} disagrees with grid product {probe_count}",
            base.probes.len()
        ));
    }
    validate_companions(base, inventory, affinity_dims)?;
    validate_range_table(directory, inputs, base, affinity_dims)
}

/// Compare the baked range table with the derivation from cells, probes and
/// the cell locator.
fn validate_range_table(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
) -> Result<(), ClusterDirectoryError> {
    let expected_ranges = derive_expected_ranges(directory, inputs, base, affinity_dims)?;
    if expected_ranges != directory.ranges {
        let first = expected_ranges
            .iter()
            .zip(&directory.ranges)
            .position(|(expected, actual)| expected != actual)
            .unwrap_or(expected_ranges.len().min(directory.ranges.len()));
        return Err(ClusterDirectoryError::ResourceMismatch(format!(
            "range table disagrees with cell/grid coverage at range {first}: expected {} ranges, got {}",
            expected_ranges.len(),
            directory.ranges.len()
        )));
    }
    Ok(())
}

fn validate_companions(
    base: &OctahedralShVolumeSection,
    inventory: ClusterDirectoryShInventory<'_>,
    affinity_dims: [u32; 3],
) -> Result<(), ClusterDirectoryError> {
    if let Some(direct) = inventory.direct
        && (direct.grid_dimensions != base.grid_dimensions
            || !float_array_bits_equal(direct.grid_origin, base.grid_origin)
            || !float_array_bits_equal(direct.cell_size, base.cell_size)
            || direct.tile_dimension != base.tile_dimension
            || direct.tile_border != base.tile_border
            || direct.atlas_dimensions != base.atlas_dimensions
            || direct.atlas_tiles_per_row != base.atlas_tiles_per_row
            || direct.layer_count != base.layer_count
            || direct.tiles_per_layer != base.tiles_per_layer
            || direct.irradiance_format != base.irradiance_format)
    {
        return resource_mismatch("id 35 grid/stored-node layout disagrees with id 34");
    }
    if let Some(billboard) = inventory.billboard {
        if billboard.grid_dimensions != base.grid_dimensions
            || !float_array_bits_equal(billboard.grid_origin, base.grid_origin)
            || !float_array_bits_equal(billboard.cell_size, base.cell_size)
        {
            return resource_mismatch("id 47 grid disagrees with id 34");
        }
        let expected_scatter_len =
            base.probes
                .len()
                .checked_mul(4)
                .ok_or(ClusterDirectoryError::SizeOverflow(
                    "billboard scatter probe count",
                ))?;
        if billboard.scatter_rgba.len() != expected_scatter_len {
            return resource_mismatch("id 47 payload length disagrees with id 34 probe count");
        }
        for (probe_index, probe) in base.probes.iter().enumerate() {
            let expected = if probe.validity == 0 {
                0
            } else {
                BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16
            };
            if billboard.scatter_rgba[probe_index * 4 + 3] != expected {
                return resource_mismatch(format!(
                    "id 47 validity for probe {probe_index} disagrees with id 34"
                ));
            }
        }
    }
    if let Some(delta) = inventory.delta {
        validate_sparse_shape(
            27,
            delta.affinity_factor,
            delta.affinity_dims,
            &delta.affinity_offsets,
            affinity_dims,
        )?;
        validate_descriptor_indices(
            27,
            &delta.animation_descriptor_indices,
            base.animation_descriptors.len(),
        )?;
        validate_delta_storage(
            base,
            27,
            delta.tile_dimension,
            delta.tile_border,
            &delta.cell_levels,
            &delta.affinity_offsets,
        )?;
    }
    if let Some(delta) = inventory.direct_delta {
        validate_sparse_shape(
            41,
            delta.affinity_factor,
            delta.affinity_dims,
            &delta.affinity_offsets,
            affinity_dims,
        )?;
        let selection_count = inventory
            .shadow_selection
            .ok_or_else(|| {
                ClusterDirectoryError::ResourceMismatch(
                    "id 41 is present without id 40 selection".into(),
                )
            })?
            .light_indices
            .len();
        if let Some(&light) = delta
            .affinity_lights
            .iter()
            .find(|&&light| light as usize >= selection_count)
        {
            return resource_mismatch(format!(
                "id 41 selection index {light} outside id 40 count {selection_count}"
            ));
        }
        validate_delta_storage(
            base,
            41,
            delta.tile_dimension,
            delta.tile_border,
            &delta.cell_levels,
            &delta.affinity_offsets,
        )?;
    }
    if let Some(delta) = inventory.animated_direct_delta {
        validate_sparse_shape(
            45,
            delta.affinity_factor,
            delta.affinity_dims,
            &delta.affinity_offsets,
            affinity_dims,
        )?;
        validate_descriptor_indices(
            45,
            &delta.animation_descriptor_indices,
            base.animation_descriptors.len(),
        )?;
        validate_delta_storage(
            base,
            45,
            delta.tile_dimension,
            delta.tile_border,
            &delta.cell_levels,
            &delta.affinity_offsets,
        )?;
    }
    if let Some(delta) = inventory.animated_billboard_delta {
        let source = inventory.animated_direct_delta.ok_or_else(|| {
            ClusterDirectoryError::ResourceMismatch("id 48 is present without id 45".into())
        })?;
        if inventory.billboard.is_none() {
            return resource_mismatch("id 48 is present without id 47");
        }
        validate_sparse_shape(
            48,
            delta.affinity_factor,
            delta.affinity_dims,
            &delta.affinity_offsets,
            affinity_dims,
        )?;
        if delta.animation_descriptor_indices != source.animation_descriptor_indices
            || delta.affinity_offsets != source.affinity_offsets
            || delta.affinity_lights != source.affinity_lights
        {
            return resource_mismatch("id 48 descriptor/CSR layout disagrees with id 45");
        }
    }
    Ok(())
}

fn validate_delta_storage(
    base: &OctahedralShVolumeSection,
    section_id: u32,
    tile_dimension: u32,
    tile_border: u32,
    cell_levels: &[u8],
    affinity_offsets: &[u32],
) -> Result<(), ClusterDirectoryError> {
    if tile_dimension != base.tile_dimension || tile_border != base.tile_border {
        return resource_mismatch(format!(
            "id {section_id} tile geometry disagrees with id 34"
        ));
    }
    crate::sh_volume::validate_storage_levels_against_delta(
        base.grid_dimensions,
        &base.probes,
        cell_levels,
        affinity_offsets,
    )
    .map_err(|error| {
        ClusterDirectoryError::ResourceMismatch(format!(
            "id {section_id} storage metadata disagrees with id 34: {error}"
        ))
    })
}

fn validate_sparse_shape(
    section_id: u32,
    affinity_factor: u8,
    dimensions: [u32; 3],
    offsets: &[u32],
    expected: [u32; 3],
) -> Result<(), ClusterDirectoryError> {
    if affinity_factor != 4 || dimensions != expected {
        return resource_mismatch(format!(
            "id {section_id} affinity factor/dimensions {affinity_factor}/{dimensions:?} disagree with 4/{expected:?}"
        ));
    }
    let cell_count = checked_product(dimensions)? as usize;
    if offsets.len() != cell_count + 1 {
        return resource_mismatch(format!(
            "id {section_id} CSR offset count {} disagrees with affinity cell count {cell_count}",
            offsets.len()
        ));
    }
    Ok(())
}

fn validate_descriptor_indices(
    section_id: u32,
    indices: &[u32],
    descriptor_count: usize,
) -> Result<(), ClusterDirectoryError> {
    if let Some(&index) = indices
        .iter()
        .find(|&&index| index != u32::MAX && index as usize >= descriptor_count)
    {
        return resource_mismatch(format!(
            "id {section_id} descriptor index {index} outside id 34 descriptor count {descriptor_count}"
        ));
    }
    Ok(())
}

fn derive_expected_ranges(
    directory: &ClusterDirectorySection,
    inputs: ClusterDirectoryValidationInputs<'_>,
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
) -> Result<Vec<ClusterRangeRecord>, ClusterDirectoryError> {
    let (result, counts, _) =
        coverage::derive_ranges_with_counts(directory, inputs, base, affinity_dims)?;
    let mut expected_start = 0usize;
    for (cluster_id, (cluster, count)) in directory.clusters.iter().zip(counts).enumerate() {
        let canonical_start = if count == 0 { 0 } else { expected_start };
        if cluster.range_start as usize != canonical_start || cluster.range_count != count {
            return resource_mismatch(format!(
                "cluster {cluster_id} range slice does not match derived coverage"
            ));
        }
        expected_start += count as usize;
    }
    Ok(result)
}

fn sparse_offsets(inventory: ClusterDirectoryShInventory<'_>) -> Vec<&[u32]> {
    let mut result = Vec::new();
    if let Some(section) = inventory.delta {
        result.push(section.affinity_offsets.as_slice());
    }
    if let Some(section) = inventory.direct_delta {
        result.push(section.affinity_offsets.as_slice());
    }
    if let Some(section) = inventory.animated_direct_delta {
        result.push(section.affinity_offsets.as_slice());
    }
    if let Some(section) = inventory.animated_billboard_delta {
        result.push(section.affinity_offsets.as_slice());
    }
    result
}

fn locate_cell(
    locator: &CellLocatorSection,
    runtime_cell_count: u32,
    point: [f32; 3],
) -> Result<u32, ClusterDirectoryError> {
    let mut child = locator.root;
    let mut remaining = locator.nodes.len() + 1;
    loop {
        if remaining == 0 {
            return invalid("cell locator traversal did not terminate");
        }
        remaining -= 1;
        match child {
            CellLocatorChild::Cell(cell) => {
                if cell >= runtime_cell_count {
                    return invalid(format!(
                        "cell locator terminal cell {cell} outside {runtime_cell_count} runtime cells"
                    ));
                }
                return Ok(cell);
            }
            CellLocatorChild::Node(index) => {
                let node = locator.nodes.get(index as usize).ok_or_else(|| {
                    ClusterDirectoryError::InvalidData(format!(
                        "cell locator node {index} is out of range"
                    ))
                })?;
                let signed = node.plane_normal[0] * point[0]
                    + node.plane_normal[1] * point[1]
                    + node.plane_normal[2] * point[2]
                    - node.plane_distance;
                child = if signed >= 0.0 { node.front } else { node.back };
            }
        }
    }
}

fn cell_to_cluster(directory: &ClusterDirectorySection) -> Vec<u32> {
    let mut result = vec![0; directory.runtime_cell_count as usize];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let begin = cluster.member_start as usize;
        let end = begin + cluster.member_count as usize;
        for &cell in &directory.members[begin..end] {
            result[cell as usize] = cluster_id as u32;
        }
    }
    result
}

fn affinity_dimensions(grid: [u32; 3]) -> Result<[u32; 3], ClusterDirectoryError> {
    if grid == [0, 0, 0] {
        return Ok(grid);
    }
    if grid.contains(&0) {
        return resource_mismatch(format!("id 34 has mixed-zero grid dimensions {grid:?}"));
    }
    let mut result = [0; 3];
    for axis in 0..3 {
        result[axis] = grid[axis]
            .checked_add(3)
            .ok_or(ClusterDirectoryError::SizeOverflow(
                "affinity dimension ceiling",
            ))?
            / 4;
    }
    Ok(result)
}

fn brick_probe_bounds(
    brick: u32,
    affinity_dims: [u32; 3],
    grid_dims: [u32; 3],
) -> Result<([u32; 3], [u32; 3]), ClusterDirectoryError> {
    let coords = brick_coords(brick, affinity_dims);
    let min = std::array::from_fn(|axis| coords[axis] * 4);
    let max = std::array::from_fn(|axis| (min[axis] + 3).min(grid_dims[axis] - 1));
    Ok((min, max))
}

fn probe_coords(index: u32, dimensions: [u32; 3]) -> [u32; 3] {
    let xy = dimensions[0] * dimensions[1];
    [
        index % dimensions[0],
        (index / dimensions[0]) % dimensions[1],
        index / xy,
    ]
}

fn brick_coords(index: u32, dimensions: [u32; 3]) -> [u32; 3] {
    probe_coords(index, dimensions)
}

fn flatten(coords: [u32; 3], dimensions: [u32; 3]) -> Result<u32, ClusterDirectoryError> {
    coords[0]
        .checked_add(
            dimensions[0]
                .checked_mul(
                    coords[1]
                        .checked_add(
                            dimensions[1]
                                .checked_mul(coords[2])
                                .ok_or(ClusterDirectoryError::SizeOverflow("grid flatten"))?,
                        )
                        .ok_or(ClusterDirectoryError::SizeOverflow("grid flatten"))?,
                )
                .ok_or(ClusterDirectoryError::SizeOverflow("grid flatten"))?,
        )
        .ok_or(ClusterDirectoryError::SizeOverflow("grid flatten"))
}

fn probe_position(coords: [u32; 3], base: &OctahedralShVolumeSection) -> [f32; 3] {
    std::array::from_fn(|axis| base.grid_origin[axis] + coords[axis] as f32 * base.cell_size[axis])
}

fn resource_domain(section_id: u32) -> Option<ClusterResourceDomain> {
    match SectionId::from_u32(section_id)? {
        SectionId::OctahedralShVolume
        | SectionId::DirectShVolume
        | SectionId::BillboardDirectScatterVolume => Some(ClusterResourceDomain::DenseProbe),
        SectionId::DeltaShVolumes
        | SectionId::DirectShDeltaVolumes
        | SectionId::AnimatedDirectShDeltaVolumes
        | SectionId::AnimatedBillboardDirectScatterDeltaVolumes => {
            Some(ClusterResourceDomain::AffinityCell)
        }
        _ => None,
    }
}

fn validate_bounds(
    cluster: usize,
    min: [f32; 3],
    max: [f32; 3],
) -> Result<(), ClusterDirectoryError> {
    for axis in 0..3 {
        if !min[axis].is_finite() || !max[axis].is_finite() || min[axis] > max[axis] {
            return invalid(format!(
                "cluster {cluster} has non-finite or inverted bounds"
            ));
        }
        if is_negative_zero(min[axis]) || is_negative_zero(max[axis]) {
            return invalid(format!(
                "cluster {cluster} bounds contain noncanonical negative zero"
            ));
        }
    }
    Ok(())
}

fn validate_slice(
    cluster: usize,
    label: &str,
    start: u32,
    count: u32,
    expected_start: u32,
    total: usize,
) -> Result<(), ClusterDirectoryError> {
    if count == 0 && start != 0 {
        return invalid(format!(
            "cluster {cluster} zero-count {label} slice must use start zero"
        ));
    }
    if count != 0 && start != expected_start {
        return invalid(format!(
            "cluster {cluster} {label} slice starts at {start}, expected {expected_start}"
        ));
    }
    let end = start
        .checked_add(count)
        .ok_or(ClusterDirectoryError::SizeOverflow("cluster slice end"))?;
    if end as usize > total {
        return invalid(format!(
            "cluster {cluster} {label} slice ends outside table"
        ));
    }
    Ok(())
}

fn checked_product(dimensions: [u32; 3]) -> Result<u32, ClusterDirectoryError> {
    dimensions[0]
        .checked_mul(dimensions[1])
        .and_then(|value| value.checked_mul(dimensions[2]))
        .ok_or(ClusterDirectoryError::SizeOverflow(
            "resource dimension product",
        ))
}

fn try_vec<T>(count: u32, label: &'static str) -> Result<Vec<T>, ClusterDirectoryError> {
    let count = usize_count(count)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| ClusterDirectoryError::AllocationFailed(label))?;
    Ok(result)
}

fn usize_count(count: u32) -> Result<usize, ClusterDirectoryError> {
    usize::try_from(count)
        .map_err(|_| ClusterDirectoryError::SizeOverflow("u32 count does not fit usize"))
}
fn u32_len(len: usize, label: &'static str) -> Result<u32, ClusterDirectoryError> {
    u32::try_from(len).map_err(|_| ClusterDirectoryError::SizeOverflow(label))
}
fn canonical_zero(value: f32) -> f32 {
    if value == 0.0 { 0.0 } else { value }
}
fn is_negative_zero(value: f32) -> bool {
    value.to_bits() == (-0.0f32).to_bits()
}
fn float_array_bits_equal(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|axis| a[axis].to_bits() == b[axis].to_bits())
}
fn invalid<T>(message: impl Into<String>) -> Result<T, ClusterDirectoryError> {
    Err(ClusterDirectoryError::InvalidData(message.into()))
}
fn canonical_mismatch<T>(message: impl Into<String>) -> Result<T, ClusterDirectoryError> {
    Err(ClusterDirectoryError::CanonicalPartitionMismatch(
        message.into(),
    ))
}
fn resource_mismatch<T>(message: impl Into<String>) -> Result<T, ClusterDirectoryError> {
    Err(ClusterDirectoryError::ResourceMismatch(message.into()))
}
