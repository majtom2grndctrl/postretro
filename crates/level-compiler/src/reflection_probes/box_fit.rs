// Reflection-probe box fit: the room a water face sees, from rays cast against the static world.
// See: context/lib/resource_management.md §5.1

use glam::DVec3;

use super::hit::SceneTracer;

/// Fit rays: the zenith plus this many azimuths (from +X toward +Z) on each
/// elevation ring, every [`FIT_ELEVATION_STEP_DEG`] from the horizon up to
/// [`FIT_RING_COUNT`] rings. Water sees only its upper hemisphere, so no ray
/// points below the horizon. Eight azimuths per quadrant put rays on the box
/// axes and both diagonals; 2.5° rings see a far wall through the band
/// between a low rim prop and the ceiling (e24's still pool sees 3.4°).
pub(super) const FIT_AZIMUTHS: usize = 32;
const FIT_ELEVATION_STEP_DEG: f64 = 2.5;
pub(super) const FIT_RING_COUNT: usize = 36;
/// Hits on one side within this distance of each other lie on one plane.
const FIT_PLANE_MERGE_M: f64 = 0.01;

/// One fit ray: its direction, the solid angle it stands for (as a share of
/// the hemisphere), and its azimuth column; the zenith has none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FitRay {
    pub direction: DVec3,
    pub weight: f64,
    pub azimuth: Option<usize>,
}

/// The pinned fit pattern, zenith first, then rings in elevation order with
/// azimuths ascending. A ring ray's weight is its share of the band between
/// the half-steps around its elevation (the horizon ring's band starts at the
/// horizon); the zenith stands for the cap above the last band.
pub(crate) fn fit_rays() -> Vec<FitRay> {
    let half = (FIT_ELEVATION_STEP_DEG * 0.5).to_radians();
    let top = (FIT_RING_COUNT as f64 - 0.5) * FIT_ELEVATION_STEP_DEG;
    let mut rays = vec![FitRay {
        direction: DVec3::Y,
        weight: 1.0 - top.to_radians().sin(),
        azimuth: None,
    }];
    for ring in 0..FIT_RING_COUNT {
        let elevation = (ring as f64 * FIT_ELEVATION_STEP_DEG).to_radians();
        let band = (elevation + half).sin() - (elevation - half).max(0.0).sin();
        let (sin_e, cos_e) = elevation.sin_cos();
        for k in 0..FIT_AZIMUTHS {
            let azimuth = std::f64::consts::TAU * k as f64 / FIT_AZIMUTHS as f64;
            let (sin_a, cos_a) = azimuth.sin_cos();
            rays.push(FitRay {
                direction: DVec3::new(cos_e * cos_a, sin_e, cos_e * sin_a),
                weight: band / FIT_AZIMUTHS as f64,
                azimuth: Some(k),
            });
        }
    }
    rays
}

/// The axis-aligned bounds of the static world; fitted boxes clamp to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WorldBounds {
    pub min: DVec3,
    pub max: DVec3,
}

impl WorldBounds {
    /// Bounds of `positions`, or `None` when there are none.
    pub(crate) fn from_positions<'p>(
        positions: impl Iterator<Item = &'p [f32; 3]>,
    ) -> Option<Self> {
        let mut bounds: Option<Self> = None;
        for p in positions {
            let p = DVec3::from(p.map(f64::from));
            bounds = Some(match bounds {
                Some(b) => Self {
                    min: b.min.min(p),
                    max: b.max.max(p),
                },
                None => Self { min: p, max: p },
            });
        }
        bounds
    }

    pub(crate) fn diagonal(&self) -> f64 {
        (self.max - self.min).length()
    }

    /// Where a ray from `origin` inside the bounds leaves them.
    fn exit_point(&self, origin: DVec3, direction: DVec3) -> DVec3 {
        let mut t_exit = f64::INFINITY;
        for axis in 0..3 {
            let d = direction[axis];
            if d.abs() > 1e-12 {
                let wall = if d > 0.0 {
                    self.max[axis]
                } else {
                    self.min[axis]
                };
                t_exit = t_exit.min((wall - origin[axis]) / d);
            }
        }
        origin + direction * t_exit.max(0.0)
    }

    fn clamp(&self, p: DVec3) -> DVec3 {
        p.clamp(self.min, self.max)
    }
}

