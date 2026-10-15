// Reflection-probe closest hit: the static-world triangle a capture ray reaches, and its surface attributes.
// See: context/lib/build_pipeline.md §Ray traversal · resource_management.md §5

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::octahedral;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::ray_traversal::BoundedRay;

/// Hits nearer than this are the ray's own origin surface, as in the SH bake.
const RAY_EPSILON: f32 = 1.0e-3;

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
    /// Weights of `triangle`'s vertices; they sum to 1.
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
        let ray = Ray::new(
            Point3::new(origin.x, origin.y, origin.z),
            Vector3::new(direction.x, direction.y, direction.z),
        );
        let query = BoundedRay::new(&ray, max_distance);
        let geom = &self.geometry.geometry;
        let mut best: Option<(f32, usize, [f32; 3], u32)> = None;
        for prim in self.bvh.traverse_iterator(&query, self.primitives) {
            let start = prim.index_offset as usize;
            let end = start + prim.index_count as usize;
            let mut tri = start;
            while tri + 3 <= end {
                let corners = [
                    geom.indices[tri],
                    geom.indices[tri + 1],
                    geom.indices[tri + 2],
                ];
                let [p0, p1, p2] = corners.map(|i| Vec3::from(geom.vertices[i as usize].position));
                if let Some((t, u, v)) = front_face_hit(origin, direction, p0, p1, p2)
                    && t > RAY_EPSILON
                    && t < max_distance
                    && best.is_none_or(|(d, ..)| t < d)
                {
                    best = Some((t, tri, [1.0 - u - v, u, v], prim.index_offset));
                    query.shrink_to(t);
                }
                tri += 3;
            }
        }
        let (distance, tri, barycentric, index_offset) = best?;
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
        crate::lightmap_bake::segment_clear(self.bvh, self.primitives, self.geometry, from, to)
    }

    fn face_for_offset(&self, index_offset: u32) -> Option<u32> {
        self.face_by_offset
            .binary_search_by_key(&index_offset, |&(offset, _)| offset)
            .ok()
            .map(|slot| self.face_by_offset[slot].1)
    }

    /// Interpolated attributes at `hit` along a ray of unit `direction` from
    /// `origin`.
    pub(crate) fn surface(&self, hit: &SurfaceHit, origin: Vec3, direction: Vec3) -> SurfacePoint {
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
            position: origin + direction * hit.distance,
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

/// Möller–Trumbore against the triangle's front side only: `(t, u, v)` where
/// `u` and `v` weight `p1` and `p2`. Front is counter-clockwise, the winding
/// geometry extraction fans faces in around their normal.
fn front_face_hit(
    origin: Vec3,
    dir: Vec3,
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
) -> Option<(f32, f32, f32)> {
    let edge1 = p1 - p0;
    let edge2 = p2 - p0;
    if edge1.cross(edge2).dot(dir) >= 0.0 {
        return None;
    }
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < 1.0e-8 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - p0;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    (t > 0.0).then_some((t, u, v))
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
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::reflection_probes::reflection_probes_test_fixtures::{RoomFace, box_room};

    #[test]
    fn closest_hit_returns_the_floor_face_uv_and_barycentrics_below_a_point() {
        let room = box_room([-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]);
        let (bvh, primitives, _) = build_bvh(&room).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &room);

        let origin = Vec3::new(0.5, 1.0, -0.25);
        let down = Vec3::NEG_Y;
        let hit = tracer
            .closest_hit(origin, down, 100.0)
            .expect("the floor is below");
        assert_eq!(hit.face, RoomFace::Floor as u32);
        assert!((hit.distance - 1.0).abs() < 1e-5, "{}", hit.distance);
        let weights: f32 = hit.barycentric.iter().sum();
        assert!((weights - 1.0).abs() < 1e-5);
        assert!(hit.barycentric.iter().all(|w| (0.0..=1.0).contains(w)));

        let surface = tracer.surface(&hit, origin, down);
        assert!((surface.position - Vec3::new(0.5, 0.0, -0.25)).length() < 1e-5);
        assert!((surface.normal - Vec3::Y).length() < 1e-3);
        assert_eq!(surface.texture_index, RoomFace::Floor as u32);
        // The fixture maps 1 m to 32 texels from the room's min corner.
        assert!(
            (surface.uv[0] - 2.5 * 32.0).abs() < 1e-3,
            "{:?}",
            surface.uv
        );
        assert!(
            (surface.uv[1] - 1.75 * 32.0).abs() < 1e-3,
            "{:?}",
            surface.uv
        );
        assert!((surface.texel_density - 32.0).abs() < 1e-2);
    }

    #[test]
    fn closest_hit_culls_back_faces_as_the_world_pass_does() {
        let room = box_room([-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]);
        let (bvh, primitives, _) = build_bvh(&room).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &room);

        // From below the floor, looking up: the floor shows its back to the
        // ray, so the ray passes it and lands on the ceiling's front.
        let hit = tracer
            .closest_hit(Vec3::new(0.0, -1.0, 0.0), Vec3::Y, 100.0)
            .expect("the ceiling faces the ray");
        assert_eq!(hit.face, RoomFace::Ceiling as u32);
        assert!((hit.distance - 4.0).abs() < 1e-5, "{}", hit.distance);
        // And from above the ceiling, looking down, the floor.
        let hit = tracer
            .closest_hit(Vec3::new(0.0, 4.0, 0.0), Vec3::NEG_Y, 100.0)
            .expect("the floor faces the ray");
        assert_eq!(hit.face, RoomFace::Floor as u32);
        assert!((hit.distance - 4.0).abs() < 1e-5, "{}", hit.distance);
    }
}
