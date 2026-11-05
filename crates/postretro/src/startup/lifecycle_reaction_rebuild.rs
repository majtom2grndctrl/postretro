//! Rebuilds of the active reaction consumers (subscribers, system bindings,
//! trigger and player events) at level install and staged recompose.
//! See: context/lib/boot_sequence.md §3 · context/lib/scripting.md §2

use super::*;

impl App {
    pub(crate) fn rebuild_active_reaction_subscribers(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // A recompose that already meets a threshold fires through the same
        // queue as kill-driven progress, dispatched on the next frame.
        let progress_events = rebuild_reaction_subscribers(
            &mut session.progress_tracker,
            &mut session.crossing_detector,
            &session.scripting.script_ctx,
            SubscriberRebuild::Recompose,
        );
        session.pending_death_events.extend(progress_events);
        session
            .scripting
            .slot_accumulator_bindings
            .rebuild(&session.scripting.script_ctx);
    }

    /// Rebind inline `setState` IR after the active reaction set changes. The
    /// app-side command queue carries raw JSON across the entities boundary;
    /// only this binary-side table holds `BoundProgram<StoreScope>` values and
    /// known rejected command identities.
    pub(crate) fn rebuild_active_system_reaction_bindings(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let script_ctx = session.scripting.script_ctx.clone();
        session
            .scripting
            .system_reaction_ir_bindings
            .rebuild(&script_ctx.data_registry.borrow(), &script_ctx);
    }

    /// Rebind trigger events and player events after staged mod-init recomposes
    /// the active reaction set. Tick dispatch holds bound commands, never
    /// reaction names, so it must be refreshed alongside the other active
    /// reaction consumers.
    pub(crate) fn rebuild_active_trigger_bindings(&mut self) {
        let bindings = {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let mut bindings = build_trigger_bindings(
                &session.scripting.script_ctx,
                session.scripting.command_diagnostics.clone(),
                session.scripting.spawn_context.clone(),
            );
            {
                let registry = session.scripting.script_ctx.registry.borrow();
                let data_registry = session.scripting.script_ctx.data_registry.borrow();
                bindings.install_manifest_events(
                    &registry,
                    &data_registry,
                    &session.scripting.script_ctx,
                );
            }
            bindings
        };
        self.trigger_bindings = bindings;
        if let Some(session) = self.session.as_ref() {
            self.teleporter_bindings =
                postretro_sim::teleporter_system::TeleporterBindingTable::build(
                    &session.scripting.script_ctx,
                    session.scripting.command_diagnostics.clone(),
                    session.scripting.spawn_context.clone(),
                );
        }
        self.warn_hosted_trigger_machine_local();
        self.rebuild_player_events(true);
    }

    pub(super) fn warn_hosted_trigger_machine_local(&self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        crate::startup::reaction_validation::warn_hosted_trigger_machine_local(
            &session.scripting.script_ctx,
            &self.trigger_bindings,
            matches!(
                session.net_endpoint.as_ref(),
                Some(crate::netcode::NetEndpoint::Host { .. })
            ),
        );
    }

    /// Rebind player events over the active composed set. A recompose keeps
    /// edge memory for every condition and edge that survives it. The assist
    /// pair table rebinds here too, replacing the whole table and re-arming
    /// its fault warnings; a connected client binds nothing.
    pub(crate) fn rebuild_player_events(&mut self, keep_edge_memory: bool) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let script_ctx = session.scripting.script_ctx.clone();
        session.assist_state.rebind(&script_ctx);
        postretro_sim::sim::defense_parameters::DefenseParameters::install(&script_ctx);
        let session = &*session;
        let previous = std::mem::take(&mut self.player_events);
        self.player_events = postretro_sim::player_events::PlayerEventTable::build(
            &session.scripting.script_ctx,
            session.scripting.command_diagnostics.clone(),
            session.scripting.spawn_context.clone(),
            keep_edge_memory.then_some(&previous),
        );
    }
}

/// Which lifecycle moment rebuilds the subscribers. A `progress` counts kills
/// among the entities carrying its tag at level install, so only an install
/// captures that membership ([`capture_progress_membership`]); a recompose keeps
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubscriberRebuild {
    LevelInstall,
    Recompose,
}

