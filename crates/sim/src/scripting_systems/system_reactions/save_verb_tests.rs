// Save verbs and `checkpoint` from SDK call to typed command, in both runtimes,
// as a reaction body and as a sequence step.
// See: context/lib/scripting.md §10.4

use postretro_entities::{FireOrigin, MachineSource, NamedReaction, ScriptCtx};
use postretro_scripting_core::data_descriptors::{named_reaction_from_js, named_reaction_from_lua};
use postretro_scripting_core::reaction_dispatch::{
    NamedEventDispatchContext, fire_named_event_with_sequences, validate_sequence_primitives,
};
use postretro_scripting_core::reaction_registry::{
    ReactionError, ReactionPrimitiveRegistry, SystemCommandQueue, SystemReactionCommand,
    SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use serde_json::json;

use super::register_system_reaction_primitives;

const SOURCE: &str = "named:r";

fn press() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}

fn typescript_reaction(body: &str) -> NamedReaction {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        postretro_scripting_core::quickjs::evaluate_prelude(&ctx).unwrap();
        let value: rquickjs::Value = ctx.eval(format!("defineReaction('r', {body})")).unwrap();
        named_reaction_from_js(&ctx, value).unwrap_or_else(|error| panic!("`{body}`: {error}"))
    })
}

fn luau_reaction(body: &str) -> NamedReaction {
    let lua = postretro_scripting_core::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .unwrap();
    let value: mlua::Value = lua
        .load(format!(
            "local Postretro = require('postretro')\nlocal UI = require('postretro/ui')\nreturn Postretro.defineReaction('r', {body})"
        ))
        .eval()
        .unwrap();
    named_reaction_from_lua(value).unwrap_or_else(|error| panic!("`{body}`: {error}"))
}

/// Install `reaction` as a level does, fire it by name from a UI press, and
/// return what it queued for the app drain.
fn fire(reaction: NamedReaction) -> Vec<SystemReactionCommand> {
    fire_on(&ScriptCtx::new(), reaction)
}

/// [`fire`] on a given context.
fn fire_on(ctx: &ScriptCtx, reaction: NamedReaction) -> Vec<SystemReactionCommand> {
    let sequence_registry = SequencedPrimitiveRegistry::new();
    let installed = validate_sequence_primitives(vec![reaction], &sequence_registry);
    assert_eq!(installed.len(), 1, "the reaction installs");
    ctx.data_registry
        .borrow_mut()
        .populate_level(installed, Vec::new(), &[]);
    let mut system_registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut system_registry);
    let _ = fire_named_event_with_sequences(
        "r",
        &ctx.data_registry.borrow(),
        &sequence_registry,
        &ReactionPrimitiveRegistry::new(),
        &system_registry,
        ctx,
        Some(NamedEventDispatchContext {
            origin: press(),
            source: SOURCE.into(),
            values: &[],
            emitter: None,
        }),
    );
    ctx.system_commands.take()
}

