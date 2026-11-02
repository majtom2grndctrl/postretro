// Tracer test fixtures: triangle geometry, every tracer entry point, and the retired hit test.
// See: context/lib/build_pipeline.md §Ray traversal

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::sh_bake::RaytracingCtx;

/// Geometry with one face per triangle. Corners given bit-identical in two
/// triangles are bit-identical in the geometry.
pub(super) fn triangles_geometry(triangles: &[[Vec3; 3]]) -> GeometryResult {
    let mut vertices = Vec::with_capacity(triangles.len() * 3);
    let mut faces = Vec::with_capacity(triangles.len());
    let mut face_index_ranges = Vec::with_capacity(triangles.len());
    for (i, triangle) in triangles.iter().enumerate() {
        vertices.extend(triangle.iter().map(|p| {
            Vertex::new(
                p.to_array(),
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            )
        }));
        faces.push(FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        });
        face_index_ranges.push(FaceIndexRange {
            index_offset: (i * 3) as u32,
            index_count: 3,
        });
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: (0..triangles.len() as u32 * 3).collect(),
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    }
}

/// A geometry with the bake tree every tracer walks.
pub(super) struct Scene {
    pub(super) geometry: GeometryResult,
    pub(super) bvh: Bvh<f32, 3>,
    pub(super) primitives: Vec<BvhPrimitive>,
}

impl Scene {
    pub(super) fn new(triangles: &[[Vec3; 3]]) -> Self {
        let geometry = triangles_geometry(triangles);
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH");
        Self {
            geometry,
            bvh,
            primitives,
        }
    }

    fn ctx(&self) -> RaytracingCtx<'_> {
        RaytracingCtx {
            bounce_materials: None,
            bvh: &self.bvh,
            primitives: &self.primitives,
            geometry: &self.geometry,
        }
    }

    /// Which entry points report a surface on the segment `from → to`, each
    /// through its own segment or ray construction. `front` names the
    /// front-filtered ones (lead-in, reflection-probe primary ray), which
    /// count only a triangle whose front faces the ray.
    pub(super) fn entry_points_hit(&self, from: Vec3, to: Vec3) -> Vec<(&'static str, bool)> {
        let length = (to - from).length();
        let dir = (to - from) / length;
        let ctx = self.ctx();
        let probes =
            crate::reflection_probes::SceneTracer::new(&self.bvh, &self.primitives, &self.geometry);
        vec![
            (
                "lightmap segment_clear",
                !super::segment_clear(&self.bvh, &self.primitives, &self.geometry, from, to),
            ),
            (
                "sh segment_clear",
                !crate::sh_bake::segment_clear(&ctx, from, to),
            ),
            (
                "chunk light list segment_clear",
                !crate::chunk_light_list_bake::segment_clear(
                    &self.bvh,
                    &self.primitives,
                    &self.geometry,
                    &point_light(from),
                    to,
                ),
            ),
            (
                "scatter segment_clear",
                !crate::billboard_direct_scatter_bake::segment_clear(&ctx, from, to),
            ),
            (
                "sh gather closest hit",
                crate::sh_bake::gather_hit(&ctx, from, dir)
                    .is_some_and(|hit| hit.distance < length),
            ),
            (
                "front: lead-in",
                !crate::texel_footprint::front_faces_clear(
                    &self.bvh,
                    &self.primitives,
                    &self.geometry,
                    from,
                    to,
                    |_| true,
                ),
            ),
            (
                "front: reflection-probe primary ray",
                probes.closest_hit(from, dir, length).is_some(),
            ),
        ]
    }
}

/// A hard point light at `origin`, as the chunk light list traces it.
pub(super) fn point_light(origin: Vec3) -> MapLight {
    MapLight {
        origin: origin.as_dvec3(),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 1.0e5,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// The retired hit test, kept only to show what the watertight one fixes:
/// `f32` Möller–Trumbore, two-sided, rejecting `|det| < det_floor`.
pub(super) fn moller_trumbore(
    origin: Vec3,
    dir: Vec3,
    [a, b, c]: [Vec3; 3],
    det_floor: f32,
) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < det_floor {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
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
    (t > 0.0).then_some(t)
}

/// The retired test's `det` floor.
pub(super) const MT_DET_FLOOR: f32 = 1.0e-8;

/// `(a, b)`'s orthonormal complement pair, for building rays and apexes
/// around an axis.
pub(super) fn axis_frame(axis: usize) -> (DVec3, DVec3, DVec3) {
    let along = DVec3::AXES[axis];
    (
        along,
        DVec3::AXES[(axis + 1) % 3],
        DVec3::AXES[(axis + 2) % 3],
    )
}

/// xorshift64: deterministic, no dependency.
pub(super) struct XorShift(pub(super) u64);

impl XorShift {
    pub(super) fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(super) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub(super) fn log_range(&mut self, lo: f64, hi: f64) -> f64 {
        10f64.powf(self.range(lo, hi))
    }

    pub(super) fn index(&mut self, len: usize) -> usize {
        ((self.unit() * len as f64) as usize).min(len - 1)
    }

    pub(super) fn sign(&mut self) -> f64 {
        if self.unit() < 0.5 { -1.0 } else { 1.0 }
    }

    pub(super) fn unit_vec(&mut self) -> DVec3 {
        loop {
            let v = DVec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let len = v.length();
            if len > 0.1 && len <= 1.0 {
                return v / len;
            }
        }
    }
}
