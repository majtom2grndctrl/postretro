// Particle render collector: walks ParticleState entities and packs SpriteInstance bytes per collection.
// See: context/lib/scripting.md

use std::collections::{HashMap, HashSet};

use crate::prl::LevelWorld;
use crate::render::mesh_pass::mesh_visible_in_leaf;
use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use crate::visibility::VisibleCells;

/// Per-collection scratch buffers and warning state for the particle render
/// path. Owned by the game layer (not the renderer) so the wgpu boundary stays
/// inside `SmokePass`.
pub(crate) struct ParticleRenderCollector {
    buffers: HashMap<String, Vec<u8>>,
    /// Pre-built sprite-name → collection-key resolution map. Built once per
    /// level at `register_sprite` time; read on the per-particle hot path so
    /// `collect` resolves a sprite to its target buffer key with a single
    /// borrowed lookup and **zero per-particle heap allocation** (the old
    /// `resolve_collection` allocated a `String` for every particle). Every
    /// registered collection maps to itself; the resolution borrows the stored
    /// key (`&str`) rather than cloning it.
    sprite_to_collection: HashMap<String, String>,
    /// First-registered collection key — the fallback target for runtime sprite
    /// names that were never registered at level load.
    default_collection: Option<String>,
    warned_unregistered: HashSet<String>,
}