#[test]
fn save_verb_sdk_calls_queue_their_exact_command_in_both_runtimes_as_body_and_step() {
    let source = || SOURCE.to_string();
    // TypeScript passes the 0-based `forEach` row; Luau the 1-based one, which
    // its SDK converts before it crosses.
    let cases = [
        (
            "continueRun(0)",
            "UI.continueRun(1)",
            SystemReactionCommand::ContinueRun {
                index: 0,
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "newRun(2, 'campaign-test')",
            "UI.newRun(3, 'campaign-test')",
            SystemReactionCommand::NewRun {
                index: 2,
                map: "campaign-test".into(),
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "deleteRun(15)",
            "UI.deleteRun(16)",
            SystemReactionCommand::DeleteRun {
                index: 15,
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "selectCharacter(1)",
            "UI.selectCharacter(2)",
            SystemReactionCommand::SelectCharacter {
                index: 1,
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "createCharacter(3)",
            "UI.createCharacter(4)",
            SystemReactionCommand::CreateCharacter {
                index: 3,
                name: None,
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "deleteCharacter(7)",
            "UI.deleteCharacter(8)",
            SystemReactionCommand::DeleteCharacter {
                index: 7,
                dispatch_source: source(),
                origin: press(),
            },
        ),
        (
            "checkpoint('bridge')",
            "UI.checkpoint('bridge')",
            SystemReactionCommand::Checkpoint {
                name: "bridge".into(),
                dispatch_source: source(),
                origin: press(),
            },
        ),
    ];
    for (typescript, luau, expected) in cases {
        for (label, reaction) in [
            (typescript.to_string(), typescript_reaction(typescript)),
            (
                format!("[{typescript}]"),
                typescript_reaction(&format!("[{typescript}]")),
            ),
            (luau.to_string(), luau_reaction(luau)),
            (
                format!("{{ {luau} }}"),
                luau_reaction(&format!("{{ {luau} }}")),
            ),
        ] {
            assert_eq!(fire(reaction), vec![expected.clone()], "{label}");
        }
    }
}

// A descriptor that reached dispatch without reaction parsing is still
// refused there, naming the verb and the value, and queues nothing.
#[test]
fn save_verb_dispatch_refuses_bad_wire_args_naming_verb_and_value() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();
    for (verb, args, detail) in [
        (
            "continueRun",
            json!({ "index": 16 }),
            "`continueRun` row index must be a whole number from 0 to 15, got 16",
        ),
        (
            "selectCharacter",
            json!({ "index": 0.5 }),
            "`selectCharacter` row index must be a whole number from 0 to 15, got 0.5",
        ),
        (
            "deleteCharacter",
            json!({ "index": -2 }),
            "`deleteCharacter` row index must be a whole number from 0 to 15, got -2",
        ),
        (
            "newRun",
            json!({ "index": 0, "map": "" }),
            "`newRun` level (a catalog map id) must be a non-empty string",
        ),
        (
            "checkpoint",
            json!({}),
            "`checkpoint` name must be a non-empty string, got nothing",
        ),
    ] {
        match registry.dispatch(verb, &args, &queue) {
            Err(ReactionError::InvalidArgument { reason }) => {
                assert!(reason.starts_with(detail), "`{verb}` {args}: {reason}");
            }
            other => panic!("`{verb}` {args}: expected a refusal, got {other:?}"),
        }
    }
    assert!(queue.take().is_empty(), "a refused verb queues nothing");
}

// Play-test regression: a body that empties the entry after `createCharacter`
// (the dev menu did) unnamed the character, because the edit applies at
// dispatch and the verb read the slot at the drain. The name is captured when
// the verb dispatches, in both runtimes.
#[test]
fn create_character_keeps_the_name_typed_when_its_reaction_fired() {
    use postretro_entities::slot_table::SlotValue;
    for reaction in [
        typescript_reaction("[createCharacter(2), clearText({ slot: 'ui.textEntry' })]"),
        luau_reaction("{ UI.createCharacter(3), UI.clearText({ slot = 'ui.textEntry' }) }"),
    ] {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .get_mut("ui.textEntry")
            .expect("the engine declares the text entry")
            .write_value(Some(SlotValue::String("Vex".into())));
        super::SystemStateSink::install(&ctx, std::rc::Rc::default());
        let commands = fire_on(&ctx, reaction);
        assert_eq!(
            commands,
            vec![SystemReactionCommand::CreateCharacter {
                index: 2,
                name: Some("Vex".into()),
                dispatch_source: SOURCE.into(),
                origin: press(),
            }]
        );
        assert_eq!(
            ctx.slot_table.borrow().get("ui.textEntry").unwrap().value,
            Some(SlotValue::String(String::new())),
            "the clear still applied"
        );
    }
}

// The capture is the entry as the body's earlier steps left it, not as the
// body found it.
#[test]
fn create_character_captures_edits_made_earlier_in_its_body() {
    use postretro_entities::slot_table::SlotValue;
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .get_mut("ui.textEntry")
        .expect("the engine declares the text entry")
        .write_value(Some(SlotValue::String("stale".into())));
    super::SystemStateSink::install(&ctx, std::rc::Rc::default());
    let commands = fire_on(
        &ctx,
        typescript_reaction(
            "[clearText({ slot: 'ui.textEntry' }), appendText({ slot: 'ui.textEntry' }, 'Kai'), createCharacter(0)]",
        ),
    );
    assert_eq!(
        commands,
        vec![SystemReactionCommand::CreateCharacter {
            index: 0,
            name: Some("Kai".into()),
            dispatch_source: SOURCE.into(),
            origin: press(),
        }]
    );
}
