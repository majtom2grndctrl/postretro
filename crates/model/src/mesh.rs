// Skinned mesh CPU types: the Pod skinned vertex and its index buffer.
// See: context/lib/rendering_pipeline.md §9

use bytemuck::{Pod, Zeroable};

use postretro_render_data::cone_frustum::Aabb;

use crate::pose_modifier::{PoseModifier, PoseModifierStack};
use crate::skeleton::{AnimationClip, Skeleton};

/// Maximum joints addressable by a single skeleton / bone palette run. The
/// `joints` indices on [`SkinnedVertex`] are `u8`, so 256 is the hard ceiling
/// a single skinned draw can index without widening the attribute.
pub const MAX_JOINTS: usize = 256;

/// One skinned-mesh vertex. CPU-only Pod data — the render pass derives the
/// wgpu vertex layout from these field widths later (the renderer owns GPU; this
/// module never touches wgpu).
///
/// Encoding mirrors `postretro_render_data::geometry::WorldVertex`: octahedral normal/tangent in
/// `u16 x 2`, UV quantized to `u16 x 2`. The skinning attributes (`joints`,
/// `weights`) are appended; weights are `u8` normalized 0..255 → 0..1 in the
/// vertex shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    /// Base UV, quantized 0..65535 → 0..1.
    pub base_uv: [u16; 2],
    /// Octahedral-encoded unit normal (u16 x 2).
    pub normal_oct: [u16; 2],
    /// Packed tangent: u16 octahedral u-component, u16 v-component with the
    /// bitangent sign in bit 15. Same scheme as `WorldVertex::tangent_packed`.
    pub tangent_packed: [u16; 2],
    /// Joint indices into the bone palette run for this vertex's instance.
    pub joints: [u8; 4],
    /// Joint weights, normalized 0..255 → 0..1 in the vertex shader. The four
    /// weights are expected to sum to 255 for a fully-weighted vertex.
    pub weights: [u8; 4],
}

impl SkinnedVertex {
    /// Degenerate single-bone vertex: bound rigidly to joint 0 with full weight.
    /// Used when a mesh primitive carries no skinning attributes (a static mesh
    /// hung under the skinned path) — joint 0 then resolves to the instance's
    /// world transform.
    // Test-only constructor for rigid vertices: the loader emits this encoding
    // ([0,0,0,0] / [255,0,0,0]) inline for non-skinned primitives and never
    // calls this.
    pub fn rigid(
        position: [f32; 3],
        base_uv: [u16; 2],
        normal_oct: [u16; 2],
        tangent_packed: [u16; 2],
    ) -> Self {
        Self {
            position,
            base_uv,
            normal_oct,
            tangent_packed,
            joints: [0, 0, 0, 0],
            weights: [255, 0, 0, 0],
        }
    }
}

/// A skinned mesh: one interleaved vertex stream plus a 32-bit index buffer.
/// Materials and the skeleton are carried alongside on
/// [`crate::gltf_loader::LoadedModel`], not embedded here.
///
/// `vertices` and `indices` stay public because the renderer uploads them
/// directly across the crate boundary. Call [`SkinnedMesh::compute_bounds`]
/// after changing vertex positions. Loader-produced meshes then call
/// [`SkinnedMesh::compute_conservative_animation_bounds`] before renderer
/// culling or shadow planning.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinnedMesh {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
    /// Conservative local-space AABB over every pose the renderer can produce.
    /// The renderer stamps this onto admitted instances for shadow and sampled-SH
    /// planning, so animation can never move a sampled pixel outside the gate.
    bounds: Aabb,
}

impl SkinnedMesh {
    /// Cached conservative local-space AABB over every renderer-produced pose.
    /// For a mesh without clips or pose modifiers this equals the tight bind-pose
    /// bound. The value is eager and manually invalidated, not lazy.
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// Recompute the tight bind-pose AABB over every vertex position and reset
    /// [`SkinnedMesh::bounds`] to it. A mesh with no vertices yields a zero box
    /// (see [`Aabb::from_points`]). Called by the glTF loader after merging
    /// primitives; kept here so any future mesh producer reuses the same source.
    pub fn compute_bounds(&mut self) {
        self.bounds = Aabb::from_points(
            self.vertices
                .iter()
                .map(|v| glam::Vec3::from_array(v.position)),
        );
    }

