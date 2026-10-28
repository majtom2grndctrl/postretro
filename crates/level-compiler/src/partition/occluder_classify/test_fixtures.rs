// Synthetic brush sets for the occluder classifier and the occluder tier: box
// brushes, sealed halls, and each acceptance fixture with the verdicts and
// cells it pins.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use glam::{DQuat, DVec3};

use super::{
    OCCLUDER_ABSOLUTE_LENGTH_M, OCCLUDER_FLOOR_M, OCCLUDER_SEE_OVER_M, OCCLUDER_SPAN_FRACTION,
    OccluderVerdict,
};
use crate::map_data::{BrushPlane, BrushVolume};
use crate::partition::Aabb;
use crate::partition::region_polytope::RegionPolytope;

/// Slab thickness of the sealed halls around each fixture.
const HALL_SLAB_M: f64 = 0.5;

/// Slab thickness of the rooms built edge to edge (pins R30, R31).
const ROOM_SLAB_M: f64 = 0.3;

/// Step either side of a constant in the rows that decide each one both ways.
const EDGE_STEP_M: f64 = 1e-3;

// ---- brushes ----

/// Axis-aligned box brush with exact planes.
pub(crate) fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    let planes = [
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ]
    .into_iter()
    .map(|(normal, distance)| BrushPlane { normal, distance })
    .collect();
    BrushVolume {
        planes,
        sides: Vec::new(),
        aabb: Aabb { min, max },
    }
}

/// Box brush in its own frame: `half` extents about `center`, turned `yaw`
/// radians about engine up.
pub(crate) fn turned_box(center: DVec3, half: DVec3, yaw: f64) -> BrushVolume {
    let turned = turned_about_up(&box_brush(-half, half), yaw);
    let planes = turned
        .planes
        .iter()
        .map(|plane| BrushPlane {
            normal: plane.normal,
            distance: plane.distance + center.dot(plane.normal),
        })
        .collect();
    brush_from_planes(planes, center, half.length())
}

/// `brush` turned `yaw` radians about engine up through the world origin.
pub(crate) fn turned_about_up(brush: &BrushVolume, yaw: f64) -> BrushVolume {
    let turn = DQuat::from_rotation_y(yaw);
    let planes = brush
        .planes
        .iter()
        .map(|plane| BrushPlane {
            normal: turn * plane.normal,
            distance: plane.distance,
        })
        .collect();
    let center = turn * ((brush.aabb.min + brush.aabb.max) * 0.5);
    let reach = (brush.aabb.max - brush.aabb.min).length() * 0.5;
    brush_from_planes(planes, center, reach)
}

/// Brush bounded by `planes`, its AABB cut from a box `reach` (plus a metre)
/// about `center` that encloses it.
fn brush_from_planes(planes: Vec<BrushPlane>, center: DVec3, reach: f64) -> BrushVolume {
    let mut polytope = RegionPolytope::from_aabb(&Aabb {
        min: center - DVec3::splat(reach + 1.0),
        max: center + DVec3::splat(reach + 1.0),
    });
    for plane in &planes {
        polytope = polytope.clip(plane.normal, plane.distance).1;
    }
    BrushVolume {
        planes,
        sides: Vec::new(),
        aabb: polytope.vertex_aabb(),
    }
}

/// Index of the floor in `hall`'s brushes; the other slabs follow it as
/// ceiling, then the -x, +x, -z and +z walls.
pub(crate) const HALL_FLOOR: usize = 0;

/// Sealed hall of 0.5 m slabs overlapping at the corners: interior
/// `[0, size]`, floor top at y = 0.
pub(crate) fn hall(size: DVec3) -> Vec<BrushVolume> {
    let t = HALL_SLAB_M;
    let s = size;
    vec![
        box_brush(DVec3::new(-t, -t, -t), DVec3::new(s.x + t, 0.0, s.z + t)),
        box_brush(
            DVec3::new(-t, s.y, -t),
            DVec3::new(s.x + t, s.y + t, s.z + t),
        ),
        box_brush(DVec3::new(-t, 0.0, -t), DVec3::new(0.0, s.y, s.z + t)),
        box_brush(DVec3::new(s.x, 0.0, -t), DVec3::new(s.x + t, s.y, s.z + t)),
        box_brush(DVec3::new(0.0, 0.0, -t), DVec3::new(s.x, s.y, 0.0)),
        box_brush(DVec3::new(0.0, 0.0, s.z), DVec3::new(s.x, s.y, s.z + t)),
    ]
}

