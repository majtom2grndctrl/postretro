// Cone-frustum geometry for spotlight shadow culling (planes + enclosing AABB
// from a spotlight's light-space view-projection matrix) and world-space Aabb
// utilities: transformed enclosure, from_points, empty/expand, Pod/Zeroable —
// shared by both the cone-cull and entity bind-pose cull paths.
//
// See: context/lib/rendering_pipeline.md §7.1

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

/// Axis-aligned bounding box. World-space at the cone-cull sites; LOCAL (model)
/// space when carried as a per-model bound on a skinned mesh (see
/// `crate::model::mesh::SkinnedMesh::bounds`) — the per-light caster cull
/// transforms the local box by the instance transform before testing it against
/// a cone/face frustum, so one `Aabb` type serves the model bound and the
/// `aabb_intersects_frustum` predicate.
///
/// `Pod`/`Zeroable` so it rides on the Pod CPU model struct (glam's `bytemuck`
/// feature makes `Vec3` Pod). `#[repr(C)]` pins the `min`-then-`max` layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// An empty box: `min` at `+inf`, `max` at `-inf`, so the first
    /// [`Aabb::expand`] adopts that point exactly. Folding over no points leaves
    /// it inverted (degenerate), which [`Aabb::from_points`] collapses to a zero box.
    pub(crate) fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    /// Grow the box to enclose `point`.
    pub(crate) fn expand(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// Enclose this box after `transform` is applied to it, returning the tight
    /// world-space AABB. Transforms all 8 local corners and takes their min/max,
    /// so an arbitrary rotation (or shear/scale) produces a correct axis-aligned
    /// enclosure rather than the wrong box a component-wise transform of just
    /// `min`/`max` would give. Used by the per-light entity caster cull: the
    /// instance's local (bind-pose) model bound is transformed by its world
    /// matrix before testing against a cone frustum.
    pub(crate) fn transformed(&self, transform: &Mat4) -> Aabb {
        let corners = [
            Vec3::new(self.min.x, self.min.y, self.min.z),
            Vec3::new(self.max.x, self.min.y, self.min.z),
            Vec3::new(self.min.x, self.max.y, self.min.z),
            Vec3::new(self.max.x, self.max.y, self.min.z),
            Vec3::new(self.min.x, self.min.y, self.max.z),
            Vec3::new(self.max.x, self.min.y, self.max.z),
            Vec3::new(self.min.x, self.max.y, self.max.z),
            Vec3::new(self.max.x, self.max.y, self.max.z),
        ];
        Self::from_points(corners.iter().map(|&c| transform.transform_point3(c)))
    }

    /// Build the tight AABB over `points`. An empty iterator yields a zero box
    /// (`min == max == origin`) rather than the inverted [`Aabb::empty`] sentinel,
    /// so a points-less mesh has a well-formed (if degenerate) bound.
    pub(crate) fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut aabb = Self::empty();
        for p in points {
            aabb.expand(p);
        }
        if aabb.min.x > aabb.max.x {
            return Self {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            };
        }
        aabb
    }
}

/// Extract the 6 cone-frustum planes from a spotlight's light-space
/// view-projection matrix (the one `light_space_matrix()` returns, with its
/// `fov_y`/`far`/`near` clamps baked in).
///
/// Delegates to [`extract_frustum_planes_for_gpu`] so the CPU cone-frustum path
/// and the GPU BVH-cull path share one plane-extraction implementation.
/// Convention (per that function, mirrored by the WGSL
/// `is_aabb_outside_frustum`): 6 planes from the combined matrix rows —
/// L,R,B,T,N,F = `r3+r0, r3-r0, r3+r1, r3-r1, r2, r3-r2` — normalized,
/// emitted as `[nx,ny,nz,d]`; a point `p` is *outside* a plane when
/// `dot(normal, p) + d < 0`.
pub fn cone_frustum_planes(light_space_matrix: &Mat4) -> [Vec4; 6] {
    let raw = extract_frustum_planes_for_gpu(light_space_matrix);
    raw.map(|p| Vec4::new(p[0], p[1], p[2], p[3]))
}

