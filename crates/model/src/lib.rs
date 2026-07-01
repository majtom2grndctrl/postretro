// postretro-model crate: CPU-only mesh, skeleton, glTF load, animation sampling.
// See: context/lib/rendering_pipeline.md §9
//
// This crate is CPU-only by contract: it must not import or depend on `wgpu`.
// The renderer owns GPU and builds the vertex layout / bind
// groups from these plain Pod types. Keep it that way.

pub mod anim;
pub mod gltf_loader;
pub mod mesh;
pub mod sample_params;
pub mod skeleton;

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
pub struct ModelHandle(pub String);

impl ModelHandle {
    /// The underlying handle string (the raw `MeshComponent.model` path).
    pub fn as_str(&self) -> &str {
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
