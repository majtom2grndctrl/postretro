// Hot-reload refresh planner for live descriptor-authored components.

use std::collections::HashMap;

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::light::{FalloffKind, LightComponent, LightKind};
use crate::scripting::components::mesh::MeshComponent;
use crate::scripting::components::player_movement::{MovementState, PlayerMovementComponent};
use crate::scripting::components::weapon::WeaponComponent;
use crate::scripting::data_descriptors::{EntityTypeDescriptor, LightDescriptor};
use crate::scripting::provenance::{
    DescriptorComponentKind, DescriptorMapOverride, DescriptorProvenance, parse_bool, parse_f32,
    parse_nonneg_f32, parse_vec3,
};
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, RegistryError, Transform,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DescriptorRefreshPlan {
    pub(crate) actions: Vec<DescriptorRefreshAction>,
    pub(crate) diagnostics: Vec<DescriptorRefreshDiagnostic>,
}

impl DescriptorRefreshPlan {
    fn new() -> Self {
        Self {
            actions: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn diagnostic(
        &mut self,
        entity: EntityId,
        descriptor: &str,
        component: Option<DescriptorComponentKind>,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(DescriptorRefreshDiagnostic {
            entity,
            descriptor: descriptor.to_string(),
            component,
            message: message.into(),
        });
    }

    fn keep_old(
        &mut self,
        entity: EntityId,
        descriptor: &str,
        component: DescriptorComponentKind,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        self.actions.push(DescriptorRefreshAction::KeepOld {
            entity,
            component,
            reason: reason.clone(),
        });
        self.diagnostic(entity, descriptor, Some(component), reason);
    }
}

// These actions are built and drained in small batches during descriptor
// hot-reload, not stored in bulk, so boxing `Replace` (which carries a full
// `ComponentValue`) to satisfy the size heuristic would add heap indirection
// for no real benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DescriptorRefreshAction {
    Replace {
        entity: EntityId,
        component: ComponentValue,
    },
    Remove {
        entity: EntityId,
        kind: ComponentKind,
    },
    KeepOld {
        entity: EntityId,
        component: DescriptorComponentKind,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DescriptorRefreshDiagnostic {
    pub(crate) entity: EntityId,
    pub(crate) descriptor: String,
    pub(crate) component: Option<DescriptorComponentKind>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DescriptorRefreshApplySummary {
    pub(crate) applied_actions: usize,
    pub(crate) kept_old_actions: usize,
    pub(crate) dropped_missing_targets: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum DescriptorRefreshApplyError {
    #[error("refresh action target {entity} no longer has component {kind:?}")]
    ComponentMissing {
        entity: EntityId,
        kind: ComponentKind,
    },
    #[error("unexpected registry error while applying descriptor refresh action: {0}")]
    Registry(RegistryError),
}

pub(crate) fn plan_descriptor_refresh(
    old_descriptors: &[EntityTypeDescriptor],
    new_descriptors: &[EntityTypeDescriptor],
    registry: &EntityRegistry,
) -> DescriptorRefreshPlan {
    let old_by_name = descriptor_map(old_descriptors);
    let new_by_name = descriptor_map(new_descriptors);
    let mut plan = DescriptorRefreshPlan::new();

    let provenance_rows: Vec<(EntityId, DescriptorProvenance)> = registry
        .iter_with_kind(ComponentKind::DescriptorProvenance)
        .filter_map(|(id, value)| match value {
            ComponentValue::DescriptorProvenance(p) => Some((id, p.clone())),
            _ => None,
        })
        .collect();

    for (entity, provenance) in provenance_rows {
        let name = provenance.canonical_name.as_str();
        let old_descriptor = old_by_name.get(name).copied();
        let new_descriptor = new_by_name.get(name).copied();

        if old_descriptor.is_none() {
            plan.diagnostic(
                entity,
                name,
                None,
                "source descriptor was absent from the previous snapshot",
            );
        }

        match new_descriptor {
            Some(new_descriptor) => {
                if old_descriptor.is_none() {
                    // No prior descriptor snapshot to diff against, so we cannot
                    // tell which live components this provenance row actually
                    // owned. Applying the new descriptor blind could overwrite
                    // components that were never descriptor-owned; skip the
                    // refresh and leave the live entity untouched.
                    continue;
                }
                plan_existing_descriptor(entity, &provenance, new_descriptor, registry, &mut plan);
            }
            None => {
                plan_removed_descriptor(entity, &provenance, registry, &mut plan);
            }
        }
    }

    plan
}

pub(crate) fn apply_descriptor_refresh_plan(
    plan: &DescriptorRefreshPlan,
    registry: &mut EntityRegistry,
) -> Result<DescriptorRefreshApplySummary, DescriptorRefreshApplyError> {
    preflight_descriptor_refresh_plan(plan, registry)?;

    let mut summary = DescriptorRefreshApplySummary::default();
    for action in &plan.actions {
        match action {
            DescriptorRefreshAction::KeepOld { .. } => {
                summary.kept_old_actions += 1;
            }
            DescriptorRefreshAction::Replace { entity, component } => {
                if !registry.exists(*entity) {
                    log::warn!(
                        "[Scripting] dropping descriptor refresh action for missing entity {entity}",
                    );
                    summary.dropped_missing_targets += 1;
                    continue;
                }
                registry
                    .set_component_value(*entity, component.clone())
                    .map_err(DescriptorRefreshApplyError::Registry)?;
                summary.applied_actions += 1;
            }
            DescriptorRefreshAction::Remove { entity, kind } => {
                if !registry.exists(*entity) {
                    log::warn!(
                        "[Scripting] dropping descriptor refresh action for missing entity {entity}",
                    );
                    summary.dropped_missing_targets += 1;
                    continue;
                }
                registry
                    .remove_component_kind(*entity, *kind)
                    .map_err(|err| match err {
                        RegistryError::ComponentNotFound { .. } => {
                            DescriptorRefreshApplyError::ComponentMissing {
                                entity: *entity,
                                kind: *kind,
                            }
                        }
                        other => DescriptorRefreshApplyError::Registry(other),
                    })?;
                summary.applied_actions += 1;
            }
        }
    }

    Ok(summary)
}

fn preflight_descriptor_refresh_plan(
    plan: &DescriptorRefreshPlan,
    registry: &EntityRegistry,
) -> Result<(), DescriptorRefreshApplyError> {
    for action in &plan.actions {
        let DescriptorRefreshAction::Remove { entity, kind } = action else {
            continue;
        };
        if !registry.exists(*entity) {
            continue;
        }
        if !registry
            .has_component_kind(*entity, *kind)
            .map_err(DescriptorRefreshApplyError::Registry)?
        {
            return Err(DescriptorRefreshApplyError::ComponentMissing {
                entity: *entity,
                kind: *kind,
            });
        }
    }
    Ok(())
}

fn descriptor_map(descriptors: &[EntityTypeDescriptor]) -> HashMap<&str, &EntityTypeDescriptor> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            descriptor
                .canonical_name
                .as_deref()
                .map(|name| (name, descriptor))
        })
        .collect()
}

fn plan_existing_descriptor(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
    plan: &mut DescriptorRefreshPlan,
) {
    for component in DescriptorComponentKind::ALL {
        let new_declares = descriptor_declares(new_descriptor, component);
        let live_exists = live_component_exists(registry, entity, component);

        if new_declares {
            if !provenance.owns(component) {
                plan.keep_old(
                    entity,
                    &provenance.canonical_name,
                    component,
                    "new descriptor declares component but provenance does not prove descriptor ownership",
                );
                continue;
            }
            if !live_exists {
                plan.keep_old(
                    entity,
                    &provenance.canonical_name,
                    component,
                    "new descriptor declares component but the live component is missing",
                );
                continue;
            }
            plan_component_replace(
                entity,
                provenance,
                new_descriptor,
                registry,
                component,
                plan,
            );
        } else if provenance.owns(component) {
            if live_exists {
                log::debug!(
                    "[Scripting] descriptor refresh will remove {:?} from entity {} for `{}`: component no longer declared and provenance proves ownership",
                    component,
                    entity,
                    provenance.canonical_name,
                );
                plan.actions.push(DescriptorRefreshAction::Remove {
                    entity,
                    kind: component.component_kind(),
                });
            }
        } else if live_exists {
            plan.diagnostic(
                entity,
                &provenance.canonical_name,
                Some(component),
                "new descriptor no longer declares component, but provenance does not prove ownership; preserving live component",
            );
        }
    }
}

fn plan_removed_descriptor(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    registry: &EntityRegistry,
    plan: &mut DescriptorRefreshPlan,
) {
    let mut removed_any = false;
    for component in DescriptorComponentKind::ALL {
        let live_exists = live_component_exists(registry, entity, component);
        if provenance.owns(component) {
            if live_exists {
                log::debug!(
                    "[Scripting] descriptor refresh will remove {:?} from entity {} for `{}`: source descriptor was removed",
                    component,
                    entity,
                    provenance.canonical_name,
                );
                plan.actions.push(DescriptorRefreshAction::Remove {
                    entity,
                    kind: component.component_kind(),
                });
                removed_any = true;
            }
        } else if live_exists {
            plan.diagnostic(
                entity,
                &provenance.canonical_name,
                Some(component),
                "descriptor was removed, but provenance does not prove component ownership; preserving live component",
            );
        }
    }

    if !removed_any && provenance.owned_components.is_empty() {
        plan.diagnostic(
            entity,
            &provenance.canonical_name,
            None,
            "descriptor was removed, but provenance recorded no descriptor-owned components",
        );
    }
}

fn descriptor_declares(
    descriptor: &EntityTypeDescriptor,
    component: DescriptorComponentKind,
) -> bool {
    match component {
        DescriptorComponentKind::Weapon => descriptor.weapon.is_some(),
        DescriptorComponentKind::Movement => descriptor.movement.is_some(),
        DescriptorComponentKind::Light => descriptor.light.is_some(),
        DescriptorComponentKind::Emitter => descriptor.emitter.is_some(),
        DescriptorComponentKind::Mesh => descriptor.mesh.is_some(),
        DescriptorComponentKind::Health => descriptor.health.is_some(),
    }
}

fn live_component_exists(
    registry: &EntityRegistry,
    entity: EntityId,
    component: DescriptorComponentKind,
) -> bool {
    match component {
        DescriptorComponentKind::Weapon => {
            registry.get_component::<WeaponComponent>(entity).is_ok()
        }
        DescriptorComponentKind::Movement => registry
            .get_component::<PlayerMovementComponent>(entity)
            .is_ok(),
        DescriptorComponentKind::Light => registry.get_component::<LightComponent>(entity).is_ok(),
        DescriptorComponentKind::Emitter => registry
            .get_component::<BillboardEmitterComponent>(entity)
            .is_ok(),
        DescriptorComponentKind::Mesh => registry.get_component::<MeshComponent>(entity).is_ok(),
        DescriptorComponentKind::Health => {
            registry.get_component::<HealthComponent>(entity).is_ok()
        }
    }
}

fn plan_component_replace(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
    component: DescriptorComponentKind,
    plan: &mut DescriptorRefreshPlan,
) {
    let result = match component {
        DescriptorComponentKind::Weapon => plan_weapon_replace(entity, new_descriptor, registry),
        DescriptorComponentKind::Movement => {
            plan_movement_replace(entity, new_descriptor, registry)
        }
        DescriptorComponentKind::Light => {
            plan_light_replace(entity, provenance, new_descriptor, registry)
        }
        DescriptorComponentKind::Emitter => {
            plan_emitter_replace(entity, provenance, new_descriptor, registry)
        }
        // Mesh component hot-reload refresh is declined: the per-entity animation
        // runtime carries live state (current state, entry stamps, in-flight
        // fades) that an in-place field copy would clobber. The planner keeps the
        // existing live component (no replace action). A dedicated mesh-refresh
        // path can land later if hot-reload of the animation surface is needed.
        DescriptorComponentKind::Mesh => Err("mesh component refresh is not supported".to_string()),
        DescriptorComponentKind::Health => plan_health_replace(entity, new_descriptor, registry),
    };

    match result {
        Ok(component) => {
            log::debug!(
                "[Scripting] descriptor refresh will replace {:?} on entity {} for `{}`: compatible refreshed descriptor",
                component.kind(),
                entity,
                provenance.canonical_name,
            );
            plan.actions
                .push(DescriptorRefreshAction::Replace { entity, component });
        }
        Err(reason) => plan.keep_old(entity, &provenance.canonical_name, component, reason),
    }
}

fn plan_weapon_replace(
    entity: EntityId,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
) -> Result<ComponentValue, String> {
    let descriptor = new_descriptor
        .weapon
        .as_ref()
        .ok_or_else(|| "new descriptor has no weapon component".to_string())?;
    let live = registry
        .get_component::<WeaponComponent>(entity)
        .map_err(|err| format!("live weapon component unavailable: {err}"))?;
    // Clone the live component and refresh in-place so `WeaponComponent` stays
    // the single source of truth for which fields are live state versus
    // authored stats; copying preserved fields here would silently reset any
    // future live-state field the component starts tracking.
    let mut refreshed = live.clone();
    refreshed.refresh_from_descriptor(descriptor);
    Ok(ComponentValue::Weapon(refreshed))
}

fn plan_health_replace(
    entity: EntityId,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
) -> Result<ComponentValue, String> {
    let descriptor = new_descriptor
        .health
        .as_ref()
        .ok_or_else(|| "new descriptor has no health component".to_string())?;
    let live = registry
        .get_component::<HealthComponent>(entity)
        .map_err(|err| format!("live health component unavailable: {err}"))?;
    // Clone the live component and refresh in-place so `HealthComponent` stays
    // the single source of truth for which fields are live state (`current`,
    // `death_handled`) versus authored data (`max`, `hitbox`). `current` clamps
    // to the new max; the death latch is preserved.
    let mut refreshed = *live;
    refreshed.refresh_from_descriptor(descriptor);
    Ok(ComponentValue::Health(refreshed))
}

fn plan_movement_replace(
    entity: EntityId,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
) -> Result<ComponentValue, String> {
    let descriptor = new_descriptor
        .movement
        .as_ref()
        .ok_or_else(|| "new descriptor has no movement component".to_string())?;
    let live = registry
        .get_component::<PlayerMovementComponent>(entity)
        .map_err(|err| format!("live movement component unavailable: {err}"))?;

    // Capsule-vs-pawn-transform compatibility: the pawn `Transform.position` is
    // the capsule center, so the capsule's lowest point sits at
    // `position.y - (half_height + radius)` (the eye/camera attaches above it
    // at `eye_height`; see entity_model.md §7b). A refreshed capsule tall enough
    // to drop that lowest point below the world-origin floor plane (y=0) would
    // embed the pawn's feet in the ground at its current standing position. The
    // movement tick cannot keep Rust the sole executor of a pawn spawned inside
    // geometry, so reject the refresh and keep the previous live component.
    // This is a runtime-state test against the live transform, not a static
    // field check — parse-time validation already bounds the capsule dimensions
    // themselves (see movement_descriptor_from_js / _from_lua).
    let position = registry
        .get_component::<Transform>(entity)
        .map_err(|err| format!("live pawn transform unavailable: {err}"))?
        .position;
    let capsule_lower_extent = descriptor.capsule.half_height + descriptor.capsule.radius;
    if position.y - capsule_lower_extent < 0.0 {
        return Err(format!(
            "refreshed capsule lower extent ({capsule_lower_extent}) exceeds pawn height above the floor ({}); capsule would embed the pawn in the ground",
            position.y
        ));
    }

    let mut refreshed = PlayerMovementComponent::from_descriptor(descriptor);
    // Preserve only live physics-integration state — velocity, the grounded flag,
    // and the airborne-tick counter. Everything descriptor-derived reseeds from
    // the new descriptor.
    refreshed.velocity = live.velocity;
    refreshed.is_grounded = live.is_grounded;
    refreshed.air_ticks = live.air_ticks;
    // Hot-reload is a descriptor swap, not a mid-game state resume: every ability
    // budget reseeds from the new descriptor rather than carrying the live (now
    // stale-against-the-new-max) count across. `air_jumps_remaining` and
    // `air_dashes_remaining` come back full from `from_descriptor`,
    // `dash_cooldown_ms` resets to 0, and `movement_state` drops to `Normal` so no
    // in-flight `Dash` survives with a refreshed budget and cleared cooldown. This
    // is what makes an authored change to `air.jumps` (or the dash budget) take
    // effect immediately on save instead of only after the next landing.
    refreshed.movement_state = MovementState::Normal;
    Ok(ComponentValue::PlayerMovement(refreshed))
}

fn plan_light_replace(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
) -> Result<ComponentValue, String> {
    let mut descriptor = new_descriptor
        .light
        .clone()
        .ok_or_else(|| "new descriptor has no light component".to_string())?;
    apply_light_overrides(entity, provenance, &mut descriptor, registry)?;
    let live = registry
        .get_component::<LightComponent>(entity)
        .map_err(|err| format!("live light component unavailable: {err}"))?;

    let mut refreshed = live.clone();
    refreshed.intensity = descriptor.intensity;
    refreshed.color = descriptor.color;
    refreshed.falloff_range = descriptor.range;
    // Descriptor-spawned lights are always runtime-dynamic: the engine updates
    // them per frame for position and animation. Baked indirect lighting (the
    // only path that produces static lights) comes from the level compiler, not
    // descriptor spawn, so an authored or overridden `is_dynamic = false` must
    // not stick on a descriptor-spawned light.
    refreshed.is_dynamic = true;
    if !matches!(refreshed.light_type, LightKind::Point) {
        refreshed.light_type = LightKind::Point;
    }
    if !matches!(refreshed.falloff_model, FalloffKind::InverseSquared) {
        refreshed.falloff_model = FalloffKind::InverseSquared;
    }
    Ok(ComponentValue::Light(refreshed))
}

fn plan_emitter_replace(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    new_descriptor: &EntityTypeDescriptor,
    registry: &EntityRegistry,
) -> Result<ComponentValue, String> {
    let mut refreshed = new_descriptor
        .emitter
        .clone()
        .ok_or_else(|| "new descriptor has no emitter component".to_string())?;
    apply_emitter_overrides(entity, provenance, &mut refreshed, registry)?;
    validate_refreshed_emitter(&refreshed)?;
    let live = registry
        .get_component::<BillboardEmitterComponent>(entity)
        .map_err(|err| format!("live emitter component unavailable: {err}"))?;

    if refreshed.sprite != live.sprite {
        return Err(format!(
            "refreshed sprite `{}` differs from live sprite `{}`; preserving live particles",
            refreshed.sprite, live.sprite
        ));
    }

    refreshed.burst = live.burst;
    if live.spin_animation.is_some() {
        refreshed.spin_animation = live.spin_animation.clone();
        refreshed.spin_rate = live.spin_rate;
    }
    Ok(ComponentValue::BillboardEmitter(refreshed))
}

fn validate_refreshed_emitter(component: &BillboardEmitterComponent) -> Result<(), String> {
    if !component.lifetime.is_finite() || component.lifetime <= 0.0 {
        return Err(format!(
            "refreshed emitter lifetime must be > 0, got {}",
            component.lifetime
        ));
    }
    if !component.rate.is_finite() || component.rate < 0.0 {
        return Err(format!(
            "refreshed emitter rate must be >= 0, got {}",
            component.rate
        ));
    }
    if !component.spread.is_finite() || component.spread < 0.0 {
        return Err(format!(
            "refreshed emitter spread must be >= 0, got {}",
            component.spread
        ));
    }
    if !component.drag.is_finite() || component.drag < 0.0 {
        return Err(format!(
            "refreshed emitter drag must be >= 0, got {}",
            component.drag
        ));
    }
    if component.size_over_lifetime.is_empty() {
        return Err("refreshed emitter size_over_lifetime must be nonempty".to_string());
    }
    if component.opacity_over_lifetime.is_empty() {
        return Err("refreshed emitter opacity_over_lifetime must be nonempty".to_string());
    }
    if component.sprite.is_empty() {
        return Err("refreshed emitter sprite must be nonempty".to_string());
    }
    if let Some(animation) = component.spin_animation.as_ref() {
        if !animation.duration.is_finite() || animation.duration <= 0.0 {
            return Err(format!(
                "refreshed emitter spin_animation.duration must be > 0, got {}",
                animation.duration
            ));
        }
        if animation.rate_curve.is_empty() {
            return Err("refreshed emitter spin_animation.rate_curve must be nonempty".to_string());
        }
    }
    Ok(())
}

fn apply_light_overrides(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    descriptor: &mut LightDescriptor,
    registry: &EntityRegistry,
) -> Result<(), String> {
    for field in provenance.overrides_for(DescriptorComponentKind::Light) {
        let raw = override_raw(entity, registry, field)?;
        match field {
            DescriptorMapOverride::LightInitialIntensity => {
                descriptor.intensity = parse_nonneg_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::LightInitialRange => {
                descriptor.range = parse_nonneg_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::LightInitialIsDynamic => {
                descriptor.is_dynamic = parse_bool(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::LightInitialColor => {
                descriptor.color = parse_vec3(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn apply_emitter_overrides(
    entity: EntityId,
    provenance: &DescriptorProvenance,
    component: &mut BillboardEmitterComponent,
    registry: &EntityRegistry,
) -> Result<(), String> {
    for field in provenance.overrides_for(DescriptorComponentKind::Emitter) {
        let raw = override_raw(entity, registry, field)?;
        match field {
            DescriptorMapOverride::EmitterInitialRate => {
                component.rate = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialSpread => {
                component.spread = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialLifetime => {
                component.lifetime = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialBuoyancy => {
                component.buoyancy = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialDrag => {
                component.drag = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialSpinRate => {
                component.spin_rate = parse_f32(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialBurst => {
                component.burst =
                    Some(raw.trim().parse::<u32>().map_err(|_| {
                        format!("map override `{}` is no longer valid", field.key())
                    })?);
            }
            DescriptorMapOverride::EmitterInitialSprite => {
                if raw.is_empty() {
                    return Err(format!("map override `{}` is no longer valid", field.key()));
                }
                component.sprite = raw;
            }
            DescriptorMapOverride::EmitterInitialColor => {
                component.color = parse_vec3(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            DescriptorMapOverride::EmitterInitialVelocity => {
                component.velocity = parse_vec3(&raw)
                    .ok_or_else(|| format!("map override `{}` is no longer valid", field.key()))?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn override_raw(
    entity: EntityId,
    registry: &EntityRegistry,
    field: DescriptorMapOverride,
) -> Result<String, String> {
    registry
        .get_map_kvp(entity, field.key())
        .map_err(|err| format!("map override `{}` unavailable: {err}", field.key()))?
        .ok_or_else(|| {
            format!(
                "map override `{}` was recorded but no longer exists",
                field.key()
            )
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use glam::Vec3;

    use super::*;
    use crate::scripting::components::billboard_emitter::SpinAnimation;
    use crate::scripting::components::particle::ParticleState;
    use crate::scripting::components::player_movement::MovementState;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, DashParams, FallParams, FireMode, GroundParams,
        PlayerMovementDescriptor, ResolutionMode, SpeedParams, WeaponDescriptor,
    };
    use crate::scripting::provenance::{DescriptorMapOverride, DescriptorSpawnPath};
    use crate::scripting::registry::Transform;

    fn weapon_descriptor(name: &str, damage: f32) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage,
                range: 100.0,
                cooldown_ms: 250.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
            }),
            mesh: None,
            health: None,
        }
    }

    fn light_descriptor(name: &str, is_dynamic: bool) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: Some(LightDescriptor {
                color: [0.1, 0.2, 0.3],
                intensity: 2.0,
                range: 8.0,
                is_dynamic,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
        }
    }

    fn emitter_component(sprite: &str, rate: f32) -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate,
            burst: Some(3),
            spread: 0.5,
            lifetime: 2.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.0,
            drag: 0.1,
            size_over_lifetime: [1.0].into(),
            opacity_over_lifetime: [1.0].into(),
            color: [1.0, 1.0, 1.0],
            sprite: sprite.to_string(),
            spin_rate: 1.0,
            spin_animation: None,
        }
    }

    fn emitter_descriptor(name: &str, sprite: &str, rate: f32) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: Some(emitter_component(sprite, rate)),
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
        }
    }

    fn movement_descriptor(name: &str, jumps: u32) -> EntityTypeDescriptor {
        movement_descriptor_with_capsule(name, jumps, 0.35, 0.9, 1.1)
    }

    fn movement_descriptor_with_capsule(
        name: &str,
        jumps: u32,
        radius: f32,
        half_height: f32,
        eye_height: f32,
    ) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: Some(PlayerMovementDescriptor {
                capsule: CapsuleParams {
                    radius,
                    half_height,
                    eye_height,
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
                    jumps,
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
            }),
            weapon: None,
            mesh: None,
            health: None,
        }
    }

    /// A movement descriptor with dash enabled and a given air-dash budget.
    /// `jumps` seeds `air.jumps` so the live `air_jumps_remaining` clamp can be
    /// kept compatible across the refresh. Used to exercise the hot-reload
    /// dash-state reset.
    fn movement_descriptor_with_dash(
        name: &str,
        jumps: u32,
        air_dashes: u32,
    ) -> EntityTypeDescriptor {
        let mut descriptor = movement_descriptor(name, jumps);
        descriptor.movement.as_mut().unwrap().dash = Some(DashParams {
            boost_speed: 12.0,
            momentum_retention: 0.5,
            steer_control: 0.5,
            dash_drag: 8.0,
            cooldown_ms: 600.0,
            air_dashes,
            preserve_vertical: false,
        });
        descriptor
    }

    fn provenance(name: &str, owned: &[DescriptorComponentKind]) -> DescriptorProvenance {
        DescriptorProvenance {
            canonical_name: name.to_string(),
            owned_components: owned.iter().copied().collect(),
            map_overrides: BTreeSet::new(),
            spawn_path: DescriptorSpawnPath::MapPlacement,
        }
    }

    fn health_descriptor(name: &str, max: f32) -> EntityTypeDescriptor {
        use crate::scripting::data_descriptors::HealthDescriptor;
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: Some(HealthDescriptor { max, hitbox: None }),
        }
    }

    #[test]
    fn health_refresh_updates_max_and_clamps_current() {
        let old = vec![health_descriptor("dummy", 100.0)];
        let new = vec![health_descriptor("dummy", 40.0)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut live = HealthComponent::from_descriptor(old[0].health.as_ref().unwrap());
        live.current = 90.0;
        registry.set_component(id, live).unwrap();
        registry
            .set_component(id, provenance("dummy", &[DescriptorComponentKind::Health]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::Health(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected health replacement");
        };
        assert_eq!(component.max, 40.0);
        assert_eq!(component.current, 40.0, "current clamps to the new max");
    }

    #[test]
    fn weapon_refresh_preserves_runtime_state() {
        let old = vec![weapon_descriptor("pistol", 10.0)];
        let new = vec![weapon_descriptor("pistol", 25.0)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut weapon = WeaponComponent::from_descriptor(old[0].weapon.as_ref().unwrap());
        weapon.cooldown_remaining_ms = 42.0;
        weapon.shoot_press_consumed = true;
        registry.set_component(id, weapon).unwrap();
        registry
            .set_component(id, provenance("pistol", &[DescriptorComponentKind::Weapon]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        assert_eq!(plan.actions.len(), 1);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::Weapon(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected weapon replacement");
        };
        assert_eq!(component.damage, 25.0);
        assert_eq!(component.cooldown_remaining_ms, 42.0);
        assert!(component.shoot_press_consumed);
    }

    #[test]
    fn light_refresh_forces_descriptor_spawned_light_dynamic() {
        let old = vec![light_descriptor("lamp", true)];
        let new = vec![light_descriptor("lamp", false)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut live = LightComponent {
            origin: [1.0, 2.0, 3.0],
            light_type: LightKind::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: 5.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            is_dynamic: true,
            animated_slot: None,
            animation: None,
        };
        live.is_dynamic = false;
        registry.set_component(id, live).unwrap();
        registry
            .set_component(id, provenance("lamp", &[DescriptorComponentKind::Light]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::Light(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected light replacement");
        };
        assert!(component.is_dynamic);
        assert_eq!(component.intensity, 2.0);
        assert_eq!(component.falloff_range, 8.0);
    }

    /// A transform whose Y places the capsule center high enough that the
    /// canonical `movement_descriptor` capsule (half_height 0.9 + radius 0.35 =
    /// 1.25) keeps the pawn's feet above the world-origin floor plane (y=0).
    fn standing_pawn_transform() -> Transform {
        Transform {
            position: Vec3::new(0.0, 2.0, 0.0),
            ..Transform::default()
        }
    }

    #[test]
    fn movement_refresh_preserves_compatible_runtime_state() {
        let old = vec![movement_descriptor("player", 2)];
        let new = vec![movement_descriptor("player", 3)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(standing_pawn_transform());
        let mut live = PlayerMovementComponent::from_descriptor(old[0].movement.as_ref().unwrap());
        live.velocity = Vec3::new(1.0, 2.0, 3.0);
        live.is_grounded = true;
        live.air_jumps_remaining = 1;
        live.air_ticks = 7;
        registry.set_component(id, live).unwrap();
        registry
            .set_component(
                id,
                provenance("player", &[DescriptorComponentKind::Movement]),
            )
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::PlayerMovement(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected movement replacement");
        };
        assert_eq!(component.air.jumps, 3);
        assert_eq!(component.velocity, Vec3::new(1.0, 2.0, 3.0));
        assert!(component.is_grounded);
        // The air-jump budget reseeds from the new descriptor (was live=1), so an
        // authored jump-count change takes effect on save, not only after landing.
        assert_eq!(component.air_jumps_remaining, 3);
        assert_eq!(component.air_ticks, 7);
    }

    #[test]
    fn movement_refresh_raises_air_jumps_to_new_descriptor_max() {
        // Regression: authoring a higher `air.jumps` and saving must make the new
        // jump budget usable immediately on hot-reload — not only after the next
        // landing, and not requiring an engine reboot. The live
        // `air_jumps_remaining` reseeds from the new descriptor's max.
        let old = vec![movement_descriptor("player", 2)];
        let new = vec![movement_descriptor("player", 5)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(standing_pawn_transform());
        let mut live = PlayerMovementComponent::from_descriptor(old[0].movement.as_ref().unwrap());
        live.is_grounded = true;
        live.air_jumps_remaining = 2; // grounded, at the OLD max
        registry.set_component(id, live).unwrap();
        registry
            .set_component(
                id,
                provenance("player", &[DescriptorComponentKind::Movement]),
            )
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::PlayerMovement(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected movement replacement");
        };
        assert_eq!(component.air.jumps, 5, "new max cap applied");
        assert_eq!(
            component.air_jumps_remaining, 5,
            "live air-jump budget reseeds to the new max on reload"
        );
    }

    #[test]
    fn movement_refresh_resets_dash_state_to_normal() {
        // Hot-reload is a descriptor swap, not a mid-game resume: an in-flight
        // `Dash` (with a spent air-dash budget and an armed cooldown) must not
        // survive the refresh, since the dash ability state reseeds from the new
        // descriptor. The component would otherwise carry a `Dash` state with a
        // refilled budget and a cleared cooldown.
        let old = vec![movement_descriptor_with_dash("player", 2, 2)];
        let new = vec![movement_descriptor_with_dash("player", 3, 3)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(standing_pawn_transform());
        let mut live = PlayerMovementComponent::from_descriptor(old[0].movement.as_ref().unwrap());
        live.velocity = Vec3::new(1.0, 2.0, 3.0);
        live.is_grounded = true;
        live.air_jumps_remaining = 1;
        live.air_ticks = 7;
        live.air_dashes_remaining = 0;
        live.dash_cooldown_ms = 500.0;
        live.movement_state = MovementState::Dash {
            elapsed_ms: 40.0,
            boost: Vec3::new(9.0, 0.0, 0.0),
        };
        registry.set_component(id, live).unwrap();
        registry
            .set_component(
                id,
                provenance("player", &[DescriptorComponentKind::Movement]),
            )
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::PlayerMovement(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected movement replacement");
        };
        // All ability budgets reseed from the new descriptor; movement_state
        // drops back to Normal.
        assert_eq!(component.movement_state, MovementState::Normal);
        assert_eq!(component.dash_cooldown_ms, 0.0);
        assert_eq!(component.air_dashes_remaining, 3);
        assert_eq!(component.air_jumps_remaining, 3);
        // Physics-integration state is still preserved across the refresh.
        assert_eq!(component.velocity, Vec3::new(1.0, 2.0, 3.0));
        assert!(component.is_grounded);
        assert_eq!(component.air_ticks, 7);
    }

    #[test]
    fn movement_refresh_keeps_old_when_capsule_embeds_pawn_in_floor() {
        // Pawn is standing with its capsule center at y=1.0 — feet at the floor
        // for the old capsule (half_height 0.9 + radius 0.35 = 1.25 would already
        // be too tall, so the old capsule is sized to fit). The refreshed capsule
        // grows tall enough that its lower extent drops below the world-origin
        // floor plane at the pawn's current position, which must be rejected.
        let old = vec![movement_descriptor_with_capsule("player", 0, 0.3, 0.5, 0.4)];
        let new = vec![movement_descriptor_with_capsule("player", 0, 0.5, 1.5, 0.4)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform {
            position: Vec3::new(0.0, 1.0, 0.0),
            ..Transform::default()
        });
        let live = PlayerMovementComponent::from_descriptor(old[0].movement.as_ref().unwrap());
        registry.set_component(id, live).unwrap();
        registry
            .set_component(
                id,
                provenance("player", &[DescriptorComponentKind::Movement]),
            )
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert_eq!(plan.actions.len(), 1);
        let DescriptorRefreshAction::KeepOld { reason, .. } = &plan.actions[0] else {
            panic!("expected keep-old action, got {:?}", plan.actions[0]);
        };
        assert!(
            reason.contains("embed the pawn in the ground"),
            "unexpected keep-old reason: {reason}"
        );
        assert_eq!(plan.diagnostics.len(), 1);
    }

    #[test]
    fn emitter_refresh_preserves_live_burst_and_animation_state() {
        let old = vec![emitter_descriptor("smoke", "smoke", 5.0)];
        let new = vec![emitter_descriptor("smoke", "smoke", 12.0)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut live = old[0].emitter.clone().unwrap();
        live.burst = None;
        live.spin_rate = 4.0;
        live.spin_animation = Some(SpinAnimation {
            duration: 2.0,
            rate_curve: vec![0.0, 4.0],
        });
        registry.set_component(id, live.clone()).unwrap();
        registry
            .set_component(id, provenance("smoke", &[DescriptorComponentKind::Emitter]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
        let DescriptorRefreshAction::Replace {
            component: ComponentValue::BillboardEmitter(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected emitter replacement");
        };
        assert_eq!(component.rate, 12.0);
        assert_eq!(component.burst, None);
        assert_eq!(component.spin_rate, 4.0);
        assert_eq!(component.spin_animation, live.spin_animation);
    }

    #[test]
    fn emitter_refresh_preserves_live_particles() {
        let old = vec![emitter_descriptor("smoke", "smoke", 5.0)];
        let new = vec![emitter_descriptor("smoke", "smoke", 12.0)];
        let mut registry = EntityRegistry::new();
        let emitter_id = registry.spawn(Transform::default());
        registry
            .set_component(emitter_id, old[0].emitter.clone().unwrap())
            .unwrap();
        registry
            .set_component(
                emitter_id,
                provenance("smoke", &[DescriptorComponentKind::Emitter]),
            )
            .unwrap();
        let particle_id = registry.spawn(Transform::default());
        let particle = ParticleState {
            velocity: [0.1, 0.2, 0.3],
            age: 0.4,
            lifetime: 2.0,
            buoyancy: 0.0,
            drag: 0.1,
            size_curve: [1.0, 0.5].into(),
            opacity_curve: [1.0, 0.0].into(),
            emitter: Some(emitter_id),
        };
        registry
            .set_component(particle_id, particle.clone())
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);
        let summary = apply_descriptor_refresh_plan(&plan, &mut registry).unwrap();

        assert_eq!(summary.applied_actions, 1);
        assert_eq!(
            registry
                .get_component::<BillboardEmitterComponent>(emitter_id)
                .unwrap()
                .rate,
            12.0,
        );
        assert_eq!(
            registry
                .get_component::<ParticleState>(particle_id)
                .unwrap(),
            &particle,
            "refreshing an emitter component must not remove or rewrite live particle entities",
        );
    }

    #[test]
    fn descriptor_component_removal_requires_provenance_ownership() {
        let old = vec![emitter_descriptor("smoke", "smoke", 5.0)];
        let mut new = old[0].clone();
        new.emitter = None;
        let new = vec![new];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, old[0].emitter.clone().unwrap())
            .unwrap();
        registry
            .set_component(id, provenance("smoke", &[DescriptorComponentKind::Emitter]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert_eq!(
            plan.actions,
            vec![DescriptorRefreshAction::Remove {
                entity: id,
                kind: ComponentKind::BillboardEmitter,
            }]
        );
    }

    #[test]
    fn component_without_ownership_is_not_removed() {
        let old = vec![emitter_descriptor("smoke", "smoke", 5.0)];
        let mut new = old[0].clone();
        new.emitter = None;
        let new = vec![new];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, old[0].emitter.clone().unwrap())
            .unwrap();
        registry
            .set_component(id, provenance("smoke", &[]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert!(plan.actions.is_empty());
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(
            plan.diagnostics[0]
                .message
                .contains("does not prove ownership")
        );
    }

    #[test]
    fn light_refresh_reapplies_recorded_map_override() {
        let old = vec![light_descriptor("lamp", true)];
        let new = vec![light_descriptor("lamp", true)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_map_kvps(
                id,
                HashMap::from([("initial_intensity".to_string(), "9.0".to_string())]),
            )
            .unwrap();
        registry
            .set_component(
                id,
                LightComponent {
                    origin: [0.0, 0.0, 0.0],
                    light_type: LightKind::Point,
                    intensity: 9.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: FalloffKind::InverseSquared,
                    falloff_range: 5.0,
                    cone_angle_inner: None,
                    cone_angle_outer: None,
                    cone_direction: None,
                    is_dynamic: true,
                    animated_slot: None,
                    animation: None,
                },
            )
            .unwrap();
        let mut prov = provenance("lamp", &[DescriptorComponentKind::Light]);
        prov.map_overrides
            .insert(DescriptorMapOverride::LightInitialIntensity);
        registry.set_component(id, prov).unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        let DescriptorRefreshAction::Replace {
            component: ComponentValue::Light(component),
            ..
        } = &plan.actions[0]
        else {
            panic!("expected light replacement");
        };
        assert_eq!(component.intensity, 9.0);
    }

    #[test]
    fn emitter_refresh_keeps_old_when_recorded_override_no_longer_validates() {
        let old = vec![emitter_descriptor("smoke", "smoke", 5.0)];
        let new = vec![emitter_descriptor("smoke", "smoke", 12.0)];
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_map_kvps(
                id,
                HashMap::from([("initial_lifetime".to_string(), "-1.0".to_string())]),
            )
            .unwrap();
        registry
            .set_component(id, old[0].emitter.clone().unwrap())
            .unwrap();
        let mut prov = provenance("smoke", &[DescriptorComponentKind::Emitter]);
        prov.map_overrides
            .insert(DescriptorMapOverride::EmitterInitialLifetime);
        registry.set_component(id, prov).unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert_eq!(plan.actions.len(), 1);
        let DescriptorRefreshAction::KeepOld { reason, .. } = &plan.actions[0] else {
            panic!("expected keep-old action");
        };
        assert!(reason.contains("lifetime"));
        assert_eq!(plan.diagnostics.len(), 1);
    }

    #[test]
    fn apply_refresh_plan_drops_actions_for_missing_entities() {
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry.despawn(id).unwrap();
        let plan = DescriptorRefreshPlan {
            actions: vec![DescriptorRefreshAction::Replace {
                entity: id,
                component: ComponentValue::Weapon(WeaponComponent::from_descriptor(
                    weapon_descriptor("pistol", 10.0).weapon.as_ref().unwrap(),
                )),
            }],
            diagnostics: Vec::new(),
        };

        let summary = apply_descriptor_refresh_plan(&plan, &mut registry).unwrap();

        assert_eq!(summary.applied_actions, 0);
        assert_eq!(summary.dropped_missing_targets, 1);
    }

    #[test]
    fn mesh_descriptor_refresh_declines_and_keeps_live_animation_state() {
        // Regression: a hot-reload of a descriptor whose `components.mesh` would
        // refresh must decline (KeepOld) rather than clobbering the live animation
        // runtime state (current state, entry stamps, in-flight fade) with a
        // freshly-spawned default.
        use crate::scripting::components::mesh::{AnimationState, FadeSourceKind, MeshAnimation};
        use crate::scripting::data_descriptors::MeshDescriptor;
        use std::collections::HashMap;

        // Build a minimal MeshDescriptor (the authored descriptor surface).
        // `clip_index` stays `None` here — it is unresolved at descriptor parse
        // time, exactly as the real JS/Luau parsers produce.
        let descriptor_states: HashMap<String, AnimationState> = [
            (
                "idle".to_string(),
                AnimationState {
                    clip: "idle_clip".into(),
                    looping: true,
                    crossfade_ms: 150.0,
                    interrupt: crate::scripting::components::mesh::InterruptPolicy::Smooth,
                    clip_index: None,
                },
            ),
            (
                "attack".to_string(),
                AnimationState {
                    clip: "attack_clip".into(),
                    looping: false,
                    crossfade_ms: 150.0,
                    interrupt: crate::scripting::components::mesh::InterruptPolicy::Smooth,
                    clip_index: None,
                },
            ),
        ]
        .into_iter()
        .collect();

        let mesh_descriptor = MeshDescriptor {
            model: "decraniated".into(),
            animations: descriptor_states,
            default_state: Some("idle".into()),
        };

        let make_descriptor = |name: &str| EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: Some(mesh_descriptor.clone()),
            health: None,
        };

        let old = vec![make_descriptor("robot")];
        let new = vec![make_descriptor("robot")];

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());

        // Construct a live MeshComponent with non-default animation runtime state:
        // current_state has advanced to "attack", a resolved entry stamp is set,
        // and a previous-state fade is in flight. Live states carry resolved
        // clip_index values (filled at level load), unlike the descriptor states.
        let live_states: HashMap<String, AnimationState> = [
            (
                "idle".to_string(),
                AnimationState {
                    clip: "idle_clip".into(),
                    looping: true,
                    crossfade_ms: 150.0,
                    interrupt: crate::scripting::components::mesh::InterruptPolicy::Smooth,
                    clip_index: Some(0),
                },
            ),
            (
                "attack".to_string(),
                AnimationState {
                    clip: "attack_clip".into(),
                    looping: false,
                    crossfade_ms: 150.0,
                    interrupt: crate::scripting::components::mesh::InterruptPolicy::Smooth,
                    clip_index: Some(1),
                },
            ),
        ]
        .into_iter()
        .collect();
        let live_anim = MeshAnimation {
            states: live_states,
            default_state: "idle".into(),
            current_state: "attack".into(),
            entered_at: Some(42.0),
            previous_state: Some("idle".into()),
            previous_entered_at: Some(10.0),
            fade_source: FadeSourceKind::Clip,
            interrupted_outgoing: None,
        };
        let live = MeshComponent {
            model: "decraniated".into(),
            animation: Some(live_anim),
        };
        registry.set_component(id, live).unwrap();
        registry
            .set_component(id, provenance("robot", &[DescriptorComponentKind::Mesh]))
            .unwrap();

        let plan = plan_descriptor_refresh(&old, &new, &registry);

        assert_eq!(plan.actions.len(), 1);
        let DescriptorRefreshAction::KeepOld {
            entity,
            component,
            reason,
        } = &plan.actions[0]
        else {
            panic!("expected keep-old action, got {:?}", plan.actions[0]);
        };
        assert_eq!(*entity, id);
        assert_eq!(*component, DescriptorComponentKind::Mesh);
        assert!(
            reason.contains("mesh"),
            "unexpected keep-old reason: {reason}"
        );
        // The live component must be untouched: the registry still holds the
        // original non-default animation state (KeepOld records no Replace).
        let preserved = registry.get_component::<MeshComponent>(id).unwrap();
        let preserved_anim = preserved.animation.as_ref().unwrap();
        assert_eq!(preserved_anim.current_state, "attack");
        assert_eq!(preserved_anim.entered_at, Some(42.0));
        assert_eq!(preserved_anim.previous_state.as_deref(), Some("idle"));
        assert_eq!(preserved_anim.previous_entered_at, Some(10.0));
        assert_eq!(plan.diagnostics.len(), 1);
    }

    #[test]
    fn apply_refresh_plan_replaces_and_removes_components() {
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                WeaponComponent::from_descriptor(
                    weapon_descriptor("pistol", 10.0).weapon.as_ref().unwrap(),
                ),
            )
            .unwrap();
        registry
            .set_component(id, emitter_component("smoke", 5.0))
            .unwrap();
        let replacement = WeaponComponent::from_descriptor(
            weapon_descriptor("pistol", 25.0).weapon.as_ref().unwrap(),
        );
        let plan = DescriptorRefreshPlan {
            actions: vec![
                DescriptorRefreshAction::Replace {
                    entity: id,
                    component: ComponentValue::Weapon(replacement.clone()),
                },
                DescriptorRefreshAction::Remove {
                    entity: id,
                    kind: ComponentKind::BillboardEmitter,
                },
            ],
            diagnostics: Vec::new(),
        };

        let summary = apply_descriptor_refresh_plan(&plan, &mut registry).unwrap();

        assert_eq!(summary.applied_actions, 2);
        assert_eq!(
            registry.get_component::<WeaponComponent>(id).unwrap(),
            &replacement
        );
        assert!(matches!(
            registry.get_component::<BillboardEmitterComponent>(id),
            Err(RegistryError::ComponentNotFound {
                kind: ComponentKind::BillboardEmitter,
                ..
            })
        ));
    }
}