impl ParticleRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            sprite_to_collection: HashMap::new(),
            default_collection: None,
            warned_unregistered: HashSet::new(),
        }
    }

    /// Pre-register a sprite collection at level load. The first registered
    /// collection becomes the fallback for unregistered runtime names. Caller
    /// is responsible for the matching `SmokePass::register_collection` upload
    /// — this collector only tracks bookkeeping.
    ///
    /// Called at level load for every distinct `BillboardEmitterComponent.sprite`
    /// value in the registry after classname dispatch. Populates the
    /// `sprite_to_collection` map so the render-collect hot path never allocates.
    pub(crate) fn register_sprite(&mut self, collection: &str) {
        let is_new = self
            .sprite_to_collection
            .insert(collection.to_string(), collection.to_string())
            .is_none();
        if is_new && self.default_collection.is_none() {
            self.default_collection = Some(collection.to_string());
        }
        self.buffers.entry(collection.to_string()).or_default();
    }

    /// Drop all per-level state (resolution map, default, warning record).
    /// Buffers are retained so capacity carries across reloads.
    #[allow(dead_code)]
    pub(crate) fn reset_for_level(&mut self) {
        self.sprite_to_collection.clear();
        self.default_collection = None;
        self.warned_unregistered.clear();
        for buf in self.buffers.values_mut() {
            buf.clear();
        }
    }

    /// Walk the registry, bucket by `SpriteVisual.sprite`, pack bytes. Clears
    /// each per-collection buffer first so capacity is reused across frames.
    ///
    /// Mirrors the mesh collector's render-frame cull (`mesh_render.rs`): each
    /// particle is gated against this frame's `visible` cell set, so off-screen /
    /// adjacent-room smoke is never packed for drawing. The world (`None` only
    /// when no level is loaded — particles cannot exist without an emitter, which
    /// requires a level) supplies the `find_leaf` BSP query.
    ///
    /// Cull granularity is the **emitter**, not the particle: a per-particle
    /// `find_leaf` + visible-set scan would regress at the high particle counts
    /// this slice targets. Instead a per-frame `emitter -> visible` memo resolves
    /// each emitter's leaf exactly once (on first sight of one of its particles)
    /// by looking up the emitter's `Transform.position` and `find_leaf`-ing it
    /// (mirroring `emitter_bridge`'s origin lookup), then reuses that decision for
    /// every other particle sharing the emitter.
    ///
    /// **Orphaned particles** (emitter despawned, so its `Transform` is gone —
    /// see `entity_model.md` §8) are **drawn, not culled**: a mid-life puff must
    /// not pop when its emitter dies. Orphans are a rare, cheap tail.
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
    ) {
        for buf in self.buffers.values_mut() {
            buf.clear();
        }

        // Per-frame memo: emitter id -> "is its cell visible this frame". One
        // `find_leaf` per distinct emitter, reused across all its particles.
        let mut emitter_visible: HashMap<EntityId, bool> = HashMap::new();

        for (id, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            let ComponentValue::ParticleState(particle) = value else {
                continue;
            };
            let Ok(transform) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let Ok(visual) = registry.get_component::<SpriteVisual>(id) else {
                continue;
            };

            if !Self::particle_visible(
                world,
                visible,
                registry,
                particle.emitter,
                &mut emitter_visible,
            ) {
                continue;
            }

            // Resolve the target collection key. The common case (sprite was
            // registered at level load) is a borrowed map lookup with no
            // allocation. Only an unregistered runtime sprite takes the cold
            // fallback branch (which may emit a one-time warning).
            let target: &str = match self.sprite_to_collection.get(&visual.sprite) {
                Some(key) => key.as_str(),
                None => match Self::resolve_fallback(
                    &self.default_collection,
                    &mut self.warned_unregistered,
                    &visual.sprite,
                ) {
                    Some(key) => key,
                    None => continue,
                },
            };
            let Some(buf) = self.buffers.get_mut(target) else {
                continue;
            };
            pack_particle_instance(transform, particle, visual, buf);
        }
    }

    /// Cull decision for a single particle, gated at **emitter** granularity.
    ///
    /// - `DrawAll` / no world → always visible (draw-all short-circuit, mirroring
    ///   `mesh_visible`): no `find_leaf` descent on that path.
    /// - Orphaned particle (`emitter` is `None`, or its `Transform` is gone) →
    ///   visible: orphans complete their lifetime, never culled.
    /// - Otherwise resolve the emitter's leaf once (memoized) and test membership
    ///   in the visible cell set.
    fn particle_visible(
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
        registry: &EntityRegistry,
        emitter: Option<EntityId>,
        memo: &mut HashMap<EntityId, bool>,
    ) -> bool {
        // Draw-all short-circuit: every particle draws, so skip the (non-trivial)
        // BSP descent entirely. Also covers the no-level case.
        let (VisibleCells::Culled(_), Some(world)) = (visible, world) else {
            return true;
        };

        // Orphaned particle: no emitter back-reference at all → draw it.
        let Some(emitter) = emitter else {
            return true;
        };

        if let Some(&visible_flag) = memo.get(&emitter) {
            return visible_flag;
        }

        // First sight of this emitter: look up its origin and resolve the leaf.
        // If the emitter's `Transform` is gone (it despawned mid-frame but its
        // particles linger), the particle is orphaned → drawn, not culled.
        let flag = match registry.get_component::<Transform>(emitter) {
            Ok(transform) => {
                let leaf = world.find_leaf(transform.position) as u32;
                mesh_visible_in_leaf(visible, leaf)
            }
            Err(_) => true,
        };
        memo.insert(emitter, flag);
        flag
    }

    /// Cold-path resolution for a sprite that was **not** pre-registered at
    /// level load (so it missed the `sprite_to_collection` map). Falls back to
    /// the default collection, emitting a one-time `log::warn!` per offending
    /// sprite. Returns the borrowed default key, or `None` when no collections
    /// are registered at all (no fallback target available).
    ///
    /// Takes the two fields it touches by reference (not `&mut self`) so the
    /// caller's hot path can borrow `self.buffers` and `self.sprite_to_collection`
    /// disjointly without aliasing. The returned `&str` borrows
    /// `default_collection`; no per-particle allocation occurs on either path.
    fn resolve_fallback<'a>(
        default_collection: &'a Option<String>,
        warned_unregistered: &mut HashSet<String>,
        sprite: &str,
    ) -> Option<&'a str> {
        let fallback = default_collection.as_deref()?;
        if !warned_unregistered.contains(sprite) {
            warned_unregistered.insert(sprite.to_string());
            log::warn!(
                "[ParticleRender] sprite '{sprite}' was not registered at level load; \
                 falling back to default collection '{fallback}'. \
                 Add it to the level-load definition sweep."
            );
        }
        Some(fallback)
    }

    /// Iterate `(collection, bytes)` pairs with non-empty payloads. Order is
    /// HashMap-iteration order; under additive blending, draw order does not
    /// affect the composited result — contributions are commutative.
    pub(crate) fn iter_collections(&self) -> impl Iterator<Item = (&str, &[u8])> + '_ {
        self.buffers
            .iter()
            .filter(|(_, bytes)| !bytes.is_empty())
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
    }
}

