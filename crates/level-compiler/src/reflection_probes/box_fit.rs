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
const FIT_AZIMUTHS: usize = 32;
const FIT_ELEVATION_STEP_DEG: f64 = 2.5;
const FIT_RING_COUNT: usize = 36;
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

/// The box a face's water sees, from `origin` just above it: the room shell.
/// Walls vote by azimuth column: walking a column up from the horizon, the
/// last wall its rays hit before its first ceiling-facing hit is the shell
/// there, and each side takes the plane most columns vote for (ties to the
/// farther). A wall a column meets only above a ceiling hit is ceiling
/// detail, such as the side of a beam spanning the room or the riser of a
/// tray ceiling, and is no shell, unless another column meets the same plane
/// below any ceiling: then it is a wall with an opening under it, and a
/// column seeing through a doorway tops out on the lintel. A sunken pool's
/// column tops out on the room wall above its pit, and a column over a low
/// prop on the wall beyond it, so none of them pulls the box off the room.
/// The ceiling is the downward-facing plane the most sky ends on. A side
/// nothing votes for falls back to the farthest point reached that way,
/// misses counting where they leave the world bounds; the box is clamped to
/// those bounds and always holds the origin.
pub(crate) fn fit_box(
    tracer: &SceneTracer<'_>,
    origin: DVec3,
    bounds: &WorldBounds,
    rays: &[FitRay],
) -> (DVec3, DVec3) {
    let origin = bounds.clamp(origin);
    let reach = bounds.diagonal() as f32 + 1.0;
    let origin_f32 = origin.as_vec3();
    // Per column: the last wall below its first ceiling hit, and the last
    // wall at all.
    let mut column_top: [Option<(usize, f64)>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_last: [Option<(usize, f64)>; FIT_AZIMUTHS] = [None; FIT_AZIMUTHS];
    let mut column_closed = [false; FIT_AZIMUTHS];
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
                }
            }
            (Some(side), Some(column)) => {
                let wall = Some((side, point[SIDES[side].0]));
                column_last[column] = wall;
                if !column_closed[column] {
                    column_top[column] = wall;
                }
            }
            _ => {}
        }
    }
    // A plane some column meets below any ceiling is a wall.
    let is_wall = |side: usize, coordinate: f64| {
        column_top
            .iter()
            .flatten()
            .any(|&(s, c)| s == side && (c - coordinate).abs() < FIT_PLANE_MERGE_M)
    };
    let mut votes: [Vec<(f64, f64)>; 5] = Default::default();
    for (top, last) in column_top.into_iter().zip(column_last) {
        let top = match last {
            Some((side, coordinate)) if is_wall(side, coordinate) => last,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::reflection_probes::reflection_probes_test_fixtures::{
        GeometryBuilder, box_room, rect,
    };
    use glam::Vec3;

    fn fit(geometry: &crate::geometry::GeometryResult, origin: DVec3) -> (DVec3, DVec3) {
        let (bvh, primitives, _) = build_bvh(geometry).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, geometry);
        let bounds =
            WorldBounds::from_positions(geometry.geometry.vertices.iter().map(|v| &v.position))
                .unwrap();
        fit_box(&tracer, origin, &bounds, &fit_rays())
    }

    fn quad(builder: &mut GeometryBuilder, normal: Vec3, corners: [Vec3; 4]) {
        builder.push_face(normal, &corners, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    }

    fn assert_box(actual: (DVec3, DVec3), min: [f64; 3], max: [f64; 3]) {
        let close = |a: DVec3, b: [f64; 3]| (a - DVec3::from(b)).abs().max_element() < 1e-3;
        assert!(close(actual.0, min) && close(actual.1, max), "{actual:?}");
    }

    #[test]
    fn fit_box_reaches_the_walls_and_ceiling_of_a_plain_room() {
        let room = box_room([-4.0, -1.0, -3.0], [5.0, 3.0, 2.0]);
        let fitted = fit(&room, DVec3::new(1.0, 0.002, -0.5));
        assert_box(fitted, [-4.0, 0.002, -3.0], [5.0, 3.0, 2.0]);
    }

    /// A pool sunk 1.5 m into a 12 m room's floor: the water sees the pit's
    /// walls only at the horizon, and the room's walls above them.
    #[test]
    fn fit_box_of_a_sunken_pool_reaches_the_room_not_the_pit() {
        let (lo, hi) = (Vec3::new(-6.0, -1.5, -6.0), Vec3::new(6.0, 4.0, 6.0));
        let pit = 2.0;
        let mut b = GeometryBuilder::default();
        // Room shell above the floor line.
        quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, lo.x),
            (Vec3::NEG_X, 0, hi.x),
            (Vec3::Z, 2, lo.z),
            (Vec3::NEG_Z, 2, hi.z),
        ] {
            quad(
                &mut b,
                normal,
                rect(Vec3::new(lo.x, 0.0, lo.z), hi, axis, value),
            );
        }
        // Floor around the pit, as four strips.
        let floor = |b: &mut GeometryBuilder, x0: f32, z0: f32, x1: f32, z1: f32| {
            let r = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, 0.0);
            quad(b, Vec3::Y, r);
        };
        floor(&mut b, lo.x, lo.z, hi.x, -pit);
        floor(&mut b, lo.x, pit, hi.x, hi.z);
        floor(&mut b, lo.x, -pit, -pit, pit);
        floor(&mut b, pit, -pit, hi.x, pit);
        // The pit: walls facing its centre, and its floor.
        let (plo, phi) = (Vec3::new(-pit, lo.y, -pit), Vec3::new(pit, 0.0, pit));
        quad(&mut b, Vec3::Y, rect(plo, phi, 1, lo.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, -pit),
            (Vec3::NEG_X, 0, pit),
            (Vec3::Z, 2, -pit),
            (Vec3::NEG_Z, 2, pit),
        ] {
            quad(&mut b, normal, rect(plo, phi, axis, value));
        }
        let geometry = b.finish(vec!["t".to_string()]);

        let water = -0.4;
        let fitted = fit(&geometry, DVec3::new(0.3, water + 0.002, -0.2));
        assert_box(fitted, [-6.0, water + 0.002, -6.0], [6.0, 4.0, 6.0]);
    }

    /// Two 6 m rooms joined by a 2 m doorway in the wall at x = 0: a pool in
    /// the west room fits the west room, not both.
    #[test]
    fn fit_box_stops_at_a_doorway_wall() {
        let (lo, hi) = (Vec3::new(-6.0, 0.0, -3.0), Vec3::new(6.0, 3.0, 3.0));
        let mut b = GeometryBuilder::default();
        quad(&mut b, Vec3::Y, rect(lo, hi, 1, lo.y));
        quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, lo.x),
            (Vec3::NEG_X, 0, hi.x),
            (Vec3::Z, 2, lo.z),
            (Vec3::NEG_Z, 2, hi.z),
        ] {
            quad(&mut b, normal, rect(lo, hi, axis, value));
        }
        // The dividing wall, both faces, around a doorway z −1..1, y 0..2.
        for normal in [Vec3::NEG_X, Vec3::X] {
            let pieces = [
                (Vec3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 3.0, -1.0)),
                (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 3.0, 3.0)),
                (Vec3::new(0.0, 2.0, -1.0), Vec3::new(0.0, 3.0, 1.0)),
            ];
            for (plo, phi) in pieces {
                quad(&mut b, normal, rect(plo, phi, 0, 0.0));
            }
        }
        let geometry = b.finish(vec!["t".to_string()]);

        // Off the doorway's axis, as a pool beside a doorway would be.
        let fitted = fit(&geometry, DVec3::new(-3.0, 0.502, 0.5));
        assert_box(fitted, [-6.0, 0.502, -3.0], [0.0, 3.0, 3.0]);
    }

    /// A 1.6 m wall on the pool's rim, 2 m off, as e24's grain markers stand:
    /// the far wall shows in the band above it, and the box reaches it.
    #[test]
    fn fit_box_reaches_past_a_low_wall_on_the_rim() {
        let mut b = GeometryBuilder::default();
        let (lo, hi) = (Vec3::new(-12.0, 0.0, -6.0), Vec3::new(12.0, 10.0, 6.0));
        quad(&mut b, Vec3::Y, rect(lo, hi, 1, lo.y));
        quad(&mut b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, lo.x),
            (Vec3::NEG_X, 0, hi.x),
            (Vec3::Z, 2, lo.z),
            (Vec3::NEG_Z, 2, hi.z),
        ] {
            quad(&mut b, normal, rect(lo, hi, axis, value));
        }
        let (plo, phi) = (Vec3::new(2.0, 0.0, -2.5), Vec3::new(2.4, 1.6, 2.5));
        for (normal, axis, value) in [
            (Vec3::NEG_X, 0, plo.x),
            (Vec3::X, 0, phi.x),
            (Vec3::Y, 1, phi.y),
            (Vec3::NEG_Z, 2, plo.z),
            (Vec3::Z, 2, phi.z),
        ] {
            quad(&mut b, normal, rect(plo, phi, axis, value));
        }
        let geometry = b.finish(vec!["t".to_string()]);

        let fitted = fit(&geometry, DVec3::new(0.0, 0.302, 0.0));
        assert_box(fitted, [-12.0, 0.302, -6.0], [12.0, 10.0, 6.0]);
    }

    /// The walls and ceiling of the room `lo`..`hi`; no floor, since fit rays
    /// never point down.
    fn open_floor_room(b: &mut GeometryBuilder, lo: Vec3, hi: Vec3) {
        quad(b, Vec3::NEG_Y, rect(lo, hi, 1, hi.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, lo.x),
            (Vec3::NEG_X, 0, hi.x),
            (Vec3::Z, 2, lo.z),
            (Vec3::NEG_Z, 2, hi.z),
        ] {
            quad(b, normal, rect(lo, hi, axis, value));
        }
    }

    /// A beam spanning the room above the pool, 1 m deep under a 4 m
    /// ceiling: columns toward it meet the ceiling beyond the beam first,
    /// then the beam's side, which must not become the room's wall.
    #[test]
    fn fit_box_ignores_a_ceiling_beam_spanning_the_room() {
        let (lo, hi) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 4.0, 6.0));
        let mut b = GeometryBuilder::default();
        open_floor_room(&mut b, lo, hi);
        let (blo, bhi) = (Vec3::new(0.0, 3.0, lo.z), Vec3::new(0.4, hi.y, hi.z));
        for (normal, axis, value) in [
            (Vec3::NEG_Y, 1, blo.y),
            (Vec3::NEG_X, 0, blo.x),
            (Vec3::X, 0, bhi.x),
        ] {
            quad(&mut b, normal, rect(blo, bhi, axis, value));
        }
        let geometry = b.finish(vec!["t".to_string()]);

        let fitted = fit(&geometry, DVec3::new(-2.0, 0.302, 0.3));
        assert_box(fitted, [-6.0, 0.302, -6.0], [6.0, 4.0, 6.0]);
    }

    /// A tray ceiling: a 3 m soffit 2 m deep around the walls, the centre
    /// raised to 4 m. Columns meet the soffit, then the raised centre's
    /// riser, which must not become the room's wall.
    #[test]
    fn fit_box_of_a_tray_ceiling_reaches_the_walls() {
        let (lo, hi) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(6.0, 3.0, 6.0));
        let mut b = GeometryBuilder::default();
        for (normal, axis, value) in [
            (Vec3::X, 0, lo.x),
            (Vec3::NEG_X, 0, hi.x),
            (Vec3::Z, 2, lo.z),
            (Vec3::NEG_Z, 2, hi.z),
        ] {
            quad(&mut b, normal, rect(lo, hi, axis, value));
        }
        // The soffit as four strips around the raised centre.
        let inner = 4.0;
        let soffit = |b: &mut GeometryBuilder, x0: f32, z0: f32, x1: f32, z1: f32| {
            let r = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, hi.y);
            quad(b, Vec3::NEG_Y, r);
        };
        soffit(&mut b, lo.x, lo.z, hi.x, -inner);
        soffit(&mut b, lo.x, inner, hi.x, hi.z);
        soffit(&mut b, lo.x, -inner, -inner, inner);
        soffit(&mut b, inner, -inner, hi.x, inner);
        // The raised centre: its risers facing inward, and its ceiling.
        let (clo, chi) = (
            Vec3::new(-inner, hi.y, -inner),
            Vec3::new(inner, 4.0, inner),
        );
        quad(&mut b, Vec3::NEG_Y, rect(clo, chi, 1, chi.y));
        for (normal, axis, value) in [
            (Vec3::X, 0, clo.x),
            (Vec3::NEG_X, 0, chi.x),
            (Vec3::Z, 2, clo.z),
            (Vec3::NEG_Z, 2, chi.z),
        ] {
            quad(&mut b, normal, rect(clo, chi, axis, value));
        }
        let geometry = b.finish(vec!["t".to_string()]);

        // Most sky ends on the raised centre, so the box's top is its 4 m.
        let fitted = fit(&geometry, DVec3::new(0.3, 0.302, -0.2));
        assert_box(fitted, [-6.0, 0.302, -6.0], [6.0, 4.0, 6.0]);
    }

    #[test]
    fn fit_rays_cover_the_upper_hemisphere_by_solid_angle() {
        let rays = fit_rays();
        assert_eq!(rays.len(), 1 + FIT_AZIMUTHS * FIT_RING_COUNT);
        assert!(rays.iter().all(|r| r.direction.y >= 0.0 && r.weight > 0.0));
        assert!(
            rays.iter()
                .all(|r| (r.direction.length() - 1.0).abs() < 1e-12)
        );
        // Weights partition the hemisphere: 2π sr over 2π.
        let total: f64 = rays.iter().map(|r| r.weight).sum();
        assert!((total - 1.0).abs() < 1e-9, "{total}");
    }
}