// ---- fixtures ----

/// A brush a fixture pins to a verdict.
pub(crate) struct Pin {
    pub(crate) label: &'static str,
    pub(crate) brush: usize,
    pub(crate) verdict: OccluderVerdict,
}

/// A classification fixture: its brushes, the verdicts it pins, and pairs of
/// interior points that must locate to different cells.
pub(crate) struct Fixture {
    pub(crate) name: String,
    pub(crate) brushes: Vec<BrushVolume>,
    pub(crate) pins: Vec<Pin>,
    pub(crate) apart: Vec<(DVec3, DVec3)>,
}

impl Fixture {
    pub(crate) fn new(name: impl Into<String>, brushes: Vec<BrushVolume>) -> Self {
        Self {
            name: name.into(),
            brushes,
            pins: Vec::new(),
            apart: Vec::new(),
        }
    }

    /// A fixture standing in a sealed hall whose floor, an occluder, it pins.
    // Regression: a single centre probe landed in the brush standing on the
    // floor's centre, so the floor read as fronting no open space.
    pub(crate) fn in_hall(name: impl Into<String>, size: DVec3) -> Self {
        let mut fixture = Self::new(name, hall(size));
        fixture.pin("floor", HALL_FLOOR, OccluderVerdict::Occludes);
        fixture
    }

    pub(crate) fn pin(&mut self, label: &'static str, brush: usize, verdict: OccluderVerdict) {
        self.pins.push(Pin {
            label,
            brush,
            verdict,
        });
    }

    pub(crate) fn add_pinned(
        &mut self,
        label: &'static str,
        brush: BrushVolume,
        verdict: OccluderVerdict,
    ) -> usize {
        self.brushes.push(brush);
        let index = self.brushes.len() - 1;
        self.pin(label, index, verdict);
        index
    }
}

/// Every classification fixture, for the order-invariance rows.
pub(crate) fn classification_fixtures() -> Vec<Fixture> {
    let mut fixtures = vec![
        retaining_wall(),
        stair_platform(),
        floor_to_ceiling_column(),
        column_in_niche(),
        columns_down_corridor(),
        curb(),
        lamp_post(),
        street_deck(),
        red_pillar(),
        two_sealed_rooms(),
        room_in_hall(),
        open_air_maze(),
        turned_curb(),
        turned_retaining_wall(),
        cube_in_room(),
        chain_walls(),
    ];
    for edge in Edge::ALL {
        fixtures.extend(constant_edges(edge));
    }
    fixtures
}

// Classification table (Acceptance §Automated). Each brush stands on its
// hall's occluding floor, except the street deck, which is raised.

pub(crate) fn retaining_wall() -> Fixture {
    let mut fixture = Fixture::in_hall("retaining wall", DVec3::new(52.0, 13.0, 60.0));
    fixture.add_pinned(
        "retaining wall",
        box_brush(DVec3::new(13.0, 0.0, 30.0), DVec3::new(39.0, 6.5, 30.81)),
        OccluderVerdict::Occludes,
    );
    fixture
}

/// A shaft platform (`emit_shaft_stairs`: 112 u square, 24 u thick, in a
/// 384 u shaft).
pub(crate) fn stair_platform() -> Fixture {
    let mut fixture = Fixture::in_hall("stair platform", DVec3::new(9.75, 20.0, 9.75));
    let (lo, hi) = ((9.75 - 2.84) * 0.5, (9.75 + 2.84) * 0.5);
    fixture.add_pinned(
        "stair platform",
        box_brush(DVec3::new(lo, 0.0, lo), DVec3::new(hi, 0.61, hi)),
        OccluderVerdict::ShortSpan,
    );
    fixture
}

pub(crate) fn floor_to_ceiling_column() -> Fixture {
    let mut fixture = Fixture::in_hall("floor-to-ceiling column", DVec3::new(13.0, 8.0, 30.0));
    fixture.add_pinned(
        "column",
        box_brush(DVec3::new(5.75, 0.0, 14.25), DVec3::new(7.25, 8.0, 15.75)),
        OccluderVerdict::ShortSpan,
    );
    fixture
}

