// Door rows: doors recognised before the occluder-tier build, their splitters
// bounding the doorway, and sealing read against the runtime portals.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::BTreeSet;

use glam::DVec3;

use super::test_fixtures::{Cells, MapText, engine, walk_blocked};
use super::{classify, doors};
use crate::partition::Aabb;
use crate::partition::occluder_classify::OccluderVerdict;

const NORTH: [f64; 3] = [0.0, 1.0, 0.0];
const EAST: [f64; 3] = [1.0, 0.0, 0.0];

/// Engine-space bounds of a Quake-unit box.
fn engine_box(min: [f64; 3], max: [f64; 3]) -> Aabb {
    Aabb::from_points(&[engine(min), engine(max)])
}

/// Runtime portals whose centroid lies inside `bounds`, within 1 mm.
fn portals_inside(cells: &Cells, bounds: &Aabb) -> Vec<u32> {
    let pad = DVec3::splat(1e-3);
    cells
        .runtime
        .portals
        .iter()
        .enumerate()
        .filter(|(_, portal)| {
            let centroid =
                portal.polygon.iter().copied().sum::<DVec3>() / portal.polygon.len() as f64;
            centroid.cmpge(bounds.min - pad).all() && centroid.cmple(bounds.max + pad).all()
        })
        .map(|(index, _)| index as u32)
        .collect()
}

// ---- a door in an exempt wall (C11) ----

/// Two wall pieces 40 u (1.0 m) wide, touching only the floor and ceiling.
const PIECES: [([f64; 3], [f64; 3]); 2] = [
    ([250.0, 320.0, 0.0], [290.0, 328.0, 128.0]),
    ([350.0, 320.0, 0.0], [390.0, 328.0, 128.0]),
];
/// The door between them, 60 u (1.5 m) wide, floor to ceiling.
const DOOR: ([f64; 3], [f64; 3]) = ([290.0, 320.0, 0.0], [350.0, 328.0, 128.0]);
const DOOR_CENTRE: [f64; 3] = [320.0, 324.0, 64.0];
const FRONT: [f64; 3] = [320.0, 160.0, 60.0];
const BACK: [f64; 3] = [320.0, 480.0, 60.0];

/// A hall 640 × 640 × 128 u (16 × 16 × 3.25 m) crossed by two wall pieces
/// with gaps of 250 u to the side walls, and, `with_door`, a door between
/// the pieces. Each piece is under the floor on its broad axis and touches no
/// other exempt brush, so the wall stays exempt as a whole.
fn exempt_wall(with_door: bool) -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 128.0]);
    for (min, max) in PIECES {
        map.solid(min, max);
    }
    if with_door {
        map.mover("door", &[DOOR], 112.0);
    }
    map
}

