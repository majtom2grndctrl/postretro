//! The frame-end drain of in-tick sources' residual work: trigger residuals,
//! then player-event residuals, each in tick order and authored order.
//! See: context/lib/scripting.md §12 (Player events)

use postretro_entities::{
    EntityId, EntityRegistry, FireOrigin, FireSubject, ScriptCtx, SystemCommandFireContext,
};
use postretro_foundation::Seat;
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
/// to `follow_ups`. Trigger residuals drain first, then player-event residuals;
/// each source keeps tick order and authored order. A player event evaluates
/// after the tick's triggers fire, so its batch follows theirs.
#[allow(clippy::too_many_arguments)]
pub fn drain_frame_residuals(
    trigger_residuals: &[(TriggerResidualHandle, EntityId, PlayerId, Vec<PlayerId>)],
    trigger_bindings: &TriggerBindingTable,
    trigger_system: &TriggerSystem,
    player_event_residuals: &[PlayerEventResidual],
    player_events: &PlayerEventTable,
    scheduler: &ReactionScheduler,
    registries: ResidualRegistries<'_>,
    script_ctx: &ScriptCtx,
    follow_ups: &mut Vec<(String, FireOrigin)>,
    resolve_seat: impl Fn(PlayerId, &EntityRegistry) -> Option<Seat>,
) {
    for (handle, trigger, player, occupants) in trigger_residuals {
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
        let origin = {
            let registry = script_ctx.registry.borrow();
            FireOrigin::HostOnly(FireSubject::Trigger {
                player: resolve_seat(*player, &registry),
                trigger: *trigger,
                occupants: occupants
                    .iter()
                    .filter_map(|player| resolve_seat(*player, &registry))
                    .collect(),
            })
        };
        let previous = script_ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: format!("trigger:{trigger:?}"),
                origin,
                ..SystemCommandFireContext::default()
            });
        follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            registries.sequence,
            registries.reaction,
            registries.system,
            script_ctx,
            ResidualOrigin::TriggerBinding,
        ));
        script_ctx.system_commands.replace_fire_context(previous);
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
        // no paired exit to cancel an interruptible wait. The instance key is
        // the reaction and the event player, so one parked tail runs per
        // (reaction, player) across every player event: a second event firing
        // the same reaction for that player while its tail is parked does not
        // park another (the scheduler's non-interruptible re-fire rule).
        let _origin = scheduler.begin_origin(residual.pawn, PlayerId::Local(residual.pawn), false);
        // Presentation in this fire list plays on the event player's machine;
        // the app routes it. The marked local pawn with no seat presents here.
        let previous = script_ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: "playerEvent".to_string(),
                origin: FireOrigin::HostOnly(FireSubject::PlayerEvent {
                    player: match residual.player {
                        PlayerKey::Seat(seat) => Some(seat),
                        PlayerKey::Unseated(_) => None,
                    },
                }),
                ..SystemCommandFireContext::default()
            });
        // Player-event residuals share the in-tick-bound contract (their
        // consequential work already ran in the tick), so they use `TriggerBinding`.
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
            &[(
                trigger_residual,
                trigger,
                PlayerId::Local(pawn),
                vec![PlayerId::Local(pawn)],
            )],
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
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(_) => None,
            },
        );

        assert_eq!(
            follow_ups,
            vec![
                (
                    "afterTrigger".to_string(),
                    FireOrigin::HostOnly(FireSubject::None)
                ),
                (
                    "afterPlayer".to_string(),
                    FireOrigin::HostOnly(FireSubject::None)
                )
            ]
        );
        use postretro_entities::Recipients;
        let routed: Vec<_> = world
            .script_ctx
            .system_commands
            .take_ordered()
            .into_iter()
            .filter_map(|(recipients, command)| match command {
                SystemReactionCommand::PlaySound { sound, .. } => Some((recipients, sound)),
                _ => None,
            })
            .collect();
        assert_eq!(
            routed,
            vec![
                (Recipients::Everyone, "beep".to_string()),
                (Recipients::Seats(vec![Seat(1)]), "bleed".to_string()),
            ]
        );
        assert_eq!(
            world.script_ctx.system_commands.fire_context(),
            SystemCommandFireContext::default()
        );
    }
    #[test]
    fn unseated_player_event_origin_presents_locally_and_restores_ambient_context() {
        let mut world = World::new();
        world.spawn_player(None, 20.0);
        world.install(
            vec![
                chime("bleed", "hiss", "done"),
                NamedReaction {
                    name: "done".to_string(),
                    descriptor: ReactionDescriptor::Sequence(vec![]),
                },
            ],
            vec![player_event(
                PlayerEventEdge::Becomes,
                lt(input("player.health"), number(50.0)),
                &["bleed"],
            )],
        );
        world.tick();
        assert_eq!(world.residuals.len(), 1);
        let ambient = SystemCommandFireContext {
            source: "outer".to_string(),
            origin: FireOrigin::HostOnly(FireSubject::None),
            ..Default::default()
        };
        world
            .script_ctx
            .system_commands
            .replace_fire_context(ambient.clone());
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        let mut follow_ups = Vec::new();
        drain_frame_residuals(
            &[],
            &Default::default(),
            &TriggerSystem::default(),
            &world.residuals,
            &world.table,
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &SequencedPrimitiveRegistry::new(),
                reaction: &ReactionPrimitiveRegistry::new(),
                system: &system,
            },
            &world.script_ctx,
            &mut follow_ups,
            |_, _| None,
        );
        assert_eq!(world.script_ctx.system_commands.fire_context(), ambient);
        assert_eq!(
            follow_ups,
            vec![("done".to_string(), FireOrigin::HostOnly(FireSubject::None))]
        );
        let commands = world.script_ctx.system_commands.take_ordered();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].0,
            postretro_entities::Recipients::Local,
            "an unseated player event retains E16's local presentation default"
        );
    }

    #[test]
    fn trigger_origin_resolves_captured_occupants_and_drops_unbound_players() {
        let mut world = World::new();
        let pawn = world.spawn_player(Some(Seat(1)), 100.0);
        let trigger = {
            let mut registry = world.script_ctx.registry.borrow_mut();
            let trigger = registry.spawn(postretro_entities::Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "occupants".to_string(),
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
        let mut occupants_chime = chime("occupants", "alarm", "after");
        if let ReactionDescriptor::Primitive(primitive) = &mut occupants_chime.descriptor {
            primitive.args["audience"] = json!("@trigger");
        }
        world.install(vec![occupants_chime], vec![]);
        let bindings = TriggerBindingTable::build_with_script_ctx(
            &world.script_ctx.registry.borrow(),
            &world.script_ctx.data_registry.borrow(),
            &world.script_ctx,
        );
        let handle = bindings
            .execute_with_script_ctx(
                trigger,
                TriggerEventEdge::Enter,
                &mut world.script_ctx.registry.borrow_mut(),
                &world.script_ctx,
                &TriggerFireContext {
                    fired_trigger: Some(trigger),
                    activator: Some(pawn),
                    ..Default::default()
                },
            )
            .residual()
            .unwrap();
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        let mut follow_ups = Vec::new();
        let captured = vec![
            PlayerId::Local(pawn),
            PlayerId::Remote(3),
            PlayerId::Remote(4),
        ];
        drain_frame_residuals(
            &[(handle, trigger, PlayerId::Remote(4), captured)],
            &bindings,
            &TriggerSystem::default(),
            &[],
            &world.table,
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &SequencedPrimitiveRegistry::new(),
                reaction: &ReactionPrimitiveRegistry::new(),
                system: &system,
            },
            &world.script_ctx,
            &mut follow_ups,
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(3) => Some(Seat(3)),
                PlayerId::Remote(_) => None,
            },
        );
        let commands = world.script_ctx.system_commands.take_ordered();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].0,
            postretro_entities::Recipients::Seats(vec![Seat(1), Seat(3)]),
            "the edge snapshot, rather than the empty live trigger system, supplies occupants"
        );
        assert_eq!(
            follow_ups,
            vec![("after".to_string(), FireOrigin::HostOnly(FireSubject::None))]
        );
        assert_eq!(
            world.script_ctx.system_commands.fire_context(),
            SystemCommandFireContext::default()
        );
    }
}
