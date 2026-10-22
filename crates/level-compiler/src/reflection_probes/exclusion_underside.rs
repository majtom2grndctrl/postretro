// Reflection-probe capture exclusion's underside rule: downward faces over a probe's bound water.
// See: context/lib/resource_management.md §5.2

use glam::{DVec2, DVec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;

use super::exclusion::ExclusionScene;
use super::placement::{FaceGeometry, PLANE_TOLERANCE_M, PlacedProbe};

/// An orthonormal frame of the reflector: heights along its normal, 2D
/// coordinates in its plane.
pub(super) struct PlaneBasis {
    pub(super) normal: DVec3,
    u: DVec3,
    v: DVec3,
}

impl PlaneBasis {
    pub(super) fn new(normal: DVec3) -> Self {
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
pub(super) struct WaterFace {
    height: f64,
    outline: Vec<DVec2>,
    bounds: Rect,
}

/// The probe's bound water faces; a face id 52 does not hold is skipped.
pub(super) fn bound_water(
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
pub(super) struct DownwardFace {
    triangles: Vec<[DVec3; 3]>,
    bounds: Rect,
    /// Its highest point along the normal.
    top: f64,
}

/// Each brush's surviving downward faces, through the face → brush table: a
/// face points down when n·N ≤ `max_normal_dot`.
pub(super) fn downward_faces(
    scene: &ExclusionScene<'_>,
    basis: &PlaneBasis,
    max_normal_dot: f64,
) -> Vec<Vec<DownwardFace>> {
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
        if normal.dot(basis.normal) > max_normal_dot {
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
pub(super) fn has_underside(
    faces: &[DownwardFace],
    water: &[WaterFace],
    basis: &PlaneBasis,
) -> bool {
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
