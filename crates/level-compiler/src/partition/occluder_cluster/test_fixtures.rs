// Synthetic brush sets for occluder clusters: strip facades, corners, lamps,
// colonnades, tiled floors and step runs, each split into the room without
// the row's brushes and the brushes the row adds.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::ops::Range;

use glam::{DQuat, DVec3};

use crate::map_data::BrushVolume;
use crate::partition::occluder_classify::test_fixtures::{
    Edge, Fixture, box_brush, hall, turned_about_up,
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
        lamp_post_in_alcove(),
        colonnade(),
        tiled_floor_with_pillar(),
        facade_with_lamp(),
        step_run(STEP_RUN_STEPS),
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

// ---- coverage edge ----

/// A facade 24 m long and 6 m tall across a hall 40 × 10 × 30 m whose
/// bounding face passes the occluder rule: a cap rail 1 m tall over sixteen
/// strips 0.6 m wide at 1.5 m pitch, exactly half the face, with the last
/// strip one coverage cell (0.1 m) narrower under the constant and wider
/// over it (pin R12). Every edge lands on a cell boundary.
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
/// its bounding face sees 1.5 of 2.5 m and passes the occluder rule while
/// its solid covers under a fifth of it.
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
/// beam 0.3 m square, 21 m long, across a hall 40 × 10 × 30 m: the bounding
/// face, 21 × 4 m, passes the occluder rule; its solid covers about a third.
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
