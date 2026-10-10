// Equivalence and scale coverage for the id-49 affinity ownership sweep.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::*;

/// The original per-interval scan, kept verbatim as the oracle. Quadratic in
/// the range count; only for small generated directories.
fn reference_affinity_ownership(
    directory: &ClusterDirectorySection,
) -> Result<(), ClusterDirectoryError> {
    for (resource_index, resource) in directory.resources.iter().enumerate() {
        if resource.domain != ClusterResourceDomain::AffinityCell {
            continue;
        }
        let mut boundaries = BTreeSet::new();
        for cluster in &directory.clusters {
            let begin = cluster.range_start as usize;
            let end = begin + cluster.range_count as usize;
            for range in directory.ranges[begin..end]
                .iter()
                .filter(|range| range.resource_index as usize == resource_index)
            {
                boundaries.insert(range.start);
                boundaries.insert(range.start + range.count);
            }
        }
        let boundaries: Vec<u32> = boundaries.into_iter().collect();
        for pair in boundaries.windows(2) {
            let start = pair[0];
            if start == pair[1] {
                continue;
            }
            let mut references = Vec::new();
            for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
                let begin = cluster.range_start as usize;
                let end = begin + cluster.range_count as usize;
                if let Some(range) = directory.ranges[begin..end].iter().find(|range| {
                    range.resource_index as usize == resource_index
                        && range.start <= start
                        && start < range.start + range.count
                }) {
                    references.push((cluster_id as u32, range));
                }
            }
            if references.is_empty() {
                continue;
            }
            let owner = references[0].1.owner_cluster_id;
            if references
                .iter()
                .any(|(_, range)| range.owner_cluster_id != owner)
            {
                return invalid(format!(
                    "resource {resource_index} affinity cell {start} has disagreeing owners"
                ));
            }
            let owner_references = references
                .iter()
                .filter(|(cluster, range)| {
                    *cluster == owner && range.role == ClusterRangeRole::Owned
                })
                .count();
            if owner_references != 1 {
                return invalid(format!(
                    "resource {resource_index} affinity cell {start} does not have exactly one covering owner"
                ));
            }
        }
    }
    Ok(())
}

fn reference_validate_structure(
    directory: &ClusterDirectorySection,
) -> Result<(), ClusterDirectoryError> {
    directory.validate_tables()?;
    reference_affinity_ownership(directory)
}

/// Candidate resources in ascending section order: (section id, affinity?).
const RESOURCE_CANDIDATES: [(u32, bool); 4] = [(27, true), (34, false), (41, true), (45, true)];

