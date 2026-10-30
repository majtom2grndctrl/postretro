// Door rows: doors recognised before the occluder-tier build, their splitters
// bounding the doorway, and sealing read against the runtime portals.
// See: context/plans/done/occluder-first-partition/index.md §Acceptance

use std::collections::BTreeSet;

use glam::DVec3;

use super::test_fixtures::{Cells, MapText, engine, quake_triple, walk_blocked};
use super::{classify, doors};
use crate::map_data::MapData;
use crate::map_format::MapFormat;
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

// ---- a door turned 45° in plan ----

/// The exempt wall's turn in plan, about the door's vertical axis.
const TURN: f64 = std::f64::consts::FRAC_PI_4;

/// A Quake-unit point turned by `TURN` about the vertical axis through
/// `centre`.
fn turned_about(centre: [f64; 3], q: [f64; 3]) -> [f64; 3] {
    let (sin, cos) = TURN.sin_cos();
    let (x, y) = (q[0] - centre[0], q[1] - centre[1]);
    [
        centre[0] + cos * x - sin * y,
        centre[1] + sin * x + cos * y,
        q[2],
    ]
}

/// A Quake-unit point turned by `TURN` about the vertical axis through
/// `DOOR_CENTRE`.
fn turned(q: [f64; 3]) -> [f64; 3] {
    turned_about(DOOR_CENTRE, q)
}

/// The `.map` text of the box `min`..`max` turned by `turned`.
fn turned_box_brush(min: [f64; 3], max: [f64; 3]) -> String {
    turned_brush(DOOR_CENTRE, min, max, None)
}

/// The `.map` text of the box `min`..`max` turned by `TURN` about `centre`.
/// Each side's three points are two corners of its bottom edge and the first
/// corner lifted, so its normal is exactly horizontal. With `broad_span`, the
/// two sides across y take their points at those x and at heights 0 and 1
/// instead, so every box authored with one span shares those two planes bit
/// for bit.
fn turned_brush(
    centre: [f64; 3],
    min: [f64; 3],
    max: [f64; 3],
    broad_span: Option<[f64; 2]>,
) -> String {
    const TEXTURE: &str = "debug_wall_grey";
    let (z0, z1) = (min[2], max[2]);
    // Counter-clockwise in plan, so each side faces out to the right of its
    // edge, as the map format winds a plane's points.
    let plan = [
        [min[0], min[1]],
        [max[0], min[1]],
        [max[0], max[1]],
        [min[0], max[1]],
    ];
    let mut faces: Vec<String> = (0..4)
        .map(|i| {
            let (mut a, mut b) = (plan[i], plan[(i + 1) % 4]);
            let (mut low, mut high) = (z0, z1);
            // Sides 0 and 2 run along x; side 2 runs back.
            if let Some([x0, x1]) = broad_span
                && i % 2 == 0
            {
                (a[0], b[0]) = if i == 0 { (x0, x1) } else { (x1, x0) };
                (low, high) = (0.0, 1.0);
            }
            let [ax, ay, _] = turned_about(centre, [a[0], a[1], 0.0]);
            let [bx, by, _] = turned_about(centre, [b[0], b[1], 0.0]);
            format!(
                "( {ax} {ay} {low} ) ( {ax} {ay} {high} ) ( {bx} {by} {low} ) {TEXTURE} 0 0 0 1 1"
            )
        })
        .collect();
    faces.push(format!(
        "( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1"
    ));
    faces.push(format!(
        "( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1"
    ));
    format!("{{\n{}\n}}", faces.join("\n"))
}

/// `exempt_wall(true)` with its pieces and door turned by `TURN`, parsed as
/// `prl-build` parses it. The hall stays square to the axes.
fn turned_exempt_wall() -> MapData {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 128.0]);
    let pieces: Vec<String> = PIECES
        .iter()
        .map(|&(min, max)| turned_box_brush(min, max))
        .collect();
    let door = turned_box_brush(DOOR.0, DOOR.1);
    parse_with_turned_door(&map, &pieces, &door, DOOR_CENTRE, 112.0)
}