/// Extract the 6 frustum planes from a combined view-projection matrix in the
/// layout the cull WGSL (`bvh_cull.wgsl::is_aabb_outside_frustum`) consumes.
///
/// Convention: 6 planes from the combined matrix rows — L,R,B,T,N,F =
/// `r3+r0, r3-r0, r3+r1, r3-r1, r2, r3-r2` — normalized, emitted as
/// `[nx,ny,nz,d]`. Camera and shadow projections use WebGPU `[0, 1]` depth,
/// so the near plane is `r2` (`z_clip >= 0`) while far remains `r3-r2`
/// (`z_clip <= w_clip`). Inside-sign matches the WGSL: a point `p` is
/// *outside* a plane when `dot(normal, p) + d < 0`.
pub fn extract_frustum_planes_for_gpu(view_proj: &Mat4) -> [[f32; 4]; 6] {
    let row = |n: usize| -> glam::Vec4 {
        glam::Vec4::new(
            view_proj.col(0)[n],
            view_proj.col(1)[n],
            view_proj.col(2)[n],
            view_proj.col(3)[n],
        )
    };

    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);

    let raw_planes = [
        r3 + r0, // Left
        r3 - r0, // Right
        r3 + r1, // Bottom
        r3 - r1, // Top
        r2,      // Near, WebGPU z_clip >= 0
        r3 - r2, // Far
    ];

    let mut gpu_planes = [[0.0f32; 4]; 6];
    for (i, raw) in raw_planes.iter().enumerate() {
        let normal = glam::Vec3::new(raw.x, raw.y, raw.z);
        let length = normal.length();
        if length > 0.0 {
            let inv_len = 1.0 / length;
            let n = normal * inv_len;
            gpu_planes[i] = [n.x, n.y, n.z, raw.w * inv_len];
        }
    }
    gpu_planes
}

/// Test an AABB against a 6-plane frustum, returning `true` when the box is
/// inside or intersecting the frustum (i.e. *not* fully outside any plane).
///
/// Mirrors the WGSL `is_aabb_outside_frustum` in `bvh_cull.wgsl` exactly,
/// inverted: for each plane, pick the AABB corner furthest along the plane
/// normal (the "positive vertex") and test it. If that furthest corner is
/// behind a plane (`dot(normal, p) + d < 0`), the whole box is outside that
/// plane, hence outside the frustum.
///
/// Shared by rank-time culling (Task 2, cone AABB vs. camera frustum) and the
/// AC#2 unit test (cone planes vs. world AABB) — one CPU predicate, mirroring
/// the GPU convention so both provably agree.
pub fn aabb_intersects_frustum(aabb: &Aabb, planes: &[Vec4; 6]) -> bool {
    for plane in planes {
        let normal = plane.truncate();
        let d = plane.w;
        // Positive vertex: the AABB corner furthest along `normal`.
        let p = Vec3::new(
            if normal.x >= 0.0 {
                aabb.max.x
            } else {
                aabb.min.x
            },
            if normal.y >= 0.0 {
                aabb.max.y
            } else {
                aabb.min.y
            },
            if normal.z >= 0.0 {
                aabb.max.z
            } else {
                aabb.min.z
            },
        );
        if normal.dot(p) + d < 0.0 {
            return false;
        }
    }
    true
}

