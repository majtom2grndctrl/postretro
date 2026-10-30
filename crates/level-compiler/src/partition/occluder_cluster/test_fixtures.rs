// Synthetic brush sets for occluder clusters: strip facades, corners, mixed-
// height rows, lamps, colonnades, low walls, tiled floors and step runs, each
// split into the room without the row's brushes and the brushes the row adds.
// See: context/plans/done/occluder-first-partition/index.md §Acceptance

use std::ops::Range;

use glam::{DQuat, DVec3};

use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::test_fixtures::{
    Edge, Fixture, box_brush, hall, turned_about_up, turned_box,
};

/// Depth of every strip facade's slab.
const FACADE_DEPTH_M: f64 = 0.3;

/// A facade's piers, under the 1.2 m floor across, so each is exempt alone.
const PIER_M: f64 = 1.0;

/// Nominal pier pitch along a facade; piers stand at both ends.
const BAY_PITCH_M: f64 = 2.5;

/// Height of a sill band and of the cap band, under the floor.
const BAND_M: f64 = 1.0;

/// Storey height: a sill band under a window 2 m tall.
const STOREY_M: f64 = 3.0;

/// A cluster row's fixture: `fixture.brushes[..base]` is the room without
/// the brushes the row adds, which follow it.
pub(crate) struct ClusterFixture {
    pub(crate) fixture: Fixture,
    pub(crate) base: usize,
}

impl ClusterFixture {
    fn new(name: &str, base: Vec<BrushVolume>) -> Self {
        let base_len = base.len();
        Self {
            fixture: Fixture::new(name, base),
            base: base_len,
        }
    }

    fn add(&mut self, brushes: impl IntoIterator<Item = BrushVolume>) -> &mut Self {
        self.fixture.brushes.extend(brushes);
        self
    }

    /// The brushes the row adds.
    pub(crate) fn added(&self) -> Range<usize> {
        self.base..self.fixture.brushes.len()
    }

    /// The room without the row's brushes.
    pub(crate) fn without(&self) -> Vec<BrushVolume> {
        self.fixture.brushes[..self.base].to_vec()
    }

    /// The whole fixture, its points included, turned `yaw` radians in plan
    /// about the world origin.
    pub(crate) fn turned(mut self, yaw: f64) -> Self {
        let turn = DQuat::from_rotation_y(yaw);
        self.fixture.name = format!("{} turned {:.0}°", self.fixture.name, yaw.to_degrees());
        self.fixture.brushes = self
            .fixture
            .brushes
            .iter()
            .map(|brush| turned_about_up(brush, yaw))
            .collect();
        for (a, b) in &mut self.fixture.apart {
            *a = turn * *a;
            *b = turn * *b;
        }
        self
    }
}

/// Every cluster fixture, for the order-invariance row.
pub(crate) fn cluster_fixtures() -> Vec<ClusterFixture> {
    let mut fixtures: Vec<ClusterFixture> = Edge::ALL.into_iter().map(coverage_edge).collect();
    fixtures.extend([
        strip_facade(),
        spaced_strips(),
        strip_facade().turned(std::f64::consts::FRAC_PI_4),
        spaced_strips().turned(std::f64::consts::FRAC_PI_4),
        corner_facade(),
        joined_facades(),
        mixed_height_row(),
        low_wall_between_posts(),
        lamp_post_in_alcove(),
        colonnade(),
        tiled_floor_with_pillar(),
        facade_with_lamp(),
        step_run(STEP_RUN_STEPS),
        facade_with_tilted_sign(OFF_TRUE_RAD).turned(std::f64::consts::FRAC_PI_4),
        facade_with_tilted_sign(-OFF_TRUE_RAD).turned(std::f64::consts::FRAC_PI_4),
        stacked_cubes(),
        stacked_cubes().turned(std::f64::consts::FRAC_PI_2),
        stacked_posts_in_collar(),
        stacked_posts_in_collar().turned(std::f64::consts::FRAC_PI_2),
        balcony_balustrade(),
        balustrade_over_stairwell(),
        louvred_screen(),
    ]);
    fixtures
}

// ---- strip facades ----

