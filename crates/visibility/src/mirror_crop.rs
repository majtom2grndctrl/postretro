// Planar water mirror crop: the selected plane's padded screen bounds, composed into the mirror's view-projection.
// See: context/lib/rendering_pipeline.md §2, §7.3a

#[cfg(test)]
mod tests;

use glam::{DVec3, Mat4, Vec2, Vec3, Vec4};

use crate::portal_vis::clip_polygon_to_planes;
use crate::region_walk::SNAP_STEPS;
use crate::visibility::{FrustumPlane, NEAR_PLANE_INDEX, frustum_plane_array};

/// NDC slack each crop edge moves outward by before the screen clamp: it
/// absorbs f32 error in the clip and projection, and lets a face clipped at
/// a screen edge crop to exactly that edge.
const CROP_SLACK: f64 = 1.0e-4;

/// The pad the app crops the mirror with: render-cpu's
/// `planar_reflection::MIRROR_CROP_PAD_RADIANS`, which documents the
/// derivation and whose test pins this copy equal. Twice the largest
/// pixel-style normal tilt (a 2 + 2 texel displayed offset × 0.05 tangent
/// per texel, so 0.4 rad of ray turn) plus π × the horizon clamp's 1e-3.
/// This crate cannot depend on render-cpu; its mirror sweep and cropped
/// headroom test read this copy, so they measure the walk the app runs.
pub const MIRROR_CROP_PAD_RADIANS: f32 = 2.0 * 4.0 * 0.05 + std::f32::consts::PI * 1.0e-3;

/// An NDC rectangle, `min` ≤ `max` on both axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NdcRect {
    pub min: Vec2,
    pub max: Vec2,
}

impl NdcRect {
    /// The whole screen.
    #[cfg(test)]
    pub(crate) const FULL: Self = Self {
        min: Vec2::NEG_ONE,
        max: Vec2::ONE,
    };

    /// The fraction of the full screen this rect covers.
    #[cfg(test)]
    pub(crate) fn screen_fraction(&self) -> f32 {
        let size = (self.max - self.min).max(Vec2::ZERO);
        size.x * size.y / 4.0
    }
}

/// The crop's polygon clip buffers, reused across frames so a steady state
/// allocates nothing.
#[derive(Debug, Default)]
pub struct MirrorCropScratch {
    clip_a: Vec<Vec3>,
    clip_b: Vec<Vec3>,
}

/// The mirror crop under `view_proj` with apex `eye`:
/// [`mirror_crop_bounds`] widened by [`align_to_walk_grid`], so a walk
/// under the cropped view-projection never reaches a cell the uncropped walk
/// does not. `None` when no face keeps any part on screen.
pub(crate) fn mirror_crop_rect<'a>(
    view_proj: Mat4,
    eye: Vec3,
    faces: impl IntoIterator<Item = &'a [Vec3]>,
    pad_radians: f32,
    scratch: &mut MirrorCropScratch,
) -> Option<NdcRect> {
    mirror_crop_bounds(view_proj, eye, faces, pad_radians, scratch).map(align_to_walk_grid)
}

