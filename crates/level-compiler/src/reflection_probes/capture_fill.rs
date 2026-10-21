// Reflection-probe capture fill: a kept brush's covering side, read where CSG removed it against an excluded brush.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_exclusion::{BrushFaces, CONTACT_TOLERANCE_M};
use super::hit::{SceneTracer, SurfaceHit, SurfacePoint};
use crate::geometry::{GeometryResult, brush_side_texel_uv};
use crate::map_data::{BrushSide, BrushVolume};

/// How far along the ray a covering side may be crossed before the ray
/// enters an excluded hull or after it leaves, and still fill; also the
/// plane-match and in-polygon slack. Brush CSG cuts the kept face across a
/// gap within its contact epsilon, so a side just past the hull's exit is
/// the one the excluded brush removed. Measured along the ray, not off the
/// plane, so a grazing ray never reaches far past the exit.
const FILL_TOLERANCE_M: f64 = CONTACT_TOLERANCE_M;
/// Normals within this cosine of each other are one plane's.
const COPLANAR_MIN_COS: f64 = 1.0 - 1.0e-4;

/// A kept brush an excluded one touches, with each side's surviving
/// coplanar post-cut faces.
#[derive(Debug, Clone)]
pub(super) struct KeptCandidate {
    pub brush: u32,
    /// Parallel to the brush's `sides`.
    sides: Vec<CoveringSide>,
}

#[derive(Debug, Clone, Default)]
struct CoveringSide {
    /// Surviving post-cut faces of the brush on this side's plane, ascending.
    faces: Vec<u32>,
    /// The side's texture in the level's texture table.
    texture_index: Option<u32>,
}

/// A ray that crossed an excluded hull reads a kept brush's covering side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fill {
    /// From the ray origin to the fill point, on the covering side's plane.
    pub distance: f32,
    /// The covering side (`BrushVolume::sides` of `brush`) whose material
    /// and texture projection the fill reads.
    pub brush: u32,
    pub side: u32,
    pub texture_index: u32,
    /// The nearest point to the fill on the pooled surviving faces, whose
    /// lightmap, animated block, texel density, SH and sdf light it reads.
    /// Its `distance` is the fill's.
    pub lit: SurfaceHit,
}

impl KeptCandidate {
    /// One candidate per brush of `kept` (sorted), each side matched to the
    /// post-cut faces of that brush lying on its plane.
    pub(super) fn build(
        volumes: &[BrushVolume],
        brush_faces: &BrushFaces,
        geometry: &GeometryResult,
        kept: &[u32],
    ) -> Vec<Self> {
        let names = &geometry.texture_names.names;
        kept.iter()
            .map(|&brush| {
                let brush_sides = &volumes[brush as usize].sides;
                let mut sides: Vec<CoveringSide> = brush_sides
                    .iter()
                    .map(|side| CoveringSide {
                        faces: Vec::new(),
                        texture_index: names
                            .iter()
                            .position(|name| *name == side.texture)
                            .map(|i| i as u32),
                    })
                    .collect();
                // The brush's faces arrive ascending, so each side's do.
                for &face in brush_faces.of(brush) {
                    let Some((normal, distance)) = face_plane(geometry, face as usize) else {
                        continue;
                    };
                    for (side, covering) in brush_sides.iter().zip(&mut sides) {
                        if side.normal.dot(normal) >= COPLANAR_MIN_COS
                            && (side.distance - distance).abs() <= FILL_TOLERANCE_M
                        {
                            covering.faces.push(face);
                        }
                    }
                }
                Self { brush, sides }
            })
            .collect()
    }
}

/// Where the ray `(origin, unit direction)`, inside an excluded hull between
/// `entry` and `exit` (its distances), first crosses into one of the `slots`
/// candidates through a side whose polygon holds the crossing. Every
/// candidate side on that crossing's plane pools its surviving faces, so a
/// brush that lost the coplanar dedupe never decides; the fill reads the
/// nearest point on them, ties to the lowest post-cut face. `None` when no
/// side is crossed, or the first crossed has no surviving face: the exit
/// decides instead. It runs per excluded hull per capture ray, so it
/// allocates nothing.
pub(super) fn find_fill(
    tracer: &SceneTracer<'_>,
    volumes: &[BrushVolume],
    candidates: &[KeptCandidate],
    slots: &[usize],
    ray: (DVec3, DVec3),
    [entry, exit]: [f64; 2],
) -> Option<Fill> {
    let crossings = || {
        slots.iter().flat_map(move |&slot| {
            let sides = &volumes[candidates[slot].brush as usize].sides;
            sides.iter().enumerate().filter_map(move |(index, side)| {
                Some((crossing(side, ray, [entry, exit])?, slot, index))
            })
        })
    };
    let first = crossings().map(|(t, ..)| t).min_by(f64::total_cmp)?;
    let (origin, direction) = ray;
    let point = origin + direction * first;
    // Every crossed side on the first crossing's plane pools its faces. A
    // face belongs to one brush's one side, so none is visited twice.
    let mut best: Option<Nearest> = None;
    for (_, slot, index) in crossings() {
        let side = &volumes[candidates[slot].brush as usize].sides[index];
        if (side.normal.dot(point) - side.distance).abs() > FILL_TOLERANCE_M {
            continue;
        }
        for &face in &candidates[slot].sides[index].faces {
            nearest_on_face(tracer, face, point, (slot, index), &mut best);
        }
    }
    let Nearest {
        hit: lit,
        slot,
        side: index,
        ..
    } = best?;
    let candidate = &candidates[slot];
    let texture_index = candidate.sides[index]
        .texture_index
        .unwrap_or_else(|| tracer.geometry().geometry.faces[lit.face as usize].texture_index);
    let distance = first as f32;
    Some(Fill {
        distance,
        brush: candidate.brush,
        side: index as u32,
        texture_index,
        lit: SurfaceHit { distance, ..lit },
    })
}

