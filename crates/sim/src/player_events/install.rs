//! Install-time binding of the composed player events: conditions bind once
//! against the player condition scope, fire lists partition into in-tick
//! commands and a frame-end residual as trigger fire lists do.
//! See: context/lib/scripting.md §12 (Player events)

use std::cell::RefCell;

use postretro_entities::ScriptCtx;
use postretro_foundation::{BakedIr, CURRENT_IR_VERSION, IrType, bind};
use postretro_scripting_core::data_descriptors::{ComposedPlayerEvent, NamedReaction};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::ir_player_scope::PlayerConditionScope;
use postretro_scripting_core::ir_scopes::DispatchScope;

use super::{BoundPlayerEvent, PlayerEventResidualHandle, PlayerEventTable};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::spawner::SpawnContext;
use crate::trigger_bindings::{TRIGGER_EVENT_INPUTS, partition_direct_reaction};

impl PlayerEventTable {
    /// Bind the active player events in `script_ctx`'s data registry.
    ///
    /// Host and single player only: a connected client registers nothing.
    /// `previous` is the table this one replaces on a recompose; every event
    /// whose condition and edge survive keeps its edge memory, so editing a
    /// fire list re-fires nothing. A level install passes `None`, starting
    /// every player unobserved.
    pub fn build(
        script_ctx: &ScriptCtx,
        command_diagnostics: MoverCommandDiagnostics,
        spawn_context: SpawnContext,
        previous: Option<&PlayerEventTable>,
    ) -> Self {
        let mut table = Self {
            command_diagnostics,
            spawn_context,
            ..Self::default()
        };
        if !group_commands_apply_here(script_ctx) {
            return table;
        }
        let data_registry = script_ctx.data_registry.borrow();
        if data_registry.player_events.is_empty() {
            return table;
        }
        let scope = PlayerConditionScope::new(script_ctx.clone());
        {
            let slot_table = script_ctx.slot_table.borrow();
            let mut bound = BoundAddresses::default();
            for composed in &data_registry.player_events {
                let Some(event) = table.bind_event(
                    composed,
                    &scope,
                    &data_registry,
                    &slot_table,
                    script_ctx,
                    previous,
                    &mut bound,
                ) else {
                    continue;
                };
                table.events.push(event);
            }
        }
        table.scope = Some(scope);
        table.dispatch_scope = Some(RefCell::new(DispatchScope::script(
            script_ctx.clone(),
            TRIGGER_EVENT_INPUTS,
        )));
        table
    }

    fn bind_event(
        &mut self,
        composed: &ComposedPlayerEvent,
        scope: &PlayerConditionScope,
        data_registry: &DataRegistry,
        slot_table: &postretro_entities::SlotTable,
        script_ctx: &ScriptCtx,
        previous: Option<&PlayerEventTable>,
        bound: &mut BoundAddresses,
    ) -> Option<BoundPlayerEvent> {
        let descriptor = &composed.descriptor;
        let source = &composed.source;
        scope.begin_condition();
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: descriptor.condition.clone(),
        };
        let program = match bind(&baked, scope) {
            Ok(program) if program.root_type == IrType::Bool => program,
            Ok(_) => {
                scope.finish_condition();
                log::error!(
                    "[Scripting] player event {source}: condition must produce Bool, but produces Number; not installed"
                );
                return None;
            }
            Err(error) => {
                scope.finish_condition();
                log::error!(
                    "[Scripting] player event {source}: condition failed to bind ({error}); not installed"
                );
                return None;
            }
        };
        let inputs = scope.finish_condition();

        let mut commands = Vec::new();
        let mut steps = Vec::new();
        for address in &descriptor.fire {
            if let Some(first) = bound.first_binding(descriptor, address) {
                log::warn!(
                    "[Scripting] player event: reaction `{address}` is bound by both {first} and {source} on one condition and edge; it binds once"
                );
                continue;
            }
            bound.record(descriptor, address, source);
            let matched: Vec<&NamedReaction> = data_registry
                .reactions
                .iter()
                .filter(|reaction| reaction.name == *address)
                .collect();
            if matched.is_empty() {
                log::warn!(
                    "[Scripting] player event {source}: fire address `{address}` matches no active reaction; skipped"
                );
                continue;
            }
            for (body_ordinal, reaction) in matched.into_iter().enumerate() {
                partition_direct_reaction(
                    reaction,
                    body_ordinal,
                    data_registry,
                    slot_table,
                    Some(script_ctx),
                    &mut commands,
                    &mut steps,
                );
            }
        }
        let residual = (!steps.is_empty()).then(|| {
            let handle = PlayerEventResidualHandle(self.residuals.len());
            self.residuals.push(steps);
            handle
        });

        let memory = previous
            .and_then(|previous| {
                previous.events.iter().find(|bound| {
                    bound.edge == descriptor.edge && bound.condition == descriptor.condition
                })
            })
            .map(|bound| bound.memory.clone())
            .unwrap_or_default();

        Some(BoundPlayerEvent {
            source: source.clone(),
            condition: descriptor.condition.clone(),
            edge: descriptor.edge,
            program,
            inputs,
            commands,
            residual,
            memory,
        })
    }
}

/// Every (condition, edge, address) bound so far, in composed order. Two
/// entries reaching one triple bind it once: running the reaction twice would
/// double its effects. Conditions match structurally.
#[derive(Default)]
struct BoundAddresses {
    entries: Vec<(
        postretro_foundation::IrNode,
        postretro_scripting_core::data_descriptors::PlayerEventEdge,
        String,
        postretro_scripting_core::data_descriptors::PlayerEventSource,
    )>,
}

impl BoundAddresses {
    fn first_binding(
        &self,
        descriptor: &postretro_scripting_core::data_descriptors::PlayerEventDescriptor,
        address: &str,
    ) -> Option<&postretro_scripting_core::data_descriptors::PlayerEventSource> {
        self.entries
            .iter()
            .find(|(condition, edge, bound, _)| {
                *edge == descriptor.edge && bound == address && *condition == descriptor.condition
            })
            .map(|(_, _, _, source)| source)
    }

    fn record(
        &mut self,
        descriptor: &postretro_scripting_core::data_descriptors::PlayerEventDescriptor,
        address: &str,
        source: &postretro_scripting_core::data_descriptors::PlayerEventSource,
    ) {
        self.entries.push((
            descriptor.condition.clone(),
            descriptor.edge,
            address.to_string(),
            source.clone(),
        ));
    }
}