/// Rebuild the level's reaction subscribers from the current data registry and
/// slot table. At level install this only drops the previous level's
/// kill-progress state: membership is captured later, by
/// [`capture_progress_membership`], once the map placements it counts exist. A
/// recompose keeps that membership, its kill record and its fired latches, and
/// only re-reads the subscriptions. The state-crossing detector reinitializes on
/// both. Returns progress events a recompose's subscriptions already meet (a
/// lowered `at`); install returns none. Free function so both [`App`]'s method
/// and segment B drive it without an `App`.
pub(super) fn rebuild_reaction_subscribers(
    progress_tracker: &mut postretro_scripting_core::reaction_dispatch::ProgressTracker,
    crossing_detector: &mut postretro_scripting_core::state_crossings::CrossingDetector,
    script_ctx: &postretro_entities::ScriptCtx,
    rebuild: SubscriberRebuild,
) -> Vec<String> {
    {
        let mut data_registry = script_ctx.data_registry.borrow_mut();
        // Group reactions by dispatch address (name). For the existing
        // command-target sentinel rule, an address is incompatible with crossing
        // dispatch only when EVERY reaction there needs trigger-fire context; a
        // sentinel-free sibling keeps the address subscribed. Audience tokens
        // and `on.player` have a separate whole-address rejection below: any
        // sibling using either is enough to reject that address, since crossing
        // dispatch cannot supply that context.
        let (fully_sentinel_bound, level_load_has_sentinel): (
            std::collections::HashSet<String>,
            bool,
        ) = {
            // Per address: (any reaction sentinel-bound, all reactions sentinel-bound).
            let mut per_address: std::collections::HashMap<&str, (bool, bool)> =
                std::collections::HashMap::new();
            for reaction in &data_registry.reactions {
                let uses_sentinel = reaction_uses_trigger_sentinel(reaction);
                let (any, all) = per_address
                    .entry(reaction.name.as_str())
                    .or_insert((false, true));
                *any |= uses_sentinel;
                *all &= uses_sentinel;
            }
            // The legacy warning is for trigger sentinels other than the event
            // player: an `@player` levelLoad reaction already gets the
            // `on.player` error below.
            let level_load_has_sentinel = data_registry.reactions.iter().any(|reaction| {
                reaction.name == "levelLoad"
                    && reaction_uses_trigger_sentinel(reaction)
                    && !postretro_scripting_core::player_event_scope::reaction_uses_event_player(
                        &reaction.descriptor,
                    )
            });
            // A name is strippable only when every reaction registered there is
            // sentinel-bound (each map key has at least one reaction, so the
            // non-empty requirement holds by construction).
            let fully_sentinel_bound = per_address
                .into_iter()
                .filter(|(_, (_, all))| *all)
                .map(|(name, _)| name.to_string())
                .collect();
            (fully_sentinel_bound, level_load_has_sentinel)
        };
        // `on.player` exists only in a player event's fire. Unlike a trigger
        // sentinel, one such reaction costs the crossing its whole address,
        // the rejection unit player events use for the same token.
        let mut event_player_addresses: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for reaction in &data_registry.reactions {
            if postretro_scripting_core::player_event_scope::reaction_uses_event_player(
                &reaction.descriptor,
            ) {
                event_player_addresses.insert(reaction.name.clone());
            }
        }
        let audience_addresses: std::collections::HashMap<String, String> = data_registry
            .reactions
            .iter()
            .filter_map(|reaction| {
                crate::startup::reaction_validation::reaction_audience_token(&reaction.descriptor)
                    .map(|token| (reaction.name.clone(), token.to_string()))
            })
            .collect();
        data_registry.crossings.retain_mut(|crossing| {
            let crossing_id = crossing.slot.as_deref().unwrap_or("<predicate>");
            crossing.fire.retain(|address| {
                if let Some(token) = audience_addresses.get(address) {
                    log::error!("[Scripting] crossing on `{crossing_id}` drops address `{address}`: reaction `{address}` uses audience `{token}`, which this source does not publish");
                    return false;
                }

                if event_player_addresses.contains(address) {
                    log::error!(
                        "[Scripting] crossing on `{crossing_id}` drops address `{address}`: reaction `{address}` uses `on.player`, which only a player event publishes"
                    );
                    return false;
                }
                let strip = fully_sentinel_bound.contains(address);
                if strip {
                    log::warn!(
                        "[Scripting] crossing on `{crossing_id}` drops address `{address}`: every reaction at that address needs trigger-fire context, incompatible with crossing dispatch"
                    );
                }
                !strip
            });
            !crossing.fire.is_empty()
        });
        for reaction in &mut data_registry.reactions {
            if reaction.name == "levelLoad"
                && let Some(token) = crate::startup::reaction_validation::reaction_audience_token(
                    &reaction.descriptor,
                )
            {
                log::error!(
                    "[Scripting] levelLoad: reaction `levelLoad` uses audience `{token}`, which this source does not publish; dropping the reaction"
                );
                reaction.descriptor = postretro_entities::ReactionDescriptor::Sequence(Vec::new());
            }
        }
        if level_load_has_sentinel {
            log::warn!(
                "[Scripting] levelLoad references trigger-sentinel work; incompatible commands will be skipped"
            );
        }
        if event_player_addresses.contains("levelLoad")
            && !audience_addresses.contains_key("levelLoad")
        {
            log::error!(
                "[Scripting] levelLoad: reaction `levelLoad` uses `on.player`, which only a player event publishes; its `on.player` work never runs at level load"
            );
        }
    }
    let progress_events = match rebuild {
        SubscriberRebuild::LevelInstall => {
            progress_tracker.clear();
            Vec::new()
        }
        SubscriberRebuild::Recompose => {
            progress_tracker.recompose(&script_ctx.data_registry.borrow())
        }
    };
    crossing_detector.clear();
    crossing_detector.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.slot_table.borrow(),
        script_ctx,
    );
    progress_events
}

