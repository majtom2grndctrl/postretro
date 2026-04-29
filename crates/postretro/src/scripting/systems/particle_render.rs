// Particle render collector: walks ParticleState entities and packs SpriteInstance bytes per collection.
// See: context/lib/scripting.md

use std::collections::{HashMap, HashSet};

use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityRegistry, Transform};

/// Per-collection scratch buffers and warning state for the particle render
/// path. Owned by the game layer (not the renderer) so the wgpu boundary stays
/// inside `SmokePass`.
pub(crate) struct ParticleRenderCollector {
    buffers: HashMap<String, Vec<u8>>,
    registered: HashSet<String>,
    default_collection: Option<String>,
    warned_unregistered: HashSet<String>,
}

impl ParticleRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            registered: HashSet::new(),
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
    /// value in the registry after classname dispatch.
    pub(crate) fn register_sprite(&mut self, collection: &str) {
        if self.registered.insert(collection.to_string()) && self.default_collection.is_none() {
            self.default_collection = Some(collection.to_string());
        }
        self.buffers.entry(collection.to_string()).or_default();
    }

    /// Drop all per-level state (registered set, default, warning record).
    /// Buffers are retained so capacity carries across reloads.
    #[allow(dead_code)]
    pub(crate) fn reset_for_level(&mut self) {
        self.registered.clear();
        self.default_collection = None;
        self.warned_unregistered.clear();
        for buf in self.buffers.values_mut() {
            buf.clear();
        }
    }

    /// Walk the registry, bucket by `SpriteVisual.sprite`, pack bytes. Clears
    /// each per-collection buffer first so capacity is reused across frames.
    pub(crate) fn collect(&mut self, registry: &EntityRegistry) {
        for buf in self.buffers.values_mut() {
            buf.clear();
        }

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

            let Some(target) = self.resolve_collection(&visual.sprite) else {
                continue;
            };
            let Some(buf) = self.buffers.get_mut(&target) else {
                continue;
            };
            pack_particle_instance(transform, particle, visual, buf);
        }
    }

    /// Map a `SpriteVisual.sprite` to the collection key whose buffer should
    /// receive the packed bytes. Falls back to the default collection (with a
    /// one-time `log::warn!`) when the sprite name was not pre-registered at
    /// level load. Returns `None` only when no collections are registered at
    /// all (no fallback target available).
    ///
    /// Returns an owned `String` so the caller can re-borrow `self.buffers`
    /// mutably without aliasing this method's `&mut self`. At particle scale
    /// the per-call allocation is dwarfed by the pack itself; future profiling
    /// could pre-cache a `HashMap<sprite -> collection>` if it ever shows up.
    fn resolve_collection(&mut self, sprite: &str) -> Option<String> {
        if self.registered.contains(sprite) {
            return Some(sprite.to_string());
        }
        let fallback = self.default_collection.clone()?;
        if self.warned_unregistered.insert(sprite.to_string()) {
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
    use crate::scripting::components::particle::ParticleState;
    use crate::scripting::components::sprite_visual::SpriteVisual;
    use crate::scripting::registry::{EntityRegistry, Transform};
    use glam::Vec3;

    fn spawn_particle(registry: &mut EntityRegistry, position: Vec3, sprite: &str, age: f32) {
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
                    size_curve: vec![1.0],
                    opacity_curve: vec![1.0],
                    emitter: None,
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

    #[test]
    fn collect_packs_one_particle_into_registered_collection_buffer() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut registry, Vec3::new(1.0, 2.0, 3.0), "smoke", 0.4);

        collector.collect(&registry);
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

        collector.collect(&registry);
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

        collector.collect(&registry);
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

        collector.collect(&registry);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        // Both ghost particles bucket into the default ("smoke") collection.
        let smoke_bytes = pairs.iter().find(|(n, _)| *n == "smoke").unwrap().1;
        assert_eq!(smoke_bytes.len(), 2 * SPRITE_INSTANCE_SIZE);
        // 'ghost' should be recorded as warned-once. A second collect tick
        // must not add a duplicate entry.
        assert!(collector.warned_unregistered.contains("ghost"));
        let warn_count = collector.warned_unregistered.len();
        collector.collect(&registry);
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

        collector.collect(&registry);
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
        collector.collect(&registry);
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
        collector.collect(&registry);
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

        collector.collect(&registry);
        assert!(collector.iter_collections().next().is_none());
    }
}
