// Runtime-owned impact-policy binding and per-fire evaluation.
// See: context/lib/scripting.md (Impact-policy composition and evaluation).

use postretro_entities::components::health::{
    DamageProducer, IMPACT_SOURCE_TOKEN, IMPACT_TARGET_TOKEN, ImpactDispatch,
};
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationFade, PresentationMotion,
    PresentationPresenter, PresentationSpawn, PresentationTemplateHandle, ScriptCtx, SlotValue,
    Transform,
};
use postretro_foundation::ir::{
    BakedIr, BindingScope, BoundProgram, CURRENT_IR_VERSION, IrNode, IrType, IrValue,
    ResolvedInput, ResolvedOutput, bind, eval_value,
};
use postretro_foundation::{ImpactEventDescriptor, validate_ascii_identifier};
use postretro_scripting_core::data_descriptors::{
    DamagedEnemiesOverlay, PresentationOverlay, PresentationOverlaySource, PresentationTemplate,
    PresentationWorldAnchor,
};
use postretro_scripting_core::ir_scopes::{EntityOutputHandle, EntityScope};
use postretro_scripting_core::store_bridge::validate_slot_value;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;

use crate::impact_effects::{ImpactEffect, apply_effect};

/// The single consumer of the health chokepoint's impact-dispatch queue.
///
/// The runtime retains raw global and level descriptors, rebuilds their bound
/// form on registration changes, and evaluates only the in-tick producer arm.
/// One [`EntityScope`] is shared by all bound policies so every operand on an
/// impact fire observes exactly one state/store/fact snapshot.
pub(crate) struct ImpactPolicyRuntime {
    ctx: ScriptCtx,
    /// Process-scoped mod identity. Impact descriptors retain their authored
    /// ids on the wire; this is only the internal composition/logging prefix.
    mod_id: Option<String>,
    global_events: Vec<ImpactEventDescriptor>,
    presentation_templates: HashMap<String, PresentationTemplate>,
    presentation_overlays: Vec<BoundPresentationOverlay>,
    /// Dispatch-driven tracking input. This is populated at the same
    /// synchronous impact evaluation point as `present`, never by taking the
    /// destructive registry dispatch queue a second time.
    pending_overlay_damage: Vec<DamagedEnemyOverlayDamage>,
    /// Reused union of host-local, remote-recipient, and just-damaged targets
    /// sampled by the post-tick overlay pass.
    overlay_entity_scratch: Vec<EntityId>,
    level_events: Vec<ImpactEventDescriptor>,
    active_level_tags: Vec<String>,
    scope: EntityScope,
    policies: Vec<BoundImpactPolicy>,
    consequential: Vec<PlannedEffect>,
    presentation: Vec<PlannedEffect>,
}

/// One dispatch-driven damaged-enemy refresh. The host presentation sender
/// resolves `source` through `MovementOwners` after Task 6 has settled the
/// frame's overlay facts; local and unowned sources intentionally remain local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DamagedEnemyOverlayDamage {
    pub(crate) entity: EntityId,
    pub(crate) source: Option<EntityId>,
}

/// One authoritative fact sample from Task 6's post-tick overlay stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DamagedEnemyOverlayFact {
    pub(crate) entity: EntityId,
    pub(crate) health_fraction: f32,
    pub(crate) shield_fraction: f32,
    pub(crate) has_shield: bool,
    pub(crate) alive: bool,
}

/// The post-tick handoff from the host overlay lifecycle to the remote
/// presentation sender. It contains no wire identity: only netcode owns the
/// `EntityId` to non-recycled `NetworkId` conversion.
#[derive(Debug, Default)]
pub(crate) struct DamagedEnemyOverlayFrame {
    pub(crate) damage: Vec<DamagedEnemyOverlayDamage>,
    pub(crate) facts: Vec<DamagedEnemyOverlayFact>,
}

struct BoundImpactPolicy {
    id: String,
    base_filter_tag: Option<String>,
    filter_tag: Option<String>,
    groups: Vec<BoundGroup>,
}

struct BoundGroup {
    when: Option<BoundProgram<EntityScope>>,
    effects: Vec<BoundEffect>,
}

struct BoundPresentationOverlay {
    template: PresentationTemplateHandle,
    world_anchor: PresentationWorldAnchor,
    max_visible: usize,
    linger_seconds: f64,
    hide_at_full: bool,
    scope: OverlayStateScope,
    shield: Option<BoundOverlayShield>,
}

/// The authored portion of the damaged-enemy overlay lifecycle that a
/// connected client needs to render host-pushed facts. Values themselves stay
/// on the presentation channel; this carries no registry-derived combat state.
#[derive(Clone)]
pub(crate) struct ClientOverlayConfig {
    pub(crate) template: PresentationTemplateHandle,
    pub(crate) world_anchor: PresentationWorldAnchor,
    pub(crate) max_visible: usize,
    pub(crate) linger_seconds: f64,
    pub(crate) hide_at_full: bool,
}

struct BoundOverlayShield {
    value: BoundProgram<OverlayStateScope>,
    max: BoundProgram<OverlayStateScope>,
}

/// Read-only per-entity state scope used by `damagedEnemies.shield`. Keeping
/// it local to the presentation adopter prevents overlay expressions from
/// accidentally acquiring impact facts, store reads, or write handles.
#[derive(Default)]
struct OverlayStateScope {
    names: RefCell<Vec<String>>,
    values: RefCell<Vec<f32>>,
}