/// Where the ray crosses into `side`'s brush through `side`: within
/// [`FILL_TOLERANCE_M`] of the hull's `entry`..`exit` along the ray, inside
/// the side's polygon. `None` when the ray heads out through the side or
/// crosses it elsewhere.
fn crossing(
    side: &BrushSide,
    (origin, direction): (DVec3, DVec3),
    [entry, exit]: [f64; 2],
) -> Option<f64> {
    let rate = side.normal.dot(direction);
    if rate >= 0.0 {
        return None;
    }
    let t = (side.distance - side.normal.dot(origin)) / rate;
    let within = t >= entry - FILL_TOLERANCE_M && t <= exit + FILL_TOLERANCE_M;
    (within && polygon_holds(side, origin + direction * t)).then_some(t)
}

/// The surface a fill reads, facing a ray of unit `direction` from
/// `origin`: the lit face point's lighting attributes and position, so SH
/// and sdf light are evaluated on the kept surface, never inside the
/// excluded hull; the covering side's material at the fill point's UV.
pub(crate) fn fill_surface(
    tracer: &SceneTracer<'_>,
    volumes: &[BrushVolume],
    fill: &Fill,
    origin: Vec3,
    direction: Vec3,
) -> SurfacePoint {
    let side = &volumes[fill.brush as usize].sides[fill.side as usize];
    let point = origin.as_dvec3() + direction.as_dvec3() * f64::from(fill.distance);
    let mut surface = tracer.surface_at(&fill.lit, tracer.position_on(&fill.lit), direction);
    surface.uv = brush_side_texel_uv(point, side.normal, &side.tex_projection);
    surface.texture_index = fill.texture_index;
    surface
}

/// The pooled face point nearest a fill so far: its squared distance, the
/// hit on its triangle, and the candidate slot and side that pooled it.
struct Nearest {
    squared: f64,
    hit: SurfaceHit,
    slot: usize,
    side: usize,
}

/// Fold the point of `face` nearest `point` into `best`. A nearer point
/// replaces it, and an equally near one only from a lower face, so the
/// result never depends on the order faces are visited: ties go to the
/// lowest index, and within a face to its first triangle.
fn nearest_on_face(
    tracer: &SceneTracer<'_>,
    face: u32,
    point: DVec3,
    (slot, side): (usize, usize),
    best: &mut Option<Nearest>,
) {
    let geometry = &tracer.geometry().geometry;
    let range = tracer.geometry().face_index_ranges[face as usize];
    let start = range.index_offset as usize;
    let (triangles, _) =
        geometry.indices[start..start + range.index_count as usize].as_chunks::<3>();
    for &corners in triangles {
        let [a, b, c] =
            corners.map(|i| Vec3::from(geometry.vertices[i as usize].position).as_dvec3());
        let (nearest, barycentric) = closest_point_on_triangle(point, a, b, c);
        let squared = nearest.distance_squared(point);
        let replaces = best.as_ref().is_none_or(|best| {
            squared < best.squared || (squared == best.squared && face < best.hit.face)
        });
        if replaces {
            *best = Some(Nearest {
                squared,
                hit: SurfaceHit {
                    distance: 0.0,
                    face,
                    triangle: corners,
                    barycentric: barycentric.map(|w| w as f32),
                },
                slot,
                side,
            });
        }
    }
}

/// The point of triangle `abc` nearest `p`, with its barycentrics
/// (Ericson, Real-Time Collision Detection §5.1.5).
fn closest_point_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, [f64; 3]) {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, [1.0 - v, v, 0.0]);
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// Whether `point`, on or near `side`'s plane, lies inside its convex
/// polygon within [`FILL_TOLERANCE_M`], whichever way it winds.
fn polygon_holds(side: &BrushSide, point: DVec3) -> bool {
    let vertices = &side.vertices;
    if vertices.len() < 3 {
        return false;
    }
    let centroid = vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    vertices.iter().enumerate().all(|(i, &a)| {
        let b = vertices[(i + 1) % vertices.len()];
        let edge = b - a;
        let mut inward = side.normal.cross(edge);
        let length = inward.length();
        if length < 1e-12 {
            return true;
        }
        inward /= length;
        if inward.dot(centroid - a) < 0.0 {
            inward = -inward;
        }
        inward.dot(point - a) >= -FILL_TOLERANCE_M
    })
}

/// The unit normal and plane distance of `face`, from its first
/// non-degenerate triangle.
fn face_plane(geometry: &GeometryResult, face: usize) -> Option<(DVec3, f64)> {
    let range = geometry.face_index_ranges[face];
    let start = range.index_offset as usize;
    let section = &geometry.geometry;
    section.indices[start..start + range.index_count as usize]
        .as_chunks::<3>()
        .0
        .iter()
        .find_map(|tri| {
            let [a, b, c] =
                tri.map(|i| Vec3::from(section.vertices[i as usize].position).as_dvec3());
            let normal = (b - a).cross(c - a).try_normalize()?;
            Some((normal, normal.dot(a)))
        })
}
