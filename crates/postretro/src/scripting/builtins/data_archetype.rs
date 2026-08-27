// Data-archetype spawn path: walks `world.map_entities` against the
// `DataRegistry.entities` table populated at mod-manifest ingestion time,
// materializing placeable descriptors into ECS entities with their component
// presets attached. Weapon-only descriptors are equip targets, not map
// placements.
//
// See: context/lib/build_pipeline.md §Built-in Classname Routing
//      context/lib/scripting.md §2 (data context lifecycle)
//
// The built-in classname-dispatch sweep runs first; this pass receives the
// set of classnames that were already handled and skips them. A classname
// that appears in BOTH dispatch tables logs a `warn!` once per classname and
// keeps the built-in result (built-in wins).

use std::collections::{BTreeSet, HashSet};

use glam::Vec3;

use super::MapEntity;
use crate::scripting_systems::ai::{ENEMY_DEFAULT_FACTION, FACTION_STATE_FIELD};
#[cfg(test)]
use postretro_entities::AmmoReserve;
use postretro_entities::components::agent::attach_agent;
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::components::brain::{attach_brain_graph, validate_brain_animation_states};
use postretro_entities::components::health::HealthComponent;
#[cfg(test)]
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_entities::components::mesh::{
    MeshAnimation, MeshComponent, capsule_center_to_feet_origin_offset,
};
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::touchable::TouchableComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorMapOverride, DescriptorProvenance, DescriptorSpawnPath,
    parse_bool,
};
use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::{NavAgentParams, ProjectileBodyVisual};
#[cfg(test)]
use postretro_scripting_core::data_descriptors::WeaponResource;
use postretro_scripting_core::data_descriptors::{EntityTypeDescriptor, LightDescriptor};

pub(super) use super::wieldable_inventory::compose_wieldable_inventory;
pub(crate) use super::wieldable_inventory::compose_wieldable_inventory_from_slots;

/// Capsule fallback for a descriptor-spawned agent when the map has no navmesh
/// (`agent_params == None`). The agent still materializes — it simply cannot
/// path until a navmesh is present. Values mirror the canonical human-ish agent
/// the navmesh bake targets (`NavAgentParams` defaults), so a fallback capsule
/// is plausibly sized even with no bake to read from.
pub(crate) const DEFAULT_AGENT_PARAMS: NavAgentParams = NavAgentParams {
    radius: 0.35,
    height: 1.8,
    step_height: 0.4,
    max_slope_deg: 45.0,
};

/// Apply the `initial_<field>` KVP override convention to the descriptor's
/// component presets. Each scalar field (`f32`, `u32`) parses via `FromStr`;
/// `[f32; 3]` parses as three space-delimited floats. Parse failures
/// `warn!` with the diagnostic origin and offending key/value pair, leaving
/// the descriptor default in place.
///
/// Returns the set of overrides that actually landed (parse succeeded and
/// the field was written). The caller accumulates these into
/// `DescriptorProvenance.map_overrides` so the hot-reload refresh planner
/// knows which overrides to reapply when a descriptor is refreshed at runtime.
/// Only successful overrides are included — a bad parse leaves neither the
/// field nor a provenance entry. Uses `BTreeSet` to match
/// `DescriptorProvenance.map_overrides` (deterministic order for serde and
/// test equality).
fn apply_emitter_kvp_overrides(
    component: &mut BillboardEmitterComponent,
    entity: &MapEntity,
) -> BTreeSet<DescriptorMapOverride> {
    let mut applied = BTreeSet::new();
    for (key, raw) in entity.key_values.iter() {
        let Some(field) = key.strip_prefix("initial_") else {
            continue;
        };
        match field {
            "rate" if parse_into_f32(raw, &mut component.rate, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialRate);
            }
            "spread" if parse_into_f32(raw, &mut component.spread, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialSpread);
            }
            "lifetime" if parse_into_f32(raw, &mut component.lifetime, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialLifetime);
            }
            "buoyancy" if parse_into_f32(raw, &mut component.buoyancy, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialBuoyancy);
            }
            "drag" if parse_into_f32(raw, &mut component.drag, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialDrag);
            }
            "spin_rate" if parse_into_f32(raw, &mut component.spin_rate, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialSpinRate);
            }
            "burst" => match raw.trim().parse::<u32>() {
                Ok(v) => {
                    component.burst = Some(v);
                    applied.insert(DescriptorMapOverride::EmitterInitialBurst);
                }
                Err(_) => warn_parse(entity, key, raw),
            },
            "sprite" => {
                if raw.is_empty() {
                    warn_parse(entity, key, raw);
                } else {
                    component.sprite = raw.clone();
                    applied.insert(DescriptorMapOverride::EmitterInitialSprite);
                }
            }
            "color" if parse_into_vec3(raw, &mut component.color, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialColor);
            }
            "velocity" if parse_into_vec3(raw, &mut component.velocity, entity, key) => {
                applied.insert(DescriptorMapOverride::EmitterInitialVelocity);
            }
            _ => {}
        }
    }
    applied
}

/// Apply the `initial_<field>` KVP override convention to a `LightDescriptor`.
/// Mirrors `apply_emitter_kvp_overrides`: parse failures `warn!` and leave the
/// descriptor default in place.
///
/// Returns the set of overrides that actually landed. The caller accumulates
/// these into `DescriptorProvenance.map_overrides` so the hot-reload refresh
/// planner can reapply them when the descriptor is refreshed at runtime.
fn apply_light_kvp_overrides(
    descriptor: &mut LightDescriptor,
    entity: &MapEntity,
) -> BTreeSet<DescriptorMapOverride> {
    let mut applied = BTreeSet::new();
    for (key, raw) in entity.key_values.iter() {
        let Some(field) = key.strip_prefix("initial_") else {
            continue;
        };
        match field {
            // Mirror the validation applied at descriptor parse time: reject
            // negative or non-finite values at parse time so a bad override
            // never lands on the descriptor (e.g. `initial_intensity -5.0`).
            "intensity" if parse_into_nonneg_f32(raw, &mut descriptor.intensity, entity, key) => {
                applied.insert(DescriptorMapOverride::LightInitialIntensity);
            }
            "range" if parse_into_nonneg_f32(raw, &mut descriptor.range, entity, key) => {
                applied.insert(DescriptorMapOverride::LightInitialRange);
            }
            "is_dynamic" => match parse_bool(raw) {
                Some(v) => {
                    descriptor.is_dynamic = v;
                    applied.insert(DescriptorMapOverride::LightInitialIsDynamic);
                }
                None => warn_parse(entity, key, raw),
            },
            "color" if parse_into_vec3(raw, &mut descriptor.color, entity, key) => {
                applied.insert(DescriptorMapOverride::LightInitialColor);
            }
            _ => {}
        }
    }
    applied
}

fn parse_into_f32(raw: &str, slot: &mut f32, entity: &MapEntity, key: &str) -> bool {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => {
            *slot = v;
            true
        }
        _ => {
            warn_parse(entity, key, raw);
            false
        }
    }
}

/// Like `parse_into_f32` but additionally rejects negative values, mirroring
/// `LightDescriptor::validate()`. Bad values warn and leave the descriptor
/// default in place — the `slot` is only written on success.
fn parse_into_nonneg_f32(raw: &str, slot: &mut f32, entity: &MapEntity, key: &str) -> bool {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() && v >= 0.0 => {
            *slot = v;
            true
        }
        _ => {
            warn_parse(entity, key, raw);
            false
        }
    }
}

fn parse_into_vec3(raw: &str, slot: &mut [f32; 3], entity: &MapEntity, key: &str) -> bool {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() != 3 {
        warn_parse(entity, key, raw);
        return false;
    }
    let mut out = [0.0f32; 3];
    for (i, part) in parts.iter().enumerate() {
        match part.parse::<f32>() {
            Ok(v) if v.is_finite() => out[i] = v,
            _ => {
                warn_parse(entity, key, raw);
                return false;
            }
        }
    }
    *slot = out;
    true
}

fn warn_parse(entity: &MapEntity, key: &str, raw: &str) {
    log::warn!(
        "[Loader] {origin}: key `{key}` has invalid value `{raw}`; using descriptor default",
        origin = entity.diagnostic_origin(),
    );
}

/// Find an `EntityTypeDescriptor` whose `canonical_name` equals `classname`.
/// Linear scan — descriptor lists are small (one entry per registered class)
/// so a HashMap would be premature. Descriptors with `canonical_name = None`
/// have no map-placement form and are always skipped here.
///
/// `entity_spawner` (a `player_spawn`-mirroring, archetype-referencing spawn
/// marker) resolves through this lookup at install time. Roadmap: composable
/// archetypes and FGD generated from the registry still sit on this lookup;
/// the abstraction grows from this single match site.
pub(crate) fn find_descriptor<'a>(
    descriptors: &'a [EntityTypeDescriptor],
    classname: &str,
) -> Option<&'a EntityTypeDescriptor> {
    descriptors
        .iter()
        .find(|d| d.canonical_name.as_deref() == Some(classname))
}

fn is_directly_map_placeable(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.light.is_some()
        || descriptor.emitter.is_some()
        || descriptor.movement.is_some()
        || descriptor.mesh.is_some()
        || descriptor.health.is_some()
        || descriptor.touchable.is_some()
}

pub(crate) fn ai_capsule_center_from_feet_offset(
    descriptor: &EntityTypeDescriptor,
    agent_params: Option<NavAgentParams>,
) -> Vec3 {
    if !descriptor_carries_brain(descriptor) {
        return Vec3::ZERO;
    }
    let params = agent_params.unwrap_or(DEFAULT_AGENT_PARAMS);
    -capsule_center_to_feet_origin_offset(params.radius, params.height)
}

/// Whether this descriptor authors a behavior-graph brain.
fn descriptor_carries_brain(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.behavior.is_some()
}

/// Whether materializing this descriptor would attach the engine-owned AI pair
/// (`ComponentKind::Brain` + `ComponentKind::Agent`) — i.e. whether the
/// descriptor carries a brain block. This is the *pre-materialization* mirror of
/// the live-component predicate
/// `crate::netcode::descriptor_class::is_networked_ai_enemy`,
/// which can only inspect those components AFTER an entity exists: a brain block
/// is the sole thing `attach_descriptor_components` keys the `Brain` + `Agent`
/// attachment on, so [`descriptor_carries_brain`] holds exactly when that
/// predicate would later return `true` for an eligible descriptor spawn of this
/// descriptor.
///
/// Used by the connected-client install path to drop AI-enemy map
/// placements *before* dispatch, since those enemies must arrive only as
/// host-authoritative snapshots — never as locally-spawned authoritative copies.
/// Keying on the brain block (not classname strings, not
/// `DescriptorProvenance.owned_components`, which never tracks AI) keeps the same
/// single definition of "AI map enemy" the live predicate enforces.
pub(crate) fn descriptor_materializes_ai_enemy(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor_carries_brain(descriptor)
}

/// Whether this descriptor materializes a host-authoritative world item. A world
/// item is defined solely by its `touchable` block; the host derives outbound
/// replication membership from the corresponding live component.
pub(crate) fn descriptor_materializes_world_item(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.touchable.is_some()
}