/// `map`, a hall alone, with the brush texts `pieces` closing its
/// worldspawn and a door of the brush text `door` authored closed where it
/// stands, its centre `centre`, sliding `rise` units upward to open, parsed
/// as `prl-build` parses it.
fn parse_with_turned_door(
    map: &MapText,
    pieces: &[String],
    door: &str,
    centre: [f64; 3],
    rise: f64,
) -> MapData {
    // The pieces close worldspawn, after the hall, as `exempt_wall` orders
    // its brushes.
    let world = map.text();
    let world = world
        .strip_suffix("}\n")
        .expect("the text is worldspawn alone");
    let mut text = format!("{world}{}\n}}\n", pieces.join("\n"));
    // The door slides open upward from where it stands, as `MapText::mover`
    // authors one.
    let open = [centre[0], centre[1], centre[2] + rise];
    text.push_str(&format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"door\"\n\"path\" \"door_closed\"\n\
         \"speed\" \"4\"\n\"move_mode\" \"once\"\n\"start_on_spawn\" \"0\"\n{}\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"door_closed\"\n\
         \"next\" \"door_open\"\n\"origin\" \"{}\"\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"door_open\"\n\
         \"origin\" \"{}\"\n}}\n",
        door,
        quake_triple(centre),
        quake_triple(open),
    ));

    let dir = tempfile::tempdir().expect("temporary map directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create fixture maps directory");
    let path = maps.join("fixture.map");
    std::fs::write(&path, text).expect("write fixture map");
    crate::parse::parse_map_file(&path, MapFormat::IdTech2).expect("fixture map parses")
}

#[test]
fn door_turned_45_degrees_in_plan_seals_its_opening_closed_and_passes_open() {
    // Oblique door planes: clipping leaves the portal corners on the door's
    // faces and edges only to within rounding, and sealing certifies cover
    // with no tolerance.
    let map_data = turned_exempt_wall();
    let classification = classify(&map_data.brush_volumes).expect("classifies");
    for piece in [
        classification.verdicts.len() - 2,
        classification.verdicts.len() - 1,
    ] {
        assert_ne!(
            classification.verdicts[piece],
            OccluderVerdict::Occludes,
            "precondition: the turned wall pieces stay exempt, so the door's own planes bound \
             its cell"
        );
    }
    let recognised = doors(&map_data.brush_volumes, &map_data.kinematic_movers).expect("doors");
    assert_eq!(recognised.len(), 1, "the turned mover fills a doorway");

    let cells = Cells::of_map_data(map_data);
    let (front, back) = (turned(FRONT), turned(BACK));
    assert_ne!(
        cells.cell_at(front),
        cells.cell_at(back),
        "closed, the door puts its two sides in different cells"
    );
    let door_cell = cells.cell_at(DOOR_CENTRE);
    let onto_door: Vec<u32> = cells
        .runtime
        .portals
        .iter()
        .enumerate()
        .filter(|(_, portal)| portal.front_leaf == door_cell || portal.back_leaf == door_cell)
        .map(|(index, _)| index as u32)
        .collect();
    assert!(!onto_door.is_empty(), "runtime portals lead onto the door");
    let sealed = &cells.sealed_portals()[0];
    for portal in &onto_door {
        assert!(
            sealed.contains(portal),
            "portal {portal} onto the door's cell is not sealed by the closed door \
             (sealed {sealed:?})"
        );
    }

    let world = cells.visibility_world();
    let ahead: [f64; 3] = std::array::from_fn(|axis| back[axis] - front[axis]);
    let (open, _) = walk_blocked(&world, front, ahead, &[]);
    assert!(
        open.contains(&(door_cell as u32)),
        "open, the doorway passes"
    );
    let (closed, _) = walk_blocked(&world, front, ahead, sealed);
    assert!(
        !closed.contains(&(door_cell as u32)),
        "closed, the doorway blocks"
    );
}

// ---- a door flush in an occluding wall turned 45° in plan ----