/// One single-cell cluster per cluster range list, with contiguous range slices.
fn directory_from_cluster_ranges(
    resources: Vec<ClusterResourceRecord>,
    per_cluster: Vec<Vec<ClusterRangeRecord>>,
) -> ClusterDirectorySection {
    let cluster_count = per_cluster.len() as u32;
    let mut clusters = Vec::new();
    let mut ranges = Vec::new();
    for (cluster_id, cluster_ranges) in per_cluster.into_iter().enumerate() {
        let range_count = cluster_ranges.len() as u32;
        clusters.push(ClusterRecord {
            bounds_min: [cluster_id as f32, 0.0, 0.0],
            bounds_max: [cluster_id as f32 + 1.0, 1.0, 1.0],
            member_start: cluster_id as u32,
            member_count: 1,
            range_start: if range_count == 0 {
                0
            } else {
                ranges.len() as u32
            },
            range_count,
            primitive_count: 0,
            flags: 0,
        });
        ranges.extend(cluster_ranges);
    }
    ClusterDirectorySection {
        runtime_cell_count: cluster_count,
        primitive_limit: 4,
        cell_limit: 1,
        clusters,
        resources,
        members: (0..cluster_count).collect(),
        ranges,
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

/// Coalesce one cluster's per-cell (role, owner) labels into maximal ranges.
fn coalesce_cells(
    resource_index: u32,
    cells: &[Option<(ClusterRangeRole, u32)>],
    out: &mut Vec<ClusterRangeRecord>,
) {
    let mut cell = 0usize;
    while cell < cells.len() {
        let Some(label) = cells[cell] else {
            cell += 1;
            continue;
        };
        let start = cell;
        while cell < cells.len() && cells[cell] == Some(label) {
            cell += 1;
        }
        out.push(ClusterRangeRecord {
            resource_index,
            start: start as u32,
            count: (cell - start) as u32,
            owner_cluster_id: label.1,
            role: label.0,
        });
    }
}

#[derive(Debug, Clone)]
struct RawRange {
    resource_choice: usize,
    gap: u32,
    len: u32,
    owner_raw: u32,
    role_raw: u8,
}

#[derive(Debug, Clone)]
enum Chaos {
    None,
    SwapFirstTwo,
    DuplicateLast,
}

/// Arbitrary per-cluster ranges: independent owners and roles, unaligned
/// boundaries across clusters, occasional out-of-range owners, role
/// mismatches, and unsorted or overlapping slices from the chaos knob. Gaps
/// are always >= 1, so one cluster's own same-resource ranges never touch
/// and never need coalescing.
///
/// The corruption knobs (out-of-range owners, role mismatches, chaos) are
/// kept low per range, but with coalescing off the table they are now the
/// main `validate_tables` rejection source, alongside occasional extent
/// overflow when several ranges land on the same resource in one cluster.
/// Cross-cluster disagreement and coverage gaps — the sweep's own error
/// classes — come for free from independent random owners on overlapping
/// ranges and do not need these knobs turned up.
fn raw_directory() -> impl Strategy<Value = ClusterDirectorySection> {
    let raw_range = (0usize..4, 1u32..4, 1u32..7, 0u32..128, 0u8..100).prop_map(
        |(resource_choice, gap, len, owner_raw, role_raw)| RawRange {
            resource_choice,
            gap,
            len,
            owner_raw,
            role_raw,
        },
    );
    let chaos = prop_oneof![
        97 => Just(Chaos::None),
        1 => Just(Chaos::SwapFirstTwo),
        1 => Just(Chaos::DuplicateLast),
    ];
    (
        1usize..7,
        1u8..16,
        prop::collection::vec(24u32..64, 4),
        prop::collection::vec((prop::collection::vec(raw_range, 0..8), chaos), 7),
    )
        .prop_map(|(cluster_count, resource_mask, lengths, clusters)| {
            let chosen: Vec<usize> = (0..4)
                .filter(|bit| resource_mask & (1 << bit) != 0)
                .collect();
            let resources: Vec<ClusterResourceRecord> = chosen
                .iter()
                .map(|&candidate| {
                    let (section_id, affinity) = RESOURCE_CANDIDATES[candidate];
                    ClusterResourceRecord {
                        section_id,
                        domain: if affinity {
                            ClusterResourceDomain::AffinityCell
                        } else {
                            ClusterResourceDomain::DenseProbe
                        },
                        dimensions: [lengths[candidate], 1, 1],
                    }
                })
                .collect();
            let per_cluster = clusters
                .into_iter()
                .take(cluster_count)
                .enumerate()
                .map(|(cluster_id, (mut raw, chaos))| {
                    for range in &mut raw {
                        range.resource_choice %= chosen.len();
                    }
                    raw.sort_by_key(|range| range.resource_choice);
                    let mut out = Vec::new();
                    let mut cursor: Option<(usize, u32)> = None;
                    for range in raw {
                        let resource_index = range.resource_choice;
                        let base = match cursor {
                            Some((index, end)) if index == resource_index => end,
                            _ => 0,
                        };
                        let start = base + range.gap;
                        let affinity =
                            resources[resource_index].domain == ClusterResourceDomain::AffinityCell;
                        let (owner, role) = if affinity {
                            let owner = if range.owner_raw < 126 {
                                range.owner_raw % cluster_count as u32
                            } else {
                                cluster_count as u32
                            };
                            let consistent = if owner == cluster_id as u32 {
                                ClusterRangeRole::Owned
                            } else {
                                ClusterRangeRole::Halo
                            };
                            let role = match range.role_raw {
                                98 if consistent == ClusterRangeRole::Owned => {
                                    ClusterRangeRole::Halo
                                }
                                98 => ClusterRangeRole::Owned,
                                99 => ClusterRangeRole::Dense,
                                _ => consistent,
                            };
                            (owner, role)
                        } else if range.role_raw == 99 {
                            (0, ClusterRangeRole::Owned)
                        } else {
                            (DENSE_OWNER_SENTINEL, ClusterRangeRole::Dense)
                        };
                        out.push(ClusterRangeRecord {
                            resource_index: resource_index as u32,
                            start,
                            count: range.len,
                            owner_cluster_id: owner,
                            role,
                        });
                        cursor = Some((resource_index, start + range.len));
                    }
                    match chaos {
                        Chaos::SwapFirstTwo if out.len() >= 2 => out.swap(0, 1),
                        Chaos::DuplicateLast if !out.is_empty() => {
                            out.push(out[out.len() - 1].clone())
                        }
                        _ => {}
                    }
                    out
                })
                .collect();
            directory_from_cluster_ranges(resources, per_cluster)
        })
}

#[derive(Debug, Clone)]
enum Perturbation {
    None,
    /// Remove one range of one cluster (dropped owner or harmless halo loss).
    DropRange {
        cluster: usize,
        range: usize,
    },
    /// Point one range at a different owner (disagreement or role mismatch).
    Reassign {
        cluster: usize,
        range: usize,
        owner: usize,
    },
    /// Split one range in two (uncoalesced neighbours) when it is long enough.
    Split {
        cluster: usize,
        range: usize,
    },
}

/// Directories built from a per-cell owner assignment, so most are valid:
/// every owner covers its cells as `Owned`, other clusters add `Halo` views.
/// One optional perturbation then breaks a single local invariant.
fn consistent_directory() -> impl Strategy<Value = ClusterDirectorySection> {
    let perturbation = prop_oneof![
        4 => Just(Perturbation::None),
        1 => (0usize..6, 0usize..64).prop_map(|(cluster, range)| Perturbation::DropRange { cluster, range }),
        1 => (0usize..6, 0usize..64, 0usize..6)
            .prop_map(|(cluster, range, owner)| Perturbation::Reassign { cluster, range, owner }),
        1 => (0usize..6, 0usize..64).prop_map(|(cluster, range)| Perturbation::Split { cluster, range }),
    ];
    (
        1usize..7,
        1u8..16,
        prop::collection::vec(1u32..40, 4),
        // Per candidate resource and cell: owner choice (>= cluster count means
        // uncovered) and a halo-visibility bitmask over clusters.
        prop::collection::vec(prop::collection::vec((0usize..9, 0u8..64), 40), 4),
        perturbation,
    )
        .prop_map(
            |(cluster_count, resource_mask, lengths, cells, perturbation)| {
                let chosen: Vec<usize> = (0..4)
                    .filter(|bit| resource_mask & (1 << bit) != 0)
                    .collect();
                let mut resources = Vec::new();
                let mut per_cluster = vec![Vec::new(); cluster_count];
                for (resource_index, &candidate) in chosen.iter().enumerate() {
                    let (section_id, affinity) = RESOURCE_CANDIDATES[candidate];
                    let len = lengths[candidate] as usize;
                    resources.push(ClusterResourceRecord {
                        section_id,
                        domain: if affinity {
                            ClusterResourceDomain::AffinityCell
                        } else {
                            ClusterResourceDomain::DenseProbe
                        },
                        dimensions: [len as u32, 1, 1],
                    });
                    for (cluster_id, cluster_ranges) in per_cluster.iter_mut().enumerate() {
                        let labels: Vec<Option<(ClusterRangeRole, u32)>> = cells[candidate][..len]
                            .iter()
                            .map(|&(owner, halo_mask)| {
                                if owner >= cluster_count {
                                    return None;
                                }
                                if !affinity {
                                    return (owner == cluster_id).then_some((
                                        ClusterRangeRole::Dense,
                                        DENSE_OWNER_SENTINEL,
                                    ));
                                }
                                if owner == cluster_id {
                                    Some((ClusterRangeRole::Owned, owner as u32))
                                } else if halo_mask & (1 << cluster_id) != 0 {
                                    Some((ClusterRangeRole::Halo, owner as u32))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        coalesce_cells(resource_index as u32, &labels, cluster_ranges);
                    }
                }
                match perturbation {
                    Perturbation::None => {}
                    Perturbation::DropRange { cluster, range } => {
                        let ranges = &mut per_cluster[cluster % cluster_count];
                        if !ranges.is_empty() {
                            let index = range % ranges.len();
                            ranges.remove(index);
                        }
                    }
                    Perturbation::Reassign {
                        cluster,
                        range,
                        owner,
                    } => {
                        let ranges = &mut per_cluster[cluster % cluster_count];
                        if !ranges.is_empty() {
                            let index = range % ranges.len();
                            if ranges[index].role != ClusterRangeRole::Dense {
                                let cluster_id = (cluster % cluster_count) as u32;
                                let owner = (owner % cluster_count) as u32;
                                ranges[index].owner_cluster_id = owner;
                                ranges[index].role = if owner == cluster_id {
                                    ClusterRangeRole::Owned
                                } else {
                                    ClusterRangeRole::Halo
                                };
                            }
                        }
                    }
                    Perturbation::Split { cluster, range } => {
                        let ranges = &mut per_cluster[cluster % cluster_count];
                        if !ranges.is_empty() {
                            let index = range % ranges.len();
                            if ranges[index].count >= 2 {
                                let mut tail = ranges[index].clone();
                                let head_count = ranges[index].count / 2;
                                ranges[index].count = head_count;
                                tail.start += head_count;
                                tail.count -= head_count;
                                ranges.insert(index + 1, tail);
                            }
                        }
                    }
                }
                directory_from_cluster_ranges(resources, per_cluster)
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn affinity_ownership_sweep_matches_reference_on_arbitrary_ranges(
        directory in raw_directory(),
    ) {
        prop_assert_eq!(directory.validate_structure(), reference_validate_structure(&directory));
    }

    #[test]
    fn affinity_ownership_sweep_matches_reference_on_owner_assigned_ranges(
        directory in consistent_directory(),
    ) {
        prop_assert_eq!(directory.validate_structure(), reference_validate_structure(&directory));
    }
}

/// Per-generator tallies of which verdict class a directory reached:
/// rejected by `validate_tables` before the sweep even runs, or reaching the
/// sweep and being accepted, or rejected for one of the sweep's two error
/// classes.
#[derive(Default)]
struct Verdicts {
    accepted: u32,
    disagreeing: u32,
    uncovered: u32,
    tables: u32,
}

impl Verdicts {
    fn total(&self) -> u32 {
        self.accepted + self.disagreeing + self.uncovered + self.tables
    }

    /// Directories that got past `validate_tables` and exercised the sweep,
    /// regardless of the sweep's own verdict.
    fn reached_sweep(&self) -> u32 {
        self.accepted + self.disagreeing + self.uncovered
    }

    fn record(&mut self, directory: &ClusterDirectorySection) {
        if directory.validate_tables().is_err() {
            self.tables += 1;
            return;
        }
        match reference_affinity_ownership(directory) {
            Ok(()) => self.accepted += 1,
            Err(ClusterDirectoryError::InvalidData(message))
                if message.ends_with("has disagreeing owners") =>
            {
                self.disagreeing += 1
            }
            Err(ClusterDirectoryError::InvalidData(message))
                if message.ends_with("does not have exactly one covering owner") =>
            {
                self.uncovered += 1
            }
            Err(other) => panic!("unexpected ownership error {other:?}"),
        }
    }
}

/// Guards the equivalence properties against a vacuous generator. Verdicts
/// are tracked per generator (not pooled) because `consistent_directory`
/// alone can satisfy a pooled tally: it is built from a valid per-cell owner
/// assignment plus a single local perturbation, so nearly every case clears
/// `validate_tables` and reaches the sweep. `raw_directory` is the only
/// source of unaligned, multi-interval overlap shapes, so it must reach the
/// sweep often enough, on its own, to actually exercise the disagreement and
/// coverage checks — pooling would let `raw_directory` fail `validate_tables`
/// almost every time without this test noticing.
///
/// Each generator draws from its own deterministic runner. Sharing one
/// runner would interleave the two draw sequences, so an unrelated change to
/// either strategy shifts what the other one draws.
#[test]
fn affinity_ownership_generators_reach_every_verdict_class() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut raw_runner = TestRunner::deterministic();
    let mut consistent_runner = TestRunner::deterministic();
    let raw = raw_directory();
    let consistent = consistent_directory();
    let mut raw_verdicts = Verdicts::default();
    let mut consistent_verdicts = Verdicts::default();
    for _ in 0..1024 {
        raw_verdicts.record(&raw.new_tree(&mut raw_runner).unwrap().current());
        consistent_verdicts.record(
            &consistent
                .new_tree(&mut consistent_runner)
                .unwrap()
                .current(),
        );
    }

    let pooled_accepted = raw_verdicts.accepted + consistent_verdicts.accepted;
    let pooled_disagreeing = raw_verdicts.disagreeing + consistent_verdicts.disagreeing;
    let pooled_uncovered = raw_verdicts.uncovered + consistent_verdicts.uncovered;
    let pooled_tables = raw_verdicts.tables + consistent_verdicts.tables;
    assert!(
        pooled_accepted > 0 && pooled_disagreeing > 0 && pooled_uncovered > 0 && pooled_tables > 0,
        "accepted {pooled_accepted}, disagreeing {pooled_disagreeing}, uncovered {pooled_uncovered}, tables {pooled_tables}"
    );

    // `raw_directory` must reach the sweep on a meaningful share of its own
    // cases, not merely as a rounding error under `consistent_directory`.
    let raw_total = raw_verdicts.total();
    let raw_reached_sweep = raw_verdicts.reached_sweep();
    assert!(
        raw_reached_sweep.saturating_mul(4) >= raw_total,
        "raw_directory reached the sweep {raw_reached_sweep}/{raw_total} times, want at least 25%"
    );
    // And, alone, it must reach both of the sweep's ownership error classes.
    assert!(
        raw_verdicts.disagreeing > 0 && raw_verdicts.uncovered > 0,
        "raw_directory alone did not reach every ownership error class: disagreeing {}, uncovered {}",
        raw_verdicts.disagreeing,
        raw_verdicts.uncovered
    );
}

/// Owner-assigned directory at a scale where the per-interval scan would take
/// minutes (tens of thousands of boundaries times thousands of clusters). It
/// asserts only verdicts; a quadratic regression shows up as a stalled test,
/// never as a flaky timing threshold.
#[test]
fn affinity_ownership_sweep_validates_large_directory() {
    const CLUSTERS: u32 = 8192;
    const OWNED_PER_CLUSTER: u32 = 8;
    const HALO_REACH: u32 = 3;
    let cells = CLUSTERS * OWNED_PER_CLUSTER;
    let resources: Vec<ClusterResourceRecord> = [27u32, 41, 45]
        .into_iter()
        .map(|section_id| ClusterResourceRecord {
            section_id,
            domain: ClusterResourceDomain::AffinityCell,
            dimensions: [cells, 1, 1],
        })
        .collect();
    let per_cluster: Vec<Vec<ClusterRangeRecord>> = (0..CLUSTERS)
        .map(|cluster| {
            let mut ranges = Vec::new();
            for resource_index in 0..resources.len() as u32 {
                // Halo the tail of the previous owner's run, own a run, then
                // halo the head of the next owner's run.
                if cluster > 0 {
                    ranges.push(ClusterRangeRecord {
                        resource_index,
                        start: cluster * OWNED_PER_CLUSTER - HALO_REACH,
                        count: HALO_REACH,
                        owner_cluster_id: cluster - 1,
                        role: ClusterRangeRole::Halo,
                    });
                }
                ranges.push(ClusterRangeRecord {
                    resource_index,
                    start: cluster * OWNED_PER_CLUSTER,
                    count: OWNED_PER_CLUSTER,
                    owner_cluster_id: cluster,
                    role: ClusterRangeRole::Owned,
                });
                if cluster + 1 < CLUSTERS {
                    ranges.push(ClusterRangeRecord {
                        resource_index,
                        start: (cluster + 1) * OWNED_PER_CLUSTER,
                        count: HALO_REACH,
                        owner_cluster_id: cluster + 1,
                        role: ClusterRangeRole::Halo,
                    });
                }
            }
            ranges
        })
        .collect();
    let mut directory = directory_from_cluster_ranges(resources, per_cluster);
    directory.validate_structure().unwrap();

    // Retarget one halo on the last resource: the cell it views now has two
    // owners, and the error names that cell.
    let cluster = CLUSTERS / 2;
    let record = &directory.clusters[cluster as usize];
    let last_resource_halo = record.range_start as usize + record.range_count as usize - 1;
    directory.ranges[last_resource_halo].owner_cluster_id = cluster + 2;
    assert_eq!(
        directory.validate_structure(),
        Err(ClusterDirectoryError::InvalidData(format!(
            "resource 2 affinity cell {} has disagreeing owners",
            (cluster + 1) * OWNED_PER_CLUSTER
        )))
    );
}