/// Partition map placements for a **connected client** install:
/// returns only the placements that should still materialize locally, dropping
/// any whose matched descriptor would materialize a host-authoritative AI enemy
/// ([`descriptor_materializes_ai_enemy`]) or world item
/// ([`descriptor_materializes_world_item`]). Those entities reach the client solely
/// via host snapshots; spawning a local copy here would duplicate their
/// host-authoritative state.
///
/// Placements whose classname has no descriptor match are retained untouched so the
/// downstream dispatch handles their unknown-classname / built-in-collision diagnostics
/// exactly as it would on a host. Single-player and listen-host installs never call this
/// (they keep every placement); only the connected-client lifecycle path filters.
pub(crate) fn filter_out_client_host_replicated_placements(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<MapEntity> {
    entities
        .iter()
        .filter(
            |entity| match find_descriptor(descriptors, &entity.classname) {
                Some(descriptor) => {
                    !descriptor_materializes_ai_enemy(descriptor)
                        && !descriptor_materializes_world_item(descriptor)
                }
                // No descriptor match: retain for the normal unknown-classname
                // diagnostics in dispatch.
                None => true,
            },
        )
        .cloned()
        .collect()
}

/// Collect the distinct, non-empty mesh model handles referenced by the
/// host-authoritative map placements a connected client suppresses
/// ([`filter_out_client_host_replicated_placements`]), preserving first-seen order. GPU-free:
/// this is the pure analogue of [`crate::distinct_mesh_models`] for placements
/// that never spawn a local `MeshComponent` on a connected client, so the
/// registry-driven sweep cannot see them.
///
/// Scoped to the classes the **map actually references** (the placements passed
/// in), not every descriptor in the data registry — only host-replicated map
/// entities in this level need their model on the GPU. A placement is included only
/// when its matched descriptor materializes either an AI enemy or world item and
/// carries a `mesh` block with a non-empty `model`; ordinary placements and
/// meshless descriptors contribute nothing.
///
/// Regression (E10 AC #3): a connected client filtered out the AI-enemy
/// placement before dispatch, so its model was never in the registry-driven
/// upload set; when the host snapshot later materialized the remote enemy the
/// draw planner dropped it (no uploaded mesh in the model cache) and the real
/// model never rendered — only a dev-tools debug capsule showed. The level-load
/// sweep unions these handles with [`crate::distinct_mesh_models`] so the
/// suppressed entity's model is uploaded up front.
///
/// Each returned string is the VERBATIM renderer cache key (the descriptor's
/// holder `mesh.model` or attachment model), identical in shape to
/// [`crate::distinct_mesh_models`] output, so the caller can dedup the two sets
/// and upload each handle once.
pub(crate) fn suppressed_client_host_replicated_mesh_models(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for entity in entities {
        let Some(descriptor) = find_descriptor(descriptors, &entity.classname) else {
            continue;
        };
        if !descriptor_materializes_ai_enemy(descriptor)
            && !descriptor_materializes_world_item(descriptor)
        {
            continue;
        }
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        if !mesh.model.is_empty() && seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
        }
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            if !attachment_model.is_empty() && seen.insert(attachment_model.to_string()) {
                ordered.push(attachment_model.to_string());
            }
        }
    }
    ordered
}

/// Collect every movement descriptor's holder and attachment models. A listen
/// host may select any such descriptor for an accepted net-slot pawn, while a
/// connected client receives those pawns through snapshots. Neither path may
/// upload models during gameplay, so install preloads the full descriptor set.
pub(crate) fn movement_descriptor_mesh_models(descriptors: &[EntityTypeDescriptor]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for descriptor in descriptors {
        if descriptor.movement.is_none() {
            continue;
        }
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        if !mesh.model.is_empty() && seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
        }
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            if !attachment_model.is_empty() && seen.insert(attachment_model.to_string()) {
                ordered.push(attachment_model.to_string());
            }
        }
    }
    ordered
}

/// Collect third- and first-person models declared by weapon descriptors.
/// Wieldable instances intentionally have no `MeshComponent`, so a registry-driven
/// sweep cannot discover either presentation asset. Every role may present an
/// active weapon, therefore this list is always unioned into the level-load model
/// sweep rather than being client-only.
pub(crate) fn weapon_presentation_models(descriptors: &[EntityTypeDescriptor]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for descriptor in descriptors {
        let Some(weapon) = descriptor.weapon.as_ref() else {
            continue;
        };
        for model in [
            weapon.third_person_model.as_deref(),
            weapon.viewmodel.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|model| !model.is_empty())
        {
            if seen.insert(model.to_string()) {
                ordered.push(model.to_string());
            }
        }
    }
    ordered
}

/// One projectile sprite collection that must be uploaded before a weapon can
/// materialize its flight entity. The renderer owns the upload; this is only
/// descriptor-driven level-install discovery.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectileSpriteCollection {
    pub(crate) collection: String,
    /// Required loop period when this consumer advances billboard age. Static
    /// projectile bodies leave it unconstrained because they always pack age zero.
    pub(crate) lifetime: Option<f32>,
    pub(crate) emissive: f32,
    /// A projectile body cadence is translated to the collection's loop period
    /// only after the actual frame count is known at level install.
    pub(crate) frame_duration_ms: Option<f32>,
    /// Stable author-facing origin used when collection draw contracts conflict.
    pub(crate) source: String,
}

/// Collect projectile body and trail presentation assets from the full weapon
/// descriptor table. Projectile entities are spawned during gameplay, after
/// the ordinary registry-based level sweep has completed, so their resources
/// must be enrolled from descriptor data up front.
pub(crate) fn projectile_presentation_assets(
    descriptors: &[EntityTypeDescriptor],
) -> (Vec<String>, Vec<ProjectileSpriteCollection>) {
    let mut seen_models = HashSet::new();
    let mut models = Vec::new();
    let mut sprites = Vec::new();

    for (descriptor_index, descriptor) in descriptors.iter().enumerate() {
        let Some(projectile) = descriptor
            .weapon
            .as_ref()
            .and_then(|weapon| weapon.projectile.as_ref())
        else {
            continue;
        };

        let descriptor_name = descriptor
            .canonical_name
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("descriptor[{descriptor_index}]"));

        if let Some(trail) = projectile.visual.trail.as_ref()
            && !trail.sprite.is_empty()
        {
            sprites.push(ProjectileSpriteCollection {
                collection: trail.sprite.clone(),
                lifetime: Some(trail.lifetime),
                emissive: 0.0,
                frame_duration_ms: None,
                source: format!("{descriptor_name}.projectile.visual.trail"),
            });
        }

        match &projectile.visual.body {
            ProjectileBodyVisual::Sprite {
                sprite,
                emissive,
                frame_duration_ms,
                ..
            } if !sprite.is_empty() => {
                sprites.push(ProjectileSpriteCollection {
                    collection: sprite.clone(),
                    // No-cadence bodies pack age 0.0, so another valid consumer
                    // may choose the collection loop period without changing them.
                    lifetime: None,
                    emissive: *emissive,
                    frame_duration_ms: *frame_duration_ms,
                    source: format!("{descriptor_name}.projectile.visual.body"),
                });
            }
            ProjectileBodyVisual::Model { model }
                if !model.is_empty() && seen_models.insert(model.clone()) =>
            {
                models.push(model.clone());
            }
            _ => {}
        }
    }

    (models, sprites)
}

/// Collect world-mesh models for wieldables that can later leave an inventory.
/// Inventory composition strips their `MeshComponent`, so a registry-driven
/// install sweep cannot discover a descriptor referenced only by a loadout.
/// Requiring both weapon and touchable authoring keeps this preload scoped to
/// instances the drop path can actually restore as world items.
pub(crate) fn touchable_wieldable_world_models(
    descriptors: &[EntityTypeDescriptor],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for descriptor in descriptors {
        if descriptor.weapon.is_none() || descriptor.touchable.is_none() {
            continue;
        }
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        if !mesh.model.is_empty() && seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
        }
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            if !attachment_model.is_empty() && seen.insert(attachment_model.to_string()) {
                ordered.push(attachment_model.to_string());
            }
        }
    }
    ordered
}

/// Attach descriptor components to an already-spawned entity. `initial_*` KVP
/// overrides are applied to `emitter` and `light` before attachment;
/// `movement` receives descriptor values verbatim. Weapon attachment is opt-in
/// because direct map placements may be otherwise-placeable without becoming
/// wieldable instances.
/// The light is always forced dynamic regardless of the descriptor's
/// `is_dynamic` field (baked indirect lighting is not supported for
/// descriptor-spawned lights), with a `warn!` if the descriptor had it set to
/// `false`.
pub(crate) fn attach_descriptor_components(
    registry: &mut EntityRegistry,
    id: EntityId,
    descriptor: &EntityTypeDescriptor,
    entity: &MapEntity,
    attach_weapon: bool,
    spawn_path: DescriptorSpawnPath,
    agent_params: Option<NavAgentParams>,
) {
    let mut owned_components = BTreeSet::new();
    let mut map_overrides = BTreeSet::new();

    if let Some(emitter) = descriptor.emitter.clone() {
        let mut component = emitter;
        map_overrides.extend(apply_emitter_kvp_overrides(&mut component, entity));
        // `set_component` only fails on a stale id — the id was just returned.
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Emitter);
    }

    if let Some(light_desc) = descriptor.light.clone() {
        let mut light_desc = light_desc;
        map_overrides.extend(apply_light_kvp_overrides(&mut light_desc, entity));

        if !light_desc.is_dynamic {
            log::warn!(
                "[Loader] {origin}: descriptor-spawned light `{cls}` was authored \
                 `is_dynamic = false`; forcing dynamic (baked indirect not supported \
                 for descriptor-spawned lights)",
                origin = entity.diagnostic_origin(),
                cls = entity.classname,
            );
        }

        let component = LightComponent {
            origin: [entity.origin.x, entity.origin.y, entity.origin.z],
            light_type: LightKind::Point,
            intensity: light_desc.intensity,
            color: light_desc.color,
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: light_desc.range,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            is_dynamic: true,
            animated_slot: None,
            follow_transform: false,
            carrier: None,
            animation: None,
        };
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Light);
    }

    if let Some(movement_desc) = descriptor.movement.as_ref() {
        let component = PlayerMovementComponent::from_descriptor(movement_desc);
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Movement);
    }

    if let Some(component) = descriptor_mesh_component(descriptor, agent_params) {
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Mesh);
    }

    if let Some(health_desc) = descriptor.health.as_ref() {
        let mut component = HealthComponent::from_descriptor(health_desc);
        let origin_shift = ai_capsule_center_from_feet_offset(descriptor, agent_params);
        if let Some(hitbox) = component.hitbox.as_mut() {
            hitbox.offset -= origin_shift;
        }
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Health);
    }

    if let Some(touchable_desc) = descriptor.touchable.as_ref() {
        let _ = registry.set_component(id, TouchableComponent::from_descriptor(touchable_desc));
        owned_components.insert(DescriptorComponentKind::Touchable);
    }

    // A behavior graph materializes the engine-owned brain AND a movable
    // navigation agent (the tick drives the agent each tick).
    //
    // The agent's capsule is seeded from the navmesh's baked `NavAgentParams`
    // (passed down from the attach call site — never read inside the component).
    // When the map has no navmesh (`agent_params == None`), the capsule falls
    // back to an engine default and the agent simply cannot path. Move speed
    // comes from the graph. After both components land, the brain's state →
    // animation-state map is validated against the entity's mesh
    // (cross-component: neither block could see the mesh at its own parse).
    let brain_move_speed = if let Some(behavior) = descriptor.behavior.as_ref() {
        let _ = attach_brain_graph(registry, id, behavior);
        Some(behavior.move_speed)
    } else {
        None
    };
    if let Some(move_speed) = brain_move_speed {
        let aggro_armed = ai_aggro_armed_on_spawn(entity);
        let home_anchor = registry
            .get_component::<Transform>(id)
            .expect("newly spawned descriptor entity carries a Transform")
            .position;
        if let Ok(mut brain) = registry
            .get_component::<postretro_entities::components::brain::BrainComponent>(id)
            .cloned()
        {
            brain.aggro_armed = aggro_armed;
            brain.home_anchor = home_anchor;
            let _ = registry.set_component(id, brain);
        }
        registry
            .entity_state_mut(id)
            .expect("newly spawned descriptor entity carries entity state")
            .set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION);

        let params = agent_params.unwrap_or(DEFAULT_AGENT_PARAMS);
        let _ = attach_agent(registry, id, &params, move_speed);

        // Warn-once per undeclared animation-state name; the tick keeps the prior
        // animation for those states. Called here for its spawn-time side
        // effects — the return value (unmapped state names) is not consumed; the
        // tick handles `UnknownState` at tick time. It also reconciles the
        // graph's rest animation with the mesh's `defaultState`, which is why it
        // runs AFTER `descriptor_mesh_component` has attached the mesh.
        let _ = validate_brain_animation_states(registry, id);
    }

    if attach_weapon {
        if let Some(weapon_desc) = descriptor.weapon.as_ref() {
            let component = WeaponComponent::from_descriptor_with_canonical(
                weapon_desc,
                descriptor.canonical_name.as_deref(),
            );
            let _ = registry.set_component(id, component);
            owned_components.insert(DescriptorComponentKind::Weapon);
        }
    }

    if let Some(canonical_name) = descriptor.canonical_name.clone() {
        let provenance = DescriptorProvenance {
            canonical_name,
            owned_components,
            map_overrides,
            spawn_path,
        };
        let _ = registry.set_component(id, provenance);
    }
}