/// Compute the world-space AABB enclosing the spotlight's cone, derived from
/// the light-space view-projection matrix (so it carries the same
/// `fov_y`/`far`/`near` clamps as the rendered shadow projection).
///
/// Transforms the 8 NDC cube corners through the inverse of the light-space
/// matrix back into world space and takes their AABB. Building it from THAT
/// matrix — rather than re-deriving from raw `cone_angle_outer`/`falloff_range`
/// — guarantees the cull volume matches the rendered shadow frustum exactly.
///
/// NDC z spans `[0, 1]` because `light_space_matrix()` uses glam's
/// `perspective_rh` (Vulkan/D3D/Metal depth range), matching the cube corners
/// below. A non-invertible matrix (degenerate light) yields a point AABB at the
/// origin, which the AABB-vs-frustum predicate handles without panicking.
///
/// Retained as a cone-cull helper and exercised by the cone-frustum and
/// shadow-ranking regression tests; the camera-frustum pre-filter that once
/// called it in the forward path was removed (it could wrongly drop a shadow
/// whose cone reached a camera-visible receiver — see `SpotShadowPool::rank_lights`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn cone_enclosing_aabb(light_space_matrix: &Mat4) -> Aabb {
    let inv = light_space_matrix.inverse();

    // 8 corners of the NDC cube: x,y in [-1, 1], z in [0, 1].
    const NDC_CORNERS: [(f32, f32, f32); 8] = [
        (-1.0, -1.0, 0.0),
        (1.0, -1.0, 0.0),
        (-1.0, 1.0, 0.0),
        (1.0, 1.0, 0.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (-1.0, 1.0, 1.0),
        (1.0, 1.0, 1.0),
    ];

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (x, y, z) in NDC_CORNERS {
        let clip = Vec4::new(x, y, z, 1.0);
        let world = inv * clip;
        // Perspective divide back to world space.
        let w = if world.w.abs() > 1e-8 { world.w } else { 1.0 };
        let p = Vec3::new(world.x / w, world.y / w, world.z / w);
        min = min.min(p);
        max = max.max(p);
    }

    if !min.is_finite() || !max.is_finite() {
        return Aabb {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        };
    }

    Aabb { min, max }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::spot_shadow::light_space_matrix;
    use crate::prl::{FalloffModel, LightType, MapLight};

    /// Spotlight at the origin aimed down -Z, used as the cone under test.
    fn spot_down_neg_z() -> MapLight {
        MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    fn matrix_row(m: &Mat4, n: usize) -> Vec4 {
        Vec4::new(m.col(0)[n], m.col(1)[n], m.col(2)[n], m.col(3)[n])
    }

    fn normalized_plane(row: Vec4) -> [f32; 4] {
        let normal = row.truncate();
        let inv_len = 1.0 / normal.length();
        let n = normal * inv_len;
        [n.x, n.y, n.z, row.w * inv_len]
    }

    fn assert_plane_approx(actual: [f32; 4], expected: [f32; 4]) {
        let eps = 1e-5_f32;
        for i in 0..4 {
            assert!(
                (actual[i] - expected[i]).abs() < eps,
                "plane component {i}: expected {}, got {}",
                expected[i],
                actual[i]
            );
        }
    }

    #[test]
    fn extract_frustum_planes_for_gpu_uses_webgpu_zero_to_one_near_plane() {
        let m = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 10.0);
        let planes = extract_frustum_planes_for_gpu(&m);

        let r2 = matrix_row(&m, 2);
        let r3 = matrix_row(&m, 3);
        assert_plane_approx(planes[4], normalized_plane(r2));
        assert_plane_approx(planes[5], normalized_plane(r3 - r2));
    }

    /// AC#2: a world AABB inside the cone is classified inside; one fully
    /// outside the cone (behind the light, opposite the aim) is classified
    /// outside. Same predicate the GPU per-slot cull mirrors.
    #[test]
    fn cone_frustum_classifies_inside_and_outside_aabbs() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        // A small box well within the cone: a few meters down -Z, on axis.
        let inside = Aabb {
            min: Vec3::new(-0.5, -0.5, -10.5),
            max: Vec3::new(0.5, 0.5, -9.5),
        };
        assert!(
            aabb_intersects_frustum(&inside, &planes),
            "on-axis box inside the cone must classify as inside"
        );

        // A box behind the light (positive Z) cannot be in a cone aimed at -Z.
        let behind = Aabb {
            min: Vec3::new(-0.5, -0.5, 9.5),
            max: Vec3::new(0.5, 0.5, 10.5),
        };
        assert!(
            !aabb_intersects_frustum(&behind, &planes),
            "box behind the light must classify as outside the cone"
        );

        // A box far off to the side, beyond the cone's angular spread.
        let off_axis = Aabb {
            min: Vec3::new(49.5, -0.5, -10.5),
            max: Vec3::new(50.5, 0.5, -9.5),
        };
        assert!(
            !aabb_intersects_frustum(&off_axis, &planes),
            "box outside the cone's angular spread must classify as outside"
        );
    }

    /// The enclosing AABB derived from the light-space matrix must contain the
    /// cone: it spans the aim direction (reaching toward the far plane) and
    /// stays bounded near the apex.
    #[test]
    fn cone_enclosing_aabb_spans_aim_direction() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let aabb = cone_enclosing_aabb(&m);

        // Cone aims down -Z with a 20m range, so the box must extend to roughly
        // -20 in Z and include the apex at the origin.
        assert!(
            aabb.min.z < -19.0,
            "enclosing AABB should reach the far plane (~-20), got min.z = {}",
            aabb.min.z
        );
        assert!(
            aabb.max.z > -0.5,
            "enclosing AABB should include the apex near the origin, got max.z = {}",
            aabb.max.z
        );
        // Lateral extent is bounded by the cone half-angle at 20m, not infinite.
        assert!(
            aabb.min.x.is_finite() && aabb.max.x.is_finite(),
            "enclosing AABB lateral extent must be finite"
        );
    }

    /// A point inside the enclosing AABB and on the cone axis must also pass the
    /// plane predicate — the two representations agree on the obvious interior.
    #[test]
    fn enclosing_aabb_interior_point_passes_planes() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        // Tiny box at the cone center, halfway to the far plane.
        let center = Aabb {
            min: Vec3::new(-0.1, -0.1, -10.1),
            max: Vec3::new(0.1, 0.1, -9.9),
        };
        assert!(aabb_intersects_frustum(&center, &planes));
    }

    /// Transforming a local AABB by a 90° rotation must produce a correct
    /// world-space enclosure — the 8-corner method, not a component-wise
    /// transform of `min`/`max`. A box thin in X and tall in Y, rotated 90° about
    /// Z, must come back tall in X and thin in Y (extents swapped), with the box
    /// still centered at the origin.
    #[test]
    fn aabb_transformed_encloses_rotation() {
        let local = Aabb {
            min: Vec3::new(-1.0, -3.0, -0.5),
            max: Vec3::new(1.0, 3.0, 0.5),
        };
        let rot = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let world = local.transformed(&rot);

        let eps = 1e-5_f32;
        // X half-extent now ~3 (was the Y half-extent), Y half-extent now ~1.
        assert!(
            (world.max.x - 3.0).abs() < eps && (world.min.x + 3.0).abs() < eps,
            "rotated box X extent should be ±3, got [{}, {}]",
            world.min.x,
            world.max.x
        );
        assert!(
            (world.max.y - 1.0).abs() < eps && (world.min.y + 1.0).abs() < eps,
            "rotated box Y extent should be ±1, got [{}, {}]",
            world.min.y,
            world.max.y
        );
        // Z is unaffected by a Z rotation.
        assert!((world.max.z - 0.5).abs() < eps && (world.min.z + 0.5).abs() < eps);
    }

    /// A translation must shift both corners by the same offset (no rotation, so
    /// the enclosure is exact).
    #[test]
    fn aabb_transformed_translates() {
        let local = Aabb {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        let world = local.transformed(&Mat4::from_translation(Vec3::new(10.0, -5.0, 2.0)));
        let eps = 1e-5_f32;
        assert!((world.min - Vec3::new(9.0, -6.0, 1.0)).length() < eps);
        assert!((world.max - Vec3::new(11.0, -4.0, 3.0)).length() < eps);
    }

    /// A degenerate (non-invertible) light-space matrix must not panic and must
    /// return the documented fallback: a zero-extent point AABB at the origin.
    ///
    /// Mat4::ZERO has determinant 0, so glam's `inverse()` produces NaN/Inf
    /// entries. The per-corner w-divide guard sets w=1 for near-zero w, but
    /// the resulting NaN coordinates leave min/max at Inf/−Inf, which are
    /// non-finite — triggering the explicit fallback to the origin point.
    #[test]
    fn cone_enclosing_aabb_degenerate_matrix_returns_origin_point() {
        let aabb = cone_enclosing_aabb(&Mat4::ZERO);
        let eps = 1e-6_f32;
        assert!(
            aabb.min.x.abs() < eps && aabb.min.y.abs() < eps && aabb.min.z.abs() < eps,
            "degenerate matrix fallback: min should be origin, got {:?}",
            aabb.min
        );
        assert!(
            aabb.max.x.abs() < eps && aabb.max.y.abs() < eps && aabb.max.z.abs() < eps,
            "degenerate matrix fallback: max should be origin, got {:?}",
            aabb.max
        );
    }

    /// An AABB straddling the cone apex (the origin region) must be classified
    /// as intersecting. The existing inside-tests only cover mid-cone boxes;
    /// this covers the near-apex boundary where the near-clip plane lives.
    ///
    /// The box extends from the apex (z=0) back into the cone volume (z=-2),
    /// fully on axis. The portion z∈[-2, -0.1] lies inside the near..far
    /// depth range, so the positive-vertex test must not reject it as "behind
    /// the near plane".
    #[test]
    fn cone_frustum_apex_straddling_aabb_classifies_as_intersecting() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        // Box runs from the apex at z=0 down to z=-2, on axis. Covers the
        // near-clip transition (near=0.1 in world-space = z=-0.1).
        let apex_box = Aabb {
            min: Vec3::new(-0.2, -0.2, -2.0),
            max: Vec3::new(0.2, 0.2, 0.0),
        };
        assert!(
            aabb_intersects_frustum(&apex_box, &planes),
            "AABB straddling the cone apex must be classified as intersecting"
        );
    }

    /// An AABB that grazes the cone's right side plane from just inside must be
    /// classified as intersecting; one clearly past the side boundary must not.
    ///
    /// The cone half-angle is 0.4 rad; at z=−10 the radius is ~4.23 m. A box
    /// whose positive vertex (rightmost edge) sits at x≈4.0 is well inside the
    /// side plane. A box displaced far beyond the radius (x≥10) is definitively
    /// outside. The positive-vertex selection in `aabb_intersects_frustum` must
    /// route to the correct AABB corner in both cases.
    #[test]
    fn cone_frustum_grazing_side_plane_aabb_classified_correctly() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        // tan(0.4) ≈ 0.4228; cone radius at z=-10 is ~4.23 m.
        // A box with its right edge at x=4.0 is inside the side plane.
        let just_inside = Aabb {
            min: Vec3::new(3.0, -0.5, -10.5),
            max: Vec3::new(4.0, 0.5, -9.5),
        };
        assert!(
            aabb_intersects_frustum(&just_inside, &planes),
            "AABB with positive vertex inside the cone side plane must intersect"
        );

        // A box displaced well beyond the cone radius: positive vertex at x=10
        // is far outside the side plane (~4.23 m radius at that depth).
        let clearly_outside = Aabb {
            min: Vec3::new(9.5, -0.5, -10.5),
            max: Vec3::new(10.5, 0.5, -9.5),
        };
        assert!(
            !aabb_intersects_frustum(&clearly_outside, &planes),
            "AABB well outside the cone side plane must not intersect"
        );
    }
}