/// Strips of a facade `length` long and `height` tall in its own plane, as
/// `[along0, along1, up0, up1]`: piers at both ends and every bay, a sill
/// band under each storey's window between piers, and a cap band the full
/// length. Each strip is under the floor on one broad axis.
pub(crate) fn facade_rects(length: f64, height: f64) -> Vec<[f64; 4]> {
    let bays = ((length - PIER_M) / BAY_PITCH_M).floor().max(1.0) as usize;
    let pitch = (length - PIER_M) / bays as f64;
    let cap = height - BAND_M;
    let mut rects = vec![[0.0, length, cap, height]];
    for k in 0..=bays {
        let a = k as f64 * pitch;
        rects.push([a, a + PIER_M, 0.0, cap]);
    }
    for k in 0..bays {
        let (a0, a1) = (k as f64 * pitch + PIER_M, (k + 1) as f64 * pitch);
        let mut y = 0.0;
        while y + BAND_M < cap - 1e-9 {
            rects.push([a0, a1, y, y + BAND_M]);
            y += STOREY_M;
        }
    }
    rects
}

/// The openings `facade_rects` leaves, as `[along0, along1, up0, up1]`: in
/// each bay, a window between each sill band and the band or cap above it.
pub(crate) fn facade_windows(length: f64, height: f64) -> Vec<[f64; 4]> {
    let bays = ((length - PIER_M) / BAY_PITCH_M).floor().max(1.0) as usize;
    let pitch = (length - PIER_M) / bays as f64;
    let cap = height - BAND_M;
    let mut windows = Vec::new();
    for k in 0..bays {
        let (a0, a1) = (k as f64 * pitch + PIER_M, (k + 1) as f64 * pitch);
        let mut y = 0.0;
        while y + BAND_M < cap - 1e-9 {
            windows.push([a0, a1, y + BAND_M, (y + STOREY_M).min(cap)]);
            y += STOREY_M;
        }
    }
    windows
}

/// A facade along +x from `x0`, its slab `z0..z0 + FACADE_DEPTH_M`.
fn facade_along_x(x0: f64, z0: f64, length: f64, height: f64) -> Vec<BrushVolume> {
    facade_rects(length, height)
        .into_iter()
        .map(|[a0, a1, y0, y1]| {
            box_brush(
                DVec3::new(x0 + a0, y0, z0),
                DVec3::new(x0 + a1, y1, z0 + FACADE_DEPTH_M),
            )
        })
        .collect()
}

/// A facade along +z from `z0`, its slab `x0..x0 + FACADE_DEPTH_M`.
fn facade_along_z(x0: f64, z0: f64, length: f64, height: f64) -> Vec<BrushVolume> {
    facade_rects(length, height)
        .into_iter()
        .map(|[a0, a1, y0, y1]| {
            box_brush(
                DVec3::new(x0, y0, z0 + a0),
                DVec3::new(x0 + FACADE_DEPTH_M, y1, z0 + a1),
            )
        })
        .collect()
}

/// A strip facade 31 m long and 15 m tall across a hall 60 × 25 × 40 m,
/// its slab at z = 20 m, a third of it windows (pins R13, R23).
pub(crate) fn strip_facade() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("strip facade", hall(DVec3::new(60.0, 25.0, 40.0)));
    fixture.add(facade_along_x(15.0, 20.0, 31.0, 15.0));
    fixture
        .fixture
        .apart
        .push((DVec3::new(30.5, 1.5, 15.0), DVec3::new(30.5, 1.5, 25.0)));
    fixture
}

/// The strip facade's piers alone: spaced a bay apart, each touching only
/// the occluding floor (pin R13).
pub(crate) fn spaced_strips() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("spaced strips", hall(DVec3::new(60.0, 25.0, 40.0)));
    let piers = facade_rects(31.0, 15.0)
        .into_iter()
        .filter(|&[a0, a1, y0, _]| y0 == 0.0 && (a1 - a0 - PIER_M).abs() < 1e-9)
        .map(|[a0, a1, _, _]| {
            box_brush(
                DVec3::new(15.0 + a0, 0.0, 20.0),
                DVec3::new(15.0 + a1, 15.0, 20.0 + FACADE_DEPTH_M),
            )
        });
    fixture.add(piers);
    fixture
}