/// Materialize the descriptor-owned mesh presentation shared by local spawns
/// and connected-client remote enemies. Descriptor state maps already carry
/// validated `travelSpeed` overrides; this seam also applies the shared
/// absent-`speedScale` default and capsule-center-to-feet render-origin offset
/// for AI and movement pawns.
pub(crate) fn descriptor_mesh_component(
    descriptor: &EntityTypeDescriptor,
    agent_params: Option<NavAgentParams>,
) -> Option<MeshComponent> {
    let mesh_desc = descriptor.mesh.as_ref()?;
    let origin_offset = if let Some(movement) = descriptor.movement.as_ref() {
        // Player transforms are their collision-capsule centers, while player
        // meshes are authored at their feet. `half_height` names the distance
        // from the center to each spherical-cap center, so the total capsule
        // height is twice `(half_height + radius)`.
        let capsule = &movement.capsule;
        capsule_center_to_feet_origin_offset(
            capsule.radius,
            2.0 * (capsule.half_height + capsule.radius),
        )
    } else if descriptor_carries_brain(descriptor) {
        let params = agent_params.unwrap_or(DEFAULT_AGENT_PARAMS);
        capsule_center_to_feet_origin_offset(params.radius, params.height)
    } else {
        Vec3::ZERO
    };
    let component = match &mesh_desc.default_state {
        Some(default_state) => MeshComponent::animated(
            mesh_desc.model.clone(),
            MeshAnimation::new(mesh_desc.animations.clone(), default_state.clone())
                .with_speed_scale(mesh_desc.speed_scale()),
        ),
        None => MeshComponent::stateless(mesh_desc.model.clone()),
    };
    // Descriptor maps are unordered. Give the component a stable attachment
    // order so later presentation collection is deterministic across runs.
    let mut attachments: Vec<(String, String)> = mesh_desc
        .attachments
        .iter()
        .map(|(socket, model)| (socket.clone(), model.clone()))
        .collect();
    attachments.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    Some(
        component
            .with_attachments(attachments)
            .with_origin_offset(origin_offset)
            .with_shadow_bias_scale(mesh_desc.shadow_bias_scale)
            .with_shadow_only(mesh_desc.shadow_only),
    )
}

/// Seed an AI brain's aggro gate from a map placement.
///
/// This deliberately consumes the bare `enabled_on_spawn` KVP instead of the
/// usual `initial_*` descriptor-override namespace: it is a set-piece gate,
/// not descriptor-owned gameplay tuning. The descriptor refresh planner does
/// not replace `BrainComponent`, so a hot descriptor reload preserves this
/// placement-seeded host state rather than reopening a sealed enemy.
fn ai_aggro_armed_on_spawn(entity: &MapEntity) -> bool {
    let Some(raw) = entity.key_values.get("enabled_on_spawn") else {
        log::warn!(
            "[Loader] {}: AI enemy has no `enabled_on_spawn`; defaulting aggro gate open",
            entity.diagnostic_origin(),
        );
        return true;
    };
    match parse_bool(raw) {
        Some(value) => value,
        None => {
            log::warn!(
                "[Loader] {}: AI enemy `enabled_on_spawn` value `{raw}` is not boolean; \
                 defaulting aggro gate open",
                entity.diagnostic_origin(),
            );
            true
        }
    }
}

pub(super) fn spawn_descriptor_instance(
    registry: &mut EntityRegistry,
    descriptor: &EntityTypeDescriptor,
    entity: &MapEntity,
    attach_weapon: bool,
    spawn_path: DescriptorSpawnPath,
    agent_params: Option<NavAgentParams>,
) -> Option<EntityId> {
    let origin_shift = ai_capsule_center_from_feet_offset(descriptor, agent_params);
    let transform = Transform {
        position: entity.origin + origin_shift,
        rotation: entity.rotation_quat(),
        scale: Vec3::ONE,
    };

    let id = registry.try_spawn(transform, &entity.tags)?;

    attach_descriptor_components(
        registry,
        id,
        descriptor,
        entity,
        attach_weapon,
        spawn_path,
        agent_params,
    );
    Some(id)
}

/// Spawn descriptor-driven entities for every `MapEntity` whose classname
/// matches a registered descriptor AND was not already handled by the
/// built-in dispatch.
///
/// # Returns
///
/// The set of classnames for which at least one descriptor-spawned entity was
/// successfully materialized (i.e. `registry.try_spawn` returned `Some`).
/// Classnames that were skipped because they appear in `handled_by_builtin`
/// are excluded — even if no entity actually landed via the built-in path
/// (registry exhausted), the classname is still considered owned by built-in
/// dispatch and is not included here. This means callers must not union this
/// set with the built-in set to derive "all claimed classnames": the built-in
/// set already covers the collision cases. Contrast with
/// [`super::apply_classname_dispatch`], which includes every classname a
/// handler was attempted for, independent of spawn success.
///
/// Placements whose classname is not found in either the descriptor list or
/// `handled_by_builtin`, and is not in the structural exclusion set
/// (`worldspawn`, `player_spawn`), log a `warn!` once per distinct classname
/// per sweep.
pub(crate) fn apply_data_archetype_dispatch(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    handled_by_builtin: &HashSet<String>,
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> HashSet<String> {
    // Warn-once tracking for descriptor/built-in collisions and for placements
    // referencing a classname that has no descriptor match.
    // Scoped per sweep; current callers run exactly once per level load.
    let mut collision_warned: HashSet<String> = HashSet::new();
    let mut unknown_warned: HashSet<String> = HashSet::new();
    let mut handled: HashSet<String> = HashSet::new();

    for entity in entities {
        let Some(descriptor) = find_descriptor(descriptors, &entity.classname) else {
            // No descriptor match. The built-in dispatch already handled this
            // classname (or attempted to) when it appears in `handled_by_builtin`;
            // otherwise the classname is either an unmodeled placement or a
            // structural marker (worldspawn, player_spawn) routed elsewhere.
            // Warn once per distinct unknown classname per sweep.
            let cls = entity.classname.as_str();
            let is_structural = cls == "worldspawn" || cls == PLAYER_START_CLASSNAME;
            if !handled_by_builtin.contains(cls)
                && !is_structural
                && unknown_warned.insert(cls.to_string())
            {
                log::warn!(
                    "[Loader] {origin}: classname `{cls}` has no registered descriptor; placement dropped",
                    origin = entity.diagnostic_origin(),
                );
            }
            continue;
        };

        if handled_by_builtin.contains(&entity.classname) {
            if collision_warned.insert(entity.classname.clone()) {
                log::warn!(
                    "[Loader] {origin}: classname `{}` is registered both as a built-in handler \
                     and a data-script entity descriptor; built-in handler wins",
                    entity.classname,
                    origin = entity.diagnostic_origin(),
                );
            }
            // Intentionally skips descriptor spawn even if the built-in
            // returned None (registry exhausted) — built-in attempted is
            // treated as built-in handled.
            continue;
        }

        if !is_directly_map_placeable(descriptor) {
            continue;
        }

        let Some(id) = spawn_descriptor_instance(
            registry,
            descriptor,
            entity,
            descriptor.touchable.is_some(),
            DescriptorSpawnPath::MapPlacement,
            agent_params,
        ) else {
            log::warn!(
                "[Loader] {origin}: entity registry exhausted; dropping descriptor-spawned `{cls}`",
                origin = entity.diagnostic_origin(),
                cls = entity.classname,
            );
            continue;
        };

        // Mirror the per-placement KVP bag so `getEntityProperty` works
        // uniformly across spawn paths. Always write — even an empty bag —
        // to honor the invariant that every map-spawned entity has a
        // `kvp_table` entry (matches the built-in dispatch path).
        let _ = registry.set_map_kvps(id, entity.key_values.clone());

        handled.insert(entity.classname.clone());
    }

    handled
}

/// Classname for the FGD `player_spawn` point entity. Spawn points are
/// extracted from `world.map_entities` before the built-in / data-archetype
/// dispatch sweeps and processed by [`spawn_from_player_starts`].
pub(crate) const PLAYER_START_CLASSNAME: &str = "player_spawn";

/// Spawn one entity per `player_spawn` placement, using each placement's
/// `entity_class` KVP (default `"player"`) to look up an
/// [`EntityTypeDescriptor`]. Component attachment uses the same descriptor
/// materialization helper as the data-archetype sweep, while
/// `components.inventory.loadout` spawns sibling wieldable instances only when
/// the target descriptors declare weapon components. The per-placement KVP bag is forwarded with
/// `entity_class` stripped so it is not confused with an `initial_*` override.
/// Tags from the `player_spawn` placement are passed directly to `try_spawn`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlayerSpawnResult {
    pub(crate) spawned: usize,
}

