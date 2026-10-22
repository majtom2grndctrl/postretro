// Ghost eligibility: the one place that decides what can ghost, and as which category.
// See: context/lib/entity_model.md §7d

use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry};
use postretro_level_format::ghost_links::GhostMembers;

use crate::netcode::{ClientReplication, ReplicatedDescriptorClass};

/// The member category a link shows or hides, in the id-56 bit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
// Entities and projectiles classify once descriptor opt-ins land.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum GhostCategory {
    Players,
    /// Non-player descriptor entities.
    Entities,
    Projectiles,
    /// Kinematic movers a link's `ghost_tags` name.
    Movers,
}

impl GhostCategory {
    pub(crate) fn members_bit(self) -> GhostMembers {
        match self {
            Self::Players => GhostMembers::PLAYERS,
            Self::Entities => GhostMembers::ENTITIES,
            Self::Projectiles => GhostMembers::PROJECTILES,
            Self::Movers => GhostMembers::MOVERS,
        }
    }
}

/// Where this machine's ghost candidates come from.
#[derive(Clone, Copy)]
pub(crate) enum CandidateView<'a> {
    /// Host and single player: the registry holds the authoritative entities,
    /// with their components and provenance.
    Authoritative,
    /// Connected client: remote display copies, known by the replicated class
    /// each was materialized from.
    Client(&'a ClientReplication),
}

/// What classification may read about one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateOrigin<'a> {
    /// A registry entity this machine owns; read its components.
    Local,
    /// A client display copy; it carries no provenance, only its class.
    Replicated(ReplicatedDescriptorClass<'a>),
}

/// The ghost eligibility chokepoint: whether `entity` can ever ghost, and as
/// which category. Nothing else decides eligibility.
///
/// Thin slice: player pawns only, hardcoded here. Task 6 replaces the body with
/// the descriptor `ghost` attribute (default false for every descriptor, the
/// player's included) resolved from the class each entity was materialized
/// from, plus the projectile and entity categories.
pub(crate) fn classify(
    registry: &EntityRegistry,
    entity: EntityId,
    origin: CandidateOrigin<'_>,
) -> Option<GhostCategory> {
    match origin {
        CandidateOrigin::Local => {
            let provenance = registry
                .get_component::<DescriptorProvenance>(entity)
                .ok()?;
            let pawn = matches!(
                provenance.spawn_path,
                DescriptorSpawnPath::PlayerSpawn | DescriptorSpawnPath::NetworkSlot
            ) && registry.has_component_kind(entity, ComponentKind::PlayerMovement)
                == Ok(true);
            pawn.then_some(GhostCategory::Players)
        }
        CandidateOrigin::Replicated(ReplicatedDescriptorClass::Pawn(_)) => {
            Some(GhostCategory::Players)
        }
        CandidateOrigin::Replicated(_) => None,
    }
}

/// Visit every eligible candidate once, with its category and origin. The
/// viewer's own pawn never ghosts on its own machine, on any role.
pub(crate) fn visit_candidates(
    registry: &EntityRegistry,
    view: CandidateView<'_>,
    mut visit: impl FnMut(EntityId, GhostCategory, CandidateOrigin<'_>),
) {
    let viewer = crate::followed_player_pawn(registry);
    let mut offer = |entity: EntityId, origin: CandidateOrigin<'_>| {
        if Some(entity) == viewer {
            return;
        }
        if let Some(category) = classify(registry, entity, origin) {
            visit(entity, category, origin);
        }
    };
    match view {
        CandidateView::Authoritative => {
            for (entity, _) in registry.iter_with_kind(ComponentKind::PlayerMovement) {
                offer(entity, CandidateOrigin::Local);
            }
        }
        CandidateView::Client(replication) => {
            for (entity, class) in replication.remote_entities_by_class() {
                offer(entity, CandidateOrigin::Replicated(class));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drift guard: each category's bit is the one its `ghost_members` word sets.
    #[test]
    fn ghost_category_bits_follow_the_id_56_category_names() {
        for (name, bit) in GhostMembers::NAMED {
            let category = match name {
                "players" => GhostCategory::Players,
                "entities" => GhostCategory::Entities,
                "projectiles" => GhostCategory::Projectiles,
                "movers" => GhostCategory::Movers,
                other => panic!("unmapped category {other}"),
            };
            assert_eq!(category.members_bit(), bit);
        }
    }
}