/// The hall's -z wall, 13 m long, holds a niche 2 m wide and 1.5 m deep;
/// the column fills its depth, flush with the wall, 0.25 m from each side.
pub(crate) fn column_in_niche() -> Fixture {
    let (width, height, length) = (13.0, 6.0, 20.0);
    let mut brushes = hall(DVec3::new(width, height, length));
    brushes.remove(4);
    let (t, depth) = (HALL_SLAB_M, 1.5);
    let (niche_lo, niche_hi) = (5.5, 7.5);
    brushes.extend([
        box_brush(
            DVec3::new(0.0, 0.0, -depth),
            DVec3::new(niche_lo, height, 0.0),
        ),
        box_brush(
            DVec3::new(niche_hi, 0.0, -depth),
            DVec3::new(width, height, 0.0),
        ),
        box_brush(
            DVec3::new(niche_lo, 0.0, -depth - t),
            DVec3::new(niche_hi, height, -depth),
        ),
        box_brush(
            DVec3::new(niche_lo, -t, -depth),
            DVec3::new(niche_hi, 0.0, -t),
        ),
        box_brush(
            DVec3::new(niche_lo, height, -depth),
            DVec3::new(niche_hi, height + t, -t),
        ),
    ]);
    let mut fixture = Fixture::new("column in a niche", brushes);
    fixture.pin("floor", HALL_FLOOR, OccluderVerdict::Occludes);
    fixture.add_pinned(
        "column",
        box_brush(
            DVec3::new(niche_lo + 0.25, 0.0, -depth),
            DVec3::new(niche_hi - 0.25, height, 0.0),
        ),
        OccluderVerdict::ShortSpan,
    );
    fixture
}

pub(crate) fn columns_down_corridor() -> Fixture {
    let mut fixture = Fixture::in_hall("columns down a corridor", DVec3::new(6.0, 4.0, 30.0));
    for z in [3.0, 9.0, 15.0, 21.0, 27.0] {
        fixture.add_pinned(
            "corridor column",
            box_brush(
                DVec3::new(2.25, 0.0, z - 0.75),
                DVec3::new(3.75, 4.0, z + 0.75),
            ),
            OccluderVerdict::ShortSpan,
        );
    }
    fixture
}

pub(crate) fn curb() -> Fixture {
    let mut fixture = Fixture::in_hall("curb", DVec3::new(30.0, 6.0, 30.0));
    fixture.add_pinned(
        "curb",
        box_brush(DVec3::new(2.5, 0.0, 15.0), DVec3::new(27.5, 0.3, 15.3)),
        OccluderVerdict::UnderFloor,
    );
    fixture
}

pub(crate) fn lamp_post() -> Fixture {
    let mut fixture = Fixture::in_hall("lamp post", DVec3::new(20.0, 8.0, 20.0));
    fixture.add_pinned(
        "lamp post",
        box_brush(DVec3::new(9.9, 0.0, 9.9), DVec3::new(10.1, 5.0, 10.1)),
        OccluderVerdict::UnderFloor,
    );
    fixture
}

/// A deck 30 m long and 0.5 m thick spanning a corridor 4 m wide, its
/// underside 5 m above the corridor floor.
pub(crate) fn street_deck() -> Fixture {
    let mut fixture = Fixture::in_hall("street deck", DVec3::new(4.0, 13.0, 60.0));
    fixture.add_pinned(
        "street deck",
        box_brush(DVec3::new(0.0, 5.0, 15.0), DVec3::new(4.0, 5.5, 45.0)),
        OccluderVerdict::Occludes,
    );
    fixture
}

pub(crate) fn red_pillar() -> Fixture {
    let mut fixture = Fixture::in_hall("red pillar", DVec3::new(20.0, 6.0, 20.0));
    fixture.add_pinned(
        "red pillar",
        box_brush(DVec3::new(9.5, 0.0, 9.5), DVec3::new(10.5, 3.7, 10.5)),
        OccluderVerdict::UnderFloor,
    );
    fixture
}

// Cell rows.

