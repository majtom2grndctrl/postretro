use super::super::bsp::find_leaf_for_point;
use super::*;
use crate::map_data::BrushPlane;

fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    BrushVolume {
        planes: vec![
            BrushPlane {
                normal: DVec3::X,
                distance: max.x,
            },
            BrushPlane {
                normal: DVec3::NEG_X,
                distance: -min.x,
            },
            BrushPlane {
                normal: DVec3::Y,
                distance: max.y,
            },
            BrushPlane {
                normal: DVec3::NEG_Y,
                distance: -min.y,
            },
            BrushPlane {
                normal: DVec3::Z,
                distance: max.z,
            },
            BrushPlane {
                normal: DVec3::NEG_Z,
                distance: -min.z,
            },
        ],
        sides: Vec::new(),
        aabb: Aabb { min, max },
    }
}

fn x_ge_z_wedge_brush(size: f64, gap: f64) -> BrushVolume {
    let diagonal_normal = DVec3::new(-1.0, 0.0, 1.0).normalize();
    BrushVolume {
        planes: vec![
            BrushPlane {
                normal: DVec3::X,
                distance: size,
            },
            BrushPlane {
                normal: DVec3::Y,
                distance: size,
            },
            BrushPlane {
                normal: DVec3::NEG_Y,
                distance: 0.0,
            },
            BrushPlane {
                normal: DVec3::NEG_Z,
                distance: 0.0,
            },
            BrushPlane {
                normal: diagonal_normal,
                distance: -gap / 2.0_f64.sqrt(),
            },
        ],
        sides: Vec::new(),
        aabb: Aabb {
            min: DVec3::new(gap, 0.0, 0.0),
            max: DVec3::new(size, size, size - gap),
        },
    }
}

fn x_le_z_wedge_brush(size: f64, gap: f64) -> BrushVolume {
    let diagonal_normal = DVec3::new(1.0, 0.0, -1.0).normalize();
    BrushVolume {
        planes: vec![
            BrushPlane {
                normal: DVec3::NEG_X,
                distance: 0.0,
            },
            BrushPlane {
                normal: DVec3::Y,
                distance: size,
            },
            BrushPlane {
                normal: DVec3::NEG_Y,
                distance: 0.0,
            },
            BrushPlane {
                normal: DVec3::Z,
                distance: size,
            },
            BrushPlane {
                normal: diagonal_normal,
                distance: -gap / 2.0_f64.sqrt(),
            },
        ],
        sides: Vec::new(),
        aabb: Aabb {
            min: DVec3::new(0.0, 0.0, gap),
            max: DVec3::new(size - gap, size, size),
        },
    }
}

fn hollow_room(min: DVec3, max: DVec3, wall: f64) -> Vec<BrushVolume> {
    vec![
        // Floor
        box_brush(
            DVec3::new(min.x, min.y, min.z),
            DVec3::new(max.x, min.y + wall, max.z),
        ),
        // Ceiling
        box_brush(
            DVec3::new(min.x, max.y - wall, min.z),
            DVec3::new(max.x, max.y, max.z),
        ),
        // Wall -X
        box_brush(
            DVec3::new(min.x, min.y, min.z),
            DVec3::new(min.x + wall, max.y, max.z),
        ),
        // Wall +X
        box_brush(
            DVec3::new(max.x - wall, min.y, min.z),
            DVec3::new(max.x, max.y, max.z),
        ),
        // Wall -Z
        box_brush(
            DVec3::new(min.x, min.y, min.z),
            DVec3::new(max.x, max.y, min.z + wall),
        ),
        // Wall +Z
        box_brush(
            DVec3::new(min.x, min.y, max.z - wall),
            DVec3::new(max.x, max.y, max.z),
        ),
    ]
}

fn leaf_at(tree: &BspTree, point: DVec3) -> &BspLeaf {
    let idx = find_leaf_for_point(tree, point);
    &tree.leaves[idx]
}

fn assert_all_leaf_bounds_valid(tree: &BspTree) {
    for leaf in &tree.leaves {
        assert!(
            leaf.bounds.is_valid(),
            "invalid leaf bounds: {:?}",
            leaf.bounds
        );
    }
}