impl Default for ParticleRenderCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Pack one particle into the `SPRITE_INSTANCE_SIZE = 32` byte layout that
/// matches the GPU-side `SpriteInstance` layout pinned in `crates/postretro/src/fx/smoke.rs`.
///
/// Layout: `(position.xyz, age) + (size, rotation, opacity, _pad)`.
/// `SpriteVisual.tint` is intentionally not packed — the current GPU layout
/// has no color channel (plan §Non-goals).
fn pack_particle_instance(
    transform: &Transform,
    particle: &ParticleState,
    visual: &SpriteVisual,
    out: &mut Vec<u8>,
) {
    out.extend_from_slice(&transform.position.x.to_ne_bytes());
    out.extend_from_slice(&transform.position.y.to_ne_bytes());
    out.extend_from_slice(&transform.position.z.to_ne_bytes());
    out.extend_from_slice(&particle.age.to_ne_bytes());
    out.extend_from_slice(&visual.size.to_ne_bytes());
    out.extend_from_slice(&visual.rotation.to_ne_bytes());
    out.extend_from_slice(&visual.opacity.to_ne_bytes());
    out.extend_from_slice(&0.0f32.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::smoke::SPRITE_INSTANCE_SIZE;
    use crate::prl::{BspChild, LeafData, LevelWorld};
    use crate::scripting::components::particle::ParticleState;
    use crate::scripting::components::sprite_visual::SpriteVisual;
    use crate::scripting::registry::{EntityRegistry, Transform};
    use glam::Vec3;
    use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

    // The cull path the collector exercises (`particle_visible`) shares the same
    // membership half as the mesh path (`mesh_pass::mesh_visible_in_leaf`), whose
    // own tests pin the membership decision against a synthetic visible-set.
    // Here we drive the collector against a minimal single-leaf world (leaf 0
    // spans all space, so any position lands in leaf 0); a visible set of
    // `Culled([0])` includes it and `Culled([1])` culls it.
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

    /// Spawn an emitter-less particle (orphan back-ref `None`).
    fn spawn_particle(registry: &mut EntityRegistry, position: Vec3, sprite: &str, age: f32) {
        spawn_particle_for(registry, position, sprite, age, None);
    }

    /// Spawn a particle whose `emitter` back-reference is `emitter`.
    fn spawn_particle_for(
        registry: &mut EntityRegistry,
        position: Vec3,
        sprite: &str,
        age: f32,
        emitter: Option<EntityId>,
    ) {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                ParticleState {
                    velocity: [0.0, 0.0, 0.0],
                    age,
                    lifetime: 1.0,
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_curve: [1.0].into(),
                    opacity_curve: [1.0].into(),
                    emitter,
                },
            )
            .unwrap();
        registry
            .set_component(
                id,
                SpriteVisual {
                    sprite: sprite.into(),
                    size: 1.0,
                    opacity: 0.5,
                    rotation: 0.25,
                    tint: [1.0, 1.0, 1.0],
                },
            )
            .unwrap();
    }

    /// Spawn a bare emitter entity (just a `Transform`) and return its id so a
    /// particle can back-reference it. The cull resolves the emitter's origin
    /// via this `Transform` and `find_leaf`-s it.
    fn spawn_emitter(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        registry.spawn(Transform {
            position,
            ..Transform::default()
        })
    }

    #[test]
    fn collect_packs_one_particle_into_registered_collection_buffer() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut registry, Vec3::new(1.0, 2.0, 3.0), "smoke", 0.4);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        let (name, bytes) = pairs[0];
        assert_eq!(name, "smoke");
        assert_eq!(bytes.len(), SPRITE_INSTANCE_SIZE);
        // Byte sanity: position.x at offset 0 and age at offset 12.
        let px = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert!((px - 1.0).abs() < 1e-6);
        assert!((age - 0.4).abs() < 1e-6);
    }

    #[test]
    fn multiple_emitters_one_collection_share_one_record_draw_call() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut registry, Vec3::ZERO, "smoke", 0.1);
        spawn_particle(&mut registry, Vec3::new(0.0, 1.0, 0.0), "smoke", 0.2);
        spawn_particle(&mut registry, Vec3::new(0.0, 2.0, 0.0), "smoke", 0.3);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1, "single collection => single draw");
        assert_eq!(pairs[0].1.len(), 3 * SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn different_collections_produce_separate_draws() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        collector.register_sprite("spark");
        spawn_particle(&mut registry, Vec3::ZERO, "smoke", 0.1);
        spawn_particle(&mut registry, Vec3::ZERO, "spark", 0.1);
        spawn_particle(&mut registry, Vec3::ZERO, "spark", 0.2);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let mut pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        pairs.sort_by_key(|(n, _)| *n);
        assert_eq!(pairs.len(), 2);
        let smoke_bytes = pairs.iter().find(|(n, _)| *n == "smoke").unwrap().1;
        let spark_bytes = pairs.iter().find(|(n, _)| *n == "spark").unwrap().1;
        assert_eq!(smoke_bytes.len(), SPRITE_INSTANCE_SIZE);
        assert_eq!(spark_bytes.len(), 2 * SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn unregistered_sprite_falls_back_to_default_and_warns_once() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke"); // first registered = default
        collector.register_sprite("spark");
        spawn_particle(&mut registry, Vec3::ZERO, "ghost", 0.1);
        spawn_particle(&mut registry, Vec3::ZERO, "ghost", 0.2);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        // Both ghost particles bucket into the default ("smoke") collection.
        let smoke_bytes = pairs.iter().find(|(n, _)| *n == "smoke").unwrap().1;
        assert_eq!(smoke_bytes.len(), 2 * SPRITE_INSTANCE_SIZE);
        // 'ghost' should be recorded as warned-once. A second collect tick
        // must not add a duplicate entry.
        assert!(collector.warned_unregistered.contains("ghost"));
        let warn_count = collector.warned_unregistered.len();
        collector.collect(&registry, None, &VisibleCells::DrawAll);
        assert_eq!(collector.warned_unregistered.len(), warn_count);
    }

    #[test]
    fn map_only_sprite_renders_to_its_own_bucket_when_registered() {
        // Regression for the AC: a `billboard_emitter` map entity with
        // `sprite = "dust"` and no script preset referencing "dust" must still
        // render into the "dust" collection rather than the default fallback.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke"); // default, from a script preset
        collector.register_sprite("dust"); // from a map-only sweep
        spawn_particle(&mut registry, Vec3::new(5.0, 0.0, 0.0), "dust", 0.1);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "dust");
        // No warning emitted: the sprite was registered at level load.
        assert!(collector.warned_unregistered.is_empty());
    }

    #[test]
    fn collect_clears_buffers_between_frames_without_dropping_capacity() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut registry, Vec3::ZERO, "smoke", 0.1);
        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let cap_after_first = collector.buffers["smoke"].capacity();
        assert!(cap_after_first >= SPRITE_INSTANCE_SIZE);

        // Despawn everything: next collect should produce an empty buffer
        // but preserve capacity.
        let ids: Vec<_> = registry
            .iter_with_kind(ComponentKind::ParticleState)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            registry.despawn(id).unwrap();
        }
        collector.collect(&registry, None, &VisibleCells::DrawAll);
        assert!(collector.buffers["smoke"].is_empty());
        assert_eq!(collector.buffers["smoke"].capacity(), cap_after_first);
    }

    #[test]
    fn collect_with_no_registrations_drops_particles_silently() {
        // If level load harvested zero sprites (no emitters anywhere), the
        // collector has no fallback target. Particles existing in that state
        // are anomalous; collect must not panic and must produce no draws.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        spawn_particle(&mut registry, Vec3::ZERO, "smoke", 0.1);

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        assert!(collector.iter_collections().next().is_none());
    }

    #[test]
    fn collect_packs_particle_whose_emitter_is_in_a_visible_cell() {
        // Emitter lands in leaf 0; the visible set includes leaf 0 → the
        // particle is packed. Mirrors `mesh_render::collect_emits_one_visible_*`.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(1.0, 2.0, 3.0));
        spawn_particle_for(
            &mut registry,
            Vec3::new(1.0, 2.0, 3.0),
            "smoke",
            0.4,
            Some(emitter),
        );

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_excludes_particle_whose_emitter_is_in_a_nonvisible_cell() {
        // Emitter lands in leaf 0, but only leaf 1 is visible → the particle is
        // culled out. Mirrors `mesh_render::collect_excludes_mesh_in_nonvisible_cell`.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(1.0, 2.0, 3.0));
        spawn_particle_for(
            &mut registry,
            Vec3::new(1.0, 2.0, 3.0),
            "smoke",
            0.4,
            Some(emitter),
        );

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        assert!(
            collector.iter_collections().next().is_none(),
            "particle whose emitter cell is not visible must not be packed"
        );
    }

    #[test]
    fn collect_groups_emitter_decision_across_its_particles() {
        // Two particles share one emitter (one BSP-leaf lookup gates both). The
        // emitter is in a non-visible cell → neither particle draws.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::ZERO);
        spawn_particle_for(&mut registry, Vec3::ZERO, "smoke", 0.1, Some(emitter));
        spawn_particle_for(
            &mut registry,
            Vec3::new(0.0, 1.0, 0.0),
            "smoke",
            0.2,
            Some(emitter),
        );

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        assert!(collector.iter_collections().next().is_none());

        // Same emitter now visible → both particles draw.
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), 2 * SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_draws_orphaned_particle_when_emitter_despawned() {
        // The emitter is in a non-visible cell, then despawns: the particle's
        // back-reference dangles (its `Transform` is gone). An orphan completes
        // its lifetime — it must be drawn, not culled.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::ZERO);
        spawn_particle_for(&mut registry, Vec3::ZERO, "smoke", 0.4, Some(emitter));

        // While the emitter lives in a non-visible cell, the particle is culled.
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        assert!(collector.iter_collections().next().is_none());

        // Emitter despawns; the particle is now orphaned and must draw despite
        // the same non-visible cell set.
        registry.despawn(emitter).unwrap();
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(
            pairs.len(),
            1,
            "orphaned particle must be drawn, not culled"
        );
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_draws_particle_with_no_emitter_backref() {
        // A particle with `emitter == None` (no back-reference at all) is an
        // orphan from the cull's perspective and is always drawn.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        spawn_particle_for(&mut registry, Vec3::ZERO, "smoke", 0.4, None);

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_draws_all_particles_when_visibility_is_draw_all() {
        // No portal culling this frame: every particle draws regardless of its
        // emitter's cell — no regression. Mirrors the mesh draw-all path.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::ZERO);
        spawn_particle_for(&mut registry, Vec3::ZERO, "smoke", 0.1, Some(emitter));
        spawn_particle_for(
            &mut registry,
            Vec3::new(0.0, 1.0, 0.0),
            "smoke",
            0.2,
            Some(emitter),
        );

        collector.collect(&registry, Some(&world), &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), 2 * SPRITE_INSTANCE_SIZE);
    }

    /// Sibling bench to `particle_sim::bench_500_particles_one_frame_*`,
    /// covering the **render-collect** path: 500 particles sharing one emitter
    /// in a visible cell, so each particle exercises the per-frame cull memo
    /// lookup and the `sprite_to_collection` resolution (the path that used to
    /// allocate a `String` per particle). Same 500-particle scale and < 500µs
    /// budget as the tick bench, so a regression on either hot path is caught.
    #[test]
    #[ignore = "release-mode bench; run with `cargo test --release -- --ignored`"]
    fn bench_500_particles_collect_under_half_a_millisecond() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_leaf_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(1.0, 2.0, 3.0));
        for i in 0..500 {
            spawn_particle_for(
                &mut registry,
                Vec3::new(i as f32 * 0.01, 1.0, 0.0),
                "smoke",
                0.4,
                Some(emitter),
            );
        }
        // Warm the buffers' capacity so the timed run reuses it (matches
        // steady-state production where buffers persist across frames).
        let visible = VisibleCells::Culled(vec![0]);
        collector.collect(&registry, Some(&world), &visible);

        let start = std::time::Instant::now();
        collector.collect(&registry, Some(&world), &visible);
        let elapsed = start.elapsed();

        // Sanity: all 500 packed into the one collection.
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), 500 * SPRITE_INSTANCE_SIZE);
        assert!(
            elapsed.as_micros() < 500,
            "500-particle collect took {elapsed:?}; expected < 500µs"
        );
    }
}