/// Level install: capture kill-progress membership and subscribe the composed
/// `progress` set. Runs after every map placement has materialized — the
/// data-archetype sweep (descriptor NPCs, player pawns) and the spawner
/// resolve — and before `levelLoad`, so a `progress` over a map-placed NPC tag
/// counts those NPCs while anything `levelLoad` or a later tick spawns stays
/// out. Returns events whose threshold the set already meets (an `at` at or
/// below zero); the caller dispatches them after `levelLoad`.
pub(super) fn capture_progress_membership(
    progress_tracker: &mut postretro_scripting_core::reaction_dispatch::ProgressTracker,
    script_ctx: &postretro_entities::ScriptCtx,
) -> Vec<String> {
    progress_tracker.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.registry.borrow(),
    )
}

pub(super) fn reaction_uses_trigger_sentinel(
    reaction: &postretro_scripting_core::data_descriptors::NamedReaction,
) -> bool {
    use postretro_scripting_core::data_descriptors::{ReactionDescriptor, SequenceTarget};

    match &reaction.descriptor {
        ReactionDescriptor::Primitive(primitive) => primitive.target.is_some(),
        // Only a subject-token step is trigger-sentinel work. A control step
        // (`@wait`/`@fire`), a group step and a system step run under any
        // source: a `levelLoad` body holding them must keep its crossing
        // subscriptions and must not warn "levelLoad references
        // trigger-sentinel work".
        ReactionDescriptor::Sequence(steps) => steps.iter().any(|step| {
            matches!(
                step.id,
                SequenceTarget::Activators
                    | SequenceTarget::FiredTrigger
                    | SequenceTarget::FiredTeleporter
                    | SequenceTarget::EventPlayer
            )
        }),
        ReactionDescriptor::Progress(_) => false,
    }
}

pub(super) fn build_trigger_bindings(
    script_ctx: &postretro_entities::ScriptCtx,
    command_diagnostics: crate::mover_commands::MoverCommandDiagnostics,
    spawn_context: crate::spawner::SpawnContext,
) -> TriggerBindingTable {
    TriggerBindingTable::build_with_script_ctx_and_diagnostics(
        &script_ctx.registry.borrow(),
        &script_ctx.data_registry.borrow(),
        script_ctx,
        command_diagnostics,
        spawn_context,
    )
}
