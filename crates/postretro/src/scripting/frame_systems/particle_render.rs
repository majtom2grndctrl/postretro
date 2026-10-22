// Particle-pool and projectile billboard collection into shared renderer instance bytes.
// See: context/lib/entity_model.md §8 · context/lib/rendering_pipeline.md §7.4

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE;
use postretro_sim::particle_pool::ParticlePool;
use postretro_visibility::{Frustum, VisibleCells};

struct CollectionBuffer {
    name: String,
    bytes: Vec<u8>,
    registered: bool,
}

/// The game layer owns collection bookkeeping; GPU resources remain in SmokePass.
pub(crate) struct ParticleRenderCollector {
    buffers: Vec<CollectionBuffer>,
    sprite_to_collection: HashMap<String, usize>,
    default_collection: Option<usize>,
    warned_unregistered: HashSet<String>,
    visible_cells: HashSet<u32>,
    cull_cells: bool,
}

impl ParticleRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            buffers: Vec::new(),
            sprite_to_collection: HashMap::new(),
            default_collection: None,
            warned_unregistered: HashSet::new(),
            visible_cells: HashSet::new(),
            cull_cells: false,
        }
    }

    /// Return an index stable for the loaded level. Registration and renderer
    /// upload happen together at level load, before producers resolve blocks.
    pub(crate) fn register_sprite(&mut self, collection: &str) -> usize {
        if let Some(&index) = self.sprite_to_collection.get(collection) {
            return index;
        }
        // Reuse a prior level's byte buffer when the same collection returns.
        let index = self
            .buffers
            .iter()
            .position(|buffer| buffer.name == collection)
            .unwrap_or_else(|| {
                self.buffers.push(CollectionBuffer {
                    name: collection.to_owned(),
                    bytes: Vec::new(),
                    registered: false,
                });
                self.buffers.len() - 1
            });
        self.buffers[index].registered = true;
        self.sprite_to_collection
            .insert(collection.to_owned(), index);
        self.default_collection.get_or_insert(index);
        index
    }

    /// Resolve once when minting a parameter block, outside the particle hot loop.
    /// Projectile bodies also use this path because they retain string components.
    pub(crate) fn resolve_collection(&mut self, collection: &str) -> Option<usize> {
        if let Some(&index) = self.sprite_to_collection.get(collection) {
            return Some(index);
        }
        let fallback = self.default_collection?;
        if !self.warned_unregistered.contains(collection) {
            self.warned_unregistered.insert(collection.to_owned());
            log::warn!(
                "[ParticleRender] collection id '{collection}' was not registered at level load; \
                 falling back to default collection '{}'. \
                 Add it to the level-load definition sweep.",
                self.buffers[fallback].name,
            );
        }
        Some(fallback)
    }

    /// Forget level registrations and warnings while retaining high-water scratch.
    pub(crate) fn reset_for_level(&mut self) {
        self.sprite_to_collection.clear();
        self.default_collection = None;
        self.warned_unregistered.clear();
        self.visible_cells.clear();
        self.cull_cells = false;
        for buffer in &mut self.buffers {
            buffer.bytes.clear();
            buffer.registered = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        particles: &ParticlePool,
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
    ) {
        self.collect_at_tick(registry, particles, world, visible, 0.0, None);
    }

    /// The optional frustum is the full camera frustum, never a narrowed portal
    /// frustum. Exact own-position cell culling follows the conservative pretest.
    /// `presentation_tick` is the host simulation epoch, fractional on clients.
    pub(crate) fn collect_at_tick(
        &mut self,
        registry: &EntityRegistry,
        particles: &ParticlePool,
        world: Option<&LevelWorld>,
        visible: &VisibleCells,
        presentation_tick: f64,
        frustum: Option<&Frustum>,
    ) {
        for buffer in &mut self.buffers {
            buffer.bytes.clear();
        }
        self.visible_cells.clear();
        self.cull_cells = if let (VisibleCells::Culled(cells), Some(_)) = (visible, world) {
            self.visible_cells.extend(cells.iter().copied());
            true
        } else {
            false
        };

        for particle in particles.iter() {
            if frustum.is_some_and(|frustum| {
                quad_outside_frustum(particle.position, particle.size, frustum)
            }) || !self.position_visible(particle.position, world)
            {
                continue;
            }
            let Some(buffer) = self
                .buffers
                .get_mut(particle.collection)
                .filter(|buffer| buffer.registered)
            else {
                continue;
            };
            pack_sprite_instance(
                particle.position,
                particle.age,
                particle.size,
                particle.rotation,
                particle.opacity,
                &mut buffer.bytes,
            );
        }

        // Gameplay projectiles remain individually addressed registry entities.
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
            self.collect_sprite(transform, visual, gameplay_sprite_age(projectile), world);
        }

        // Provenance restricts this to replicated projectile display copies.
        for (id, value) in registry.iter_with_kind(ComponentKind::DescriptorProvenance) {
            let ComponentValue::DescriptorProvenance(provenance) = value else {
                continue;
            };
            if provenance.spawn_path != DescriptorSpawnPath::ProjectilePresentation
                || registry
                    .has_component_kind(id, ComponentKind::Projectile)
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
            let age = presentation_sprite_age(registry, id, presentation_tick);
            self.collect_sprite(transform, visual, age, world);
        }
    }

    /// The registered collection at `index`, for a draw that buckets by the
    /// indices [`Self::resolve_collection`] hands out (the billboard ghosts).
    pub(crate) fn registered_collection_name(&self, index: usize) -> Option<&str> {
        self.buffers
            .get(index)
            .filter(|buffer| buffer.registered)
            .map(|buffer| buffer.name.as_str())
    }

    fn position_visible(&self, position: Vec3, world: Option<&LevelWorld>) -> bool {
        !self.cull_cells
            || self.visible_cells.contains(
                &(world
                    .expect("cell culling requires a loaded world")
                    .locate_cell(position) as u32),
            )
    }

    fn collect_sprite(
        &mut self,
        transform: &Transform,
        visual: &SpriteVisual,
        age: f32,
        world: Option<&LevelWorld>,
    ) {
        if !self.position_visible(transform.position, world) {
            return;
        }
        let Some(index) = self.resolve_collection(&visual.collection) else {
            return;
        };
        pack_sprite_instance(
            transform.position,
            age,
            visual.size,
            visual.rotation,
            visual.opacity,
            &mut self.buffers[index].bytes,
        );
    }

    pub(crate) fn iter_collections(&self) -> impl Iterator<Item = (&str, &[u8])> + '_ {
        self.buffers
            .iter()
            .filter(|buffer| buffer.registered && !buffer.bytes.is_empty())
            .map(|buffer| (buffer.name.as_str(), buffer.bytes.as_slice()))
    }
}

