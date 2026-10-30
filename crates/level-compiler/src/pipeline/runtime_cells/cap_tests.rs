// Cell-size cap rows: occluder-tier leaves wider than the cap divide on the
// world grid, its guards refuse sliver, slab and mostly-blocked lines, and
// doors keep sealing across a cap line.
// See: context/plans/done/occluder-first-partition/index.md §Decisions

use glam::{DQuat, DVec3};

use super::FirstPass;
use super::test_fixtures::{Cells, MapText, engine};
use super::{classify, partition_with_occluders};
use crate::map_data::BrushVolume;
use crate::map_format::MapFormat;
use crate::partition::occluder_classify::test_fixtures::{box_brush, hall, turned_box};
use crate::partition::occluder_cluster::test_fixtures::facade_rects;
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::{Aabb, CAP_MIN_PIECE_FRACTION, CELL_SIZE_CAP_M, CellSizeCap};

/// The cap in Quake units, so fixtures sit on the cap's world grid.
fn cap_q() -> f64 {
    CELL_SIZE_CAP_M / MapFormat::IdTech2.units_to_meters()
}

/// `n` caps in Quake units: fixtures sized in caps pin the same behaviour at
/// any cap value.
fn caps(n: f64) -> f64 {
    n * cap_q()
}

/// A hall `[length, width, height]` 4.6875 caps long, under a cap wide and
/// over one tall: grid lines at one to four caps cut it into four whole caps
/// and a remainder over the sliver guard's quarter cap, and a cap along up
/// would cut it too.
fn long_hall() -> [f64; 3] {
    [caps(4.6875), caps(0.78125), caps(1.5625)]
}

/// A sealed hall of 16-unit slabs, interior `min..max` in Quake units.
fn hall_between(map: &mut MapText, min: [f64; 3], max: [f64; 3]) -> &mut MapText {
    let t = 16.0;
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    map.solid([x0 - t, y0 - t, z0 - t], [x1 + t, y1 + t, z0])
        .solid([x0 - t, y0 - t, z1], [x1 + t, y1 + t, z1 + t])
        .solid([x0 - t, y0 - t, z0], [x0, y1 + t, z1])
        .solid([x1, y0 - t, z0], [x1 + t, y1 + t, z1])
        .solid([x0, y0 - t, z0], [x1, y0, z1])
        .solid([x0, y1, z0], [x1, y1 + t, z1])
}

/// Interior cell bounds, engine space.
fn interior_cell_bounds(cells: &Cells) -> Vec<Aabb> {
    (0..cells.cell_tree().leaves.len())
        .filter(|&cell| cells.is_interior_cell(cell))
        .map(|cell| cells.cell_tree().leaves[cell].bounds.clone())
        .collect()
}

#[test]
fn cell_size_cap_divides_a_hall_several_caps_long_on_the_world_grid_and_never_along_up() {
    // Its walls occlude, so it is one occluder-tier leaf before the cap.
    let size = long_hall();
    let mut map = MapText::new();
    map.hall(size);
    let cells = Cells::of(&map);

    let interior = interior_cell_bounds(&cells);
    assert_eq!(
        interior.len(),
        5,
        "grid lines at one, two, three and four caps cut the hall into five cells: \
         {interior:#?}"
    );
    let height = engine([0.0, 0.0, size[2]]).y;
    for bounds in &interior {
        let size = bounds.max - bounds.min;
        for axis in [0, 2] {
            assert!(
                size[axis] <= 1.25 * CELL_SIZE_CAP_M + 1e-6,
                "cell {bounds:?} is wider than 1.25 caps"
            );
        }
        assert!(
            (size.y - height).abs() < 1e-6,
            "the cap never splits along up: {bounds:?}"
        );
    }
    let (near, mid_width) = (caps(0.2), size[1] / 2.0);
    assert_ne!(
        cells.cell_at([near, mid_width, 60.0]),
        cells.cell_at([near + caps(2.3), mid_width, 60.0]),
        "points 2.3 caps apart down the hall lie in different cells"
    );
    assert_eq!(cells.runtime.portals.len(), 4, "one portal per cap line");
    for portal in &cells.runtime.portals {
        let z = portal.polygon[0].z;
        assert!(
            portal.polygon.iter().all(|p| (p.z - z).abs() < 1e-9),
            "cap portals stand on vertical world-grid planes"
        );
        let lines = z / CELL_SIZE_CAP_M;
        assert!(
            (lines - lines.round()).abs() < 1e-9,
            "portal at z {z} lies off the cap grid"
        );
    }
}