/// A strip facade 15 m tall turning a 90° corner, arms 30 m from the corner,
/// in a hall 70 m square under a ceiling 25 m up (pin R26). Arm A runs along
/// x on z = 20 m; arm B along z on x = 20 m, its first pier touching A's.
pub(crate) fn corner_facade() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("corner facade", hall(DVec3::new(70.0, 25.0, 70.0)));
    fixture.add(facade_along_x(20.0, 20.0, 30.0, 15.0));
    fixture.add(facade_along_z(
        20.0,
        20.0 + FACADE_DEPTH_M,
        30.0 - FACADE_DEPTH_M,
        15.0,
    ));
    fixture.fixture.apart.extend([
        (DVec3::new(35.0, 1.5, 15.0), DVec3::new(35.0, 1.5, 25.0)),
        (DVec3::new(15.0, 1.5, 35.0), DVec3::new(25.0, 1.5, 35.0)),
    ]);
    fixture
}

/// Two strip facades facing across a street 14.7 m wide, joined by a sign
/// 1 m tall touching a pier of each (pin R26). The sign is the row's brush.
pub(crate) fn joined_facades() -> ClusterFixture {
    let mut base = hall(DVec3::new(60.0, 25.0, 50.0));
    base.extend(facade_along_x(15.0, 15.0, 31.0, 15.0));
    base.extend(facade_along_x(15.0, 30.0, 31.0, 15.0));
    let mut fixture = ClusterFixture::new("facades joined by a sign", base);
    fixture.add([box_brush(
        DVec3::new(30.0, 10.0, 15.0 + FACADE_DEPTH_M),
        DVec3::new(30.0 + FACADE_DEPTH_M, 11.0, 30.0),
    )]);
    fixture.fixture.apart.extend([
        (DVec3::new(30.5, 1.5, 10.0), DVec3::new(30.5, 1.5, 22.0)),
        (DVec3::new(30.5, 1.5, 22.0), DVec3::new(30.5, 1.5, 40.0)),
    ]);
    fixture
}

/// The mixed-height row's frontages along x, as `(length, height)`: a long
/// 16 m building abutting a short 21 m one. Each covers about 63% of its
/// own face, but over the row's bounding box the sky above the lower roof
/// leaves under half covered.
pub(crate) const MIXED_ROW_FRONTAGES: [(f64, f64); 2] = [(40.0, 16.0), (5.0, 21.0)];

/// Where the mixed-height row starts along x.
const MIXED_ROW_X0: f64 = 8.0;

/// The mixed-height row's slab starts at this z.
const MIXED_ROW_Z0: f64 = 20.0;

/// The `MIXED_ROW_FRONTAGES` as strip facades touching end to end across a
/// hall 60 × 30 × 40 m, their slab at z = 20 m (owner ruling after Task 9).
pub(crate) fn mixed_height_row() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("mixed-height row", hall(DVec3::new(60.0, 30.0, 40.0)));
    let mut x0 = MIXED_ROW_X0;
    for (length, height) in MIXED_ROW_FRONTAGES {
        fixture.add(facade_along_x(x0, MIXED_ROW_Z0, length, height));
        x0 += length;
    }
    fixture.fixture.apart.extend([
        (DVec3::new(20.0, 1.5, 15.0), DVec3::new(20.0, 1.5, 25.0)),
        (DVec3::new(50.5, 1.5, 15.0), DVec3::new(50.5, 1.5, 25.0)),
    ]);
    fixture
}

/// A wall 1 m tall between posts 0.3 m square and 5 m tall, at 8 m centres
/// with one flush with each end, 24 m long across a hall 40 × 10 × 30 m:
/// every brush is under the floor on a broad axis, and a broad face of its
/// bounding box, 24 × 5 m, passes the occluder rule.
pub(crate) fn low_wall_between_posts() -> ClusterFixture {
    let (x0, z0) = (8.0, 15.0);
    let depth = DVec3::Z * LOW_WALL_POST_M;
    let posts = LOW_WALL_POSTS;
    let mut fixture = ClusterFixture::new(
        "low wall between tall posts",
        hall(DVec3::new(40.0, 10.0, 30.0)),
    );
    fixture.add(posts.map(|a| {
        box_brush(
            DVec3::new(x0 + a, 0.0, z0),
            DVec3::new(x0 + a + LOW_WALL_POST_M, LOW_WALL_POST_HEIGHT_M, z0) + depth,
        )
    }));
    fixture.add(posts.windows(2).map(|pair| {
        box_brush(
            DVec3::new(x0 + pair[0] + LOW_WALL_POST_M, 0.0, z0),
            DVec3::new(x0 + pair[1], LOW_WALL_HEIGHT_M, z0) + depth,
        )
    }));
    fixture
}

