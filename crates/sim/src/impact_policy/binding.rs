// Impact-policy binding: authored policy JSON to bound groups and effects,
// and each fire's effect planning against the frozen dispatch scope.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §11

use super::*;

pub(super) struct BoundImpactPolicy {
    pub(super) id: String,
    pub(super) base_filter_tag: Option<String>,
    pub(super) filter_tag: Option<String>,
    pub(super) groups: Vec<BoundGroup>,
}

pub(super) struct BoundGroup {
    pub(super) when: Option<BoundProgram<EntityScope>>,
    pub(super) effects: Vec<BoundEffect>,
}

pub(super) enum BoundEffect {
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
    GrantLayer {
        layer: String,
        amount: Option<BoundProgram<EntityScope>>,
    },
    AdjustSentiment {
        from_recipient: CommandRecipient,
        to_recipient: CommandRecipient,
        delta: BoundProgram<EntityScope>,
    },
    SetLifeState {
        state: String,
    },
}

/// Which opaque impact command-target token a planned effect resolves at apply.
///
/// Numeric IR operands remain bound to the impact target scope; this only
/// chooses the entity that receives a command after evaluation completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandRecipient {
    Target,
    Source,
}

/// The recipient shape of a planned command. Most impact effects retain the
/// original single-recipient contract; faction sentiment deliberately carries
/// both directional endpoints all the way from bind through application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandRecipients {
    One(CommandRecipient),
    Pair {
        from_recipient: CommandRecipient,
        to_recipient: CommandRecipient,
    },
}

pub(super) enum PlannedEffect {
    Write {
        recipient: CommandRecipient,
        handle: EntityOutputHandle,
        value: IrValue,
    },
    Command {
        recipients: CommandRecipients,
        effect: ImpactEffect,
    },
}

pub(super) fn bind_policy(
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

pub(super) fn qualified_impact_event_id(mod_id: Option<&str>, authored_id: &str) -> String {
    match mod_id {
        Some(mod_id) => format!("{mod_id}:{authored_id}"),
        None => authored_id.to_string(),
    }
}

pub(super) fn bind_group(entry: &Value, scope: &EntityScope) -> Result<BoundGroup, String> {
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

pub(super) fn bind_effect(entry: &Value, scope: &EntityScope) -> Result<BoundEffect, String> {
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
        "grantLayer" => {
            require_impact_token(target, primitive, IMPACT_SOURCE_TOKEN)?;
            let handle = postretro_foundation::defense::HandleRef::from_json(
                args.get("layer")
                    .ok_or_else(|| "grantLayer args is missing `layer`".to_string())?,
            )
            .map_err(|error| format!("grantLayer `layer`: {error}"))?;
            let layer = handle
                .expect_layer()
                .map_err(|error| format!("grantLayer `layer`: {error}"))?;
            if !scope.declares_defense_layer(layer) {
                return Err(format!(
                    "grantLayer `layer`: `{layer}` is not a declared defense layer"
                ));
            }
            let amount = match args.get("amount") {
                None => None,
                Some(value) => {
                    let amount = bind_read(value, scope)?;
                    if amount.root_type != IrType::Number {
                        return Err("grantLayer `amount` must evaluate to a number".to_string());
                    }
                    Some(amount)
                }
            };
            Ok(BoundEffect::GrantLayer {
                layer: layer.to_string(),
                amount,
            })
        }
        "adjustSentiment" => {
            let from_recipient = impact_recipient(target, primitive)?;
            let to_recipient = impact_recipient(
                Some(required_string(args, "toward", "adjustSentiment args")?),
                "adjustSentiment toward",
            )?;
            let delta = bind_read(
                args.get("delta")
                    .ok_or_else(|| "adjustSentiment args is missing `delta`".to_string())?,
                scope,
            )?;
            if delta.root_type != IrType::Number {
                return Err("adjustSentiment `delta` must evaluate to a number".to_string());
            }
            Ok(BoundEffect::AdjustSentiment {
                from_recipient,
                to_recipient,
                delta,
            })
        }
        "setLifeState" => source_life_state::bind_set_life_state(target, args, scope),
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

pub(super) fn bind_read(
    value: &Value,
    scope: &EntityScope,
) -> Result<BoundProgram<EntityScope>, String> {
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

pub(super) fn bind_write(
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

pub(super) fn bind_number_write(
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

pub(super) fn plan_effect(effect: &BoundEffect, scope: &EntityScope) -> PlannedEffect {
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
            recipients: CommandRecipients::One(CommandRecipient::Source),
            effect: ImpactEffect::SetOwnerSlot {
                slot: slot.clone(),
                value: number(eval_value(value, scope)),
            },
        },
        BoundEffect::SetHealth { value, after_ms } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Target),
            effect: ImpactEffect::SetHealth {
                value: number(eval_value(value, scope)),
                after_ms: *after_ms,
            },
        },
        BoundEffect::Despawn { after_ms } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Target),
            effect: ImpactEffect::Despawn {
                after_ms: *after_ms,
            },
        },
        BoundEffect::PlayAnimation { state } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Target),
            effect: ImpactEffect::PlayAnimation {
                state: state.clone(),
            },
        },
        BoundEffect::Present { template, value } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Target),
            effect: ImpactEffect::Present {
                template: template.clone(),
                value: number(eval_value(value, scope)),
            },
        },
        BoundEffect::GrantHealth { amount } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Source),
            effect: ImpactEffect::GrantHealth {
                amount: number(eval_value(amount, scope)),
            },
        },
        BoundEffect::GrantAmmo { pool, amount } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Source),
            effect: ImpactEffect::GrantAmmo {
                pool: pool.clone(),
                amount: number(eval_value(amount, scope)),
            },
        },
        BoundEffect::GrantLayer { layer, amount } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Source),
            effect: ImpactEffect::GrantLayer {
                layer: layer.clone(),
                amount: amount
                    .as_ref()
                    .map(|amount| number(eval_value(amount, scope))),
            },
        },
        BoundEffect::AdjustSentiment {
            from_recipient,
            to_recipient,
            delta,
        } => PlannedEffect::Command {
            recipients: CommandRecipients::Pair {
                from_recipient: *from_recipient,
                to_recipient: *to_recipient,
            },
            effect: ImpactEffect::AdjustSentiment {
                delta: number(eval_value(delta, scope)),
            },
        },
        BoundEffect::SetLifeState { state } => PlannedEffect::Command {
            recipients: CommandRecipients::One(CommandRecipient::Source),
            effect: ImpactEffect::SetLifeState {
                state: state.clone(),
            },
        },
    }
}

pub(super) fn object<'a>(
    value: &'a Value,
    context: &str,
) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))
}

pub(super) fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} is missing string `{key}`"))
}

pub(super) fn optional_ms(args: &Map<String, Value>) -> Result<Option<f32>, String> {
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

pub(super) fn require_impact_token(
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

pub(super) fn impact_recipient(
    token: Option<&str>,
    context: &str,
) -> Result<CommandRecipient, String> {
    match token {
        Some(IMPACT_TARGET_TOKEN) => Ok(CommandRecipient::Target),
        Some(IMPACT_SOURCE_TOKEN) => Ok(CommandRecipient::Source),
        _ => Err(format!(
            "{context} must target {IMPACT_TARGET_TOKEN} or {IMPACT_SOURCE_TOKEN}"
        )),
    }
}

pub(super) fn number(value: IrValue) -> f32 {
    match value {
        IrValue::Number(value) => value,
        IrValue::Bool(_) => 0.0,
    }
}