#[test]
fn cell_size_cap_leaves_a_hall_under_the_cap_whole() {
    // 0.94 cap long across the grid line at one cap, and over a cap tall.
    let mut map = MapText::new();
    hall_between(
        &mut map,
        [caps(0.5859), 0.0, 0.0],
        [caps(1.5234), caps(0.78125), caps(1.5625)],
    );
    let cells = Cells::of(&map);
    assert_eq!(interior_cell_bounds(&cells).len(), 1, "one interior cell");
    assert!(cells.runtime.portals.is_empty(), "and no portal");
}

#[test]
fn cell_size_cap_sliver_guard_decides_at_a_quarter_cap() {
    // A hall from `start` to two caps along x: past the cap, with one grid
    // line inside it, one cap from its far end, leaving `cap_q - start`.
    let quarter = CAP_MIN_PIECE_FRACTION * cap_q();
    let cells_from = |start: f64| {
        let mut map = MapText::new();
        hall_between(
            &mut map,
            [start, 0.0, 0.0],
            [caps(2.0), caps(0.78125), 256.0],
        );
        interior_cell_bounds(&Cells::of(&map)).len()
    };
    assert_eq!(
        cells_from(cap_q() - (quarter - 8.0)),
        1,
        "a line leaving a piece just under a quarter cap is refused"
    );
    assert_eq!(
        cells_from(cap_q() - (quarter + 8.0)),
        2,
        "a line leaving a piece just over a quarter cap splits"
    );
}

/// Occluder-tier leaves ending on the grid line at one cap in a hall one and a
/// half caps long, 128 u (3.25 m) tall and under a cap wide, whose only grid
/// line inside it is that one. A block across the hall's whole width
/// straddles the line, `block_top` units tall, under the see-over height so
/// exempt; none when `block_top` is `None`. Any leaf ending on the line means
/// the cap split there.
fn tier_leaves_on_the_line_over_a_block(block_top: Option<f64>) -> usize {
    let width = caps(0.78125);
    let mut map = MapText::new();
    hall_between(&mut map, [0.0, 0.0, 0.0], [caps(1.5), width, 128.0]);
    if let Some(top) = block_top {
        map.solid([cap_q() - 32.0, 0.0, 0.0], [cap_q() + 32.0, width, top]);
    }
    let cells = Cells::of(&map);
    let line_z = engine([cap_q(), 0.0, 0.0]).z;
    cells
        .partition
        .tier
        .leaves
        .iter()
        .filter(|leaf| {
            (leaf.bounds.min.z - line_z).abs() < 1e-6 || (leaf.bounds.max.z - line_z).abs() < 1e-6
        })
        .count()
}

#[test]
fn cell_size_cap_refuses_a_line_whose_slice_is_mostly_solid() {
    // A block 96 u (2.44 m) tall leaves the line's slice three quarters
    // solid.
    assert!(
        tier_leaves_on_the_line_over_a_block(None) > 0,
        "precondition: the open hall splits on the line"
    );
    assert_eq!(
        tier_leaves_on_the_line_over_a_block(Some(96.0)),
        0,
        "the line through the block is refused"
    );
}

#[test]
fn cell_size_cap_refuses_a_line_whose_slice_is_exactly_half_open() {
    // A block 64 u tall leaves the slice exactly half open on the authoring
    // grid: not strictly more than half, however its pieces' areas round.
    assert_eq!(
        tier_leaves_on_the_line_over_a_block(Some(64.0)),
        0,
        "the line through a half-solid slice is refused"
    );
}

// Regression: the open-slice guard sampled an 8 × 8 lattice, which read a
// slice just over half open as exactly half and refused it, leaving a 36 m
// cell over the roofs on stress-street.
#[test]
fn cell_size_cap_splits_on_a_line_whose_slice_is_just_over_half_open() {
    // A block 60 u tall leaves the slice 53% open by area. The lattice read
    // it as exactly half: its four lower rows, centred at 8 to 56 u, lay in
    // the block.
    assert!(
        tier_leaves_on_the_line_over_a_block(Some(60.0)) > 0,
        "the line through a slice just over half open splits"
    );
}

