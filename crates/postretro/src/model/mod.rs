// CPU-only skinned-model module: mesh, skeleton, glTF load, animation sampling.
// See: context/lib/rendering_pipeline.md §5
//
// This module is CPU-only by contract: it must not import or depend on `wgpu`.
// The renderer (`crate::render`) owns GPU and builds the vertex layout / bind
// groups from these plain Pod types. Keep it that way.
//
// Task 1 lands the CPU-only type surface; Tasks 2 (load), 3 (render pass),
// 4 (sample), and 5 (collect) wire it up. Until then the loader / mesh / palette
// types are constructed only by tests, so allow dead code module-wide rather
// than scatter per-item attributes that the wiring tasks then have to remove.
#![allow(dead_code)]

pub(crate) mod anim;
pub(crate) mod gltf_loader;
pub(crate) mod mesh;
pub(crate) mod skeleton;

use bytemuck::{Pod, Zeroable};

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
/// * **Per-instance / per-draw alignment to M3.5 indirect.** When the
///   many-instance task lands, the per-instance base index is expected to live
///   in the same per-instance data path the M3.5 indirect-draw shape carries
///   (instance data SSBO indexed by draw), not a push constant. The exact
///   carrier is that task's call.
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