#[test]
fn door_in_an_exempt_wall_bounds_its_opening_and_seals_every_portal_across_it() {
    // Pins R8 and R17.
    let with = exempt_wall(true);
    let without = exempt_wall(false);
    let classification = classify(&with.parse().brush_volumes).expect("classifies");
    let pieces = [
        classification.verdicts.len() - 2,
        classification.verdicts.len() - 1,
    ];
    for piece in pieces {
        assert_ne!(classification.verdicts[piece], OccluderVerdict::Occludes);
    }
    assert!(
        !classification
            .tier_roles()
            .iter()
            .enumerate()
            .any(|(brush, role)| pieces.contains(&brush)
                && *role != crate::partition::TierRole::Exempt),
        "the wall pieces stay exempt alone and as a cluster"
    );
    let without_classification =
        classify(&without.parse().brush_volumes).expect("classifies without the door");
    assert_eq!(
        classification.verdicts, without_classification.verdicts,
        "the wall classifies the same with the door removed (R17)"
    );
    assert_eq!(
        classification.tier_roles(),
        without_classification.tier_roles()
    );

    let map_data = with.parse();
    let recognised = doors(&map_data.brush_volumes, &map_data.kinematic_movers).expect("doors");
    assert_eq!(recognised.len(), 1, "the mover fills a doorway");

    let bare = Cells::of(&without);
    assert_eq!(
        bare.cell_at(FRONT),
        bare.cell_at(BACK),
        "precondition: without the door the exempt wall bounds no cell"
    );
    let cells = Cells::of_map_data(map_data);
    assert_ne!(
        cells.cell_at(FRONT),
        cells.cell_at(BACK),
        "closed, the door puts its two sides in different cells"
    );

    let sealed = &cells.sealed_portals()[0];
    let crossing = portals_inside(&cells, &engine_box(DOOR.0, DOOR.1));
    assert!(
        !crossing.is_empty(),
        "at least one runtime portal crosses the opening"
    );
    for portal in &crossing {
        assert!(
            sealed.contains(portal),
            "portal {portal} crosses the opening but the closed door does not seal it \
             (sealed {sealed:?})"
        );
    }

    // Closed, the walk reaches nothing through the doorway itself; open, it
    // passes again. The rooms still see each other around the exempt wall.
    let world = cells.visibility_world();
    let door_cell = cells.cell_at(DOOR_CENTRE) as u32;
    let back_cell = cells.cell_at(BACK) as u32;
    let (open, _) = walk_blocked(&world, FRONT, NORTH, &[]);
    assert!(open.contains(&door_cell), "open, the doorway passes");
    let (closed, _) = walk_blocked(&world, FRONT, NORTH, sealed);
    assert!(!closed.contains(&door_cell), "closed, the doorway blocks");
    assert!(
        closed.contains(&back_cell),
        "the gaps beside the exempt wall still pass"
    );
}

// ---- rooms A, B and C (C12) ----

/// A doorway 60 u (1.5 m) wide and 100 u tall in a wall on `x`: two jamb
/// pieces over 20 m long, each occluding on the absolute length, and an
/// exempt lintel.
fn wall_with_doorway(map: &mut MapText, x: f64) {
    map.solid([x, 0.0, 0.0], [x + 16.0, 860.0, 160.0])
        .solid([x, 920.0, 0.0], [x + 16.0, 1800.0, 160.0])
        .solid([x, 860.0, 100.0], [x + 16.0, 920.0, 160.0]);
}

/// The A–B door flush with its wall, and one half the wall's depth set in
/// the middle of the doorway, whose broad planes split the doorway.
const FLUSH_DOOR: ([f64; 3], [f64; 3]) = ([400.0, 860.0, 0.0], [416.0, 920.0, 100.0]);
const RECESSED_DOOR: ([f64; 3], [f64; 3]) = ([404.0, 860.0, 0.0], [412.0, 920.0, 100.0]);

/// Rooms A, B and C in a row in a hall 1200 × 1800 × 160 u (30 × 46 × 4 m),
/// split by occluding walls with a doorway in each, and `door` in the
/// doorway between A and B. The dividing walls' slices through the hall
/// outsize the jambs' ends, so the walls split first and each doorway is its
/// own occluder-tier region.
fn three_rooms(door: Option<([f64; 3], [f64; 3])>) -> MapText {
    let mut map = MapText::new();
    map.hall([1200.0, 1800.0, 160.0]);
    wall_with_doorway(&mut map, 400.0);
    wall_with_doorway(&mut map, 800.0);
    if let Some(door) = door {
        map.mover("door_ab", &[door], 96.0);
    }
    map
}

/// Room C's interior cells, as bounds, and the portals touching them, as
/// vertex lists: geometry only, since cell and portal ids renumber when a
/// door elsewhere adds cells.
fn room_c(cells: &Cells) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut c_cells = BTreeSet::new();
    for x in (0..10).map(|i| 830.0 + 36.0 * f64::from(i)) {
        for y in (0..10).map(|i| 10.0 + 197.0 * f64::from(i)) {
            for z in [10.0, 80.0, 150.0] {
                let cell = cells.cell_at([x, y, z]);
                if cells.is_interior_cell(cell) {
                    c_cells.insert(cell);
                }
            }
        }
    }
    let round = |v: DVec3| format!("{:.6} {:.6} {:.6}", v.x, v.y, v.z);
    let bounds = c_cells
        .iter()
        .map(|&cell| {
            let b = &cells.cell_tree().leaves[cell].bounds;
            format!("{} / {}", round(b.min), round(b.max))
        })
        .collect();
    let portals = cells
        .runtime
        .portals
        .iter()
        .filter(|portal| {
            c_cells.contains(&portal.front_leaf) || c_cells.contains(&portal.back_leaf)
        })
        .map(|portal| {
            let mut vertices: Vec<String> = portal.polygon.iter().map(|&v| round(v)).collect();
            vertices.sort();
            vertices.join(", ")
        })
        .collect();
    (bounds, portals)
}

