//! Bind trigger event reactions at level install and execute their fixed-tick work.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §12

mod command_binding;
#[cfg(test)]
mod group_tick_tests;
mod install;
mod manifest_events;
mod partition;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod trigger_event_tests;

pub(crate) use partition::partition_direct_reaction;

pub use manifest_events::{
    ResolutionDiagnostics, ResolvedTriggerEvent, TriggerEventSource,
    resolve_manifest_trigger_events,
};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use postretro_entities::{EntityId, EntityRegistry, ScriptCtx, SlotTable};
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;

#[cfg(test)]
use command_binding::bind_command;

use crate::mover_commands::MoverCommandDiagnostics;
#[cfg(test)]
pub(crate) use crate::trigger_commands::BoundTriggerCommandKind;
use crate::trigger_commands::{BoundTriggerCommand, TriggerFireContext};
use crate::trigger_system::TriggerEventEdge;

pub(crate) const TRIGGER_EVENT_INPUTS: &[(&str, postretro_foundation::IrType)] =
    &[("@occupancy", postretro_foundation::IrType::Number)];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TriggerResidualHandle(usize);

#[derive(Debug, Clone)]
pub struct TriggerResidual {
    steps: Vec<PrepartitionedReactionStep>,
}

impl TriggerResidual {
    pub fn steps(&self) -> &[PrepartitionedReactionStep] {
        &self.steps
    }
}

#[derive(Default)]
pub struct TriggerBindingTable {
    bindings: HashMap<(EntityId, TriggerEventEdge), TriggerBinding>,
    /// The fixed set of script-owned edges is built at level install and
    /// reused by every tick. Rebuilding this from `bindings` in the tick loop
    /// needlessly allocated a hash set each frame.
    bound_edges: HashSet<(EntityId, TriggerEventEdge)>,
    /// Trigger-event programs bind once and share this sequential dispatch
    /// scope. Fixed-tick fires never overlap, so reseeding it per evaluation
    /// preserves isolation without allocating a boxed input array per command.
    dispatch_scope: Option<RefCell<DispatchScope>>,
    residuals: Vec<TriggerResidual>,
    command_diagnostics: MoverCommandDiagnostics,
    spawn_context: crate::spawner::SpawnContext,
}

impl fmt::Debug for TriggerBindingTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TriggerBindingTable")
            .field("bindings", &self.bindings)
            .field("bound_edges", &self.bound_edges)
            .field(
                "dispatch_scope",
                &self.dispatch_scope.as_ref().map(|_| "install-owned"),
            )
            .field("residuals", &self.residuals)
            .field("command_diagnostics", &self.command_diagnostics)
            .field("spawn_context", &self.spawn_context)
            .finish()
    }
}

#[derive(Debug)]
struct TriggerBinding {
    commands: Vec<BoundTriggerCommand>,
    residual: Option<TriggerResidualHandle>,
}

/// Result of one fixed-tick binding execution. The test-only command list is
/// deliberately captured at the command-buffer boundary, where duplicate
/// partitioning cannot hide behind idempotent final component state.
#[derive(Debug)]
pub struct TriggerBindingExecution {
    residual: Option<TriggerResidualHandle>,
    #[cfg(test)]
    pub(crate) commands: Vec<BoundTriggerCommandKind>,
    #[cfg(feature = "test-support")]
    command_count: usize,
}

impl TriggerBindingExecution {
    pub fn residual(self) -> Option<TriggerResidualHandle> {
        self.residual
    }

    #[cfg(feature = "test-support")]
    pub fn command_count(&self) -> usize {
        self.command_count
    }
}

impl TriggerBindingTable {
    pub fn bound_edges(&self) -> &HashSet<(EntityId, TriggerEventEdge)> {
        &self.bound_edges
    }

    #[cfg(not(feature = "test-support"))]
    pub(crate) fn execute(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        fire_context: &TriggerFireContext,
    ) -> TriggerBindingExecution {
        self.execute_inner(trigger, edge, registry, slot_table, fire_context)
    }

    #[cfg(feature = "test-support")]
    pub fn execute(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        fire_context: &TriggerFireContext,
    ) -> TriggerBindingExecution {
        self.execute_inner(trigger, edge, registry, slot_table, fire_context)
    }

    fn execute_inner(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        fire_context: &TriggerFireContext,
    ) -> TriggerBindingExecution {
        let Some(binding) = self.bindings.get(&(trigger, edge)) else {
            return TriggerBindingExecution {
                residual: None,
                #[cfg(test)]
                commands: Vec::new(),
                #[cfg(feature = "test-support")]
                command_count: 0,
            };
        };
        #[cfg(test)]
        let mut commands = Vec::with_capacity(binding.commands.len());
        for command in &binding.commands {
            command.execute(
                registry,
                slot_table,
                &self.command_diagnostics,
                &self.spawn_context,
                fire_context,
            );
            #[cfg(test)]
            commands.push(command.kind());
        }
        TriggerBindingExecution {
            residual: binding.residual,
            #[cfg(test)]
            commands,
            #[cfg(feature = "test-support")]
            command_count: binding.commands.len(),
        }
    }

    /// Execute against the live script context. IR commands reuse the
    /// install-owned script-capability `DispatchScope`, reseeded for each
    /// command; literal writes retain their validated batch operation.
    pub fn execute_with_script_ctx(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        script_ctx: &ScriptCtx,
        fire_context: &TriggerFireContext,
    ) -> TriggerBindingExecution {
        let Some(binding) = self.bindings.get(&(trigger, edge)) else {
            return TriggerBindingExecution {
                residual: None,
                #[cfg(test)]
                commands: Vec::new(),
                #[cfg(feature = "test-support")]
                command_count: 0,
            };
        };
        let Some(dispatch_scope) = self.dispatch_scope.as_ref() else {
            log::warn!(
                "[Trigger] live script dispatch was requested without an install-owned dispatch scope"
            );
            return TriggerBindingExecution {
                residual: binding.residual,
                #[cfg(test)]
                commands: Vec::new(),
                #[cfg(feature = "test-support")]
                command_count: binding.commands.len(),
            };
        };
        let mut dispatch_scope = dispatch_scope.borrow_mut();
        #[cfg(test)]
        let mut commands = Vec::with_capacity(binding.commands.len());
        for command in &binding.commands {
            command.execute_with_script_ctx(
                registry,
                script_ctx,
                &mut dispatch_scope,
                &self.command_diagnostics,
                &self.spawn_context,
                fire_context,
            );
            #[cfg(test)]
            commands.push(command.kind());
        }
        TriggerBindingExecution {
            residual: binding.residual,
            #[cfg(test)]
            commands,
            #[cfg(feature = "test-support")]
            command_count: binding.commands.len(),
        }
    }

    pub fn residual(&self, handle: TriggerResidualHandle) -> Option<&TriggerResidual> {
        self.residuals.get(handle.0)
    }

    /// Whether this trigger edge has an active binding from its brush event or
    /// a composed manifest trigger event.
    #[cfg(feature = "dev-tools")]
    pub fn is_bound(&self, trigger: EntityId, edge: TriggerEventEdge) -> bool {
        self.bindings.contains_key(&(trigger, edge))
    }

    #[cfg(test)]
    fn binding(&self, trigger: EntityId, edge: TriggerEventEdge) -> Option<&TriggerBinding> {
        self.bindings.get(&(trigger, edge))
    }
}
