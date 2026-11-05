// Damaged-enemy overlay: dispatch-driven roster edges, per-entity fact
// sampling (health and the authored shield pair) and hide-at-full.
// See: context/lib/scripting.md (Impact-policy composition and evaluation) · context/lib/ui.md

use super::*;

/// One dispatch-driven damaged-enemy refresh, with the original host script
/// clock captured before deferred post-tick roster selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamagedEnemyOverlayDamage {
    pub entity: EntityId,
    pub source: Option<EntityId>,
    pub damaged_at: f64,
}

/// One authoritative fact sample from Task 6's post-tick overlay stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamagedEnemyOverlayFact {
    pub entity: EntityId,
    pub health_fraction: f32,
    pub shield_fraction: f32,
    pub has_shield: bool,
    pub alive: bool,
}

/// Damage edges and sampled facts from the local single-player overlay update.
/// Hosted selection consumes damage synchronously and samples its roster separately.
#[derive(Debug, Default)]
pub struct DamagedEnemyOverlayFrame {
    pub damage: Vec<DamagedEnemyOverlayDamage>,
    pub facts: Vec<DamagedEnemyOverlayFact>,
}

pub(super) struct BoundPresentationOverlay {
    pub(super) template: PresentationTemplateHandle,
    pub(super) world_anchor: PresentationWorldAnchor,
    pub(super) max_visible: usize,
    pub(super) linger_seconds: f64,
    pub(super) hide_at_full: bool,
    pub(super) scope: OverlayStateScope,
    pub(super) shield: Option<BoundOverlayShield>,
}

/// The authored portion of the damaged-enemy overlay lifecycle that a
/// connected client needs to render host-pushed facts. Values themselves stay
/// on the presentation channel; this carries no registry-derived combat state.
#[derive(Clone)]
pub struct ClientOverlayConfig {
    pub template: PresentationTemplateHandle,
    pub world_anchor: PresentationWorldAnchor,
    pub max_visible: usize,
    pub linger_seconds: f64,
    pub hide_at_full: bool,
}

pub(super) struct BoundOverlayShield {
    pub(super) value: BoundProgram<OverlayStateScope>,
    pub(super) max: BoundProgram<OverlayStateScope>,
}

/// Read-only per-entity state scope used by `damagedEnemies.shield`. Keeping
/// it local to the presentation adopter prevents overlay expressions from
/// accidentally acquiring impact facts, store reads, or write handles.
#[derive(Default)]
pub(super) struct OverlayStateScope {
    names: RefCell<Vec<String>>,
    values: RefCell<Vec<f32>>,
}

impl OverlayStateScope {
    pub(super) fn seed(&self, registry: &EntityRegistry, entity: EntityId) {
        let state = registry
            .get_component::<postretro_entities::EntityStateComponent>(entity)
            .ok();
        let names = self.names.borrow();
        let mut values = self.values.borrow_mut();
        for (index, name) in names.iter().enumerate() {
            values[index] = state.map_or(0.0, |state| state.get(name));
        }
    }
}

