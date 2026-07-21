// Runtime-owned impact-policy binding and per-fire evaluation.
// See: context/plans/in-progress/E16--impact-policy-substrate/index.md (Task 5).

use postretro_entities::components::health::{DamageProducer, ImpactDispatch};
use postretro_entities::{EntityId, ScriptCtx};
use postretro_foundation::ImpactEventDescriptor;
use postretro_foundation::ir::{
    BakedIr, BindingScope, BoundProgram, CURRENT_IR_VERSION, IrNode, IrType, IrValue, bind,
    eval_value,
};
use postretro_scripting_core::ir_scopes::{EntityOutputHandle, EntityScope};
use serde_json::{Map, Value};

use crate::impact_effects::{ImpactEffect, apply_effect};

/// The single consumer of the health chokepoint's impact-dispatch queue.
///
/// The runtime retains raw global and level descriptors, rebuilds their bound
/// form on registration changes, and evaluates only the in-tick producer arm.
/// One [`EntityScope`] is shared by all bound policies so every operand on an
/// impact fire observes exactly one state/store/fact snapshot.
pub(crate) struct ImpactPolicyRuntime {
    ctx: ScriptCtx,
    global_events: Vec<ImpactEventDescriptor>,
    level_events: Vec<ImpactEventDescriptor>,
    scope: EntityScope,
    policies: Vec<BoundImpactPolicy>,
    consequential: Vec<PlannedEffect>,
    presentation: Vec<PlannedEffect>,
}

struct BoundImpactPolicy {
    id: String,
    filter_tag: Option<String>,
    groups: Vec<BoundGroup>,
}

struct BoundGroup {
    when: Option<BoundProgram<EntityScope>>,
    effects: Vec<BoundEffect>,
}

enum BoundEffect {
    Write(BoundProgram<EntityScope>),
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
}

enum PlannedEffect {
    Write {
        handle: EntityOutputHandle,
        value: IrValue,
    },
    Command(ImpactEffect),
}

impl ImpactPolicyRuntime {
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            scope: EntityScope::impact(ctx.clone()),
            ctx,
            global_events: Vec::new(),
            level_events: Vec::new(),
            policies: Vec::new(),
            consequential: Vec::new(),
            presentation: Vec::new(),
        }
    }

    /// Replace the complete mod-scope descriptor snapshot. A staged mod-init
    /// commit has the same snapshot semantics as initial mod registration.
    pub(crate) fn replace_global_events(&mut self, events: Vec<ImpactEventDescriptor>) {
        self.global_events = events;
        self.rebuild();
    }

    /// Replace the per-level descriptors after `setupLevel()` finishes. Global
    /// entries are intentionally retained and precede these in load order.
    pub(crate) fn replace_level_events(&mut self, events: Vec<ImpactEventDescriptor>) {
        self.level_events = events;
        self.rebuild();
    }

    pub(crate) fn clear_level_events(&mut self) {
        self.level_events.clear();
        self.rebuild();
    }

    /// Drain all currently published impact fires. App-drain dispatches are
    /// deliberately consumed without evaluation: v1 charts that producer at
    /// the choke point but gives it no impact policy surface.
    pub(crate) fn evaluate_pending(&mut self) {
        let dispatches = self.ctx.registry.borrow_mut().take_impact_dispatches();
        for dispatch in dispatches {
            if dispatch.producer != DamageProducer::InTick {
                continue;
            }
            self.evaluate_dispatch(dispatch);
        }
    }

    fn rebuild(&mut self) {
        let scope = EntityScope::impact(self.ctx.clone());
        let mut policies = Vec::with_capacity(self.global_events.len() + self.level_events.len());
        for descriptor in self.global_events.iter().chain(&self.level_events) {
            match bind_policy(descriptor, &scope) {
                Ok(policy) => policies.push(policy),
                Err(error) => log::warn!(
                    "[Impact] policy `{}` was skipped during bind: {error}",
                    descriptor.id
                ),
            }
        }
        self.scope = scope;
        self.policies = policies;
    }

    fn evaluate_dispatch(&mut self, dispatch: ImpactDispatch) {
        let tags = {
            let registry = self.ctx.registry.borrow();
            let Ok(tags) = registry.get_tags(dispatch.target) else {
                return;
            };
            tags.to_vec()
        };

        if let Err(error) = self.scope.seed_impact(&dispatch) {
            log::warn!("[Impact] dispatch scope seed failed; skipping impact: {error:?}");
            return;
        }

        self.consequential.clear();
        self.presentation.clear();

        // A later matching variant replaces an earlier one with the same
        // derived id. Removing then appending also makes cross-event execution
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
                        BoundEffect::PlayAnimation { .. } => self.presentation.push(planned),
                        BoundEffect::Write(_)
                        | BoundEffect::SetHealth { .. }
                        | BoundEffect::Despawn { .. } => self.consequential.push(planned),
                    }
                }
            }
        }

        self.apply_planned(dispatch.target, false);
        self.apply_planned(dispatch.target, true);
    }

    fn apply_planned(&mut self, target: EntityId, presentation: bool) {
        let effects = if presentation {
            &mut self.presentation
        } else {
            &mut self.consequential
        };
        effects.reverse();
        while let Some(effect) = effects.pop() {
            match effect {
                PlannedEffect::Write { handle, value } => self.scope.write(&handle, value),
                PlannedEffect::Command(effect) => {
                    apply_effect(&mut self.ctx.registry.borrow_mut(), target, &effect);
                }
            }
        }
    }
}