/// The low wall's length, and where its posts start along it.
pub(crate) const LOW_WALL_LENGTH_M: f64 = 24.0;
pub(crate) const LOW_WALL_POSTS: [f64; 4] = [0.0, 8.0, 16.0, LOW_WALL_LENGTH_M - LOW_WALL_POST_M];

/// The low wall's posts: width and depth, and height.
pub(crate) const LOW_WALL_POST_M: f64 = 0.3;
pub(crate) const LOW_WALL_POST_HEIGHT_M: f64 = 5.0;

/// The low wall's height, under the 2.5 m see-over height.
pub(crate) const LOW_WALL_HEIGHT_M: f64 = 1.0;

/// The strip facade with a lamp 0.3 m deep touching a pier's street face:
/// its broad face agrees with the facade's but it stands proud of the slab
/// (pin R4). The lamp is the row's brush.
pub(crate) fn facade_with_lamp() -> ClusterFixture {
    let facade = strip_facade();
    let mut fixture = ClusterFixture::new("strip facade with a lamp", facade.fixture.brushes);
    fixture.fixture.apart = facade.fixture.apart;
    fixture.add([box_brush(
        DVec3::new(30.2, 3.5, 20.0 - FACADE_DEPTH_M),
        DVec3::new(30.8, 4.3, 20.0),
    )]);
    fixture
}

/// A turn in plan inside the section angle, 5°: the tilted sign's off its
/// facade, and each louvre's off its row.
pub(crate) const OFF_TRUE_RAD: f64 = 5.0 * std::f64::consts::PI / 180.0;

/// The strip facade with a sign 1 m square and 0.3 m deep turned `tilt`
/// radians in plan off it, one vertical edge touching a pier's street face:
/// its broad face agrees with the facade's within the section angle without
/// being parallel to it. The sign is the row's brush.
// Regression: one normal stood for every normal within the section angle,
// the first in world xyz order; turned 45°, a sign 5° off could sort first,
// and judged across the sign's normal no two strips shared a slab.
pub(crate) fn facade_with_tilted_sign(tilt: f64) -> ClusterFixture {
    let facade = strip_facade();
    let mut fixture = ClusterFixture::new(
        &format!("strip facade with a sign {:+.0}° off", tilt.to_degrees()),
        facade.fixture.brushes,
    );
    fixture.fixture.apart = facade.fixture.apart;
    let (half, depth) = (0.5, FACADE_DEPTH_M * 0.5);
    let reach = half * tilt.sin().abs() + depth * tilt.cos();
    fixture.add([turned_box(
        DVec3::new(30.5, 4.0, 20.0 - reach),
        DVec3::new(half, half, depth),
        tilt,
    )]);
    fixture
}

/// Two cubes 1.5 m on a side stacked into a pillar 3 m tall, in a corridor
/// 2.5 m wide along x between benches 2 m tall, in a hall 30 × 6 × 10 m.
/// Every cube offers all three faces, so the pair forms one set under both
/// horizontal normals. Faced down the corridor the pillar spans more than
/// half its width on most of its face and occludes; faced across it, it
/// does not. Alone, the lower cube stands under the see-over height and the
/// upper one fronts the hall's full width over the benches on most of its
/// face.
// Regression: a set was judged under the first normal it formed under, in
// world xyz order, so turning the map 90° flipped its verdict.
pub(crate) fn stacked_cubes() -> ClusterFixture {
    let (x0, z0, side) = (14.25, 4.25, 1.5);
    let mut base = hall(DVec3::new(30.0, 6.0, 10.0));
    base.extend([
        box_brush(DVec3::ZERO, DVec3::new(30.0, 2.0, 3.75)),
        box_brush(DVec3::new(0.0, 0.0, 6.25), DVec3::new(30.0, 2.0, 10.0)),
    ]);
    let mut fixture = ClusterFixture::new("cubes stacked in a benched corridor", base);
    fixture.add([0.0, side].map(|y| {
        box_brush(
            DVec3::new(x0, y, z0),
            DVec3::new(x0 + side, y + side, z0 + side),
        )
    }));
    fixture
        .fixture
        .apart
        .push((DVec3::new(10.0, 1.0, 5.0), DVec3::new(20.0, 1.0, 5.0)));
    fixture
}