impl Default for ParticleRenderCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// A gameplay projectile's flipbook age: its flight age while an authored
/// cadence advances it, else 0 (a static body).
fn gameplay_sprite_age(projectile: &ProjectileComponent) -> f32 {
    if projectile.flipbook_active {
        projectile.elapsed_flight_age
    } else {
        0.0
    }
}

/// A projectile display copy's flipbook age at `presentation_tick`, the host
/// simulation epoch its fixed spawn tick counts from.
fn presentation_sprite_age(registry: &EntityRegistry, id: EntityId, presentation_tick: f64) -> f32 {
    registry
        .projectile_presentation_age(id)
        .ok()
        .filter(|timing| timing.flipbook_active)
        .map(|timing| {
            ((presentation_tick - f64::from(timing.spawn_tick)).max(0.0) as f32)
                * crate::frame_timing::TICK_DURATION.as_secs_f32()
        })
        .unwrap_or(0.0)
}

/// A projectile body's billboard and flipbook age, exactly as the billboard
/// pass draws it: a gameplay projectile's, or a projectile display copy's
/// (presentation provenance, no gameplay state). `None` for every other
/// entity, so a bare `SpriteVisual` is never a billboard body.
pub(crate) fn projectile_sprite_body(
    registry: &EntityRegistry,
    id: EntityId,
    presentation_tick: f64,
) -> Option<(&SpriteVisual, f32)> {
    let visual = registry.get_component::<SpriteVisual>(id).ok()?;
    if let Ok(projectile) = registry.get_component::<ProjectileComponent>(id) {
        return Some((visual, gameplay_sprite_age(projectile)));
    }
    let provenance = registry.get_component::<DescriptorProvenance>(id).ok()?;
    (provenance.spawn_path == DescriptorSpawnPath::ProjectilePresentation).then(|| {
        (
            visual,
            presentation_sprite_age(registry, id, presentation_tick),
        )
    })
}

