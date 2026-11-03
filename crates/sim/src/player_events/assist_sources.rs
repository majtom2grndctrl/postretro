//! The discrete `assistReceived()` / `assistGiven()` sources: the tick's assist
//! completions, applied at the head of the player-event pass, before the
//! condition snapshot, so that tick's conditions see their writes.
//! See: context/lib/scripting.md §12 (Player events, Assist)

use postretro_entities::{EntityId, EntityRegistry, ScriptCtx};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::group_resolution::extend_with_player_pawns;

use super::{BoundTrigger, PlayerEventResidual, PlayerEventTable, PlayerKey};
use crate::sim::assist::AssistCompletion;

/// Which side of a completion a discrete source binds `on.player` to.
#[derive(Clone, Copy)]
enum Side {
    Target,
    Helper,
}

impl PlayerEventTable {
    /// Apply `completions` in `players()` order of the target. Each is
    /// re-checked just before its own application, after every earlier one's
    /// fire lists have run, and dropped unless the target still holds the
    /// recorded life state, has not entered a state since the record, and
    /// that state declares `assistable`. An applied completion fires every
    /// `assistReceived` subscription on the target, then every `assistGiven`
    /// subscription on the helper, each in composed order. The helper is not
    /// re-checked: a helper downed after the record still earns its
    /// `assistGiven`.
    pub(super) fn apply_assist_completions(
        &mut self,
        script_ctx: &ScriptCtx,
        completions: &[AssistCompletion],
        residuals: &mut Vec<PlayerEventResidual>,
    ) {
        if completions.is_empty() {
            return;
        }
        let has_discrete = self.events.iter().any(|event| {
            matches!(
                event.trigger,
                BoundTrigger::AssistReceived | BoundTrigger::AssistGiven
            )
        });
        if !has_discrete {
            return;
        }
        let Some(dispatch_scope) = self.dispatch_scope.as_ref() else {
            return;
        };

        // The recorder emits at most one completion per target, in helper
        // order; reorder by the target's place in the group. A target no
        // longer in the group cannot pass the re-check and is left out.
        let pawns = &mut self.scratch.pawns;
        pawns.clear();
        extend_with_player_pawns(&script_ctx.registry.borrow(), None, pawns);
        let mut ordered: Vec<(usize, AssistCompletion)> = completions
            .iter()
            .filter_map(|completion| {
                pawns
                    .iter()
                    .position(|pawn| *pawn == completion.target)
                    .map(|index| (index, *completion))
            })
            .collect();
        ordered.sort_by_key(|(index, _)| *index);

        let mut dispatch_scope = dispatch_scope.borrow_mut();
        for (_, completion) in ordered {
            if !still_assistable(&script_ctx.registry.borrow(), &completion) {
                continue;
            }
            for (side, pawn) in [
                (Side::Target, completion.target),
                (Side::Helper, completion.helper),
            ] {
                let player = player_key(&script_ctx.registry.borrow(), pawn);
                for event in 0..self.events.len() {
                    let fires = matches!(
                        (side, &self.events[event].trigger),
                        (Side::Target, BoundTrigger::AssistReceived)
                            | (Side::Helper, BoundTrigger::AssistGiven)
                    );
                    if fires {
                        self.fire_event(
                            event,
                            player,
                            pawn,
                            script_ctx,
                            &mut script_ctx.registry.borrow_mut(),
                            &mut dispatch_scope,
                            residuals,
                        );
                    }
                }
            }
        }
    }
}

/// The re-check: the target still holds the recorded life-state id under the
/// recorded entry count, and that state declares `assistable`. A move to
/// another state, or a leave and re-entry of the recorded one (the entry count
/// moves on), drops the completion, even when the target entered its state in
/// the tick that recorded it.
fn still_assistable(registry: &EntityRegistry, completion: &AssistCompletion) -> bool {
    registry
        .get_component::<PlayerMovementComponent>(completion.target)
        .is_ok_and(|movement| {
            movement.life_state == completion.life_state
                && movement.life_state_entry == completion.life_state_entry
                && movement.assistable
        })
}

/// The player identity a fire's residual routes to, as the edge pass keys it.
fn player_key(registry: &EntityRegistry, pawn: EntityId) -> PlayerKey {
    registry
        .seat_for_pawn(pawn)
        .map_or(PlayerKey::Unseated(pawn), PlayerKey::Seat)
}
