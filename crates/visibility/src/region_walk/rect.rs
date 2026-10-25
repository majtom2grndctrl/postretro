// The walk's per-cell region: a grow-only screen rect on a fixed snap grid.
// See: context/lib/rendering_pipeline.md §2

use glam::{Vec3, Vec4};

use crate::visibility::FrustumPlane;

/// Snap grid steps per NDC unit. Bounds snap outward, so a rect only takes
/// values on a finite lattice and every cell's rect can grow only so often.
pub(crate) const SNAP_STEPS: i32 = 256;
/// NDC pad applied before the outward snap, so a bound sitting on a grid
/// line still lands on the outer side of it.
const SNAP_PAD: f32 = 1e-4;
/// Outward pad, in snap steps, on the planes a rect clips with. The
/// clipped bounds are intersected with the rect afterwards, so the pad never
/// widens a region; it only keeps tangent portals.
const CLIP_PAD_STEPS: f32 = 0.5;
/// Clip-space w (view depth) below which projecting a point is unstable. A
/// clipped polygon that comes this close to the eye plane hands on its
/// source rect instead.
pub(crate) const MIN_PROJECT_W: f32 = 1e-3;

/// Inclusive NDC bounds in snap-grid steps; the full screen is
/// `[-SNAP_STEPS, SNAP_STEPS]` on both axes. A rect can be zero-width on an
/// axis (a portal seen edge-on) but never inverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub x0: i32,
    pub x1: i32,
    pub y0: i32,
    pub y1: i32,
}

impl ScreenRect {
    pub(crate) const FULL: Self = Self {
        x0: -SNAP_STEPS,
        x1: SNAP_STEPS,
        y0: -SNAP_STEPS,
        y1: SNAP_STEPS,
    };

    /// `None` when the two share no point.
    pub(crate) fn intersect(&self, other: &Self) -> Option<Self> {
        let rect = Self {
            x0: self.x0.max(other.x0),
            x1: self.x1.min(other.x1),
            y0: self.y0.max(other.y0),
            y1: self.y1.min(other.y1),
        };
        (rect.x0 <= rect.x1 && rect.y0 <= rect.y1).then_some(rect)
    }

    /// Union in place. Returns whether any bound moved, compared on the snap
    /// grid, so a contribution inside the rect is a no-op.
    pub(crate) fn grow(&mut self, other: &Self) -> bool {
        let before = *self;
        self.x0 = self.x0.min(other.x0);
        self.x1 = self.x1.max(other.x1);
        self.y0 = self.y0.min(other.y0);
        self.y1 = self.y1.max(other.y1);
        *self != before
    }

    /// Clamp each bound into `outer`: a degenerate rect on `outer`'s edge
    /// for a contribution that float slop pushed just outside it.
    pub(crate) fn clamp_into(&self, outer: &Self) -> Self {
        let clamp = |v: i32, lo: i32, hi: i32| v.clamp(lo, hi);
        Self {
            x0: clamp(self.x0, outer.x0, outer.x1),
            x1: clamp(self.x1, outer.x0, outer.x1),
            y0: clamp(self.y0, outer.y0, outer.y1),
            y1: clamp(self.y1, outer.y0, outer.y1),
        }
    }

    /// The rect's four side planes in world space, pushed outward by
    /// [`CLIP_PAD_STEPS`] so a portal touching the rect only along an edge
    /// still clips to a polygon. Each plane passes through the eye, since the
    /// projection maps the eye to clip-space x = y = w = 0. `rows` are the
    /// view-projection's rows.
    pub(crate) fn push_planes(&self, rows: &[Vec4; 4], out: &mut Vec<FrustumPlane>) {
        let step = 1.0 / SNAP_STEPS as f32;
        let lo = |v: i32| (v as f32 - CLIP_PAD_STEPS) * step;
        let hi = |v: i32| (v as f32 + CLIP_PAD_STEPS) * step;
        let sides = [
            rows[0] - rows[3] * lo(self.x0),
            rows[3] * hi(self.x1) - rows[0],
            rows[1] - rows[3] * lo(self.y0),
            rows[3] * hi(self.y1) - rows[1],
        ];
        for side in sides {
            let normal = side.truncate();
            let length = normal.length();
            if length > 0.0 {
                out.push(FrustumPlane {
                    normal: normal / length,
                    dist: side.w / length,
                });
            }
        }
    }

    /// Outward-snapped screen bounds of `points`, clamped to the full screen.
    /// `None` if any point's w is below [`MIN_PROJECT_W`].
    pub(crate) fn bound_points(points: &[Vec3], rows: &[Vec4; 4]) -> Option<Self> {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for point in points {
            let h = point.extend(1.0);
            let w = rows[3].dot(h);
            if w.is_nan() || w < MIN_PROJECT_W {
                return None;
            }
            let ndc = [rows[0].dot(h) / w, rows[1].dot(h) / w];
            for axis in 0..2 {
                min[axis] = min[axis].min(ndc[axis]);
                max[axis] = max[axis].max(ndc[axis]);
            }
        }
        if points.is_empty() {
            return None;
        }
        let snap_lo = |v: f32| snap((v - SNAP_PAD) * SNAP_STEPS as f32, f32::floor);
        let snap_hi = |v: f32| snap((v + SNAP_PAD) * SNAP_STEPS as f32, f32::ceil);
        Some(Self {
            x0: snap_lo(min[0]),
            x1: snap_hi(max[0]),
            y0: snap_lo(min[1]),
            y1: snap_hi(max[1]),
        })
    }
}

fn snap(v: f32, round: fn(f32) -> f32) -> i32 {
    (round(v) as i64).clamp(-(SNAP_STEPS as i64), SNAP_STEPS as i64) as i32
}