impl BindingScope for OverlayStateScope {
    type InputHandle = usize;
    type OutputHandle = ();

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>> {
        let state_name = name.strip_prefix("@state.")?;
        if state_name.is_empty() {
            return None;
        }
        let mut names = self.names.borrow_mut();
        let index = names
            .iter()
            .position(|bound| bound == state_name)
            .unwrap_or_else(|| {
                let index = names.len();
                names.push(state_name.to_owned());
                self.values.borrow_mut().push(0.0);
                index
            });
        Some(ResolvedInput {
            handle: index,
            ir_type: IrType::Number,
        })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<Self::OutputHandle>> {
        None
    }

    fn read(&self, handle: &Self::InputHandle) -> IrValue {
        IrValue::Number(self.values.borrow().get(*handle).copied().unwrap_or(0.0))
    }

    fn write(&mut self, _handle: &Self::OutputHandle, _value: IrValue) {}
}

impl ImpactPolicyRuntime {
    pub(super) fn retain_local_overlay_damage(
        &mut self,
        registry: &EntityRegistry,
        damage: DamagedEnemyOverlayDamage,
    ) {
        let Some(binding) = self.presentation_overlays.first() else {
            return;
        };
        let is_alive = |entity| {
            registry
                .get_component::<postretro_entities::components::health::HealthComponent>(entity)
                .is_ok_and(|health| health.current > 0.0)
        };
        self.pending_overlay_damage
            .retain(|pending| is_alive(pending.entity));
        if !is_alive(damage.entity) || !damage.damaged_at.is_finite() {
            return;
        }
        if let Some(pending) = self
            .pending_overlay_damage
            .iter_mut()
            .find(|pending| pending.entity == damage.entity)
        {
            if damage.damaged_at >= pending.damaged_at {
                *pending = damage;
            }
            return;
        }
        if self.pending_overlay_damage.len() < binding.max_visible {
            self.pending_overlay_damage.push(damage);
            return;
        }
        let Some((least_recent_index, least_recent)) = self
            .pending_overlay_damage
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (left.damaged_at, left.entity.to_raw())
                    .partial_cmp(&(right.damaged_at, right.entity.to_raw()))
                    .expect("retained overlay damage times are finite")
            })
        else {
            return;
        };
        if (damage.damaged_at, damage.entity.to_raw())
            > (least_recent.damaged_at, least_recent.entity.to_raw())
        {
            self.pending_overlay_damage[least_recent_index] = damage;
        }
    }

    /// Transfer damage edges without selecting targets or reading presentation
    /// state. The authoritative roster owns their subsequent lifetime.
    pub fn take_damaged_enemy_overlay_damage(&mut self) -> Vec<DamagedEnemyOverlayDamage> {
        std::mem::take(&mut self.pending_overlay_damage)
    }

    /// Sample only the caller's chosen targets, once per distinct ID. This
    /// performs no source selection and never reads or mutates a draw pool.
    pub fn sample_damaged_enemy_overlay_facts(
        &mut self,
        registry: &EntityRegistry,
        entities: impl IntoIterator<Item = EntityId>,
    ) -> Vec<DamagedEnemyOverlayFact> {
        let Some(binding) = self.presentation_overlays.first() else {
            return Vec::new();
        };
        self.overlay_entity_scratch.clear();
        self.overlay_entity_scratch.extend(entities);
        self.overlay_entity_scratch
            .sort_by_key(|entity| entity.to_raw());
        self.overlay_entity_scratch.dedup();
        self.overlay_entity_scratch
            .iter()
            .map(|&entity| sample_overlay_fact(binding, registry, entity))
            .collect()
    }

    /// Consume dispatch-driven refreshes, then stamp only already-tracked
    /// overlays once after all fixed ticks. This keeps the damage event edge
    /// separate from the bounded per-frame health/shield read.
    pub fn update_damaged_enemy_overlays(
        &mut self,
        pool: &mut crate::presentation_pool::PresentationPool,
        registry: &EntityRegistry,
        hit_zones: &crate::scripting_systems::hit_zones::HitZoneStore,
        anim_time: f64,
        remote_tracked_entities: impl IntoIterator<Item = EntityId>,
        is_remote_source: impl Fn(Option<EntityId>) -> bool,
    ) -> DamagedEnemyOverlayFrame {
        let mut frame = DamagedEnemyOverlayFrame {
            damage: self.take_damaged_enemy_overlay_damage(),
            facts: Vec::new(),
        };
        let Some(binding) = self.presentation_overlays.first() else {
            return frame;
        };

        for damage in &frame.damage {
            if !is_remote_source(damage.source) {
                pool.refresh_overlay(
                    crate::presentation_pool::OverlayKind::DamagedEnemies,
                    damage.entity,
                    binding.template.clone(),
                    binding.linger_seconds,
                    binding.max_visible,
                    u64::from(damage.entity.to_raw()),
                );
            }
        }

        frame.facts = self.sample_damaged_enemy_overlay_facts(
            registry,
            pool.tracked_overlay_ids_iter(crate::presentation_pool::OverlayKind::DamagedEnemies)
                .chain(remote_tracked_entities)
                .chain(frame.damage.iter().map(|damage| damage.entity)),
        );
        let binding = &self.presentation_overlays[0];
        for fact in &frame.facts {
            let entity = fact.entity;
            if !fact.alive {
                pool.evict_overlay(
                    crate::presentation_pool::OverlayKind::DamagedEnemies,
                    entity,
                );
                continue;
            }
            if pool.has_overlay(
                crate::presentation_pool::OverlayKind::DamagedEnemies,
                entity,
            ) {
                if let Some(anchor) = overlay_anchor(
                    registry,
                    hit_zones,
                    entity,
                    &binding.world_anchor,
                    anim_time,
                ) {
                    pool.stamp_damaged_enemy_overlay(
                        entity,
                        fact.health_fraction,
                        fact.shield_fraction,
                        fact.has_shield,
                        anchor,
                        binding.hide_at_full
                            && registry
                                .get_component::<postretro_entities::components::health::HealthComponent>(entity)
                                .is_ok_and(|health| health.current == health.max),
                    );
                } else {
                    pool.evict_overlay(
                        crate::presentation_pool::OverlayKind::DamagedEnemies,
                        entity,
                    );
                }
            }
        }
        frame
    }
}