fn policy_matches(policy: &BoundImpactPolicy, tags: &[String]) -> bool {
    policy
        .filter_tag
        .as_ref()
        .is_none_or(|filter| tags.iter().any(|tag| tag == filter))
}

fn bind_policy(
    descriptor: &ImpactEventDescriptor,
    scope: &EntityScope,
) -> Result<BoundImpactPolicy, String> {
    let mut groups = Vec::with_capacity(descriptor.policy.len());
    for entry in &descriptor.policy {
        groups.push(bind_group(entry, scope)?);
    }
    Ok(BoundImpactPolicy {
        id: descriptor.id.clone(),
        filter_tag: descriptor.filter_tag.clone(),
        groups,
    })
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
    let target = effect.get("target").and_then(Value::as_str);

    match primitive {
        "despawn" => {
            require_impact_target(target, primitive)?;
            Ok(BoundEffect::Despawn {
                after_ms: optional_ms(args)?,
            })
        }
        "playAnim" => {
            require_impact_target(target, primitive)?;
            Ok(BoundEffect::PlayAnimation {
                state: required_string(args, "clip", "playAnim args")?.to_string(),
            })
        }
        "setHealth" => {
            require_impact_target(target, primitive)?;
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
        "setState" if target == Some("@impact.target") => {
            let name = required_string(args, "name", "target setState args")?;
            let value = args
                .get("value")
                .ok_or_else(|| "target setState args is missing `value`".to_string())?;
            bind_write(format!("@state.{name}"), value, scope).map(BoundEffect::Write)
        }
        "setState" if target.is_none() => {
            let slot = required_string(args, "slot", "slot setState args")?;
            let value = args
                .get("value")
                .ok_or_else(|| "slot setState args is missing `value`".to_string())?;
            bind_write(slot.to_string(), value, scope).map(BoundEffect::Write)
        }
        "setState" => {
            Err("setState may target only @impact.target or a bare store slot".to_string())
        }
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

fn plan_effect(effect: &BoundEffect, scope: &EntityScope) -> PlannedEffect {
    match effect {
        BoundEffect::Write(program) => PlannedEffect::Write {
            handle: program
                .output
                .as_ref()
                .expect("bound impact write has an output handle")
                .clone(),
            value: eval_value(program, scope),
        },
        BoundEffect::SetHealth { value, after_ms } => {
            PlannedEffect::Command(ImpactEffect::SetHealth {
                value: number(eval_value(value, scope)),
                after_ms: *after_ms,
            })
        }
        BoundEffect::Despawn { after_ms } => PlannedEffect::Command(ImpactEffect::Despawn {
            after_ms: *after_ms,
        }),
        BoundEffect::PlayAnimation { state } => {
            PlannedEffect::Command(ImpactEffect::PlayAnimation {
                state: state.clone(),
            })
        }
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

fn require_impact_target(target: Option<&str>, primitive: &str) -> Result<(), String> {
    if target == Some("@impact.target") {
        Ok(())
    } else {
        Err(format!("{primitive} must target @impact.target"))
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
    use postretro_entities::Transform;
    use postretro_entities::components::health::{
        DamageContext, HealthComponent, apply_damage_with_context,
    };
    use postretro_entities::data_descriptors::HealthDescriptor;
    use postretro_entities::slot_table::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_foundation::DamagePayload;
    use serde_json::json;

    fn event(id: &str, tag: &str, policy: Vec<Value>) -> ImpactEventDescriptor {
        ImpactEventDescriptor {
            id: id.to_string(),
            filter_tag: Some(tag.to_string()),
            policy,
        }
    }

    fn input(name: &str) -> Value {
        json!({ "op": "input", "name": name })
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

    fn store_write(slot: &str, value: Value) -> Value {
        json!({
            "primitive": "setState",
            "args": { "slot": slot, "value": value },
        })
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
            accumulate: None,
        })
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

    fn hit(ctx: &ScriptCtx, target: EntityId, producer: DamageProducer) {
        let mut context = DamageContext::new("impact-policy-test");
        context.producer = producer;
        apply_damage_with_context(
            &mut ctx.registry.borrow_mut(),
            target,
            &DamagePayload { amount: 1.0 },
            context,
        );
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
                        store_write("impact.broken", json!({ "op": "add", "a": input("impact.broken"), "b": number(1.0) })),
                        { "primitive": "despawn", "target": "@impact.target", "args": {} },
                    ],
                }),
            ],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        runtime.evaluate_pending();
        assert_eq!(state(&ctx, target, "hits"), 1.0);
        assert_eq!(store(&ctx, "impact.broken"), 0.0);

        hit(&ctx, target, DamageProducer::InTick);
        runtime.evaluate_pending();
        assert_eq!(state(&ctx, target, "hits"), 2.0);
        assert_eq!(store(&ctx, "impact.broken"), 0.0);

        hit(&ctx, target, DamageProducer::InTick);
        runtime.evaluate_pending();
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
        runtime.evaluate_pending();

        assert_eq!(state(&ctx, target, "first"), 1.0);
        assert_eq!(state(&ctx, target, "second"), 1.0);
    }

    #[test]
    fn matching_override_uses_last_registered_policy_only() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate", "reinforced"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "same",
            "crate",
            vec![state_write("base_only", number(1.0))],
        )]);
        runtime.replace_level_events(vec![event(
            "same",
            "reinforced",
            vec![state_write("variant", number(3.0))],
        )]);

        hit(&ctx, target, DamageProducer::InTick);
        runtime.evaluate_pending();

        assert_eq!(state(&ctx, target, "base_only"), 0.0);
        assert_eq!(state(&ctx, target, "variant"), 3.0);
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
        runtime.evaluate_pending();

        assert_eq!(state(&ctx, target, "crate"), 1.0);
        assert_eq!(state(&ctx, target, "vase"), 1.0);
    }

    #[test]
    fn app_drain_dispatches_are_consumed_without_policy_evaluation() {
        let ctx = ScriptCtx::new();
        let target = target(&ctx, &["crate"]);
        let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
        runtime.replace_global_events(vec![event(
            "app-drain",
            "crate",
            vec![state_write("ran", number(1.0))],
        )]);

        hit(&ctx, target, DamageProducer::AppDrain);
        runtime.evaluate_pending();

        assert_eq!(state(&ctx, target, "ran"), 0.0);
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
        runtime.evaluate_pending();

        assert_eq!(state(&ctx, target, "order"), 2.0);
    }
}
