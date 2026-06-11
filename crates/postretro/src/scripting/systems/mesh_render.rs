// Mesh render collector: walks MeshComponent entities and gathers per-instance
// skinned-draw inputs (model handle + interpolated transform) for the renderer.
// See: context/lib/entity_model.md §5 · context/lib/rendering_pipeline.md §9

use super::mesh_anim::{self, MeshClipTables};
use crate::model::ModelHandle;
use crate::prl::LevelWorld;
use crate::render::mesh_instances::{MeshInstanceInput, MeshSampleParams};
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
    /// visible set, and emit the survivors' draw inputs (handle, interpolated
    /// transform, resolved sample params, optional capture instruction).
    ///
    /// Clears the instance list first (reusing capacity), then for each mesh
    /// entity: read-borrows its `MeshComponent` (the model handle + optional
    /// animation state) and its `Transform`. The cull tests the entity's
    /// **current-tick** transform translation (stable per-tick visibility) via
    /// the pure `mesh_pass::mesh_visible`; survivors emit their **interpolated**
    /// transform (Task A's accessor at the frame `alpha`, the same alpha the
    /// player camera reads from `frame_timing`) so the model renders smoothly
    /// between ticks.
    ///
    /// Animation: `anim_time` is the accumulated game-layer animation clock
    /// (`frame_dt × time_scale`); `tables` is the level-load clip table set. For
    /// an animated entity the collector resolves its current/previous states into
    /// per-instance `MeshSampleParams` (clip-local times, crossfade weight,
    /// snapshot fade) and emits a one-time capture instruction on a `"smooth"`
    /// interrupt frame. A stateless `prop_mesh` entity (no animation block) gets
    /// the default params: first clip, looped, `anim_time + per-instance phase`.
    ///
    /// The per-instance phase seed is the raw `EntityId`, folded into a
    /// deterministic phase offset so a spawned wave does not animate lock-step
    /// (looping states only — one-shot states play from entry, no phase). It also
    /// keys the snapshot store on a `"smooth"` capture.
    ///
    /// [`instances`]: MeshRenderCollector::instances
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
        anim_time: f64,
        tables: &MeshClipTables,
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

            let handle = ModelHandle::from(mesh.model.clone());
            let seed = id.to_raw();
            let (sample, capture) =
                resolve_sample(mesh.animation.as_ref(), &handle, tables, anim_time, seed);

            self.instances.push(MeshInstanceInput {
                model: handle,
                transform: glam::Mat4::from_scale_rotation_translation(
                    transform.scale,
                    transform.rotation,
                    transform.position,
                ),
                phase_seed: seed,
                sample,
                capture,
            });
        }
    }

    /// The per-instance draw inputs to plan this frame (cull already applied).
    pub(crate) fn instances(&self) -> &[MeshInstanceInput] {
        &self.instances
    }
}

/// Resolve one entity's sample params + optional capture instruction.
///
/// Stateless (`animation == None`) or a model whose clip table is absent (never
/// uploaded): the default stateless params — first clip, looped, `anim_time +
/// per-instance phase`. The phase de-syncs a spawned wave (looping props).
///
/// Animated, with a clip table: delegate to [`mesh_anim::animate_entity`], which
/// computes clip-local times, the crossfade weight, the snapshot fade, and the
/// `"smooth"`-interrupt capture instruction. If the current state is unresolved
/// (no usable clip) the entity falls back to the stateless default so it still
/// renders (its bind pose / first clip) rather than vanishing.
fn resolve_sample(
    animation: Option<&crate::scripting::components::mesh::MeshAnimation>,
    handle: &ModelHandle,
    tables: &MeshClipTables,
    anim_time: f64,
    seed: u32,
) -> (
    MeshSampleParams,
    Option<crate::render::mesh_instances::CaptureInstruction>,
) {
    let table = tables.get(handle);

    // Animated entity with a resolved clip table → state-driven sampling.
    if let (Some(anim), Some(table)) = (animation, table) {
        // Per-instance phase from the CURRENT state's clip duration so a looping
        // wave de-syncs; one-shot states ignore it inside `animate_entity`.
        let phase = current_state_phase(anim, table, seed);
        if let Some(result) = mesh_anim::animate_entity(anim, table, anim_time, phase) {
            let mut capture = result.capture;
            if let Some(c) = capture.as_mut() {
                c.seed = seed; // key the snapshot store on the entity id
            }
            return (result.sample, capture);
        }
    }

    // Stateless / unresolved / un-uploaded: today's behavior. The primary clip is
    // index 0; phase folds in against its duration (0 if the model is uncached).
    let duration = table.and_then(|t| t.duration(0)).unwrap_or(0.0);
    let phase = crate::render::mesh_instances::instance_phase(seed, duration);
    (MeshSampleParams::stateless(anim_time as f32 + phase), None)
}

