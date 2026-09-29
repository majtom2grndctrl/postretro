use glam::Vec3;
use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData,
};

use super::dry_run_test_fixtures::{METER, bc6h_formats, chart, input};
use super::mandatory::Granularity;
use super::portal_distance::{HubCell, HubPortal, PortalGraphInput, recompute_pairs};
use super::pvs_sampling::{SamplingStats, eye_points, sample_pvs};
use super::{DryRunInput, run_dry_run};

/// A U-turn, 4 m tall: corridor 0 (x 0–10) opens at x = 10 into shaft 1
/// (x 10–14, z 0–14), which opens back at x = 10 into corridor 2 (z 10–14).
/// Solid cell 3 fills x 0–10, z 4–10. Cell 0's AABB overstates it to z 6, as
/// a convex cell's box can, so its upper lattice points land in solid.
const BOUNDS: [([f32; 3], [f32; 3]); 4] = [
    ([0.0, 0.0, 0.0], [10.0, 4.0, 6.0]),
    ([10.0, 0.0, 0.0], [14.0, 4.0, 14.0]),
    ([0.0, 0.0, 10.0], [10.0, 4.0, 14.0]),
    ([0.0, 0.0, 4.0], [10.0, 4.0, 10.0]),
];
const SOLID: [bool; 4] = [false, false, false, true];

fn portal_polygons() -> [(u32, u32, Vec<[f32; 3]>); 2] {
    let quad = |z0: f32, z1: f32| {
        vec![
            [10.0, 0.0, z0],
            [10.0, 4.0, z0],
            [10.0, 4.0, z1],
            [10.0, 0.0, z1],
        ]
    };
    [(0, 1, quad(0.0, 4.0)), (1, 2, quad(10.0, 14.0))]
}

fn u_turn_world() -> LevelWorld {
    let refs: [&[u32]; 4] = [&[0], &[0, 1], &[1], &[]];
    let mut portal_refs = Vec::new();
    let cells = BOUNDS
        .iter()
        .zip(SOLID)
        .zip(refs)
        .enumerate()
        .map(|(cell, (((min, max), solid), cell_refs))| {
            let data = CellData {
                bounds_min: Vec3::from(*min),
                bounds_max: Vec3::from(*max),
                face_start: cell as u32,
                face_count: u32::from(!solid),
                portal_ref_start: portal_refs.len() as u32,
                portal_ref_count: cell_refs.len() as u32,
                is_solid: solid,
                is_exterior: false,
                is_drawable: !solid,
            };
            portal_refs.extend_from_slice(cell_refs);
            data
        })
        .collect();
    let plane = |normal: Vec3, distance, front, back| CellLocatorNodeData {
        plane_normal: normal,
        plane_distance: distance,
        front,
        back,
    };
    let nodes = vec![
        plane(
            Vec3::X,
            10.0,
            CellLocatorChild::Cell(1),
            CellLocatorChild::Node(1),
        ),
        plane(
            Vec3::Z,
            10.0,
            CellLocatorChild::Cell(2),
            CellLocatorChild::Node(2),
        ),
        plane(
            Vec3::Z,
            4.0,
            CellLocatorChild::Cell(3),
            CellLocatorChild::Cell(0),
        ),
    ];
    let portals = portal_polygons()
        .into_iter()
        .map(|(front, back, polygon)| PortalData {
            polygon: polygon.into_iter().map(Vec3::from).collect(),
            front_cell: front as usize,
            back_cell: back as usize,
        })
        .collect();
    LevelWorld::new_visibility_only(
        cells,
        portal_refs,
        CellLocatorChild::Node(0),
        nodes,
        portals,
        true,
    )
    .expect("valid U-turn visibility world")
}

/// The U-turn as a full dry-run input: one chart per open cell, cells 2 and 3
/// sharing cluster 2, untruncated id-46 pairs.
fn u_turn_input() -> DryRunInput {
    let graph = PortalGraphInput {
        cells: BOUNDS
            .iter()
            .zip(SOLID)
            .map(|((min, max), solid)| HubCell {
                bounds_min: *min,
                bounds_max: *max,
                solid,
            })
            .collect(),
        portals: portal_polygons()
            .into_iter()
            .map(|(front, back, vertices)| HubPortal {
                front,
                back,
                vertices,
            })
            .collect(),
    };
    let pairs = recompute_pairs(&graph, 128 * METER);
    let charts = vec![
        chart(0, 0, 0, 0, 20, 20),
        chart(1, 0, 20, 0, 12, 30),
        chart(2, 0, 32, 0, 24, 16),
    ];
    let mut fixture = input(bc6h_formats(64, 1, true), &[0, 1, 2, 2], charts, pairs);
    for (cell, (min, max)) in fixture.cells.iter_mut().zip(BOUNDS) {
        cell.center = [0, 1, 2].map(|axis| 0.5 * (min[axis] + max[axis]));
    }
    fixture.cells[3].camera_candidate = false;
    fixture.portal_graph = Some(graph);
    fixture.visibility_world = Some(u_turn_world());
    fixture
}

