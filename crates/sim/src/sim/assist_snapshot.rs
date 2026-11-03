//! The assist facts a snapshot carries for each connected helper: its pair
//! table and the required ticks its pawn receives as target.
//! See: context/lib/scripting.md §12 (Assist) · context/lib/networking.md
//! §Version gates
//!
//! The snapshot build evaluates the pair table once per snapshot from the
//! registry state the snapshot's movement records read, so a frame-end
//! write lands in both this table and the next tick's accumulator pass.
//! The received count reads the last pass's row-6 records and evaluates
//! nothing.

use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::group_resolution::extend_with_player_pawns;
use postretro_scripting_core::slot_table::SlotTable;

use super::assist::AssistState;
use super::assist_pairs::{AssistTarget, PairValues, evaluate};

/// One helper's assist facts for its snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PawnAssistFacts {
    /// One entry per other pawn whose `assistable` bool is set, in
    /// `players()` order.
    pub pairs: Vec<(EntityId, PairValues)>,
    /// The `required_ticks` of the highest-progress helper on this pawn, ties
    /// by `players()` order; 0 with none or while its `assistable` is false.
    pub received_required_ticks: u32,
}

impl AssistState {
    /// The facts for each of `helpers`, in order, evaluated against the
    /// registry state as it stands, whatever each helper's own state.
    pub fn snapshot_facts(
        &self,
        registry: &EntityRegistry,
        slot_table: &SlotTable,
        helpers: &[EntityId],
    ) -> Vec<PawnAssistFacts> {
        let mut group = Vec::new();
        extend_with_player_pawns(registry, None, &mut group);
        let assistable = |pawn: EntityId| {
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .is_ok_and(|movement| movement.assistable)
        };
        let candidates: Vec<EntityId> = group
            .iter()
            .copied()
            .filter(|pawn| assistable(*pawn))
            .collect();
        if !candidates.is_empty() && !helpers.is_empty() {
            let mut seeded = group.clone();
            seeded.extend(helpers.iter().filter(|helper| !group.contains(helper)));
            self.pair_table().seed_sides(registry, slot_table, &seeded);
        }
        helpers
            .iter()
            .map(|&helper| PawnAssistFacts {
                pairs: candidates
                    .iter()
                    .filter(|target| **target != helper)
                    .map(|&target| {
                        // Every candidate has an entry; one whose profile
                        // cannot be read is unassistable.
                        let values = AssistTarget::of(registry, target)
                            .map_or(PairValues::UNASSISTABLE, |side| {
                                evaluate(self.pair_table(), helper, side)
                            });
                        (target, values)
                    })
                    .collect(),
                received_required_ticks: if assistable(helper) {
                    self.received_required_ticks(helper)
                } else {
                    0
                },
            })
            .collect()
    }

    /// The `required_ticks` in the row-6 record of the highest-progress
    /// helper on `target` at the last pass, ties by `players()` order; 0 with
    /// none.
    fn received_required_ticks(&self, target: EntityId) -> u32 {
        let mut best: Option<(u64, u32)> = None;
        for (_, record) in self.helpers() {
            if record.progress.target != Some(target) {
                continue;
            }
            if best.is_none_or(|(progress, _)| record.progress.progress > progress) {
                best = Some((record.progress.progress, record.row6.required_ticks));
            }
        }
        best.map_or(0, |(_, required_ticks)| required_ticks)
    }
}