/// The per-instance phase offset for an entity's current animation state,
/// derived from its clip duration (looping de-sync). A state with no resolved
/// clip yields phase 0.
fn current_state_phase(
    anim: &crate::scripting::components::mesh::MeshAnimation,
    table: &super::mesh_anim::ModelClipTable,
    seed: u32,
) -> f32 {
    let duration = anim
        .states
        .get(&anim.current_state)
        .and_then(|s| s.clip_index)
        .and_then(|i| table.duration(i))
        .unwrap_or(0.0);
    crate::render::mesh_instances::instance_phase(seed, duration)
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
            .set_component(id, MeshComponent::stateless(model.into()))
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
        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![0]),
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
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

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
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

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
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
        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
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
            .set_component(id, MeshComponent::stateless("m".into()))
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

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            0.5,
            0.0,
            &MeshClipTables::new(),
        );
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
        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
        let cap_after_first = collector.instances.capacity();
        assert!(cap_after_first >= 1);

        let ids: Vec<_> = registry
            .iter_with_kind(ComponentKind::Mesh)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            registry.despawn(id).unwrap();
        }
        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
        );
        assert!(collector.instances().is_empty());
        assert_eq!(collector.instances.capacity(), cap_after_first);
    }

    // --- Animated-state sample-param resolution through `collect` ---------------

    use crate::model::ModelHandle;
    use crate::model::anim::Loop;
    use crate::render::mesh_instances::FadeSource;
    use crate::render::mesh_pass::ClipMetadata;
    use crate::scripting::components::mesh::{AnimationState, InterruptPolicy, MeshAnimation};
    use crate::scripting::components::mesh::{
        resolve_pending_animation_stamps, switch_animation_state,
    };
    use std::collections::HashMap;

    fn clip_meta(pairs: &[(&str, f32)]) -> Vec<ClipMetadata> {
        pairs
            .iter()
            .map(|(name, duration)| ClipMetadata {
                name: (*name).to_string(),
                duration: *duration,
            })
            .collect()
    }

    fn state(clip: &str, looping: bool, crossfade_ms: f32, idx: Option<usize>) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms,
            interrupt: InterruptPolicy::Smooth,
            clip_index: idx,
        }
    }

    /// Tables for a model "grunt" with idle (idx 0, 2s) + walk (idx 1, 2s).
    fn grunt_tables() -> MeshClipTables {
        let mut t = MeshClipTables::new();
        t.insert(
            ModelHandle::from("grunt"),
            &clip_meta(&[("idle", 2.0), ("walk", 2.0)]),
        );
        t
    }

    fn spawn_animated(reg: &mut EntityRegistry, pos: Vec3) -> crate::scripting::registry::EntityId {
        // Both states carry a nonzero crossfade so a switch starts a fade — needed
        // to exercise the smooth-interrupt capture path.
        let mut states = HashMap::new();
        states.insert("idle".into(), state("idle", true, 100.0, Some(0)));
        states.insert("walk".into(), state("walk", true, 200.0, Some(1)));
        let id = reg.spawn(Transform {
            position: pos,
            ..Transform::default()
        });
        reg.set_component(
            id,
            MeshComponent {
                model: "grunt".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
            },
        )
        .unwrap();
        id
    }

    #[test]
    fn collect_stateless_uses_first_clip_looped_with_phase() {
        // A stateless prop_mesh: first clip (index 0), looped, time = anim_time +
        // per-instance phase. Two distinct seeds give distinct phases (not lock-step).
        let mut reg = EntityRegistry::new();
        let world = single_leaf_world();
        let mut collector = MeshRenderCollector::new();
        let mut tables = MeshClipTables::new();
        tables.insert(ModelHandle::from("prop"), &clip_meta(&[("spin", 4.0)]));
        spawn_mesh(&mut reg, "prop", Vec3::new(1.0, 0.0, 0.0));
        spawn_mesh(&mut reg, "prop", Vec3::new(2.0, 0.0, 0.0));

        collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 3.0, &tables);
        let insts = collector.instances();
        assert_eq!(insts.len(), 2);
        for inst in insts {
            assert_eq!(inst.sample.primary.clip_index, 0, "stateless = first clip");
            assert_eq!(
                inst.sample.primary.loop_policy,
                Loop::Wrap,
                "stateless loops"
            );
            assert!(inst.sample.fade.is_none(), "stateless never fades");
            assert!(
                inst.sample.primary.time >= 3.0,
                "time = anim_time + phase ≥ clock"
            );
            assert!(inst.capture.is_none());
        }
        // Distinct phases → distinct sample times (wave de-sync).
        assert!(
            (insts[0].sample.primary.time - insts[1].sample.primary.time).abs() > 1.0e-4,
            "two stateless instances are not lock-step",
        );
    }

    #[test]
    fn collect_animated_plays_default_state_then_switched_state() {
        let mut reg = EntityRegistry::new();
        let world = single_leaf_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);

        // Default state idle (clip 0) at spawn.
        collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 0.5, &tables);
        assert_eq!(
            collector.instances()[0].sample.primary.clip_index,
            0,
            "plays default idle"
        );

        // Switch to walk; the new state's clip (1) drives the primary leg.
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);
        collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 5.0, &tables);
        assert_eq!(
            collector.instances()[0].sample.primary.clip_index,
            1,
            "setAnimationState switch plays the new state's clip",
        );
    }

    #[test]
    fn collect_smooth_interrupt_emits_capture_keyed_by_seed() {
        // idle→walk starts a fade (walk fades in over 200ms). Interrupting that
        // fade with walk→idle (default = smooth) records a snapshot fade source;
        // the collector then emits a capture instruction keyed by the entity seed
        // and a snapshot fade leg, INSIDE the new idle fade window.
        let mut reg = EntityRegistry::new();
        let world = single_leaf_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);

        // idle→walk: walk begins fading in from idle.
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);
        // Interrupt the walk fade with walk→idle (smooth). The entered idle has a
        // 100ms crossfade, so a fade window is open and the source is a snapshot.
        switch_animation_state(&mut reg, id, "idle");
        resolve_pending_animation_stamps(&mut reg, 1.02);

        // Collect 0.02s into idle's 100ms fade — capture due this frame.
        collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 1.04, &tables);
        let inst = &collector.instances()[0];
        let capture = inst
            .capture
            .as_ref()
            .expect("smooth interrupt emits a capture instruction");
        assert_eq!(capture.seed, id.to_raw(), "capture keyed by entity seed");
        assert!(
            matches!(
                inst.sample.fade.map(|f| f.from),
                Some(FadeSource::Snapshot { .. })
            ),
            "the interrupted fade blends from a snapshot source",
        );
        assert_eq!(
            inst.sample.primary.clip_index, 0,
            "primary is the entered idle"
        );
    }
}