pub(crate) fn spawn_from_player_starts(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> PlayerSpawnResult {
    spawn_from_player_starts_with_carried_loadout(
        spawn_points,
        descriptors,
        registry,
        agent_params,
        None,
    )
}

/// Player-start materialization with an optional carried record for the first
/// local movement pawn. The caller owns seat lookup; this descriptor layer
/// deliberately receives only the already-resolved record.
pub(crate) fn spawn_from_player_starts_with_carried_loadout(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
    carried_loadout: Option<&crate::netcode::CarriedState>,
) -> PlayerSpawnResult {
    let mut spawned = 0usize;

    for entity in spawn_points {
        let entity_class = entity
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");

        let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
            log::warn!(
                "[Loader] {origin}: entity_class `{entity_class}` not registered; skipping spawn point",
                origin = entity.diagnostic_origin(),
            );
            continue;
        };

        let Some(id) = spawn_descriptor_instance(
            registry,
            descriptor,
            entity,
            true,
            DescriptorSpawnPath::PlayerSpawn,
            agent_params,
        ) else {
            log::warn!(
                "[Loader] {origin}: entity registry exhausted; dropping player spawn `{entity_class}`",
                origin = entity.diagnostic_origin(),
            );
            continue;
        };

        let is_first_local_pawn = registry.local_player_pawn().is_none()
            && matches!(
                registry.has_component_kind(id, ComponentKind::PlayerMovement),
                Ok(true)
            );
        if is_first_local_pawn {
            let _ = registry.mark_local_player_pawn(id);
            crate::netcode::restore_carried_health(carried_loadout, registry, id);
        }

        // Forward the per-placement KVP bag (sans `entity_class`, which is a
        // routing hint, not a runtime property) so `getEntityProperty` works
        // uniformly for player-start-spawned entities.
        let mut kvps = entity.key_values.clone();
        kvps.remove("entity_class");
        let _ = registry.set_map_kvps(id, kvps);

        let _ = compose_wieldable_inventory(
            registry,
            id,
            descriptor,
            entity,
            descriptors,
            is_first_local_pawn.then_some(carried_loadout).flatten(),
        );

        spawned += 1;
    }

    if !spawn_points.is_empty() {
        log::info!(
            "[Loader] spawned {spawned} player(s) from {total} player_spawn entries",
            total = spawn_points.len(),
        );
    }

    PlayerSpawnResult { spawned }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::{
        ProjectileBodyVisual, ProjectileDescriptor, ProjectileTrailVisual, ProjectileVisual,
    };
    use postretro_scripting_core::data_descriptors::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams, TouchMode,
        TouchableDescriptor, WeaponDescriptor,
    };
    use std::collections::HashMap;

    // Shared descriptor/placement builders live in the sibling fixture module so
    // the netcode agreement test can reuse them without a private-helper reach
    // or a duplicate copy. See testing_guide.md §4.
    use super::super::data_archetype_test_fixtures::{
        behavior_enemy_descriptor, mesh_descriptor, placement,
    };

    fn light_descriptor(classname: &str, is_dynamic: bool) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: Some(LightDescriptor {
                color: [0.5, 0.5, 0.5],
                intensity: 1.0,
                range: 8.0,
                is_dynamic,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    #[test]
    fn descriptor_spawn_attaches_stateless_mesh_component() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![mesh_descriptor("prop", false)];
        let placements = vec![placement("prop", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 1, "mesh-only descriptor is map-placeable");

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Mesh)
            .next()
            .expect("mesh component spawned");
        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(mesh.model, "decraniated");
        assert!(
            mesh.animation.is_none(),
            "descriptor with no `animations` block yields a stateless mesh"
        );

        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
        assert!(provenance.owns(DescriptorComponentKind::Mesh));
    }

    #[test]
    fn descriptor_mesh_offsets_capsule_center_to_feet_for_movement_and_ai_only() {
        let mesh_only = mesh_descriptor("prop", false);
        assert_eq!(
            descriptor_mesh_component(&mesh_only, None)
                .expect("mesh-only descriptor materializes")
                .origin_offset,
            Vec3::ZERO,
            "mesh-only descriptors keep their authored transform origin"
        );

        let mut movement = mesh_descriptor("player", false);
        let movement_params = movement_descriptor();
        let expected_movement_offset = capsule_center_to_feet_origin_offset(
            movement_params.capsule.radius,
            2.0 * (movement_params.capsule.half_height + movement_params.capsule.radius),
        );
        movement.movement = Some(movement_params);
        assert_eq!(
            descriptor_mesh_component(&movement, None)
                .expect("movement descriptor materializes")
                .origin_offset,
            expected_movement_offset,
            "movement-pawn meshes are authored at feet while transforms are capsule centers"
        );

        let behavior = behavior_enemy_descriptor("grunt");
        let agent_params = NavAgentParams {
            radius: 0.2,
            height: 2.0,
            ..DEFAULT_AGENT_PARAMS
        };
        assert_eq!(
            descriptor_mesh_component(&behavior, Some(agent_params))
                .expect("AI descriptor materializes")
                .origin_offset,
            capsule_center_to_feet_origin_offset(agent_params.radius, agent_params.height),
            "AI descriptors retain the established nav-agent offset"
        );
    }

    #[test]
    fn descriptor_mesh_materializes_shadow_bias_scale() {
        let mut descriptor = mesh_descriptor("prop", false);
        descriptor
            .mesh
            .as_mut()
            .expect("fixture has mesh descriptor")
            .shadow_bias_scale = 2.5;

        let mesh = descriptor_mesh_component(&descriptor, None)
            .expect("mesh descriptor materializes a mesh component");
        assert!(
            (mesh.shadow_bias_scale - 2.5).abs() < f32::EPSILON,
            "descriptor authoring value must reach the runtime mesh component"
        );
    }

    #[test]
    fn descriptor_mesh_materializes_unresolved_attachments_in_socket_order() {
        let mut descriptor = mesh_descriptor("prop", false);
        descriptor
            .mesh
            .as_mut()
            .expect("fixture has mesh descriptor")
            .attachments = [
            ("z_socket".to_string(), "models/z_prop.gltf".to_string()),
            ("a_socket".to_string(), "models/a_prop.gltf".to_string()),
        ]
        .into_iter()
        .collect();

        let mesh = descriptor_mesh_component(&descriptor, None)
            .expect("mesh descriptor materializes a mesh component");
        assert_eq!(
            mesh.attachments
                .iter()
                .map(|attachment| (attachment.socket.as_str(), attachment.model.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("a_socket", "models/a_prop.gltf"),
                ("z_socket", "models/z_prop.gltf"),
            ],
            "data-archetype materialization copies a deterministic socket/model list"
        );
        assert!(mesh.attachments.iter().all(|attachment| {
            attachment.binding
                == postretro_entities::components::mesh::AttachmentBinding::Unresolved
        }));
    }

    #[test]
    fn descriptor_spawn_attaches_animated_mesh_with_default_state() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![mesh_descriptor("decraniated_mob", true)];
        let placements = vec![placement("decraniated_mob", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Mesh)
            .next()
            .expect("animated mesh spawned");
        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(mesh.model, "decraniated");
        let anim = mesh.animation.as_ref().expect("animation block attached");
        // Declared state map copied in; current = default; entry stamp pending.
        assert_eq!(anim.default_state, "idle");
        assert_eq!(anim.current_state, "idle");
        assert!(anim.entered_at.is_none(), "spawn entry stamp is pending");
        assert!(anim.previous_state.is_none(), "no fade active at spawn");
        assert_eq!(anim.states.len(), 2);
        assert!(anim.states.contains_key("idle"));
        assert!(anim.states.contains_key("attack"));
    }

    #[test]
    fn descriptor_spawn_materializes_locomotion_contract() {
        use postretro_scripting_core::data_descriptors::LocomotionDescriptor;

        let mut descriptor = mesh_descriptor("decraniated_mob", true);
        let mesh_desc = descriptor.mesh.as_mut().unwrap();
        mesh_desc.locomotion = Some(LocomotionDescriptor { speed_scale: false });
        mesh_desc.animations.get_mut("idle").unwrap().travel_speed = Some(2.75);

        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(
            &[placement("decraniated_mob", &[])],
            &[descriptor],
            &HashSet::new(),
            &mut reg,
            None,
        );

        let (id, _) = reg.iter_with_kind(ComponentKind::Mesh).next().unwrap();
        let animation = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert!(!animation.speed_scale);
        assert_eq!(animation.states["idle"].travel_speed, Some(2.75));
    }

    #[test]
    fn descriptor_animated_mesh_exposes_model_to_distinct_model_sweep() {
        // The level-load model sweep (`distinct_mesh_models` in main.rs) keys off
        // the mesh component's `model` field via `ComponentKind::Mesh` iteration.
        // Guard the same contract from the registry side for a descriptor-spawned
        // animated mesh.
        use postretro_entities::registry::{ComponentKind, ComponentValue};

        let mut reg = EntityRegistry::new();
        let descriptors = vec![mesh_descriptor("mob", true)];
        let placements = vec![placement("mob", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let model = reg
            .iter_with_kind(ComponentKind::Mesh)
            .find_map(|(_, value)| match value {
                ComponentValue::Mesh(m) => Some(m.model.clone()),
                _ => None,
            })
            .expect("descriptor-spawned mesh exposes its model to the sweep");
        assert_eq!(model, "decraniated");
    }

    #[test]
    fn descriptor_spawn_attaches_health_component_with_current_equal_to_max() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_scripting_core::data_descriptors::{HealthDescriptor, HitboxDescriptor};

        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("target_dummy".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: Some(HealthDescriptor {
                max: 75.0,
                hitbox: Some(HitboxDescriptor {
                    half_extents: [0.5, 1.0, 0.5],
                    offset: None,
                }),
                zone_multipliers: std::collections::HashMap::new(),
            }),
            behavior: None,
        }];
        let placements = vec![placement("target_dummy", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 1, "health-only descriptor is map-placeable");

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Health)
            .next()
            .expect("health component spawned");
        let health = reg.get_component::<HealthComponent>(id).unwrap();
        assert_eq!(health.max, 75.0);
        assert_eq!(health.current, 75.0, "current initializes to max at spawn");
        assert!(health.hitbox.is_some());

        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
        assert!(provenance.owns(DescriptorComponentKind::Health));
    }

    #[test]
    fn descriptor_spawn_creates_entity_with_light_component() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 1);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .expect("light spawned");
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.is_dynamic);
        assert_eq!(light.falloff_range, 8.0);
        assert_eq!(light.origin, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn descriptor_spawn_records_provenance_with_owned_components_and_overrides() {
        let mut reg = EntityRegistry::new();
        let mut descriptor = light_descriptor("torch", true);
        descriptor.emitter = Some(BillboardEmitterComponent {
            rate: 6.0,
            burst: None,
            spread: 0.4,
            lifetime: 3.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.2,
            drag: 0.5,
            size_over_lifetime: [1.0].into(),
            opacity_over_lifetime: [1.0, 0.0].into(),
            color: [1.0, 1.0, 1.0],
            sprite: "smoke".to_string(),
            spin_rate: 0.0,
            spin_animation: None,
        });
        let descriptors = vec![descriptor];
        let placements = vec![placement(
            "torch",
            &[
                ("initial_intensity", "5.5"),
                ("initial_rate", "20.5"),
                ("initial_burst", "not-a-u32"),
            ],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::DescriptorProvenance)
            .next()
            .expect("provenance should be recorded");
        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();

        assert_eq!(provenance.canonical_name, "torch");
        assert_eq!(provenance.spawn_path, DescriptorSpawnPath::MapPlacement);
        assert!(provenance.owns(DescriptorComponentKind::Light));
        assert!(provenance.owns(DescriptorComponentKind::Emitter));
        assert!(
            provenance
                .map_overrides
                .contains(&DescriptorMapOverride::LightInitialIntensity)
        );
        assert!(
            provenance
                .map_overrides
                .contains(&DescriptorMapOverride::EmitterInitialRate)
        );
        assert!(
            !provenance
                .map_overrides
                .contains(&DescriptorMapOverride::EmitterInitialBurst),
            "invalid overrides should not be recorded as reappliable"
        );
    }

    #[test]
    fn map_sweep_skips_weapon_only_descriptors() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![weapon_descriptor("reference_pistol")];
        let placements = vec![placement("reference_pistol", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 0);
        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Weapon)
                .next()
                .is_none(),
            "weapon-only descriptors are equip targets, not direct map placements",
        );
    }

    #[test]
    fn map_sweep_skips_weapon_component_on_otherwise_placeable_descriptor() {
        let mut reg = EntityRegistry::new();
        let mut descriptor = weapon_descriptor("weapon_torch");
        descriptor.light = light_descriptor("weapon_torch", true).light;
        let descriptors = vec![descriptor];
        let placements = vec![placement("weapon_torch", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 1);

        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Weapon)
                .next()
                .is_none(),
            "direct map placement must not attach weapon components even when another component makes the descriptor placeable",
        );

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .expect("placeable sibling component still spawns");
        assert!(reg.get_component::<LightComponent>(id).unwrap().is_dynamic);
        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
        assert!(provenance.owns(DescriptorComponentKind::Light));
        assert!(!provenance.owns(DescriptorComponentKind::Weapon));
    }

    #[test]
    fn map_sweep_spawns_weapon_and_touchable_for_touchable_wieldable() {
        let mut reg = EntityRegistry::new();
        let mut descriptor = weapon_descriptor("reference_pistol");
        descriptor.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Press,
            radius: 32.0,
        });
        let placements = vec![placement("reference_pistol", &[])];

        let handled = apply_data_archetype_dispatch(
            &placements,
            &[descriptor],
            &HashSet::new(),
            &mut reg,
            None,
        );

        assert_eq!(handled.len(), 1);
        let (id, _) = reg
            .iter_with_kind(ComponentKind::Touchable)
            .next()
            .expect("touchable wieldable should spawn");
        let position = reg
            .get_component::<Transform>(id)
            .expect("world item transform should attach")
            .position;
        assert!(
            (position - Vec3::new(1.0, 2.0, 3.0)).length_squared() <= f32::EPSILON,
            "map placement should retain its authored position"
        );
        assert!(reg.get_component::<WeaponComponent>(id).is_ok());
        let touchable = reg
            .get_component::<TouchableComponent>(id)
            .expect("touchable component should attach");
        assert_eq!(touchable.mode, TouchMode::Press);
        assert!((touchable.radius - 32.0).abs() <= f32::EPSILON);
        let provenance = reg
            .get_component::<DescriptorProvenance>(id)
            .expect("descriptor provenance should attach");
        assert!(provenance.owns(DescriptorComponentKind::Weapon));
        assert!(provenance.owns(DescriptorComponentKind::Touchable));
    }

    #[test]
    fn descriptor_spawn_forces_dynamic_when_descriptor_was_baked() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", false)];
        let placements = vec![placement("torch", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        assert!(reg.get_component::<LightComponent>(id).unwrap().is_dynamic);
    }

    #[test]
    fn descriptor_spawn_skips_classnames_handled_by_builtin() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[])];
        let mut handled = HashSet::new();
        handled.insert("torch".to_string());
        let result =
            apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg, None);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn initial_prefix_kvp_overrides_descriptor_field() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement(
            "torch",
            &[("initial_intensity", "5.5"), ("initial_range", "20")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.intensity, 5.5);
        assert_eq!(light.falloff_range, 20.0);
    }

    #[test]
    fn initial_color_kvp_overrides_via_space_delimited_floats() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_color", "1.0 0.5 0.25")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.color, [1.0, 0.5, 0.25]);
    }

    #[test]
    fn initial_is_dynamic_kvp_accepts_trenchbroom_zero_one() {
        // TrenchBroom's checkbox/switch widget writes `"0"` and `"1"` into
        // the .map file. Both must parse correctly; `"true"` and `"false"`
        // remain accepted (case-insensitive) for hand-authored .map files.
        for (raw, expected) in [
            ("0", false),
            ("1", true),
            ("true", true),
            ("false", false),
            ("TRUE", true),
            ("False", false),
        ] {
            let mut reg = EntityRegistry::new();
            let descriptors = vec![light_descriptor("torch", false)];
            let placements = vec![placement("torch", &[("initial_is_dynamic", raw)])];
            apply_data_archetype_dispatch(
                &placements,
                &descriptors,
                &HashSet::new(),
                &mut reg,
                None,
            );
            // The descriptor light is forced dynamic at spawn regardless of the
            // descriptor's `is_dynamic`. To verify the parse landed on the
            // descriptor field itself, we exercise `apply_light_kvp_overrides`
            // directly:
            let mut desc = LightDescriptor {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: 1.0,
                is_dynamic: !expected,
            };
            let entity = placement("torch", &[("initial_is_dynamic", raw)]);
            apply_light_kvp_overrides(&mut desc, &entity);
            assert_eq!(
                desc.is_dynamic, expected,
                "raw `{raw}` should parse to {expected}",
            );
        }
    }

    #[test]
    fn initial_is_dynamic_kvp_falls_back_on_unrecognized_value() {
        // Any non-"0"/"1"/"true"/"false" value warns and leaves the
        // descriptor default in place.
        let mut desc = LightDescriptor {
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: 1.0,
            is_dynamic: true,
        };
        let entity = placement("torch", &[("initial_is_dynamic", "yes")]);
        apply_light_kvp_overrides(&mut desc, &entity);
        assert!(desc.is_dynamic);
    }

    #[test]
    fn initial_intensity_negative_falls_back_to_descriptor_default() {
        // Validation applied at descriptor parse time rejects negative
        // intensity. The KVP override path must apply the same check, so a
        // map author writing `initial_intensity = -5.0` does not produce
        // a descriptor that would have been rejected at ingestion time.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_intensity", "-5.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        // Descriptor default (1.0) preserved despite the bad override.
        assert_eq!(light.intensity, 1.0);
    }

    #[test]
    fn initial_range_negative_falls_back_to_descriptor_default() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_range", "-2.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.falloff_range, 8.0);
    }

    #[test]
    fn malformed_initial_kvp_falls_back_to_descriptor_default() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[("initial_intensity", "not-a-number")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .unwrap();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert_eq!(light.intensity, 1.0);
    }

    #[test]
    fn descriptor_spawn_skips_all_instances_of_a_conflicting_classname() {
        // Multiple placements with the same conflicting classname must all be
        // dropped (built-in wins). The warn-once dedup is exercised by
        // running enough placements that the guard would fire repeatedly if
        // the dedup were broken; we verify zero descriptor spawns and
        // — to actually drive the dedup path — also re-invoke dispatch with
        // another colliding placement and confirm the count stays at zero.
        // The `warn!` itself is logged via `log::warn!` and not captured here;
        // logging is verified manually.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![
            placement("torch", &[]),
            placement("torch", &[]),
            placement("torch", &[]),
        ];
        let mut handled = HashSet::new();
        handled.insert("torch".to_string());

        let result =
            apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg, None);

        assert_eq!(
            result.len(),
            0,
            "all colliding placements must be skipped by the conflict guard"
        );
        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Light)
                .next()
                .is_none(),
            "no descriptor-spawned light should land for a colliding classname"
        );

        // Second invocation: the dedup state is per-call, so the guard must
        // continue to drop colliding placements on a fresh dispatch pass.
        let more = vec![placement("torch", &[])];
        let result2 = apply_data_archetype_dispatch(&more, &descriptors, &handled, &mut reg, None);
        assert_eq!(result2.len(), 0);
    }

    #[test]
    fn emitter_initial_velocity_kvp_overrides_velocity_field() {
        // The component field is named `velocity` so the `initial_<field>`
        // KVP convention spells the override `initial_velocity` cleanly,
        // without a redundant prefix.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            inventory: None,
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0, 0.0].into(),
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement(
            "campfire",
            &[("initial_velocity", "1.0 2.0 3.0")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.velocity, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn emitter_bare_velocity_kvp_is_not_an_alias() {
        // Bare `velocity` (without the `initial_` prefix) must not consume the
        // KVP — that key is reserved for behavior-script `getEntityProperty`
        // consumption. Only `initial_velocity` writes to the field.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            inventory: None,
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0, 0.0].into(),
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement("campfire", &[("velocity", "9.0 9.0 9.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        // Descriptor default preserved — the bare `velocity` key did not
        // override the field.
        assert_eq!(component.velocity, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn emitter_initial_kvp_overrides_scalar_field() {
        // `initial_rate` overrides the descriptor's `rate` default at spawn.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("campfire".to_string()),
            inventory: None,
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0, 0.0].into(),
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement("campfire", &[("initial_rate", "20.5")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.rate, 20.5);
    }

    #[test]
    fn emitter_initial_burst_kvp_overrides_u32_field() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("burstfire".to_string()),
            inventory: None,
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 0.0,
                burst: None,
                spread: 0.4,
                lifetime: 0.6,
                velocity: [0.0, 2.0, 0.0],
                buoyancy: -1.0,
                drag: 0.1,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0, 0.0].into(),
                color: [1.0, 0.8, 0.3],
                sprite: "spark".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement("burstfire", &[("initial_burst", "24")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(component.burst, Some(24));
    }

    #[test]
    fn emitter_malformed_initial_kvp_falls_back_to_descriptor_default() {
        // A bad value for a known `initial_*` key on the emitter should warn
        // but leave the descriptor's value untouched. No crash.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("smolder".to_string()),
            inventory: None,
            light: None,
            emitter: Some(BillboardEmitterComponent {
                rate: 6.0,
                burst: None,
                spread: 0.4,
                lifetime: 3.0,
                velocity: [0.0, 1.0, 0.0],
                buoyancy: 0.2,
                drag: 0.5,
                size_over_lifetime: [1.0].into(),
                opacity_over_lifetime: [1.0, 0.0].into(),
                color: [1.0, 1.0, 1.0],
                sprite: "smoke".to_string(),
                spin_rate: 0.0,
                spin_animation: None,
            }),
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement(
            "smolder",
            &[("initial_rate", "not-a-float"), ("initial_burst", "noisy")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::BillboardEmitter)
            .next()
            .expect("emitter should still spawn");
        let component = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        // Bad scalar value left descriptor default in place.
        assert_eq!(component.rate, 6.0);
        // Bad burst value left descriptor default (None) in place.
        assert_eq!(component.burst, None);
    }

    /// Guards built-in priority over data-archetype dispatch end-to-end: a
    /// classname registered both as a built-in AND via mod-manifest ingestion
    /// must spawn through the built-in path only.
    /// Drives both dispatch sweeps in the same order `main.rs` does:
    /// `apply_classname_dispatch` first, then `apply_data_archetype_dispatch`
    /// with the returned `handled` set. Asserts exactly one entity exists.
    #[test]
    fn dual_registered_classname_spawns_through_builtin_only() {
        use crate::scripting::builtins::{
            ClassnameDispatch, apply_classname_dispatch, register_builtins,
        };
        use postretro_entities::registry::ComponentKind;

        // Built-in dispatch already covers `billboard_emitter`.
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        // Register a data-archetype descriptor for the same classname.
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("billboard_emitter".to_string()),
            inventory: None,
            light: Some(LightDescriptor {
                color: [1.0, 0.0, 0.0],
                intensity: 5.0,
                range: 10.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];

        let placements = vec![placement("billboard_emitter", &[])];

        let mut reg = EntityRegistry::new();
        let handled = apply_classname_dispatch(&placements, &dispatch, &mut reg);
        let descriptor_handled =
            apply_data_archetype_dispatch(&placements, &descriptors, &handled, &mut reg, None);

        assert_eq!(
            descriptor_handled.len(),
            0,
            "data-archetype path must skip a classname owned by built-in dispatch",
        );
        // Built-in handler spawned exactly one billboard_emitter.
        let emitter_count = reg.iter_with_kind(ComponentKind::BillboardEmitter).count();
        assert_eq!(
            emitter_count, 1,
            "exactly one billboard_emitter entity should exist (built-in only)",
        );
        // And no descriptor-driven light landed.
        assert_eq!(
            reg.iter_with_kind(ComponentKind::Light).count(),
            0,
            "descriptor's light must not have been attached — built-in path wins",
        );
    }

    #[test]
    fn descriptor_spawn_with_no_matching_descriptor_skips_silently() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("ungoverned", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 0);
    }

    #[test]
    fn descriptor_with_no_canonical_name_is_unreachable_from_direct_placement() {
        // A descriptor with `canonical_name = None` has no direct map-placement
        // form. Two `.map` placements naming "ghost" in a single dispatch pass
        // must not spawn the descriptor — and the unknown-classname warn must
        // fire exactly once per distinct classname per sweep (verified by the
        // warn-dedup `HashSet` contract; the `log::warn!` itself is not
        // captured here).
        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: None,
            inventory: None,
            light: Some(LightDescriptor {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: 5.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }];
        let placements = vec![placement("ghost", &[]), placement("ghost", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 0);
        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Light)
                .next()
                .is_none(),
            "descriptor with no canonical_name must not spawn from direct placement",
        );
    }

    #[test]
    fn descriptor_with_some_canonical_name_spawns_from_direct_placement() {
        // Regression guard: the canonical_name = Some(...) path still routes a
        // direct map placement to the matching descriptor.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("foo", true)];
        let placements = vec![placement("foo", &[])];
        let handled = apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut reg,
            None,
        );
        assert_eq!(handled.len(), 1);
        assert!(handled.contains("foo"));
        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Light)
                .next()
                .is_some(),
        );
    }

    #[test]
    fn player_spawn_marker_routes_to_named_descriptor_via_entity_class() {
        // A `player_spawn` marker resolves its target via `entity_class` (or
        // the default `"player"`). A descriptor with canonical_name =
        // Some("player") receives the spawn — exactly one entity lands.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(live_count(&reg), 1);
    }

    #[test]
    fn player_spawn_and_default_weapon_record_spawn_paths() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];
        let points = vec![spawn_point(&[])];

        let _result = spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let player_id = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("player entity should spawn with inventory");
        let player_provenance = reg
            .get_component::<DescriptorProvenance>(player_id)
            .expect("player provenance should be recorded");
        assert_eq!(
            player_provenance.spawn_path,
            DescriptorSpawnPath::PlayerSpawn
        );
        assert_eq!(player_provenance.canonical_name, "player");

        let weapon_id = reg
            .get_component::<Inventory>(player_id)
            .unwrap()
            .active_wieldable()
            .expect("default weapon should spawn as active wieldable");
        let weapon_provenance = reg
            .get_component::<DescriptorProvenance>(weapon_id)
            .expect("weapon provenance should be recorded");
        assert_eq!(
            weapon_provenance.spawn_path,
            DescriptorSpawnPath::DefaultWeapon
        );
        assert_eq!(weapon_provenance.canonical_name, "reference_pistol");
        assert!(weapon_provenance.owns(DescriptorComponentKind::Weapon));
    }

    // --- spawn_from_player_starts -------------------------------------------

    /// A descriptor with no components — sufficient as a stub `"player"` entry
    /// for spawn-point tests that only care about transform / tags / KVPs.
    fn stub_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 12.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 64.0,
                cooldown_ms: 180.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn ammo_weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = weapon_descriptor(classname);
        descriptor.weapon.as_mut().unwrap().resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "bullets.light".to_string(),
            magazine: 12,
            cost_per_shot: 1,
            reserve: 48,
            reload_ms: 900,
            reload_style: ReloadStyle::Magazine,
        }));
        descriptor
    }

    fn player_with_default_weapon(classname: &str, default_weapon: &str) -> EntityTypeDescriptor {
        player_with_loadout(classname, &[default_weapon])
    }

    fn player_with_loadout(classname: &str, loadout: &[&str]) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: Some(postretro_entities::InventoryDescriptor {
                loadout: loadout.iter().map(|name| (*name).to_string()).collect(),
            }),
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    #[test]
    fn player_inventory_loadout_spawns_ordered_wieldables_up_to_capacity() {
        let names = (0..(WIELDABLE_SLOT_CAPACITY + 1))
            .map(|index| format!("weapon_{index}"))
            .collect::<Vec<_>>();
        let loadout = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut descriptors = vec![player_with_loadout("player", &loadout)];
        descriptors.extend(names.iter().map(|name| weapon_descriptor(name)));
        let mut reg = EntityRegistry::new();

        let _result = spawn_from_player_starts(&[spawn_point(&[])], &descriptors, &mut reg, None);

        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("player inventory should be attached");
        let inventory = reg.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.switch_target, None);
        assert!(inventory.wieldables.iter().all(Option::is_some));
        assert_eq!(
            reg.iter_with_kind(ComponentKind::Weapon).count(),
            WIELDABLE_SLOT_CAPACITY
        );
        assert_eq!(live_count(&reg), WIELDABLE_SLOT_CAPACITY + 1);
    }

    #[test]
    fn o37_duplicate_descriptor_loadout_creates_independent_instances_and_one_shared_reserve() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_loadout("player", &["reference_pistol", "reference_pistol"]),
            ammo_weapon_descriptor("reference_pistol"),
        ];

        let _ = spawn_from_player_starts(&[spawn_point(&[])], &descriptors, &mut reg, None);

        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("spawned pawn owns an inventory");
        let inventory = reg.get_component::<Inventory>(pawn).unwrap();
        let first = inventory.wieldables[0].expect("first duplicate slot materializes");
        let second = inventory.wieldables[1].expect("second duplicate slot materializes");
        assert_ne!(
            first, second,
            "duplicate descriptor entries are distinct instances"
        );

        let mut first_component = reg.get_component::<WeaponComponent>(first).unwrap().clone();
        first_component.magazine = 3;
        reg.set_component(first, first_component).unwrap();
        assert_eq!(
            reg.get_component::<WeaponComponent>(second)
                .unwrap()
                .magazine,
            12,
            "changing one duplicate's magazine does not mutate its sibling"
        );
        assert_eq!(
            reg.get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            48,
            "two slots sharing one ammo type seed its pawn reserve once"
        );
    }

    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        }
    }

    fn player_with_movement(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: Some(movement_descriptor()),
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn spawn_point(kvps: &[(&str, &str)]) -> MapEntity {
        placement(PLAYER_START_CLASSNAME, kvps)
    }

    fn spawn_point_at(origin: Vec3, angles: Vec3, kvps: &[(&str, &str)]) -> MapEntity {
        let mut e = spawn_point(kvps);
        e.origin = origin;
        e.angles = angles;
        e
    }

    #[test]
    fn carried_health_restores_only_the_first_local_player_start() {
        use postretro_scripting_core::data_descriptors::HealthDescriptor;

        let mut player = player_with_movement("player");
        player.health = Some(HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        let mut registry = EntityRegistry::new();
        let starts = [
            spawn_point(&[]),
            spawn_point_at(Vec3::new(8.0, 0.0, 0.0), Vec3::ZERO, &[]),
        ];

        spawn_from_player_starts_with_carried_loadout(
            &starts,
            &[player],
            &mut registry,
            None,
            Some(&crate::netcode::CarriedState {
                health_current: Some(36.0),
                ..Default::default()
            }),
        );

        let local = registry
            .local_player_pawn()
            .expect("first movement pawn is local");
        let local_health = registry
            .get_component::<HealthComponent>(local)
            .expect("descriptor materialized health")
            .current;
        assert!(
            (local_health - 36.0).abs() <= 1.0e-6,
            "expected carried health 36.0, got {local_health}"
        );
        let other_health: Vec<f32> = registry
            .iter_with_kind(ComponentKind::Health)
            .filter_map(|(id, _)| (id != local).then_some(id))
            .map(|id| {
                registry
                    .get_component::<HealthComponent>(id)
                    .unwrap()
                    .current
            })
            .collect();
        assert_eq!(other_health.len(), 1);
        assert!(
            (other_health[0] - 100.0).abs() <= 1.0e-6,
            "the second player start must retain descriptor health"
        );
    }

    fn live_count(reg: &EntityRegistry) -> usize {
        reg.iter_with_kind(postretro_entities::registry::ComponentKind::Transform)
            .count()
    }

    #[test]
    fn single_spawn_point_spawns_one_entity_at_position_and_facing() {
        use postretro_scripting_core::conv::EulerDegrees;

        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let origin = Vec3::new(4.0, 5.0, 6.0);
        // pitch=10°, yaw=-30°, roll=0° — exercises two axes without hitting
        // 90° boundaries where YXZ vs other orderings collapse.
        let pitch_deg: f32 = 10.0;
        let yaw_deg: f32 = -30.0;
        let roll_deg: f32 = 0.0;
        let angles = Vec3::new(
            pitch_deg.to_radians(),
            yaw_deg.to_radians(),
            roll_deg.to_radians(),
        );
        let points = vec![spawn_point_at(origin, angles, &[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(live_count(&reg), 1);
        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        let t = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(t.position, origin);
        // Assert rotation against a known degree input via EulerDegrees::to_quat,
        // not against rotation_quat() itself (which would be a tautology).
        let expected = EulerDegrees {
            pitch: pitch_deg,
            yaw: yaw_deg,
            roll: roll_deg,
        }
        .to_quat();
        let eps = 1e-5;
        assert!(
            (t.rotation.x - expected.x).abs() < eps
                && (t.rotation.y - expected.y).abs() < eps
                && (t.rotation.z - expected.z).abs() < eps
                && (t.rotation.w - expected.w).abs() < eps,
            "rotation mismatch: got {:?}, expected {:?}",
            t.rotation,
            expected,
        );
    }

    #[test]
    fn single_spawn_point_marks_spawned_movement_pawn_as_local() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![player_with_movement("player")];
        let origin = Vec3::new(4.0, 5.0, 6.0);
        let points = vec![spawn_point_at(origin, Vec3::ZERO, &[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let local = reg
            .local_player_pawn()
            .expect("player_spawn should mark the selected local pawn");
        assert!(
            reg.get_component::<PlayerMovementComponent>(local).is_ok(),
            "marked local pawn should carry PlayerMovement"
        );
        assert_eq!(
            reg.get_component::<Transform>(local).unwrap().position,
            origin
        );
    }

    #[test]
    fn multiple_spawn_points_spawn_one_entity_each() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[]), spawn_point(&[]), spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(live_count(&reg), 3);
    }

    #[test]
    fn multiple_spawn_points_mark_first_loaded_movement_pawn_in_placement_order() {
        fn marked_position(points: Vec<MapEntity>) -> Vec3 {
            let mut reg = EntityRegistry::new();
            let descriptors = vec![player_with_movement("player")];
            spawn_from_player_starts(&points, &descriptors, &mut reg, None);
            let local = reg
                .local_player_pawn()
                .expect("one spawned pawn should be marked local");
            reg.get_component::<Transform>(local).unwrap().position
        }

        let alpha = spawn_point_at(Vec3::new(-3.0, 0.0, 0.0), Vec3::ZERO, &[]);
        let beta = spawn_point_at(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, &[]);

        assert_eq!(
            marked_position(vec![alpha.clone(), beta.clone()]),
            Vec3::new(-3.0, 0.0, 0.0)
        );
        assert_eq!(
            marked_position(vec![beta, alpha]),
            Vec3::new(3.0, 0.0, 0.0),
            "the selected local pawn follows spawn-point placement order"
        );
    }

    #[test]
    fn player_spawn_marks_first_successful_movement_pawn_when_earlier_spawn_has_no_movement() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("prop"), player_with_movement("player")];
        let prop_origin = Vec3::new(-3.0, 0.0, 0.0);
        let player_origin = Vec3::new(3.0, 0.0, 0.0);
        let points = vec![
            spawn_point_at(prop_origin, Vec3::ZERO, &[("entity_class", "prop")]),
            spawn_point_at(player_origin, Vec3::ZERO, &[("entity_class", "player")]),
        ];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(live_count(&reg), 2);
        let local = reg
            .local_player_pawn()
            .expect("later movement pawn should be marked local");
        assert!(
            reg.get_component::<PlayerMovementComponent>(local).is_ok(),
            "marked local pawn should carry PlayerMovement"
        );
        assert_eq!(
            reg.get_component::<Transform>(local).unwrap().position,
            player_origin
        );
    }

    #[test]
    fn entity_class_defaults_to_player_when_kvp_absent() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(live_count(&reg), 1);
    }

    #[test]
    fn player_spawn_materializes_default_weapon_as_active_wieldable() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];
        let points = vec![spawn_point(&[])];

        let result = spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(result.spawned, 1);
        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("player inventory");
        let weapon_id = reg
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .expect("active wieldable");
        let weapon = reg.get_component::<WeaponComponent>(weapon_id).unwrap();
        assert_eq!(weapon.damage, 12.0);
        assert_eq!(weapon.effective().credit_source, "reference_pistol");
        assert_eq!(live_count(&reg), 2, "player plus sibling weapon entity");
    }

    #[test]
    fn player_spawn_seeds_pawn_local_ammo_reserve_and_full_weapon_magazine() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            ammo_weapon_descriptor("reference_pistol"),
        ];

        let _result = spawn_from_player_starts(&[spawn_point(&[])], &descriptors, &mut reg, None);
        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("spawned pawn");
        let weapon_id = reg
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .expect("active wieldable");

        assert_eq!(
            reg.get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            48
        );
        assert_eq!(
            reg.get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .magazine,
            12
        );
    }

    #[test]
    fn player_spawn_without_weapon_resource_does_not_create_ammo_reserve() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];

        let _result = spawn_from_player_starts(&[spawn_point(&[])], &descriptors, &mut reg, None);
        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .unwrap();
        let _weapon_id = reg
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();

        assert!(reg.get_component::<AmmoReserve>(pawn).is_err());
    }

    #[test]
    fn default_weapon_must_resolve_to_weapon_descriptor() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "torch"),
            light_descriptor("torch", true),
        ];
        let points = vec![spawn_point(&[])];

        let result = spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(result.spawned, 1);
        let pawn = reg
            .iter_with_kind(ComponentKind::Inventory)
            .next()
            .map(|(id, _)| id)
            .expect("pawn receives empty inventory");
        assert!(
            reg.get_component::<Inventory>(pawn)
                .unwrap()
                .active_wieldable()
                .is_none()
        );
        assert_eq!(
            live_count(&reg),
            1,
            "player spawned without a weapon entity"
        );
        assert!(
            reg.iter_with_kind(postretro_entities::registry::ComponentKind::Weapon)
                .next()
                .is_none(),
            "non-weapon inventory target must not produce an active no-op entity",
        );
    }

    #[test]
    fn entity_class_kvp_routes_to_named_descriptor() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player"), stub_descriptor("spectator")];
        let points = vec![
            spawn_point(&[("entity_class", "player")]),
            spawn_point(&[("entity_class", "spectator")]),
        ];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        // Both spawn — exactly one per spawn point regardless of routing.
        assert_eq!(live_count(&reg), 2);
    }

    #[test]
    fn unknown_entity_class_is_skipped() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![
            spawn_point(&[("entity_class", "ghost")]),
            spawn_point(&[]), // defaults to "player" — should still spawn
        ];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        assert_eq!(
            live_count(&reg),
            1,
            "only the spawn point with a registered entity_class should land",
        );
    }

    #[test]
    fn empty_spawn_points_list_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        spawn_from_player_starts(&[], &descriptors, &mut reg, None);
        assert_eq!(live_count(&reg), 0);
    }

    #[test]
    fn tags_are_forwarded_to_spawned_entity() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let mut sp = spawn_point(&[]);
        sp.tags = vec!["co-op".to_string(), "team-red".to_string()];
        let points = vec![sp];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        let tags = reg.get_tags(id).unwrap();
        assert_eq!(tags, &["co-op".to_string(), "team-red".to_string()]);
    }

    #[test]
    fn custom_kvps_are_forwarded_with_entity_class_stripped() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![stub_descriptor("player")];
        let points = vec![spawn_point(&[
            ("entity_class", "player"),
            ("loadout", "shotgun"),
        ])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Transform)
            .next()
            .unwrap();
        // Custom KVP available via the same bag data-archetype-spawned entities use.
        assert_eq!(
            reg.get_map_kvp(id, "loadout").unwrap().as_deref(),
            Some("shotgun"),
        );
        // `entity_class` is a routing hint, not a runtime property — stripped.
        assert_eq!(reg.get_map_kvp(id, "entity_class").unwrap(), None);
    }

    #[test]
    fn descriptor_components_attach_to_player_start_spawn() {
        // A `"player"` archetype carrying a light descriptor should produce a
        // light-bearing entity at the spawn point.
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("player", true)];
        let points = vec![spawn_point_at(Vec3::new(7.0, 0.0, 0.0), Vec3::ZERO, &[])];

        spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(postretro_entities::registry::ComponentKind::Light)
            .next()
            .expect("descriptor light should attach to spawn-point entity");
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.is_dynamic);
        assert_eq!(light.origin, [7.0, 0.0, 0.0]);
    }

    // ---- Connected-client host-authoritative placement suppression ----

    #[test]
    fn behavior_descriptor_materializes_ai_enemy() {
        // An authored behavior graph is the sole AI classifier; light,
        // mesh, and health-only descriptors are non-AI props.
        assert!(descriptor_materializes_ai_enemy(
            &behavior_enemy_descriptor("grunt")
        ));
        assert!(!descriptor_materializes_ai_enemy(&mesh_descriptor(
            "prop", false
        )));
        assert!(!descriptor_materializes_ai_enemy(&light_descriptor(
            "torch", true
        )));
    }

    #[test]
    fn descriptor_materializes_world_item_when_touchable() {
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });

        assert!(descriptor_materializes_world_item(&item));
        assert!(!descriptor_materializes_world_item(&mesh_descriptor(
            "crate", false
        )));
    }

    #[test]
    fn client_filter_drops_host_replicated_placements_keeps_props() {
        // The connected-client pre-dispatch filter drops behavior-authored AI enemies
        // and touchable world items, while keeping ordinary local props.
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            item,
            mesh_descriptor("crate", false),
        ];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
        ];

        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);

        assert_eq!(
            kept.len(),
            1,
            "host-replicated placements drop, crate stays"
        );
        assert_eq!(kept[0].classname, "crate");
    }

    #[test]
    fn suppressed_host_replicated_mesh_models_collects_filtered_models_for_upload() {
        // Regression: a connected client filters host-replicated placements out
        // before dispatch, so their models are absent from the registry-driven upload
        // set. This pins the level-load union for both AI enemies and world items.
        let mut grunt = behavior_enemy_descriptor("grunt");
        grunt.mesh.as_mut().unwrap().attachments =
            [("hand".to_string(), "models/grunt_prop.gltf".to_string())]
                .into_iter()
                .collect();
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        item.mesh = mesh_descriptor("reference_pistol", false).mesh;
        item.mesh.as_mut().expect("fixture mesh").model = "models/pistol_world.gltf".to_string();
        let descriptors = vec![grunt, item, mesh_descriptor("crate", false)];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
            placement("mystery", &[]),
        ];

        let models = suppressed_client_host_replicated_mesh_models(&placements, &descriptors);

        // Both host-replicated categories contribute their models, while the ordinary
        // crate and unknown classname add nothing.
        assert_eq!(
            models,
            vec![
                "decraniated".to_string(),
                "models/grunt_prop.gltf".to_string(),
                "models/pistol_world.gltf".to_string(),
            ],
            "suppressed host-replicated models must preload"
        );
    }

    #[test]
    fn suppressed_host_replicated_mesh_models_empty_without_suppressed_placements() {
        // No map-referenced host-replicated placement means the ordinary registry
        // sweep already covers every mesh.
        let descriptors = vec![mesh_descriptor("crate", false)];
        let placements = vec![placement("crate", &[]), placement("mystery", &[])];

        assert!(
            suppressed_client_host_replicated_mesh_models(&placements, &descriptors).is_empty()
        );
    }

    #[test]
    fn movement_descriptor_mesh_models_collects_every_player_presentation() {
        // Hosts may assign any movement descriptor to a joining slot, and clients
        // receive the same set through snapshots. Preload holder and attachment
        // models; unrelated mesh-only descriptors must not leak in.
        let mut avatar = player_with_movement("co_op_avatar");
        avatar.mesh = mesh_descriptor("co_op_avatar", false).mesh;
        let mesh = avatar.mesh.as_mut().expect("fixture supplies a mesh");
        mesh.model = "models/exo_red/model.gltf".to_string();
        mesh.attachments = [
            ("hand_r".to_string(), "models/smg/model.gltf".to_string()),
            ("back".to_string(), "models/backpack/model.gltf".to_string()),
        ]
        .into_iter()
        .collect();

        let models = movement_descriptor_mesh_models(&[
            avatar,
            mesh_descriptor("scenery", false),
            player_with_movement("invisible_player"),
        ]);

        assert_eq!(
            models,
            vec![
                "models/exo_red/model.gltf".to_string(),
                "models/backpack/model.gltf".to_string(),
                "models/smg/model.gltf".to_string(),
            ],
            "only movement descriptors contribute and attachments retain deterministic socket order"
        );
    }

    #[test]
    fn weapon_presentation_models_collects_nonempty_third_and_first_person_paths_once() {
        let mut pistol = weapon_descriptor("pistol");
        pistol.weapon.as_mut().unwrap().third_person_model =
            Some("models/pistol/model.gltf".to_string());
        pistol.weapon.as_mut().unwrap().viewmodel = Some("models/pistol/view.gltf".to_string());
        let mut duplicate = weapon_descriptor("pistol_variant");
        duplicate.weapon.as_mut().unwrap().third_person_model =
            Some("models/pistol/model.gltf".to_string());
        duplicate.weapon.as_mut().unwrap().viewmodel = Some("models/pistol/view.gltf".to_string());
        let mut rifle = weapon_descriptor("rifle");
        rifle.weapon.as_mut().unwrap().third_person_model =
            Some("models/rifle/model.gltf".to_string());
        let mut empty = weapon_descriptor("empty");
        empty.weapon.as_mut().unwrap().third_person_model = Some(String::new());

        assert_eq!(
            weapon_presentation_models(&[
                pistol,
                mesh_descriptor("scenery", false),
                duplicate,
                empty,
                rifle,
            ]),
            vec![
                "models/pistol/model.gltf".to_string(),
                "models/pistol/view.gltf".to_string(),
                "models/rifle/model.gltf".to_string(),
            ],
            "declared weapon presentation models preserve descriptor order and dedupe paths"
        );
    }

    #[test]
    fn projectile_presentation_assets_retains_every_collection_consumer_contract() {
        // Regression: flight entities materialize after the registry-based level
        // sweep, so their models and sprite collections must come from descriptors.
        let mut sprite = weapon_descriptor("plasma");
        sprite.weapon.as_mut().unwrap().resolution = ResolutionMode::Projectile;
        sprite.weapon.as_mut().unwrap().projectile = Some(ProjectileDescriptor {
            speed: 40.0,
            radius: 0.2,
            lifetime_ms: 2_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/plasma.png".to_string(),
                    size: 0.4,
                    opacity: 0.9,
                    rotation: 0.0,
                    tint: [0.2, 0.8, 1.0],
                    emissive: 2.5,
                    frame_duration_ms: Some(60.0),
                },
                trail: Some(ProjectileTrailVisual {
                    sprite: "sprites/trail.png".to_string(),
                    rate: 60.0,
                    lifetime: 0.5,
                    burst: None,
                    spread: 0.0,
                    velocity: [0.0, 0.0, 0.0],
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_over_lifetime: vec![0.2, 0.0],
                    opacity_over_lifetime: vec![1.0, 0.0],
                    color: [1.0, 1.0, 1.0],
                    spin_rate: 0.0,
                    spin_animation: None,
                }),
                light: None,
                impact_light: None,
            },
        });
        let mut rocket = weapon_descriptor("rocket");
        rocket.weapon.as_mut().unwrap().resolution = ResolutionMode::Projectile;
        rocket.weapon.as_mut().unwrap().projectile = Some(ProjectileDescriptor {
            speed: 25.0,
            radius: 0.4,
            lifetime_ms: 3_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Model {
                    model: "models/rocket.gltf".to_string(),
                },
                // Deliberately shares the first weapon's trail collection with a
                // conflicting lifetime. Harvest must retain both contracts so
                // level install can report the conflict instead of first-wins.
                trail: Some(ProjectileTrailVisual {
                    sprite: "sprites/trail.png".to_string(),
                    rate: 20.0,
                    lifetime: 0.75,
                    burst: None,
                    spread: 0.0,
                    velocity: [0.0, 0.0, 0.0],
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_over_lifetime: vec![0.2, 0.0],
                    opacity_over_lifetime: vec![1.0, 0.0],
                    color: [1.0, 1.0, 1.0],
                    spin_rate: 0.0,
                    spin_animation: None,
                }),
                light: None,
                impact_light: None,
            },
        });

        let (models, sprites) = projectile_presentation_assets(&[sprite, rocket]);

        assert_eq!(models, vec!["models/rocket.gltf"]);
        assert_eq!(sprites.len(), 3);
        let expected = [
            (
                "sprites/trail.png",
                Some(0.5),
                0.0,
                None,
                "plasma.projectile.visual.trail",
            ),
            (
                "sprites/plasma.png",
                None,
                2.5,
                Some(60.0),
                "plasma.projectile.visual.body",
            ),
            (
                "sprites/trail.png",
                Some(0.75),
                0.0,
                None,
                "rocket.projectile.visual.trail",
            ),
        ];
        for (actual, (collection, lifetime, emissive, frame_duration_ms, source)) in
            sprites.iter().zip(expected)
        {
            assert_eq!(actual.collection, collection);
            assert_eq!(actual.source, source);
            match (actual.lifetime, lifetime) {
                (Some(actual), Some(expected)) => {
                    assert!((actual - expected).abs() <= f32::EPSILON)
                }
                (None, None) => {}
                values => panic!("lifetime mismatch: {values:?}"),
            }
            assert!((actual.emissive - emissive).abs() <= f32::EPSILON);
            match (actual.frame_duration_ms, frame_duration_ms) {
                (Some(actual), Some(expected)) => {
                    assert!((actual - expected).abs() <= f32::EPSILON)
                }
                (None, None) => {}
                values => panic!("frame duration mismatch: {values:?}"),
            }
        }
    }

    #[test]
    fn touchable_wieldable_world_models_collects_loadout_only_drop_assets() {
        // Regression: a touchable weapon referenced only by a starting inventory
        // lost its MeshComponent before the install sweep, so a later drop had no
        // uploaded world model or clip data.
        let mut droppable = weapon_descriptor("droppable");
        droppable.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        droppable.mesh = mesh_descriptor("droppable", false).mesh;
        let mesh = droppable
            .mesh
            .as_mut()
            .expect("fixture supplies world mesh");
        mesh.model = "models/droppable/world.gltf".to_string();
        mesh.attachments = [
            (
                "muzzle".to_string(),
                "models/droppable/muzzle.gltf".to_string(),
            ),
            (
                "battery".to_string(),
                "models/droppable/battery.gltf".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let mut held_only = weapon_descriptor("held_only");
        held_only.mesh = mesh_descriptor("held_only", false).mesh;
        let mut non_weapon_touchable = mesh_descriptor("touch_prop", false);
        non_weapon_touchable.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });

        assert_eq!(
            touchable_wieldable_world_models(&[
                droppable,
                held_only,
                non_weapon_touchable,
                mesh_descriptor("scenery", false),
            ]),
            vec![
                "models/droppable/world.gltf".to_string(),
                "models/droppable/battery.gltf".to_string(),
                "models/droppable/muzzle.gltf".to_string(),
            ],
            "only meshes that the drop path can restore are preloaded"
        );
    }

    #[test]
    fn client_filter_retains_unknown_classname_placements() {
        // A placement with no descriptor match is not host-replicated; the filter
        // retains it so the dispatch's own unknown-classname diagnostics fire.
        let descriptors = vec![behavior_enemy_descriptor("grunt")];
        let placements = vec![placement("mystery", &[]), placement("grunt", &[])];

        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].classname, "mystery");
    }

    #[test]
    fn client_filtered_dispatch_spawns_no_brain_but_host_dispatch_does() {
        // End-to-end on the dispatch seam: the SAME placements + descriptors
        // produce a Brain-bearing entity through the unfiltered (host /
        // single-player) path but NONE through the connected-client filtered
        // path. The non-AI prop materializes in BOTH.
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![placement("grunt", &[]), placement("crate", &[])];

        // Host / single-player: every placement dispatched unfiltered.
        let mut host_reg = EntityRegistry::new();
        apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut host_reg,
            None,
        );
        assert!(
            host_reg
                .iter_with_kind(ComponentKind::Brain)
                .next()
                .is_some(),
            "host/single-player materializes the AI enemy locally"
        );
        let host_crates = host_reg
            .iter_with_kind(ComponentKind::Mesh)
            .filter(|(id, _)| {
                host_reg
                    .get_component::<DescriptorProvenance>(*id)
                    .map(|p| p.canonical_name == "crate")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(host_crates, 1, "host materializes the non-AI prop");

        // Connected client: filter host-replicated placements before dispatch.
        let mut client_reg = EntityRegistry::new();
        let client_placements =
            filter_out_client_host_replicated_placements(&placements, &descriptors);
        apply_data_archetype_dispatch(
            &client_placements,
            &descriptors,
            &HashSet::new(),
            &mut client_reg,
            None,
        );
        assert!(
            client_reg
                .iter_with_kind(ComponentKind::Brain)
                .next()
                .is_none(),
            "connected client must NOT spawn a local authoritative AI enemy"
        );
        assert!(
            client_reg
                .iter_with_kind(ComponentKind::Agent)
                .next()
                .is_none(),
            "no Agent either — the AI pair is suppressed together"
        );
        let client_crates = client_reg
            .iter_with_kind(ComponentKind::Mesh)
            .filter(|(id, _)| {
                client_reg
                    .get_component::<DescriptorProvenance>(*id)
                    .map(|p| p.canonical_name == "crate")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            client_crates, 1,
            "connected client still materializes the non-AI prop"
        );
    }

    #[test]
    fn behavior_descriptor_materialization_yields_live_brain_and_agent() {
        // Invariant the suppression mechanism rests on: a brain descriptor
        // materialization attaches BOTH live `Brain` and `Agent` columns.
        // Without this, `is_networked_ai_enemy` (which reads those live columns)
        // and the pre-materialization `descriptor_materializes_ai_enemy` could
        // disagree.
        let descriptors = vec![behavior_enemy_descriptor("grunt")];
        let placements = vec![placement("grunt", &[])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("behavior descriptor materializes a Brain");
        assert!(
            matches!(reg.has_component_kind(id, ComponentKind::Agent), Ok(true)),
            "behavior descriptor materializes an Agent alongside the Brain"
        );
    }

    #[test]
    fn behavior_descriptor_gets_capsule_center_to_feet_offsets() {
        // The other two branches gated on "carries a brain": the spawn transform
        // shift and the mesh render-origin offset. They are equal-and-opposite.
        use postretro_foundation::NavAgentParams;

        let params = Some(NavAgentParams {
            radius: 0.4,
            height: 1.6,
            step_height: 0.3,
            max_slope_deg: 45.0,
        });
        let descriptor = behavior_enemy_descriptor("grunt");
        let shift = ai_capsule_center_from_feet_offset(&descriptor, params);
        assert_eq!(
            shift,
            Vec3::new(0.0, 0.8, 0.0),
            "the spawn transform lifts feet-authored origins to capsule center"
        );
        let mesh = descriptor_mesh_component(&descriptor, params)
            .expect("behavior descriptor carries a mesh");
        assert_eq!(
            mesh.origin_offset, -shift,
            "the mesh renders back down at the feet"
        );
        assert_eq!(
            descriptor_mesh_component(&mesh_descriptor("crate", false), params)
                .unwrap()
                .origin_offset,
            Vec3::ZERO,
            "a brain-less prop takes no capsule offset at all"
        );
    }

    #[test]
    fn behavior_descriptor_spawn_seeds_the_authored_graph_on_the_brain() {
        // The authored graph is retained verbatim and the brain starts in its
        // `initial` state.
        use postretro_entities::components::brain::BrainComponent;

        let descriptor = behavior_enemy_descriptor("grunt");
        let authored = descriptor
            .behavior
            .clone()
            .expect("the fixture authors a behavior graph");
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(
            &[placement("grunt", &[])],
            &[descriptor],
            &HashSet::new(),
            &mut reg,
            None,
        );

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("behavior descriptor materializes a Brain");
        let brain = reg.get_component::<BrainComponent>(id).unwrap();
        assert_eq!(*brain.graph, authored, "the authored graph is retained");
        assert_eq!(brain.state_name(), Some(authored.envelope.initial.as_str()));
        assert_eq!(
            brain.home_anchor,
            reg.get_component::<Transform>(id)
                .expect("behavior enemy has a transform")
                .position,
            "host brain anchors to its spawn transform rather than descriptor data"
        );
        assert_eq!(
            reg.entity_state_mut(id)
                .expect("behavior enemy carries entity state")
                .get(FACTION_STATE_FIELD),
            ENEMY_DEFAULT_FACTION,
            "host descriptor assembly seeds the transparent default enemy faction"
        );
    }

    #[test]
    fn behavior_enemy_enabled_on_spawn_false_seeds_closed_aggro_gate() {
        use postretro_entities::components::brain::BrainComponent;

        let descriptors = vec![behavior_enemy_descriptor("sealed_grunt")];
        let placements = vec![placement("sealed_grunt", &[("enabled_on_spawn", "false")])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("AI enemy should materialize a brain");
        assert!(
            !reg.get_component::<BrainComponent>(id).unwrap().aggro_armed,
            "the bare placement KVP closes the host-only aggro gate"
        );
    }

    #[test]
    fn behavior_enemy_missing_or_malformed_enabled_on_spawn_defaults_gate_open() {
        use postretro_entities::components::brain::BrainComponent;

        let descriptors = vec![behavior_enemy_descriptor("grunt")];
        let placements = vec![
            placement("grunt", &[]),
            placement("grunt", &[("enabled_on_spawn", "not-a-bool")]),
        ];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let gates: Vec<bool> = reg
            .iter_with_kind(ComponentKind::Brain)
            .map(|(id, _)| reg.get_component::<BrainComponent>(id).unwrap().aggro_armed)
            .collect();
        assert_eq!(gates, vec![true, true]);
    }

    #[test]
    fn behavior_descriptor_spawn_uses_capsule_center_transform_without_moving_authored_hitbox() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_foundation::NavAgentParams;
        use postretro_scripting_core::data_descriptors::{HealthDescriptor, HitboxDescriptor};

        let params = NavAgentParams {
            radius: 0.4,
            height: 1.6,
            step_height: 0.3,
            max_slope_deg: 45.0,
        };
        let mut descriptor = behavior_enemy_descriptor("grunt");
        descriptor.health = Some(HealthDescriptor {
            max: 60.0,
            hitbox: Some(HitboxDescriptor {
                half_extents: [0.4, 0.9, 0.4],
                offset: Some([0.0, 0.9, 0.0]),
            }),
            zone_multipliers: std::collections::HashMap::new(),
        });
        let placement = placement("grunt", &[]);
        let authored_hitbox_center = placement.origin + Vec3::new(0.0, 0.9, 0.0);

        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(
            &[placement],
            &[descriptor],
            &HashSet::new(),
            &mut reg,
            Some(params),
        );

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Agent)
            .next()
            .expect("behavior descriptor materializes an Agent");
        let transform = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(
            transform.position,
            Vec3::new(1.0, 2.8, 3.0),
            "AI gameplay transform is normalized to capsule center at spawn"
        );

        let health = reg.get_component::<HealthComponent>(id).unwrap();
        let hitbox = health.hitbox.expect("authored hitbox materialized");
        assert!(
            (transform.position + hitbox.offset - authored_hitbox_center).length() < 1.0e-5,
            "spawn-time transform normalization must preserve the authored world hitbox"
        );
    }
}
