// CPU-only skinned-model module: mesh, skeleton, glTF load, animation sampling.
// See: context/lib/rendering_pipeline.md §9
//
// This module is CPU-only by contract: it must not import or depend on `wgpu`.
// The renderer (`crate::render`) owns GPU and builds the vertex layout / bind
// groups from these plain Pod types. Keep it that way.

pub(crate) mod anim;
pub(crate) mod gltf_loader;
pub(crate) mod mesh;
pub(crate) mod skeleton;

use bytemuck::{Pod, Zeroable};

/// A model identity: the raw `MeshComponent.model` string a mesh entity renders.
///
/// Map-authored paths are assumed already canonical, so this is the verbatim
/// string with no normalization or interning — it is the cache key the renderer
/// uses to dedup uploaded models (one `UploadedModel` per distinct handle) and
/// the grouping key the per-frame draw planner buckets instances by. CPU-only:
/// the collector (game side) produces it from the component; the renderer
/// consumes it. Cloning is a `String` clone — cheap at the handful-of-models
/// scale a frame carries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ModelHandle(pub(crate) String);

impl ModelHandle {
    /// The underlying handle string (the raw `MeshComponent.model` path).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ModelHandle {
    fn from(s: &str) -> Self {
        ModelHandle(s.to_string())
    }
}

impl From<String> for ModelHandle {
    fn from(s: String) -> Self {
        ModelHandle(s)
    }
}

/// One bone-palette entry: a joint's skinning matrix (column-major, matching
/// glam's `Mat4` memory order).
///
/// **Shared-storage-buffer scheme.** Every skinned instance's palette is one
/// contiguous run of `BonePaletteEntry` values appended into a single shared
/// storage buffer each frame. A per-instance **base index** (the offset of the
/// instance's first entry in that buffer) is supplied per draw; the vertex
/// shader adds a vertex's `joints[i]` to the base index to address its joint.
/// This keeps one buffer for the whole frame and one small per-draw scalar,
/// rather than a buffer (or bind group) per instance.
///
/// --- Provisional contracts (named, not frozen — the broadening tasks vote) ---
///
/// * **Per-instance / per-draw alignment to indirect draw.** When the
///   many-instance indirect-draw task lands, the per-instance base index is
///   expected to live in the same per-instance data path that task's
///   indirect-draw shape carries (instance data SSBO indexed by draw), not a
///   push constant. The exact carrier is that task's call.
/// * **Depth-only skinned variant.** The shadow task chooses the depth-only
///   skinned vertex/pipeline shape. It is expected to reuse this same palette
///   buffer + base-index scheme (position + joints/weights only); the color
///   attributes are dropped. Not allocated here.
/// * **Lighting bind group.** The settled-lighting-interface task chooses the
///   skinned mesh's lighting bind group (SH ambient + dynamic direct). The mesh
///   pass leaves an additive slot for it (see `render::mesh_pass`); it is not
///   allocated until that interface settles.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BonePaletteEntry {
    pub matrix: [[f32; 4]; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_palette_entry_pod_round_trips_through_bytes() {
        let entry = BonePaletteEntry {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [4.0, 5.0, 6.0, 1.0],
            ],
        };
        let bytes = bytemuck::bytes_of(&entry);
        let back: BonePaletteEntry = *bytemuck::from_bytes(bytes);
        assert_eq!(entry, back);
    }
}