/// A hall 1400 u (35.6 m) square and 160 u (4.1 m) tall, cut corner to
/// corner by a wall 16 u (0.4 m) thick. Before the turn the wall runs along x
/// through `DIAGONAL_CENTRE`, half the hall's diagonal (990 u) and 8 u more
/// each way, so each end, turned, lies inside the hall's corner slabs.
const DIAGONAL_HALL: [f64; 3] = [1400.0, 1400.0, 160.0];
const DIAGONAL_CENTRE: [f64; 3] = [700.0, 700.0, 50.0];
/// Two jambs over 20 m long, each occluding on the absolute length, and an
/// exempt lintel over a doorway 60 u wide and 100 u tall, unturned.
const DIAGONAL_WALL: [([f64; 3], [f64; 3]); 3] = [
    ([-298.0, 692.0, 0.0], [670.0, 708.0, 160.0]),
    ([730.0, 692.0, 0.0], [1698.0, 708.0, 160.0]),
    ([670.0, 692.0, 100.0], [730.0, 708.0, 160.0]),
];
/// The door, unturned: flush with both faces of the wall, and running 4 u
/// into each jamb, as a door hung in its frame does, so the doorway's sides
/// lie inside it.
const DIAGONAL_DOOR: ([f64; 3], [f64; 3]) = ([666.0, 692.0, 0.0], [734.0, 708.0, 100.0]);
/// The door exactly filling the doorway, unturned: its edges lie on the
/// jambs' ends, which bound the doorway, up to parse noise.
const DIAGONAL_DOOR_BETWEEN_JAMBS: ([f64; 3], [f64; 3]) =
    ([670.0, 692.0, 0.0], [730.0, 708.0, 100.0]);
/// Points 160 u (4 m) in front of and behind the door, unturned.
const DIAGONAL_FRONT: [f64; 3] = [700.0, 540.0, 60.0];
const DIAGONAL_BACK: [f64; 3] = [700.0, 860.0, 60.0];

/// The diagonal hall with its wall and `door` turned by `TURN` about
/// `DIAGONAL_CENTRE`. With `shared_faces`, every wall piece and the door
/// author their two broad faces through the same points, so the door's faces
/// are the wall's planes bit for bit; without, each brush authors them
/// through its own corners, and the parser's f32 planes put the door's faces
/// off the wall's by parse noise.
fn turned_occluding_wall(door: ([f64; 3], [f64; 3]), shared_faces: bool) -> MapData {
    let span = shared_faces.then_some([door.0[0], door.1[0]]);
    let mut map = MapText::new();
    map.hall(DIAGONAL_HALL);
    let pieces: Vec<String> = DIAGONAL_WALL
        .iter()
        .map(|&(min, max)| turned_brush(DIAGONAL_CENTRE, min, max, span))
        .collect();
    let door = turned_brush(DIAGONAL_CENTRE, door.0, door.1, span);
    parse_with_turned_door(&map, &pieces, &door, DIAGONAL_CENTRE, 96.0)
}

#[test]
fn door_flush_in_an_occluding_wall_turned_45_degrees_seals_closed_and_passes_open() {
    // The wall's faces split at occluder rank, so the portals onto the door
    // lie on a wall face, its outward plane, never on the door's own inward
    // plane, and the door's face matches it only up to rounding or parse
    // noise. Sealing must read the portal as lying on the door's face
    // whichever side of it the corners fell.
    assert_turned_occluding_wall_door_seals_closed_and_passes_open(DIAGONAL_DOOR);
}

#[test]
fn door_between_occluding_jambs_turned_45_degrees_seals_closed_and_passes_open() {
    // The jambs' ends split at occluder rank, so they bound the doorway and
    // its portals, and the door's edges lie on them only up to parse noise:
    // a door edge a few micrometres inside a jamb's end leaves a sliver of
    // portal past it, which sealing must read as noise, not a slot.
    assert_turned_occluding_wall_door_seals_closed_and_passes_open(DIAGONAL_DOOR_BETWEEN_JAMBS);
}

