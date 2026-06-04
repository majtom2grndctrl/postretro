// Mesh render collector: walks MeshComponent entities and gathers per-instance
// skinned-draw data for the renderer. Task 5 fills the collect logic.
// See: context/lib/scripting.md

use crate::scripting::registry::EntityRegistry;

/// Per-frame scratch state for the skinned-mesh render path. Owned by the game
/// layer (not the renderer) so the wgpu boundary stays inside `MeshPass` —
/// mirrors `ParticleRenderCollector`'s ownership split.
///
/// **Stub.** Task 5 fills `collect` (walk `MeshComponent` entities, resolve each
/// instance's transform + bone palette, and bucket per-model draw data) and the
/// iteration surface the renderer consumes.
#[allow(dead_code)] // Task 5 constructs and drives this; pre-emptive structure.
pub(crate) struct MeshRenderCollector {}

#[allow(dead_code)] // Task 5 calls these.
impl MeshRenderCollector {
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// Walk the registry and gather per-instance skinned-draw data.
    ///
    /// **Stub.** Task 5 fills the body.
    #[allow(unused_variables)]
    pub(crate) fn collect(&mut self, registry: &EntityRegistry) {
        // Task 5: iterate ComponentKind::Mesh entities, resolve transform +
        // animation state, and pack per-model instance data.
    }
}

impl Default for MeshRenderCollector {
    fn default() -> Self {
        Self::new()
    }
}