impl OverlayStateScope {
    fn seed(&self, registry: &EntityRegistry, entity: EntityId) {
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

enum BoundEffect {
    Write(BoundProgram<EntityScope>),
    SetOwnerSlot {
        slot: String,
        value: BoundProgram<EntityScope>,
    },
    SetHealth {
        value: BoundProgram<EntityScope>,
        after_ms: Option<f32>,
    },
    Despawn {
        after_ms: Option<f32>,
    },
    PlayAnimation {
        state: String,
    },
    Present {
        template: String,
        value: BoundProgram<EntityScope>,
    },
    GrantHealth {
        amount: BoundProgram<EntityScope>,
    },
    GrantAmmo {
        pool: String,
        amount: BoundProgram<EntityScope>,
    },
}

/// Which opaque impact command-target token a planned effect resolves at apply.
///
/// Numeric IR operands remain bound to the impact target scope; this only
/// chooses the entity that receives a command after evaluation completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandRecipient {
    Target,
    Source,
}

enum PlannedEffect {
    Write {
        recipient: CommandRecipient,
        handle: EntityOutputHandle,
        value: IrValue,
    },
    Command {
        recipient: CommandRecipient,
        effect: ImpactEffect,
    },
}

impl ImpactPolicyRuntime {
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            scope: EntityScope::impact(ctx.clone()),
            ctx,
            mod_id: None,
            global_events: Vec::new(),
            presentation_templates: HashMap::new(),
            presentation_overlays: Vec::new(),
            pending_overlay_damage: Vec::new(),
            overlay_entity_scratch: Vec::new(),
            level_events: Vec::new(),
            active_level_tags: Vec::new(),
            policies: Vec::new(),
            consequential: Vec::new(),
            presentation: Vec::new(),
        }
    }

    /// Capture the first committed mod id for impact-policy composition.
    ///
    /// The scripting runtime freezes this identity across staged reloads. An
    /// absent manifest remains absent, which intentionally leaves level event
    /// ids unqualified rather than producing a bare `:` prefix.
    pub(crate) fn set_mod_id(&mut self, mod_id: Option<String>) {
        let Some(mod_id) = mod_id else {
            return;
        };
        if self.mod_id.is_none() {
            self.mod_id = Some(mod_id);
            self.rebuild();
        }
    }

    /// Replace the complete mod-scope descriptor snapshot. A staged mod-init
    /// commit has the same snapshot semantics as initial mod registration.
    pub(crate) fn replace_global_events(&mut self, events: Vec<ImpactEventDescriptor>) {
        self.global_events = events;
        self.rebuild();
    }

    /// Replace the renderer-consumed passive template snapshot committed by
    /// mod init. Invalid descriptors were already contained by the manifest
    /// drain; retaining only one template per id keeps an unexpected duplicate
    /// harmless for runtime callers as well.
    pub(crate) fn replace_presentation_templates(&mut self, templates: Vec<PresentationTemplate>) {
        self.presentation_templates.clear();
        for template in templates {
            if let Err(error) = template.validate() {
                log::warn!(
                    "[Impact] presentation template `{}` was ignored: {error}",
                    template.id
                );
                continue;
            }
            if self
                .presentation_templates
                .insert(template.id.clone(), template)
                .is_some()
            {
                log::warn!("[Impact] duplicate presentation template was replaced");
            }
        }
    }

    /// Snapshot the committed definitions for the renderer-owned UI layout
    /// cache. Template widgets are never laid out in this policy runtime.
    pub(crate) fn presentation_templates(&self) -> Vec<PresentationTemplate> {
        self.presentation_templates.values().cloned().collect()
    }

    /// Borrow the committed template registry for frame-local client ingest.
    /// Renderer installation still takes an owned snapshot only at manifest
    /// commit; ordinary net polls must not clone every widget subtree.
    pub(crate) fn presentation_template_registry(&self) -> &HashMap<String, PresentationTemplate> {
        &self.presentation_templates
    }

    /// Snapshot the client-relevant lifetime and anchor configuration for the
    /// one supported damaged-enemy overlay. The host alone evaluates health and
    /// shield state; clients only apply pushed facts through this configuration.
    pub(crate) fn client_overlay_config(&self) -> Option<ClientOverlayConfig> {
        self.presentation_overlays
            .first()
            .map(|overlay| ClientOverlayConfig {
                template: overlay.template.clone(),
                world_anchor: overlay.world_anchor.clone(),
                max_visible: overlay.max_visible,
                linger_seconds: overlay.linger_seconds,
                hide_at_full: overlay.hide_at_full,
            })
    }

    /// Replace the complete passive overlay declaration snapshot. Currently
    /// one `damagedEnemies` source owns the EntityId-keyed overlay map; later
    /// overlay source kinds can add their own disjoint key namespace instead of
    /// making two declarations fight over one target key.
    pub(crate) fn replace_presentation_overlays(&mut self, overlays: Vec<PresentationOverlay>) {
        self.presentation_overlays.clear();
        self.pending_overlay_damage.clear();
        if overlays.len() > 1 {
            log::warn!(
                "[Impact] presentation overlay snapshot contained more than one descriptor; all overlays were ignored"
            );
            return;
        }
        for overlay in overlays {
            if let Err(error) = overlay.validate() {
                log::warn!("[Impact] presentation overlay was ignored: {error}");
                continue;
            }
            let Some(template) = self.presentation_templates.get(&overlay.template) else {
                log::warn!(
                    "[Impact] presentation overlay template `{}` is not registered; overlay skipped",
                    overlay.template
                );
                continue;
            };
            let Some(world_anchor) = template.world_anchor.clone() else {
                log::warn!(
                    "[Impact] presentation overlay template `{}` has no `worldAnchor`; overlay skipped",
                    template.id
                );
                continue;
            };
            let PresentationOverlaySource::DamagedEnemies(source) = overlay.over;
            match bind_damaged_enemies_overlay(
                PresentationTemplateHandle::from(template.id.clone()),
                world_anchor,
                overlay.max_visible,
                source,
            ) {
                Ok(binding) => self.presentation_overlays.push(binding),
                Err(error) => {
                    log::warn!("[Impact] presentation overlay was skipped during bind: {error}")
                }
            }
        }
    }

    /// Replace the per-level descriptors after `setupLevel()` finishes. Global
    /// entries are intentionally retained and precede these in load order.
    pub(crate) fn replace_level_events(
        &mut self,
        events: Vec<ImpactEventDescriptor>,
        active_level_tags: &[String],
    ) {
        self.level_events = events;
        self.active_level_tags = active_level_tags.to_vec();
        self.rebuild();
    }

    pub(crate) fn clear_level_events(&mut self) {
        self.level_events.clear();
        self.active_level_tags.clear();
        self.rebuild();
    }

    /// Evaluate every dispatch currently published by one damage call while
    /// the fixed-tick producer still owns the registry. Producers call this
    /// immediately after each hit, never as a post-tick batch.
    pub(crate) fn evaluate_pending_in_registry(
        &mut self,
        registry: &mut postretro_entities::EntityRegistry,
    ) {
        let dispatches = registry.take_impact_dispatches();
        for dispatch in dispatches {
            if dispatch.producer != DamageProducer::InTick {
                continue;
            }
            self.evaluate_dispatch(registry, dispatch);
        }
    }

    /// Consume the app-drain producer arm after reaction dispatch settles.
    /// v1 intentionally runs no policy for these impacts. An in-tick dispatch
    /// reaching this sink is an ordering bug and is dropped rather than being
    /// evaluated against a stale post-tick snapshot.
    pub(crate) fn discard_app_drain_pending(&mut self) {
        let dispatches = self.ctx.registry.borrow_mut().take_impact_dispatches();
        for dispatch in dispatches {
            if dispatch.producer == DamageProducer::InTick {
                log::error!(
                    "[Impact] dropped in-tick dispatch that escaped synchronous evaluation"
                );
            }
        }
    }

    fn rebuild(&mut self) {
        let scope = EntityScope::impact(self.ctx.clone());
        let mut policies = Vec::with_capacity(self.global_events.len() + self.level_events.len());
        let mut base_filters = HashMap::<String, Option<String>>::new();
        for descriptor in self
            .global_events
            .iter()
            .filter(|descriptor| levels_match(&descriptor.levels, &self.active_level_tags))
            .chain(&self.level_events)
        {
            let id = qualified_impact_event_id(self.mod_id.as_deref(), &descriptor.id);
            let base_filter_tag = if descriptor.is_override {
                if descriptor.filter_tag.is_none() {
                    log::warn!(
                        "[Impact] override `{}` was skipped: override filter requires `tag`",
                        id
                    );
                    continue;
                }
                let Some(filter) = base_filters.get(&id).cloned() else {
                    log::warn!("[Impact] {}", unknown_override_diagnostic(&id));
                    continue;
                };
                filter
            } else {
                base_filters.insert(id.clone(), descriptor.filter_tag.clone());
                descriptor.filter_tag.clone()
            };
            match bind_policy(descriptor, &id, base_filter_tag, &scope) {
                Ok(policy) => policies.push(policy),
                Err(error) => {
                    log::warn!("[Impact] policy `{}` was skipped during bind: {error}", id)
                }
            }
        }
        self.scope = scope;
        self.policies = policies;
    }

    fn evaluate_dispatch(
        &mut self,
        registry: &mut postretro_entities::EntityRegistry,
        dispatch: ImpactDispatch,
    ) {
        if !self.presentation_overlays.is_empty() {
            self.pending_overlay_damage.push(DamagedEnemyOverlayDamage {
                entity: dispatch.target,
                source: dispatch.source,
            });
        }
        let tags = {
            let Ok(tags) = registry.get_tags(dispatch.target) else {
                return;
            };
            tags.to_vec()
        };

        if let Err(error) = self.scope.seed_impact_from_registry(registry, &dispatch) {
            log::warn!("[Impact] dispatch scope seed failed; skipping impact: {error:?}");
            return;
        }

        self.consequential.clear();
        self.presentation.clear();

        // A later matching variant replaces an earlier one with the same
        // author id. Removing then appending also makes cross-event execution
        // follow the selected descriptors' registration order.
        let mut selected: Vec<usize> = Vec::new();
        for (index, policy) in self.policies.iter().enumerate() {
            if !policy_matches(policy, &tags) {
                continue;
            }
            if let Some(previous) = selected
                .iter()
                .position(|previous| self.policies[*previous].id == policy.id)
            {
                selected.remove(previous);
            }
            selected.push(index);
        }

        // No application happens in this loop. Thus every gate and every
        // operand sees the one scope snapshot seeded above, even across
        // independent events and groups.
        for index in selected {
            let policy = &self.policies[index];
            for group in &policy.groups {
                let eligible = group.when.as_ref().is_none_or(|when| {
                    matches!(eval_value(when, &self.scope), IrValue::Bool(true))
                });
                if !eligible {
                    continue;
                }
                for effect in &group.effects {
                    let planned = plan_effect(effect, &self.scope);
                    match effect {
                        BoundEffect::PlayAnimation { .. } | BoundEffect::Present { .. } => {
                            self.presentation.push(planned)
                        }
                        BoundEffect::Write(_)
                        | BoundEffect::SetOwnerSlot { .. }
                        | BoundEffect::SetHealth { .. }
                        | BoundEffect::Despawn { .. }
                        | BoundEffect::GrantHealth { .. }
                        | BoundEffect::GrantAmmo { .. } => self.consequential.push(planned),
                    }
                }
            }
        }

        self.apply_planned(registry, &dispatch, false);
        self.apply_planned(registry, &dispatch, true);
    }

    fn apply_planned(
        &mut self,
        registry: &mut postretro_entities::EntityRegistry,
        dispatch: &ImpactDispatch,
        presentation: bool,
    ) {
        let ctx = self.ctx.clone();
        // Drain this lane before executing it: presentation application needs
        // an immutable template lookup while the lane itself is otherwise a
        // mutable borrow of `self`.
        let mut effects = if presentation {
            std::mem::take(&mut self.presentation)
        } else {
            std::mem::take(&mut self.consequential)
        };
        effects.reverse();
        while let Some(effect) = effects.pop() {
            match effect {
                PlannedEffect::Write {
                    recipient: CommandRecipient::Target,
                    handle,
                    value,
                } => self.scope.write_with_registry(registry, &handle, value),
                PlannedEffect::Write {
                    recipient: CommandRecipient::Source,
                    ..
                } => {
                    debug_assert!(
                        false,
                        "impact writes are bound to the target-scoped entity state"
                    );
                }
                PlannedEffect::Command { recipient, effect } => {
                    if let ImpactEffect::Present { template, value } = effect {
                        self.apply_presentation_spawn(registry, dispatch, &template, value);
                        continue;
                    }
                    let recipient = match recipient {
                        CommandRecipient::Target => Some(dispatch.target),
                        CommandRecipient::Source => {
                            dispatch.source.filter(|source| registry.exists(*source))
                        }
                    };
                    if let Some(recipient) = recipient {
                        match effect {
                            ImpactEffect::SetOwnerSlot { slot, value } => {
                                Self::apply_owner_slot(&ctx, registry, recipient, &slot, value);
                            }
                            effect => apply_effect(registry, recipient, &effect),
                        }
                    }
                }
            }
        }
    }

    fn apply_owner_slot(
        ctx: &ScriptCtx,
        registry: &EntityRegistry,
        recipient: postretro_entities::EntityId,
        slot: &str,
        value: f32,
    ) {
        let Some(seat) = registry.seat_for_pawn(recipient) else {
            log::warn!("[Impact] owner write for slot `{slot}` resolved no seat; skipping");
            return;
        };

        let mut table = ctx.slot_table.borrow_mut();
        let Some(record) = table.get_mut(slot) else {
            debug_assert!(false, "bound owner slot `{slot}` disappeared before apply");
            return;
        };
        let Ok(value) = validate_slot_value(slot, &record.schema, SlotValue::Number(value)) else {
            log::warn!("[Impact] owner write for slot `{slot}` failed validation; skipping");
            return;
        };
        record.set_per_seat_value(seat, value);
    }

    /// Presentation's numeric fact was frozen by `plan_effect` before any
    /// consequences applied. Only the anchor deliberately comes from the live
    /// target here: staged scripted despawn preserves Transform until the
    /// app-owned frame-end removal pass.
    fn apply_presentation_spawn(
        &self,
        registry: &mut EntityRegistry,
        dispatch: &ImpactDispatch,
        template_id: &str,
        value: f32,
    ) {
        let Some(template) = self.presentation_templates.get(template_id) else {
            log::warn!(
                "[Impact] presentation template `{template_id}` is not registered; spawn skipped"
            );
            return;
        };
        let Ok(transform) = registry.get_component::<Transform>(dispatch.target) else {
            log::warn!(
                "[Impact] target has no world transform; presentation `{template_id}` was skipped"
            );
            return;
        };

        let mut facts = postretro_entities::PresentationFacts::new();
        facts.insert("value".to_string(), PresentationFact::Number(value));
        registry.push_presentation_spawn(PresentationSpawn {
            world_anchor: transform.position,
            template: PresentationTemplateHandle::from(template.id.clone()),
            facts,
            presenter: dispatch
                .source
                .map(|source| PresentationPresenter(source.to_raw())),
            lifetime_seconds: template.lifetime_ms as f32 / 1_000.0,
            motion: PresentationMotion {
                rise_pixels: template.motion.rise,
                easing: template.motion.easing,
            },
            fade: PresentationFade {
                duration_seconds: template.lifetime_ms.saturating_sub(template.fade.start_ms)
                    as f32
                    / 1_000.0,
            },
            scatter_radius: template.spawn_scatter.radius,
        });
    }

    /// Consume dispatch-driven refreshes, then stamp only already-tracked
    /// overlays once after all fixed ticks. This keeps the damage event edge
    /// separate from the bounded per-frame health/shield read.
    pub(crate) fn update_damaged_enemy_overlays(
        &mut self,
        pool: &mut crate::presentation_pool::PresentationPool,
        registry: &EntityRegistry,
        hit_zones: &crate::scripting_systems::hit_zones::HitZoneStore,
        anim_time: f64,
        remote_tracked_entities: impl IntoIterator<Item = EntityId>,
        is_remote_source: impl Fn(Option<EntityId>) -> bool,
    ) -> DamagedEnemyOverlayFrame {
        let mut frame = DamagedEnemyOverlayFrame {
            damage: std::mem::take(&mut self.pending_overlay_damage),
            facts: Vec::new(),
        };
        let Some(binding) = self.presentation_overlays.first_mut() else {
            return frame;
        };

        for damage in &frame.damage {
            if !is_remote_source(damage.source) {
                pool.refresh_overlay(
                    damage.entity,
                    binding.template.clone(),
                    binding.linger_seconds,
                    binding.max_visible,
                    u64::from(damage.entity.to_raw()),
                );
            }
        }

        let mut tracked_entities = std::mem::take(&mut self.overlay_entity_scratch);
        tracked_entities.clear();
        tracked_entities.extend(pool.tracked_overlay_ids_iter());
        tracked_entities.extend(remote_tracked_entities);
        tracked_entities.extend(frame.damage.iter().map(|damage| damage.entity));
        tracked_entities.sort_by_key(|entity| entity.to_raw());
        tracked_entities.dedup();
        frame.facts.reserve(tracked_entities.len());

        for &entity in &tracked_entities {
            let Ok(health) = registry
                .get_component::<postretro_entities::components::health::HealthComponent>(entity)
            else {
                frame.facts.push(DamagedEnemyOverlayFact {
                    entity,
                    health_fraction: 0.0,
                    shield_fraction: 0.0,
                    has_shield: false,
                    alive: false,
                });
                pool.evict_overlay(entity);
                continue;
            };
            if health.current <= 0.0 {
                frame.facts.push(DamagedEnemyOverlayFact {
                    entity,
                    health_fraction: 0.0,
                    shield_fraction: 0.0,
                    has_shield: false,
                    alive: false,
                });
                pool.evict_overlay(entity);
                continue;
            }

            let health_fraction = fraction_or_zero(health.current, health.max);
            let (shield_fraction, has_shield) =
                binding.shield.as_ref().map_or((0.0, false), |shield| {
                    binding.scope.seed(registry, entity);
                    let value = number_from_ir(eval_value(&shield.value, &binding.scope));
                    let max = number_from_ir(eval_value(&shield.max, &binding.scope));
                    let has_shield = max.is_finite() && max > 0.0;
                    (
                        if has_shield {
                            fraction_or_zero(value, max)
                        } else {
                            0.0
                        },
                        has_shield,
                    )
                });
            if pool.has_overlay(entity) {
                if let Some(anchor) = overlay_anchor(
                    registry,
                    hit_zones,
                    entity,
                    &binding.world_anchor,
                    anim_time,
                ) {
                    pool.stamp_damaged_enemy_overlay(
                        entity,
                        health_fraction,
                        shield_fraction,
                        has_shield,
                        anchor,
                        binding.hide_at_full && health.current == health.max,
                    );
                } else {
                    pool.evict_overlay(entity);
                }
            }
            frame.facts.push(DamagedEnemyOverlayFact {
                entity,
                health_fraction,
                shield_fraction,
                has_shield,
                alive: true,
            });
        }
        self.overlay_entity_scratch = tracked_entities;
        frame
    }
}

