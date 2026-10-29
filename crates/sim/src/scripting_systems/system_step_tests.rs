// System steps against their bodies: every system reaction kind, as a body and
// as a sequence step, through every source that fires reactions on the frame-
// end drain or binds them into a tick.
// See: context/lib/scripting.md §10.4 (System steps)

#![cfg(test)]

use postretro_entities::reactions::system_commands::SystemReactionKind;
use postretro_entities::{
    FireOrigin, FireSubject, MachineSource, NamedReaction, NumericRange, PrimitiveDescriptor,
    ReactionDescriptor, ScriptCtx, SequenceStep, SequenceTarget, SlotOwnership, SlotRecord,
    SlotSchema, SlotTable, SlotType, SlotValue, SystemCommandFireContext,
};
use postretro_foundation::Seat;
use postretro_scripting_core::reaction_dispatch::{
    NamedEventDispatchContext, ResidualOrigin, fire_named_event_with_sequences,
    fire_prepartitioned_reactions_with_sequences, validate_sequence_primitives,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingFire;
use serde_json::json;

use super::system_reactions::register_system_reaction_primitives;
use crate::trigger_bindings::{BindSite, partition_direct_reaction};

/// Valid args for each kind. Exhaustive, so a new kind fails to build here
/// until it has a step proof.
fn sample_args(kind: SystemReactionKind) -> serde_json::Value {
    use SystemReactionKind::*;
    match kind {
        PlaySound => json!({ "sound": "x" }),
        Rumble => json!({ "strong": 0.5, "durationMs": 100 }),
        FlashScreen => json!({ "color": [1, 0, 0, 1], "durationMs": 50 }),
        Vignette => json!({ "strength": 0.5, "durationMs": 100 }),
        ScreenShake => json!({ "amplitude": 4, "durationMs": 100 }),
        ShowDialog => json!({ "tree": "dialog" }),
        OpenMenu => json!({ "tree": "menu" }),
        CloseDialog | RestartLevel | ReturnToFrontend => json!({}),
        LoadLevel => json!({ "map": "next" }),
        SetState => json!({ "slot": "step.flag", "value": 1 }),
        SetSentiment => json!({ "from": "a", "to": "b", "value": 0.5 }),
        AdjustSentiment => json!({ "from": "a", "to": "b", "delta": 0.5 }),
        SetGhostAudio => json!({ "volume": 0.5 }),
        CellWrite => json!({ "scope": "hud", "cell": "c", "value": 1 }),
        AppendText => json!({ "slot": "step.text", "text": "a" }),
        BackspaceText | ClearText => json!({ "slot": "step.text" }),
    }
}

fn body(name: &str, kind: SystemReactionKind) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: kind.primitive_name().into(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args: sample_args(kind),
        }),
    }
}

fn step(name: &str, kind: SystemReactionKind) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::System,
            primitive: kind.primitive_name().into(),
            args: sample_args(kind),
        }]),
    }
}

fn slot(slot_type: SlotType, default: SlotValue) -> SlotRecord {
    let range =
        matches!(slot_type, SlotType::Number).then_some(NumericRange { min: 0.0, max: 1.0 });
    SlotRecord::new(SlotSchema {
        slot_type,
        default: Some(default),
        range,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: Default::default(),
        per_owner: false,
        accumulate: None,
    })
}

fn slots() -> SlotTable {
    let mut slots = SlotTable::new();
    slots
        .insert_namespace(
            "step",
            vec![
                (
                    "flag".into(),
                    slot(SlotType::Number, SlotValue::Number(0.0)),
                ),
                (
                    "text".into(),
                    slot(SlotType::String, SlotValue::String(String::new())),
                ),
            ],
        )
        .unwrap();
    slots
}

struct Fixture {
    ctx: ScriptCtx,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
}

impl Fixture {
    fn new(reaction: NamedReaction) -> Self {
        let ctx = ScriptCtx::new();
        *ctx.slot_table.borrow_mut() = slots();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let sequence_registry = SequencedPrimitiveRegistry::new();
        // A system step installs through the same admission a level runs.
        let installed = validate_sequence_primitives(vec![reaction], &sequence_registry);
        assert_eq!(installed.len(), 1, "the reaction installs");
        ctx.data_registry
            .borrow_mut()
            .populate_level(installed, Vec::new(), &[]);
        Self {
            ctx,
            sequence_registry,
            reaction_registry: ReactionPrimitiveRegistry::new(),
            system_registry,
        }
    }

