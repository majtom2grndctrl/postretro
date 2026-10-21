// Reflection-probe capture exclusion: which static brushes each probe's primary rays skip.
// See: context/lib/resource_management.md §5.2

use glam::DVec3;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;

use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, ReflectionCapture};
use crate::partition::Aabb;

use super::capture_exclusion::ProjectionBox;
use super::exclusion_underside::{PlaneBasis, bound_water, downward_faces, has_underside};
use super::placement::PlacedProbe;

/// Water reflects about its plane, whose normal is up.
pub(crate) const WATER_REFLECTOR_NORMAL: DVec3 = DVec3::Y;

/// The size threshold's share of the water width up to the knee.
pub(crate) const THRESHOLD_NEAR_SHARE: f64 = 0.85;
/// Water width (m) up to which the share stays [`THRESHOLD_NEAR_SHARE`].
pub(crate) const THRESHOLD_KNEE_WIDTH_M: f64 = 10.0;
/// The share at [`THRESHOLD_FAR_WIDTH_M`]; past the knee the share follows
/// the power law through both anchors, continuing beyond the far one.
pub(crate) const THRESHOLD_FAR_SHARE: f64 = 0.33;
pub(crate) const THRESHOLD_FAR_WIDTH_M: f64 = 50.0;

/// The smallest march stride divisor among the reflection tiers that run the
/// screen-space layer: the coarsest stride, so the thin-brush floor holds at
/// every tier. A compiler constant, not a read of the runtime presets, so
/// retuning a finer tier never re-keys a capture; a test pins it to them.
pub(crate) const SSR_COARSEST_STRIDE_DIVISOR: u32 = 180;
/// The engine's default horizontal field of view. A copy: the binary's
/// `camera::HFOV` (crates/postretro/src/camera.rs) is out of the compiler's
/// reach, so a test pins this to it.
pub(crate) const DEFAULT_HFOV_DEGREES: f64 = 100.0;
/// The aspect the floor's vertical field of view is taken at.
pub(crate) const FLOOR_ASPECT: f64 = 16.0 / 9.0;

/// A face points down, toward the reflector, when n·N ≤ −sin 1°.
pub(crate) const UNDERSIDE_MAX_NORMAL_DOT: f64 = -0.017_452_406_437_283_51;

/// The constants that shape the rule, as the capture's cache key reads them:
/// a retune of any one re-keys every fluid map's capture, even where it moves
/// no brush in or out. A constant the rule gains joins this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExclusionConstants {
    pub near_share: f64,
    pub knee_width_m: f64,
    pub far_share: f64,
    pub far_width_m: f64,
    pub stride_divisor: u32,
    pub hfov_degrees: f64,
    pub floor_aspect: f64,
    pub underside_max_normal_dot: f64,
}

impl ExclusionConstants {
    pub(crate) const PRODUCTION: Self = Self {
        near_share: THRESHOLD_NEAR_SHARE,
        knee_width_m: THRESHOLD_KNEE_WIDTH_M,
        far_share: THRESHOLD_FAR_SHARE,
        far_width_m: THRESHOLD_FAR_WIDTH_M,
        stride_divisor: SSR_COARSEST_STRIDE_DIVISOR,
        hfov_degrees: DEFAULT_HFOV_DEGREES,
        floor_aspect: FLOOR_ASPECT,
        underside_max_normal_dot: UNDERSIDE_MAX_NORMAL_DOT,
    };

    /// Every field, little-endian, in declaration order.
    pub(crate) fn key_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 * 8);
        for value in [
            self.near_share,
            self.knee_width_m,
            self.far_share,
            self.far_width_m,
            self.hfov_degrees,
            self.floor_aspect,
            self.underside_max_normal_dot,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.stride_divisor.to_le_bytes());
        out
    }
}

/// What classification reads besides the placed probes.
pub(crate) struct ExclusionScene<'a> {
    /// Hulls in engine metres; `aabb` is the unclipped hull.
    pub brush_volumes: &'a [BrushVolume],
    /// Index-aligned with `brush_volumes`.
    pub capture_override: &'a [ReflectionCapture],
    /// Each post-cut face's source brush, index-aligned with `geometry`'s
    /// faces; `None` for a face the oversize cut emptied.
    pub face_brush: &'a [Option<u32>],
    /// The post-cut static world: each brush's surviving faces.
    pub geometry: &'a GeometryResult,
    /// Id 52, whose surface faces the probes bind.
    pub environment_volumes: &'a EnvironmentVolumesSection,
}

/// Each probe's excluded brushes: ascending, deduplicated indices into
/// `scene.brush_volumes`. A brush is excluded from a probe when it is set to
/// `Never`, or follows the rule (`Auto`) and is smaller than the probe's
/// threshold in the reflector's plane, at least the floor thick along its
/// normal, and has no underside over the probe's water. `Always` keeps.
/// Every brush of the map is classified for every probe.
#[cfg(test)]
pub(crate) fn classify_probes(scene: &ExclusionScene<'_>, probes: &[PlacedProbe]) -> Vec<Vec<u32>> {
    classify(scene, probes, |_, _| true)
}