// Regression: the open-slice guard judged a corridor's slices against its
// whole occluder-tier leaf, solid mass included, so 120 m corridors on
// stress-warren-hallway-inspection stayed whole at every cap.
#[test]
fn cell_size_cap_cuts_a_corridor_several_caps_long_whose_tier_leaf_also_holds_solid() {
    // A corridor 128 u (3.25 m) wide and 192 u (4.88 m) tall under an exempt
    // mass filling the rest of a sealed hall 512 u (13 m) tall: the hall's
    // slabs bound one occluder-tier leaf, and each cap line's slice through
    // it is only three eighths open.
    let [length, width, corridor_top] = [caps(4.6875), 128.0, 192.0];
    let mut map = MapText::new();
    hall_between(&mut map, [0.0, 0.0, 0.0], [length, width, 512.0]);
    map.solid([0.0, 0.0, corridor_top], [length, width, 512.0]);
    let cells = Cells::of(&map);

    let tier = &cells.partition.tier;
    let tier_leaf = tier.leaf_tier_leaf[cells.leaf_at([caps(0.2), width / 2.0, 60.0])];
    assert!(
        tier.leaves[tier_leaf]
            .leaf_range
            .clone()
            .any(|leaf| cells.full_tree().leaves[leaf].is_solid),
        "precondition: the corridor's occluder-tier leaf also holds solid"
    );
    let interior = interior_cell_bounds(&cells);
    assert_eq!(
        interior.len(),
        5,
        "grid lines at one, two, three and four caps cut the corridor into five cells: \
         {interior:#?}"
    );
    for bounds in &interior {
        let size = bounds.max - bounds.min;
        assert!(
            size.x.max(size.z) <= 1.25 * CELL_SIZE_CAP_M + 1e-6,
            "cell {bounds:?} is wider than 1.25 caps"
        );
    }
    assert_ne!(
        cells.cell_at([caps(0.2), width / 2.0, 60.0]),
        cells.cell_at([caps(2.5), width / 2.0, 60.0]),
        "points 2.3 caps apart down the corridor lie in different cells"
    );
}

#[test]
fn cell_size_cap_adds_no_exterior_cell_in_the_void_around_the_world() {
    // The slack shell around the world is exterior void; the cap never cuts
    // it into a grid of void cells.
    let mut map = MapText::new();
    map.hall(long_hall());
    let cells = Cells::of(&map);
    assert_eq!(
        cells.runtime.cells.exterior_cells.len(),
        exterior_cells_uncapped(&map),
        "the cap adds no exterior cell"
    );
}

/// Exterior cells of `map` built with no cap, for comparison.
fn exterior_cells_uncapped(map: &MapText) -> usize {
    let map_data = map.parse();
    let brushes = &map_data.brush_volumes;
    let classification = classify(brushes).expect("classifies");
    let (result, tier) = crate::partition::partition_with_occluder_tier(
        brushes,
        &classification.tier_roles(),
        &[],
        &[],
        None,
    )
    .expect("partition builds");
    let portals = crate::portals::generate_portals(&result.tree);
    let exterior = crate::visibility::find_exterior_leaves(&result.tree, &portals);
    let pockets = crate::portals::interior_pockets(&result.tree, &tier, &exterior);
    crate::partition::CellTree::build(&result.tree, &tier, &exterior, &pockets)
        .exterior_cells
        .len()
}

#[test]
fn cell_size_cap_cells_do_not_depend_on_brush_order() {
    // A hall several caps long and over a cap wide, so the cap cuts both
    // horizontal axes, with a row of exempt pillars and a low exempt curb.
    let [length, width] = [caps(4.6875), caps(1.4)];
    let mut map = MapText::new();
    map.hall([length, width, 256.0]);
    for k in 0..12 {
        let x = caps(0.2) + caps(0.371) * f64::from(k);
        map.solid([x, width / 2.0, 0.0], [x + 40.0, width / 2.0 + 40.0, 256.0]);
    }
    map.solid([0.0, width - 140.0, 0.0], [length, width - 128.0, 12.0]);

    let cells_of = |reverse: bool| {
        let mut map_data = map.parse();
        if reverse {
            map_data.brush_volumes.reverse();
        }
        let cells = Cells::of_map_data(map_data);
        let mut bounds: Vec<[i64; 6]> = cells
            .partition
            .tier
            .leaves
            .iter()
            .map(|leaf| {
                let q = |v: f64| (v * 1e6).round() as i64;
                let (a, b) = (leaf.bounds.min, leaf.bounds.max);
                [q(a.x), q(a.y), q(a.z), q(b.x), q(b.y), q(b.z)]
            })
            .collect();
        bounds.sort_unstable();
        (bounds, cells.runtime.portals.len())
    };
    let (forward, reversed) = (cells_of(false), cells_of(true));
    assert!(forward.0.len() > 5, "the cap divides the hall");
    assert_eq!(forward, reversed, "same occluder-tier leaves and portals");
}

// ---- doors across a cap line (C10-C13) ----