    /// Compute a conservative local-space envelope for every animation clip,
    /// crossfade, captured snapshot, and rotation-only pose modifier.
    ///
    /// For each joint, translation is bounded by the component extrema across
    /// rest pose and every authored key; scale is bounded by the largest absolute
    /// component. Rotations need no authored extrema because they preserve
    /// length. Applying those bounds from a skinned point through its joint's
    /// parent chain bounds every sampled local-TRS composition. The final vertex
    /// radius is the non-negative skin-weighted sum of its joint radii, matching
    /// the GPU's linear blend. Crossfades and snapshots remain inside the same
    /// translation/scale extrema, while current pose modifiers change rotations
    /// only. The resulting origin-centered box is intentionally conservative,
    /// but contains no arbitrary padding.
    pub fn compute_conservative_animation_bounds(
        &mut self,
        skeleton: &Skeleton,
        clips: &[AnimationClip],
        pose_stack: &PoseModifierStack,
    ) {
        self.compute_bounds();
        if self.vertices.is_empty() || (clips.is_empty() && pose_stack.is_empty()) {
            return;
        }
        pose_modifiers_preserve_translation_and_scale(pose_stack);

        let mut local_bounds: Vec<LocalTransformBound> = skeleton
            .joints
            .iter()
            .map(|joint| {
                LocalTransformBound::from_rest(joint.rest_local.translation, joint.rest_local.scale)
            })
            .collect();
        for clip in clips {
            for (joint_index, tracks) in clip.joints.iter().enumerate() {
                let Some(bound) = local_bounds.get_mut(joint_index) else {
                    break;
                };
                for &translation in tracks.translation.values() {
                    bound.include_translation(translation);
                }
                for &scale in tracks.scale.values() {
                    bound.include_scale(scale);
                }
            }
        }

        let bind_radius = self
            .vertices
            .iter()
            .map(|vertex| vec3_length_f64(glam::Vec3::from_array(vertex.position)))
            .fold(0.0_f64, f64::max);
        let mut radius = bind_radius;
        for vertex in &self.vertices {
            let position = glam::Vec3::from_array(vertex.position);
            let mut vertex_radius = 0.0_f64;
            for (&joint_index, &weight) in vertex.joints.iter().zip(&vertex.weights) {
                if weight == 0 {
                    continue;
                }
                let joint_index = joint_index as usize;
                let Some(joint) = skeleton.joints.get(joint_index) else {
                    continue;
                };
                let inverse_bind = glam::Mat4::from_cols_array_2d(&joint.inverse_bind);
                let mut influenced_radius =
                    vec3_length_f64(inverse_bind.transform_point3(position));
                let mut cursor = Some(joint_index);
                while let Some(index) = cursor {
                    let Some(local) = local_bounds.get(index) else {
                        break;
                    };
                    influenced_radius =
                        local.translation_radius() + local.max_abs_scale * influenced_radius;
                    cursor = skeleton.joints.get(index).and_then(|joint| joint.parent);
                }
                vertex_radius += f64::from(weight) / 255.0 * influenced_radius;
            }
            radius = radius.max(vertex_radius);
        }

        let radius = radius.min(f64::from(f32::MAX)) as f32;
        self.bounds = Aabb {
            min: glam::Vec3::splat(-radius),
            max: glam::Vec3::splat(radius),
        };
    }
}

#[derive(Debug, Clone, Copy)]
struct LocalTransformBound {
    max_abs_translation: glam::Vec3,
    max_abs_scale: f64,
}

impl LocalTransformBound {
    fn from_rest(translation: glam::Vec3, scale: glam::Vec3) -> Self {
        let mut bound = Self {
            max_abs_translation: glam::Vec3::ZERO,
            max_abs_scale: 0.0,
        };
        bound.include_translation(translation);
        bound.include_scale(scale);
        bound
    }

    fn include_translation(&mut self, translation: glam::Vec3) {
        if translation.is_finite() {
            self.max_abs_translation = self.max_abs_translation.max(translation.abs());
        }
    }

    fn include_scale(&mut self, scale: glam::Vec3) {
        if scale.is_finite() {
            self.max_abs_scale = self.max_abs_scale.max(f64::from(scale.abs().max_element()));
        }
    }

    fn translation_radius(self) -> f64 {
        vec3_length_f64(self.max_abs_translation)
    }
}

fn vec3_length_f64(value: glam::Vec3) -> f64 {
    let x = f64::from(value.x);
    let y = f64::from(value.y);
    let z = f64::from(value.z);
    (x * x + y * y + z * z).sqrt()
}

