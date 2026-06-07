// Mesh render collector: walks MeshComponent entities and gathers per-instance
// skinned-draw inputs (model handle + interpolated transform) for the renderer.
// See: context/lib/entity_model.md §5 · context/lib/rendering_pipeline.md §9

use crate::model::ModelHandle;
use crate::prl::LevelWorld;
use crate::render::mesh_instances::MeshInstanceInput;
use crate::render::mesh_pass::mesh_visible;
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityRegistry, Transform};
use crate::visibility::VisibleCells;

/// Per-frame scratch state for the skinned-mesh render path. Owned by the game
/// layer (not the renderer) so the wgpu boundary stays inside `MeshPass` —
/// mirrors `ParticleRenderCollector`'s ownership split.
///
/// Runs in the render-frame collection sub-stage (NOT the game-logic tick): it
/// reads the registry + the world + this frame's visible-cell set, applies the
/// pure `mesh_pass::mesh_visible` cull, and emits per-instance draw inputs
/// (model handle + interpolated world transform). It never touches wgpu — the
/// renderer consumes [`instances`] and owns the GPU upload + draw recording.
///
/// [`instances`]: MeshRenderCollector::instances
pub(crate) struct MeshRenderCollector {
    /// Per-frame instance list: surviving (model handle, interpolated transform,
    /// phase seed) tuples. Cleared + refilled each `collect` so capacity carries
    /// across frames.
    instances: Vec<MeshInstanceInput>,
}