#[test]
fn closed_door_seals_every_portal_piece_a_cap_line_cuts_across_its_doorway() {
    // The exempt-wall door of the C11 row, in a hall several caps long, set
    // across the grid line at one cap: the cap cuts each room's portal onto
    // the door in two, and the door covers each piece.
    let line = cap_q();
    let mut map = MapText::new();
    map.hall([caps(4.6875), 640.0, 128.0])
        .solid([line - 70.0, 320.0, 0.0], [line - 30.0, 328.0, 128.0])
        .solid([line + 30.0, 320.0, 0.0], [line + 70.0, 328.0, 128.0]);
    let door = ([line - 30.0, 320.0, 0.0], [line + 30.0, 328.0, 128.0]);
    map.mover("door", &[door], 112.0);
    let cells = Cells::of(&map);

    assert_ne!(
        cells.cell_at([line - 15.0, 160.0, 60.0]),
        cells.cell_at([line + 15.0, 160.0, 60.0]),
        "the cap line crosses the room in front of the doorway"
    );
    let pad = DVec3::splat(1e-3);
    let door_box = Aabb::from_points(&[engine(door.0), engine(door.1)]);
    let crossing: Vec<u32> = cells
        .runtime
        .portals
        .iter()
        .enumerate()
        .filter(|(_, portal)| {
            let centroid =
                portal.polygon.iter().copied().sum::<DVec3>() / portal.polygon.len() as f64;
            centroid.cmpge(door_box.min - pad).all() && centroid.cmple(door_box.max + pad).all()
        })
        .map(|(index, _)| index as u32)
        .collect();
    assert!(
        crossing.len() >= 3,
        "the cap line splits the door's face portals: {crossing:?}"
    );
    let sealed = &cells.sealed_portals()[0];
    for portal in &crossing {
        assert!(
            sealed.contains(portal),
            "portal {portal} crosses the doorway but the closed door does not seal it \
             (sealed {sealed:?})"
        );
    }
}

// ---- slab guard ----

/// The facade hall, 70 × 25 m and 4.6 caps deep along z.
fn facade_hall() -> DVec3 {
    DVec3::new(70.0, 25.0, 4.6 * CELL_SIZE_CAP_M)
}

/// `facade_hall` with a strip facade 31 m long and 15 m tall, its slab from
/// `z0` 0.3 m deep, turned `yaw` in plan about its own centre. With `z0`
/// near two caps, deeper open space lies on its +z side, so its section, and
/// the region holding its slab, lie toward z = 0.
fn facade_in_hall(z0: f64, yaw: f64) -> Vec<BrushVolume> {
    let mut brushes = hall(facade_hall());
    let (length, height, depth) = (31.0, 15.0, 0.3);
    let centre = DVec3::new(35.0, 0.0, z0 + depth / 2.0);
    let turn = DQuat::from_rotation_y(yaw);
    for [a0, a1, y0, y1] in facade_rects(length, height) {
        let local = DVec3::new((a0 + a1) / 2.0 - length / 2.0, (y0 + y1) / 2.0, 0.0);
        let half = DVec3::new((a1 - a0) / 2.0, (y1 - y0) / 2.0, depth / 2.0);
        brushes.push(turned_box(centre + turn * local, half, yaw));
    }
    brushes
}

#[test]
fn cell_size_cap_refuses_a_line_through_a_facade_slab_along_it_and_crosses_one_turned_away() {
    // The line at two caps crosses the slab of a facade 2 m short of it. At
    // 25° the slab lies along the cap's z lines and the line is refused; at
    // 40° it lies across them and the line splits. The sliver guard passes
    // the line either way, so the slab guard decides.
    let line = 2.0 * CELL_SIZE_CAP_M;
    let decide = |yaw_degrees: f64| {
        let brushes = facade_in_hall(line - 2.0, yaw_degrees.to_radians());
        let classification = classify(&brushes).expect("classifies");
        let section = classification
            .clusters
            .iter()
            .flat_map(|cluster| &cluster.sections)
            .find(|section| section.occludes)
            .expect("the facade occludes as a section")
            .clone();
        let first_pass = FirstPass::build(&brushes)
            .expect("first pass builds")
            .expect("brushes");
        let cap = CellSizeCap::new(first_pass.space(), &classification);

        // The region the slab lies in: the hall's air behind the bounding
        // face.
        let outward = section.bounding_face;
        let face = section.frame.center.dot(outward) + section.frame.extents[2] * 0.5;
        let hall_air = RegionPolytope::from_aabb(&Aabb {
            min: DVec3::ZERO,
            max: facade_hall(),
        });
        let region = hall_air.clip(outward, face).1;
        let bounds = region.vertex_aabb();
        let least = CAP_MIN_PIECE_FRACTION * CELL_SIZE_CAP_M;
        assert!(
            line - bounds.min.z >= least && bounds.max.z - line >= least,
            "precondition: the line leaves no sliver ({bounds:?})"
        );
        assert!(
            cap.crosses_a_slab(2, line, &region),
            "precondition: the line crosses the slab within the region"
        );
        cap.accepts_line(2, line, &region)
    };
    assert!(!decide(25.0), "a line through a slab along it is refused");
    assert!(decide(40.0), "a line across a slab turned away splits");
}