/// Two posts 2.45 m square and 2.45 m tall stacked into a pillar 4.9 m tall,
/// centred in a collar 3.3 m tall and 0.3 m thick whose inside is 4.85 m
/// along x and 4.9 m along z, in a hall 20 × 8 × 20 m. Faced along either
/// horizontal axis the pillar occludes: below the collar's top it spans half
/// the collar's inside across its face, and the gaps it fronts reach the
/// floor, 1.2 m along x and 1.225 m along z. Both judgments have the same
/// box and cover all of it, so only the bounding faces' open depth sets them
/// apart, and the faces across z front the wider gaps. Alone, the lower post
/// tops out under the see-over height and the upper one fronts the hall's
/// width over the collar on most of its face.
// Regression: of two occluding judgments the first in world xyz order stood
// for the set, so turning the map 90° moved its bounding face and outline
// from the faces across z to those across x.
pub(crate) fn stacked_posts_in_collar() -> ClusterFixture {
    let (centre, post, wall, collar_top) = (10.0, 2.45, 0.3, 3.3);
    let (x0, x1) = (centre - 4.85 / 2.0, centre + 4.85 / 2.0);
    let (z0, z1) = (centre - 4.9 / 2.0, centre + 4.9 / 2.0);
    let mut base = hall(DVec3::new(20.0, 8.0, 20.0));
    base.extend([
        box_brush(
            DVec3::new(x0 - wall, 0.0, z0 - wall),
            DVec3::new(x1 + wall, collar_top, z0),
        ),
        box_brush(
            DVec3::new(x0 - wall, 0.0, z1),
            DVec3::new(x1 + wall, collar_top, z1 + wall),
        ),
        box_brush(
            DVec3::new(x0 - wall, 0.0, z0),
            DVec3::new(x0, collar_top, z1),
        ),
        box_brush(
            DVec3::new(x1, 0.0, z0),
            DVec3::new(x1 + wall, collar_top, z1),
        ),
    ]);
    let mut fixture = ClusterFixture::new("posts stacked in an uneven collar", base);
    let (p0, p1) = (centre - post / 2.0, centre + post / 2.0);
    fixture
        .add([0.0, post].map(|y| box_brush(DVec3::new(p0, y, p0), DVec3::new(p1, y + post, p1))));
    fixture
        .fixture
        .apart
        .push((DVec3::new(centre, 1.0, 8.1), DVec3::new(centre, 1.0, 11.9)));
    fixture
}

/// The balcony deck's height above the hall floor, and the balcony's width.
pub(crate) const BALCONY_DECK_M: f64 = 5.0;
pub(crate) const BALCONY_WIDTH_M: f64 = 12.0;

/// Balustrade posts: how many, and how wide.
pub(crate) const BALUSTRADE_POSTS: usize = 12;
pub(crate) const BALUSTRADE_POST_M: f64 = 0.2;

/// Balustrade rail and post heights above the deck.
pub(crate) const BALUSTRADE_RAIL_M: f64 = 0.3;
pub(crate) const BALUSTRADE_HEIGHT_M: f64 = 2.6;

/// A balcony 12 m wide and 6 m deep off a hall 40 × 13 × 11 m, its deck a
/// solid platform 5 m up between side walls full height, and on the deck's
/// edge a balustrade 0.1 m deep, flush with it: a rail 0.3 m tall and twelve
/// posts 0.2 m wide to 2.6 m above the deck, the end posts against the side
/// walls. From the deck a broad face of its bounding box passes the occluder
/// rule; from the hall it sees 12 of 40 m and fails. The hall is shallow
/// enough that the deck and side walls occlude with or without the
/// balustrade shortening their spans. The balustrade is the row's brushes.
// Regression: the roofline read the lower of the two faces' floors, the
// hall's, whose see-over line lies under the balustrade's foot, so each gap
// counted only its rail and the balustrade read solid.
pub(crate) fn balcony_balustrade() -> ClusterFixture {
    let x1 = BALCONY_X0_M + BALCONY_WIDTH_M;
    let deck = box_brush(
        DVec3::new(BALCONY_X0_M, 0.0, 0.0),
        DVec3::new(x1, BALCONY_DECK_M, BALCONY_EDGE_M),
    );
    balustrade_on_balcony("balustrade on a balcony edge", vec![deck])
}

