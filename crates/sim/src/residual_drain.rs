//! The frame-end drain of in-tick sources' residual work: trigger residuals,
//! then player-event residuals, each in tick order and authored order.
//! See: context/lib/scripting.md §12 (Player events)

use postretro_entities::{EntityId, ScriptCtx, SystemCommandFireContext};
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use crate::player_events::{PlayerEventResidual, PlayerEventTable, PlayerKey};
use crate::scripting_systems::reaction_scheduler::ReactionScheduler;
use crate::trigger_bindings::{TriggerBindingTable, TriggerResidualHandle};
use crate::trigger_system::{PlayerId, TriggerSystem};

/// The reaction registries a residual dispatches through.
#[derive(Clone, Copy)]
pub struct ResidualRegistries<'a> {
    pub sequence: &'a SequencedPrimitiveRegistry,
    pub reaction: &'a ReactionPrimitiveRegistry,
    pub system: &'a SystemReactionRegistry,
}

/// Run every residual one frame's ticks left, appending follow-up addresses
/// to `follow_ups`. Trigger residuals drain first, then player-event residuals:
/// within each tick a player event evaluates after the triggers have fired, so
/// the drain keeps that order source by source.
#[allow(clippy::too_many_arguments)]
pub fn drain_frame_residuals(
    trigger_residuals: &[(TriggerResidualHandle, EntityId, PlayerId)],
    trigger_bindings: &TriggerBindingTable,
    trigger_system: &TriggerSystem,
    player_event_residuals: &[PlayerEventResidual],
    player_events: &PlayerEventTable,
    scheduler: &ReactionScheduler,
    registries: ResidualRegistries<'_>,
    script_ctx: &ScriptCtx,
    follow_ups: &mut Vec<String>,
) {
    for (handle, trigger, player) in trigger_residuals {
        let Some(residual) = trigger_bindings.residual(*handle) else {
            log::warn!("[Trigger] residual handle {handle:?} was not bound at install");
            continue;
        };
        // Scope the origin guard to THIS residual only, released before the
        // deferred batch: a `wait` reached synchronously here keys its
        // instance to this `(trigger, player)`, while a batch-seeded `fire`
        // stays sourceless. An interruptible instance parks only while its
        // origin's enter is live, so a player who left within the frame does
        // not park an uncancellable beat.
        let paired_enter_standing = trigger_system
            .paired_enters()
            .contains(&(*trigger, *player));
        let _origin = scheduler.begin_origin(*trigger, *player, paired_enter_standing);
        follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            registries.sequence,
            registries.reaction,
            registries.system,
            script_ctx,
            ResidualOrigin::TriggerBinding,
        ));
    }
    for residual in player_event_residuals {
        let Some(steps) = player_events.residual(residual.handle) else {
            log::warn!(
                "[Scripting] player-event residual {:?} was not bound at install",
                residual.handle
            );
            continue;
        };
        // A `wait` reached here keys its instance to the event player's pawn,
        // so two players' tails never cancel or restart each other. There is
        // no paired exit to cancel an interruptible wait.
        let _origin = scheduler.begin_origin(residual.pawn, PlayerId::Local(residual.pawn), false);
        // Presentation in this fire list plays on the event player's machine;
        // the app routes it. The marked local pawn with no seat presents here.
        let previous = script_ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: "playerEvent".to_string(),
                presentation_seat: match residual.player {
                    PlayerKey::Seat(seat) => Some(seat),
                    PlayerKey::Unseated(_) => None,
                },
                ..SystemCommandFireContext::default()
            });
        follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
            steps,
            registries.sequence,
            registries.reaction,
            registries.system,
            script_ctx,
            ResidualOrigin::TriggerBinding,
        ));
        script_ctx.system_commands.replace_fire_context(previous);
    }
}

