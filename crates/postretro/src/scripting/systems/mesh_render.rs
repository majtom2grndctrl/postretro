// Mesh render collector: walks MeshComponent entities and gathers per-instance
// skinned-draw matrices for the renderer.
// See: context/lib/scripting.md

use glam::Mat4;

use crate::prl::LevelWorld;
use crate::render::mesh_pass::mesh_visible;
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityRegistry, Transform};
use crate::visibility::VisibleCells;

/// Per-frame scratch state for the skinned-mesh render path. Owned by the game
/// layer (not the renderer) so the wgpu boundary stays inside `MeshPass` —
/// mirrors `ParticleRenderCollector`'s ownership split.
///
/// Runs in the render-frame collection sub-stage (NOT the game-logic tick): it
/// reads the registry + the world + this frame's visible-cell set, applies the
/// pure `mesh_pass::mesh_visible` cull, and packs the surviving per-instance
/// world matrices. It never touches wgpu — the renderer consumes [`draws`] and
/// owns the GPU upload + record_draw.
///
/// [`draws`]: MeshRenderCollector::draws
pub(crate) struct MeshRenderCollector {
    /// Per-frame draw list: final per-instance world matrices that survived the
    /// cull. Cleared + refilled each `collect` so capacity carries across frames.
    draws: Vec<Mat4>,
}

impl MeshRenderCollector {
    pub(crate) fn new() -> Self {
        Self { draws: Vec::new() }
    }

    /// Walk `ComponentKind::Mesh` entities, cull each against the frame's
    /// visible set, and pack the survivors' world matrices.
    ///
    /// Clears the draw list first (reusing capacity), then for each mesh entity:
    /// read-only-borrows its `Transform`, culls via the pure
    /// `mesh_pass::mesh_visible` (point→leaf membership in `visible`), and pushes
    /// the composed world matrix for survivors. The renderer reads [`draws`]
    /// and records one direct draw per matrix — it needs no world reference
    /// because the cull already happened here.
    ///
    /// [`draws`]: MeshRenderCollector::draws
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
    ) {
        self.draws.clear();

        for (id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
            // The model handle (`MeshComponent.model`) is resolved at the spawn
            // seam this slice (one model, uploaded renderer-side); the collector
            // only needs the transform + cull decision. Future multi-model work
            // keys the draw by this handle.
            let ComponentValue::Mesh(_mesh) = value else {
                continue;
            };
            let Ok(transform) = registry.get_component::<Transform>(id) else {
                continue;
            };
            if !mesh_visible(world, visible, transform.position) {
                continue;
            }
            self.draws.push(Mat4::from_scale_rotation_translation(
                transform.scale,
                transform.rotation,
                transform.position,
            ));
        }
    }

    /// The per-instance world matrices to draw this frame (cull already applied).
    pub(crate) fn draws(&self) -> &[Mat4] {
        &self.draws
    }
}

impl Default for MeshRenderCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{BspChild, LeafData, LevelWorld};
    use crate::scripting::components::mesh::MeshComponent;
    use crate::scripting::registry::EntityRegistry;
    use glam::Vec3;
    use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

    fn spawn_mesh(registry: &mut EntityRegistry, position: Vec3) {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                MeshComponent {
                    model: "decraniated".into(),
                },
            )
            .unwrap();
    }

    // The collector reuses the SAME pure cull the renderer pass documents
    // (`mesh_pass::mesh_visible`); membership behavior is covered by `mesh_pass`'s
    // own cull tests against a synthetic visible-set. Here we verify the
    // collector's packing + transform composition against a minimal single-leaf
    // world (leaf 0 spans all space, so any position lands in leaf 0).

    fn single_leaf_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![LeafData {
                bounds_min: Vec3::splat(-1.0e6),
                bounds_max: Vec3::splat(1.0e6),
                face_start: 0,
                face_count: 0,
                is_solid: false,
            }],
            root: BspChild::Leaf(0),
            portals: vec![],
            leaf_portals: vec![vec![]],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: crate::prl::LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
        }
    }

    #[test]
    fn collect_packs_one_visible_mesh_world_matrix() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, Vec3::new(1.0, 2.0, 3.0));

        // Leaf 0 is the only visible cell; the mesh lands in it → draws.
        collector.collect(&registry, &world, &VisibleCells::Culled(vec![0]));
        assert_eq!(collector.draws().len(), 1);
        // Translation column carries the entity position.
        let t = collector.draws()[0].w_axis;
        assert_eq!([t.x, t.y, t.z], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn collect_excludes_mesh_in_nonvisible_cell() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, Vec3::new(1.0, 2.0, 3.0));

        // The mesh lands in leaf 0, but only leaf 1 is visible → culled out.
        collector.collect(&registry, &world, &VisibleCells::Culled(vec![1]));
        assert!(collector.draws().is_empty());
    }

    #[test]
    fn collect_clears_between_frames_without_dropping_capacity() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, Vec3::ZERO);
        collector.collect(&registry, &world, &VisibleCells::DrawAll);
        let cap_after_first = collector.draws.capacity();
        assert!(cap_after_first >= 1);

        let ids: Vec<_> = registry
            .iter_with_kind(ComponentKind::Mesh)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            registry.despawn(id).unwrap();
        }
        collector.collect(&registry, &world, &VisibleCells::DrawAll);
        assert!(collector.draws().is_empty());
        assert_eq!(collector.draws.capacity(), cap_after_first);
    }
}