/// Where the balcony starts along x, and how far its deck reaches out from
/// the hall's back wall.
const BALCONY_X0_M: f64 = 14.0;
const BALCONY_EDGE_M: f64 = 6.0;

/// The stairwell's width along the balcony and its depth across it.
const STAIRWELL_M: f64 = 3.0;

/// `balcony_balustrade` with a stairwell 3 m square down through the deck's
/// middle third to the hall floor, right behind the balustrade but for a lip
/// 0.1 m deep under it. The deck face's middle lattice column drops into the
/// stairwell and its outer two onto the deck, so under the roofline the
/// columns of the middle third count only their solid and those of the
/// outer two count up to 2.5 m above the deck: the balustrade still reads
/// porous. The deck's pieces and the lip each occlude alone.
// Regression: the roofline read one floor for the whole face, the lowest its
// lattice found, here the stairwell's, whose see-over line lies under the
// balustrade's foot, so every gap counted only its rail and the balustrade
// read solid.
pub(crate) fn balustrade_over_stairwell() -> ClusterFixture {
    let (x0, x1) = (BALCONY_X0_M, BALCONY_X0_M + BALCONY_WIDTH_M);
    let middle = x0 + BALCONY_WIDTH_M / 2.0;
    let (w0, w1) = (middle - STAIRWELL_M / 2.0, middle + STAIRWELL_M / 2.0);
    let (deck, edge) = (BALCONY_DECK_M, BALCONY_EDGE_M);
    let lip = edge - BALUSTRADE_DEPTH_M;
    let back = lip - STAIRWELL_M;
    let pieces = vec![
        box_brush(DVec3::new(x0, 0.0, 0.0), DVec3::new(w0, deck, edge)),
        box_brush(DVec3::new(w1, 0.0, 0.0), DVec3::new(x1, deck, edge)),
        box_brush(DVec3::new(w0, 0.0, 0.0), DVec3::new(w1, deck, back)),
        box_brush(DVec3::new(w0, 0.0, lip), DVec3::new(w1, deck, edge)),
    ];
    balustrade_on_balcony("balustrade over a stairwell", pieces)
}

/// The balustrade's depth, flush with the deck's edge.
const BALUSTRADE_DEPTH_M: f64 = 0.1;

/// The balcony's hall, `deck` and side walls, with the balustrade on the
/// deck's edge as the row's brushes.
fn balustrade_on_balcony(name: &str, deck_brushes: Vec<BrushVolume>) -> ClusterFixture {
    let (x0, deck, edge, t) = (BALCONY_X0_M, BALCONY_DECK_M, BALCONY_EDGE_M, 0.5);
    let x1 = x0 + BALCONY_WIDTH_M;
    let mut base = hall(DVec3::new(40.0, 13.0, 11.0));
    base.extend(deck_brushes);
    base.extend([
        box_brush(DVec3::new(x0 - t, 0.0, 0.0), DVec3::new(x0, 13.0, edge)),
        box_brush(DVec3::new(x1, 0.0, 0.0), DVec3::new(x1 + t, 13.0, edge)),
    ]);
    let mut fixture = ClusterFixture::new(name, base);
    let z0 = edge - BALUSTRADE_DEPTH_M;
    let rail_top = deck + BALUSTRADE_RAIL_M;
    fixture.add([box_brush(
        DVec3::new(x0, deck, z0),
        DVec3::new(x1, rail_top, edge),
    )]);
    let pitch = (x1 - x0 - BALUSTRADE_POST_M) / (BALUSTRADE_POSTS - 1) as f64;
    fixture.add((0..BALUSTRADE_POSTS).map(|k| {
        let x = x0 + k as f64 * pitch;
        box_brush(
            DVec3::new(x, rail_top, z0),
            DVec3::new(x + BALUSTRADE_POST_M, deck + BALUSTRADE_HEIGHT_M, edge),
        )
    }));
    fixture
}

