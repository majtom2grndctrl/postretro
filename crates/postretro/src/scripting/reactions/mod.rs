// Scripting reaction dispatch adapters and compatibility re-exports.
// See: context/lib/scripting.md §10
#![allow(unused_imports)]

use postretro_entities::{DataRegistry, ScriptCtx, SlotTable};
use postretro_foundation::IrValue;
use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

pub(crate) use crate::fx::emitter_reactions::{set_emitter_rate, set_spin_rate};
pub(crate) use crate::fx::fog_reactions::{
    set_fog_animation, set_fog_density, set_fog_edge_softness, set_fog_falloff, set_fog_glow,
    set_fog_params,
};
pub(crate) use crate::health::reactions as apply_damage;
pub(crate) use animation as set_animation_state;

pub(crate) mod animation;
pub(crate) mod registry;
pub(crate) mod system_commands;

#[cfg(test)]
pub(crate) mod log_capture;

pub(crate) use postretro_scripting_core::reaction_registry::ReactionError;

/// Detect settled state crossings and synchronously dispatch their named reactions.
/// App owns the surrounding frame and system-command drains; this owns their shared dispatch seam.
pub(crate) fn dispatch_state_crossings_with_sequences(
    crossing_detector: &mut CrossingDetector,
    slot_table: &SlotTable,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let crossing_events = crossing_detector.detect(slot_table);
    for fire in &crossing_events {
        let dispatch_values = [("@rising".to_string(), IrValue::Bool(fire.rising))];
        let _ = fire_named_event_with_sequences(
            &fire.reaction,
            data_registry,
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
            Some(&dispatch_values),
        );
    }
    crossing_events
        .into_iter()
        .map(|fire| fire.reaction)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        CrossingCondition, CrossingDescriptor, NamedReaction, NumericRange, PrimitiveDescriptor,
        ReactionDescriptor, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_scripting_core::reaction_registry::SystemReactionCommand;

    use crate::scripting_systems::system_reactions::{
        SystemReactionIrBindings, SystemReactionIrDispatch, register_system_reaction_primitives,
    };

    fn number_slot(default: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(default)),
            range: Some(NumericRange {
                min: 0.0,
                max: 10.0,
            }),
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: Default::default(),
            accumulate: None,
        })
    }

    fn system_reaction(name: &str, primitive: &str, args: serde_json::Value) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: primitive.to_string(),
                tag: None,
                on_complete: None,
                args,
            }),
        }
    }

    fn drain_direction_write(
        ctx: &ScriptCtx,
        bindings: &SystemReactionIrBindings,
    ) -> (SystemReactionIrDispatch, usize) {
        let mut outcome = None;
        let mut unrelated_commands = 0;
        for command in ctx.system_commands.take() {
            match command {
                SystemReactionCommand::SetState {
                    slot,
                    value,
                    dispatch_values,
                } => {
                    outcome = Some(bindings.dispatch(&slot, &value, &dispatch_values, ctx));
                }
                SystemReactionCommand::PlaySound { .. } => unrelated_commands += 1,
                other => panic!("unexpected command in crossing fixture: {other:?}"),
            }
        }
        (
            outcome.expect("direction reaction enqueues one setState"),
            unrelated_commands,
        )
    }

    #[test]
    fn both_edge_crossing_carries_direction_while_sourceless_dispatch_skips_it() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(
                "acceptance",
                vec![
                    ("source".to_string(), number_slot(0.0)),
                    ("direction".to_string(), number_slot(7.0)),
                ],
            )
            .unwrap();
        let direction_value = serde_json::json!({
            "op": "select",
            "cond": { "op": "input", "name": "@rising" },
            "a": { "op": "const", "value": 1 },
            "b": { "op": "const", "value": 0 }
        });
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                system_reaction(
                    "levelLoad",
                    "setState",
                    serde_json::json!({
                        "slot": "acceptance.direction",
                        "value": direction_value
                    }),
                ),
                system_reaction(
                    "levelLoad",
                    "playSound",
                    serde_json::json!({ "sound": "unrelated" }),
                ),
            ],
            vec![CrossingDescriptor {
                slot: Some("acceptance.source".to_string()),
                condition: CrossingCondition::Above { threshold: 0.5 },
                max: 1.0,
                edge: Some("both".to_string()),
                fire: vec!["levelLoad".to_string()],
            }],
            &[],
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let mut bindings = SystemReactionIrBindings::default();
        bindings.rebuild(&data, &ctx);
        let mut detector = CrossingDetector::new();
        detector.initialize(&data, &ctx.slot_table.borrow(), &ctx);

        fire_named_event_with_sequences(
            "levelLoad",
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &ctx,
            None,
        );
        assert_eq!(
            drain_direction_write(&ctx, &bindings),
            (SystemReactionIrDispatch::Rejected, 1),
            "levelLoad publishes no @rising, but unrelated reactions still dispatch"
        );
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("acceptance.direction")
                .unwrap()
                .value,
            Some(SlotValue::Number(7.0))
        );

        for (source, expected_direction) in [(1.0, 1.0), (0.0, 0.0)] {
            ctx.slot_table
                .borrow_mut()
                .get_mut("acceptance.source")
                .unwrap()
                .value = Some(SlotValue::Number(source));
            assert_eq!(
                dispatch_state_crossings_with_sequences(
                    &mut detector,
                    &ctx.slot_table.borrow(),
                    &data,
                    &sequence_registry,
                    &reaction_registry,
                    &system_registry,
                    &ctx,
                ),
                vec!["levelLoad".to_string()]
            );
            assert_eq!(
                drain_direction_write(&ctx, &bindings),
                (SystemReactionIrDispatch::Evaluated, 1)
            );
            assert_eq!(
                ctx.slot_table
                    .borrow()
                    .get("acceptance.direction")
                    .unwrap()
                    .value,
                Some(SlotValue::Number(expected_direction)),
                "select(on.rising, 1, 0) observes the crossing's value direction"
            );
        }
    }
}
