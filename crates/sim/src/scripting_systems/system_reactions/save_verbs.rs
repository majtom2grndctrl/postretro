// Save verbs and `checkpoint`: the registry handlers that turn their wire args
// into typed commands for the app drain. What each does is the app's.
// See: context/lib/scripting.md §10.4 · context/lib/scripting.md §5

use postretro_entities::FireOrigin;
use postretro_entities::engine_state_catalog::TEXT_ENTRY_SLOT;
use postretro_scripting_core::data_descriptors::{
    AuthoredRowBase, SaveVerbArgs, parse_save_verb_args,
};
use postretro_scripting_core::reaction_registry::{
    ReactionError, SystemCommandQueue, SystemReactionCommand, SystemReactionRegistry,
};

/// Register `continueRun`, `newRun`, `deleteRun`, `selectCharacter`,
/// `createCharacter`, `deleteCharacter` and `checkpoint`. Each carries its
/// fire's source and origin so the app drain can refuse it by scene and role
/// and log that once per source.
pub(super) fn register_save_verb_primitives(registry: &mut SystemReactionRegistry) {
    register_row_verb(registry, "continueRun", |index, dispatch_source, origin| {
        SystemReactionCommand::ContinueRun {
            index,
            dispatch_source,
            origin,
        }
    });
    register_row_verb(registry, "deleteRun", |index, dispatch_source, origin| {
        SystemReactionCommand::DeleteRun {
            index,
            dispatch_source,
            origin,
        }
    });
    register_row_verb(
        registry,
        "selectCharacter",
        |index, dispatch_source, origin| SystemReactionCommand::SelectCharacter {
            index,
            dispatch_source,
            origin,
        },
    );
    registry.register("createCharacter", |args, queue| {
        let SaveVerbArgs::Row(index) = parse(args, "createCharacter")? else {
            unreachable!("`createCharacter` parses to a slot row");
        };
        let context = queue.fire_context();
        queue.push(SystemReactionCommand::CreateCharacter {
            index,
            name: queue.text_at_dispatch(TEXT_ENTRY_SLOT),
            dispatch_source: context.source,
            origin: context.origin,
        });
        Ok(())
    });
    register_row_verb(
        registry,
        "deleteCharacter",
        |index, dispatch_source, origin| SystemReactionCommand::DeleteCharacter {
            index,
            dispatch_source,
            origin,
        },
    );
    registry.register("newRun", |args, queue| {
        let SaveVerbArgs::NewRun { index, map } = parse(args, "newRun")? else {
            unreachable!("`newRun` parses to its own args");
        };
        let context = queue.fire_context();
        queue.push(SystemReactionCommand::NewRun {
            index,
            map,
            dispatch_source: context.source,
            origin: context.origin,
        });
        Ok(())
    });
    registry.register("checkpoint", |args, queue| {
        let SaveVerbArgs::Checkpoint { name } = parse(args, "checkpoint")? else {
            unreachable!("`checkpoint` parses to its own args");
        };
        let context = queue.fire_context();
        queue.push(SystemReactionCommand::Checkpoint {
            name,
            dispatch_source: context.source,
            origin: context.origin,
        });
        Ok(())
    });
}

/// A verb whose only argument is a slot row.
fn register_row_verb(
    registry: &mut SystemReactionRegistry,
    verb: &'static str,
    command: fn(usize, String, FireOrigin) -> SystemReactionCommand,
) {
    registry.register(verb, move |args, queue: &SystemCommandQueue| {
        let SaveVerbArgs::Row(index) = parse(args, verb)? else {
            unreachable!("`{verb}` parses to a slot row");
        };
        let context = queue.fire_context();
        queue.push(command(index, context.source, context.origin));
        Ok(())
    });
}

/// The wire index is 0-based in both runtimes: the Luau SDK has already
/// converted its 1-based `forEach` row. Reaction parsing refuses bad args
/// first; this guards a descriptor that reached dispatch some other way.
fn parse(args: &serde_json::Value, verb: &str) -> Result<SaveVerbArgs, ReactionError> {
    parse_save_verb_args(verb, args, AuthoredRowBase::Zero)
        .expect("a registered save verb is a save verb")
        .map_err(|reason| ReactionError::InvalidArgument { reason })
}
