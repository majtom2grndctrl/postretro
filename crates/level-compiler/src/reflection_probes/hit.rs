// Reflection-probe closest hit: the static-world triangle a capture ray reaches, and its surface attributes.
// See: context/lib/build_pipeline.md §Ray traversal · resource_management.md §5

use bvh::bvh::Bvh;
use glam::Vec3;
use postretro_level_format::octahedral;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::ray_traversal::{
    TraceRay, closest_point_on_triangle, primitive_triangles, triangle_points,
};

/// Hits nearer than this are the ray's own origin surface: the tracer's
/// shared offset, re-exported so `hit::RAY_EPSILON` paths resolve.
pub(crate) use crate::ray_traversal::RAY_EPSILON;

/// The static world as the capture sees it: the compiler BVH over every
/// static face. Movers and environment volumes are not in it; `switch`
/// brushes join the static world brush set, so they are.
pub(crate) struct SceneTracer<'a> {
    bvh: &'a Bvh<f32, 3>,
    primitives: &'a [BvhPrimitive],
    geometry: &'a GeometryResult,
    /// `(index_offset, face)` sorted by offset: a BVH primitive is one face's
    /// index range, so its offset names the face.
    face_by_offset: Vec<(u32, u32)>,
}

/// The front face a ray reached, with the triangle and barycentrics that
/// interpolate its vertex attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SurfaceHit {
    pub distance: f32,
    pub face: u32,
    /// Vertex indices of the hit triangle, in index-buffer order.
    pub triangle: [u32; 3],
    /// Weights of `triangle`'s vertices, each in `[0, 1]`; they sum to 1.
    /// A traced hit's name the point on the triangle nearest where the ray
    /// crossed its plane.
    pub barycentric: [f32; 3],
}

/// Vertex attributes at a hit, interpolated as the rasterizer would.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SurfacePoint {
    pub position: Vec3,
    /// The face's unit shading normal, facing the ray.
    pub normal: Vec3,
    pub texture_index: u32,
    /// Base UV in raw texel units; the renderer divides by the diffuse size.
    pub uv: [f32; 2],
    /// Block-local lightmap UV in `[0, 1]`.
    pub lightmap_uv: [f32; 2],
    /// Id-22 block id + 1; 0 means no lightmap.
    pub lightmap_block: u16,
    /// Id-25 animated block id + 1; 0 means the face has no animated direct.
    pub animated_block: u16,
    /// Base-texture texels per meter on this triangle, for mip selection.
    pub texel_density: f32,
}

/// The faces one probe's primary rays skip, as the index offsets of their
/// BVH primitives, sorted: a primitive is one face's index range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FaceMask {
    offsets: Vec<u32>,
}

impl FaceMask {
    pub(crate) fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }
}

/// A masked trace: the nearest kept front face, and the nearest skipped one
/// in front of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MaskedHit {
    pub hit: Option<SurfaceHit>,
    pub skipped: Option<SurfaceHit>,
}

/// Whether a trace skips a BVH primitive, by its index offset.
trait PrimitiveMask {
    fn skips(&self, index_offset: u32) -> bool;
}

/// The whole world: shadow rays, the box fit and an unfiltered capture.
struct Unmasked;

impl PrimitiveMask for Unmasked {
    #[inline(always)]
    fn skips(&self, _: u32) -> bool {
        false
    }
}

impl PrimitiveMask for FaceMask {
    #[inline]
    fn skips(&self, index_offset: u32) -> bool {
        self.offsets.binary_search(&index_offset).is_ok()
    }
}

/// `(t, first index of the triangle, barycentrics, primitive index offset)`.
type Candidate = (f32, usize, [f32; 3], u32);