/// The NDC rect, under `view_proj` with apex `eye`, holding every ray from
/// `eye` that lies within `pad_radians` of a ray through some point of
/// `faces`, clamped to the screen. `None` when no face keeps any part on
/// screen in front of the eye.
///
/// Each face is clipped to `view_proj`'s side planes and near plane first:
/// only the plane's on-screen part reflects, and the near clip keeps every
/// projected point in front of the eye. The pad is an angle around each
/// point's own ray, so it holds at every depth along it: for each side, the
/// rect's edge plane through the eye keeps the whole cone of half-angle
/// `pad_radians` around every clipped vertex's ray on its inner side (the
/// cones around a convex polygon's points are held once its vertices' are).
/// A cone reaching the eye plane leaves that side at the screen edge.
pub(crate) fn mirror_crop_bounds<'a>(
    view_proj: Mat4,
    eye: Vec3,
    faces: impl IntoIterator<Item = &'a [Vec3]>,
    pad_radians: f32,
    scratch: &mut MirrorCropScratch,
) -> Option<NdcRect> {
    let planes = frustum_plane_array(view_proj);
    // Left, right, bottom, top and near: the far plane never hides water a
    // fragment shows.
    let clip: [FrustumPlane; 5] = [
        planes[0],
        planes[1],
        planes[2],
        planes[3],
        planes[NEAR_PLANE_INDEX],
    ];
    let sin_pad = f64::from(pad_radians.clamp(0.0, std::f32::consts::FRAC_PI_2)).sin();
    let rows = [view_proj.row(0), view_proj.row(1), view_proj.row(3)];
    let axes = [
        SideAxis::new(rows[0].truncate(), rows[2].truncate()),
        SideAxis::new(rows[1].truncate(), rows[2].truncate()),
    ];
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    let mut any = false;
    for face in faces {
        let clipped = clip_polygon_to_planes(face, &clip, &mut scratch.clip_a, &mut scratch.clip_b);
        if clipped.len() < 3 {
            continue;
        }
        any = true;
        for &point in clipped {
            let ray = (point - eye).as_dvec3();
            let length = ray.length();
            if !(length.is_finite() && length > 0.0) {
                continue;
            }
            let ray = ray / length;
            for (axis, side) in axes.iter().enumerate() {
                min[axis] = min[axis].min(side.min_edge(ray, sin_pad));
                max[axis] = max[axis].max(side.max_edge(ray, sin_pad));
            }
        }
    }
    if !any {
        return None;
    }
    let clamp = |v: f64| v.clamp(-1.0, 1.0) as f32;
    let rect = NdcRect {
        min: Vec2::new(clamp(min[0] - CROP_SLACK), clamp(min[1] - CROP_SLACK)),
        max: Vec2::new(clamp(max[0] + CROP_SLACK), clamp(max[1] + CROP_SLACK)),
    };
    (rect.min.x < rect.max.x && rect.min.y < rect.max.y).then_some(rect)
}

/// The smallest rect holding `rect`, per axis, whose remap to the full
/// screen is an integer zoom `k` about a centre on the walk's snap grid
/// (`rendering_pipeline.md` §2). The cropped walk then snaps on a grid that
/// refines the full screen's: every full-screen grid line is one of its
/// lines, and its snap pad is `1/k` of the full screen's, so each rect it
/// snaps lies inside the rect the uncropped walk snaps from the same or a
/// larger polygon. Without this, an outward snap on the cropped grid can
/// cross a full-screen grid line the uncropped snap stops at, and a portal
/// touching that sliver reaches a cell the uncropped walk does not. The
/// cost is a crop up to the next width `2/k` per axis.
pub(crate) fn align_to_walk_grid(rect: NdcRect) -> NdcRect {
    let (x0, x1) = align_axis(rect.min.x, rect.max.x);
    let (y0, y1) = align_axis(rect.min.y, rect.max.y);
    NdcRect {
        min: Vec2::new(x0, y0),
        max: Vec2::new(x1, y1),
    }
}

/// One axis of [`align_to_walk_grid`]: the largest zoom `k` whose span
/// `[c − 1/k, c + 1/k]`, centred on a grid line `c`, holds `lo..hi` and
/// stays on screen; the full screen (`k = 1`) when none does.
fn align_axis(lo: f32, hi: f32) -> (f32, f32) {
    let grid = f64::from(SNAP_STEPS);
    let (lo, hi) = (f64::from(lo), f64::from(hi));
    if !(lo.is_finite() && hi.is_finite()) {
        return (-1.0, 1.0);
    }
    let centre = (lo + hi) * 0.5;
    // Rounding the centre to the grid moves it at most half a grid step.
    let mut k = (1.0 / ((hi - lo) * 0.5 + 0.5 / grid)).floor() as i64;
    while k > 1 {
        let reach = 1.0 / k as f64;
        let lowest = ((reach - 1.0) * grid).ceil() / grid;
        let highest = ((1.0 - reach) * grid).floor() / grid;
        let c = ((centre * grid).round() / grid).clamp(lowest, highest);
        if lowest <= highest && c - reach <= lo && c + reach >= hi {
            return ((c - reach) as f32, (c + reach) as f32);
        }
        k -= 1;
    }
    (-1.0, 1.0)
}

