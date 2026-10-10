//! Host-only player events (`players().on`): per-player state edges, evaluated
//! once per authoritative tick after the tick settles.
//! See: context/lib/scripting.md §12 (Player events)

mod evaluate;
mod install;
#[cfg(test)]
mod semantics_tests;
#[cfg(test)]
pub(crate) mod tests;

use std::cell::RefCell;

use postretro_entities::{EntityId, ScriptCtx};
use postretro_foundation::{BoundProgram, IrNode, Seat};
use postretro_scripting_core::data_descriptors::{PlayerEventEdge, PlayerEventSource};
use postretro_scripting_core::ir_player_scope::PlayerConditionScope;
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;

use crate::mover_commands::MoverCommandDiagnostics;
use crate::spawner::SpawnContext;
use crate::trigger_commands::{BoundTriggerCommand, TriggerFireContext};

/// A player's identity for edge memory. A seat survives disconnect holds and
/// reclaims; the pawn and the connection id do not. The marked local pawn is
/// a player even when no seat ledger bound it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerKey {
    Seat(Seat),
    Unseated(EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayerEventResidualHandle(usize);

/// Frame-end work one player-event fire leaves for the app drain, in fire
/// order. `player` is the internal target the fire's presentation routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerEventResidual {
    pub handle: PlayerEventResidualHandle,
    pub player: PlayerKey,
    pub pawn: EntityId,
}

struct BoundPlayerEvent {
    source: PlayerEventSource,
    /// Content key, with `edge`, for carrying edge memory across a recompose.
    condition: IrNode,
    edge: PlayerEventEdge,
    program: BoundProgram<PlayerConditionScope>,
    /// The per-player scope inputs the condition reads.
    inputs: Vec<usize>,
    commands: Vec<BoundTriggerCommand>,
    residual: Option<PlayerEventResidualHandle>,
    /// The condition's last value for every player observed last tick, in
    /// player order. An absent player counts as false.
    memory: Vec<(PlayerKey, bool)>,
}

/// The installed player events of one level, with their edge memory.
#[derive(Default)]
pub struct PlayerEventTable {
    events: Vec<BoundPlayerEvent>,
    scope: Option<PlayerConditionScope>,
    /// Fire-list programs bind once and share this scope, reseeded per fire.
    dispatch_scope: Option<RefCell<DispatchScope>>,
    residuals: Vec<Vec<PrepartitionedReactionStep>>,
    command_diagnostics: MoverCommandDiagnostics,
    spawn_context: SpawnContext,
    scratch: evaluate::Scratch,
}

impl std::fmt::Debug for PlayerEventTable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlayerEventTable")
            .field(
                "events",
                &self
                    .events
                    .iter()
                    .map(|event| event.source.to_string())
                    .collect::<Vec<_>>(),
            )
            .field("residuals", &self.residuals)
            .finish()
    }
}

impl PlayerEventTable {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Edge-memory entries across every event: one per observed player.
    #[cfg(test)]
    pub(crate) fn edge_memory_len(&self) -> usize {
        self.events.iter().map(|event| event.memory.len()).sum()
    }

    /// The frame-end steps a residual handle names.
    pub fn residual(&self, handle: PlayerEventResidualHandle) -> Option<&[PrepartitionedReactionStep]> {
        self.residuals.get(handle.0).map(Vec::as_slice)
    }

    /// Evaluate every condition against the settled tick, then apply every
    /// fire's in-tick commands, appending frame-end residuals to `residuals`.
    /// No fire's writes are visible to this tick's evaluation.
    pub fn run_tick(&mut self, script_ctx: &ScriptCtx, residuals: &mut Vec<PlayerEventResidual>) {
        if self.events.is_empty() {
            return;
        }
        {
            let registry = script_ctx.registry.borrow();
            let slot_table = script_ctx.slot_table.borrow();
            self.evaluate(&registry, &slot_table);
        }
        self.apply(script_ctx, residuals);
    }

    fn apply(&mut self, script_ctx: &ScriptCtx, residuals: &mut Vec<PlayerEventResidual>) {
        let Some(dispatch_scope) = self.dispatch_scope.as_ref() else {
            return;
        };
        let mut dispatch_scope = dispatch_scope.borrow_mut();
        let mut registry = script_ctx.registry.borrow_mut();
        for fire in &self.scratch.fires {
            let event = &self.events[fire.event];
            let fire_context = TriggerFireContext {
                event_player: Some(fire.pawn),
                ..TriggerFireContext::default()
            };
            for command in &event.commands {
                command.execute_with_script_ctx(
                    &mut registry,
                    script_ctx,
                    &mut dispatch_scope,
                    &self.command_diagnostics,
                    &self.spawn_context,
                    &fire_context,
                );
            }
            if let Some(handle) = event.residual {
                residuals.push(PlayerEventResidual {
                    handle,
                    player: fire.player,
                    pawn: fire.pawn,
                });
            }
        }
    }
}