/// Two rooms 8 m square and 4 m tall sharing an 8 m wall, every 0.3 m slab
/// covering only its room's face, so slabs meet edge to edge and each wall's
/// own line runs past its ends into the void (pin R30).
pub(crate) fn two_sealed_rooms() -> Fixture {
    let (t, side, height) = (ROOM_SLAB_M, 8.0, 4.0);
    let mut fixture = Fixture::new("two sealed rooms", Vec::new());
    for (x0, x1) in [(0.0, side), (side + t, 2.0 * side + t)] {
        fixture.add_pinned(
            "floor",
            box_brush(DVec3::new(x0, -t, 0.0), DVec3::new(x1, 0.0, side)),
            OccluderVerdict::Occludes,
        );
        fixture.add_pinned(
            "ceiling",
            box_brush(
                DVec3::new(x0, height, 0.0),
                DVec3::new(x1, height + t, side),
            ),
            OccluderVerdict::Occludes,
        );
        fixture.add_pinned(
            "south wall",
            box_brush(DVec3::new(x0, 0.0, -t), DVec3::new(x1, height, 0.0)),
            OccluderVerdict::Occludes,
        );
        fixture.add_pinned(
            "north wall",
            box_brush(DVec3::new(x0, 0.0, side), DVec3::new(x1, height, side + t)),
            OccluderVerdict::Occludes,
        );
    }
    for (label, x0) in [
        ("west wall", -t),
        ("dividing wall", side),
        ("east wall", 2.0 * side + t),
    ] {
        fixture.add_pinned(
            label,
            box_brush(DVec3::new(x0, 0.0, 0.0), DVec3::new(x0 + t, height, side)),
            OccluderVerdict::Occludes,
        );
    }
    fixture.apart.push((
        DVec3::new(side * 0.5, 2.0, side * 0.5),
        DVec3::new(side * 1.5 + t, 2.0, side * 0.5),
    ));
    fixture
}

/// A closed room 8 m square and 4 m tall of 0.3 m slabs, meeting edge to
/// edge, on the floor of a hall 40 m square and 13 m tall (pin R31).
pub(crate) fn room_in_hall() -> Fixture {
    let (t, lo, hi, height) = (ROOM_SLAB_M, 16.0, 24.0, 4.0);
    let mut fixture = Fixture::new("room in a hall", hall(DVec3::new(40.0, 13.0, 40.0)));
    let walls = [
        ("room west wall", (lo - t, lo), (lo, hi)),
        ("room east wall", (hi, hi + t), (lo, hi)),
        ("room south wall", (lo, hi), (lo - t, lo)),
        ("room north wall", (lo, hi), (hi, hi + t)),
    ];
    for (label, (x0, x1), (z0, z1)) in walls {
        fixture.add_pinned(
            label,
            box_brush(DVec3::new(x0, 0.0, z0), DVec3::new(x1, height, z1)),
            OccluderVerdict::Occludes,
        );
    }
    fixture.add_pinned(
        "room roof",
        box_brush(DVec3::new(lo, height, lo), DVec3::new(hi, height + t, hi)),
        OccluderVerdict::Occludes,
    );
    fixture
        .apart
        .push((DVec3::new(20.0, 2.0, 20.0), DVec3::new(10.0, 2.0, 20.0)));
    fixture
}

/// An open-air serpentine maze under a sky 100 m up: walls 12 m long, 3 m
/// tall and 0.3 m thick, alternately off each side of a court 14 m wide, so
/// each corridor is 2 m wide and turns through a 2 m gap at a free end. A
/// railing 1 m tall runs down the middle corridor.
pub(crate) fn open_air_maze() -> Fixture {
    let (corridor, t, length, height, court) = (2.0, 0.3, 12.0, 3.0, 14.0);
    let walls = 4;
    let depth = (walls + 1) as f64 * corridor + walls as f64 * t;
    let mut fixture = Fixture::new("open-air maze", hall(DVec3::new(court, 100.0, depth)));
    for k in 0..walls {
        let z0 = corridor + k as f64 * (corridor + t);
        let x0 = if k % 2 == 0 { 0.0 } else { court - length };
        fixture.add_pinned(
            "maze wall",
            box_brush(
                DVec3::new(x0, 0.0, z0),
                DVec3::new(x0 + length, height, z0 + t),
            ),
            OccluderVerdict::Occludes,
        );
    }
    let middle = 2.0 * (corridor + t) + corridor * 0.5;
    fixture.add_pinned(
        "railing",
        box_brush(
            DVec3::new(3.0, 0.0, middle - 0.05),
            DVec3::new(11.0, 1.0, middle + 0.05),
        ),
        OccluderVerdict::UnderFloor,
    );
    let corridor_point =
        |k: usize| DVec3::new(court * 0.5, 1.5, k as f64 * (corridor + t) + corridor * 0.5);
    for k in 0..walls {
        fixture
            .apart
            .push((corridor_point(k), corridor_point(k + 1)));
    }
    fixture
}