pub(super) fn sample_overlay_fact(
    binding: &BoundPresentationOverlay,
    registry: &EntityRegistry,
    entity: EntityId,
) -> DamagedEnemyOverlayFact {
    let mut fact = DamagedEnemyOverlayFact {
        entity,
        health_fraction: 0.0,
        shield_fraction: 0.0,
        has_shield: false,
        alive: false,
    };
    let Ok(health) =
        registry.get_component::<postretro_entities::components::health::HealthComponent>(entity)
    else {
        return fact;
    };
    if health.current <= 0.0 {
        return fact;
    }
    fact.health_fraction = fraction_or_zero(health.current, health.max);
    if let Some(shield) = &binding.shield {
        binding.scope.seed(registry, entity);
        let value = number_from_ir(eval_value(&shield.value, &binding.scope));
        let max = number_from_ir(eval_value(&shield.max, &binding.scope));
        fact.has_shield = max.is_finite() && max > 0.0;
        if fact.has_shield {
            fact.shield_fraction = fraction_or_zero(value, max).max(0.0);
        }
    }
    fact.alive = true;
    fact
}

pub(super) fn bind_damaged_enemies_overlay(
    template: PresentationTemplateHandle,
    world_anchor: PresentationWorldAnchor,
    max_visible: usize,
    source: DamagedEnemiesOverlay,
) -> Result<BoundPresentationOverlay, String> {
    let scope = OverlayStateScope::default();
    let linger_seconds = f64::from(source.linger_ms) / 1_000.0;
    let hide_at_full = source.hide_at_full;
    let shield = source
        .shield
        .map(|shield| {
            let value = bind_overlay_number(&shield.value, &scope, "shield.value")?;
            let max = bind_overlay_number(&shield.max, &scope, "shield.max")?;
            Ok::<BoundOverlayShield, String>(BoundOverlayShield { value, max })
        })
        .transpose()?;
    Ok(BoundPresentationOverlay {
        template,
        world_anchor,
        max_visible,
        linger_seconds,
        hide_at_full,
        scope,
        shield,
    })
}

pub(super) fn bind_overlay_number(
    root: &IrNode,
    scope: &OverlayStateScope,
    field: &str,
) -> Result<BoundProgram<OverlayStateScope>, String> {
    let program = bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: root.clone(),
        },
        scope,
    )
    .map_err(|error| format!("{field} is invalid: {error}"))?;
    if program.root_type != IrType::Number {
        return Err(format!("{field} must evaluate to a number"));
    }
    Ok(program)
}

pub(super) fn number_from_ir(value: IrValue) -> f32 {
    match value {
        IrValue::Number(value) if value.is_finite() => value,
        _ => 0.0,
    }
}

pub(super) fn fraction_or_zero(value: f32, max: f32) -> f32 {
    if value.is_finite() && max.is_finite() && max > 0.0 {
        let fraction = value / max;
        if fraction.is_finite() { fraction } else { 0.0 }
    } else {
        0.0
    }
}

pub(super) fn overlay_anchor(
    registry: &EntityRegistry,
    hit_zones: &crate::scripting_systems::hit_zones::HitZoneStore,
    entity: EntityId,
    anchor: &PresentationWorldAnchor,
    anim_time: f64,
) -> Option<glam::Vec3> {
    let offset = glam::Vec3::Y * anchor.offset_y;
    if let Some(socket) = anchor
        .socket
        .as_deref()
        .and_then(|socket| hit_zones.posed_socket_world(registry, entity, socket, anim_time))
    {
        return Some(socket + offset);
    }

    let transform = registry.get_component::<Transform>(entity).ok()?;
    if !transform.position.is_finite() {
        return None;
    }
    if let Ok(health) =
        registry.get_component::<postretro_entities::components::health::HealthComponent>(entity)
        && let Some(hitbox) = &health.hitbox
        && hitbox.offset.is_finite()
        && hitbox.half_extents.is_finite()
    {
        let top =
            transform.position + hitbox.offset + glam::Vec3::Y * hitbox.half_extents.y + offset;
        if top.is_finite() {
            return Some(top);
        }
    }

    let mesh = registry
        .get_component::<postretro_entities::components::mesh::MeshComponent>(entity)
        .ok()?;
    let bound = hit_zones.get_by_name(&mesh.model)?.derived_bound?;
    let model_to_world =
        crate::scripting_systems::hit_zones::model_matrix(transform, mesh.origin_offset)?;
    let local_top = glam::Vec3::new(
        (bound.min.x + bound.max.x) * 0.5,
        bound.max.y,
        (bound.min.z + bound.max.z) * 0.5,
    );
    let top = model_to_world.transform_point3(local_top) + offset;
    top.is_finite().then_some(top)
}
