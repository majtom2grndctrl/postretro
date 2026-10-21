// Reflection-probe capture exclusion: which static brushes each probe's primary rays skip.
// See: context/lib/resource_management.md §5.2

use glam::{DVec2, DVec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;

use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, ReflectionCapture};
use crate::partition::Aabb;

use super::placement::{FaceGeometry, PLANE_TOLERANCE_M, PlacedProbe};

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
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn classify_probes(scene: &ExclusionScene<'_>, probes: &[PlacedProbe]) -> Vec<Vec<u32>> {
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
            (0..scene.brush_volumes.len())
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

/// An orthonormal frame of the reflector: heights along its normal, 2D
/// coordinates in its plane.
struct PlaneBasis {
    normal: DVec3,
    u: DVec3,
    v: DVec3,
}

impl PlaneBasis {
    fn new(normal: DVec3) -> Self {
        let normal = normal.normalize();
        // The world axis least aligned with the normal, made perpendicular.
        let a = normal.abs();
        let helper = if a.x <= a.y && a.x <= a.z {
            DVec3::X
        } else if a.y <= a.z {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let u = (helper - normal * helper.dot(normal)).normalize();
        Self {
            normal,
            u,
            v: normal.cross(u),
        }
    }

    fn height(&self, p: DVec3) -> f64 {
        p.dot(self.normal)
    }

    fn project(&self, p: DVec3) -> DVec2 {
        DVec2::new(p.dot(self.u), p.dot(self.v))
    }
}

/// An axis-aligned rectangle in the reflector's plane.
#[derive(Debug, Clone, Copy)]
struct Rect {
    min: DVec2,
    max: DVec2,
}

impl Rect {
    fn around(points: impl IntoIterator<Item = DVec2>) -> Self {
        points.into_iter().fold(
            Self {
                min: DVec2::splat(f64::INFINITY),
                max: DVec2::splat(f64::NEG_INFINITY),
            },
            |r, p| Self {
                min: r.min.min(p),
                max: r.max.max(p),
            },
        )
    }

    /// Whether the rectangles share more than an edge within the tolerance.
    fn overlaps(&self, other: &Self) -> bool {
        self.min.x < other.max.x - PLANE_TOLERANCE_M
            && other.min.x < self.max.x - PLANE_TOLERANCE_M
            && self.min.y < other.max.y - PLANE_TOLERANCE_M
            && other.min.y < self.max.y - PLANE_TOLERANCE_M
    }
}

/// One bound water face in the reflector's frame.
struct WaterFace {
    height: f64,
    outline: Vec<DVec2>,
    bounds: Rect,
}

/// The probe's bound water faces; a face id 52 does not hold is skipped.
fn bound_water(
    environment: &EnvironmentVolumesSection,
    probe: &PlacedProbe,
    basis: &PlaneBasis,
) -> Vec<WaterFace> {
    probe
        .faces
        .iter()
        .filter_map(|&(volume, face)| {
            let surface = environment
                .volumes
                .get(volume as usize)?
                .surface_faces
                .get(face as usize)?;
            let points: Vec<DVec3> = surface
                .vertices
                .iter()
                .map(|v| DVec3::from(v.position.map(f64::from)))
                .collect();
            let geometry = FaceGeometry::from_polygon(&points)?;
            let outline: Vec<DVec2> = points.iter().map(|&p| basis.project(p)).collect();
            Some(WaterFace {
                height: basis.height(geometry.centroid),
                bounds: Rect::around(outline.iter().copied()),
                outline,
            })
        })
        .collect()
}

/// A surviving face pointing down toward the reflector.
#[derive(Clone)]
struct DownwardFace {
    triangles: Vec<[DVec3; 3]>,
    bounds: Rect,
    /// Its highest point along the normal.
    top: f64,
}

/// Each brush's surviving downward faces, through the face → brush table.
fn downward_faces(scene: &ExclusionScene<'_>, basis: &PlaneBasis) -> Vec<Vec<DownwardFace>> {
    let mut per_brush: Vec<Vec<DownwardFace>> = vec![Vec::new(); scene.brush_volumes.len()];
    let geometry = &scene.geometry.geometry;
    let position =
        |index: u32| DVec3::from(geometry.vertices[index as usize].position.map(f64::from));
    for (face, &brush) in scene.face_brush.iter().enumerate() {
        let Some(brush) = brush else { continue };
        let (Some(slot), Some(range)) = (
            per_brush.get_mut(brush as usize),
            scene.geometry.face_index_ranges.get(face),
        ) else {
            continue;
        };
        let start = range.index_offset as usize;
        let indices = &geometry.indices[start..start + range.index_count as usize];
        let triangles: Vec<[DVec3; 3]> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&triangle| triangle.map(position))
            .collect();
        // Fanned counter-clockwise around the outward normal.
        let winding: DVec3 = triangles
            .iter()
            .map(|[a, b, c]| (*b - *a).cross(*c - *a))
            .sum();
        let Some(normal) = winding.try_normalize() else {
            continue;
        };
        if normal.dot(basis.normal) > UNDERSIDE_MAX_NORMAL_DOT {
            continue;
        }
        let corners = || triangles.iter().flatten().copied();
        slot.push(DownwardFace {
            bounds: Rect::around(corners().map(|p| basis.project(p))),
            top: corners()
                .map(|p| basis.height(p))
                .fold(f64::NEG_INFINITY, f64::max),
            triangles,
        });
    }
    per_brush
}

/// Whether any of a brush's downward faces overhangs any bound water face.
fn has_underside(faces: &[DownwardFace], water: &[WaterFace], basis: &PlaneBasis) -> bool {
    faces
        .iter()
        .any(|face| water.iter().any(|w| overhangs(face, w, basis)))
}

/// Whether the face's part above the water plane overlaps the water face in
/// projection along the normal with positive area. The overlap must be wider
/// on average (area over its longest span) than [`PLANE_TOLERANCE_M`], so a
/// face meeting the water only along an edge, f32 rounding aside, does not
/// count. "Above" starts that tolerance over the plane.
fn overhangs(face: &DownwardFace, water: &WaterFace, basis: &PlaneBasis) -> bool {
    let floor = water.height + PLANE_TOLERANCE_M;
    if face.top <= floor || !face.bounds.overlaps(&water.bounds) {
        return false;
    }
    let mut area = 0.0;
    let mut points: Vec<DVec2> = Vec::new();
    for triangle in &face.triangles {
        let above = clip_above(triangle, basis, floor);
        let mut window: Vec<DVec2> = above.iter().map(|&p| basis.project(p)).collect();
        let window_area = signed_area(&window);
        if window_area.abs() <= f64::EPSILON {
            continue;
        }
        if window_area < 0.0 {
            window.reverse();
        }
        let overlap = clip_to_convex(&water.outline, &window);
        let overlap_area = signed_area(&overlap).abs();
        if overlap_area > 0.0 {
            area += overlap_area;
            points.extend(overlap);
        }
    }
    area > PLANE_TOLERANCE_M * diameter(&points)
}

/// The part of a triangle at or above `floor` along the normal: a convex
/// polygon of up to four points, or fewer than three when none lies above.
fn clip_above(triangle: &[DVec3; 3], basis: &PlaneBasis, floor: f64) -> Vec<DVec3> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let (p, q) = (triangle[i], triangle[(i + 1) % 3]);
        let (dp, dq) = (basis.height(p) - floor, basis.height(q) - floor);
        if dp >= 0.0 {
            out.push(p);
        }
        if (dp >= 0.0) != (dq >= 0.0) {
            out.push(p + (q - p) * (dp / (dp - dq)));
        }
    }
    out
}

