// Skinned mesh CPU types: the Pod skinned vertex and its index buffer.
// See: context/lib/rendering_pipeline.md §5

use bytemuck::{Pod, Zeroable};

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
        // tangent so normal mapping survives skinning. The glTF loader (Task 2)
        // relies on a matching TANGENT source attribute.
        let v = SkinnedVertex::rigid([0.0; 3], [0, 0], [0, 0], [0xABCD, 0x1234]);
        assert_eq!(v.tangent_packed, [0xABCD, 0x1234]);
    }

    #[test]
    fn rigid_vertex_binds_identity_weighted_joint_zero() {
        let v = SkinnedVertex::rigid([0.0; 3], [0, 0], [0, 0], [0, 0]);
        assert_eq!(v.joints, [0, 0, 0, 0]);
        assert_eq!(v.weights, [255, 0, 0, 0]);
    }
}
