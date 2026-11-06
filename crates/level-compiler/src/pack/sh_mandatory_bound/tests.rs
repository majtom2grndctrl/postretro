//! Hand-computed bound fixtures and the Build Summary text.

use std::time::Duration;

use log::Level;
use postretro_level_format::SectionId;
use postretro_level_format::cell_residency_set::{
    CellResidencySetSection, DEFAULT_LEAD, ResidencyEntry,
};
use postretro_level_format::cells::{
    CELL_FLAG_DRAWABLE, CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord, CellsSection,
};
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection, ClusterHintRecord, ClusterRecord,
    ClusterResourceDomain, ClusterResourceRecord, ShMandatoryBoundEntry, ShMandatoryBoundFamily,
    WHOLE_MAP_WORST_CELL,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::sh_pool_cost::ShDenseSlotTextures;
use postretro_test_log_capture::LogCapture;

use super::*;

const FAR: u32 = DEFAULT_LEAD * 2;
/// Resource row of the one sparse family in [`two_room_directory`].
const FAMILY: u32 = 1;

fn cells(flags: &[u32]) -> CellsSection {
    CellsSection {
        cells: flags
            .iter()
            .map(|&flags| CellRecord {
                bounds_min: [0.0; 3],
                bounds_max: [2.0, 4.0, 6.0],
                flags,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            })
            .collect(),
        portal_refs: Vec::new(),
    }
}

fn open_cells(count: usize) -> CellsSection {
    cells(&vec![CELL_FLAG_DRAWABLE; count])
}

/// One cluster per cell, with a dense id-34 row and one sparse family.
fn directory(cluster_count: u32, pinned: &[u32]) -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: cluster_count,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: (0..cluster_count)
            .map(|cluster| ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: cluster,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            })
            .collect(),
        resources: vec![
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            },
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
        ],
        members: (0..cluster_count).collect(),
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: pinned
            .iter()
            .map(|&cluster_id| ClusterHintRecord {
                cluster_id,
                flags: CLUSTER_HINT_FLAG_PINNED,
                priority: 0,
            })
            .collect(),
        mandatory_bound: Vec::new(),
    }
}

fn lists(clusters: &[&[u32]]) -> ClusterLists {
    let mut lists = ClusterLists::default();
    for cluster in clusters {
        lists.push(cluster.iter().copied());
    }
    lists
}

/// Four clusters over nodes of 10, 20, 40 and 80 slots and rows of (1, 3),
/// (2, 5) and (4, 7):
///
/// | cluster | nodes | rows |
/// |---|---|---|
/// | 0 | 0, 1 | 0 |
/// | 1 | 1, 2 | 0, 1 |
/// | 2 | 3 | |
/// | 3 | 0 | 2 |
fn four_cluster_demand() -> ClusterStorageDemand {
    ClusterStorageDemand {
        cluster_nodes: lists(&[&[0, 1], &[1, 2], &[3], &[0]]),
        node_slots: vec![10, 20, 40, 80],
        families: vec![SparseFamilyDemand {
            resource_index: FAMILY,
            cluster_rows: lists(&[&[0], &[0, 1], &[], &[2]]),
            row_units: vec![(1, 3), (2, 5), (4, 7)],
        }],
    }
}

fn residency(max_lead: u32, camera_entries: &[&[(u32, u32)]]) -> CellResidencySetSection {
    let mut offsets = vec![0];
    let mut entries = Vec::new();
    for camera in camera_entries {
        entries.extend(
            camera
                .iter()
                .map(|&(cell_id, lead)| ResidencyEntry { lead, cell_id }),
        );
        offsets.push(entries.len() as u32);
    }
    CellResidencySetSection {
        max_lead,
        offsets,
        entries,
    }
}

fn family(worst_entries: u32, worst_tile_f16: u32) -> Vec<ShMandatoryBoundFamily> {
    vec![ShMandatoryBoundFamily {
        resource_index: FAMILY,
        worst_entries,
        worst_tile_f16,
    }]
}