/// Box sides a fitted plane can bound: +X, −X, +Z, −Z and the ceiling. The
/// floor of the box is the capture plane itself, since water reads only its
/// upper hemisphere.
const SIDES: [(usize, f64); 5] = [(0, 1.0), (0, -1.0), (2, 1.0), (2, -1.0), (1, 1.0)];
const CEILING: usize = 4;

/// A wall a fit ray hit: the box side it bounds and its plane's coordinate
/// along that side's axis.
type Wall = (usize, f64);

/// The box a face's water sees, from `origin` just above it: the room shell.
/// Walls vote by azimuth column, walking each column up from the horizon.
/// Each wall hit tops the column in turn until its first ceiling-facing
/// (downward) hit closes it. After that, a wall tops the column only when it
/// lies farther out than the top, along the top's side: the side of a beam
/// spanning the room, or the riser of a tray ceiling, is nearer than the
/// room wall the column met below it, so it is ceiling detail and no shell;
/// the room wall seen over a coping lip overhanging a sunken pool is farther
/// than the pit wall met under the lip, so it tops the column although the
/// lip's underside closed it.
///
/// Then a column whose last wall lies on a wall plane takes that last wall
/// instead: the plane has an opening in it, so a column seeing through a
/// doorway tops out on the lintel. A plane is a wall when some column meets
/// it before its first ceiling hit, or tops out on it and ends there having
/// met no ceiling farther out than its top on the way: beside a doorway over
/// a lipped pool, a column reaches the room wall past nothing but the lip's
/// underside, while one reaching a tray ceiling's riser has passed the
/// soffit beyond its pit wall first, so the riser is no wall. An opening
/// spanning a room's whole side leaves no column to meet its plane that
/// way: it is a beam with a room beyond it, so the box reaches the far
/// room's wall (the runtime's distance search still finds the lintel).
///
/// A sunken pool's column tops out on the room wall above its pit, and a
/// column over a low prop on the wall beyond it. A lip that hides a wall
/// from the water nearly up to a tray ceiling's soffit leaves the columns
/// facing that wall nothing to top out on but the raised centre's riser, so
/// a pool off-centre under such a lip fits the riser on the sides it lies
/// toward; an authored `reflection_volume` box overrides it. Each side takes
/// the plane most columns vote for (ties to the farther). The ceiling is the
/// downward-facing plane the most sky ends on. A side nothing votes for
/// falls back to the farthest point reached that way, misses counting where
/// they leave the world bounds; the box is clamped to those bounds and
/// always holds the origin. Rays run in their pinned order and every rule is
/// a fixed comparison over them, so the box depends on nothing but the
/// geometry.
pub(crate) fn fit_box(
    tracer: &SceneTracer<'_>,
    origin: DVec3,
    bounds: &WorldBounds,
    rays: &[FitRay],
) -> (DVec3, DVec3) {
    let origin = bounds.clamp(origin);
    let reach = bounds.diagonal() as f32 + 1.0;
    let origin_f32 = origin.as_vec3();
    // Per column: the wall that tops it, the last wall below its first
    // ceiling hit, the last wall that topped it past a ceiling before it was
    // sealed, and the last wall at all. A column is sealed once a ceiling hit
    // lies farther out than its top: it has looked past its wall.
    let mut column_top: [Option<Wall>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_open: [Option<Wall>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_raised: [Option<Wall>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_last: [Option<Wall>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_closed = [false; FIT_AZIMUTHS];
    let mut column_sealed = [false; FIT_AZIMUTHS];
    let mut ceiling: Vec<(f64, f64)> = Vec::new();
    let mut reached = [f64::NEG_INFINITY; 5];
    // Rays run ring by ring upward, so a column's later wall hit is higher.
    for ray in rays {
        let direction = ray.direction;
        let (point, facing) = match tracer.closest_hit(origin_f32, direction.as_vec3(), reach) {
            Some(hit) => {
                let surface = tracer.surface(&hit, origin_f32, direction.as_vec3());
                let point = origin + direction * f64::from(hit.distance);
                (point, facing_side(surface.normal.as_dvec3()))
            }
            None => (bounds.exit_point(origin, direction), None),
        };
        let point = bounds.clamp(point);
        for (side, &(axis, sign)) in SIDES.iter().enumerate() {
            reached[side] = reached[side].max((point[axis] - origin[axis]) * sign);
        }
        match (facing, ray.azimuth) {
            (Some(CEILING), column) => {
                ceiling.push((point.y, ray.weight));
                if let Some(column) = column {
                    column_closed[column] = true;
                    if column_top[column].is_some_and(|top| beyond(point, top)) {
                        column_sealed[column] = true;
                    }
                }
            }
            (Some(side), Some(column)) => {
                let wall = Some((side, point[SIDES[side].0]));
                column_last[column] = wall;
                if !column_closed[column] {
                    column_top[column] = wall;
                    column_open[column] = wall;
                } else if column_top[column].is_some_and(|top| beyond(point, top)) {
                    // Past the ceiling, only a wall farther out than the top
                    // replaces it; one on the top's plane or nearer is detail.
                    column_top[column] = wall;
                    if !column_sealed[column] {
                        column_raised[column] = wall;
                    }
                }
            }
            _ => {}
        }
    }
    let ended_raised: [Option<Wall>; FIT_AZIMUTHS] = std::array::from_fn(|column| {
        column_raised[column]
            .filter(|&raised| column_last[column].is_some_and(|last| same_plane(raised, last)))
    });
    let is_wall = |wall: Wall| {
        column_open
            .iter()
            .chain(&ended_raised)
            .flatten()
            .any(|&plane| same_plane(plane, wall))
    };
    let mut votes: [Vec<(f64, f64)>; 5] = Default::default();
    for (top, last) in column_top.into_iter().zip(column_last) {
        let top = match last {
            Some(last) if is_wall(last) => Some(last),
            _ => top,
        };
        if let Some((side, coordinate)) = top {
            votes[side].push((coordinate, 1.0));
        }
    }
    votes[CEILING] = ceiling;
    let mut lo = origin;
    let mut hi = origin;
    for (side, &(axis, sign)) in SIDES.iter().enumerate() {
        let plane = best_plane(&mut votes[side], origin[axis], sign)
            .unwrap_or(origin[axis] + reached[side].max(0.0) * sign);
        if sign > 0.0 {
            hi[axis] = hi[axis].max(plane);
        } else {
            lo[axis] = lo[axis].min(plane);
        }
    }
    (bounds.clamp(lo), bounds.clamp(hi))
}

/// The box side a surface facing `normal` can bound: a wall facing −X bounds
/// +X, and so on. Up-facing surfaces (floors, ledge tops) bound nothing.
fn facing_side(normal: DVec3) -> Option<usize> {
    let a = normal.abs();
    if a.y >= a.x && a.y >= a.z {
        (normal.y < 0.0).then_some(CEILING)
    } else if a.x >= a.z {
        Some(if normal.x < 0.0 { 0 } else { 1 })
    } else {
        Some(if normal.z < 0.0 { 2 } else { 3 })
    }
}

/// Whether `point` lies more than [`FIT_PLANE_MERGE_M`] farther out than
/// `wall`'s plane, along its side.
fn beyond(point: DVec3, (side, plane): Wall) -> bool {
    let (axis, sign) = SIDES[side];
    (point[axis] - plane) * sign > FIT_PLANE_MERGE_M
}

/// Whether two walls bound one side on one plane, within
/// [`FIT_PLANE_MERGE_M`].
fn same_plane(a: Wall, b: Wall) -> bool {
    a.0 == b.0 && (a.1 - b.1).abs() < FIT_PLANE_MERGE_M
}

/// The best-supported plane among `(coordinate, weight)` votes: votes within
/// [`FIT_PLANE_MERGE_M`] merge into one plane; ties go to the plane farther
/// from `origin` along `sign`.
fn best_plane(votes: &mut [(f64, f64)], origin: f64, sign: f64) -> Option<f64> {
    votes.sort_by(|a, b| ((a.0 - origin) * sign).total_cmp(&((b.0 - origin) * sign)));
    let mut best: Option<(f64, f64)> = None;
    let mut start = 0;
    while start < votes.len() {
        let mut end = start;
        let (mut weight, mut weighted) = (0.0, 0.0);
        while end < votes.len()
            && ((votes[end].0 - votes[start].0) * sign).abs() < FIT_PLANE_MERGE_M
        {
            weight += votes[end].1;
            weighted += votes[end].0 * votes[end].1;
            end += 1;
        }
        if best.is_none_or(|(_, w)| weight >= w) {
            best = Some((weighted / weight, weight));
        }
        start = end;
    }
    best.map(|(plane, _)| plane)
}
