// Particle render collector: packs ParticleState and projectile SpriteVisual bytes per collection.
// See: context/lib/scripting.md

use std::collections::{HashMap, HashSet};

use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityRegistry, Transform};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE;
use postretro_visibility::VisibleCells;

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
    pub(crate) fn reset_for_level(&mut self) {
        self.sprite_to_collection.clear();
        self.default_collection = None;
        self.warned_unregistered.clear();
        for buf in self.buffers.values_mut() {
            buf.clear();
        }
    }

    /// Walk particle and projectile-body sprites, bucket by `SpriteVisual.sprite`,
    /// and pack bytes. Clears each per-collection buffer first so capacity is
    /// reused across frames.
    ///
    /// Mirrors the mesh collector's render-frame cull (`mesh_render.rs`): each
    /// particle is gated against this frame's `visible` cell set, so off-screen /
    /// adjacent-room smoke is never packed for drawing. The world (`None` only
    /// when no level is loaded — particles cannot exist without an emitter, which
    /// requires a level) supplies the point-to-cell query.
    ///
    /// Cull granularity is the **particle**, not its emitter. Each billboard is
    /// gated by the cell of *its own* world position — the same
    /// `transform.position` that `pack_sprite_instance` writes into the GPU
    /// instance — so a smoke puff that has drifted into a portal-visible cell is
    /// drawn even when its emitter sits behind a wall, and a puff that drifted
    /// out is culled even when its emitter is on-screen. (A per-emitter decision
    /// dropped drifted-in-view particles; that was a correctness bug.)
    /// `ParticleState.emitter` is no longer consulted here; it survives only for
    /// the sim-tick spin-rate lookup.
    ///
    /// **Orphaned particles** (emitter despawned) need no special case: a
    /// particle always carries its own `Transform`, so it is culled or drawn by
    /// its own cell exactly like any other particle.
    ///
    /// **Cost.** The membership half of the cull
    /// (`VisibleCells::Culled(cells).contains`) is a linear scan, so testing it
    /// per particle would be `O(particles x visible_cells)`. Instead this builds
    /// a `HashSet<u32>` of visible cell ids **once per collect** and tests
    /// each particle in `O(1)`; `locate_cell` itself is `O(tree depth)` and cheap.
    #[cfg(test)]
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
    ) {
        self.collect_at_tick(registry, world, visible, 0.0);
    }

    /// Fixed-tick-aware collector entry point used by the renderer path. The
    /// supplied tick is the host simulation epoch (fractional on interpolating
    /// clients), never render-frame delta or local snapshot-arrival time.
    pub(crate) fn collect_at_tick(
        &mut self,
        registry: &EntityRegistry,
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
        presentation_tick: f64,
    ) {
        for buf in self.buffers.values_mut() {
            buf.clear();
        }

        // Build the per-frame O(1) visible-cell membership set once. `None` is
        // the draw-all short-circuit (DrawAll, or no world loaded): every
        // particle draws and no locator descent runs. Otherwise the set is
        // the visible cell ids, hoisting `Culled(_).contains` (a linear
        // scan) out of the per-particle loop.
        let visible_cells: Option<HashSet<u32>> = match (visible, world) {
            (VisibleCells::Culled(cells), Some(_)) => Some(cells.iter().copied().collect()),
            _ => None,
        };

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

            self.collect_sprite(
                transform,
                visual,
                particle.age,
                world,
                visible_cells.as_ref(),
            );
        }

        // A local travelling projectile has `SpriteVisual` but deliberately no
        // `ParticleState`: its visual is persistent until collision/expiry, not
        // a simulated smoke puff. A cadence-enabled body carries its fixed-tick
        // flight age; every other body must retain the exact static age zero.
        for (id, value) in registry.iter_with_kind(ComponentKind::Projectile) {
            let ComponentValue::Projectile(projectile) = value else {
                continue;
            };
            let Ok(transform) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let Ok(visual) = registry.get_component::<SpriteVisual>(id) else {
                continue;
            };
            let age = if projectile.flipbook_active {
                projectile.elapsed_flight_age
            } else {
                0.0
            };
            self.collect_sprite(transform, visual, age, world, visible_cells.as_ref());
        }

        // A host-replicated observer copy intentionally carries no gameplay
        // ProjectileComponent. Its provenance marks the exact visual-only
        // projectile shape, preventing this path from broadening into a generic
        // renderer for every entity that happens to carry SpriteVisual.
        for (id, value) in registry.iter_with_kind(ComponentKind::DescriptorProvenance) {
            let ComponentValue::DescriptorProvenance(provenance) = value else {
                continue;
            };
            if provenance.spawn_path != DescriptorSpawnPath::ProjectilePresentation
                || registry
                    .has_component_kind(id, ComponentKind::Projectile)
                    .unwrap_or(false)
                || registry
                    .has_component_kind(id, ComponentKind::ParticleState)
                    .unwrap_or(false)
            {
                continue;
            }
            let Ok(transform) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let Ok(visual) = registry.get_component::<SpriteVisual>(id) else {
                continue;
            };
            let age = registry
                .projectile_presentation_age(id)
                .ok()
                .filter(|timing| timing.flipbook_active)
                .map(|timing| {
                    ((presentation_tick - f64::from(timing.spawn_tick)).max(0.0) as f32)
                        * crate::frame_timing::TICK_DURATION.as_secs_f32()
                })
                .unwrap_or(0.0);
            self.collect_sprite(transform, visual, age, world, visible_cells.as_ref());
        }
    }

    /// Cull and pack one sprite into its registered collection. The caller passes
    /// a particle's animated age or zero for a persistent projectile body.
    fn collect_sprite(
        &mut self,
        transform: &Transform,
        visual: &SpriteVisual,
        age: f32,
        world: Option<&LevelWorld>,
        visible_cells: Option<&HashSet<u32>>,
    ) {
        // Per-billboard cull: test this sprite's OWN cell (from the same position
        // that gets packed) against the visible set. `None` => draw-all
        // short-circuit, so skip the locator descent entirely.
        if let Some(cells) = visible_cells {
            // `world` is `Some` whenever `visible_cells` is `Some`.
            let world = world.expect("visible_cells implies a loaded world");
            let cell = world.locate_cell(transform.position) as u32;
            if !cells.contains(&cell) {
                return;
            }
        }

        // Resolve the target collection key. The common case (sprite was
        // registered at level load) is a borrowed map lookup with no allocation.
        // Only an unregistered runtime sprite takes the cold fallback branch.
        let target: &str = match self.sprite_to_collection.get(&visual.sprite) {
            Some(key) => key.as_str(),
            None => match Self::resolve_fallback(
                &self.default_collection,
                &mut self.warned_unregistered,
                &visual.sprite,
            ) {
                Some(key) => key,
                None => return,
            },
        };
        let Some(buf) = self.buffers.get_mut(target) else {
            return;
        };
        pack_sprite_instance(transform, age, visual, buf);
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

/// Pack one particle into the shared sprite-instance layout pinned by
/// `postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE` and the
/// `billboard.wgsl` `SpriteInstance` contract.
///
/// Layout: `(position.xyz, age) + (size, rotation, opacity, _pad)`.
/// `SpriteVisual.tint` is intentionally not packed — the current GPU layout
/// has no color channel (plan §Non-goals).
fn pack_sprite_instance(transform: &Transform, age: f32, visual: &SpriteVisual, out: &mut Vec<u8>) {
    let start_len = out.len();
    out.extend_from_slice(&transform.position.x.to_ne_bytes());
    out.extend_from_slice(&transform.position.y.to_ne_bytes());
    out.extend_from_slice(&transform.position.z.to_ne_bytes());
    out.extend_from_slice(&age.to_ne_bytes());
    out.extend_from_slice(&visual.size.to_ne_bytes());
    out.extend_from_slice(&visual.rotation.to_ne_bytes());
    out.extend_from_slice(&visual.opacity.to_ne_bytes());
    out.extend_from_slice(&0.0f32.to_ne_bytes());
    debug_assert_eq!(out.len() - start_len, SPRITE_INSTANCE_SIZE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::particle::ParticleState;
    use postretro_entities::components::projectile::ProjectileComponent;
    use postretro_entities::components::sprite_visual::SpriteVisual;
    use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
    use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
    use postretro_level_loader::{CellData, LevelWorld};

    // The collector culls each billboard by the runtime cell of *its own* world
    // position against the frame's visible set. The membership half mirrors the
    // mesh path (`postretro_render_cpu::mesh_pass::mesh_visible_in_cell`), whose
    // own tests pin the membership decision against a synthetic visible-set.
    //
    // `single_cell_world` is a minimal world where cell 0 spans all space, so any
    // position lands in cell 0; a visible set of `Culled([0])` includes it and
    // `Culled([1])` culls it. Used by the position-agnostic packing/fallback
    // tests where every particle shares one cell.
    fn single_cell_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            cells: vec![CellData {
                bounds_min: Vec3::splat(-1.0e6),
                bounds_max: Vec3::splat(1.0e6),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            }],
            cell_portal_refs: vec![],
            cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
            has_portals: false,
            cell_visibility: None,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: postretro_render_data::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: vec![],
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: postretro_level_loader::KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
        }
    }

    /// A world with two cells split by the `x = 0` plane: a position with
    /// `x >= 0` lands in **cell 0** (front), `x < 0` lands in **cell 1** (back).
    /// This lets a test place a particle and its emitter on opposite sides of
    /// the plane so the cull decision keys on the particle's *own* cell, not the
    /// emitter's.
    fn two_cell_world() -> LevelWorld {
        let mut world = single_cell_world();
        world.cells = vec![
            CellData {
                bounds_min: Vec3::new(0.0, -1.0e6, -1.0e6),
                bounds_max: Vec3::splat(1.0e6),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            },
            CellData {
                bounds_min: Vec3::splat(-1.0e6),
                bounds_max: Vec3::new(0.0, 1.0e6, 1.0e6),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            },
        ];
        world.cell_locator_root = postretro_level_loader::CellLocatorChild::Node(0);
        world.cell_locator_nodes = vec![postretro_level_loader::CellLocatorNodeData {
            plane_normal: Vec3::new(1.0, 0.0, 0.0),
            plane_distance: 0.0,
            front: postretro_level_loader::CellLocatorChild::Cell(0),
            back: postretro_level_loader::CellLocatorChild::Cell(1),
        }];
        world
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

    /// Spawn the static billboard body of a travelling projectile. It has no
    /// `ParticleState`: flight is owned by `ProjectileComponent` and the body
    /// remains visible until the projectile resolves.
    fn spawn_projectile_sprite(
        registry: &mut EntityRegistry,
        position: Vec3,
        sprite: &str,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                ProjectileComponent {
                    direction: [0.0, 0.0, -1.0],
                    speed: 40.0,
                    radius: 0.2,
                    remaining_range: 64.0,
                    remaining_lifetime: 2.0,
                    damage: 25.0,
                    credit_source: "plasma.primary".to_string(),
                    owner_pawn: EntityId::from_raw(1),
                    owner_weapon: EntityId::from_raw(2),
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                },
            )
            .unwrap();
        registry
            .set_component(
                id,
                SpriteVisual {
                    sprite: sprite.into(),
                    size: 0.4,
                    opacity: 0.9,
                    rotation: 0.25,
                    tint: [0.2, 0.8, 1.0],
                },
            )
            .unwrap();
        id
    }

    /// Spawn a bare emitter entity (just a `Transform`) and return its id so a
    /// particle can back-reference it. The emitter's position is *not* consulted
    /// by the cull (that is per-particle now); these helpers place the emitter
    /// only to prove the particle's own cell — not its emitter's — drives the
    /// decision.
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
    fn collect_packs_projectile_sprite_body_without_particle_state() {
        // Regression: projectile bodies attach SpriteVisual but not ParticleState,
        // so the particle-only collector silently omitted them while travelling.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/plasma.png");
        let projectile = spawn_projectile_sprite(
            &mut registry,
            Vec3::new(1.0, 2.0, 3.0),
            "sprites/plasma.png",
        );
        assert!(registry.get_component::<ParticleState>(projectile).is_err());

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        let (name, bytes) = pairs[0];
        assert_eq!(name, "sprites/plasma.png");
        assert_eq!(bytes.len(), SPRITE_INSTANCE_SIZE);
        let px = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert!((px - 1.0).abs() < 1e-6);
        assert!(
            age.abs() <= f32::EPSILON,
            "persistent projectile bodies use frame zero"
        );
    }

    #[test]
    fn collect_packs_projectile_age_only_when_flipbook_is_active() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/animated-plasma");
        let projectile = spawn_projectile_sprite(
            &mut registry,
            Vec3::new(1.0, 2.0, 3.0),
            "sprites/animated-plasma",
        );
        let mut component = registry
            .get_component::<ProjectileComponent>(projectile)
            .expect("projectile component attaches")
            .clone();
        component.elapsed_flight_age = 0.18;
        registry
            .set_component(projectile, component)
            .expect("projectile component updates");

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let bytes = collector
            .iter_collections()
            .next()
            .expect("static projectile is collected")
            .1;
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(age.to_bits(), 0.0_f32.to_bits());

        let mut component = registry
            .get_component::<ProjectileComponent>(projectile)
            .expect("projectile component remains attached")
            .clone();
        component.flipbook_active = true;
        registry
            .set_component(projectile, component)
            .expect("projectile component enables cadence");
        collector.collect(&registry, None, &VisibleCells::DrawAll);
        let bytes = collector
            .iter_collections()
            .next()
            .expect("animated projectile is collected")
            .1;
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert!((age - 0.18).abs() < 1e-6);
    }

    #[test]
    fn collect_keeps_single_and_multi_frame_bodies_static_without_cadence() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        let cases = [
            "sprites/projectiles/single-frame.png",
            "sprites/projectiles/multi-frame-collection",
        ];
        for (index, sprite) in cases.iter().enumerate() {
            collector.register_sprite(sprite);
            let projectile =
                spawn_projectile_sprite(&mut registry, Vec3::new(index as f32, 0.0, 0.0), sprite);
            let mut component = registry
                .get_component::<ProjectileComponent>(projectile)
                .unwrap()
                .clone();
            component.elapsed_flight_age = 0.18;
            // A collection name alone must not opt a body into flipbook playback.
            component.flipbook_active = false;
            registry.set_component(projectile, component).unwrap();
        }

        collector.collect(&registry, None, &VisibleCells::DrawAll);
        for sprite in cases {
            let bytes = collector
                .buffers
                .get(sprite)
                .expect("registered body is collected");
            let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
            assert_eq!(
                age.to_bits(),
                0.0_f32.to_bits(),
                "{sprite} remains on frame zero until frameDurationMs is authored"
            );
        }
    }

    #[test]
    fn collect_delayed_remote_projectile_uses_original_fixed_spawn_tick() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/animated-plasma");
        let id = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                postretro_entities::provenance::DescriptorProvenance {
                    canonical_name: "reference_plasma_bolt".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::ProjectilePresentation,
                },
            )
            .expect("remote presentation provenance attaches");
        registry
            .set_component(
                id,
                SpriteVisual {
                    sprite: "sprites/animated-plasma".to_string(),
                    size: 0.4,
                    opacity: 0.9,
                    rotation: 0.25,
                    tint: [0.2, 0.8, 1.0],
                },
            )
            .expect("remote sprite visual attaches");
        registry
            .set_projectile_presentation_age(
                id,
                postretro_entities::components::projectile::ProjectilePresentationAge {
                    spawn_tick: 60,
                    flipbook_active: false,
                },
            )
            .expect("remote presentation timing attaches");

        collector.collect_at_tick(&registry, None, &VisibleCells::DrawAll, 71.0);
        let bytes = collector
            .iter_collections()
            .next()
            .expect("remote projectile is collected")
            .1;
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(age.to_bits(), 0.0_f32.to_bits());

        registry
            .set_projectile_presentation_age(
                id,
                postretro_entities::components::projectile::ProjectilePresentationAge {
                    spawn_tick: 60,
                    flipbook_active: true,
                },
            )
            .expect("remote presentation cadence enables");
        collector.collect_at_tick(&registry, None, &VisibleCells::DrawAll, 71.0);
        let bytes = collector
            .iter_collections()
            .next()
            .expect("animated remote projectile is collected")
            .1;
        let age = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let expected = 11.0 * crate::frame_timing::TICK_DURATION.as_secs_f32();
        assert!(
            (age - expected).abs() < 1e-6,
            "delayed materialization catches up from the authoritative spawn tick"
        );
    }

    #[test]
    fn collect_matches_flipbook_age_across_local_predicted_and_presentation_bodies() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        const SPRITE: &str = "sprites/projectiles/animated-plasma";
        let elapsed = 11.0 * crate::frame_timing::TICK_DURATION.as_secs_f32();
        collector.register_sprite(SPRITE);

        let local = spawn_projectile_sprite(&mut registry, Vec3::ZERO, SPRITE);
        let predicted = spawn_projectile_sprite(&mut registry, Vec3::X, SPRITE);
        for (id, predicted_shot_id) in [(local, None), (predicted, Some(9))] {
            let mut component = registry
                .get_component::<ProjectileComponent>(id)
                .unwrap()
                .clone();
            component.elapsed_flight_age = elapsed;
            component.flipbook_active = true;
            component.predicted_shot_id = predicted_shot_id;
            registry.set_component(id, component).unwrap();
        }

        let presentation = registry.spawn(Transform {
            position: Vec3::new(2.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                presentation,
                postretro_entities::provenance::DescriptorProvenance {
                    canonical_name: "reference_plasma_bolt".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::ProjectilePresentation,
                },
            )
            .unwrap();
        registry
            .set_component(
                presentation,
                SpriteVisual {
                    sprite: SPRITE.to_string(),
                    size: 0.4,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                },
            )
            .unwrap();
        registry
            .set_projectile_presentation_age(
                presentation,
                postretro_entities::components::projectile::ProjectilePresentationAge {
                    spawn_tick: 60,
                    flipbook_active: true,
                },
            )
            .unwrap();

        collector.collect_at_tick(&registry, None, &VisibleCells::DrawAll, 71.0);
        let bytes = collector
            .buffers
            .get(SPRITE)
            .expect("all three bodies share the animated collection");
        assert_eq!(bytes.len(), 3 * SPRITE_INSTANCE_SIZE);
        for instance in bytes.chunks_exact(SPRITE_INSTANCE_SIZE) {
            let age = f32::from_ne_bytes(instance[12..16].try_into().unwrap());
            assert!(
                (age - elapsed).abs() < 1e-6,
                "all three vantage paths must produce the same flipbook frame clock"
            );
        }
    }

    #[test]
    fn collect_does_not_broaden_bare_sprite_visual_into_projectile_rendering() {
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                SpriteVisual {
                    sprite: "sprites/decorative.png".to_string(),
                    size: 1.0,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                },
            )
            .unwrap();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/decorative.png");

        collector.collect(&registry, None, &VisibleCells::DrawAll);

        assert_eq!(collector.iter_collections().count(), 0);
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
    fn collect_packs_particle_in_a_visible_cell() {
        // Particle lands in cell 0; the visible set includes cell 0 => packed.
        // Mirrors `mesh_render::collect_emits_one_visible_*`.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
        spawn_particle(&mut registry, Vec3::new(1.0, 2.0, 3.0), "smoke", 0.4);

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_excludes_particle_in_a_nonvisible_cell() {
        // Particle lands in cell 0, but only cell 1 is visible => culled out.
        // Mirrors `mesh_render::collect_excludes_mesh_in_nonvisible_cell`.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
        spawn_particle(&mut registry, Vec3::new(1.0, 2.0, 3.0), "smoke", 0.4);

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        assert!(
            collector.iter_collections().next().is_none(),
            "particle whose own cell is not visible must not be packed"
        );
    }

    #[test]
    fn collect_draws_particle_that_drifted_into_view_from_a_hidden_emitter() {
        // The exact regression the per-emitter cull caused. The emitter sits in
        // the BACK half (cell 1, NOT visible), but the particle has drifted into
        // the FRONT half (cell 0, visible). The billboard must draw: it is in
        // view, even though its emitter's cell is culled.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(-5.0, 0.0, 0.0)); // cell 1
        spawn_particle_for(
            &mut registry,
            Vec3::new(5.0, 0.0, 0.0), // cell 0 - drifted into view
            "smoke",
            0.4,
            Some(emitter),
        );

        // Only cell 0 is visible (the emitter's cell 1 is hidden behind a wall).
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(
            pairs.len(),
            1,
            "particle that drifted into a visible cell must draw even when its emitter is hidden"
        );
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_culls_particle_that_drifted_out_of_view_from_a_visible_emitter() {
        // The mirror case. The emitter sits in the FRONT half (cell 0, visible),
        // but the particle has drifted into the BACK half (cell 1, NOT visible).
        // The billboard must be culled — it is out of view — even though its
        // emitter is on-screen.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0)); // cell 0
        spawn_particle_for(
            &mut registry,
            Vec3::new(-5.0, 0.0, 0.0), // cell 1 - drifted out of view
            "smoke",
            0.4,
            Some(emitter),
        );

        // Only cell 0 (the emitter's own cell) is visible.
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        assert!(
            collector.iter_collections().next().is_none(),
            "particle that drifted out of view must be culled even when its emitter is on-screen"
        );
    }

    #[test]
    fn collect_culls_two_particles_of_one_emitter_independently_by_their_own_cells() {
        // Two particles share one emitter (in the visible front cell 0) but have
        // drifted apart: one stays in cell 0 (visible), one crossed into cell 1
        // (hidden). The cull is per-particle, so exactly one draws — the shared
        // emitter does NOT gate both alike.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0)); // cell 0
        spawn_particle_for(
            &mut registry,
            Vec3::new(5.0, 0.0, 0.0), // cell 0 - visible
            "smoke",
            0.1,
            Some(emitter),
        );
        spawn_particle_for(
            &mut registry,
            Vec3::new(-5.0, 0.0, 0.0), // cell 1 - hidden
            "smoke",
            0.2,
            Some(emitter),
        );

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(
            pairs.len(),
            1,
            "the cell-0 particle draws, the cell-1 one does not"
        );
        assert_eq!(
            pairs[0].1.len(),
            SPRITE_INSTANCE_SIZE,
            "exactly one of the two sibling particles is packed"
        );
    }

    #[test]
    fn collect_culls_or_draws_orphaned_particle_by_its_own_cell() {
        // An orphaned particle (emitter despawned) gets no special case: it is
        // culled or drawn purely by its own cell, like any other particle.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0));
        // Particle in the hidden back cell 1.
        spawn_particle_for(
            &mut registry,
            Vec3::new(-5.0, 0.0, 0.0),
            "smoke",
            0.4,
            Some(emitter),
        );

        // Orphan it: its own cell (1) is not visible => culled, no special case.
        registry.despawn(emitter).unwrap();
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        assert!(
            collector.iter_collections().next().is_none(),
            "orphaned particle in a hidden cell is culled by its own cell"
        );

        // Make the orphan's own cell visible => it draws.
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1, "orphaned particle in a visible cell draws");
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_culls_particle_with_no_emitter_backref_by_its_own_cell() {
        // A particle with `emitter == None` is culled by its own cell, same as
        // any other — there is no orphan/no-backref escape hatch anymore.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        // Particle in cell 1, which is not visible => culled.
        spawn_particle_for(&mut registry, Vec3::new(-5.0, 0.0, 0.0), "smoke", 0.4, None);

        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![0]));
        assert!(
            collector.iter_collections().next().is_none(),
            "no-backref particle in a hidden cell is culled by its own cell"
        );

        // Its own cell becomes visible => it draws.
        collector.collect(&registry, Some(&world), &VisibleCells::Culled(vec![1]));
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_draws_all_particles_when_visibility_is_draw_all() {
        // No portal culling this frame: every particle draws regardless of any
        // cell (no locator descent runs) — no regression. Both particles are
        // in the hidden back cell 1, yet `DrawAll` packs both. Mirrors the mesh
        // draw-all path.
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(-5.0, 0.0, 0.0));
        spawn_particle_for(
            &mut registry,
            Vec3::new(-5.0, 0.0, 0.0),
            "smoke",
            0.1,
            Some(emitter),
        );
        spawn_particle_for(
            &mut registry,
            Vec3::new(-5.0, 1.0, 0.0),
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
    /// covering the **render-collect** path: 500 particles in a visible cell, so
    /// each particle exercises the per-billboard cull — `locate_cell` plus the
    /// O(1) `HashSet<u32>` visible-cell membership test built once per collect —
    /// and the `sprite_to_collection` resolution (the path that used to allocate
    /// a `String` per particle). Same 500-particle scale and < 500µs budget as
    /// the tick bench, so a regression on either hot path is caught.
    #[test]
    #[ignore = "release-mode bench; run with `cargo test --release -- --ignored`"]
    fn bench_500_particles_collect_under_half_a_millisecond() {
        let mut registry = EntityRegistry::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
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
