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
use crate::nav::NavAgentParams;
use crate::scripting::components::agent::attach_agent;
use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::brain::{attach_brain, validate_brain_animation_states};
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::light::{FalloffKind, LightComponent, LightKind};
use crate::scripting::components::mesh::{MeshAnimation, MeshComponent};
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::components::weapon::WeaponComponent;
use crate::scripting::data_descriptors::{EntityTypeDescriptor, LightDescriptor};
use crate::scripting::provenance::{
    DescriptorComponentKind, DescriptorMapOverride, DescriptorProvenance, DescriptorSpawnPath,
    parse_bool,
};
use crate::scripting::registry::{ComponentKind, EntityId, EntityRegistry, Transform};

/// Capsule fallback for a descriptor-spawned agent when the map has no navmesh
/// (`agent_params == None`). The agent still materializes — it simply cannot
/// path until a navmesh is present. Values mirror the canonical human-ish agent
/// the navmesh bake targets (`NavAgentParams` defaults), so a fallback capsule
/// is plausibly sized even with no bake to read from.
const DEFAULT_AGENT_PARAMS: NavAgentParams = NavAgentParams {
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
            "rate" => {
                if parse_into_f32(raw, &mut component.rate, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialRate);
                }
            }
            "spread" => {
                if parse_into_f32(raw, &mut component.spread, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialSpread);
                }
            }
            "lifetime" => {
                if parse_into_f32(raw, &mut component.lifetime, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialLifetime);
                }
            }
            "buoyancy" => {
                if parse_into_f32(raw, &mut component.buoyancy, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialBuoyancy);
                }
            }
            "drag" => {
                if parse_into_f32(raw, &mut component.drag, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialDrag);
                }
            }
            "spin_rate" => {
                if parse_into_f32(raw, &mut component.spin_rate, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialSpinRate);
                }
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
            "color" => {
                if parse_into_vec3(raw, &mut component.color, entity, key) {
                    applied.insert(DescriptorMapOverride::EmitterInitialColor);
                }
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
            "intensity" => {
                if parse_into_nonneg_f32(raw, &mut descriptor.intensity, entity, key) {
                    applied.insert(DescriptorMapOverride::LightInitialIntensity);
                }
            }
            "range" => {
                if parse_into_nonneg_f32(raw, &mut descriptor.range, entity, key) {
                    applied.insert(DescriptorMapOverride::LightInitialRange);
                }
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
/// Roadmap: composable archetypes, FGD generated from the registry, and a
/// `mob_spawn` marker (mirroring `player_spawn`) will sit on this lookup.
/// The abstraction grows from this single match site.
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
}

/// Whether materializing this descriptor would attach the engine-owned AI pair
/// (`ComponentKind::Brain` + `ComponentKind::Agent`) — i.e. whether the
/// descriptor carries an `ai` block. This is the *pre-materialization* mirror of
/// the live-component predicate `crate::netcode::is_networked_ai_map_enemy`,
/// which can only inspect those components AFTER an entity exists: the `ai`
/// block is the sole thing `attach_descriptor_components` keys the `Brain` +
/// `Agent` attachment on, so `descriptor.ai.is_some()` holds exactly when that
/// predicate would later return `true` for a `MapPlacement` spawn of this
/// descriptor.
///
/// Used by the connected-client install path (E10 Task 5) to drop AI-enemy map
/// placements *before* dispatch, since those enemies must arrive only as
/// host-authoritative snapshots — never as locally-spawned authoritative copies.
/// Keying on `ai.is_some()` (not classname strings, not
/// `DescriptorProvenance.owned_components`, which never tracks AI) keeps the same
/// single definition of "AI map enemy" the live predicate enforces.
pub(crate) fn descriptor_materializes_ai_enemy(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.ai.is_some()
}

/// Partition map placements for a **connected client** install (E10 Task 5):
/// returns only the placements that should still materialize locally, dropping
/// any whose matched descriptor would materialize an authoritative AI enemy
/// ([`descriptor_materializes_ai_enemy`]). Those enemies are host-authoritative
/// and reach the client solely via host snapshots (a later task materializes the
/// remote presentation); spawning a local authoritative copy here would be a
/// second, never-replicated brain.
///
/// Placements whose classname has no descriptor match are retained untouched —
/// they are not AI enemies and the downstream dispatch handles their
/// unknown-classname / built-in-collision diagnostics exactly as it would on a
/// host. Single-player and listen-host installs never call this (they keep every
/// placement); only the connected-client lifecycle path filters.
pub(crate) fn filter_out_client_ai_enemies(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<MapEntity> {
    entities
        .iter()
        .filter(
            |entity| match find_descriptor(descriptors, &entity.classname) {
                Some(descriptor) => !descriptor_materializes_ai_enemy(descriptor),
                // No descriptor match: not an AI enemy — retain for the normal
                // unknown-classname diagnostics in dispatch.
                None => true,
            },
        )
        .cloned()
        .collect()
}

/// Collect the distinct, non-empty mesh model handles referenced by the
/// AI-enemy map placements a connected client suppresses
/// ([`filter_out_client_ai_enemies`]), preserving first-seen order. GPU-free:
/// this is the pure analogue of [`crate::distinct_mesh_models`] for placements
/// that never spawn a local `MeshComponent` on a connected client, so the
/// registry-driven sweep cannot see them.
///
/// Scoped to the classes the **map actually references** (the placements passed
/// in), not every AI descriptor in the data registry — only enemies the host
/// can replicate into this level need their model on the GPU. A placement is
/// included only when its matched descriptor both materializes an AI enemy
/// ([`descriptor_materializes_ai_enemy`]) AND carries a `mesh` block with a
/// non-empty `model`; non-AI placements and AI descriptors without a renderable
/// mesh contribute nothing.
///
/// Regression (E10 AC #3): a connected client filtered out the AI-enemy
/// placement before dispatch, so its model was never in the registry-driven
/// upload set; when the host snapshot later materialized the remote enemy the
/// draw planner dropped it (no uploaded mesh in the model cache) and the real
/// model never rendered — only a dev-tools debug capsule showed. The level-load
/// sweep unions these handles with [`crate::distinct_mesh_models`] so the
/// suppressed enemy's model is uploaded up front.
///
/// Each returned string is the VERBATIM renderer cache key (the descriptor's
/// `mesh.model`), identical in shape to [`crate::distinct_mesh_models`] output,
/// so the caller can dedup the two sets and upload each handle once.
pub(crate) fn suppressed_ai_enemy_mesh_models(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for entity in entities {
        let Some(descriptor) = find_descriptor(descriptors, &entity.classname) else {
            continue;
        };
        if !descriptor_materializes_ai_enemy(descriptor) {
            continue;
        }
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        if mesh.model.is_empty() {
            continue;
        }
        if seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
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
fn attach_descriptor_components(
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

    if let Some(mesh_desc) = descriptor.mesh.as_ref() {
        // No `animations` block ⇒ stateless mesh (model handle only). Otherwise
        // copy the declared state map in via `MeshAnimation::new`: current =
        // default state, entry stamp pending (filled by the resolve pass), no
        // active fade. Parse-time validation guarantees `default_state`
        // is `Some` exactly when the map is non-empty and names a declared state.
        let component = match &mesh_desc.default_state {
            Some(default_state) => MeshComponent {
                model: mesh_desc.model.clone(),
                animation: Some(MeshAnimation::new(
                    mesh_desc.animations.clone(),
                    default_state.clone(),
                )),
            },
            None => MeshComponent::stateless(mesh_desc.model.clone()),
        };
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Mesh);
    }

    if let Some(health_desc) = descriptor.health.as_ref() {
        let component = HealthComponent::from_descriptor(health_desc);
        let _ = registry.set_component(id, component);
        owned_components.insert(DescriptorComponentKind::Health);
    }

    // An `ai` descriptor materializes the engine-owned brain AND a movable
    // navigation agent (the FSM drives the agent each tick). The agent's capsule
    // is seeded from the navmesh's baked `NavAgentParams` (passed down from the
    // attach call site — never read inside the component). When the map has no
    // navmesh (`agent_params == None`), the capsule falls back to an engine
    // default and the agent simply cannot path. Move speed comes from the `ai`
    // descriptor. After both components land, the brain's logical-state →
    // animation-state map is validated against the entity's mesh (cross-component:
    // the ai block could not see the mesh at its own parse).
    if let Some(ai_desc) = descriptor.ai.as_ref() {
        // `set_component`/`attach_*` only fail on a stale id — just returned.
        let _ = attach_brain(registry, id, ai_desc);

        let params = agent_params.unwrap_or(DEFAULT_AGENT_PARAMS);
        let _ = attach_agent(registry, id, &params, ai_desc.move_speed);

        // Warn-once per undeclared animation-state name; the FSM keeps the prior
        // animation for those logical states. Called here for its spawn-time
        // validation side effect — the return value (unmapped logical states) is
        // not consumed; the FSM handles `UnknownState` at tick time.
        let _ = validate_brain_animation_states(registry, id);
    }

    if attach_weapon {
        if let Some(weapon_desc) = descriptor.weapon.as_ref() {
            let component = WeaponComponent::from_descriptor(weapon_desc);
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

pub(super) fn spawn_descriptor_instance(
    registry: &mut EntityRegistry,
    descriptor: &EntityTypeDescriptor,
    entity: &MapEntity,
    attach_weapon: bool,
    spawn_path: DescriptorSpawnPath,
    agent_params: Option<NavAgentParams>,
) -> Option<EntityId> {
    let transform = Transform {
        position: entity.origin,
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
            false,
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
/// materialization helper as the data-archetype sweep, while `defaultWeapon`
/// spawns a sibling weapon instance only when the target descriptor declares a
/// weapon component. The per-placement KVP bag is forwarded with
/// `entity_class` stripped so it is not confused with an `initial_*` override.
/// Tags from the `player_spawn` placement are passed directly to `try_spawn`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlayerSpawnResult {
    pub(crate) spawned: usize,
    pub(crate) active_wieldable: Option<EntityId>,
    pub(crate) active_wieldable_descriptor: Option<String>,
}

pub(crate) fn spawn_from_player_starts(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> PlayerSpawnResult {
    let mut spawned = 0usize;
    let mut active_wieldable = None;
    let mut active_wieldable_descriptor = None;

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

        if registry.local_player_pawn().is_none()
            && matches!(
                registry.has_component_kind(id, ComponentKind::PlayerMovement),
                Ok(true)
            )
        {
            let _ = registry.mark_local_player_pawn(id);
        }

        // Forward the per-placement KVP bag (sans `entity_class`, which is a
        // routing hint, not a runtime property) so `getEntityProperty` works
        // uniformly for player-start-spawned entities.
        let mut kvps = entity.key_values.clone();
        kvps.remove("entity_class");
        let _ = registry.set_map_kvps(id, kvps);

        if let Some(default_weapon) = descriptor.default_weapon.as_deref() {
            let Some(weapon_descriptor) = find_descriptor(descriptors, default_weapon) else {
                log::warn!(
                    "[Loader] {origin}: defaultWeapon `{default_weapon}` not registered; player spawned unarmed",
                    origin = entity.diagnostic_origin(),
                );
                spawned += 1;
                continue;
            };
            if weapon_descriptor.weapon.is_none() {
                log::warn!(
                    "[Loader] {origin}: defaultWeapon `{default_weapon}` has no weapon component; player spawned unarmed",
                    origin = entity.diagnostic_origin(),
                );
                spawned += 1;
                continue;
            }

            let weapon_entity = MapEntity {
                classname: default_weapon.to_string(),
                origin: entity.origin,
                angles: entity.angles,
                key_values: Default::default(),
                tags: vec![],
            };
            match spawn_descriptor_instance(
                registry,
                weapon_descriptor,
                &weapon_entity,
                true,
                DescriptorSpawnPath::DefaultWeapon,
                // A wieldable instance never carries an `ai` block, so no agent.
                None,
            ) {
                Some(weapon_id) => {
                    let _ = registry.set_map_kvps(weapon_id, Default::default());
                    if active_wieldable.is_none() {
                        active_wieldable = Some(weapon_id);
                        active_wieldable_descriptor = Some(default_weapon.to_string());
                    }
                }
                None => {
                    log::warn!(
                        "[Loader] {origin}: entity registry exhausted; dropping defaultWeapon `{default_weapon}`",
                        origin = entity.diagnostic_origin(),
                    );
                }
            }
        }

        spawned += 1;
    }

    if !spawn_points.is_empty() {
        log::info!(
            "[Loader] spawned {spawned} player(s) from {total} player_spawn entries",
            total = spawn_points.len(),
        );
    }

    PlayerSpawnResult {
        spawned,
        active_wieldable,
        active_wieldable_descriptor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, FallParams, FireMode, GroundParams, PlayerMovementDescriptor,
        ResolutionMode, SpeedParams, WeaponDescriptor,
    };

    // Shared descriptor/placement builders live in the sibling fixture module so
    // the netcode agreement test can reuse them without a private-helper reach
    // or a duplicate copy. See testing_guide.md §4.
    use super::super::data_archetype_test_fixtures::{
        ai_enemy_descriptor, mesh_descriptor, placement,
    };

    fn light_descriptor(classname: &str, is_dynamic: bool) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: None,
            light: Some(LightDescriptor {
                color: [0.5, 0.5, 0.5],
                intensity: 1.0,
                range: 8.0,
                is_dynamic,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Mesh)
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
    fn descriptor_spawn_attaches_animated_mesh_with_default_state() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![mesh_descriptor("decraniated_mob", true)];
        let placements = vec![placement("decraniated_mob", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Mesh)
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
    fn descriptor_animated_mesh_exposes_model_to_distinct_model_sweep() {
        // The level-load model sweep (`distinct_mesh_models` in main.rs) keys off
        // the mesh component's `model` field via `ComponentKind::Mesh` iteration.
        // Guard the same contract from the registry side for a descriptor-spawned
        // animated mesh.
        use crate::scripting::registry::{ComponentKind, ComponentValue};

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
        use crate::scripting::components::health::HealthComponent;
        use crate::scripting::data_descriptors::{HealthDescriptor, HitboxDescriptor};

        let mut reg = EntityRegistry::new();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("target_dummy".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: Some(HealthDescriptor {
                max: 75.0,
                hitbox: Some(HitboxDescriptor {
                    half_extents: [0.5, 1.0, 0.5],
                    offset: None,
                }),
                zone_multipliers: std::collections::HashMap::new(),
            }),
            ai: None,
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Health)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::DescriptorProvenance)
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
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Weapon)
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
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Weapon)
                .next()
                .is_none(),
            "direct map placement must not attach weapon components even when another component makes the descriptor placeable",
        );

        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .expect("placeable sibling component still spawns");
        assert!(reg.get_component::<LightComponent>(id).unwrap().is_dynamic);
        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
        assert!(provenance.owns(DescriptorComponentKind::Light));
        assert!(!provenance.owns(DescriptorComponentKind::Weapon));
    }

    #[test]
    fn descriptor_spawn_forces_dynamic_when_descriptor_was_baked() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![light_descriptor("torch", false)];
        let placements = vec![placement("torch", &[])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            default_weapon: None,
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
            mesh: None,
            health: None,
            ai: None,
        }];
        let placements = vec![placement(
            "campfire",
            &[("initial_velocity", "1.0 2.0 3.0")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
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
            default_weapon: None,
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
            mesh: None,
            health: None,
            ai: None,
        }];
        let placements = vec![placement("campfire", &[("velocity", "9.0 9.0 9.0")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
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
            default_weapon: None,
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
            mesh: None,
            health: None,
            ai: None,
        }];
        let placements = vec![placement("campfire", &[("initial_rate", "20.5")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
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
            default_weapon: None,
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
            mesh: None,
            health: None,
            ai: None,
        }];
        let placements = vec![placement("burstfire", &[("initial_burst", "24")])];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
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
            default_weapon: None,
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
            mesh: None,
            health: None,
            ai: None,
        }];
        let placements = vec![placement(
            "smolder",
            &[("initial_rate", "not-a-float"), ("initial_burst", "noisy")],
        )];
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        let (id, _) = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::BillboardEmitter)
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
        use crate::scripting::registry::ComponentKind;

        // Built-in dispatch already covers `billboard_emitter`.
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        // Register a data-archetype descriptor for the same classname.
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("billboard_emitter".to_string()),
            default_weapon: None,
            light: Some(LightDescriptor {
                color: [1.0, 0.0, 0.0],
                intensity: 5.0,
                range: 10.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
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
            default_weapon: None,
            light: Some(LightDescriptor {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: 5.0,
                is_dynamic: true,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
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
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Light)
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

        let result = spawn_from_player_starts(&points, &descriptors, &mut reg, None);

        let player_id = reg
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .map(|(id, _)| id)
            .find(|id| Some(*id) != result.active_wieldable)
            .expect("player entity should spawn");
        let player_provenance = reg
            .get_component::<DescriptorProvenance>(player_id)
            .expect("player provenance should be recorded");
        assert_eq!(
            player_provenance.spawn_path,
            DescriptorSpawnPath::PlayerSpawn
        );
        assert_eq!(player_provenance.canonical_name, "player");

        let weapon_id = result
            .active_wieldable
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
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
        }
    }

    fn weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 12.0,
                range: 64.0,
                cooldown_ms: 180.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
            }),
            mesh: None,
            health: None,
            ai: None,
        }
    }

    fn player_with_default_weapon(classname: &str, default_weapon: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: Some(default_weapon.to_string()),
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
        }
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
            default_weapon: None,
            light: None,
            emitter: None,
            movement: Some(movement_descriptor()),
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
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

    fn live_count(reg: &EntityRegistry) -> usize {
        reg.iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
            .count()
    }

    #[test]
    fn single_spawn_point_spawns_one_entity_at_position_and_facing() {
        use crate::scripting::conv::EulerDegrees;

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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
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
        let weapon_id = result.active_wieldable.expect("active wieldable");
        assert_eq!(
            result.active_wieldable_descriptor.as_deref(),
            Some("reference_pistol")
        );
        let weapon = reg.get_component::<WeaponComponent>(weapon_id).unwrap();
        assert_eq!(weapon.damage, 12.0);
        assert_eq!(live_count(&reg), 2, "player plus sibling weapon entity");
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
        assert!(result.active_wieldable.is_none());
        assert!(result.active_wieldable_descriptor.is_none());
        assert_eq!(
            live_count(&reg),
            1,
            "player spawned without a weapon entity"
        );
        assert!(
            reg.iter_with_kind(crate::scripting::registry::ComponentKind::Weapon)
                .next()
                .is_none(),
            "non-weapon defaultWeapon target must not produce an active no-op entity",
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Transform)
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
            .iter_with_kind(crate::scripting::registry::ComponentKind::Light)
            .next()
            .expect("descriptor light should attach to spawn-point entity");
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.is_dynamic);
        assert_eq!(light.origin, [7.0, 0.0, 0.0]);
    }

    // ---- E10 Task 5: connected-client AI-enemy spawn suppression ----

    #[test]
    fn descriptor_materializes_ai_enemy_keys_on_ai_block() {
        // An `ai` block is the sole AI classifier; light/mesh/health-only
        // descriptors are non-AI props.
        assert!(descriptor_materializes_ai_enemy(&ai_enemy_descriptor(
            "grunt"
        )));
        assert!(!descriptor_materializes_ai_enemy(&mesh_descriptor(
            "prop", false
        )));
        assert!(!descriptor_materializes_ai_enemy(&light_descriptor(
            "torch", true
        )));
    }

    #[test]
    fn client_filter_drops_ai_enemy_placements_keeps_props() {
        // The connected-client pre-dispatch filter drops AI-enemy placements and
        // keeps non-AI props in the same map.
        let descriptors = vec![
            ai_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![
            placement("grunt", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
        ];

        let kept = filter_out_client_ai_enemies(&placements, &descriptors);

        assert_eq!(kept.len(), 1, "both grunt placements dropped, crate kept");
        assert_eq!(kept[0].classname, "crate");
    }

    #[test]
    fn suppressed_ai_enemy_mesh_models_collects_filtered_enemy_models_for_upload() {
        // Regression (E10 AC #3): a connected client filters AI-enemy placements
        // out before dispatch, so their model is absent from the registry-driven
        // upload set; the host-replicated remote enemy then has no uploaded mesh
        // and renders only a debug capsule. This pins the seam that feeds the
        // suppressed enemies' models into the level-load upload union: the
        // map-referenced AI enemy's model is collected; non-AI props and
        // unknown classnames contribute nothing.
        let descriptors = vec![
            ai_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![
            placement("grunt", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
            placement("mystery", &[]),
        ];

        let models = suppressed_ai_enemy_mesh_models(&placements, &descriptors);

        // Only the AI enemy's mesh model, deduped across both grunt placements;
        // the non-AI crate and the unknown classname add nothing.
        assert_eq!(models, vec!["decraniated".to_string()]);
    }

    #[test]
    fn suppressed_ai_enemy_mesh_models_empty_without_ai_placements() {
        // No map-referenced AI enemy ⇒ nothing to pre-upload (the single-player /
        // listen-host case where the registry sweep already covers every mesh).
        let descriptors = vec![mesh_descriptor("crate", false)];
        let placements = vec![placement("crate", &[]), placement("mystery", &[])];

        assert!(suppressed_ai_enemy_mesh_models(&placements, &descriptors).is_empty());
    }

    #[test]
    fn client_filter_retains_unknown_classname_placements() {
        // A placement with no descriptor match is not an AI enemy; the filter
        // retains it so the dispatch's own unknown-classname diagnostics fire.
        let descriptors = vec![ai_enemy_descriptor("grunt")];
        let placements = vec![placement("mystery", &[]), placement("grunt", &[])];

        let kept = filter_out_client_ai_enemies(&placements, &descriptors);

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
            ai_enemy_descriptor("grunt"),
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

        // Connected client: filter AI enemies before dispatch.
        let mut client_reg = EntityRegistry::new();
        let client_placements = filter_out_client_ai_enemies(&placements, &descriptors);
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
    fn ai_descriptor_materialization_yields_live_brain_and_agent() {
        // Invariant the suppression mechanism rests on: an `ai` descriptor
        // materialization attaches BOTH live `Brain` and `Agent` columns. Without
        // this, `is_networked_ai_map_enemy` (which reads those live columns) and
        // the pre-materialization `descriptor_materializes_ai_enemy` could
        // disagree.
        let descriptors = vec![ai_enemy_descriptor("grunt")];
        let placements = vec![placement("grunt", &[])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("ai descriptor materializes a Brain");
        assert!(
            matches!(reg.has_component_kind(id, ComponentKind::Agent), Ok(true)),
            "ai descriptor materializes an Agent alongside the Brain"
        );
    }
}
