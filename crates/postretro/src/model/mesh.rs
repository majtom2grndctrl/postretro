// Skinned mesh CPU types: the Pod skinned vertex and its index buffer.
// See: context/lib/rendering_pipeline.md §9

use bytemuck::{Pod, Zeroable};

use crate::lighting::cone_frustum::Aabb;

/// Maximum joints addressable by a single skeleton / bone palette run. The
/// `joints` indices on [`SkinnedVertex`] are `u8`, so 256 is the hard ceiling
/// a single skinned draw can index without widening the attribute.
pub const MAX_JOINTS: usize = 256;

/// One skinned-mesh vertex. CPU-only Pod data — the render pass derives the
/// wgpu vertex layout from these field widths later (the renderer owns GPU; this
/// module never touches wgpu).
///
/// Encoding mirrors `crate::geometry::WorldVertex`: octahedral normal/tangent in
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
    // Kept for the loader-broadening task that admits non-skinned primitives.
    #[allow(dead_code)]
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
/// Materials and the skeleton are carried alongside in [`crate::model`], not
/// embedded here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinnedMesh {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
    /// Tight LOCAL-space (bind-pose) AABB over every vertex position, computed at
    /// glTF load. Carried CPU-side so the per-light caster cull (a later task)
    /// can transform it by an instance's world transform and test it against a
    /// light's cone/face frustum. It is a bind-pose bound; animation can push a
    /// vertex slightly past it, which the cull treats conservatively (a missed
    /// caster only loses a sliver of its own shadow at the silhouette). The
    /// renderer-side uploaded model needs no copy — the cull reads this CPU side.
    pub bounds: Aabb,
}

impl SkinnedMesh {
    /// Recompute [`SkinnedMesh::bounds`] as the tight local-space AABB over every
    /// vertex position. A mesh with no vertices yields a zero box (see
    /// [`Aabb::from_points`]). Called by the glTF loader after merging primitives;
    /// kept here (not the loader) so the bound derives from the same `position`
    /// field the GPU vertex stream uses, and any future mesh producer reuses it.
    pub fn compute_bounds(&mut self) {
        self.bounds = Aabb::from_points(
            self.vertices
                .iter()
                .map(|v| glam::Vec3::from_array(v.position)),
        );
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
        // tangent so normal mapping survives skinning. The glTF loader (`gltf_loader`)
        // requires a matching TANGENT source attribute.
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
        assert_eq!(mesh.bounds.min, glam::Vec3::new(-1.0, -4.0, -2.0));
        assert_eq!(mesh.bounds.max, glam::Vec3::new(3.0, 2.0, 0.5));
    }

    #[test]
    fn compute_bounds_empty_mesh_is_a_zero_box() {
        // A points-less mesh must not leave the inverted `Aabb::empty` sentinel
        // (min > max) on the bound — it collapses to a well-formed zero box so a
        // downstream frustum test never sees an inverted AABB.
        let mut mesh = SkinnedMesh::default();
        mesh.compute_bounds();
        assert_eq!(mesh.bounds.min, glam::Vec3::ZERO);
        assert_eq!(mesh.bounds.max, glam::Vec3::ZERO);
    }
}