#[test]
fn removing_a_door_between_rooms_a_and_b_leaves_room_c_unchanged() {
    // Pin R7, for a door flush with its wall and one recessed in it.
    let without = Cells::of(&three_rooms(None));
    let classification = classify(&without.map_data.brush_volumes).expect("classifies");
    for wall_piece in [6, 7, 9, 10] {
        assert_eq!(
            classification.verdicts[wall_piece],
            OccluderVerdict::Occludes,
            "precondition: each dividing wall's jambs occlude"
        );
    }
    let c_without = room_c(&without);
    assert!(!c_without.0.is_empty() && !c_without.1.is_empty());

    for door in [FLUSH_DOOR, RECESSED_DOOR] {
        let with = Cells::of(&three_rooms(Some(door)));
        assert_eq!(
            doors(
                &with.map_data.brush_volumes,
                &with.map_data.kinematic_movers
            )
            .expect("doors")
            .len(),
            1
        );
        let sealed = &with.sealed_portals()[0];
        let crossing = portals_inside(&with, &engine_box(door.0, door.1));
        assert!(!crossing.is_empty(), "runtime portals cross the door");
        assert!(
            crossing.iter().all(|portal| sealed.contains(portal)),
            "the closed door seals every portal across it"
        );
        if door == RECESSED_DOOR {
            assert!(
                with.runtime.cells.interior_cell_count()
                    > without.runtime.cells.interior_cell_count(),
                "the recessed door's planes split its doorway"
            );
        }

        assert_eq!(room_c(&with), c_without, "room C is unchanged by the door");

        // C is visible from A through B with the door open, hidden with it
        // closed.
        let world = with.visibility_world();
        let eye = [100.0, 890.0, 60.0];
        let c_cell = with.cell_at([1000.0, 890.0, 60.0]) as u32;
        let (open, _) = walk_blocked(&world, eye, EAST, &[]);
        assert!(open.contains(&c_cell));
        let (closed, _) = walk_blocked(&world, eye, EAST, sealed);
        assert!(!closed.contains(&c_cell));
    }
}

// ---- a door deeper than it is tall (C10) ----

/// A hall 1200 × 1000 × 256 u crossed by a wall 256 u (6.5 m) thick with a
/// doorway 256 u wide and 192 u tall, filled by a door the warren generator's
/// shape: as deep as the wall, so its thinnest axis is its height.
fn thick_wall_door() -> MapText {
    let mut map = MapText::new();
    map.hall([1200.0, 1000.0, 256.0])
        .solid([500.0, 0.0, 0.0], [756.0, 372.0, 256.0])
        .solid([500.0, 628.0, 0.0], [756.0, 1000.0, 256.0])
        .solid([500.0, 372.0, 192.0], [756.0, 628.0, 256.0])
        .mover("door", &[THICK_DOOR], 192.0);
    map
}

const THICK_DOOR: ([f64; 3], [f64; 3]) = ([500.0, 372.0, 0.0], [756.0, 628.0, 192.0]);