impl<'a> SceneTracer<'a> {
    pub(crate) fn new(
        bvh: &'a Bvh<f32, 3>,
        primitives: &'a [BvhPrimitive],
        geometry: &'a GeometryResult,
    ) -> Self {
        let mut face_by_offset: Vec<(u32, u32)> = geometry
            .face_index_ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| range.index_count > 0)
            .map(|(face, range)| (range.index_offset, face as u32))
            .collect();
        face_by_offset.sort_unstable();
        Self {
            bvh,
            primitives,
            geometry,
            face_by_offset,
        }
    }

    /// The nearest front-facing triangle within `max_distance`. Back faces are
    /// skipped, as the world pass culls them (`cull_mode: Back`, CCW front), so
    /// a ray that grazes a face's back sees what lies beyond it.
    pub(crate) fn closest_hit(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<SurfaceHit> {
        let (best, _) = self.nearest(origin, direction, RAY_EPSILON, max_distance, &Unmasked);
        best.and_then(|candidate| self.surface_hit(candidate))
    }

    /// [`Self::closest_hit`] past `min_distance` with `mask`'s faces skipped,
    /// and beside it the nearest skipped front face in front of that hit (or
    /// anywhere within `max_distance` on a miss). A re-trace from an
    /// excluded hull's exit passes the exit's distance as `min_distance`, so
    /// a kept face just past the exit is never stepped over.
    pub(crate) fn closest_hit_masked(
        &self,
        origin: Vec3,
        direction: Vec3,
        min_distance: f32,
        max_distance: f32,
        mask: &FaceMask,
    ) -> MaskedHit {
        let (best, skipped) = self.nearest(origin, direction, min_distance, max_distance, mask);
        MaskedHit {
            hit: best.and_then(|candidate| self.surface_hit(candidate)),
            skipped: skipped.and_then(|candidate| self.surface_hit(candidate)),
        }
    }

    /// The faces' mask: the BVH primitives of `faces`, by index offset.
    /// Faces with no triangles own no primitive.
    pub(crate) fn face_mask(&self, faces: impl IntoIterator<Item = u32>) -> FaceMask {
        let ranges = &self.geometry.face_index_ranges;
        let mut offsets: Vec<u32> = faces
            .into_iter()
            .filter_map(|face| ranges.get(face as usize))
            .filter(|range| range.index_count > 0)
            .map(|range| range.index_offset)
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        FaceMask { offsets }
    }

    /// The nearest unmasked front-face hit in `(min_distance, max_distance)`,
    /// and the nearest masked one in front of it. A masked primitive's
    /// triangles never shrink the query, so the traversal still reaches
    /// every unmasked face behind them. Monomorphized over [`Unmasked`], the
    /// skipped branch folds away and the unmasked trace costs what it did.
    fn nearest<M: PrimitiveMask>(
        &self,
        origin: Vec3,
        direction: Vec3,
        min_distance: f32,
        max_distance: f32,
        mask: &M,
    ) -> (Option<Candidate>, Option<Candidate>) {
        #[cfg(test)]
        crate::bake_counters::begin_ray(crate::bake_counters::RayKind::Gather);
        let ray = TraceRay::new(origin, direction);
        let query = ray.query(max_distance);
        let mut best: Option<Candidate> = None;
        let mut skipped: Option<Candidate> = None;
        for prim in self.bvh.traverse_iterator(&query, self.primitives) {
            #[cfg(test)]
            crate::bake_counters::bump_trace(|trace| trace.leaf_visits += 1);
            let masked = mask.skips(prim.index_offset);
            for tri in primitive_triangles(prim) {
                let points = triangle_points(self.geometry, tri);
                if let Some(hit) = ray.hit(points, min_distance, max_distance)
                    && hit.front
                {
                    // Distances are kept in `f32`, and a re-trace passes a
                    // kept distance back as `min_distance`: bound the `f32`
                    // value too, so a hit at that distance is never found
                    // again past it.
                    let t = hit.t as f32;
                    if t > min_distance && t < max_distance && best.is_none_or(|(d, ..)| t < d) {
                        let weights = surface_weights(&ray, points, hit.t, hit.barycentric);
                        let candidate = (t, tri, weights, prim.index_offset);
                        if !masked {
                            best = Some(candidate);
                            query.shrink_to(t);
                        } else if skipped.is_none_or(|(d, ..)| t < d) {
                            skipped = Some(candidate);
                        }
                    }
                }
            }
        }
        // A masked face found before a nearer kept one lies behind it.
        let skipped = skipped.filter(|&(s, ..)| best.is_none_or(|(d, ..)| s < d));
        (best, skipped)
    }

    fn surface_hit(
        &self,
        (distance, tri, barycentric, index_offset): Candidate,
    ) -> Option<SurfaceHit> {
        let geom = &self.geometry.geometry;
        let face = self.face_for_offset(index_offset)?;
        Some(SurfaceHit {
            distance,
            face,
            triangle: [
                geom.indices[tri],
                geom.indices[tri + 1],
                geom.indices[tri + 2],
            ],
            barycentric,
        })
    }

    /// Whether nothing in the static world blocks the segment `from` → `to`,
    /// either face of a triangle counting: the lightmap bake's visibility test.
    pub(crate) fn segment_clear(&self, from: Vec3, to: Vec3) -> bool {
        crate::ray_traversal::segment_clear(self.bvh, self.primitives, self.geometry, from, to)
    }

    fn face_for_offset(&self, index_offset: u32) -> Option<u32> {
        self.face_by_offset
            .binary_search_by_key(&index_offset, |&(offset, _)| offset)
            .ok()
            .map(|slot| self.face_by_offset[slot].1)
    }

    /// Interpolated attributes at `hit` along a ray of unit `direction` from
    /// `origin`. The position is the ray's own point at `hit.distance`, so
    /// position and distance agree; the attributes come from the nearest
    /// point on the triangle ([`Self::position_on`]). The two are the same
    /// point up to rounding, except on a tolerance hit, where the ray's point
    /// lies up to τ off the triangle in its plane: well inside any shading
    /// footprint, and the distance tile reads that point.
    pub(crate) fn surface(&self, hit: &SurfaceHit, origin: Vec3, direction: Vec3) -> SurfacePoint {
        self.surface_at(hit, origin + direction * hit.distance, direction)
    }

    /// The point on `hit`'s triangle its barycentrics name: for a traced hit,
    /// the point nearest where the ray crossed the triangle's plane.
    pub(crate) fn position_on(&self, hit: &SurfaceHit) -> Vec3 {
        let geom = &self.geometry.geometry;
        hit.triangle
            .iter()
            .zip(hit.barycentric)
            .map(|(&i, w)| Vec3::from(geom.vertices[i as usize].position) * w)
            .sum()
    }

    /// The static geometry the tracer reads.
    pub(crate) fn geometry(&self) -> &GeometryResult {
        self.geometry
    }

    /// [`Self::surface`] with the point's position given: `hit`'s triangle
    /// and barycentrics still interpolate every vertex attribute.
    pub(crate) fn surface_at(
        &self,
        hit: &SurfaceHit,
        position: Vec3,
        direction: Vec3,
    ) -> SurfacePoint {
        let geom = &self.geometry.geometry;
        let [v0, v1, v2] = hit.triangle.map(|i| &geom.vertices[i as usize]);
        let [w0, w1, w2] = hit.barycentric;
        let lerp2 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
            [
                a[0] * w0 + b[0] * w1 + c[0] * w2,
                a[1] * w0 + b[1] * w1 + c[1] * w2,
            ]
        };
        let lightmap_uv = |v: &postretro_level_format::geometry::Vertex| {
            [
                f32::from(v.lightmap_uv[0]) / 65535.0,
                f32::from(v.lightmap_uv[1]) / 65535.0,
            ]
        };
        // Faces are flat: every vertex carries the face plane's normal.
        let mut normal = Vec3::from(octahedral::decode(v0.normal_oct)).normalize_or_zero();
        if normal.dot(direction) > 0.0 {
            normal = -normal;
        }
        SurfacePoint {
            position,
            normal,
            texture_index: geom.faces[hit.face as usize].texture_index,
            uv: lerp2(v0.uv, v1.uv, v2.uv),
            lightmap_uv: lerp2(lightmap_uv(v0), lightmap_uv(v1), lightmap_uv(v2)),
            lightmap_block: v0.lightmap_block,
            animated_block: v0.animated_block,
            texel_density: texel_density(
                [v0.position, v1.position, v2.position],
                [v0.uv, v1.uv, v2.uv],
            ),
        }
    }
}