#[test]
fn eye_points_locate_into_their_own_cell_after_inset() {
    let world = u_turn_world();
    for cell in 0..3u32 {
        let mut stats = SamplingStats::default();
        let points = eye_points(&world, cell, &mut stats);
        assert_eq!(points.len(), 27, "cell {cell}: every lattice point lands");
        for (lattice, point) in &points {
            assert_eq!(world.locate_cell(*point), cell as usize, "{lattice:?}");
        }
        assert_eq!(stats.rejected, 0);
        if cell == 0 {
            // The z = 5.4 lattice layer lies in solid cell 3 and insets back.
            assert_eq!((stats.missed_into_solid, stats.inset), (9, 9));
        } else {
            assert_eq!((stats.in_cell, stats.inset), (27, 0), "cell {cell}");
        }
    }
}

#[test]
fn sampled_pvs_holds_its_own_cell_and_hides_the_u_turn() {
    let world = u_turn_world();
    let pvs = sample_pvs(&world, &[0, 1, 2]);
    assert_eq!(
        pvs.dense[0],
        vec![0, 1],
        "the far corridor sits behind the turn"
    );
    assert_eq!(pvs.dense[1], vec![0, 1, 2]);
    assert_eq!(pvs.dense[2], vec![1, 2]);
    assert!(pvs.dense[3].is_empty(), "solid cells are never sampled");
    for cell in 0..3 {
        assert!(pvs.sparse[cell].contains(&(cell as u32)));
        assert!(
            pvs.sparse[cell].iter().all(|c| pvs.dense[cell].contains(c)),
            "the dense lattice contains the sparse one"
        );
    }
    assert_eq!(pvs.stats.walks, 3 * 27 * 6);
    assert_eq!(
        (pvs.stats.step_limit_walks, pvs.stats.frustum_all_walks),
        (0, 0)
    );
}

#[test]
fn visible_set_grows_monotonically_with_movement_lead() {
    let report = run_dry_run(&u_turn_input());
    let visible = report
        .visible_set
        .as_ref()
        .expect("world and graph present");
    for result in &visible.dense {
        for window in result.leads.windows(2) {
            for ((cell, near), (_, far)) in window[0].cells.iter().zip(&window[1].cells) {
                let case = format!("{:?} cell {cell}", result.granularity);
                assert!(far.cell_count >= near.cell_count, "{case}");
                assert!(far.texel_exact >= near.texel_exact, "{case}");
                for (f, n) in far.layer.iter().zip(&near.layer) {
                    assert!(f >= n, "{case}");
                }
            }
        }
    }
    // Cell 0 sees only the shaft; a 16 m lead reaches the shaft (10.5 m) and
    // everything the shaft sees, which includes the far corridor.
    let cell_granular = &visible.dense[0];
    assert_eq!(cell_granular.granularity, Granularity::Cell);
    let set_sizes: Vec<usize> = cell_granular
        .leads
        .iter()
        .map(|lead| lead.cells[0].1.cell_count)
        .collect();
    assert_eq!(set_sizes, vec![2, 3, 3]);
    // Cluster closure drags solid cell 3 in with its cluster-mate, cell 2.
    let closure = &visible.dense[1];
    assert_eq!(closure.leads[0].cells[1].1.cell_count, 4);
    assert!(
        visible
            .dense
            .iter()
            .flat_map(|result| &result.leads)
            .all(|lead| lead.reached_without_pvs == 0)
    );
}

#[test]
fn sightline_is_the_farthest_hub_distance_to_a_sampled_visible_cell() {
    let report = run_dry_run(&u_turn_input());
    let sightlines = &report.visible_set.as_ref().unwrap().sightlines;
    // Hub metric: cell 0 → shaft is 5.10 + 5.39 m, shaft → cell 2 is
    // 5.39 + 5 m. Nobody sees across the turn, so nothing reads 20.9 m.
    let expected = [
        (0, 26f64.sqrt() + 29f64.sqrt()),
        (1, 26f64.sqrt() + 29f64.sqrt()),
        (2, 29f64.sqrt() + 5.0),
    ];
    for ((cell, meters), (expected_cell, expected_meters)) in
        sightlines.per_cell.iter().zip(expected)
    {
        assert_eq!(*cell, expected_cell);
        assert!(
            (meters - expected_meters).abs() < 1e-4,
            "{cell}: {meters} vs {expected_meters}"
        );
    }
    assert_eq!(sightlines.unreachable_visible, 0);
}

#[test]
fn visible_set_report_is_deterministic_and_states_its_lower_bound() {
    let first = run_dry_run(&u_turn_input());
    let second = run_dry_run(&u_turn_input());
    assert_eq!(first, second);
    let rendered = first.render();
    assert_eq!(rendered, second.render());
    assert!(rendered.contains("lower bound on true visibility"));
    assert!(rendered.contains("visible-set mandatory bytes per camera cell"));
    assert!(rendered.contains("convergence at L=0"));
    // Fixtures without a visibility world skip the pass entirely.
    let mut no_world = u_turn_input();
    no_world.visibility_world = None;
    let report = run_dry_run(&no_world);
    assert!(report.visible_set.is_none());
    assert!(!report.render().contains("visible-set mandatory bytes"));
}