impl MeshRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            instances: Vec::new(),
        }
    }

    /// Walk `ComponentKind::Mesh` entities, cull each against the frame's
    /// visible set, and emit the survivors' (handle, interpolated transform).
    ///
    /// Clears the instance list first (reusing capacity), then for each mesh
    /// entity: read-borrows its `MeshComponent` (the model handle) and its
    /// `Transform`. The cull tests the entity's **current-tick** transform
    /// translation (stable per-tick visibility) via the pure
    /// `mesh_pass::mesh_visible`; survivors emit their **interpolated** transform
    /// (Task A's accessor at the frame `alpha`, the same alpha the player camera
    /// reads from `frame_timing`) so the model renders smoothly between ticks.
    ///
    /// The per-instance phase seed is the raw `EntityId`, which the renderer
    /// folds into a deterministic animation-phase offset so a spawned wave does
    /// not animate lock-step.
    ///
    /// [`instances`]: MeshRenderCollector::instances
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
    ) {
        self.instances.clear();

        for (id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
            let ComponentValue::Mesh(mesh) = value else {
                continue;
            };
            // Cull on the CURRENT-TICK translation (stable per-tick visibility),
            // not the sub-tick interpolated position.
            let Ok(current) = registry.get_component::<Transform>(id) else {
                continue;
            };
            if !mesh_visible(world, visible, current.position) {
                continue;
            }
            // Draw at the interpolated transform (smooth between ticks). Fall
            // back to the current transform if the interpolated read fails (a
            // stale id is not expected mid-iteration, but never fail the frame).
            let transform = registry
                .interpolated_transform(id, alpha)
                .unwrap_or(*current);
            self.instances.push(MeshInstanceInput {
                model: ModelHandle::from(mesh.model.clone()),
                transform: glam::Mat4::from_scale_rotation_translation(
                    transform.scale,
                    transform.rotation,
                    transform.position,
                ),
                phase_seed: id.to_raw(),
            });
        }
    }

    /// The per-instance draw inputs to plan this frame (cull already applied).
    pub(crate) fn instances(&self) -> &[MeshInstanceInput] {
        &self.instances
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

    fn spawn_mesh(registry: &mut EntityRegistry, model: &str, position: Vec3) {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                MeshComponent {
                    model: model.into(),
                },
            )
            .unwrap();
    }

    // The collector reuses the SAME pure cull the renderer pass documents
    // (`mesh_pass::mesh_visible`); membership behavior is covered by `mesh_pass`'s
    // own cull tests against a synthetic visible-set. Here we verify the
    // collector's emit + transform composition against a minimal single-leaf
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
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
        }
    }

    #[test]
    fn collect_emits_one_visible_mesh_instance() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 2.0, 3.0));

        // Leaf 0 is the only visible cell; the mesh lands in it → draws.
        collector.collect(&registry, &world, &VisibleCells::Culled(vec![0]), 1.0);
        assert_eq!(collector.instances().len(), 1);
        // Translation column carries the entity position; handle preserved.
        let inst = &collector.instances()[0];
        assert_eq!(inst.model.as_str(), "decraniated");
        let t = inst.transform.w_axis;
        assert_eq!([t.x, t.y, t.z], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn collect_emits_two_instances_of_same_model_at_distinct_transforms() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 0.0, 0.0));
        spawn_mesh(&mut registry, "decraniated", Vec3::new(5.0, 0.0, 0.0));

        collector.collect(&registry, &world, &VisibleCells::DrawAll, 1.0);
        assert_eq!(collector.instances().len(), 2);
        let xs: Vec<f32> = collector
            .instances()
            .iter()
            .map(|i| i.transform.w_axis.x)
            .collect();
        assert!(
            xs.contains(&1.0) && xs.contains(&5.0),
            "distinct transforms: {xs:?}"
        );
        // Same model handle on both.
        assert!(
            collector
                .instances()
                .iter()
                .all(|i| i.model.as_str() == "decraniated")
        );
    }

    #[test]
    fn collect_emits_distinct_models_with_their_handles() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, "grunt", Vec3::new(1.0, 0.0, 0.0));
        spawn_mesh(&mut registry, "drone", Vec3::new(2.0, 0.0, 0.0));

        collector.collect(&registry, &world, &VisibleCells::DrawAll, 1.0);
        assert_eq!(collector.instances().len(), 2);
        let handles: Vec<&str> = collector
            .instances()
            .iter()
            .map(|i| i.model.as_str())
            .collect();
        assert!(handles.contains(&"grunt") && handles.contains(&"drone"));
    }

    #[test]
    fn collect_excludes_mesh_in_nonvisible_cell() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 2.0, 3.0));

        // The mesh lands in leaf 0, but only leaf 1 is visible → culled out.
        collector.collect(&registry, &world, &VisibleCells::Culled(vec![1]), 1.0);
        assert!(collector.instances().is_empty());
    }

    #[test]
    fn collect_uses_interpolated_transform_at_alpha() {
        // The mesh's current position is (10,0,0); previous-tick is (0,0,0) (the
        // spawn seed). At alpha 0.5 the collector must emit the midpoint (5,0,0)
        // — proving it reads the interpolated transform, not current or spawn.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, MeshComponent { model: "m".into() })
            .unwrap();
        // Snapshot freezes the spawn (origin) as previous-tick, then move
        // current to (10,0,0).
        registry.snapshot_transforms();
        registry
            .set_component(
                id,
                Transform {
                    position: Vec3::new(10.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        collector.collect(&registry, &world, &VisibleCells::DrawAll, 0.5);
        assert_eq!(collector.instances().len(), 1);
        let t = collector.instances()[0].transform.w_axis;
        assert!(
            (t.x - 5.0).abs() < 1.0e-4,
            "interpolated x at alpha 0.5 is 5.0, got {}",
            t.x
        );
    }

    #[test]
    fn collect_clears_between_frames_without_dropping_capacity() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_leaf_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::ZERO);
        collector.collect(&registry, &world, &VisibleCells::DrawAll, 1.0);
        let cap_after_first = collector.instances.capacity();
        assert!(cap_after_first >= 1);

        let ids: Vec<_> = registry
            .iter_with_kind(ComponentKind::Mesh)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            registry.despawn(id).unwrap();
        }
        collector.collect(&registry, &world, &VisibleCells::DrawAll, 1.0);
        assert!(collector.instances().is_empty());
        assert_eq!(collector.instances.capacity(), cap_after_first);
    }
}