/// Twelve louvres 1 m wide, 3 m tall and 0.3 m deep at 1 m centres, each
/// turned 5° in plan and overlapping its neighbours, under a rail 12 m
/// long, 0.5 m tall and 0.3 m deep square to the row, across a hall
/// 20 × 6 × 30 m. The louvres share the section's slab, but only the rail is
/// square to its box, and the rail stops inside every face of it but the
/// top.
// Regression: only member planes parallel to a box face bounded the section
// in the occluder tier, so the screen occluded with no plane across it.
pub(crate) fn louvred_screen() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("louvred screen", hall(DVec3::new(20.0, 6.0, 30.0)));
    fixture.add((0..12).map(|k| {
        turned_box(
            DVec3::new(4.5 + k as f64, 1.5, 15.0),
            DVec3::new(0.5, 1.5, 0.15),
            OFF_TRUE_RAD,
        )
    }));
    fixture.add([box_brush(
        DVec3::new(4.0, 3.0, 14.85),
        DVec3::new(16.0, 3.5, 15.15),
    )]);
    fixture
        .fixture
        .apart
        .push((DVec3::new(10.0, 1.5, 10.0), DVec3::new(10.0, 1.5, 20.0)));
    fixture
}

/// `louvred_screen`'s room with the screen one solid slab, the rail's length
/// and depth and the screen's full height, which occludes alone.
pub(crate) fn solid_screen() -> Vec<BrushVolume> {
    let mut brushes = hall(DVec3::new(20.0, 6.0, 30.0));
    brushes.push(box_brush(
        DVec3::new(4.0, 0.0, 14.85),
        DVec3::new(16.0, 3.5, 15.15),
    ));
    brushes
}

// ---- coverage edge ----

/// A facade 24 m long and 6 m tall across a hall 40 × 10 × 30 m, a broad
/// face of whose bounding box passes the occluder rule: a cap rail 1 m tall
/// over sixteen strips 0.6 m wide at 1.5 m pitch, exactly half the face,
/// with the last strip one coverage cell (0.1 m) narrower under the
/// constant and wider over it (pin R12). Every edge lands on a cell
/// boundary.
pub(crate) fn coverage_edge(edge: Edge) -> ClusterFixture {
    let (x0, z0, length, height) = (8.0, 15.0, 24.0, 6.0);
    let cell = length / crate::partition::occluder_cluster::COVERAGE_GRID_CELLS as f64;
    let last_width = 0.6
        + match edge {
            Edge::Under => -cell,
            Edge::At => 0.0,
            Edge::Over => cell,
        };
    let mut fixture = ClusterFixture::new(
        &format!("facade at coverage edge {edge:?}"),
        hall(DVec3::new(40.0, 10.0, 30.0)),
    );
    let depth = DVec3::Z * FACADE_DEPTH_M;
    fixture.add([box_brush(
        DVec3::new(x0, height - BAND_M, z0),
        DVec3::new(x0 + length, height, z0) + depth,
    )]);
    for k in 0..16 {
        let a = x0 + 1.5 * k as f64;
        let width = if k == 15 { last_width } else { 0.6 };
        fixture.add([box_brush(
            DVec3::new(a, 0.0, z0),
            DVec3::new(a + width, height - BAND_M, z0) + depth,
        )]);
    }
    fixture
        .fixture
        .apart
        .push((DVec3::new(20.0, 2.0, 10.0), DVec3::new(20.0, 2.0, 20.0)));
    fixture
}

// ---- sparse clusters ----

/// An L lamp post of touching pole, arm and head, 5 m tall with a 1.5 m
/// reach, in an alcove 2.5 m wide and 2 m deep off a hall 20 × 8 × 20 m, so
/// a broad face of its bounding box sees 1.5 of 2.5 m and passes the
/// occluder rule while its solid covers under a fifth of it.
pub(crate) fn lamp_post_in_alcove() -> ClusterFixture {
    let (lo, hi, depth, height) = (8.75, 11.25, 2.0, 8.0);
    let t = 0.5;
    let mut base = hall(DVec3::new(20.0, height, 20.0));
    base.remove(4);
    base.extend([
        box_brush(DVec3::new(0.0, -t, -depth), DVec3::new(lo, height + t, 0.0)),
        box_brush(
            DVec3::new(hi, -t, -depth),
            DVec3::new(20.0, height + t, 0.0),
        ),
        box_brush(
            DVec3::new(lo, -t, -depth - t),
            DVec3::new(hi, height + t, -depth),
        ),
        box_brush(DVec3::new(lo, -t, -depth), DVec3::new(hi, 0.0, -t)),
        box_brush(
            DVec3::new(lo, height, -depth),
            DVec3::new(hi, height + t, -t),
        ),
    ]);
    let mut fixture = ClusterFixture::new("L lamp post in an alcove", base);
    let (z0, z1) = (-1.1, -0.9);
    fixture.add([
        box_brush(DVec3::new(9.0, 0.0, z0), DVec3::new(9.2, 5.0, z1)),
        box_brush(DVec3::new(9.2, 4.8, z0), DVec3::new(10.5, 5.0, z1)),
        box_brush(DVec3::new(10.0, 4.5, z0), DVec3::new(10.5, 4.8, z1)),
    ]);
    fixture
}