/// Sutherland–Hodgman: `subject`, any simple polygon, clipped to the convex
/// counter-clockwise `window`. A concave subject may come back with
/// zero-width bridges, which add no area.
fn clip_to_convex(subject: &[DVec2], window: &[DVec2]) -> Vec<DVec2> {
    let mut out = subject.to_vec();
    for (i, &a) in window.iter().enumerate() {
        let edge = window[(i + 1) % window.len()] - a;
        let input = std::mem::take(&mut out);
        for (j, &p) in input.iter().enumerate() {
            let q = input[(j + 1) % input.len()];
            let (dp, dq) = (edge.perp_dot(p - a), edge.perp_dot(q - a));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                out.push(p + (q - p) * (dp / (dp - dq)));
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Shoelace area, positive counter-clockwise.
fn signed_area(polygon: &[DVec2]) -> f64 {
    let n = polygon.len();
    if n < 3 {
        return 0.0;
    }
    0.5 * (0..n)
        .map(|i| polygon[i].perp_dot(polygon[(i + 1) % n]))
        .sum::<f64>()
}

/// The longest distance between any two of `points`.
fn diameter(points: &[DVec2]) -> f64 {
    let mut longest: f64 = 0.0;
    for (i, &p) in points.iter().enumerate() {
        for &q in &points[i + 1..] {
            longest = longest.max(p.distance(q));
        }
    }
    longest
}
