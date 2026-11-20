// Faction-first pawn index: every targetable pawn, read once per AI pass and
// bucketed by its exact faction value, so an enemy's target scan inspects the
// buckets it is hostile to instead of every pawn.
// See: context/lib/entity_model.md §7c (enemy brain component)

use std::cell::RefCell;
use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, EntityStateComponent, LiveFactionSentiment, Transform,
};

use super::FACTION_STATE_FIELD;
use super::targeting::is_hostile;

/// One targetable pawn as the pass read it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IndexedPawn {
    pub(crate) entity: EntityId,
    pub(crate) position: Vec3,
    /// The pawn's faction leaf, read once per pass.
    pub(crate) faction: f32,
    /// The pawn's life counter, read once per pass: 0 for an NPC.
    pub(crate) life: u32,
    bucket: u32,
}

#[derive(Debug, Clone, Copy)]
struct FactionBucket {
    faction: f32,
    /// `members[start..end]`: this bucket's pawns, in scan order.
    start: u32,
    end: u32,
}

/// The pawns of one AI pass. Built at the start of every pass, after the
/// previous pass's apply, and never kept across passes: a pawn's faction,
/// position and liveness are this pass's.
#[derive(Debug, Default)]
pub(crate) struct PawnIndex {
    /// Scan order: player-movement holders, then brain-only holders, each in
    /// slot order. A pawn's index here is its scan position.
    pawns: Vec<IndexedPawn>,
    buckets: Vec<FactionBucket>,
    members: Vec<u32>,
    /// Exact faction bits → bucket, rebuilt each pass.
    bucket_of: HashMap<u32, u32>,
    /// Entity → scan position, for ledger attackers.
    by_entity: HashMap<EntityId, u32>,
    /// Per-bucket pawn counts while building.
    bucket_counts: Vec<u32>,
    /// Due-walk scratch: (distance, scan position) of hostile pawns.
    pub(crate) walk: RefCell<Vec<(f32, u32)>>,
}

impl PawnIndex {
    /// Read every targetable pawn: a player-movement or brain holder with a
    /// transform that no inventory owns, as `targeting::target_candidate`
    /// admits one. Two column walks, whatever the enemy count.
    pub(crate) fn rebuild(&mut self, registry: &EntityRegistry) {
        self.clear();

        let movement_holders = registry.iter_with_kind(ComponentKind::PlayerMovement);
        let brain_only_holders =
            registry
                .iter_with_kind(ComponentKind::Brain)
                .filter(|(entity, _)| {
                    registry
                        .get_component::<PlayerMovementComponent>(*entity)
                        .is_err()
                });
        for (entity, _) in movement_holders.chain(brain_only_holders) {
            if registry.is_inventory_owned(entity) {
                continue;
            }
            let Ok(transform) = registry.get_component::<Transform>(entity) else {
                continue;
            };
            let faction = registry
                .get_component::<EntityStateComponent>(entity)
                .map_or(0.0, |state| state.get(FACTION_STATE_FIELD));
            let life = registry
                .get_component::<PlayerMovementComponent>(entity)
                .map_or(0, |movement| movement.life_counter);
            let next = self.buckets.len() as u32;
            let bucket = *self.bucket_of.entry(faction.to_bits()).or_insert(next);
            if bucket == next {
                self.buckets.push(FactionBucket {
                    faction,
                    start: 0,
                    end: 0,
                });
                self.bucket_counts.push(0);
            }
            self.bucket_counts[bucket as usize] += 1;
            self.by_entity.insert(entity, self.pawns.len() as u32);
            self.pawns.push(IndexedPawn {
                entity,
                position: transform.position,
                faction,
                life,
                bucket,
            });
        }
        let mut start = 0;
        for (bucket, count) in self.buckets.iter_mut().zip(&self.bucket_counts) {
            bucket.start = start;
            bucket.end = start;
            start += count;
        }
        self.members.resize(self.pawns.len(), 0);
        for (position, pawn) in self.pawns.iter().enumerate() {
            let bucket = &mut self.buckets[pawn.bucket as usize];
            self.members[bucket.end as usize] = position as u32;
            bucket.end += 1;
        }
        let mut walk = self.walk.borrow_mut();
        walk.clear();
        walk.reserve(self.pawns.len());
    }

    /// Empty the index for a pass with no enemy to serve.
    pub(crate) fn clear(&mut self) {
        self.pawns.clear();
        self.buckets.clear();
        self.members.clear();
        self.bucket_of.clear();
        self.by_entity.clear();
        self.bucket_counts.clear();
    }

    /// A test-only index over `registry`.
    #[cfg(test)]
    pub(crate) fn for_test(registry: &EntityRegistry) -> Self {
        let mut index = Self::default();
        index.rebuild(registry);
        index
    }

    /// Every pawn, in scan order.
    #[cfg(test)]
    pub(crate) fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.pawns.iter().map(|pawn| pawn.entity)
    }

    pub(crate) fn pawn(&self, position: u32) -> &IndexedPawn {
        &self.pawns[position as usize]
    }

    /// `entity`'s life counter, when it is a pawn of this pass.
    pub(crate) fn life_of(&self, entity: EntityId) -> Option<u32> {
        self.position_of(entity)
            .map(|position| self.pawn(position).life)
    }

    /// `entity`'s scan position, when it is a pawn of this pass.
    pub(crate) fn position_of(&self, entity: EntityId) -> Option<u32> {
        self.by_entity.get(&entity).copied()
    }

    /// Whether `faction` is hostile to `pawn`'s bucket.
    pub(crate) fn hostile_to(
        &self,
        factions: &LiveFactionSentiment<'_>,
        faction: f32,
        pawn: &IndexedPawn,
    ) -> bool {
        is_hostile(
            factions,
            faction,
            self.buckets[pawn.bucket as usize].faction,
        )
    }

    /// Every pawn in a bucket `faction` is hostile to, as scan positions,
    /// bucket by bucket. Hostility resolves once per bucket.
    pub(crate) fn hostile_pawns<'a>(
        &'a self,
        factions: &'a LiveFactionSentiment<'_>,
        faction: f32,
    ) -> impl Iterator<Item = u32> + 'a {
        self.buckets
            .iter()
            .filter(move |bucket| is_hostile(factions, faction, bucket.faction))
            .flat_map(|bucket| {
                self.members[bucket.start as usize..bucket.end as usize]
                    .iter()
                    .copied()
            })
    }
}