/// `view_proj` followed by the NDC remap that stretches `rect` to the full
/// screen: its frustum's side planes become the rect's edges, and its near
/// and far planes are unchanged. The full rect returns `view_proj` exactly.
pub(crate) fn ndc_rect_to_full_screen(view_proj: Mat4, rect: NdcRect) -> Mat4 {
    let size = rect.max - rect.min;
    let scale = Vec2::splat(2.0) / size;
    let offset = -(rect.max + rect.min) / size;
    let remap = Mat4::from_cols(
        Vec4::new(scale.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, scale.y, 0.0, 0.0),
        Vec4::Z,
        Vec4::new(offset.x, offset.y, 0.0, 1.0),
    );
    remap * view_proj
}

/// The mirror crop rect (`mirror_crop_rect`) composed into `view_proj` by
/// the NDC remap that stretches it to the full screen
/// (`ndc_rect_to_full_screen`): the view-projection the mirror walk and cull
/// take. `None` when nothing of `faces` projects.
pub fn crop_mirror_view_proj<'a>(
    view_proj: Mat4,
    eye: Vec3,
    faces: impl IntoIterator<Item = &'a [Vec3]>,
    pad_radians: f32,
    scratch: &mut MirrorCropScratch,
) -> Option<Mat4> {
    mirror_crop_rect(view_proj, eye, faces, pad_radians, scratch)
        .map(|rect| ndc_rect_to_full_screen(view_proj, rect))
}

/// One screen axis: NDC `k` is `(a·r)/(b·r)` for a ray `r` from the eye,
/// with `a` that axis's view-projection row and `b` the w row (xyz parts).
struct SideAxis {
    a: DVec3,
    b: DVec3,
}

impl SideAxis {
    fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            a: a.as_dvec3(),
            b: b.as_dvec3(),
        }
    }

    /// The largest `k` whose edge plane `(a − k·b)·r = 0` keeps the cone of
    /// sine `s` around unit `ray` inside, `(a − k·b)·ray ≥ s·|a − k·b|`;
    /// −∞ when the cone reaches the eye plane and no edge holds it.
    fn min_edge(&self, ray: DVec3, s: f64) -> f64 {
        cone_edge(self.a.dot(ray), self.b.dot(ray), self.a, self.b, s)
    }

    /// The smallest `k` whose edge plane keeps the cone on its `k ≥ ndc`
    /// side; +∞ when none does. [`Self::min_edge`] with the axis negated.
    fn max_edge(&self, ray: DVec3, s: f64) -> f64 {
        -cone_edge(-self.a.dot(ray), self.b.dot(ray), -self.a, self.b, s)
    }
}

/// Largest `k` with `p − k·q ≥ s·|a − k·b|`, `p = a·ray`, `q = b·ray`. The
/// left side is linear in `k` and the right convex, so the feasible `k` run
/// to −∞ and end at the smaller root of the squared equality (the larger one
/// is the branch where `p − k·q` is negative).
fn cone_edge(p: f64, q: f64, a: DVec3, b: DVec3, s: f64) -> f64 {
    let s2 = s * s;
    let alpha = q * q - s2 * b.length_squared();
    if alpha.is_nan() || alpha <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let beta = -2.0 * (p * q - s2 * a.dot(b));
    let gamma = p * p - s2 * a.length_squared();
    let disc = (beta * beta - 4.0 * alpha * gamma).max(0.0);
    (-beta - disc.sqrt()) / (2.0 * alpha)
}