#[test]
fn door_deeper_than_tall_is_recognised_by_the_faces_toward_its_rooms() {
    let cells = Cells::of(&thick_wall_door());
    let recognised = doors(
        &cells.map_data.brush_volumes,
        &cells.map_data.kinematic_movers,
    )
    .expect("doors");
    assert_eq!(
        recognised.len(),
        1,
        "the block filling the doorway is a door"
    );
    let along_wall = (engine([1.0, 0.0, 0.0]) - engine([0.0; 3])).normalize();
    for &(normal, _) in &recognised[0].broad_planes {
        assert!(
            normal.dot(along_wall).abs() > 0.999,
            "its broad faces front the rooms, not the floor and lintel"
        );
    }

    let sealed = &cells.sealed_portals()[0];
    let crossing = portals_inside(&cells, &engine_box(THICK_DOOR.0, THICK_DOOR.1));
    assert!(!crossing.is_empty());
    assert!(crossing.iter().all(|portal| sealed.contains(portal)));

    let world = cells.visibility_world();
    let eye = [250.0, 500.0, 60.0];
    let far = cells.cell_at([1000.0, 500.0, 60.0]) as u32;
    let (open, _) = walk_blocked(&world, eye, EAST, &[]);
    assert!(open.contains(&far), "open, the doorway passes");
    let (closed, _) = walk_blocked(&world, eye, EAST, sealed);
    assert!(!closed.contains(&far), "closed, the doorway blocks");
}

// ---- a docked lift car (C13) ----

/// A car 160 u (4 m) square and 128 u (3.25 m) tall, floor, three walls and
/// a roof, docked on the floor of a hall 640 × 640 × 256 u at its centre.
const LIFT_CAR: [([f64; 3], [f64; 3]); 5] = [
    ([240.0, 240.0, 0.0], [400.0, 400.0, 8.0]),
    ([240.0, 240.0, 8.0], [248.0, 400.0, 120.0]),
    ([392.0, 240.0, 8.0], [400.0, 400.0, 120.0]),
    ([248.0, 392.0, 8.0], [392.0, 400.0, 120.0]),
    ([240.0, 240.0, 120.0], [400.0, 400.0, 128.0]),
];

/// A flat platform docked on the floor, 16 u (0.4 m) thick.
const PLATFORM: [([f64; 3], [f64; 3]); 1] = [([240.0, 240.0, 0.0], [400.0, 400.0, 16.0])];

/// A crate standing on the floor through the platform's edge, as the warren
/// generator can place one: it meets the platform's top from beyond, but
/// runs through the platform to the floor.
const CRATE: ([f64; 3], [f64; 3]) = ([380.0, 300.0, 0.0], [440.0, 360.0, 60.0]);

fn lift_room(car: &[([f64; 3], [f64; 3])], props: &[([f64; 3], [f64; 3])]) -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 256.0]);
    for &(min, max) in props {
        map.solid(min, max);
    }
    if !car.is_empty() {
        map.mover("lift", car, 128.0);
    }
    map
}

#[test]
fn lift_car_docked_in_a_room_leaves_its_cells_and_portals_unchanged() {
    // Pin R25: a car touching only the floor seals no doorway, so it feeds
    // no splitter. A cabin, a flat platform, and the platform with a crate
    // standing through it.
    for (car, props) in [
        (&LIFT_CAR[..], &[][..]),
        (&PLATFORM[..], &[][..]),
        (&PLATFORM[..], &[CRATE][..]),
    ] {
        let bare = Cells::of(&lift_room(&[], props));
        let map_data = lift_room(car, props).parse();
        assert_eq!(map_data.kinematic_movers.len(), 1);
        assert!(
            doors(&map_data.brush_volumes, &map_data.kinematic_movers)
                .expect("doors")
                .is_empty(),
            "a docked lift car is no door"
        );
        let cells = Cells::of_map_data(map_data);
        let view = &cells.runtime.cells.tree;
        let bare_view = &bare.runtime.cells.tree;
        assert_eq!(view.leaves.len(), bare_view.leaves.len());
        for (leaf, bare_leaf) in view.leaves.iter().zip(&bare_view.leaves) {
            assert_eq!(leaf.bounds.min, bare_leaf.bounds.min);
            assert_eq!(leaf.bounds.max, bare_leaf.bounds.max);
            assert_eq!(leaf.defining_planes, bare_leaf.defining_planes);
        }
        assert_eq!(cells.runtime.portals.len(), bare.runtime.portals.len());
        for (portal, bare_portal) in cells.runtime.portals.iter().zip(&bare.runtime.portals) {
            assert_eq!(
                (portal.front_leaf, portal.back_leaf),
                (bare_portal.front_leaf, bare_portal.back_leaf)
            );
            assert_eq!(portal.polygon, bare_portal.polygon);
        }
    }
}
