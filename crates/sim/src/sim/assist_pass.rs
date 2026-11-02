//! The host and single-player assist accumulator pass, run once per fixed tick
//! after the Weapons stage and `run_death_sweep`.
//! See: context/lib/scripting.md §12 (Assist) · context/lib/networking.md
//! §Host input command queue
//!
//! One pass, over the `players()` group in its resolution order:
//!
//! 1. Snapshot every pawn (origin, life state, elapsed, `assistable` bool,
//!    `use` capability) and its command level: the local pawn's command is
//!    always real, a remote pawn's is real per `RemotePawnCommand::real_command`,
//!    and a remote pawn with no resolved command holds nothing.
//! 2. Seed the pair scope's sides once, then evaluate every (helper permitting
//!    `use`, other pawn whose `assistable` bool is set) pair before any target
//!    choice; the results are kept for [`AssistState::pair_values`].
//! 3. Run [`step`] per helper, then allow one completion per target: the
//!    earlier helper in `players()` order wins, the others reset unlatched.
//! 4. Write `assist_progress` / `assist_received_progress` on every pawn and
//!    replace the cross-tick state, which drops pawns that left the group.

use postretro_entities::registry::{ComponentKind, ComponentValue};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{Capability, PlayerMovementComponent};
use postretro_scripting_core::group_resolution::extend_with_player_pawns;
use postretro_scripting_core::slot_table::SlotTable;

use super::assist::{
    AssistCandidate, AssistCompletion, AssistHelper, AssistRow, AssistRunner, AssistState,
    AssistStepInput, HelperProgress, HelperRecord, PawnLife, Row6Record, reported, step,
};
use super::assist_pairs::{AssistTarget, evaluate};
use super::{RemotePawnCommand, SimCommand, player_life};

/// One pawn as this pass sees it.
struct PassPawn {
    id: EntityId,
    position: glam::Vec3,
    life: PawnLife,
    elapsed: u32,
    assistable: bool,
    permits_use: bool,
    held: bool,
    real_command: bool,
}