fn bind_damaged_enemies_overlay(
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

fn bind_overlay_number(
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

fn number_from_ir(value: IrValue) -> f32 {
    match value {
        IrValue::Number(value) if value.is_finite() => value,
        _ => 0.0,
    }
}

fn fraction_or_zero(value: f32, max: f32) -> f32 {
    if value.is_finite() && max.is_finite() && max > 0.0 {
        let fraction = value / max;
        if fraction.is_finite() { fraction } else { 0.0 }
    } else {
        0.0
    }
}

fn overlay_anchor(
    registry: &EntityRegistry,
    hit_zones: &crate::scripting_systems::hit_zones::HitZoneStore,
    entity: EntityId,
    anchor: &PresentationWorldAnchor,
    anim_time: f64,
) -> Option<glam::Vec3> {
    let offset = glam::Vec3::Y * anchor.offset_y;
    if let Some(socket) = hit_zones.posed_socket_world(registry, entity, &anchor.socket, anim_time)
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

fn policy_matches(policy: &BoundImpactPolicy, tags: &[String]) -> bool {
    tag_matches(policy.base_filter_tag.as_deref(), tags)
        && tag_matches(policy.filter_tag.as_deref(), tags)
}

fn tag_matches(filter: Option<&str>, tags: &[String]) -> bool {
    filter.is_none_or(|filter| tags.iter().any(|tag| tag == filter))
}

fn levels_match(levels: &[String], active_level_tags: &[String]) -> bool {
    levels.is_empty()
        || levels
            .iter()
            .any(|level| active_level_tags.iter().any(|tag| tag == level))
}

fn unknown_override_diagnostic(id: &str) -> String {
    format!("override targets unknown event \"{id}\"")
}

fn bind_policy(
    descriptor: &ImpactEventDescriptor,
    id: &str,
    base_filter_tag: Option<String>,
    scope: &EntityScope,
) -> Result<BoundImpactPolicy, String> {
    if descriptor.is_override && descriptor.filter_tag.is_none() {
        return Err("impact override filter requires `tag`".to_string());
    }
    let mut groups = Vec::with_capacity(descriptor.policy.len());
    for entry in &descriptor.policy {
        groups.push(bind_group(entry, scope)?);
    }
    Ok(BoundImpactPolicy {
        id: id.to_string(),
        base_filter_tag,
        filter_tag: descriptor.filter_tag.clone(),
        groups,
    })
}

fn qualified_impact_event_id(mod_id: Option<&str>, authored_id: &str) -> String {
    match mod_id {
        Some(mod_id) => format!("{mod_id}:{authored_id}"),
        None => authored_id.to_string(),
    }
}

fn bind_group(entry: &Value, scope: &EntityScope) -> Result<BoundGroup, String> {
    let object = object(entry, "policy entry")?;
    if let Some(group_effects) = object.get("do") {
        let effects = group_effects
            .as_array()
            .ok_or_else(|| "impact group `do` must be an array".to_string())?
            .iter()
            .map(|effect| bind_effect(effect, scope))
            .collect::<Result<Vec<_>, _>>()?;
        let when = match object.get("when") {
            Some(value) => {
                let program = bind_read(value, scope)?;
                if program.root_type != IrType::Bool {
                    return Err("impact group `when` must evaluate to a boolean".to_string());
                }
                Some(program)
            }
            None => None,
        };
        return Ok(BoundGroup { when, effects });
    }

    Ok(BoundGroup {
        when: None,
        effects: vec![bind_effect(entry, scope)?],
    })
}

fn bind_effect(entry: &Value, scope: &EntityScope) -> Result<BoundEffect, String> {
    let effect = object(entry, "impact effect")?;
    let primitive = required_string(effect, "primitive", "impact effect")?;
    let empty_args = Map::new();
    let args = effect
        .get("args")
        .map(|value| object(value, "impact effect args"))
        .transpose()?
        .unwrap_or(&empty_args);
    let target = match effect.get("target") {
        None => None,
        Some(Value::String(target)) => Some(target.as_str()),
        Some(_) => return Err("impact effect `target` must be a string when present".to_string()),
    };

    match primitive {
        "despawn" => {
            require_impact_token(target, primitive, IMPACT_TARGET_TOKEN)?;
            Ok(BoundEffect::Despawn {
                after_ms: optional_ms(args)?,
            })
        }
        "playAnim" => {
            require_impact_token(target, primitive, IMPACT_TARGET_TOKEN)?;
            Ok(BoundEffect::PlayAnimation {
                state: required_string(args, "clip", "playAnim args")?.to_string(),
            })
        }
        "present" => {
            require_impact_token(target, primitive, IMPACT_TARGET_TOKEN)?;
            let template = required_string(args, "template", "present args")?;
            if template.is_empty() {
                return Err("present `template` must be nonempty".to_string());
            }
            let value = bind_read(
                args.get("value")
                    .ok_or_else(|| "present args is missing `value`".to_string())?,
                scope,
            )?;
            if value.root_type != IrType::Number {
                return Err("present `value` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::Present {
                template: template.to_string(),
                value,
            })
        }
        "setHealth" => {
            require_impact_token(target, primitive, IMPACT_TARGET_TOKEN)?;
            let value = bind_read(
                args.get("value")
                    .ok_or_else(|| "setHealth args is missing `value`".to_string())?,
                scope,
            )?;
            if value.root_type != IrType::Number {
                return Err("setHealth `value` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::SetHealth {
                value,
                after_ms: optional_ms(args)?,
            })
        }
        "grantHealth" => {
            require_impact_token(target, primitive, IMPACT_SOURCE_TOKEN)?;
            let amount = bind_read(
                args.get("amount")
                    .ok_or_else(|| "grantHealth args is missing `amount`".to_string())?,
                scope,
            )?;
            if amount.root_type != IrType::Number {
                return Err("grantHealth `amount` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::GrantHealth { amount })
        }
        "grantAmmo" => {
            require_impact_token(target, primitive, IMPACT_SOURCE_TOKEN)?;
            let pool = required_string(args, "type", "grantAmmo args")?;
            validate_ascii_identifier("grantAmmo.type", pool).map_err(|error| error.to_string())?;
            let amount = bind_read(
                args.get("amount")
                    .ok_or_else(|| "grantAmmo args is missing `amount`".to_string())?,
                scope,
            )?;
            if amount.root_type != IrType::Number {
                return Err("grantAmmo `amount` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::GrantAmmo {
                pool: pool.to_string(),
                amount,
            })
        }
        "setState" if target == Some("@impact.target") => {
            let name = required_string(args, "name", "target setState args")?;
            let value = args
                .get("value")
                .ok_or_else(|| "target setState args is missing `value`".to_string())?;
            bind_number_write(format!("@state.{name}"), value, scope).map(BoundEffect::Write)
        }
        "slot.set" if target.is_none() => {
            let slot = required_string(args, "slot", "slot.set args")?;
            let value = args
                .get("value")
                .ok_or_else(|| "slot.set args is missing `value`".to_string())?;
            bind_number_write(slot.to_string(), value, scope).map(BoundEffect::Write)
        }
        "slot.set" => {
            require_impact_token(target, primitive, IMPACT_SOURCE_TOKEN)?;
            let slot = required_string(args, "slot", "slot.set args")?;
            match scope.per_owner_store_slot(slot) {
                Some(true) => {}
                Some(false) => {
                    return Err(format!(
                        "slot.set owner-addressed write may only target per-owner slot `{slot}`"
                    ));
                }
                None => return Err(format!("slot.set references unknown slot `{slot}`")),
            }
            if scope.store_slot_is_readonly(slot) == Some(true) {
                return Err(format!("slot.set cannot write readonly slot `{slot}`"));
            }
            let value = bind_read(
                args.get("value")
                    .ok_or_else(|| "slot.set args is missing `value`".to_string())?,
                scope,
            )?;
            if value.root_type != IrType::Number {
                return Err("slot.set `value` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::SetOwnerSlot {
                slot: slot.to_string(),
                value,
            })
        }
        "setState" => Err("setState must target @impact.target".to_string()),
        _ => Err(format!("unsupported impact primitive `{primitive}`")),
    }
}

fn bind_read(value: &Value, scope: &EntityScope) -> Result<BoundProgram<EntityScope>, String> {
    let root: IrNode = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid impact IR expression: {error}"))?;
    bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root,
        },
        scope,
    )
    .map_err(|error| error.to_string())
}

fn bind_write(
    output: String,
    value: &Value,
    scope: &EntityScope,
) -> Result<BoundProgram<EntityScope>, String> {
    let root: IrNode = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid impact IR expression: {error}"))?;
    bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some(output),
            root,
        },
        scope,
    )
    .map_err(|error| error.to_string())
}

fn bind_number_write(
    output: String,
    value: &Value,
    scope: &EntityScope,
) -> Result<BoundProgram<EntityScope>, String> {
    let program = bind_write(output, value, scope)?;
    if program.root_type != IrType::Number {
        return Err("impact effect operand must evaluate to a number".to_string());
    }
    Ok(program)
}

fn plan_effect(effect: &BoundEffect, scope: &EntityScope) -> PlannedEffect {
    match effect {
        BoundEffect::Write(program) => PlannedEffect::Write {
            recipient: CommandRecipient::Target,
            handle: program
                .output
                .as_ref()
                .expect("bound impact write has an output handle")
                .clone(),
            value: eval_value(program, scope),
        },
        BoundEffect::SetOwnerSlot { slot, value } => PlannedEffect::Command {
            recipient: CommandRecipient::Source,
            effect: ImpactEffect::SetOwnerSlot {
                slot: slot.clone(),
                value: number(eval_value(value, scope)),
            },
        },
        BoundEffect::SetHealth { value, after_ms } => PlannedEffect::Command {
            recipient: CommandRecipient::Target,
            effect: ImpactEffect::SetHealth {
                value: number(eval_value(value, scope)),
                after_ms: *after_ms,
            },
        },
        BoundEffect::Despawn { after_ms } => PlannedEffect::Command {
            recipient: CommandRecipient::Target,
            effect: ImpactEffect::Despawn {
                after_ms: *after_ms,
            },
        },
        BoundEffect::PlayAnimation { state } => PlannedEffect::Command {
            recipient: CommandRecipient::Target,
            effect: ImpactEffect::PlayAnimation {
                state: state.clone(),
            },
        },
        BoundEffect::Present { template, value } => PlannedEffect::Command {
            recipient: CommandRecipient::Target,
            effect: ImpactEffect::Present {
                template: template.clone(),
                value: number(eval_value(value, scope)),
            },
        },
        BoundEffect::GrantHealth { amount } => PlannedEffect::Command {
            recipient: CommandRecipient::Source,
            effect: ImpactEffect::GrantHealth {
                amount: number(eval_value(amount, scope)),
            },
        },
        BoundEffect::GrantAmmo { pool, amount } => PlannedEffect::Command {
            recipient: CommandRecipient::Source,
            effect: ImpactEffect::GrantAmmo {
                pool: pool.clone(),
                amount: number(eval_value(amount, scope)),
            },
        },
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} is missing string `{key}`"))
}

fn optional_ms(args: &Map<String, Value>) -> Result<Option<f32>, String> {
    let Some(value) = args.get("afterMs") else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .ok_or_else(|| "impact effect `afterMs` must be a number".to_string())?;
    if !value.is_finite() || value > f32::MAX as f64 || value < f32::MIN as f64 {
        return Err("impact effect `afterMs` must be finite".to_string());
    }
    Ok(Some(value as f32))
}

fn require_impact_token(
    target: Option<&str>,
    primitive: &str,
    expected_token: &str,
) -> Result<(), String> {
    if target == Some(expected_token) {
        Ok(())
    } else {
        Err(format!("{primitive} must target {expected_token}"))
    }
}

fn number(value: IrValue) -> f32 {
    match value {
        IrValue::Number(value) => value,
        IrValue::Bool(_) => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::ammo_reserve::AmmoReserve;
    use postretro_entities::components::health::{
        DamageContext, HealthComponent, Hitbox, apply_damage_with_context,
    };
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, HealthDescriptor,
        PlayerMovementDescriptor, SpeedParams,
    };
    use postretro_entities::slot_table::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_entities::{EntityId, Transform};
    use postretro_foundation::{DamagePayload, Seat};
    use postretro_scripting_core::data_descriptors::DamagedEnemiesShield;
    use serde_json::json;

    fn assert_number_approx_eq(actual: f32, expected: f32) {
        const EPSILON: f32 = 1.0e-6;
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected} ± {EPSILON}, got {actual}"
        );
    }

    fn event(id: &str, tag: &str, policy: Vec<Value>) -> ImpactEventDescriptor {
        ImpactEventDescriptor {
            id: id.to_string(),
            is_override: false,
            levels: Vec::new(),
            filter_tag: Some(tag.to_string()),
            policy,
        }
    }

    fn override_event(id: &str, tag: &str, policy: Vec<Value>) -> ImpactEventDescriptor {
        let mut descriptor = event(id, tag, policy);
        descriptor.is_override = true;
        descriptor
    }

    fn input(name: &str) -> Value {
        json!({ "op": "input", "name": name })
    }

    fn owned_input(name: &str) -> Value {
        json!({ "op": "input", "name": name, "owner": IMPACT_SOURCE_TOKEN })
    }

    fn number(value: f32) -> Value {
        json!({ "op": "const", "value": value })
    }

    fn state_write(name: &str, value: Value) -> Value {
        json!({
            "primitive": "setState",
            "target": "@impact.target",
            "args": { "name": name, "value": value },
        })
    }

    fn slot_set(slot: &str, value: Value) -> Value {
        json!({
            "primitive": "slot.set",
            "args": { "slot": slot, "value": value },
        })
    }

    fn owner_slot_set(slot: &str, value: Value) -> Value {
        json!({
            "primitive": "slot.set",
            "target": "@impact.source",
            "args": { "slot": slot, "value": value },
        })
    }

    fn grant_health(amount: Value) -> Value {
        json!({
            "primitive": "grantHealth",
            "target": "@impact.source",
            "args": { "amount": amount },
        })
    }

    fn grant_ammo(pool: &str, amount: Value) -> Value {
        json!({
            "primitive": "grantAmmo",
            "target": "@impact.source",
            "args": { "type": pool, "amount": amount },
        })
    }

    fn present(template: &str, value: Value) -> Value {
        json!({
            "primitive": "present",
            "target": "@impact.target",
            "args": { "template": template, "value": value },
        })
    }

    fn presentation_template(id: &str) -> PresentationTemplate {
        serde_json::from_value(json!({
            "id": id,
            "root": {
                "kind": "text", "content": "0", "fontSize": 24.0,
                "color": [1.0, 0.35, 0.1, 1.0]
            },
            "lifetimeMs": 750,
            "motion": { "rise": 18.0, "easing": "easeOut" },
            "fade": { "startMs": 500 },
            "spawnScatter": { "radius": 0.25 },
        }))
        .expect("presentation template test fixture must deserialize")
    }

    fn overlay_template(id: &str) -> PresentationTemplate {
        let mut template = presentation_template(id);
        template.world_anchor = Some(PresentationWorldAnchor {
            socket: "status".to_string(),
            offset_y: 0.25,
        });
        template
    }

    fn damaged_enemies_overlay(
        template: &str,
        shield: Option<DamagedEnemiesShield>,
    ) -> PresentationOverlay {
        PresentationOverlay {
            over: PresentationOverlaySource::DamagedEnemies(DamagedEnemiesOverlay {
                linger_ms: 500,
                hide_at_full: true,
                shield,
            }),
            template: template.to_string(),
            max_visible: 2,
        }
    }

    fn give_target_hitbox(ctx: &ScriptCtx, target: EntityId) {
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target has health")
            .clone();
        health.hitbox = Some(Hitbox {
            half_extents: glam::Vec3::new(0.5, 1.5, 0.5),
            offset: glam::Vec3::new(0.0, 0.25, 0.0),
        });
        ctx.registry
            .borrow_mut()
            .set_component(target, health)
            .expect("target is live");
    }

    fn number_slot(value: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(value)),
            range: Some(NumericRange {
                min: -10_000.0,
                max: 10_000.0,
            }),
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn per_owner_number_slot(value: f32) -> SlotRecord {
        let mut record = number_slot(value);
        record.schema.per_owner = true;
        record
    }

    fn target(ctx: &ScriptCtx, tags: &[&str]) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let target = registry.spawn(Transform::default());
        registry
            .set_component(
                target,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 100.0,
                    hitbox: None,
                    zone_multipliers: Default::default(),
                }),
            )
            .expect("target is live");
        registry
            .set_tags(target, tags.iter().map(|tag| (*tag).to_string()).collect())
            .expect("target is live");
        target
    }

    fn mark_as_local_player(ctx: &ScriptCtx, target: EntityId) {
        let movement = PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
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
            slide: None,
            view_feel: None,
        });
        let mut registry = ctx.registry.borrow_mut();
        registry
            .set_component(target, movement)
            .expect("target is live");
        registry
            .mark_local_player_pawn(target)
            .expect("target is live");
    }

    fn source(ctx: &ScriptCtx, with_health: bool, with_ammo: bool) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let source = registry.spawn(Transform::default());
        if with_health {
            registry
                .set_component(
                    source,
                    HealthComponent::from_descriptor(&HealthDescriptor {
                        max: 100.0,
                        hitbox: None,
                        zone_multipliers: Default::default(),
                    }),
                )
                .expect("source is live");
        }
        if with_ammo {
            registry
                .set_component(source, AmmoReserve::new())
                .expect("source is live");
        }
        source
    }

    fn hit(ctx: &ScriptCtx, target: EntityId, producer: DamageProducer) {
        hit_from(ctx, target, None, producer);
    }

    fn hit_from(
        ctx: &ScriptCtx,
        target: EntityId,
        source: Option<EntityId>,
        producer: DamageProducer,
    ) {
        let mut context = DamageContext::new("impact-policy-test", producer);
        context.attacker = source;
        apply_damage_with_context(
            &mut ctx.registry.borrow_mut(),
            target,
            &DamagePayload { amount: 1.0 },
            context,
        );
    }

    fn evaluate_pending(ctx: &ScriptCtx, runtime: &mut ImpactPolicyRuntime) {
        runtime.evaluate_pending_in_registry(&mut ctx.registry.borrow_mut());
    }

    fn state(ctx: &ScriptCtx, target: EntityId, name: &str) -> f32 {
        ctx.registry
            .borrow()
            .get_component::<postretro_entities::EntityStateComponent>(target)
            .expect("target state exists")
            .get(name)
    }

    fn store(ctx: &ScriptCtx, name: &str) -> f32 {
        match ctx
            .slot_table
            .borrow()
            .get(name)
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected number slot value, got {other:?}"),
        }
    }

    fn owner_store(ctx: &ScriptCtx, name: &str, seat: Seat) -> f32 {
        match ctx
            .slot_table
            .borrow()
            .get(name)
            .and_then(|record| record.per_seat_value(seat))
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected owner number slot value, got {other:?}"),
        }
    }

    fn evaluate_slot_policy(base: f32, policy: Vec<Value>) -> f32 {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(base))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event("slot-policy", "crate", policy)]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        store(&ctx, "progress.xp")
    }

    #[test]
    fn damaged_enemy_overlay_stamps_facts_uses_hitbox_fallback_and_evicts_dead_targets() {
        use crate::presentation_pool::PresentationPool;
        use crate::scripting_systems::hit_zones::HitZoneStore;

        let ctx = ScriptCtx::new();
        let enemy = target(&ctx, &["enemy"]);
        give_target_hitbox(&ctx, enemy);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![overlay_template("enemy-status")]);
        runtime.replace_presentation_overlays(vec![damaged_enemies_overlay("enemy-status", None)]);
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();

        hit(&ctx, enemy, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |_| false,
            );
        }

        assert_eq!(pool.tracked_overlay_ids(), [enemy]);
        assert_eq!(
            pool.overlay_facts(enemy)
                .and_then(|facts| facts.get("healthFraction")),
            Some(&PresentationFact::Number(0.99))
        );
        assert_eq!(pool.overlay_is_suppressed(enemy), Some(false));
        assert_eq!(
            pool.overlay_anchor(enemy),
            Some(glam::Vec3::new(0.0, 2.0, 0.0))
        );

        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(enemy)
            .expect("target has health")
            .clone();
        health.current = health.max;
        ctx.registry
            .borrow_mut()
            .set_component(enemy, health)
            .expect("target is live");
        {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |_| false,
            );
        }
        assert_eq!(pool.overlay_is_suppressed(enemy), Some(true));

        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(enemy)
            .expect("target has health")
            .clone();
        health.current = 0.0;
        ctx.registry
            .borrow_mut()
            .set_component(enemy, health)
            .expect("target is live");
        {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |_| false,
            );
        }
        assert!(pool.tracked_overlay_ids().is_empty());
    }

    #[test]
    fn damaged_enemy_overlay_guards_zero_shield_max_and_same_frame_kills() {
        use crate::presentation_pool::PresentationPool;
        use crate::scripting_systems::hit_zones::HitZoneStore;

        let shield = DamagedEnemiesShield {
            value: IrNode::Input {
                name: "@state.shield".to_string(),
                owner: None,
            },
            max: IrNode::Input {
                name: "@state.shieldMax".to_string(),
                owner: None,
            },
        };
        let ctx = ScriptCtx::new();
        let enemy = target(&ctx, &["enemy"]);
        give_target_hitbox(&ctx, enemy);
        let mut entity_state = ctx
            .registry
            .borrow()
            .get_component::<postretro_entities::EntityStateComponent>(enemy)
            .expect("target state exists")
            .clone();
        entity_state.set("shield", 25.0);
        entity_state.set("shieldMax", 0.0);
        ctx.registry
            .borrow_mut()
            .set_component(enemy, entity_state)
            .expect("target is live");

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![overlay_template("enemy-status")]);
        runtime.replace_presentation_overlays(vec![damaged_enemies_overlay(
            "enemy-status",
            Some(shield),
        )]);
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();

        hit(&ctx, enemy, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |_| false,
            );
        }
        let facts = pool
            .overlay_facts(enemy)
            .expect("damage stamps overlay facts");
        assert_eq!(
            facts.get("shieldFraction"),
            Some(&PresentationFact::Number(0.0))
        );
        assert_eq!(facts.get("hasShield"), Some(&PresentationFact::Bool(false)));

        let doomed = target(&ctx, &["enemy"]);
        give_target_hitbox(&ctx, doomed);
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(doomed)
            .expect("doomed target has health")
            .clone();
        health.current = 1.0;
        ctx.registry
            .borrow_mut()
            .set_component(doomed, health)
            .expect("doomed target is live");
        hit(&ctx, doomed, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |_| false,
            );
        }
        assert!(
            !pool.tracked_overlay_ids().contains(&doomed),
            "a same-frame damage refresh must not draw a target killed by that hit"
        );
    }

    // Regression: a remote client's private damaged-enemy target was inserted
    // into the host renderer pool and competed with host-local overlays.
    #[test]
    fn remote_overlay_damage_samples_facts_without_entering_local_pool() {
        use crate::presentation_pool::PresentationPool;
        use crate::scripting_systems::hit_zones::HitZoneStore;

        let ctx = ScriptCtx::new();
        let enemy = target(&ctx, &["enemy"]);
        give_target_hitbox(&ctx, enemy);
        let remote_pawn = source(&ctx, false, false);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![overlay_template("enemy-status")]);
        runtime.replace_presentation_overlays(vec![damaged_enemies_overlay("enemy-status", None)]);
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();

        hit_from(&ctx, enemy, Some(remote_pawn), DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        let frame = {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [],
                |source| source == Some(remote_pawn),
            )
        };

        assert!(pool.tracked_overlay_ids().is_empty());
        assert_eq!(frame.damage.len(), 1);
        assert_eq!(frame.facts.len(), 1);
        assert_eq!(frame.facts[0].entity, enemy);

        let next = {
            let registry = ctx.registry.borrow();
            runtime.update_damaged_enemy_overlays(
                &mut pool,
                &registry,
                &hit_zones,
                0.0,
                [enemy],
                |_| false,
            )
        };
        assert!(pool.tracked_overlay_ids().is_empty());
        assert_eq!(next.facts.len(), 1, "remote recharge sampling stays live");
    }

    #[test]
    fn slot_set_evaluates_an_explicit_current_value_read() {
        let result = evaluate_slot_policy(
            10.0,
            vec![slot_set(
                "progress.xp",
                json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
            )],
        );

        assert_eq!(result, 11.0);
    }

    #[test]
    fn slot_set_aliasing_reads_the_frozen_snapshot() {
        let result = evaluate_slot_policy(
            10.0,
            vec![
                slot_set(
                    "progress.xp",
                    json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
                ),
                slot_set(
                    "progress.xp",
                    json!({ "op": "add", "a": input("progress.xp"), "b": number(2.0) }),
                ),
            ],
        );

        assert_eq!(result, 12.0);
    }

    #[test]
    fn slot_set_input_write_clobbers_an_earlier_absolute_write() {
        let result = evaluate_slot_policy(
            10.0,
            vec![
                slot_set("progress.xp", number(5.0)),
                slot_set(
                    "progress.xp",
                    json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
                ),
            ],
        );

        assert_eq!(result, 11.0);
    }

    #[test]
    fn slot_set_absolute_write_clobbers_an_earlier_input_write() {
        let result = evaluate_slot_policy(
            10.0,
            vec![
                slot_set(
                    "progress.xp",
                    json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
                ),
                slot_set("progress.xp", number(5.0)),
            ],
        );

        assert_eq!(result, 5.0);
    }

    #[test]
    fn slot_set_last_absolute_write_wins() {
        let result = evaluate_slot_policy(
            10.0,
            vec![
                slot_set("progress.xp", number(5.0)),
                slot_set("progress.xp", number(7.0)),
            ],
        );

        assert_eq!(result, 7.0);
    }

    #[test]
    fn p7_later_registered_distinct_event_write_wins_for_one_slot() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(10.0))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![
            event(
                "increment",
                "crate",
                vec![slot_set(
                    "progress.xp",
                    json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
                )],
            ),
            event(
                "replace",
                "crate",
                vec![slot_set("progress.xp", number(9.0))],
            ),
        ]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(store(&ctx, "progress.xp"), 9.0);
    }

    #[test]
    fn p8_matching_override_evicts_its_base_slot_write() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(0.0))
            .expect("new slot");
        let target = target(&ctx, &["crate", "reinforced"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "reward",
            "crate",
            vec![slot_set("progress.xp", number(1.0))],
        )]);
        runtime.replace_level_events(
            vec![override_event(
                "reward",
                "reinforced",
                vec![slot_set("progress.xp", number(2.0))],
            )],
            &[],
        );

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(store(&ctx, "progress.xp"), 2.0);
    }

    #[test]
    fn p9_empty_policy_leaves_store_unchanged() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(10.0))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event("empty", "crate", Vec::new())]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(store(&ctx, "progress.xp"), 10.0);
    }

    #[test]
    fn p10_empty_when_groups_evaluate_without_writes() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(10.0))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "empty-guards",
            "crate",
            vec![
                json!({ "when": { "op": "const", "value": false }, "do": [] }),
                json!({ "when": { "op": "const", "value": true }, "do": [] }),
            ],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(store(&ctx, "progress.xp"), 10.0);
    }

    #[test]
    fn p11_unproduced_store_slot_reads_its_declared_default() {
        let ctx = ScriptCtx::new();
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .insert("progress.xp".into(), number_slot(0.0))
                .expect("new output slot");
            slots
                .insert("progress.fresh".into(), number_slot(1.0))
                .expect("new defaulted slot");
        }
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "default-read",
            "crate",
            vec![json!({
                "when": { "op": "ge", "a": input("progress.fresh"), "b": number(1.0) },
                "do": [slot_set("progress.xp", number(9.0))],
            })],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(store(&ctx, "progress.xp"), 9.0);
    }

    // Regression: fixed-tick policies observed the prior HUD publish because
    // `player.health` was not republished between damage and impact evaluation.
    #[test]
    fn p12_fixed_tick_seam_reads_post_damage_engine_health() {
        let ctx = ScriptCtx::new();
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .insert("impact.observedHealth".into(), number_slot(0.0))
                .expect("new output slot");
        }
        let target = target(&ctx, &["player"]);
        mark_as_local_player(&ctx, target);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "capture-health",
            "player",
            vec![slot_set("impact.observedHealth", input("player.health"))],
        )]);

        let mut publisher =
            crate::scripting_systems::ui_proxy::PlayerHudStatePublisher::new(ctx.clone());
        let mut registry = ctx.registry.borrow_mut();
        let context = DamageContext::new("impact-policy-test", DamageProducer::InTick);
        apply_damage_with_context(
            &mut registry,
            target,
            &DamagePayload { amount: 1.0 },
            context,
        );
        crate::session::evaluate_pending_in_tick_impacts(
            &mut publisher,
            &mut runtime,
            &mut registry,
        );

        assert_eq!(store(&ctx, "impact.observedHealth"), 99.0);
        assert_eq!(store(&ctx, "player.health"), 99.0);
        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            99.0,
            "the published engine slot matches the post-damage component value",
        );
    }

    #[test]
    fn presentation_value_is_captured_at_plan_time_before_kill_changes_target_state() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, false, false);
        let anchor = glam::Vec3::new(3.0, 4.0, 5.0);
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut transform = *registry
                .get_component::<Transform>(target)
                .expect("target has transform");
            transform.position = anchor;
            registry
                .set_component(target, transform)
                .expect("target transform updates");
        }

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![presentation_template("damageNumber")]);
        runtime.replace_global_events(vec![event(
            "kill-number",
            "crate",
            vec![
                present("damageNumber", input("@impact.healthAfter")),
                json!({
                    "primitive": "setHealth", "target": "@impact.target",
                    "args": { "value": number(25.0) },
                }),
                json!({ "primitive": "despawn", "target": "@impact.target", "args": {} }),
            ],
        )]);

        let mut damage = DamageContext::new("kill-number", DamageProducer::InTick);
        damage.attacker = Some(source);
        apply_damage_with_context(
            &mut ctx.registry.borrow_mut(),
            target,
            &DamagePayload { amount: 100.0 },
            damage,
        );
        evaluate_pending(&ctx, &mut runtime);

        let mut registry = ctx.registry.borrow_mut();
        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .expect("staged removal keeps target alive")
                .current,
            25.0,
            "consequence phase changed live health before the presentation apply intercept"
        );
        assert!(
            registry.get_component::<Transform>(target).is_ok(),
            "staged scripted despawn leaves the anchor readable through presentation apply"
        );
        let spawns = registry.take_presentation_spawns();
        assert_eq!(spawns.len(), 1);
        let spawn = &spawns[0];
        assert_eq!(spawn.template.0, "damageNumber");
        assert_eq!(spawn.world_anchor, anchor);
        assert_eq!(
            spawn.facts.get("value"),
            Some(&PresentationFact::Number(0.0)),
            "the visual retained the plan-time killing-blow scalar, not the later health write"
        );
        assert_eq!(
            spawn.presenter,
            Some(PresentationPresenter(source.to_raw())),
            "the dispatch source is retained as the presentation presenter"
        );
    }

    #[test]
    fn presentation_without_transform_skips_the_spawn() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        ctx.registry
            .borrow_mut()
            .remove_component::<Transform>(target)
            .expect("test target initially has a transform");

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_presentation_templates(vec![presentation_template("damageNumber")]);
        runtime.replace_global_events(vec![event(
            "missing-anchor",
            "crate",
            vec![present("damageNumber", input("@impact.healthAfter"))],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert!(
            ctx.registry
                .borrow_mut()
                .take_presentation_spawns()
                .is_empty(),
            "a target without an anchor degrades to a skipped passive presentation"
        );
    }

    #[test]
    fn p13_consecutive_in_tick_hits_reseed_store_reads_for_each_fire() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("progress.xp".into(), number_slot(10.0))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "counter",
            "crate",
            vec![slot_set(
                "progress.xp",
                json!({ "op": "add", "a": input("progress.xp"), "b": number(1.0) }),
            )],
        )]);

        let mut publisher =
            crate::scripting_systems::ui_proxy::PlayerHudStatePublisher::new(ctx.clone());
        let mut registry = ctx.registry.borrow_mut();
        for expected_xp in [11.0, 12.0] {
            let context = DamageContext::new("impact-policy-test", DamageProducer::InTick);
            apply_damage_with_context(
                &mut registry,
                target,
                &DamagePayload { amount: 1.0 },
                context,
            );
            crate::session::evaluate_pending_in_tick_impacts(
                &mut publisher,
                &mut runtime,
                &mut registry,
            );
            assert_eq!(store(&ctx, "progress.xp"), expected_xp);
        }
        drop(registry);

        assert_eq!(store(&ctx, "progress.xp"), 12.0);
    }

    #[test]
    fn breakable_threshold_reads_pre_effect_state_snapshot() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("impact.broken".into(), number_slot(0.0))
            .expect("new slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "breakable",
            "crate",
            vec![
                state_write(
                    "hits",
                    json!({ "op": "add", "a": input("@state.hits"), "b": number(1.0) }),
                ),
                json!({
                    "when": { "op": "eq", "a": input("@state.hits"), "b": number(2.0) },
                    "do": [
                        slot_set(
                            "impact.broken",
                            json!({
                                "op": "add",
                                "a": input("impact.broken"),
                                "b": number(1.0),
                            }),
                        ),
                        { "primitive": "despawn", "target": "@impact.target", "args": {} },
                    ],
                }),
            ],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_eq!(state(&ctx, target, "hits"), 1.0);
        assert_eq!(store(&ctx, "impact.broken"), 0.0);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_eq!(state(&ctx, target, "hits"), 2.0);
        assert_eq!(store(&ctx, "impact.broken"), 0.0);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_eq!(state(&ctx, target, "hits"), 3.0);
        assert_eq!(store(&ctx, "impact.broken"), 1.0);
        assert!(
            ctx.registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .expect("target remains live until frame end")
                .inert
        );
    }

    #[test]
    fn matching_groups_apply_independently() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "groups",
            "crate",
            vec![
                json!({ "when": { "op": "const", "value": true }, "do": [state_write("first", number(1.0))] }),
                json!({ "when": { "op": "const", "value": true }, "do": [state_write("second", number(1.0))] }),
            ],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "first"), 1.0);
        assert_eq!(state(&ctx, target, "second"), 1.0);
    }

    #[test]
    fn consecutive_in_tick_hits_observe_the_previous_fire_effects() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "counter",
            "crate",
            vec![state_write(
                "hits",
                json!({ "op": "add", "a": input("@state.hits"), "b": number(1.0) }),
            )],
        )]);

        let mut registry = ctx.registry.borrow_mut();
        for _ in 0..2 {
            let context = DamageContext::new("impact-policy-test", DamageProducer::InTick);
            apply_damage_with_context(
                &mut registry,
                target,
                &DamagePayload { amount: 1.0 },
                context,
            );
            runtime.evaluate_pending_in_registry(&mut registry);
        }
        drop(registry);

        assert_eq!(state(&ctx, target, "hits"), 2.0);
    }

    // Regression: the entity raycast treated zero HP as an engine-owned corpse
    // state, so a downed target could never reach a later gib policy.
    #[test]
    fn downed_target_ray_hit_reaches_later_impact_policy() {
        use crate::scripting_systems::hit_zones::{HitZoneStore, nearest_entity_hit};
        use glam::Vec3;
        use postretro_entities::components::health::Hitbox;

        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["zombie"]);
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        health.current = 0.0;
        health.death_handled = true;
        health.hitbox = Some(Hitbox {
            half_extents: Vec3::splat(0.5),
            offset: Vec3::ZERO,
        });
        ctx.registry
            .borrow_mut()
            .set_component(target, health)
            .unwrap();

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "gib-downed",
            "zombie",
            vec![json!({
                "when": {
                    "op": "le",
                    "a": input("@impact.healthAfter"),
                    "b": number(-40.0),
                },
                "do": [state_write("gibbed", number(1.0))],
            })],
        )]);

        let hit = nearest_entity_hit(
            &ctx.registry.borrow(),
            &HitZoneStore::new(),
            0.0,
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::NEG_Z,
            10.0,
            0.0,
        )
        .expect("the downed target remains on the weapon ray");
        assert_eq!(hit.target, target);

        apply_damage_with_context(
            &mut ctx.registry.borrow_mut(),
            hit.target,
            &DamagePayload { amount: 50.0 },
            DamageContext::new("weapon.gib", DamageProducer::InTick),
        );
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "gibbed"), 1.0);
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            0.0,
            "stored HP remains floored while healthAfter carries the gib overshoot",
        );
    }

    // Regression: the combat demo's 48-HP dummy must down on the sixteenth
    // 3-damage pellet, then let the next pellet's raw -3 overkill trigger its
    // authored finisher. Stored HP floors at zero; overkill never accumulates
    // across pellets.
    #[test]
    fn shotgun_demo_policy_downs_on_second_shell_then_gibs_on_next_pellet() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["dummy"]);
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target has health")
            .clone();
        health.max = 48.0;
        health.current = 48.0;
        ctx.registry
            .borrow_mut()
            .set_component(target, health)
            .expect("target remains live");

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "demo-shotgun-finish",
            "dummy",
            vec![json!({
                "when": {
                    "op": "le",
                    "a": input("@impact.healthAfter"),
                    "b": number(-3.0),
                },
                "do": [{ "primitive": "despawn", "target": "@impact.target", "args": {} }],
            })],
        )]);

        for pellet in 0..16 {
            let mut registry = ctx.registry.borrow_mut();
            apply_damage_with_context(
                &mut registry,
                target,
                &DamagePayload { amount: 3.0 },
                DamageContext::new("weapon.shotgun", DamageProducer::InTick),
            );
            runtime.evaluate_pending_in_registry(&mut registry);
            drop(registry);

            assert!(
                ctx.registry
                    .borrow()
                    .get_component::<postretro_entities::DeferredEffectComponent>(target)
                    .is_ok_and(|effects| !effects.inert),
                "pellet {} must not gib the staged dummy",
                pellet + 1
            );
        }

        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target stays live while down")
                .current,
            0.0,
            "the second shell's final pellet downs the dummy"
        );

        let mut registry = ctx.registry.borrow_mut();
        apply_damage_with_context(
            &mut registry,
            target,
            &DamagePayload { amount: 3.0 },
            DamageContext::new("weapon.shotgun", DamageProducer::InTick),
        );
        runtime.evaluate_pending_in_registry(&mut registry);
        drop(registry);

        assert!(
            ctx.registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .expect("the finisher marks the target for despawn")
                .inert,
            "the next pellet's healthAfter = -3 triggers the authored finisher"
        );
    }

    #[test]
    fn matching_override_uses_last_registered_policy_only() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate", "reinforced"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.set_mod_id(Some("combat-demo".to_string()));
        runtime.replace_global_events(vec![event(
            "same",
            "crate",
            vec![state_write("base_only", number(1.0))],
        )]);
        runtime.replace_level_events(
            vec![override_event(
                "same",
                "reinforced",
                vec![state_write("variant", number(3.0))],
            )],
            &[],
        );

        assert_eq!(
            runtime
                .policies
                .iter()
                .map(|policy| policy.id.as_str())
                .collect::<Vec<_>>(),
            ["combat-demo:same", "combat-demo:same"],
            "base-filter inheritance and dispatch eviction must share the qualified id",
        );

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "base_only"), 0.0);
        assert_eq!(state(&ctx, target, "variant"), 3.0);
    }

    #[test]
    fn override_requires_both_base_and_additional_tags() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["reinforced"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![
            event("same", "crate", vec![state_write("base_only", number(1.0))]),
            override_event(
                "same",
                "reinforced",
                vec![state_write("variant", number(3.0))],
            ),
        ]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "base_only"), 0.0);
        assert_eq!(state(&ctx, target, "variant"), 0.0);
    }

    #[test]
    fn mod_scope_levels_selector_filters_before_level_composition() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut campaign = event(
            "campaign-only",
            "crate",
            vec![state_write("campaign", number(1.0))],
        );
        campaign.levels = vec!["campaign".to_string()];
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![campaign]);
        runtime.replace_level_events(Vec::new(), &["deathmatch".to_string()]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_eq!(state(&ctx, target, "campaign"), 0.0);

        runtime.replace_level_events(Vec::new(), &["campaign".to_string()]);
        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_eq!(state(&ctx, target, "campaign"), 1.0);
    }

    #[test]
    fn synchronous_impact_effect_runs_before_legacy_death_sweep() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["survivor"]);
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        health.current = 1.0;
        ctx.registry
            .borrow_mut()
            .set_component(target, health)
            .unwrap();
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "survive",
            "survivor",
            vec![json!({
                "primitive": "setHealth",
                "target": "@impact.target",
                "args": { "value": number(10.0) },
            })],
        )]);

        {
            let mut registry = ctx.registry.borrow_mut();
            let context = DamageContext::new("impact-policy-test", DamageProducer::InTick);
            apply_damage_with_context(
                &mut registry,
                target,
                &DamagePayload { amount: 1.0 },
                context,
            );
            runtime.evaluate_pending_in_registry(&mut registry);
        }
        let events = crate::sim::run_death_sweep(&ctx.registry);

        assert!(events.is_empty());
        assert!(ctx.registry.borrow().exists(target));
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            10.0
        );
    }

    #[test]
    fn non_finite_set_health_expression_resolves_to_zero_without_rearming() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["downed"]);
        let mut health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        health.current = 0.0;
        health.death_handled = true;
        ctx.registry
            .borrow_mut()
            .set_component(target, health)
            .unwrap();

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "non-finite-health",
            "downed",
            vec![json!({
                "primitive": "setHealth",
                "target": "@impact.target",
                "args": {
                    "value": {
                        "op": "mul",
                        "a": number(1.0e30),
                        "b": number(1.0e30),
                    },
                },
            })],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        let health = ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        assert_eq!(health.current, 0.0);
        assert!(
            health.death_handled,
            "IR non-finite arithmetic coerces to zero, which must not resurrect",
        );
    }

    #[test]
    fn distinct_event_ids_do_not_merge_even_when_they_share_a_target() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate", "vase"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![
            event(
                "crate-policy",
                "crate",
                vec![state_write("crate", number(1.0))],
            ),
            event(
                "vase-policy",
                "vase",
                vec![state_write("vase", number(1.0))],
            ),
        ]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "crate"), 1.0);
        assert_eq!(state(&ctx, target, "vase"), 1.0);
    }

    #[test]
    fn source_grants_credit_the_damager_with_target_scoped_operands() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, true, true);
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(source)
                .expect("source has health")
                .clone();
            health.current = 50.0;
            registry.set_component(source, health).unwrap();
        }

        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "source-grant",
            "crate",
            vec![
                grant_health(input("@impact.amount")),
                grant_ammo("cells", input("@impact.amount")),
            ],
        )]);

        hit_from(&ctx, target, Some(source), DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        let registry = ctx.registry.borrow();
        assert_eq!(
            registry
                .get_component::<HealthComponent>(source)
                .expect("source remains live")
                .current,
            51.0,
            "grantHealth applies to the damager while its operand reads the impact snapshot",
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(source)
                .expect("source has reserve")
                .available("cells"),
            1,
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            99.0,
            "the damaged target does not receive source-addressed grants",
        );
    }

    #[test]
    fn source_addressed_slot_set_rewards_the_damager_seat() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new per-owner slot");
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, false, false);
        ctx.registry.borrow_mut().bind_pawn_seat(source, Seat(7));
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "source-owner-reward",
            "crate",
            vec![owner_slot_set("currency.xp", number(5.0))],
        )]);

        hit_from(&ctx, target, Some(source), DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_number_approx_eq(owner_store(&ctx, "currency.xp", Seat(7)), 5.0);
    }

    #[test]
    fn owner_slot_write_without_a_source_seat_warns_and_keeps_siblings_running() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new per-owner slot");
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, false, false);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "owner-no-seat",
            "crate",
            vec![
                owner_slot_set("currency.xp", number(5.0)),
                state_write("sibling_runs", number(1.0)),
            ],
        )]);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            hit_from(&ctx, target, Some(source), DamageProducer::InTick);
            evaluate_pending(&ctx, &mut runtime);
        });

        assert_number_approx_eq(state(&ctx, target, "sibling_runs"), 1.0);
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("[Impact] owner write for slot `currency.xp` resolved no seat")
        }));
    }

    #[test]
    fn owner_slot_writes_are_last_writer_per_fire_and_accrue_across_hits() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new per-owner slot");
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, false, false);
        ctx.registry.borrow_mut().bind_pawn_seat(source, Seat(3));
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "owner-last-writer",
            "crate",
            vec![
                owner_slot_set("currency.xp", number(1.0)),
                owner_slot_set("currency.xp", number(2.0)),
            ],
        )]);

        hit_from(&ctx, target, Some(source), DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);
        assert_number_approx_eq(owner_store(&ctx, "currency.xp", Seat(3)), 2.0);

        runtime.replace_global_events(vec![event(
            "owner-accrual",
            "crate",
            vec![owner_slot_set(
                "currency.xp",
                json!({ "op": "add", "a": owned_input("currency.xp"), "b": number(1.0) }),
            )],
        )]);
        for expected in [3.0, 4.0] {
            hit_from(&ctx, target, Some(source), DamageProducer::InTick);
            evaluate_pending(&ctx, &mut runtime);
            assert_number_approx_eq(owner_store(&ctx, "currency.xp", Seat(3)), expected);
        }
    }

    #[test]
    fn absent_or_stale_source_skips_only_the_source_grant() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "source-may-be-absent",
            "crate",
            vec![
                grant_health(number(5.0)),
                state_write(
                    "sibling_runs",
                    json!({ "op": "add", "a": input("@state.sibling_runs"), "b": number(1.0) }),
                ),
            ],
        )]);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            hit(&ctx, target, DamageProducer::InTick);
            evaluate_pending(&ctx, &mut runtime);

            let source = source(&ctx, true, false);
            hit_from(&ctx, target, Some(source), DamageProducer::InTick);
            ctx.registry
                .borrow_mut()
                .despawn(source)
                .expect("source is live before evaluation");
            evaluate_pending(&ctx, &mut runtime);
        });

        assert_eq!(state(&ctx, target, "sibling_runs"), 2.0);
        assert!(
            captured.is_empty(),
            "an absent or stale command source is a silent per-effect skip: {captured:?}"
        );
    }

    #[test]
    fn skipped_grant_component_warns_without_aborting_sibling_effects() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, false, false);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "missing-recipient-components",
            "crate",
            vec![
                grant_health(number(5.0)),
                grant_ammo("cells", number(4.0)),
                state_write("sibling_runs", number(1.0)),
            ],
        )]);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            hit_from(&ctx, target, Some(source), DamageProducer::InTick);
            evaluate_pending(&ctx, &mut runtime);
        });

        assert_eq!(state(&ctx, target, "sibling_runs"), 1.0);
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn && message.contains("[Grant] grantHealth")
        }));
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn && message.contains("[Grant] grantAmmo")
        }));
    }

    #[test]
    fn negative_source_grant_warns_and_sibling_effect_continues() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let source = source(&ctx, true, false);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "negative-source-grant",
            "crate",
            vec![
                grant_health(number(-1.0)),
                state_write("sibling_runs", number(1.0)),
            ],
        )]);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            hit_from(&ctx, target, Some(source), DamageProducer::InTick);
            evaluate_pending(&ctx, &mut runtime);
        });

        assert_eq!(state(&ctx, target, "sibling_runs"), 1.0);
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("[Grant] grantHealth: amount -1 is negative or non-finite")
        }));
    }

    #[test]
    fn invalid_ammo_pool_skips_its_policy_while_siblings_still_bind_and_run() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.set_mod_id(Some("postretro.dev".to_string()));
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            runtime.replace_global_events(vec![
                event(
                    "invalid-pool",
                    "crate",
                    vec![grant_ammo("not valid", number(1.0))],
                ),
                event(
                    "valid-sibling",
                    "crate",
                    vec![state_write("sibling_runs", number(1.0))],
                ),
            ]);
        });

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "sibling_runs"), 1.0);
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("policy `postretro.dev:invalid-pool` was skipped during bind")
                && message.contains("`grantAmmo.type` must match [A-Za-z0-9_.:-]")
        }));
    }

    #[test]
    fn invalid_store_read_skips_only_its_policy_while_siblings_still_run() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.personal".into(), per_owner_number_slot(0.0))
            .expect("new per-owner slot");
        ctx.slot_table
            .borrow_mut()
            .insert("currency.global".into(), number_slot(0.0))
            .expect("new global slot");
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.set_mod_id(Some("postretro.dev".to_string()));

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            runtime.replace_global_events(vec![
                event(
                    "bare-per-owner-read",
                    "crate",
                    vec![slot_set("currency.global", input("currency.personal"))],
                ),
                event(
                    "owner-addressed-global-read",
                    "crate",
                    vec![slot_set("currency.global", owned_input("currency.global"))],
                ),
                event(
                    "valid-sibling",
                    "crate",
                    vec![slot_set("currency.global", number(7.0))],
                ),
            ]);
        });

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("currency.global")
                .expect("global slot")
                .value,
            Some(SlotValue::Number(7.0)),
            "invalid descriptors do not prevent a sibling descriptor from binding",
        );
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message
                    .contains("policy `postretro.dev:bare-per-owner-read` was skipped during bind")
                && message.contains("currency.personal")
        }));
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains(
                    "policy `postretro.dev:owner-addressed-global-read` was skipped during bind",
                )
                && message.contains("currency.global")
        }));
    }

    #[test]
    fn app_drain_dispatches_are_consumed_without_policy_evaluation() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "app-drain",
            "crate",
            vec![grant_health(number(10.0)), state_write("ran", number(1.0))],
        )]);

        hit(&ctx, target, DamageProducer::AppDrain);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "ran"), 0.0);
    }

    #[test]
    fn impact_effect_wire_rejects_raw_store_assignment_and_boolean_operands() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("impact.total".into(), number_slot(0.0))
            .expect("new slot");
        ctx.slot_table
            .borrow_mut()
            .insert("currency.personal".into(), per_owner_number_slot(0.0))
            .expect("new per-owner slot");
        let scope = EntityScope::impact(ctx);

        let raw_assignment = json!({
            "primitive": "setState",
            "args": { "slot": "impact.total", "value": number(99.0) },
        });
        assert_eq!(
            bind_effect(&raw_assignment, &scope).err().unwrap(),
            "setState must target @impact.target"
        );

        for malformed in [
            json!({
                "primitive": "setHealth",
                "target": "@impact.target",
                "args": { "value": { "op": "const", "value": true } },
            }),
            json!({
                "primitive": "setState",
                "target": "@impact.target",
                "args": { "name": "bad", "value": { "op": "const", "value": true } },
            }),
            json!({
                "primitive": "slot.set",
                "args": { "slot": "impact.total", "value": { "op": "const", "value": true } },
            }),
        ] {
            assert!(
                bind_effect(&malformed, &scope).is_err(),
                "numeric effect operand accepted boolean IR: {malformed}"
            );
        }

        let slot_set_with_target = json!({
            "primitive": "slot.set",
            "target": "@impact.target",
            "args": { "slot": "impact.total", "value": number(1.0) },
        });
        assert_eq!(
            bind_effect(&slot_set_with_target, &scope)
                .err()
                .expect("slot.set must reject a present target"),
            "slot.set must target @impact.source"
        );

        let bare_per_owner_error = bind_effect(&slot_set("currency.personal", number(1.0)), &scope)
            .err()
            .expect("bare per-owner output must reject");
        assert!(
            bare_per_owner_error.contains("currency.personal"),
            "the bare output diagnostic must name the per-owner slot"
        );
        assert_eq!(
            bind_effect(&owner_slot_set("impact.total", number(1.0)), &scope)
                .err()
                .expect("owner-addressed global output must reject"),
            "slot.set owner-addressed write may only target per-owner slot `impact.total`"
        );

        for primitive in ["despawn", "playAnim", "setHealth", "setState"] {
            let malformed = json!({
                "primitive": primitive,
                "target": "@impact.source",
                "args": {},
            });
            assert!(
                bind_effect(&malformed, &scope).is_err(),
                "target-bearing arm accepted the wrong token: {malformed}"
            );
        }
    }

    #[test]
    fn source_grant_wire_requires_source_token_and_number_amount() {
        let scope = EntityScope::impact(ScriptCtx::new());

        for (primitive, args) in [
            ("grantHealth", json!({ "amount": number(1.0) })),
            (
                "grantAmmo",
                json!({ "type": "cells", "amount": number(1.0) }),
            ),
        ] {
            for target in [None, Some("@impact.target")] {
                let mut effect = serde_json::json!({
                    "primitive": primitive,
                    "args": args,
                });
                if let Some(target) = target {
                    effect["target"] = json!(target);
                }
                assert_eq!(
                    bind_effect(&effect, &scope)
                        .err()
                        .expect("grant arm must reject wrong target"),
                    format!("{primitive} must target @impact.source"),
                    "grant arm accepted a non-source command target: {effect}",
                );
            }
        }

        let health_boolean = json!({
            "primitive": "grantHealth",
            "target": "@impact.source",
            "args": { "amount": { "op": "const", "value": true } },
        });
        assert_eq!(
            bind_effect(&health_boolean, &scope)
                .err()
                .expect("grantHealth boolean root must reject"),
            "grantHealth `amount` must evaluate to a number"
        );

        let ammo_boolean = json!({
            "primitive": "grantAmmo",
            "target": "@impact.source",
            "args": { "type": "cells", "amount": { "op": "const", "value": true } },
        });
        assert_eq!(
            bind_effect(&ammo_boolean, &scope)
                .err()
                .expect("grantAmmo boolean root must reject"),
            "grantAmmo `amount` must evaluate to a number"
        );
    }

    #[test]
    fn engine_binding_rejects_tagless_override_descriptors() {
        let ctx = ScriptCtx::new();
        let scope = EntityScope::impact(ctx);
        let mut descriptor = override_event("base", "elite", Vec::new());
        descriptor.filter_tag = None;

        assert_eq!(
            bind_policy(
                &descriptor,
                "postretro.dev:salvage",
                Some("crate".to_string()),
                &scope,
            )
            .err()
            .unwrap(),
            "impact override filter requires `tag`"
        );
    }

    #[test]
    fn unknown_override_diagnostic_names_composition_id() {
        assert_eq!(
            unknown_override_diagnostic("postretro.dev:salvage"),
            "override targets unknown event \"postretro.dev:salvage\""
        );
    }

    #[test]
    fn impact_policy_bind_and_override_diagnostics_use_qualified_mod_id() {
        let ctx = ScriptCtx::new();
        let mut runtime = ImpactPolicyRuntime::new(ctx);
        runtime.set_mod_id(Some("postretro.dev".to_string()));

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            runtime.replace_global_events(vec![event(
                "bad-bind",
                "crate",
                vec![grant_ammo("not valid", number(1.0))],
            )]);
            runtime.replace_level_events(vec![override_event("unknown", "crate", Vec::new())], &[]);
        });

        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("policy `postretro.dev:bad-bind` was skipped during bind")
        }));
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("override targets unknown event \"postretro.dev:unknown\"")
        }));
    }

    #[test]
    fn level_events_without_a_committed_mod_id_stay_unqualified() {
        let ctx = ScriptCtx::new();
        let mut runtime = ImpactPolicyRuntime::new(ctx);
        runtime.replace_global_events(vec![event("base", "crate", Vec::new())]);
        assert_eq!(runtime.policies[0].id, "base");

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            runtime.replace_level_events(vec![override_event("unknown", "crate", Vec::new())], &[]);
        });

        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("override targets unknown event \"unknown\"")
                && !message.contains(":unknown")
        }));
    }

    #[test]
    fn ammo_pool_identifiers_retain_colon_support() {
        let scope = EntityScope::impact(ScriptCtx::new());
        assert!(
            bind_effect(&grant_ammo("mods:cells", number(1.0)), &scope).is_ok(),
            "mod id validation must not narrow the shared ammo identifier grammar",
        );
    }

    #[test]
    fn distinct_events_apply_in_registration_order() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![
            event("first", "crate", vec![state_write("order", number(1.0))]),
            event("second", "crate", vec![state_write("order", number(2.0))]),
        ]);

        hit(&ctx, target, DamageProducer::InTick);
        evaluate_pending(&ctx, &mut runtime);

        assert_eq!(state(&ctx, target, "order"), 2.0);
    }
}