/// `door` in the turned occluding wall, with its broad faces shared with the
/// wall's and authored through its own corners: recognised, its doorway a
/// cell of its own, every portal onto that cell sealed, and the walk through
/// it passing open and blocked closed.
fn assert_turned_occluding_wall_door_seals_closed_and_passes_open(door: ([f64; 3], [f64; 3])) {
    let turn = |q: [f64; 3]| turned_about(DIAGONAL_CENTRE, q);
    for shared_faces in [true, false] {
        let map_data = turned_occluding_wall(door, shared_faces);
        let classification = classify(&map_data.brush_volumes).expect("classifies");
        // The hall's six slabs come first, then the two jambs.
        for jamb in [6, 7] {
            assert_eq!(
                classification.verdicts[jamb],
                OccluderVerdict::Occludes,
                "precondition: each turned jamb occludes, so the wall's own faces bound the \
                 doorway (shared faces {shared_faces})"
            );
        }
        let recognised = doors(&map_data.brush_volumes, &map_data.kinematic_movers).expect("doors");
        assert_eq!(
            recognised.len(),
            1,
            "the turned mover fills its doorway (shared faces {shared_faces})"
        );

        let cells = Cells::of_map_data(map_data);
        let (front, back) = (turn(DIAGONAL_FRONT), turn(DIAGONAL_BACK));
        let door_cell = cells.cell_at(DIAGONAL_CENTRE);
        assert!(
            cells.is_interior_cell(door_cell),
            "the doorway is an interior cell"
        );
        assert_ne!(cells.cell_at(front), door_cell);
        assert_ne!(cells.cell_at(back), door_cell);
        assert_ne!(
            cells.cell_at(front),
            cells.cell_at(back),
            "the wall puts its two sides in different cells"
        );

        let onto_door: Vec<u32> = cells
            .runtime
            .portals
            .iter()
            .enumerate()
            .filter(|(_, portal)| portal.front_leaf == door_cell || portal.back_leaf == door_cell)
            .map(|(index, _)| index as u32)
            .collect();
        assert!(!onto_door.is_empty(), "runtime portals lead onto the door");
        let sealed = &cells.sealed_portals()[0];
        for portal in &onto_door {
            assert!(
                sealed.contains(portal),
                "portal {portal} onto the door's cell is not sealed by the closed door \
                 (sealed {sealed:?}, shared faces {shared_faces})"
            );
        }

        let world = cells.visibility_world();
        let back_cell = cells.cell_at(back) as u32;
        let ahead: [f64; 3] = std::array::from_fn(|axis| back[axis] - front[axis]);
        let (open, _) = walk_blocked(&world, front, ahead, &[]);
        assert!(
            open.contains(&(door_cell as u32)) && open.contains(&back_cell),
            "open, the doorway passes (shared faces {shared_faces})"
        );
        let (closed, _) = walk_blocked(&world, front, ahead, sealed);
        assert!(
            !closed.contains(&(door_cell as u32)) && !closed.contains(&back_cell),
            "closed, the doorway blocks (shared faces {shared_faces})"
        );
    }
}

// ---- a door into a closet shallower than the floor ----

/// A wall across a hall 640 × 640 × 128 u, 24 u (0.6 m) short of its far
/// wall: two pieces from the side walls to a doorway 60 u wide. Each piece's
/// back face fronts only the closet and its front covers under half the
/// span across the hall, so the wall stays exempt.
const CLOSET_PIECES: [([f64; 3], [f64; 3]); 2] = [
    ([0.0, 608.0, 0.0], [290.0, 616.0, 128.0]),
    ([350.0, 608.0, 0.0], [640.0, 616.0, 128.0]),
];
const CLOSET_DOOR: ([f64; 3], [f64; 3]) = ([290.0, 608.0, 0.0], [350.0, 616.0, 128.0]);
const IN_CLOSET: [f64; 3] = [320.0, 628.0, 64.0];
const BEFORE_CLOSET: [f64; 3] = [320.0, 300.0, 60.0];

fn closet() -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 128.0]);
    for (min, max) in CLOSET_PIECES {
        map.solid(min, max);
    }
    map.mover("door", &[CLOSET_DOOR], 112.0);
    map
}