/// Six columns 1 m square and 3.7 m tall at 4 m centres under a touching
/// beam 0.3 m square, 21 m long, across a hall 40 × 10 × 30 m: a broad face
/// of its bounding box, 21 × 4 m, passes the occluder rule; its solid covers
/// about a third.
pub(crate) fn colonnade() -> ClusterFixture {
    let mut fixture = ClusterFixture::new("colonnade", hall(DVec3::new(40.0, 10.0, 30.0)));
    for k in 0..6 {
        let x = 9.5 + 4.0 * k as f64;
        fixture.add([box_brush(
            DVec3::new(x, 0.0, 14.5),
            DVec3::new(x + 1.0, 3.7, 15.5),
        )]);
    }
    fixture.add([box_brush(
        DVec3::new(9.5, 3.7, 14.85),
        DVec3::new(30.5, 4.0, 15.15),
    )]);
    fixture
}

// ---- membership ----

/// A mezzanine of 1.1 m tiles, 0.3 m thick, filling a hall 11 m square and
/// 12 m tall 6 m up, and a pillar 1 m square standing on it to 10 m (pin R4).
/// The tiles are the room; the pillar is the row's brush.
pub(crate) fn tiled_floor_with_pillar() -> ClusterFixture {
    let (tile, y0, y1) = (1.1, 6.0, 6.3);
    let mut base = hall(DVec3::new(11.0, 12.0, 11.0));
    for i in 0..10 {
        for k in 0..10 {
            let (x, z) = (i as f64 * tile, k as f64 * tile);
            base.push(box_brush(
                DVec3::new(x, y0, z),
                DVec3::new(x + tile, y1, z + tile),
            ));
        }
    }
    let mut fixture = ClusterFixture::new("tiled floor with a pillar", base);
    fixture.add([box_brush(
        DVec3::new(5.0, y1, 5.0),
        DVec3::new(6.0, 10.0, 6.0),
    )]);
    fixture
        .fixture
        .apart
        .push((DVec3::new(2.0, 8.0, 2.0), DVec3::new(2.0, 3.0, 2.0)));
    fixture
}

/// `tiled_floor_with_pillar`'s room with its mezzanine one solid slab.
pub(crate) fn solid_mezzanine() -> Vec<BrushVolume> {
    let mut brushes = hall(DVec3::new(11.0, 12.0, 11.0));
    brushes.push(box_brush(
        DVec3::new(0.0, 6.0, 0.0),
        DVec3::new(11.0, 6.3, 11.0),
    ));
    brushes
}

// ---- stepped solid ----

/// Steps in the arena staircase's run (`emit_walk_stairs`: 12 u rise, 28 u
/// tread, 320 u climb): 19.2 m long and 8.2 m high.
pub(crate) const STEP_RUN_STEPS: usize = 27;

/// A run of `steps` touching full-height steps, 0.3048 m rise and 0.7112 m
/// tread, 6.5 m wide, climbing along x in a hall 44 m long, 14 m wide and
/// 13 m tall (pin R19).
pub(crate) fn step_run(steps: usize) -> ClusterFixture {
    let (rise, tread, x0, z0, z1) = (0.3048, 0.7112, 10.0, 3.75, 10.25);
    let mut fixture = ClusterFixture::new(
        &format!("run of {steps} steps"),
        hall(DVec3::new(44.0, 13.0, 14.0)),
    );
    fixture.add((0..steps).map(|s| {
        box_brush(
            DVec3::new(x0 + s as f64 * tread, 0.0, z0),
            DVec3::new(x0 + (s + 1) as f64 * tread, (s + 1) as f64 * rise, z1),
        )
    }));
    fixture
}
