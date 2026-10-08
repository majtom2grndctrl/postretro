//! Install-time trigger-event binding: resolves brush KVP and manifest trigger
//! events to `(trigger, edge)` bindings over the composed reaction set.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §12

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, ScriptCtx, SlotTable, TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::NamedReaction;
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use std::cell::RefCell;

use super::partition::partition_direct_reaction;
use super::{
    TRIGGER_EVENT_INPUTS, TriggerBinding, TriggerBindingTable, TriggerResidual,
    TriggerResidualHandle,
};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::trigger_commands::BoundTriggerCommand;
use crate::trigger_system::TriggerEventEdge;

impl TriggerBindingTable {
    /// Construct brush-authored bindings after reaction composition. Empty brush
    /// event names add no binding here; manifest trigger events may still bind
    /// the edge. Unknown names warn once per trigger edge and do not fall back
    /// to a later drain-time lookup.
    #[cfg(any(test, feature = "test-support"))]
    pub fn build(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
    ) -> Self {
        Self::build_inner(registry, data_registry, slot_table, None)
    }

    /// Bind against the script-capability `StoreScope` at level install. The
    /// standalone table-only builder remains for literal-only test fixtures;
    /// real level installation must use this path so IR writes are validated
    /// against the live slot declarations once.
    #[cfg(any(test, feature = "test-support"))]
    pub fn build_with_script_ctx(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
    ) -> Self {
        let slot_table = script_ctx.slot_table.borrow();
        Self::build_inner(registry, data_registry, &slot_table, Some(script_ctx))
    }

    pub fn build_with_script_ctx_and_diagnostics(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
        command_diagnostics: MoverCommandDiagnostics,
        spawn_context: crate::spawner::SpawnContext,
    ) -> Self {
        let slot_table = script_ctx.slot_table.borrow();
        let mut table = Self::build_inner(registry, data_registry, &slot_table, Some(script_ctx));
        table.command_diagnostics = command_diagnostics;
        table.spawn_context = spawn_context;
        table
    }

    fn build_inner(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
        script_ctx: Option<&ScriptCtx>,
    ) -> Self {
        let mut trigger_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .map(|(id, _)| id)
            .collect();
        trigger_ids.sort_unstable();

        let mut table = Self::default();
        for trigger in trigger_ids {
            let Ok(component) = registry
                .get_component::<TriggerVolumeComponent>(trigger)
                .cloned()
            else {
                continue;
            };
            table.bind_event(
                trigger,
                TriggerEventEdge::Enter,
                &component.on_fire,
                data_registry,
                slot_table,
                script_ctx,
            );
            table.bind_event(
                trigger,
                TriggerEventEdge::Exit,
                &component.on_exit,
                data_registry,
                slot_table,
                script_ctx,
            );
        }
        table.dispatch_scope = script_ctx.map(|script_ctx| {
            RefCell::new(DispatchScope::script(
                script_ctx.clone(),
                TRIGGER_EVENT_INPUTS,
            ))
        });
        table
    }

    pub(super) fn bind_event(
        &mut self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        event_name: &str,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
        script_ctx: Option<&ScriptCtx>,
    ) {
        if event_name.is_empty() {
            return;
        }
        let matched: Vec<&NamedReaction> = data_registry
            .reactions
            .iter()
            .filter(|reaction| reaction.name == event_name)
            .collect();
        if matched.is_empty() {
            log::warn!(
                "[Trigger] {edge:?} event `{event_name}` on {trigger} does not match an active composed reaction; not binding"
            );
            return;
        }

        let mut commands = Vec::new();
        let mut steps = Vec::new();
        // `matched` filters to a single name, so this enumerate index is the
        // reaction's `body_ordinal` among same-named matches — it agrees with the
        // count `fire_named_event_with_sequences` derives from the same
        // `data_registry.reactions` order, and rides on each residual step so a
        // resumed wait tail re-enrolls under its own instance key.
        for (body_ordinal, reaction) in matched.iter().enumerate() {
            partition_direct_reaction(
                reaction,
                body_ordinal,
                data_registry,
                slot_table,
                script_ctx,
                &mut commands,
                &mut steps,
            );
        }

        if commands.is_empty() && steps.is_empty() {
            return;
        }
        self.append_binding(trigger, edge, commands, steps);
    }

    fn append_binding(
        &mut self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        commands: Vec<BoundTriggerCommand>,
        steps: Vec<PrepartitionedReactionStep>,
    ) {
        self.bound_edges.insert((trigger, edge));
        if let Some(binding) = self.bindings.get_mut(&(trigger, edge)) {
            binding.commands.extend(commands);
            if !steps.is_empty() {
                if let Some(handle) = binding.residual {
                    self.residuals[handle.0].steps.extend(steps);
                } else {
                    let handle = TriggerResidualHandle(self.residuals.len());
                    self.residuals.push(TriggerResidual { steps });
                    binding.residual = Some(handle);
                }
            }
            return;
        }
        let residual = (!steps.is_empty()).then(|| {
            let handle = TriggerResidualHandle(self.residuals.len());
            self.residuals.push(TriggerResidual { steps });
            handle
        });
        self.bindings
            .insert((trigger, edge), TriggerBinding { commands, residual });
    }

    /// Register `(trigger, edge)` as a bound edge without adding any command or
    /// residual work. E18 V5 uses this to derive the paired Exit edge for an
    /// interruptible wait whose trigger authored no `on_exit` KVP: without the
    /// edge in `bound_edges`, the Exit arm of `run_authoritative_tick_with_dispatch`
    /// `continue`s past it and the wait's cancel source is silently consumed.
    /// `bind_event` cannot serve — it returns early when there is no command or
    /// step work, before the only other `bound_edges` inserter runs.
    pub fn bind_edge_only(&mut self, trigger: EntityId, edge: TriggerEventEdge) {
        self.bound_edges.insert((trigger, edge));
    }
}
