// Inclusive point and segment tests against one posed brush, and the box
// and sphere tests that prefilter them.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::DVec3;

use super::pose::PosedBrush;
use crate::partition::Aabb;

/// How far outside a brush a point may lie and still touch it. It absorbs
/// the `f32` rounding of texel centres and lifted origins at map extents,
/// and stays well under the bake's 1 mm lift (`RAY_EPSILON`), so the lift
/// still decides whether a receiver beside a posed face is covered.
pub(crate) const CONTACT_TOLERANCE_M: f64 = 1.0e-4;

/// How far box prefilters grow a brush's or pose's bounds. Growing each
/// plane by the contact band moves an acute corner further than the band;
/// ten bands hold every corner wider than about 11°.
pub(crate) const BOX_SLACK_M: f64 = 10.0 * CONTACT_TOLERANCE_M;

/// Whether `point` lies inside `brush` or on its surface. The grown bounds
/// clamp first, as in [`segment_touches_brush`]: past an acute edge the grown
/// half-spaces alone reach `CONTACT_TOLERANCE_M / sin(θ/2)` beyond the tip,
/// over a centimetre at 1°.
pub(crate) fn point_touches_brush(point: DVec3, brush: &PosedBrush) -> bool {
    point_may_touch_box(point, &brush.bounds)
        && brush
            .planes
            .iter()
            .all(|&(normal, distance)| normal.dot(point) - distance <= CONTACT_TOLERANCE_M)
}

/// Whether `point` lies in `brush`'s interior, at least the contact band
/// inside every face. A point on a face, within the band, is not.
pub(crate) fn point_inside_brush(point: DVec3, brush: &PosedBrush) -> bool {
    brush
        .planes
        .iter()
        .all(|&(normal, distance)| normal.dot(point) - distance <= -CONTACT_TOLERANCE_M)
}

/// Whether the closed segment `start..=end` meets `brush`: crossing it,
/// grazing an edge, running flush along a face, or starting or ending inside
/// or on it. Covered means dark, so every contact counts.
pub(crate) fn segment_touches_brush(start: DVec3, end: DVec3, brush: &PosedBrush) -> bool {
    segment_touches_brush_within(start, end, brush, CONTACT_TOLERANCE_M)
}

/// [`segment_touches_brush`] with each face grown by `band`, under
/// [`BOX_SLACK_M`], in place of the contact band. The box prefilter still
/// clamps first: at half the slack, past an edge sharper than 60° the grown
/// reach stops at the slack.
pub(crate) fn segment_touches_brush_within(
    start: DVec3,
    end: DVec3,
    brush: &PosedBrush,
    band: f64,
) -> bool {
    debug_assert!(band < BOX_SLACK_M);
    if !segment_may_touch_box(start, end, &brush.bounds) {
        return false;
    }
    // Clip the segment's parameter range to each grown half-space in turn.
    let (mut enter, mut exit) = (0.0_f64, 1.0_f64);
    for &(normal, distance) in &brush.planes {
        let from = normal.dot(start) - distance - band;
        let to = normal.dot(end) - distance - band;
        if from > 0.0 && to > 0.0 {
            return false;
        }
        if from > 0.0 {
            enter = enter.max(from / (from - to));
        } else if to > 0.0 {
            exit = exit.min(from / (from - to));
        }
        if enter > exit {
            return false;
        }
    }
    true
}

/// Conservative slab test of the closed segment against `bounds` grown by
/// [`BOX_SLACK_M`]: `false` only when the segment cannot touch them.
pub(crate) fn segment_may_touch_box(start: DVec3, end: DVec3, bounds: &Aabb) -> bool {
    let (min, max) = (
        bounds.min - DVec3::splat(BOX_SLACK_M),
        bounds.max + DVec3::splat(BOX_SLACK_M),
    );
    let delta = end - start;
    let (mut enter, mut exit) = (0.0_f64, 1.0_f64);
    for axis in 0..3 {
        if delta[axis] == 0.0 {
            if start[axis] < min[axis] || start[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let near = (min[axis] - start[axis]) / delta[axis];
        let far = (max[axis] - start[axis]) / delta[axis];
        enter = enter.max(near.min(far));
        exit = exit.min(near.max(far));
        if enter > exit {
            return false;
        }
    }
    true
}

/// Whether `point` lies in `bounds` grown by [`BOX_SLACK_M`].
fn point_may_touch_box(point: DVec3, bounds: &Aabb) -> bool {
    let slack = DVec3::splat(BOX_SLACK_M);
    (bounds.min - slack).cmple(point).all() && point.cmple(bounds.max + slack).all()
}

/// Whether the closed ball of `radius` about `centre` meets `bounds`.
pub(crate) fn sphere_touches_box(centre: DVec3, radius: f64, bounds: &Aabb) -> bool {
    centre.distance(centre.clamp(bounds.min, bounds.max)) <= radius
}

/// Whether two boxes overlap once `b` grows by [`BOX_SLACK_M`].
pub(crate) fn boxes_may_touch(a: &Aabb, b: &Aabb) -> bool {
    let slack = DVec3::splat(BOX_SLACK_M);
    (a.min - slack).cmple(b.max).all() && (b.min - slack).cmple(a.max).all()
}
