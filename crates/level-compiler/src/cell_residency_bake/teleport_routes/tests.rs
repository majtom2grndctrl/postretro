use super::*;
use crate::cell_residency_bake::lead_map::{LeadSources, build_lead_map};
use crate::cell_residency_bake::{cell_residency_set_bake_cached, cell_residency_set_cache_key};
use crate::partition::BspLeaf;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cell_visibility::CoupledPairRecord;
use postretro_level_format::cells::{CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord};
use postretro_level_format::environment_volumes::EnvironmentPlane;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::teleporters::TeleporterRecord;

fn box_brush(min: [f32; 3], max: [f32; 3]) -> EnvironmentBrush {
    EnvironmentBrush {
        aabb_min: min,
        aabb_max: max,
        planes: (0..3)
            .flat_map(|axis| {
                [-1.0, 1.0].map(move |sign| {
                    let mut normal = [0.0; 3];
                    normal[axis] = sign;
                    EnvironmentPlane {
                        normal,
                        distance: if sign > 0.0 { max[axis] } else { -min[axis] },
                    }
                })
            })
            .collect(),
    }
}
fn record(brushes: Vec<EnvironmentBrush>) -> TeleporterRecord {
    TeleporterRecord {
        source_entity: 1,
        mode: TeleportMode::Seamless,
        enabled_on_spawn: true,
        brushes,
        destination_origin: [0.0; 3],
        destination_yaw: 0.0,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    }
}
fn cells_tree(bounds: &[([f32; 3], [f32; 3])]) -> (CellsSection, BspTree) {
    let cells = CellsSection {
        cells: bounds
            .iter()
            .map(|&(min, max)| CellRecord {
                bounds_min: min,
                bounds_max: max,
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            })
            .collect(),
        portal_refs: Vec::new(),
    };
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: bounds
            .iter()
            .map(|&(min, max)| BspLeaf {
                face_indices: Vec::new(),
                bounds: Aabb {
                    min: DVec3::from(min.map(f64::from)),
                    max: DVec3::from(max.map(f64::from)),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            })
            .collect(),
    };
    (cells, tree)
}
fn world(cells: &CellsSection, locator: &CellLocatorSection) -> LevelWorld {
    LevelWorld::visibility_only_from_sections(
        cells,
        &PortalsSection {
            portals: Vec::new(),
            vertices: Vec::new(),
        },
        locator,
    )
    .unwrap()
}
fn locator() -> CellLocatorSection {
    CellLocatorSection {
        root: CellLocatorChild::Cell(0),
        nodes: Vec::new(),
    }
}