    fn fire(&self, name: &str, origin: Option<FireOrigin>) -> Vec<String> {
        let _ = fire_named_event_with_sequences(
            name,
            &self.ctx.data_registry.borrow(),
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
            origin.map(|origin| NamedEventDispatchContext {
                origin,
                source: format!("named:{name}"),
                values: &[],
                emitter: None,
            }),
        );
        self.commands()
    }

    fn cross(&self, name: &str) -> Vec<String> {
        let _ = crate::scripting::reactions::dispatch_crossing_fires_with_sequences(
            vec![CrossingFire {
                reaction: name.into(),
                rising: true,
                source_id: 0,
            }],
            &self.ctx.data_registry.borrow(),
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
        );
        self.commands()
    }

    /// Bind as a trigger or teleporter does at install, then drain the
    /// residual under that source's fire context, as the frame end does.
    fn bind_and_drain(&self, site: BindSite, origin: FireOrigin) -> (String, Vec<String>) {
        let reaction = self.ctx.data_registry.borrow().reactions[0].clone();
        let mut commands = Vec::new();
        let mut steps = Vec::new();
        partition_direct_reaction(
            &reaction,
            0,
            site,
            &self.ctx.data_registry.borrow(),
            &self.ctx.slot_table.borrow(),
            Some(&self.ctx),
            &mut commands,
            &mut steps,
        );
        let previous = self
            .ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: "residual".into(),
                origin,
                ..Default::default()
            });
        let _ = fire_prepartitioned_reactions_with_sequences(
            &steps,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
            ResidualOrigin::TriggerBinding,
        );
        self.ctx.system_commands.replace_fire_context(previous);
        (format!("{commands:?}"), self.commands())
    }

    /// The queued commands with their recipients, fire-time source context
    /// left out: it names the reaction body, not what the command does.
    fn commands(&self) -> Vec<String> {
        self.ctx
            .system_commands
            .take_ordered()
            .into_iter()
            .map(|(recipients, mut command)| {
                if let SystemReactionCommand::SetState {
                    dispatch_source, ..
                } = &mut command
                {
                    dispatch_source.clear();
                }
                format!("{recipients:?} {command:?}")
            })
            .collect()
    }
}

fn trigger_origin() -> FireOrigin {
    FireOrigin::HostOnly(FireSubject::Trigger {
        player: Some(Seat(1)),
        trigger: postretro_entities::EntityId::from_raw(9),
        occupants: vec![Seat(1)],
    })
}

fn teleport_origin() -> FireOrigin {
    FireOrigin::HostOnly(FireSubject::Teleport {
        player: Some(Seat(1)),
        teleporter: postretro_entities::EntityId::from_raw(9),
    })
}

#[test]
fn every_system_reaction_kind_as_a_step_does_what_its_body_does_from_every_source() {
    for &kind in SystemReactionKind::ALL {
        let name = kind.primitive_name();
        let as_body = Fixture::new(body("r", kind));
        let as_step = Fixture::new(step("r", kind));

        let named = as_body.fire("r", None);
        assert!(!named.is_empty(), "`{name}` body queues its command");
        assert_eq!(as_step.fire("r", None), named, "`{name}` fired by name");

        let level_load = Some(FireOrigin::EveryMachine(MachineSource::HostTimeline));
        let as_body_load = Fixture::new(body("levelLoad", kind));
        let as_step_load = Fixture::new(step("levelLoad", kind));
        assert_eq!(
            as_step_load.fire("levelLoad", level_load.clone()),
            as_body_load.fire("levelLoad", level_load),
            "`{name}` from levelLoad"
        );

        assert_eq!(
            as_step.cross("r"),
            as_body.cross("r"),
            "`{name}` by crossing"
        );

        for (site, origin, source) in [
            (BindSite::Trigger, trigger_origin(), "trigger"),
            (BindSite::Teleport, teleport_origin(), "teleport"),
        ] {
            let (body_bound, body_drained) = as_body.bind_and_drain(site, origin.clone());
            let (step_bound, step_drained) = as_step.bind_and_drain(site, origin);
            assert_eq!(
                step_bound, body_bound,
                "`{name}` binds in-tick alike by {source}"
            );
            assert_eq!(
                step_drained, body_drained,
                "`{name}` drains alike by {source}"
            );
            assert!(
                body_bound != "[]" || !body_drained.is_empty(),
                "`{name}` has an effect by {source}"
            );
        }
    }
}