#[test]
fn door_into_a_closet_shallower_than_the_floor_seals_it_closed_and_passes_open() {
    // One broad face fronts the room, the other only the closet. Without
    // the door the exempt wall bounds no cell, so the door's own planes must
    // part the closet from the room.
    let map_data = closet().parse();
    let classification = classify(&map_data.brush_volumes).expect("classifies");
    for piece in [
        classification.verdicts.len() - 2,
        classification.verdicts.len() - 1,
    ] {
        assert_ne!(
            classification.verdicts[piece],
            OccluderVerdict::Occludes,
            "precondition: the closet's wall stays exempt"
        );
    }
    let recognised = doors(&map_data.brush_volumes, &map_data.kinematic_movers).expect("doors");
    assert_eq!(
        recognised.len(),
        1,
        "the mover fills the closet's doorway, though the closet is shallower than the floor"
    );

    let cells = Cells::of_map_data(map_data);
    let closet_cell = cells.cell_at(IN_CLOSET);
    assert_ne!(
        cells.cell_at(BEFORE_CLOSET),
        closet_cell,
        "closed, the door parts the closet from the room"
    );
    let sealed = &cells.sealed_portals()[0];
    let crossing = portals_inside(&cells, &engine_box(CLOSET_DOOR.0, CLOSET_DOOR.1));
    assert!(!crossing.is_empty(), "runtime portals cross the door");
    assert!(
        crossing.iter().all(|portal| sealed.contains(portal)),
        "the closed door seals every portal across it (sealed {sealed:?}, crossing {crossing:?})"
    );

    let world = cells.visibility_world();
    let (open, _) = walk_blocked(&world, BEFORE_CLOSET, NORTH, &[]);
    assert!(
        open.contains(&(closet_cell as u32)),
        "open, the closet shows"
    );
    let (closed, _) = walk_blocked(&world, BEFORE_CLOSET, NORTH, sealed);
    assert!(
        !closed.contains(&(closet_cell as u32)),
        "closed, the closet is hidden"
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

/// `FLUSH_DOOR` moved 0.5 mm into room A: proud of the wall face the
/// dividing wall's tier boundary lies on, by less than the tier's spanning
/// tolerance.
fn proud_door() -> ([f64; 3], [f64; 3]) {
    let shift = 0.5e-3 / MapFormat::IdTech2.units_to_meters();
    let (min, max) = FLUSH_DOOR;
    (
        [min[0] - shift, min[1], min[2]],
        [max[0] - shift, max[1], max[2]],
    )
}

#[test]
fn door_proud_of_its_wall_under_the_spanning_tolerance_cuts_no_room_and_still_seals() {
    // Room A meets the proud door, but no broad plane of the door spans it,
    // so the door must not hold it: its edge planes would cut room A at the
    // door's outline.
    let flush = Cells::of(&three_rooms(Some(FLUSH_DOOR)));
    let proud = Cells::of(&three_rooms(Some(proud_door())));
    assert_eq!(
        doors(
            &proud.map_data.brush_volumes,
            &proud.map_data.kinematic_movers
        )
        .expect("doors")
        .len(),
        1
    );
    assert_eq!(
        proud.cell_tree().leaves.len(),
        flush.cell_tree().leaves.len(),
        "the proud door adds no cell to the room beside it"
    );
    assert_eq!(
        proud.runtime.cells.interior_cell_count(),
        flush.runtime.cells.interior_cell_count()
    );
    assert_eq!(
        proud.runtime.portals.len(),
        flush.runtime.portals.len(),
        "the proud door adds no portal to the room beside it"
    );

    // Room B is visible from A with the door open, hidden with it closed.
    let sealed = &proud.sealed_portals()[0];
    assert!(!sealed.is_empty(), "the closed door seals a portal");
    let world = proud.visibility_world();
    let eye = [100.0, 890.0, 60.0];
    let b_cell = proud.cell_at([600.0, 890.0, 60.0]) as u32;
    let (open, _) = walk_blocked(&world, eye, EAST, &[]);
    assert!(open.contains(&b_cell));
    let (closed, _) = walk_blocked(&world, eye, EAST, sealed);
    assert!(!closed.contains(&b_cell));
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