/// The shader expands a square with half-side `size / 2` then rotates it in the
/// camera plane. Its circumradius bounds every corner at every rotation.
fn quad_outside_frustum(position: Vec3, size: f32, frustum: &Frustum) -> bool {
    let radius = size.abs() * std::f32::consts::FRAC_1_SQRT_2 + 1.0e-4;
    frustum
        .planes
        .iter()
        .any(|plane| plane.normal.dot(position) + plane.dist < -radius)
}

/// Match billboard.wgsl SpriteInstance's two vec4s. Tint has no GPU channel.
fn pack_sprite_instance(
    position: Vec3,
    age: f32,
    size: f32,
    rotation: f32,
    opacity: f32,
    out: &mut Vec<u8>,
) {
    let start_len = out.len();
    for value in [
        position.x, position.y, position.z, age, size, rotation, opacity, 0.0,
    ] {
        out.extend_from_slice(&value.to_ne_bytes());
    }
    debug_assert_eq!(out.len() - start_len, SPRITE_INSTANCE_SIZE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
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
            sh_storage: postretro_level_loader::ShStorage::Legacy,
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
            vertex_spec_owners: Vec::new(),
            gpu_lighting_payloads: Default::default(),
            lightmap_storage: Default::default(),
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: postretro_level_loader::KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            environment_volumes: Default::default(),
            teleporters: Default::default(),
            ghost_links: Default::default(),
            environment_push_forces: Vec::new(),
            reflection_probes: None,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
            cluster_directory: None,
            cell_residency_set: None,
            prl_read_counters: None,
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

    /// Spawn through the production pool path. Collection names resolve only here.
    fn spawn_particle(
        pool: &mut ParticlePool,
        collector: &mut ParticleRenderCollector,
        position: Vec3,
        sprite: &str,
    ) {
        spawn_particle_for(pool, collector, position, sprite, None);
    }

    fn spawn_particle_for(
        pool: &mut ParticlePool,
        collector: &mut ParticleRenderCollector,
        position: Vec3,
        sprite: &str,
        emitter: Option<EntityId>,
    ) {
        let parameters = postretro_sim::particle_pool::ParticleParameters {
            lifetime: 10.0,
            buoyancy: 0.0,
            drag: 0.0,
            size_curve: [1.0].into(),
            opacity_curve: [0.5].into(),
            collection: collector.resolve_collection(sprite).unwrap_or(usize::MAX),
            tint: [1.0; 3],
        };
        assert!(pool.spawn_emitter(
            emitter.unwrap_or(EntityId::from_raw(1)),
            &parameters,
            position,
            Vec3::ZERO,
            0.0
        ));
    }

    /// Projectile flight stays in the registry and the body persists until impact.
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
                    source_sounds: None,
                    predicted_visible: true,
                    source_action: None,
                    source_shot: None,
                    knockback_impulse: [0.0; 3],
                    direction: [0.0, 0.0, -1.0],
                    speed: 40.0,
                    radius: 0.2,
                    remaining_range: 64.0,
                    remaining_lifetime: 2.0,
                    damage: 25.0,
                    credit_source: "plasma.primary".to_string(),
                    owner_pawn: EntityId::from_raw(1),
                    owner_weapon: EntityId::from_raw(2),
                    attackerless: false,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: None,
                    source_weapon: None,
                    activation: None,
                },
            )
            .unwrap();
        registry
            .set_component(
                id,
                SpriteVisual {
                    collection: sprite.into(),
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
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(1.0, 2.0, 3.0),
            "smoke",
        );

        particles.step(&registry, 0.4, |_| (0.0, Vec3::ZERO));

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
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
    fn collect_packs_registry_projectile_sprite_body_without_pool_particles() {
        // Regression: the travelling body must still draw beside pooled particles.
        let mut registry = EntityRegistry::new();
        let particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/plasma.png");
        spawn_projectile_sprite(
            &mut registry,
            Vec3::new(1.0, 2.0, 3.0),
            "sprites/plasma.png",
        );
        assert!(particles.is_empty());

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
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
        let particles = ParticlePool::new();
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

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
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
        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
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
        let particles = ParticlePool::new();
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

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        for sprite in cases {
            let bytes = collector
                .iter_collections()
                .find(|(name, _)| *name == sprite)
                .expect("registered body is collected")
                .1;
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
        let particles = ParticlePool::new();
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
                    collection: "sprites/animated-plasma".to_string(),
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

        collector.collect_at_tick(
            &registry,
            &particles,
            None,
            &VisibleCells::DrawAll,
            71.0,
            None,
        );
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
        collector.collect_at_tick(
            &registry,
            &particles,
            None,
            &VisibleCells::DrawAll,
            71.0,
            None,
        );
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
        let particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        const SPRITE: &str = "sprites/projectiles/animated-plasma";
        let elapsed = 11.0 * crate::frame_timing::TICK_DURATION.as_secs_f32();
        collector.register_sprite(SPRITE);

        let local = spawn_projectile_sprite(&mut registry, Vec3::ZERO, SPRITE);
        let predicted = spawn_projectile_sprite(&mut registry, Vec3::X, SPRITE);
        let shot_id = postretro_foundation::ShotId::from_parts(
            4,
            9,
            postretro_foundation::ActivationLane::Primary,
            0,
        );
        for (id, predicted_shot_id) in [(local, None), (predicted, Some(shot_id))] {
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
                    collection: SPRITE.to_string(),
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

        collector.collect_at_tick(
            &registry,
            &particles,
            None,
            &VisibleCells::DrawAll,
            71.0,
            None,
        );
        let bytes = collector
            .iter_collections()
            .find(|(name, _)| *name == SPRITE)
            .expect("all three bodies share the animated collection")
            .1;
        assert_eq!(bytes.len(), 3 * SPRITE_INSTANCE_SIZE);
        for instance in bytes.as_chunks::<SPRITE_INSTANCE_SIZE>().0 {
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
        let particles = ParticlePool::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                SpriteVisual {
                    collection: "sprites/decorative.png".to_string(),
                    size: 1.0,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                },
            )
            .unwrap();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("sprites/decorative.png");

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);

        assert_eq!(collector.iter_collections().count(), 0);
    }

    #[test]
    fn multiple_emitters_one_collection_share_one_record_draw_call() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "smoke");
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(0.0, 1.0, 0.0),
            "smoke",
        );
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(0.0, 2.0, 0.0),
            "smoke",
        );

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1, "single collection => single draw");
        assert_eq!(pairs[0].1.len(), 3 * SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn different_collections_produce_separate_draws() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        collector.register_sprite("spark");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "smoke");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "spark");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "spark");

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        let mut pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        pairs.sort_by_key(|(n, _)| *n);
        assert_eq!(pairs.len(), 2);
        let smoke_bytes = pairs.iter().find(|(n, _)| *n == "smoke").unwrap().1;
        let spark_bytes = pairs.iter().find(|(n, _)| *n == "spark").unwrap().1;
        assert_eq!(smoke_bytes.len(), SPRITE_INSTANCE_SIZE);
        assert_eq!(spark_bytes.len(), 2 * SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn unregistered_collection_falls_back_to_default_and_warns_once() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke"); // first registered = default
        collector.register_sprite("spark");
        let capture = LogCapture::start();
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "ghost");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "ghost");

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        // Both ghost particles bucket into the default ("smoke") collection.
        let smoke_bytes = pairs.iter().find(|(n, _)| *n == "smoke").unwrap().1;
        assert_eq!(smoke_bytes.len(), 2 * SPRITE_INSTANCE_SIZE);
        capture.assert_logged_once(
            Level::Warn,
            "[ParticleRender] collection id 'ghost' was not registered at level load",
        );
        // A second collect tick must not add a duplicate warning.
        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        capture.assert_logged_once(
            Level::Warn,
            "[ParticleRender] collection id 'ghost' was not registered at level load",
        );
    }

    #[test]
    fn distinct_unregistered_collections_each_warn_once() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let capture = LogCapture::start();
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "ghost-a");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "ghost-a");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "ghost-b");

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);

        capture.assert_logged_once(
            Level::Warn,
            "[ParticleRender] collection id 'ghost-a' was not registered at level load",
        );
        capture.assert_logged_once(
            Level::Warn,
            "[ParticleRender] collection id 'ghost-b' was not registered at level load",
        );
        assert_eq!(
            capture
                .records()
                .iter()
                .filter(|record| {
                    record.level == Level::Warn
                        && record.message.contains("was not registered at level load")
                })
                .count(),
            2,
            "warning granularity is one record per unregistered collection id"
        );
    }

    #[test]
    fn map_only_collection_renders_to_its_own_bucket_when_registered() {
        // Regression for the AC: a `billboard_emitter` map entity with
        // `sprite = "dust"` and no script preset referencing "dust" must still
        // render into the "dust" collection rather than the default fallback.
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke"); // default, from a script preset
        collector.register_sprite("dust"); // from a map-only sweep
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(5.0, 0.0, 0.0),
            "dust",
        );

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "dust");
        // No warning emitted: the collection id was registered at level load.
        assert!(collector.warned_unregistered.is_empty());
    }

    #[test]
    fn collect_clears_buffers_between_frames_without_dropping_capacity() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "smoke");
        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        let cap_after_first = collector.buffers[collector.sprite_to_collection["smoke"]]
            .bytes
            .capacity();
        assert!(cap_after_first >= SPRITE_INSTANCE_SIZE);

        // Clear the pool: the next collect empties bytes but retains capacity.
        particles.clear();
        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        assert!(
            collector.buffers[collector.sprite_to_collection["smoke"]]
                .bytes
                .is_empty()
        );
        assert_eq!(
            collector.buffers[collector.sprite_to_collection["smoke"]]
                .bytes
                .capacity(),
            cap_after_first
        );
    }

    #[test]
    fn collect_with_no_registrations_drops_particles_silently() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        // If level load harvested zero sprites (no emitters anywhere), the
        // collector has no fallback target. Particles existing in that state
        // are anomalous; collect must not panic and must produce no draws.
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "smoke");
        let capture = LogCapture::start();

        collector.collect(&registry, &particles, None, &VisibleCells::DrawAll);
        assert!(collector.iter_collections().next().is_none());
        capture.assert_not_logged(Level::Warn, "was not registered at level load");
    }

    #[test]
    fn collect_packs_particle_in_a_visible_cell() {
        // Particle lands in cell 0; the visible set includes cell 0 => packed.
        // Mirrors `mesh_render::collect_emits_one_visible_*`.
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(1.0, 2.0, 3.0),
            "smoke",
        );

        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_excludes_particle_in_a_nonvisible_cell() {
        // Particle lands in cell 0, but only cell 1 is visible => culled out.
        // Mirrors `mesh_render::collect_excludes_mesh_in_nonvisible_cell`.
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(1.0, 2.0, 3.0),
            "smoke",
        );

        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![1]),
        );
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
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(-5.0, 0.0, 0.0)); // cell 1
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(5.0, 0.0, 0.0), // cell 0 - drifted into view
            "smoke",
            Some(emitter),
        );

        // Only cell 0 is visible (the emitter's cell 1 is hidden behind a wall).
        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
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
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0)); // cell 0
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 0.0, 0.0), // cell 1 - drifted out of view
            "smoke",
            Some(emitter),
        );

        // Only cell 0 (the emitter's own cell) is visible.
        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
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
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0)); // cell 0
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(5.0, 0.0, 0.0), // cell 0 - visible
            "smoke",
            Some(emitter),
        );
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 0.0, 0.0), // cell 1 - hidden
            "smoke",
            Some(emitter),
        );

        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
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
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(5.0, 0.0, 0.0));
        // Particle in the hidden back cell 1.
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 0.0, 0.0),
            "smoke",
            Some(emitter),
        );

        // Orphan it: its own cell (1) is not visible => culled, no special case.
        registry.despawn(emitter).unwrap();
        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
        assert!(
            collector.iter_collections().next().is_none(),
            "orphaned particle in a hidden cell is culled by its own cell"
        );

        // Make the orphan's own cell visible => it draws.
        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![1]),
        );
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1, "orphaned particle in a visible cell draws");
        assert_eq!(pairs[0].1.len(), SPRITE_INSTANCE_SIZE);
    }

    #[test]
    fn collect_culls_particle_with_no_emitter_backref_by_its_own_cell() {
        // A particle with `emitter == None` is culled by its own cell, same as
        // any other — there is no orphan/no-backref escape hatch anymore.
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        // Particle in cell 1, which is not visible => culled.
        particles.configure_impact_collection(collector.resolve_collection("smoke").unwrap());
        assert!(particles.spawn_impact(Vec3::new(-5.0, 0.0, 0.0), Vec3::ZERO, 0));
        assert!(particles.iter().next().unwrap().emitter.is_none());

        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
        );
        assert!(
            collector.iter_collections().next().is_none(),
            "no-backref particle in a hidden cell is culled by its own cell"
        );

        // Its own cell becomes visible => it draws.
        collector.collect(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![1]),
        );
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
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(-5.0, 0.0, 0.0));
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 0.0, 0.0),
            "smoke",
            Some(emitter),
        );
        spawn_particle_for(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 1.0, 0.0),
            "smoke",
            Some(emitter),
        );

        collector.collect(&registry, &particles, Some(&world), &VisibleCells::DrawAll);
        let pairs: Vec<(&str, &[u8])> = collector.iter_collections().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1.len(), 2 * SPRITE_INSTANCE_SIZE);
    }

    /// Retained release collector budget for 500 particles in an exact visible cell.
    #[test]
    #[ignore = "release-mode bench; run with `cargo test --release -- --ignored`"]
    fn bench_500_particles_collect_under_half_a_millisecond() {
        let mut registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = single_cell_world();
        let emitter = spawn_emitter(&mut registry, Vec3::new(1.0, 2.0, 3.0));
        for i in 0..500 {
            spawn_particle_for(
                &mut particles,
                &mut collector,
                Vec3::new(i as f32 * 0.01, 1.0, 0.0),
                "smoke",
                Some(emitter),
            );
        }
        // Warm the buffers' capacity so the timed run reuses it (matches
        // steady-state production where buffers persist across frames).
        let visible = VisibleCells::Culled(vec![0]);
        collector.collect(&registry, &particles, Some(&world), &visible);

        let start = std::time::Instant::now();
        collector.collect(&registry, &particles, Some(&world), &visible);
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
    #[test]
    fn collect_keeps_rotated_quad_overlapping_frustum_with_center_outside() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        let collection = collector.register_sprite("smoke");
        let parameters = postretro_sim::particle_pool::ParticleParameters {
            lifetime: 10.0,
            buoyancy: 0.0,
            drag: 0.0,
            size_curve: [2.0].into(),
            opacity_curve: [0.5].into(),
            collection,
            tint: [1.0; 3],
        };
        // The shader's size-2 square, rotated pi/4, reaches 1.414 from its
        // center. A half-side-only bound would incorrectly drop its left tip.
        assert!(particles.spawn_emitter(
            EntityId::from_raw(1),
            &parameters,
            Vec3::new(1.2, 0.0, 0.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_4
        ));
        assert!(particles.spawn_emitter(
            EntityId::from_raw(2),
            &parameters,
            Vec3::new(1.6, 0.0, 0.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_4
        ));
        particles.step(&registry, 1.0, |_| (0.0, Vec3::ZERO));
        let frustum = Frustum {
            planes: vec![postretro_visibility::FrustumPlane {
                normal: -Vec3::X,
                dist: 0.0,
            }],
        };
        let world = single_cell_world();
        for visible in [VisibleCells::Culled(vec![0]), VisibleCells::DrawAll] {
            collector.collect_at_tick(
                &registry,
                &particles,
                Some(&world),
                &visible,
                0.0,
                Some(&frustum),
            );
            let bytes = collector
                .iter_collections()
                .next()
                .expect("overlapping quad draws")
                .1;
            assert_eq!(bytes.len(), SPRITE_INSTANCE_SIZE);
            let position_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
            let size = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
            let rotation = f32::from_ne_bytes(bytes[20..24].try_into().unwrap());
            assert!((position_x - 1.2).abs() < 1e-6);
            assert!((size - 2.0).abs() < 1e-6);
            assert!((rotation - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        }
    }

    #[test]
    fn collect_rejects_offscreen_quad_before_cell_locator_descent() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        spawn_particle(
            &mut particles,
            &mut collector,
            Vec3::new(-5.0, 0.0, 0.0),
            "smoke",
        );
        let frustum = Frustum {
            planes: vec![postretro_visibility::FrustumPlane {
                normal: Vec3::X,
                dist: 0.0,
            }],
        };
        let mut world = single_cell_world();
        // A deliberately invalid locator makes any accidental descent fail.
        world.cell_locator_root = postretro_level_loader::CellLocatorChild::Node(usize::MAX);
        collector.collect_at_tick(
            &registry,
            &particles,
            Some(&world),
            &VisibleCells::Culled(vec![0]),
            0.0,
            Some(&frustum),
        );
        assert!(collector.iter_collections().next().is_none());
    }

    #[test]
    fn collection_indices_keep_batching_and_reset_level_registration() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        let smoke = collector.register_sprite("smoke");
        let spark = collector.register_sprite("spark");
        assert_ne!(smoke, spark);
        assert_eq!(collector.register_sprite("smoke"), smoke);
        assert_eq!(collector.resolve_collection("spark"), Some(spark));
        spawn_particle(&mut particles, &mut collector, Vec3::ZERO, "smoke");
        collector.collect(&registry, &particles, None, &VisibleCells::Culled(vec![0]));
        let capacity = collector.buffers[smoke].bytes.capacity();
        let capture = LogCapture::start();
        assert_eq!(collector.resolve_collection("unknown"), Some(smoke));
        assert_eq!(collector.resolve_collection("unknown"), Some(smoke));
        capture.assert_logged_once(Level::Warn, "collection id 'unknown' was not registered");
        particles.clear();
        collector.reset_for_level();
        assert!(collector.iter_collections().next().is_none());
        assert_eq!(collector.resolve_collection("smoke"), None);
        assert_eq!(collector.register_sprite("smoke"), smoke);
        assert_eq!(collector.buffers[smoke].bytes.capacity(), capacity);
        assert_eq!(collector.resolve_collection("unknown"), Some(smoke));
        assert_eq!(
            capture
                .records()
                .iter()
                .filter(|record| record.level == Level::Warn)
                .count(),
            2
        );
    }

    #[test]
    fn collect_ten_thousand_particles_uses_resolved_indices_and_retained_scratch() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        collector.register_sprite("spark");
        let world = two_cell_world();
        let visible = VisibleCells::Culled(vec![0]);
        for index in 0..10_000 {
            let x = if index % 2 == 0 { 1.0 } else { -1.0 };
            let sprite = if index % 4 == 0 { "spark" } else { "smoke" };
            spawn_particle(
                &mut particles,
                &mut collector,
                Vec3::new(x, 0.0, 0.0),
                sprite,
            );
        }
        collector.collect(&registry, &particles, Some(&world), &visible);
        let capacities: Vec<_> = collector
            .buffers
            .iter()
            .map(|buffer| buffer.bytes.capacity())
            .collect();
        let visible_capacity = collector.visible_cells.capacity();
        collector.collect(&registry, &particles, Some(&world), &visible);
        let bytes: usize = collector
            .iter_collections()
            .map(|(_, bytes)| bytes.len())
            .sum();
        assert_eq!(bytes, 5_000 * SPRITE_INSTANCE_SIZE);
        assert_eq!(collector.iter_collections().count(), 2);
        assert_eq!(
            collector
                .buffers
                .iter()
                .map(|buffer| buffer.bytes.capacity())
                .collect::<Vec<_>>(),
            capacities
        );
        assert_eq!(collector.visible_cells.capacity(), visible_capacity);
        assert_eq!(registry.iter_with_kind(ComponentKind::Transform).count(), 0);
    }

    /// CPU-only scaling aid; the brief's Intel Mac stress run remains the budget gate.
    #[test]
    #[ignore = "release-mode particle collector scale profiling"]
    fn profile_particle_collect_scale_with_exact_cell_culling() {
        let registry = EntityRegistry::new();
        let mut particles = ParticlePool::new();
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite("smoke");
        let world = two_cell_world();
        let visible = VisibleCells::Culled(vec![0]);
        for live in [0, 1_000, 5_000, 10_000, 20_000] {
            particles.clear();
            for index in 0..live {
                spawn_particle(
                    &mut particles,
                    &mut collector,
                    Vec3::new(if index % 2 == 0 { 1.0 } else { -1.0 }, 0.0, 0.0),
                    "smoke",
                );
            }
            collector.collect(&registry, &particles, Some(&world), &visible);
            let start = std::time::Instant::now();
            for _ in 0..120 {
                collector.collect(&registry, &particles, Some(&world), &visible);
                std::hint::black_box(
                    collector
                        .iter_collections()
                        .map(|(_, bytes)| bytes.len())
                        .sum::<usize>(),
                );
            }
            eprintln!(
                "particle_collect live={live} average={:?}",
                start.elapsed() / 120
            );
            assert_eq!(
                collector
                    .iter_collections()
                    .map(|(_, bytes)| bytes.len())
                    .sum::<usize>(),
                live / 2 * SPRITE_INSTANCE_SIZE
            );
        }
    }
}