#[test]
fn cell_size_cap_puts_no_cell_boundary_inside_an_axis_aligned_facade_slab() {
    // The grid line at two caps lies inside the slab of an unturned facade
    // from 0.11 m short of it to 0.19 m past it. The slab bounds the region
    // it lies in, so the line would cut a sliver cell off it: no
    // occluder-tier leaf beside the facade ends on it, while the line at one
    // cap still divides the region behind. Over the facade's top and past its
    // ends the line splits as anywhere.
    let line = 2.0 * CELL_SIZE_CAP_M;
    let z0 = line - 0.11;
    let brushes = facade_in_hall(z0, 0.0);
    assert!(
        line > z0 && line < z0 + 0.3,
        "precondition: the line is in the slab"
    );
    let partition = partition_with_occluders(&brushes, &[]).expect("partition builds");
    let ends_on = |z: f64| {
        partition
            .tier
            .leaves
            .iter()
            .filter(|leaf| {
                let b = &leaf.bounds;
                let beside_facade =
                    b.min.y > -1e-6 && b.min.y < 15.0 && b.min.x < 50.5 && b.max.x > 19.5;
                beside_facade && ((b.min.z - z).abs() < 1e-6 || (b.max.z - z).abs() < 1e-6)
            })
            .count()
    };
    assert_eq!(ends_on(line), 0, "no cell boundary on the line in the slab");
    assert!(
        ends_on(CELL_SIZE_CAP_M) > 0,
        "the line at one cap still splits"
    );
}

// ---- open-slice guard on brush faces ----

/// A hall 1.5 caps long on x, 3.25 m tall and 20 m wide, its only grid line
/// at one cap, where two exempt blocks across its width meet: one 2.44 m
/// tall on the near side, one 0.8 m tall on the far side. Just before the
/// line the slice is a quarter open, just past it three quarters.
fn blocks_meeting_on_a_grid_line() -> Vec<BrushVolume> {
    let (length, height, width) = (1.5 * CELL_SIZE_CAP_M, 3.25, 20.0);
    let mut brushes = hall(DVec3::new(length, height, width));
    brushes.push(box_brush(
        DVec3::new(CELL_SIZE_CAP_M - 1.6, 0.0, 0.0),
        DVec3::new(CELL_SIZE_CAP_M, 2.44, width),
    ));
    brushes.push(box_brush(
        DVec3::new(CELL_SIZE_CAP_M, 0.0, 0.0),
        DVec3::new(CELL_SIZE_CAP_M + 1.6, 0.8, width),
    ));
    brushes
}

// Regression: the open-slice guard read a slice lying on brush faces from
// whichever side the first pass's node plane, chosen by brush order, put in
// front, so solid on either side of the line flipped its verdict.
#[test]
fn cell_size_cap_reads_a_grid_line_on_brush_faces_alike_in_every_brush_order() {
    let brushes = blocks_meeting_on_a_grid_line();
    let interior = RegionPolytope::from_aabb(&Aabb {
        min: DVec3::ZERO,
        max: DVec3::new(1.5 * CELL_SIZE_CAP_M, 3.25, 20.0),
    });
    let accepts = |brushes: &[BrushVolume]| {
        let classification = classify(brushes).expect("classifies");
        let first_pass = FirstPass::build(brushes)
            .expect("first pass builds")
            .expect("brushes");
        CellSizeCap::new(first_pass.space(), &classification).accepts_line(
            0,
            CELL_SIZE_CAP_M,
            &interior,
        )
    };
    let orders: Vec<(&str, Vec<BrushVolume>)> = vec![
        ("forward", brushes.clone()),
        ("reversed", brushes.iter().rev().cloned().collect()),
        ("blocks swapped", {
            let mut swapped = brushes.clone();
            let last = swapped.len() - 1;
            swapped.swap(last - 1, last);
            swapped
        }),
    ];
    for (order, permuted) in orders {
        assert!(
            !accepts(&permuted),
            "{order}: a slice a quarter open on one side of the line is refused"
        );
    }
    let mut without_tall_block = brushes.clone();
    without_tall_block.remove(brushes.len() - 2);
    assert!(
        accepts(&without_tall_block),
        "precondition: with the low block alone, the slice is mostly open on both sides"
    );
}