#[test]
fn bound_matches_hand_computed_worst_cell_at_each_lead() {
    // Cluster 3 is pinned: node 0 (10 slots) and row 2 (4, 7) in every set.
    let directory = directory(4, &[3]);
    let residency = residency(
        FAR,
        &[
            // Default lead: clusters 0, 1 and the pin, so nodes 0, 1, 2 with
            // node 1 shared by two clusters and node 0 by the pin:
            // 10 + 20 + 40 = 70, not the 100 a per-cluster sum gives. Rows
            // 0, 1, 2: (1 + 2 + 4, 3 + 5 + 7), row 0 shared.
            // Maximum: adds cluster 2's node 3: 150.
            &[(0, 0), (1, DEFAULT_LEAD), (2, FAR)],
            // Default lead: cluster 2 and the pin: 80 + 10 = 90, the dense
            // worst, and rows (4, 7) only. Maximum: unchanged.
            &[(2, 0)],
            // Pins alone.
            &[],
            // Only its own cluster, which is the pin.
            &[(3, 0)],
        ],
    );

    let bound = sh_mandatory_bound(
        &directory,
        &open_cells(4),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(
        bound,
        [
            ShMandatoryBoundEntry {
                lead: DEFAULT_LEAD,
                worst_dense_slots: 90,
                worst_cell: 1,
                // The sparse worst sits in cell 0, not the dense worst cell 1.
                families: family(7, 15),
            },
            ShMandatoryBoundEntry {
                lead: FAR,
                worst_dense_slots: 150,
                worst_cell: 0,
                families: family(7, 15),
            },
        ]
    );
}

// A pin dropped from one lead's walk would shift only that entry.
#[test]
fn pins_count_at_every_stored_lead_and_only_once_when_also_reached() {
    let directory = directory(4, &[2]);
    let residency = residency(FAR, &[&[(2, 0)], &[], &[], &[]]);

    let bound = sh_mandatory_bound(
        &directory,
        &open_cells(4),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(bound.len(), 2);
    assert!(bound.iter().all(|entry| entry.worst_dense_slots == 80));
}

#[test]
fn a_maximum_within_the_default_lead_stores_one_entry() {
    let residency = residency(DEFAULT_LEAD / 2, &[&[(0, 0)], &[], &[], &[]]);

    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &open_cells(4),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(
        bound,
        [ShMandatoryBoundEntry {
            lead: DEFAULT_LEAD / 2,
            worst_dense_slots: 30,
            worst_cell: 0,
            families: family(1, 3),
        }]
    );
}

#[test]
fn a_maximum_equal_to_the_default_lead_stores_one_entry() {
    let residency = residency(DEFAULT_LEAD, &[&[], &[], &[], &[]]);

    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &open_cells(4),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].lead, DEFAULT_LEAD);
}

#[test]
fn solid_and_exterior_cells_never_set_the_bound() {
    let cells = cells(&[CELL_FLAG_SOLID, CELL_FLAG_EXTERIOR, CELL_FLAG_DRAWABLE, 0]);
    let residency = residency(
        DEFAULT_LEAD,
        &[
            &[(0, 0), (1, 0), (2, 0)],
            &[(0, 0), (1, 0), (2, 0)],
            &[(3, 0)],
            &[],
        ],
    );

    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &cells,
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(bound[0].worst_cell, 2);
    assert_eq!(bound[0].worst_dense_slots, 10);
}

#[test]
fn wholly_mandatory_map_reports_the_whole_map() {
    let all: &[(u32, u32)] = &[(0, 0), (1, 0), (2, 0), (3, 0)];
    let residency = residency(FAR, &[all, all, all, all]);

    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &open_cells(4),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    // The unique union: 10 + 20 + 40 + 80, and every row once.
    for entry in &bound {
        assert_eq!(entry.worst_dense_slots, 150);
        assert_eq!(entry.families, family(7, 15));
        assert_eq!(entry.worst_cell, 0);
    }
    assert_eq!(bound.len(), 2);
}

#[test]
fn no_id51_reports_the_whole_map() {
    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &open_cells(4),
        None,
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(
        bound,
        [ShMandatoryBoundEntry {
            lead: 0,
            // A sum of per-cluster figures would read 180 and (8, 18).
            worst_dense_slots: 150,
            worst_cell: WHOLE_MAP_WORST_CELL,
            families: family(7, 15),
        }]
    );
}

#[test]
fn an_id51_with_no_camera_cells_reports_the_whole_map() {
    let residency = residency(FAR, &[&[], &[], &[], &[]]);
    let bound = sh_mandatory_bound(
        &directory(4, &[]),
        &cells(&[CELL_FLAG_SOLID; 4]),
        Some(&residency),
        &four_cluster_demand(),
    )
    .unwrap();

    assert_eq!(bound[0].worst_cell, WHOLE_MAP_WORST_CELL);
}

#[test]
fn no_id34_stores_no_bound() {
    let mut directory = directory(4, &[]);
    directory.resources.remove(0);

    let bound =
        sh_mandatory_bound(&directory, &open_cells(4), None, &four_cluster_demand()).unwrap();

    assert!(bound.is_empty());
}

fn indirect_only() -> ShDenseSlotTextures {
    ShDenseSlotTextures {
        indirect_format: IRRADIANCE_FORMAT_RGBA16F,
        direct_format: None,
        direct_compose: false,
        animated_direct_compose: false,
    }
}