fn pose_modifiers_preserve_translation_and_scale(stack: &PoseModifierStack) {
    // Exhaustive drift guard: a future modifier that changes translation or
    // scale must extend the envelope math before this match can compile.
    for entry in stack.entries() {
        match &entry.modifier {
            PoseModifier::AimPitchBend { .. }
            | PoseModifier::UpperLowerSplit { .. }
            | PoseModifier::FootIk { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skinned_vertex_pod_round_trips_through_bytes() {
        let v = SkinnedVertex {
            position: [1.0, 2.0, 3.0],
            base_uv: [10, 20],
            normal_oct: [30, 40],
            tangent_packed: [50, 60],
            joints: [1, 2, 3, 4],
            weights: [100, 80, 50, 25],
        };
        let bytes = bytemuck::bytes_of(&v);
        let back: SkinnedVertex = *bytemuck::from_bytes(bytes);
        assert_eq!(v, back);
    }

    #[test]
    fn skinned_vertex_layout_carries_a_tangent() {
        // Guards the committed layout: the skinned vertex must carry a packed
        // tangent so normal mapping survives skinning. The glTF loader
        // (`gltf_loader`) packs authored tangents or supplies the default.
        let v = SkinnedVertex::rigid([0.0; 3], [0, 0], [0, 0], [0xABCD, 0x1234]);
        assert_eq!(v.tangent_packed, [0xABCD, 0x1234]);
    }

    #[test]
    fn rigid_vertex_binds_identity_weighted_joint_zero() {
        let v = SkinnedVertex::rigid([0.0; 3], [0, 0], [0, 0], [0, 0]);
        assert_eq!(v.joints, [0, 0, 0, 0]);
        assert_eq!(v.weights, [255, 0, 0, 0]);
    }

    fn vertex_at(position: [f32; 3]) -> SkinnedVertex {
        SkinnedVertex::rigid(position, [0, 0], [0, 0], [0, 0])
    }

    fn assert_vec3_close(got: glam::Vec3, want: glam::Vec3) {
        const EPS: f32 = 1.0e-6;
        assert!(
            (got - want).abs().cmple(glam::Vec3::splat(EPS)).all(),
            "expected {want:?}, got {got:?}",
        );
    }

    #[test]
    fn compute_bounds_tightly_encloses_vertex_positions() {
        // The local AABB must be the tight min/max over every vertex position —
        // the bound the per-light caster cull transforms by the instance
        // transform. Mixed-sign coordinates exercise both min and max corners.
        let mut mesh = SkinnedMesh {
            vertices: vec![
                vertex_at([-1.0, 2.0, 0.5]),
                vertex_at([3.0, -4.0, 0.5]),
                vertex_at([0.0, 0.0, -2.0]),
            ],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        mesh.compute_bounds();
        assert_vec3_close(mesh.bounds.min, glam::Vec3::new(-1.0, -4.0, -2.0));
        assert_vec3_close(mesh.bounds.max, glam::Vec3::new(3.0, 2.0, 0.5));
    }

    #[test]
    fn compute_bounds_refreshes_after_public_vertex_mutation() {
        let mut mesh = SkinnedMesh {
            vertices: vec![vertex_at([0.0, 0.0, 0.0]), vertex_at([1.0, 1.0, 1.0])],
            indices: vec![0, 1],
            ..Default::default()
        };

        mesh.compute_bounds();
        mesh.vertices[1].position = [4.0, 5.0, 6.0];
        mesh.compute_bounds();

        assert_vec3_close(mesh.bounds().min, glam::Vec3::ZERO);
        assert_vec3_close(mesh.bounds().max, glam::Vec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn compute_bounds_empty_mesh_is_a_zero_box() {
        // A points-less mesh must not leave the inverted `Aabb::empty` sentinel
        // (min > max) on the bound — it collapses to a well-formed zero box so a
        // downstream frustum test never sees an inverted AABB.
        let mut mesh = SkinnedMesh::default();
        mesh.compute_bounds();
        assert_vec3_close(mesh.bounds.min, glam::Vec3::ZERO);
        assert_vec3_close(mesh.bounds.max, glam::Vec3::ZERO);
    }

    // Regression: bind-pose bounds let animated vertices sample stale off-gate SH rows.
    #[test]
    fn conservative_animation_bounds_enclose_pose_beyond_bind_pose() {
        use crate::anim::{Loop, sample_clip_looped};
        use crate::skeleton::{
            AnimationClip, Interp, Joint, JointTracks, RestLocal, Skeleton, Track,
        };

        let mut mesh = SkinnedMesh {
            vertices: vec![vertex_at([1.0, 0.0, 0.0])],
            indices: vec![0],
            ..Default::default()
        };
        let skeleton = Skeleton::new(vec![Joint {
            parent: None,
            inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
            rest_local: RestLocal::default(),
        }])
        .expect("one root joint is topological");
        let clip = AnimationClip {
            name: "translate-past-bind".into(),
            duration: 1.0,
            joints: vec![JointTracks {
                translation: Track::new(
                    vec![0.0, 1.0],
                    vec![glam::Vec3::ZERO, glam::Vec3::new(5.0, 0.0, 0.0)],
                    Interp::Linear,
                )
                .expect("test track is valid"),
                ..Default::default()
            }],
            travel_speed: None,
        };

        mesh.compute_bounds();
        let bind = mesh.bounds();
        mesh.compute_conservative_animation_bounds(
            &skeleton,
            std::slice::from_ref(&clip),
            &PoseModifierStack::default(),
        );
        let conservative = mesh.bounds();
        let mut palette = Vec::new();
        sample_clip_looped(&clip, &skeleton, 1.0, Loop::Clamp, &mut palette);
        let posed = glam::Mat4::from_cols_array_2d(&palette[0].matrix)
            .transform_point3(glam::Vec3::new(1.0, 0.0, 0.0));

        assert!(posed.x > bind.max.x, "fixture must exceed its bind pose");
        assert!(
            posed.cmpge(conservative.min).all() && posed.cmple(conservative.max).all(),
            "posed vertex {posed:?} must remain inside {conservative:?}",
        );
    }
}