// Constant edges (A6, A7). Each fixture's box passes every test but the one
// its measure decides, and the measure sits at the named constant, so the
// rows follow any retuning.

/// Where a constant-edge fixture's measure sits against its constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Under,
    At,
    Over,
}

impl Edge {
    pub(crate) const ALL: [Edge; 3] = [Edge::Under, Edge::At, Edge::Over];

    fn offset(self) -> f64 {
        match self {
            Edge::Under => -EDGE_STEP_M,
            Edge::At => 0.0,
            Edge::Over => EDGE_STEP_M,
        }
    }

    /// The tests are inclusive: only a measure under its constant fails it.
    fn verdict(self, failing: OccluderVerdict) -> OccluderVerdict {
        match self {
            Edge::Under => failing,
            Edge::At | Edge::Over => OccluderVerdict::Occludes,
        }
    }
}

/// A wall 3 m tall filling a corridor whose width sits at the floor.
pub(crate) fn floor_edge(edge: Edge) -> Fixture {
    let width = OCCLUDER_FLOOR_M + edge.offset();
    let mut fixture = Fixture::new(
        format!("wall filling a corridor {width} m wide"),
        hall(DVec3::new(width, 6.0, 30.0)),
    );
    fixture.add_pinned(
        "corridor wall",
        box_brush(DVec3::new(0.0, 0.0, 15.0), DVec3::new(width, 3.0, 15.3)),
        edge.verdict(OccluderVerdict::UnderFloor),
    );
    fixture
}

/// A free-standing wall 6.5 m tall at the absolute length, in a hall wide
/// enough that it stays under half its open span.
pub(crate) fn absolute_length_edge(edge: Edge) -> Fixture {
    let length = OCCLUDER_ABSOLUTE_LENGTH_M + edge.offset();
    let width = 1.3 * OCCLUDER_ABSOLUTE_LENGTH_M / OCCLUDER_SPAN_FRACTION;
    let mut fixture = Fixture::new(
        format!("wall {length} m long in a hall {width} m wide"),
        hall(DVec3::new(width, 13.0, 60.0)),
    );
    let x0 = (width - length) * 0.5;
    fixture.add_pinned(
        "long wall",
        box_brush(
            DVec3::new(x0, 0.0, 30.0),
            DVec3::new(x0 + length, 6.5, 30.3),
        ),
        edge.verdict(OccluderVerdict::ShortSpan),
    );
    fixture
}

/// A free-standing wall 4 m tall at half the open span of a room 24 m wide,
/// under the absolute length.
pub(crate) fn half_span_edge(edge: Edge) -> Fixture {
    let width = 24.0;
    let length = OCCLUDER_SPAN_FRACTION * width + edge.offset();
    assert!(length < OCCLUDER_ABSOLUTE_LENGTH_M);
    let mut fixture = Fixture::new(
        format!("wall {length} m long in a room {width} m wide"),
        hall(DVec3::new(width, 6.0, 30.0)),
    );
    let x0 = (width - length) * 0.5;
    fixture.add_pinned(
        "short wall",
        box_brush(
            DVec3::new(x0, 0.0, 15.0),
            DVec3::new(x0 + length, 4.0, 15.3),
        ),
        edge.verdict(OccluderVerdict::ShortSpan),
    );
    fixture
}

/// The retaining wall with its top at the see-over height above the floor.
pub(crate) fn see_over_edge(edge: Edge) -> Fixture {
    let height = OCCLUDER_SEE_OVER_M + edge.offset();
    let mut fixture = Fixture::new(
        format!("wall {height} m tall"),
        hall(DVec3::new(52.0, 13.0, 60.0)),
    );
    fixture.add_pinned(
        "see-over wall",
        box_brush(DVec3::new(13.0, 0.0, 30.0), DVec3::new(39.0, height, 30.81)),
        edge.verdict(OccluderVerdict::LowTop),
    );
    fixture
}

