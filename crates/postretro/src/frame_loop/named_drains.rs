// Source-origin policy shared by the real frame drain and conditioned proof.
// See: context/lib/scripting.md §12 · context/lib/networking.md
use crate::{drain_named_events_with_sequences, netcode, sound_events};
use postretro_entities::{Emitter, FireOrigin, ScriptCtx};
use postretro_sim::emission::{AiEmission, MovementEmission, WeaponEmission};

pub(crate) struct NamedFrameEvents<'a> {
    pub movement: &'a [MovementEmission],
    pub ai: &'a [AiEmission],
    pub observer: &'a [netcode::weapon_cues::ObserverWeaponCueDelivery],
    pub weapon: &'a [WeaponEmission],
    pub movers: &'a [(String, Option<Emitter>)],
    pub deaths: &'a [String],
}

pub(crate) fn drain_frame_named_events(
    events: NamedFrameEvents<'_>,
    scripting: &crate::session::ScriptingCore,
    script_ctx: &ScriptCtx,
) -> Vec<(String, FireOrigin)> {
    let mut follow_ups = Vec::new();
    // Every post-tick named source uses the executing path, then
    // contributes `fire`/`on_complete` names to one bounded deferred
    // batch. This keeps movement, AI, weapon, mover, and death events
    // semantically aligned and lets waits enroll through the common
    // sequence control arm.
    follow_ups.extend(drain_named_events_with_sequences(
        events
            .movement
            .iter()
            .map(|emission| (emission.address, Some(emission.emitter.clone()))),
        // Movement events fire only for this machine's own pawn.
        postretro_entities::FireOrigin::EveryMachine(postretro_entities::MachineSource::OwnState),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events.ai.iter().filter_map(|emission| {
            emission
                .address
                .as_deref()
                .map(|address| (address, Some(emission.emitter.clone())))
        }),
        postretro_entities::FireOrigin::HostOnly(postretro_entities::FireSubject::None),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events
            .ai
            .iter()
            .filter(|emission| emission.shot_id.is_some())
            .flat_map(|emission| {
                std::iter::once("activate")
                    .chain(
                        emission
                            .action
                            .as_ref()
                            .and_then(|a| a.emits.as_ref())
                            .and_then(|e| e.activate.as_deref()),
                    )
                    .map(move |address| (address, Some(emission.emitter.clone())))
            }),
        postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events.observer.iter().flat_map(|cue| {
            let builtin = match cue.kind {
                netcode::weapon_cues::WeaponCueKind::Activate => "activate",
                netcode::weapon_cues::WeaponCueKind::Impact => "impact",
            };
            std::iter::once(builtin)
                .chain(cue.alias.as_deref())
                .map(move |address| (address, Some(cue.emitter.clone())))
        }),
        postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events.weapon.iter().flat_map(|emission| {
            sound_events::weapon_emission_addresses(emission)
                .map(move |address| (address, Some(emission.emitter.clone())))
        }),
        postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events
            .movers
            .iter()
            .map(|(name, emitter)| (name, emitter.clone())),
        postretro_entities::FireOrigin::HostOnly(postretro_entities::FireSubject::None),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups.extend(drain_named_events_with_sequences(
        events.deaths.iter().map(|name| (name, None)),
        postretro_entities::FireOrigin::HostOnly(postretro_entities::FireSubject::None),
        &script_ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        script_ctx,
    ));
    follow_ups
}

#[cfg(test)]
#[path = "named_drains_tests.rs"]
mod tests;