/// `slots` at the 1,024 B of [`indirect_only`], with a sparse family priced
/// beside them: 2 entries at 8 B and 3 f16 in two 4 B words.
fn report_for(slots: u32, families: Vec<ShMandatoryBoundFamily>) -> ShMandatoryReport {
    let entries = [
        ShMandatoryBoundEntry {
            lead: DEFAULT_LEAD,
            worst_dense_slots: slots,
            worst_cell: 1,
            families,
        },
        ShMandatoryBoundEntry {
            lead: FAR,
            worst_dense_slots: slots + 1024,
            worst_cell: 0,
            families: Vec::new(),
        },
    ];
    ShMandatoryReport::price(
        &entries,
        indirect_only(),
        &open_cells(2),
        Duration::from_millis(3),
    )
    .unwrap()
}

const THRESHOLD_SLOTS: u32 = 512 * 1024;

fn logged_warnings(report: &ShMandatoryReport) -> Vec<String> {
    let capture = LogCapture::start();
    report.log();
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .map(|record| record.message)
        .collect()
}

#[test]
fn build_summary_reports_bound_and_warns_above_threshold() {
    let at_threshold = report_for(THRESHOLD_SLOTS, Vec::new());
    assert_eq!(
        at_threshold.lines(),
        [
            "SH mandatory memory: 512.0 MiB pool demand at 16 m lead (how far ahead of the \
             camera probes load), 524288 probe slots; dense demand peaks in cell 1, near \
             (1.0, 2.0, 3.0) engine metres",
            "  at the maximum 32 m lead: 513.0 MiB pool demand, 525312 probe slots; dense \
             demand peaks in cell 0, near (1.0, 2.0, 3.0) engine metres",
        ]
    );
    assert!(
        logged_warnings(&at_threshold).is_empty(),
        "exactly at the threshold is not above it"
    );

    // One byte over would do; a sparse entry is the smallest step: 8 B.
    let above = report_for(THRESHOLD_SLOTS, family(1, 0));
    let lines = above.lines();
    assert_eq!(lines.len(), 3);
    let warning = lines[2].strip_prefix("  warning: ").expect("warning line");
    assert!(warning.contains("above the 512.0 MiB budget"), "{warning}");
    assert!(
        warning.contains("near (1.0, 2.0, 3.0) engine metres"),
        "{warning}"
    );
    assert_eq!(logged_warnings(&above), [format!("[Compiler] {warning}")]);
}

#[test]
fn the_warning_reads_the_lead_the_pool_is_sized_from() {
    // The wider lead is over budget, the sized lead is not.
    let report = report_for(THRESHOLD_SLOTS - 1, Vec::new());

    assert!(report.lines().iter().all(|line| !line.contains("warning")));
    assert!(logged_warnings(&report).is_empty());
}

#[test]
fn sparse_families_are_priced_in_pool_units() {
    let report = report_for(0, family(2, 3));

    // 2 entries × 8 B + 2 words × 4 B, against 1,024 B per slot at the wider
    // lead.
    assert_eq!(report.leads[0].bytes, 24);
    assert_eq!(report.leads[1].bytes, 1024 * 1024);
}

#[test]
fn a_whole_map_bound_says_why_and_names_no_cell() {
    let entries = [ShMandatoryBoundEntry {
        lead: 0,
        worst_dense_slots: THRESHOLD_SLOTS + 1,
        worst_cell: WHOLE_MAP_WORST_CELL,
        families: Vec::new(),
    }];
    let report =
        ShMandatoryReport::price(&entries, indirect_only(), &open_cells(1), Duration::ZERO)
            .unwrap();

    let lines = report.lines();
    assert_eq!(
        lines[0],
        "SH mandatory memory: 512.0 MiB pool demand, 524289 probe slots, the whole map: \
         without usable portals every probe stays loaded"
    );
    assert!(lines[1].contains("usable portals"), "{}", lines[1]);
}

#[test]
fn a_level_without_a_bound_says_so() {
    assert_eq!(
        ShMandatoryReport::default().lines(),
        ["SH mandatory memory: none (this level streams no SH probes)"]
    );
}

#[test]
fn an_atlas_format_the_pool_cannot_allocate_fails_by_name() {
    let textures = ShDenseSlotTextures {
        indirect_format: u32::MAX,
        ..indirect_only()
    };
    let error = ShMandatoryReport::price(&[], textures, &open_cells(1), Duration::ZERO)
        .unwrap_err()
        .to_string();

    assert!(error.contains("atlas formats"), "{error}");
}
