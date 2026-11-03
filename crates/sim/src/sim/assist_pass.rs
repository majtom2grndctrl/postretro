//! The host and single-player assist accumulator pass, run once per fixed tick
//! after the Weapons stage and `run_death_sweep`.
//! See: context/lib/scripting.md §12 (Assist) · context/lib/networking.md
//! §Host input command queue
//!
//! One pass, over the `players()` group in its resolution order:
//!
//! 1. Snapshot every pawn (origin, life state, entry count, `assistable` bool,
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
use super::assist_scratch::{PassPawn, PassStep};
use super::{RemotePawnCommand, SimCommand, player_life};

impl AssistState {
    /// Run the accumulator for this tick and return its completions, in
    /// `players()` order of the helper. Writes both progress fields on every
    /// pawn of the group. With no slot table, every per-owner side value reads
    /// missing, so an expression reading one faults rather than reusing an
    /// earlier pass's value.
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
        // The buffers are taken for the pass and put back, keeping their
        // capacity, so a steady pass allocates nothing.
        let mut scratch = std::mem::take(&mut self.scratch);
        pass_pawns(
            registry,
            local_command,
            remote_pawn_commands,
            &mut scratch.group,
            &mut scratch.pawns,
        );
        let pawns = &scratch.pawns;
        scratch.ids.clear();
        scratch.ids.extend(pawns.iter().map(|pawn| pawn.id));

        // Pair values for every (helper permitting `use`, other assistable
        // pawn), before any target choice, from one seeded registry state.
        self.pair_table()
            .seed_sides(registry, slot_table, &scratch.ids);
        self.pair_values.reset(pawns.len());
        for (helper_place, helper) in pawns.iter().enumerate() {
            if !helper.permits_use {
                continue;
            }
            for (target_place, target) in pawns.iter().enumerate() {
                if target_place == helper_place || !target.assistable {
                    continue;
                }
                if let Some(side) = AssistTarget::of(registry, target.id) {
                    let values = evaluate(self.pair_table(), helper.id, side);
                    self.pair_values.set(helper_place, target_place, values);
                }
            }
        }

        scratch.steps.clear();
        for (helper_place, helper) in pawns.iter().enumerate() {
            scratch.candidates.clear();
            scratch.candidates.extend(
                pawns
                    .iter()
                    .enumerate()
                    .filter(|(place, _)| *place != helper_place)
                    .map(|(place, pawn)| AssistCandidate {
                        id: pawn.id,
                        position: pawn.position,
                        life: pawn.life,
                        assistable: pawn.assistable,
                        pair: self.pair_values.get(helper_place, place),
                    }),
            );
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
                candidates: &scratch.candidates,
                previous,
                previous_lives: &self.lives,
            });
            scratch.steps.push(PassStep {
                helper: helper.id,
                outcome,
                lost: false,
            });
        }

        // One completion per target per tick: the earlier helper in
        // `players()` order wins; the others reset, unlatched. A loser's row-6
        // record reads "did not advance": its slot reads 0 and its target was
        // just completed, so a listen host's alpha term must not draw a sliver
        // above that 0.
        let mut completions: Vec<AssistCompletion> = Vec::new();
        for entry in &mut scratch.steps {
            if !entry.outcome.completed {
                continue;
            }
            let target = entry.outcome.target.expect("a completion has a target");
            if completions.iter().any(|done| done.target == target) {
                entry.outcome.completed = false;
                entry.outcome.latched = false;
                entry.outcome.progress = 0;
                entry.lost = true;
                continue;
            }
            let pawn = pawns
                .iter()
                .find(|pawn| pawn.id == target)
                .expect("a chosen target is a pass pawn");
            completions.push(AssistCompletion {
                helper: entry.helper,
                target,
                life_state: pawn.life.life_state,
                life_state_entry: pawn.entry,
            });
        }

        scratch.helpers.clear();
        scratch.helpers.extend(scratch.steps.iter().map(|entry| {
            let outcome = &entry.outcome;
            (
                entry.helper,
                HelperRecord {
                    progress: HelperProgress {
                        progress: outcome.progress,
                        target: outcome.target,
                        latched: outcome.latched,
                    },
                    row6: Row6Record {
                        advanced_without_completing: outcome.row == AssistRow::Advance
                            && !outcome.completed
                            && !entry.lost,
                        required_ticks: outcome.required_ticks,
                    },
                },
            )
        }));

        // Zero the fields of pawns that left the group but still exist (a
        // disconnect hold), so a reclaimed seat never shows stale progress.
        for (gone, _) in self
            .helpers
            .iter()
            .filter(|(id, _)| !scratch.ids.contains(id))
        {
            if let Some(movement) = movement_mut(registry, *gone) {
                movement.assist_progress = 0.0;
                movement.assist_received_progress = 0.0;
            }
        }
        for pawn in pawns {
            let own = scratch
                .helpers
                .iter()
                .find(|(id, _)| *id == pawn.id)
                .map_or(0, |(_, record)| record.progress.progress);
            let received = scratch
                .helpers
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

        std::mem::swap(&mut self.helpers, &mut scratch.helpers);
        scratch.lives.clear();
        scratch
            .lives
            .extend(pawns.iter().map(|pawn| (pawn.id, pawn.life)));
        std::mem::swap(&mut self.lives, &mut scratch.lives);
        self.scratch = scratch;
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
/// tick's command level for each, written into `out` (`group` is scratch).
fn pass_pawns(
    registry: &EntityRegistry,
    local_command: &SimCommand,
    remote_pawn_commands: &[RemotePawnCommand],
    group: &mut Vec<EntityId>,
    out: &mut Vec<PassPawn>,
) {
    group.clear();
    out.clear();
    extend_with_player_pawns(registry, None, group);
    let local = registry.local_player_movement_pawn();
    out.extend(group.iter().filter_map(|&id| {
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
                entry: Some(movement.life_state_entry),
            },
            entry: movement.life_state_entry,
            assistable: movement.assistable,
            permits_use: player_life::permits(registry, id, Capability::Use),
            held,
            real_command,
        })
    }));
}

#[cfg(test)]
#[path = "assist_pass_tests.rs"]
mod tests;