/// The corner weights a hit's attributes interpolate with: those of the
/// nearest point on the triangle `points` to where the ray crosses its plane
/// at `t`. A crossing inside keeps the hit test's own weights, renormalized.
/// A tolerance hit's crossing lies up to τ outside, where on a sliver its
/// weights lie far outside `[0, 1]`, so clamping them would name a point far
/// along the triangle. Its weights are the nearest point's (Ericson's
/// regions), at most τ from the crossing. Only probe rays pay for it, and
/// only on a tolerance hit.
fn surface_weights(ray: &TraceRay, points: [Vec3; 3], t: f64, barycentric: [f64; 3]) -> [f32; 3] {
    if barycentric.iter().any(|&w| w < 0.0) {
        // The crossing as the hit test's tolerance check computes it.
        let crossing = ray.origin().as_dvec3() + ray.direction().as_dvec3() * t;
        let (_, weights) = closest_point_on_triangle(crossing, points.map(Vec3::as_dvec3));
        return weights.map(|w| w as f32);
    }
    let clamped = barycentric.map(|w| w.max(0.0));
    let sum: f64 = clamped.iter().sum();
    clamped.map(|w| (w / sum) as f32)
}

/// Texels per meter from the triangle's UV-to-world area ratio. Texture
/// projections are uniform per face, so one ratio serves the whole face.
fn texel_density(positions: [[f32; 3]; 3], uvs: [[f32; 2]; 3]) -> f32 {
    let [p0, p1, p2] = positions.map(Vec3::from);
    let world_area = (p1 - p0).cross(p2 - p0).length();
    let (du1, dv1) = (uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]);
    let (du2, dv2) = (uvs[2][0] - uvs[0][0], uvs[2][1] - uvs[0][1]);
    let uv_area = (du1 * dv2 - dv1 * du2).abs();
    if world_area <= f32::EPSILON {
        return 0.0;
    }
    (uv_area / world_area).sqrt()
}

#[cfg(test)]
#[path = "hit_tests.rs"]
mod tests;