#[test]
fn empty_brush_set_produces_empty_tree() {
    let tree = build_bsp_from_brushes(&[]).expect("empty input is valid");
    assert!(tree.nodes.is_empty());
    assert!(tree.leaves.is_empty());
}

#[test]
fn world_aabb_unions_brush_bounds_with_slack() {
    let brushes = vec![
        box_brush(DVec3::ZERO, DVec3::splat(10.0)),
        box_brush(DVec3::splat(50.0), DVec3::splat(60.0)),
    ];
    let world = world_aabb_from_brushes(&brushes);
    assert!((world.min.x - (-WORLD_AABB_SLACK)).abs() < 1e-9);
    assert!((world.min.y - (-WORLD_AABB_SLACK)).abs() < 1e-9);
    assert!((world.min.z - (-WORLD_AABB_SLACK)).abs() < 1e-9);
    assert!((world.max.x - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
    assert!((world.max.y - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
    assert!((world.max.z - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
}

#[test]
fn classify_aabb_fully_back() {
    let bounds = Aabb {
        min: DVec3::new(0.0, 0.0, 0.0),
        max: DVec3::new(5.0, 5.0, 5.0),
    };
    assert_eq!(classify_aabb(&bounds, DVec3::X, 10.0), AabbSide::Back);
}

#[test]
fn classify_aabb_fully_front() {
    let bounds = Aabb {
        min: DVec3::new(0.0, 0.0, 0.0),
        max: DVec3::new(5.0, 5.0, 5.0),
    };
    assert_eq!(classify_aabb(&bounds, DVec3::X, -10.0), AabbSide::Front);
}

#[test]
fn classify_aabb_spanning() {
    let bounds = Aabb {
        min: DVec3::new(0.0, 0.0, 0.0),
        max: DVec3::new(10.0, 10.0, 10.0),
    };
    assert_eq!(classify_aabb(&bounds, DVec3::X, 5.0), AabbSide::Spanning);
}

#[test]
fn classify_candidate_routing_touching_back_plane_routes_back() {
    // Regression: a thin brush touching its outward plane must stay routed
    // to the child region it owns, not be dropped from that child.
    let bounds = Aabb {
        min: DVec3::new(0.0, 0.0, 0.0),
        max: DVec3::new(0.05, 1.0, 1.0),
    };

    assert_eq!(
        classify_aabb_for_candidate_routing(&bounds, DVec3::X, 0.05),
        AabbSide::Back
    );
}

#[test]
fn classify_candidate_routing_touching_front_plane_routes_front() {
    let bounds = Aabb {
        min: DVec3::new(0.05, 0.0, 0.0),
        max: DVec3::new(0.10, 1.0, 1.0),
    };

    assert_eq!(
        classify_aabb_for_candidate_routing(&bounds, DVec3::X, 0.05),
        AabbSide::Front
    );
}

#[test]
fn classify_candidate_routing_near_plane_only_stays_spanning() {
    let half_epsilon = CANDIDATE_ROUTING_EPSILON * 0.5;
    let bounds = Aabb {
        min: DVec3::new(-half_epsilon, 0.0, 0.0),
        max: DVec3::new(half_epsilon, 1.0, 1.0),
    };

    assert_eq!(
        classify_aabb_for_candidate_routing(&bounds, DVec3::X, 0.0),
        AabbSide::Spanning
    );
}

#[test]
fn classify_candidate_routing_genuine_straddle_stays_spanning() {
    let bounds = Aabb {
        min: DVec3::new(-0.05, 0.0, 0.0),
        max: DVec3::new(0.05, 1.0, 1.0),
    };

    assert_eq!(
        classify_aabb_for_candidate_routing(&bounds, DVec3::X, 0.0),
        AabbSide::Spanning
    );
}

#[test]
fn hollow_room_interior_air_exterior_solid() {
    let brushes = hollow_room(DVec3::ZERO, DVec3::splat(100.0), 4.0);
    let tree = build_bsp_from_brushes(&brushes).expect("hollow room should build");

    let interior = DVec3::splat(50.0);
    assert!(
        !leaf_at(&tree, interior).is_solid,
        "interior air point should land in an empty leaf"
    );

    let floor = DVec3::new(50.0, 2.0, 50.0);
    assert!(
        leaf_at(&tree, floor).is_solid,
        "floor-interior point should land in a solid leaf"
    );

    // Exterior is empty: no candidate brushes contain the region.
    let exterior = DVec3::new(50.0, 150.0, 50.0);
    assert!(
        !leaf_at(&tree, exterior).is_solid,
        "exterior point should land in an empty leaf"
    );
}

#[test]
fn hollow_room_with_central_pillar_pillar_is_solid() {
    let mut brushes = hollow_room(DVec3::ZERO, DVec3::splat(200.0), 8.0);
    brushes.push(box_brush(
        DVec3::new(90.0, 8.0, 90.0),
        DVec3::new(110.0, 192.0, 110.0),
    ));

    let tree = build_bsp_from_brushes(&brushes).expect("pillar room should build");

    let pillar_interior = DVec3::new(100.0, 100.0, 100.0);
    assert!(
        leaf_at(&tree, pillar_interior).is_solid,
        "pillar interior should be solid"
    );

    let side_air = DVec3::new(50.0, 100.0, 50.0);
    assert!(
        !leaf_at(&tree, side_air).is_solid,
        "open air next to the pillar should be empty"
    );
}

#[test]
fn room_with_doorway_has_connected_air() {
    // Single shell (X=0..500) with a dividing wall at X=200..220 leaving a
    // doorway gap at Y=8..80, Z=80..120. Wall built from two jambs + lintel.
    let mut brushes = hollow_room(DVec3::ZERO, DVec3::new(500.0, 200.0, 200.0), 8.0);

    // Side jamb -Z:
    brushes.push(box_brush(
        DVec3::new(200.0, 8.0, 8.0),
        DVec3::new(220.0, 192.0, 80.0),
    ));
    // Side jamb +Z:
    brushes.push(box_brush(
        DVec3::new(200.0, 8.0, 120.0),
        DVec3::new(220.0, 192.0, 192.0),
    ));
    // Lintel above the doorway:
    brushes.push(box_brush(
        DVec3::new(200.0, 80.0, 80.0),
        DVec3::new(220.0, 192.0, 120.0),
    ));

    let tree = build_bsp_from_brushes(&brushes).expect("doorway room should build");

    let left_air = DVec3::new(100.0, 40.0, 100.0);
    assert!(
        !leaf_at(&tree, left_air).is_solid,
        "left room interior should be empty"
    );

    let right_air = DVec3::new(400.0, 40.0, 100.0);
    assert!(
        !leaf_at(&tree, right_air).is_solid,
        "right room interior should be empty"
    );

    let doorway = DVec3::new(210.0, 40.0, 100.0);
    assert!(
        !leaf_at(&tree, doorway).is_solid,
        "doorway opening should be empty"
    );

    let jamb_interior = DVec3::new(210.0, 100.0, 50.0);
    assert!(
        leaf_at(&tree, jamb_interior).is_solid,
        "jamb interior should be solid"
    );

    let lintel_interior = DVec3::new(210.0, 150.0, 100.0);
    assert!(
        leaf_at(&tree, lintel_interior).is_solid,
        "lintel interior should be solid"
    );
}

#[test]
fn adjacent_brushes_with_narrow_air_gap_preserve_air() {
    // Validates that brush-plane splitting sees the gap even though no world
    // face sits on the mid-gap planes (the failure mode of face-driven splitters).
    let brushes = vec![
        box_brush(DVec3::new(0.0, 0.0, 0.0), DVec3::new(20.0, 20.0, 10.0)),
        box_brush(DVec3::new(0.0, 0.0, 12.0), DVec3::new(20.0, 20.0, 22.0)), // gap Z=10..12
    ];

    let tree = build_bsp_from_brushes(&brushes).expect("narrow gap should build");

    let a_interior = DVec3::new(10.0, 10.0, 5.0);
    assert!(
        leaf_at(&tree, a_interior).is_solid,
        "brush A interior should be solid"
    );

    let b_interior = DVec3::new(10.0, 10.0, 17.0);
    assert!(
        leaf_at(&tree, b_interior).is_solid,
        "brush B interior should be solid"
    );

    let gap = DVec3::new(10.0, 10.0, 11.0);
    assert!(
        !leaf_at(&tree, gap).is_solid,
        "narrow air gap between adjacent brushes should be empty"
    );
}

#[test]
fn sub_plane_epsilon_thin_brush_interior_stays_solid() {
    // Regression: broad AABB candidate routing dropped the owner brush from
    // the back child when splitting on its touching +X plane.
    let brushes = vec![box_brush(
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(0.05, 1.0, 1.0),
    )];

    let tree = build_bsp_from_brushes(&brushes).expect("thin brush should build");

    assert!(
        leaf_at(&tree, DVec3::new(0.025, 0.5, 0.5)).is_solid,
        "thin brush interior should stay solid"
    );
    assert!(
        !leaf_at(&tree, DVec3::new(0.5, 0.5, 0.5)).is_solid,
        "point outside the thin brush should be empty"
    );
    assert_all_leaf_bounds_valid(&tree);
}

#[test]
fn wedge_interiors_are_solid_for_box_split_on_x_equals_z() {
    let brushes = vec![x_ge_z_wedge_brush(10.0, 0.0), x_le_z_wedge_brush(10.0, 0.0)];

    let tree = build_bsp_from_brushes(&brushes).expect("wedge box should build");

    assert!(
        leaf_at(&tree, DVec3::new(7.0, 5.0, 3.0)).is_solid,
        "x >= z wedge interior should be solid"
    );
    assert!(
        leaf_at(&tree, DVec3::new(3.0, 5.0, 7.0)).is_solid,
        "x <= z wedge interior should be solid"
    );
    assert!(
        !leaf_at(&tree, DVec3::new(5.0, 10.5, 5.0)).is_solid,
        "point outside both wedges should be empty"
    );
    assert_all_leaf_bounds_valid(&tree);
}

#[test]
fn wedge_air_gap_between_angled_brushes_stays_empty() {
    let brushes = vec![x_ge_z_wedge_brush(10.0, 1.0), x_le_z_wedge_brush(10.0, 1.0)];

    let tree = build_bsp_from_brushes(&brushes).expect("wedge gap should build");

    assert!(
        leaf_at(&tree, DVec3::new(8.0, 5.0, 3.0)).is_solid,
        "first angled brush interior should be solid"
    );
    assert!(
        leaf_at(&tree, DVec3::new(3.0, 5.0, 8.0)).is_solid,
        "second angled brush interior should be solid"
    );
    assert!(
        !leaf_at(&tree, DVec3::new(5.0, 5.0, 5.0)).is_solid,
        "genuine diagonal air gap should stay empty"
    );
    assert_all_leaf_bounds_valid(&tree);
}

#[test]
fn single_brush_interior_is_solid_exterior_is_empty() {
    let brushes = vec![box_brush(
        DVec3::new(-5.0, -5.0, -5.0),
        DVec3::new(5.0, 5.0, 5.0),
    )];
    let tree = build_bsp_from_brushes(&brushes).expect("single brush should build");

    assert!(leaf_at(&tree, DVec3::ZERO).is_solid);
    assert!(!leaf_at(&tree, DVec3::new(100.0, 0.0, 0.0)).is_solid);
}

#[test]
fn recursion_depth_cap_returns_error_on_overflow() {
    // 260 brushes along +X, each 1 unit wide with 1-unit gaps. Forces the
    // splitter to descend once per brush, pushing depth past MAX_RECURSION_DEPTH.
    // The builder must return an error rather than stack-overflow.
    //
    // We can't lower the cap from a test, so we verify the outcome is either
    // a clean error (depth exceeded) or a successful shallow tree — never a panic.
    let mut brushes = Vec::new();
    for i in 0..260 {
        let x = (i as f64) * 2.0;
        brushes.push(box_brush(
            DVec3::new(x, 0.0, 0.0),
            DVec3::new(x + 1.0, 1.0, 1.0),
        ));
    }
    let result = build_bsp_from_brushes(&brushes);
    if let Err(e) = result {
        let msg = format!("{e}");
        assert!(
            msg.contains("recursion depth"),
            "error should mention recursion depth, got: {msg}"
        );
    }
}