impl AssistState {
    /// Run the accumulator for this tick and return its completions, in
    /// `players()` order of the helper. Writes both progress fields on every
    /// pawn of the group.
    pub fn run_host_pass(
        &mut self,
        registry: &mut EntityRegistry,
        slot_table: Option<&SlotTable>,
        local_command: &SimCommand,
        remote_pawn_commands: &[RemotePawnCommand],
    ) -> Vec<AssistCompletion> {
        // A mod that declares no `assistable` pays nothing per tick: with no
        // bound profile no pawn is ever a target, so every progress stays 0.
        if self.pair_table().is_empty() && self.helpers.is_empty() {
            return Vec::new();
        }
        let pawns = pass_pawns(registry, local_command, remote_pawn_commands);
        let ids: Vec<EntityId> = pawns.iter().map(|pawn| pawn.id).collect();

        // Pair values for every (helper permitting `use`, other assistable
        // pawn), before any target choice, from one seeded registry state.
        if let Some(slot_table) = slot_table {
            self.pair_table().seed_sides(registry, slot_table, &ids);
        }
        let mut pair_values = Vec::new();
        for helper in pawns.iter().filter(|pawn| pawn.permits_use) {
            for target in pawns
                .iter()
                .filter(|pawn| pawn.id != helper.id && pawn.assistable)
            {
                if let Some(side) = AssistTarget::of(registry, target.id) {
                    let values = evaluate(self.pair_table(), helper.id, side);
                    pair_values.push((helper.id, target.id, values));
                }
            }
        }

        let mut steps = Vec::with_capacity(pawns.len());
        for helper in &pawns {
            let candidates: Vec<AssistCandidate> = pawns
                .iter()
                .filter(|pawn| pawn.id != helper.id)
                .map(|pawn| AssistCandidate {
                    id: pawn.id,
                    position: pawn.position,
                    life: pawn.life,
                    assistable: pawn.assistable,
                    pair: pair_values
                        .iter()
                        .find(|(h, t, _)| *h == helper.id && *t == pawn.id)
                        .map(|(_, _, values)| *values),
                })
                .collect();
            let previous = self
                .helper(helper.id)
                .map(|record| record.progress)
                .unwrap_or_default();
            let outcome = step(&AssistStepInput {
                runner: AssistRunner::Host,
                helper: AssistHelper {
                    id: helper.id,
                    position: helper.position,
                    permits_use: helper.permits_use,
                    life: helper.life,
                },
                held: helper.held,
                real_command: helper.real_command,
                candidates: &candidates,
                previous,
                previous_lives: &self.lives,
            });
            steps.push((helper.id, outcome));
        }

        // One completion per target per tick (AC6): the earlier helper in
        // `players()` order wins; the others reset, unlatched. A loser's row-6
        // record reads "did not advance": its slot reads 0 and its target was
        // just completed, so a listen host's alpha term must not draw a sliver
        // above that 0.
        let mut completions: Vec<AssistCompletion> = Vec::new();
        let mut losers: Vec<EntityId> = Vec::new();
        for (helper, outcome) in &mut steps {
            if !outcome.completed {
                continue;
            }
            let target = outcome.target.expect("a completion has a target");
            if completions.iter().any(|done| done.target == target) {
                outcome.completed = false;
                outcome.latched = false;
                outcome.progress = 0;
                losers.push(*helper);
                continue;
            }
            let pawn = pawns
                .iter()
                .find(|pawn| pawn.id == target)
                .expect("a chosen target is a pass pawn");
            completions.push(AssistCompletion {
                helper: *helper,
                target,
                life_state: pawn.life.life_state,
                life_state_elapsed_ticks: pawn.elapsed,
            });
        }

        let helpers: Vec<(EntityId, HelperRecord)> = steps
            .iter()
            .map(|(helper, outcome)| {
                (
                    *helper,
                    HelperRecord {
                        progress: HelperProgress {
                            progress: outcome.progress,
                            target: outcome.target,
                            latched: outcome.latched,
                        },
                        row6: Row6Record {
                            advanced_without_completing: outcome.row == AssistRow::Advance
                                && !outcome.completed
                                && !losers.contains(helper),
                            required_ticks: outcome.required_ticks,
                        },
                    },
                )
            })
            .collect();

        // Zero the fields of pawns that left the group but still exist (a
        // disconnect hold), so a reclaimed seat never shows stale progress.
        for (gone, _) in self.helpers.iter().filter(|(id, _)| !ids.contains(id)) {
            if let Some(movement) = movement_mut(registry, *gone) {
                movement.assist_progress = 0.0;
                movement.assist_received_progress = 0.0;
            }
        }
        for pawn in &pawns {
            let own = helpers
                .iter()
                .find(|(id, _)| *id == pawn.id)
                .map_or(0, |(_, record)| record.progress.progress);
            let received = helpers
                .iter()
                .filter(|(_, record)| record.progress.target == Some(pawn.id))
                .map(|(_, record)| record.progress.progress)
                .max()
                .unwrap_or(0);
            if let Some(movement) = movement_mut(registry, pawn.id) {
                movement.assist_progress = reported(own);
                movement.assist_received_progress = if pawn.assistable {
                    reported(received)
                } else {
                    0.0
                };
            }
        }

        self.helpers = helpers;
        self.lives = pawns.iter().map(|pawn| (pawn.id, pawn.life)).collect();
        self.pair_values = pair_values;
        completions
    }
}

fn movement_mut(
    registry: &mut EntityRegistry,
    pawn: EntityId,
) -> Option<&mut PlayerMovementComponent> {
    match registry.get_component_value_mut(pawn, ComponentKind::PlayerMovement) {
        Ok(ComponentValue::PlayerMovement(movement)) => Some(movement.as_mut()),
        _ => None,
    }
}

/// The `players()` group's movement pawns, in its resolution order, with this
/// tick's command level for each.
fn pass_pawns(
    registry: &EntityRegistry,
    local_command: &SimCommand,
    remote_pawn_commands: &[RemotePawnCommand],
) -> Vec<PassPawn> {
    let mut group = Vec::new();
    extend_with_player_pawns(registry, None, &mut group);
    let local = registry.local_player_movement_pawn();
    group
        .into_iter()
        .filter_map(|id| {
            let movement = registry.get_component::<PlayerMovementComponent>(id).ok()?;
            let position = registry.get_component::<Transform>(id).ok()?.position;
            let (held, real_command) = if Some(id) == local {
                (local_command.assist_held, true)
            } else if let Some(remote) = remote_pawn_commands.iter().find(|r| r.pawn == id) {
                (remote.command.assist_held, remote.real_command)
            } else {
                (false, false)
            };
            Some(PassPawn {
                id,
                position,
                life: PawnLife {
                    life_state: movement.life_state,
                    elapsed: Some(movement.life_state_elapsed_ticks),
                },
                elapsed: movement.life_state_elapsed_ticks,
                assistable: movement.assistable,
                permits_use: player_life::permits(registry, id, Capability::Use),
                held,
                real_command,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "assist_pass_tests.rs"]
mod tests;