/// [`classify_probes`], each probe's set restricted to brushes whose hulls
/// touch its projection box (floor ignored, within `PLANE_TOLERANCE_M`): the
/// stage's sets. The capture is the same either way: a ray reaching a hull
/// off the box has left the box for good, so it ends outside it or misses
/// whether that hull is kept or excluded. Behind an earlier excluded brush
/// both take the leak guard on that brush; with none, the kept hull's hit
/// and the leak guard on the excluded hull read the same face at the same
/// distance. So the work, and the key's share of the brushes, shrink to what
/// each box holds.
pub(crate) fn classify_probes_within_boxes(
    scene: &ExclusionScene<'_>,
    probes: &[PlacedProbe],
) -> Vec<Vec<u32>> {
    classify(scene, probes, |projection_box, brush| {
        projection_box.touches(&brush.aabb)
    })
}

/// The rule over the brushes `in_scope` admits for each probe's box.
fn classify(
    scene: &ExclusionScene<'_>,
    probes: &[PlacedProbe],
    in_scope: impl Fn(&ProjectionBox, &BrushVolume) -> bool,
) -> Vec<Vec<u32>> {
    debug_assert_eq!(scene.capture_override.len(), scene.brush_volumes.len());
    debug_assert_eq!(
        scene.face_brush.len(),
        scene.geometry.face_index_ranges.len()
    );
    let basis = PlaneBasis::new(WATER_REFLECTOR_NORMAL);
    let extents: Vec<BrushExtents> = scene
        .brush_volumes
        .iter()
        .map(|brush| BrushExtents::of(&brush.aabb, basis.normal))
        .collect();
    let downward = downward_faces(scene, &basis);
    probes
        .iter()
        .map(|probe| {
            let width = water_width(probe);
            let (threshold, floor) = (size_threshold(width), thin_floor(width));
            let water = bound_water(scene.environment_volumes, probe, &basis);
            let projection_box = ProjectionBox::of(probe);
            (0..scene.brush_volumes.len())
                .filter(|&brush| in_scope(&projection_box, &scene.brush_volumes[brush]))
                .filter(|&brush| {
                    let capture = scene
                        .capture_override
                        .get(brush)
                        .copied()
                        .unwrap_or_default();
                    match capture {
                        ReflectionCapture::Always => false,
                        ReflectionCapture::Never => true,
                        ReflectionCapture::Auto => {
                            excluded_by_rule(extents[brush], threshold, floor, || {
                                has_underside(&downward[brush], &water, &basis)
                            })
                        }
                    }
                })
                .map(|brush| brush as u32)
                .collect()
        })
        .collect()
}

/// W: the side of a square holding the probe's bound water, in metres.
pub(crate) fn water_width(probe: &PlacedProbe) -> f64 {
    probe.bound_area.max(0.0).sqrt()
}

/// t(W) = p(W)·W: brushes smaller than this in the reflector's plane leave
/// the probe.
pub(crate) fn size_threshold(width: f64) -> f64 {
    let share = if width <= THRESHOLD_KNEE_WIDTH_M {
        THRESHOLD_NEAR_SHARE
    } else {
        THRESHOLD_NEAR_SHARE * (width / THRESHOLD_KNEE_WIDTH_M).powf(-threshold_exponent())
    };
    share * width
}

/// a = ln(p_near / p_far) / ln(W_far / W_knee), exact: rounding it moves t
/// at 50 m and 100 m by a few millimetres.
fn threshold_exponent() -> f64 {
    (THRESHOLD_NEAR_SHARE / THRESHOLD_FAR_SHARE).ln()
        / (THRESHOLD_FAR_WIDTH_M / THRESHOLD_KNEE_WIDTH_M).ln()
}

/// f ÷ W = 2·tan(θv/2) / D: the extent one coarsest march stride spans per
/// metre of view distance, with θv the default vertical field of view.
pub(crate) fn floor_ratio() -> f64 {
    let half_horizontal = (DEFAULT_HFOV_DEGREES.to_radians() * 0.5).tan();
    let vertical_fov = 2.0 * (half_horizontal / FLOOR_ASPECT).atan();
    2.0 * (vertical_fov * 0.5).tan() / f64::from(SSR_COARSEST_STRIDE_DIVISOR)
}

/// f(W): brushes thinner than this along the reflector normal stay in the
/// probe, since the march can step over them from view distance W.
pub(crate) fn thin_floor(width: f64) -> f64 {
    width * floor_ratio()
}

/// A hull's size against a reflector normal, measured on its axis-aligned
/// bounds, so a rotated brush reads up to √2 wider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BrushExtents {
    /// The longest box edge projected into the reflector's plane. For an
    /// axis-aligned normal, the longest in-plane axis extent: for water, the
    /// longest horizontal extent; for a wall, vertical or along the wall.
    pub size: f64,
    /// The box's extent along the normal.
    pub thinness: f64,
}

impl BrushExtents {
    pub(crate) fn of(aabb: &Aabb, normal: DVec3) -> Self {
        let n = normal.normalize();
        let extent = (aabb.max - aabb.min).max(DVec3::ZERO);
        let size = (0..3)
            .map(|axis| extent[axis] * (1.0 - n[axis] * n[axis]).max(0.0).sqrt())
            .fold(0.0, f64::max);
        Self {
            size,
            thinness: extent.dot(n.abs()),
        }
    }
}

/// The automatic rule; `has_underside` runs only for a brush the size and
/// floor would exclude.
pub(crate) fn excluded_by_rule(
    extents: BrushExtents,
    threshold: f64,
    floor: f64,
    has_underside: impl FnOnce() -> bool,
) -> bool {
    extents.size < threshold && extents.thinness >= floor && !has_underside()
}