#[cfg(test)]
mod tests {
    use postretro_entities::reactions::system_commands::SystemReactionCommand;
    use postretro_entities::{
        MoverCommand, PrimitiveDescriptor, ReactionDescriptor, TriggerActivation, TriggerFireMode,
        TriggerVolumeComponent,
    };
    use postretro_foundation::Seat;
    use postretro_scripting_core::data_descriptors::{NamedReaction, PlayerEventEdge};
    use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use serde_json::json;

    use super::*;
    use crate::player_events::tests::{World, input, lt, number, player_event};
    use crate::scripting::reactions::system_commands::register_system_reaction_primitives;
    use crate::trigger_commands::TriggerFireContext;
    use crate::trigger_system::TriggerEventEdge;

    fn chime(name: &str, sound: &str, then: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: Some(then.to_string()),
                args: json!({ "sound": sound }),
            }),
        }
    }

    // A trigger fire and a player-event fire on one tick: trigger residuals
    // drain first, then the player event's, each with its follow-ups after.
    #[test]
    fn player_event_residuals_drain_after_trigger_residuals() {
        let mut world = World::new();
        let pawn = world.spawn_player(Some(Seat(1)), 20.0);
        let trigger = {
            let mut registry = world.script_ctx.registry.borrow_mut();
            let trigger = registry.spawn(postretro_entities::Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "plateBeep".to_string(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            trigger
        };
        world.install(
            vec![
                chime("plateBeep", "beep", "afterTrigger"),
                chime("bleed", "bleed", "afterPlayer"),
                chime("afterTrigger", "noop", "end"),
                // A player event's follow-up must be context-free: no
                // presentation, so an empty body.
                NamedReaction {
                    name: "afterPlayer".to_string(),
                    descriptor: ReactionDescriptor::Sequence(Vec::new()),
                },
                chime("end", "noop", "end"),
            ],
            vec![player_event(
                PlayerEventEdge::Becomes,
                lt(input("player.health"), number(50.0)),
                &["bleed"],
            )],
        );
        let bindings = {
            let registry = world.script_ctx.registry.borrow();
            let data_registry = world.script_ctx.data_registry.borrow();
            TriggerBindingTable::build_with_script_ctx(&registry, &data_registry, &world.script_ctx)
        };
        let trigger_residual = bindings
            .execute_with_script_ctx(
                trigger,
                TriggerEventEdge::Enter,
                &mut world.script_ctx.registry.borrow_mut(),
                &world.script_ctx,
                &TriggerFireContext {
                    fired_trigger: Some(trigger),
                    activator: Some(pawn),
                    ..TriggerFireContext::default()
                },
            )
            .residual()
            .expect("the plate leaves a residual");
        world.tick();

        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        let sequence = SequencedPrimitiveRegistry::new();
        let reaction = ReactionPrimitiveRegistry::new();
        let mut follow_ups = Vec::new();
        drain_frame_residuals(
            &[(trigger_residual, trigger, PlayerId::Local(pawn))],
            &bindings,
            &TriggerSystem::default(),
            &world.residuals,
            &world.table,
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &sequence,
                reaction: &reaction,
                system: &system,
            },
            &world.script_ctx,
            &mut follow_ups,
        );

        assert_eq!(
            follow_ups,
            vec!["afterTrigger".to_string(), "afterPlayer".to_string()]
        );
        let sounds: Vec<String> = world
            .script_ctx
            .system_commands
            .take()
            .into_iter()
            .filter_map(|command| match command {
                SystemReactionCommand::PlaySound { sound, .. } => Some(sound),
                _ => None,
            })
            .collect();
        assert_eq!(
            sounds,
            vec!["beep".to_string()],
            "the trigger's sound plays where it drains"
        );
        let routed: Vec<(Seat, String)> = world
            .script_ctx
            .system_commands
            .take_routed()
            .into_iter()
            .filter_map(|(seat, command)| match command {
                SystemReactionCommand::PlaySound { sound, .. } => Some((seat, sound)),
                _ => None,
            })
            .collect();
        assert_eq!(
            routed,
            vec![(Seat(1), "bleed".to_string())],
            "the player event's sound is routed to its player"
        );
    }
}