#[test]
fn teleport_residency_source_closed_face_edge_vertex_contacts_ignore_solid_and_exterior() {
    let (mut cells, tree) = cells_tree(&[
        ([0.0; 3], [1.0; 3]),
        ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
        ([1.0, 1.0, 0.0], [2.0, 2.0, 1.0]),
        ([1.0; 3], [2.0; 3]),
        ([0.0; 3], [1.0; 3]),
        ([0.0; 3], [1.0; 3]),
    ]);
    cells.cells[4].flags = CELL_FLAG_SOLID;
    cells.cells[5].flags = CELL_FLAG_EXTERIOR;
    let section = TeleportersSection {
        teleporters: vec![record(vec![box_brush([0.25; 3], [1.0; 3])])],
    };
    let routes = build_routes(
        TeleportResidencyInput {
            section: &section,
            tree: &tree,
        },
        &cells,
        &world(&cells, &locator()),
    )
    .unwrap();
    assert_eq!(routes[0].source_cells, [0, 1, 2, 3]);
    assert_eq!(routes[0].arrival_cells, [0, 1, 2, 3]);
}
#[test]
fn teleport_residency_rejects_overlapping_aabbs_when_convex_regions_are_disjoint() {
    let (cells, mut tree) = cells_tree(&[([0.0; 3], [2.0; 3]), ([0.0; 3], [2.0; 3])]);
    tree.leaves[0]
        .defining_planes
        .push((-DVec3::new(1.0, 1.0, 0.0), -1.0));
    tree.leaves[1]
        .defining_planes
        .push((DVec3::new(1.0, 1.0, 0.0), 3.0));
    let section = TeleportersSection {
        teleporters: vec![record(vec![box_brush([1.8, 1.8, 0.2], [1.9, 1.9, 0.4])])],
    };
    let routes = build_routes(
        TeleportResidencyInput {
            section: &section,
            tree: &tree,
        },
        &cells,
        &world(&cells, &locator()),
    )
    .unwrap();
    assert_eq!(routes[0].source_cells, [1]);
}
#[test]
fn teleport_residency_seamless_four_rotations_map_whole_union_with_vertical_offset() {
    for degrees in [0.0f32, 90.0, 180.0, -90.0] {
        let source = vec![
            box_brush([1.0, 2.0, 0.5], [2.0, 3.0, 1.5]),
            box_brush([3.0, 2.0, 0.5], [4.0, 3.0, 1.5]),
        ];
        let mut teleporter = record(source);
        teleporter.landmark_origin = [1.0, 1.0, 1.0];
        teleporter.landmark_yaw = 0.25;
        teleporter.destination_origin = [20.0, 6.0, 10.0];
        teleporter.destination_yaw = 0.25 + degrees.to_radians();
        let rotation = DQuat::from_rotation_y(f64::from(
            teleporter.destination_yaw - teleporter.landmark_yaw,
        ));
        let image_center =
            |x| rotation * (DVec3::new(x, 2.5, 1.0) - DVec3::ONE) + DVec3::new(20.0, 6.0, 10.0);
        let bounds = |x| {
            let c = image_center(x);
            (
                (c - DVec3::splat(0.6)).as_vec3().to_array(),
                (c + DVec3::splat(0.6)).as_vec3().to_array(),
            )
        };
        let (cells, tree) =
            cells_tree(&[([0.0, 1.0, 0.0], [5.0, 4.0, 2.0]), bounds(1.5), bounds(3.5)]);
        let section = TeleportersSection {
            teleporters: vec![teleporter],
        };
        let routes = build_routes(
            TeleportResidencyInput {
                section: &section,
                tree: &tree,
            },
            &cells,
            &world(&cells, &locator()),
        )
        .unwrap();
        assert_eq!(routes[0].source_cells, [0], "{degrees}");
        assert_eq!(routes[0].arrival_cells, [1, 2], "{degrees}");
    }
}
#[test]
fn teleport_residency_classic_uses_locator_on_plane_front_rule() {
    let (cells, tree) = cells_tree(&[
        ([0.0; 3], [10.0, 1.0, 1.0]),
        ([10.0, 0.0, 0.0], [20.0, 1.0, 1.0]),
    ]);
    let locator = CellLocatorSection {
        root: CellLocatorChild::Node(0),
        nodes: vec![CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: 10.0,
            front: CellLocatorChild::Cell(1),
            back: CellLocatorChild::Cell(0),
        }],
    };
    let mut teleporter = record(vec![box_brush([1.0, 0.25, 0.25], [2.0, 0.75, 0.75])]);
    teleporter.mode = TeleportMode::Classic;
    teleporter.destination_origin = [10.0, 0.5, 0.5];
    let section = TeleportersSection {
        teleporters: vec![teleporter],
    };
    let routes = build_routes(
        TeleportResidencyInput {
            section: &section,
            tree: &tree,
        },
        &cells,
        &world(&cells, &locator),
    )
    .unwrap();
    assert_eq!(routes[0].source_cells, [0]);
    assert_eq!(routes[0].arrival_cells, [1]);
}
fn pair(a: u32, b: u32, d: u32) -> CoupledPairRecord {
    CoupledPairRecord {
        cell_a: a,
        cell_b: b,
        distance: d,
        aperture: 0,
    }
}
#[test]
fn teleport_residency_minimum_source_distance_and_merged_nearest_origins_win() {
    let neighbors = Neighbors::from_pairs(7, &[pair(0, 1, 8), pair(0, 2, 3), pair(0, 3, 5)]);
    let routes = [
        TeleportResidencyRoute {
            source_cells: vec![1],
            arrival_cells: vec![4],
        },
        TeleportResidencyRoute {
            source_cells: vec![2, 3],
            arrival_cells: vec![5],
        },
    ];
    let arrivals = arrival_origins(&routes, &neighbors, &[0, 1, 2, 3, 4, 5, 6], 7, 8);
    assert!(arrivals[0].contains(&(5, 3)));
    assert!(arrivals[2].contains(&(5, 0)));
    let visible = vec![vec![], vec![4], vec![], vec![], vec![], vec![4, 6], vec![]];
    let map = build_lead_map(
        &LeadSources {
            neighbors: &neighbors,
            visible: &visible,
            cell_count: 7,
            arrival_origins: &arrivals,
        },
        &[0],
        8,
    );
    for cell in [4, 5, 6] {
        assert!(
            map.entries.contains(&(cell, 3)),
            "cell {cell}: {:?}",
            map.entries
        );
    }
    let mut visible_nearer = visible.clone();
    visible_nearer[2] = vec![4];
    let map = build_lead_map(
        &LeadSources {
            neighbors: &neighbors,
            visible: &visible_nearer,
            cell_count: 7,
            arrival_origins: &arrivals,
        },
        &[0],
        8,
    );
    assert!(map.entries.contains(&(4, 3)));
    let limited = arrival_origins(&routes, &neighbors, &[0, 1, 2, 3, 4, 5, 6], 7, 2);
    assert!(limited[0].is_empty(), "outside maximum lead");
}
fn corridor_fixture() -> (
    CellsSection,
    PortalsSection,
    CellLocatorSection,
    BspTree,
    TeleportersSection,
) {
    let (cells, portals, locator) =
        crate::cell_residency_bake::test_fixtures::l_corridor_sections();
    let bounds: Vec<_> = cells
        .cells
        .iter()
        .map(|c| (c.bounds_min, c.bounds_max))
        .collect();
    let (_, tree) = cells_tree(&bounds);
    let mut t = record(vec![box_brush([1.0, 0.5, 0.5], [2.0, 3.0, 3.0])]);
    t.mode = TeleportMode::Classic;
    t.destination_origin = [45.0, 2.0, 59.0];
    (
        cells,
        portals,
        locator,
        tree,
        TeleportersSection {
            teleporters: vec![t],
        },
    )
}
#[test]
fn teleport_residency_disabled_parity_cold_warm_bytes_and_same_cell_key_mutations() {
    let (cells, portals, locator, tree, section) = corridor_fixture();
    let temp = tempfile::tempdir().unwrap();
    let cache = crate::cache::StageCache::new(temp.path()).unwrap();
    let bake = |section: &TeleportersSection, cache| {
        cell_residency_set_bake_cached(
            &cells,
            &portals,
            &locator,
            Some(TeleportResidencyInput {
                section,
                tree: &tree,
            }),
            cache,
            &crate::bake_control::BakeControl::unrestricted(),
        )
        .unwrap()
        .unwrap()
    };
    let cold = bake(&section, None);
    let warm = bake(&section, Some(&cache));
    let hit = bake(&section, Some(&cache));
    assert_eq!(cold.to_bytes(), warm.to_bytes());
    assert_eq!(hit.to_bytes(), cold.to_bytes());
    let mut disabled = section.clone();
    disabled.teleporters[0].enabled_on_spawn = false;
    assert_eq!(bake(&disabled, None), cold);
    // Non-vacuous: the teleporter changes the bake, so parity above is not
    // two teleporter-blind bakes agreeing.
    let without = cell_residency_set_bake_cached(
        &cells,
        &portals,
        &locator,
        None,
        None,
        &crate::bake_control::BakeControl::unrestricted(),
    )
    .unwrap()
    .unwrap();
    assert_ne!(without.to_bytes(), cold.to_bytes());
    let key = |s: &TeleportersSection, t: &BspTree| {
        cell_residency_set_cache_key(&cells, &portals, &locator, s, Some(t)).as_filename()
    };
    let base = key(&section, &tree);
    for field in 0..5 {
        let mut changed = section.clone();
        let t = &mut changed.teleporters[0];
        match field {
            0 => t.destination_origin[0] += 0.1,
            1 => {
                t.mode = TeleportMode::Seamless;
                t.landmark_origin[0] = 0.1
            }
            2 => t.mode = TeleportMode::Seamless,
            3 => t.brushes[0].planes[0].distance += 0.01,
            _ => t.destination_yaw += 0.01,
        }
        assert_ne!(key(&changed, &tree), base, "field {field}");
    }
    let mut fine_tree = tree;
    fine_tree.leaves[0].bounds.max.x += 1.0e-10;
    assert_ne!(
        key(&section, &fine_tree),
        base,
        "f64 classification inputs must be keyed"
    );
}
#[test]
fn teleport_residency_leaf_prefilter_matches_exhaustive_routes() {
    let (cells, portals, corridor_locator, tree, classic) = corridor_fixture();
    let corridor =
        LevelWorld::visibility_only_from_sections(&cells, &portals, &corridor_locator).unwrap();
    let mut seamless = classic.clone();
    for yaw in [0.0f32, 90.0, -135.0] {
        let mut t = classic.teleporters[0].clone();
        t.mode = TeleportMode::Seamless;
        t.landmark_origin = [1.5, 1.75, 1.75];
        t.landmark_yaw = 0.3;
        t.destination_yaw = 0.3 + yaw.to_radians();
        seamless.teleporters.push(t);
    }
    // A destination in no playable cell still keeps the source route.
    let mut nowhere = classic.clone();
    nowhere.teleporters[0].destination_origin = [-500.0, -500.0, -500.0];
    let mut checked_arrivals = 0;
    for section in [&classic, &seamless, &nowhere] {
        let input = TeleportResidencyInput {
            section,
            tree: &tree,
        };
        let filtered = build_routes_with(input, &cells, &corridor, LeafPrefilter::On).unwrap();
        let exhaustive = build_routes_with(input, &cells, &corridor, LeafPrefilter::Off).unwrap();
        assert_eq!(filtered, exhaustive);
        assert!(filtered.iter().all(|route| !route.source_cells.is_empty()));
        checked_arrivals += filtered
            .iter()
            .filter(|route| !route.arrival_cells.is_empty())
            .count();
    }
    assert!(checked_arrivals >= 4, "{checked_arrivals}");
    let (contact_cells, contact_tree) = cells_tree(&[
        ([0.0; 3], [1.0; 3]),
        ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
        ([1.0; 3], [2.0; 3]),
        ([5.0; 3], [6.0; 3]),
    ]);
    let contact = TeleportersSection {
        teleporters: vec![record(vec![box_brush([0.25; 3], [1.0; 3])])],
    };
    let input = TeleportResidencyInput {
        section: &contact,
        tree: &contact_tree,
    };
    let contact_world = world(&contact_cells, &locator());
    let filtered = build_routes_with(input, &contact_cells, &contact_world, LeafPrefilter::On);
    let exhaustive = build_routes_with(input, &contact_cells, &contact_world, LeafPrefilter::Off);
    assert_eq!(filtered.unwrap(), exhaustive.unwrap());
}