/// A wall 12 m long and 4 m tall off the hall's -x wall, a gap the floor
/// deep in front of its -z wall, the slot between them closed by a pier at
/// the free end. Fronting the slot the wall fills its span; fronting the
/// hall, 12 of 40 m, it fails both horizontal tests. Only the slot face can
/// pass, so the gap decides; under it the hall face's short span is the
/// verdict.
pub(crate) fn one_face_gap_edge(edge: Edge) -> Fixture {
    let gap = OCCLUDER_FLOOR_M + edge.offset();
    let mut fixture = Fixture::new(
        format!("wall {gap} m off a hall wall"),
        hall(DVec3::new(40.0, 6.0, 20.0)),
    );
    fixture.add_pinned(
        "slot wall",
        box_brush(DVec3::new(0.0, 0.0, gap), DVec3::new(12.0, 4.0, gap + 0.3)),
        edge.verdict(OccluderVerdict::ShortSpan),
    );
    fixture.brushes.push(box_brush(
        DVec3::new(12.0, 0.0, 0.0),
        DVec3::new(12.3, 4.0, gap),
    ));
    fixture
}

/// Every constant-edge fixture at `edge`.
pub(crate) fn constant_edges(edge: Edge) -> [Fixture; 5] {
    [
        floor_edge(edge),
        absolute_length_edge(edge),
        half_span_edge(edge),
        see_over_edge(edge),
        one_face_gap_edge(edge),
    ]
}

// Rotated brushes (pin R5).

/// The 25 m curb turned 45° in plan in a room 30 m across on both
/// horizontal axes, where its world-axis box would pass both broad axes.
pub(crate) fn turned_curb() -> Fixture {
    let mut fixture = Fixture::in_hall("curb turned 45°", DVec3::new(30.0, 6.0, 30.0));
    fixture.add_pinned(
        "turned curb",
        turned_box(
            DVec3::new(15.0, 0.15, 15.0),
            DVec3::new(12.5, 0.15, 0.15),
            std::f64::consts::FRAC_PI_4,
        ),
        OccluderVerdict::UnderFloor,
    );
    fixture
}

/// The retaining-wall fixture turned 45° in plan, hall and all.
pub(crate) fn turned_retaining_wall() -> Fixture {
    let mut fixture = retaining_wall();
    fixture.name = "retaining wall turned 45°, hall and all".into();
    fixture.brushes = fixture
        .brushes
        .iter()
        .map(|brush| turned_about_up(brush, std::f64::consts::FRAC_PI_4))
        .collect();
    fixture
}

// Order invariance.

/// A 10 m cube on the floor of a room 18 m across, 30 m long and 18 m tall
/// (pin R18). Every face ties for broad; the faces fronting the room's
/// length see 10 of its 18 m width and pass, so the cube occludes.
pub(crate) fn cube_in_room() -> Fixture {
    let mut fixture = Fixture::in_hall("10 m cube", DVec3::new(18.0, 18.0, 30.0));
    fixture.add_pinned(
        "cube",
        box_brush(DVec3::new(4.0, 0.0, 10.0), DVec3::new(14.0, 10.0, 20.0)),
        OccluderVerdict::Occludes,
    );
    fixture
}

/// The chain in a hall 40 m by 25 m and 13 m tall, every wall 7 m tall and
/// 0.3 m thick (pin R28): a 21 m wall along z, 2 m from each end wall; a
/// 12 m wall along x off its side, its far face 8.5 m from the 21 m wall's
/// end; a 12 m wall along z off that face at its midpoint. Each 12 m wall's
/// free end is 2.5 m from the hall wall.
pub(crate) fn chain_walls() -> Fixture {
    let (t, height) = (0.3, 7.0);
    let mut fixture = Fixture::new("chain", hall(DVec3::new(40.0, 13.0, 25.0)));
    let first_x = 40.0 - 2.5 - 12.0 - t;
    fixture.add_pinned(
        "21 m wall",
        box_brush(
            DVec3::new(first_x, 0.0, 2.0),
            DVec3::new(first_x + t, height, 23.0),
        ),
        OccluderVerdict::Occludes,
    );
    let second_far_z = 2.0 + 8.5;
    fixture.add_pinned(
        "second wall",
        box_brush(
            DVec3::new(first_x + t, 0.0, second_far_z - t),
            DVec3::new(37.5, height, second_far_z),
        ),
        OccluderVerdict::Occludes,
    );
    let mid_x = (first_x + t + 37.5) * 0.5;
    fixture.add_pinned(
        "third wall",
        box_brush(
            DVec3::new(mid_x - t * 0.5, 0.0, second_far_z),
            DVec3::new(mid_x + t * 0.5, height, 25.0 - 2.5),
        ),
        OccluderVerdict::Occludes,
    );
    fixture
}
