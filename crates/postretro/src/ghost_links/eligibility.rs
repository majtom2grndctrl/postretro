// Ghost eligibility: the one place that decides what can ghost, and as which category.
// See: context/lib/entity_model.md §7d

use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
};
use postretro_level_format::ghost_links::GhostMembers;

use super::movers::GhostMovers;
use super::opt_ins::GhostOptIns;
use crate::netcode::{ClientReplication, ReplicatedDescriptorClass};

/// The member category a link shows or hides, in the id-56 bit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
/// Resolved from this peer's own descriptors (`opt_ins`) by the class the
/// entity was materialized from, the same way on every role:
/// - a player pawn ghosts when its descriptor sets `ghost`;
/// - a non-player descriptor entity ghosts when its descriptor sets `ghost`;
/// - a projectile ghosts when its firing weapon sets `projectile.ghost`,
///   whoever fired it, a monster included.
///
/// - a kinematic mover ghosts when some link's `ghost_tags` names it; which
///   links may show it is per link ([`GhostMovers::listed_by`]).
///
/// The engine never branches on actor kind: a player descriptor that omits
/// `ghost` puts no pawn in any ghost set. Movers have no descriptor: every
/// peer loads them, tags included, from the level file.
pub(crate) fn classify(
    registry: &EntityRegistry,
    entity: EntityId,
    origin: CandidateOrigin<'_>,
    opt_ins: &GhostOptIns,
    movers: &GhostMovers,
) -> Option<GhostCategory> {
    match origin {
        CandidateOrigin::Local => classify_local(registry, entity, opt_ins, movers),
        CandidateOrigin::Replicated(class) => classify_replicated(class, opt_ins),
    }
}

/// A registry entity this machine owns or loaded: a mover by its tags, then by
/// its gameplay projectile state or the descriptor provenance it spawned with.
fn classify_local(
    registry: &EntityRegistry,
    entity: EntityId,
    opt_ins: &GhostOptIns,
    movers: &GhostMovers,
) -> Option<GhostCategory> {
    if let Ok(mover) = registry.get_component::<KinematicMoverComponent>(entity) {
        return movers
            .can_ghost(mover.mover_id)
            .then_some(GhostCategory::Movers);
    }
    if let Ok(projectile) = registry.get_component::<ProjectileComponent>(entity) {
        // Every gameplay projectile records the weapon descriptor it was fired
        // from, an enemy's included; its `owner_weapon` may be a provenance id.
        let weapon = projectile.source_weapon.as_deref()?;
        return opt_ins
            .projectile(weapon)
            .then_some(GhostCategory::Projectiles);
    }
    let provenance = registry
        .get_component::<DescriptorProvenance>(entity)
        .ok()?;
    let name = provenance.canonical_name.as_str();
    match provenance.spawn_path {
        DescriptorSpawnPath::PlayerSpawn | DescriptorSpawnPath::NetworkSlot => {
            let pawn =
                registry.has_component_kind(entity, ComponentKind::PlayerMovement) == Ok(true);
            (pawn && opt_ins.descriptor(name)).then_some(GhostCategory::Players)
        }
        DescriptorSpawnPath::MapPlacement | DescriptorSpawnPath::RuntimeSpawn => {
            // A carried wieldable draws with its holder, never on its own.
            (!registry.is_inventory_owned(entity) && opt_ins.descriptor(name))
                .then_some(GhostCategory::Entities)
        }
        // A remote client's fire, flown on the host under its weapon's class.
        // A presentation copy with no body is the host's hidden mirror of a
        // gameplay projectile that classifies on its own.
        DescriptorSpawnPath::ProjectilePresentation => (has_projectile_body(registry, entity)
            && opt_ins.projectile(name))
        .then_some(GhostCategory::Projectiles),
        DescriptorSpawnPath::DefaultWeapon => None,
    }
}

fn has_projectile_body(registry: &EntityRegistry, entity: EntityId) -> bool {
    registry.get_component::<SpriteVisual>(entity).is_ok()
        || registry.get_component::<MeshComponent>(entity).is_ok()
}

/// A client display copy: only the class tag it was materialized from.
fn classify_replicated(
    class: ReplicatedDescriptorClass<'_>,
    opt_ins: &GhostOptIns,
) -> Option<GhostCategory> {
    match class {
        ReplicatedDescriptorClass::Pawn(name) => {
            opt_ins.descriptor(name).then_some(GhostCategory::Players)
        }
        ReplicatedDescriptorClass::Descriptor(name) => {
            opt_ins.descriptor(name).then_some(GhostCategory::Entities)
        }
        ReplicatedDescriptorClass::Projectile(weapon) => opt_ins
            .projectile(weapon)
            .then_some(GhostCategory::Projectiles),
    }
}

/// Visit every eligible candidate once, with its category and origin. Bounded
/// to the kinds that can be members, allocation free. The viewer's own pawn and
/// projectiles never ghost on its own machine, on any role: a client's
/// predicted shots are local entities its replicated view never offers. Movers
/// are local on every role (each peer loads them from the level file) and are
/// visited only when some link lists one.
pub(crate) fn visit_candidates(
    registry: &EntityRegistry,
    view: CandidateView<'_>,
    opt_ins: &GhostOptIns,
    movers: &GhostMovers,
    mut visit: impl FnMut(EntityId, GhostCategory, CandidateOrigin<'_>),
) {
    let viewer = crate::followed_player_pawn(registry);
    let mut offer = |entity: EntityId, origin: CandidateOrigin<'_>| {
        if Some(entity) == viewer {
            return;
        }
        if let Some(category) = classify(registry, entity, origin, opt_ins, movers) {
            visit(entity, category, origin);
        }
    };
    match view {
        CandidateView::Authoritative => {
            // Pawns, descriptor entities and remote-fire visuals carry provenance;
            // gameplay projectiles carry their own state instead.
            for (entity, _) in registry.iter_with_kind(ComponentKind::DescriptorProvenance) {
                if registry.has_component_kind(entity, ComponentKind::Projectile) != Ok(true) {
                    offer(entity, CandidateOrigin::Local);
                }
            }
            for (entity, value) in registry.iter_with_kind(ComponentKind::Projectile) {
                let ComponentValue::Projectile(projectile) = value else {
                    continue;
                };
                if viewer.is_none_or(|viewer| projectile.owner_pawn != viewer) {
                    offer(entity, CandidateOrigin::Local);
                }
            }
        }
        CandidateView::Client(replication) => {
            for (entity, class) in replication.remote_entities_by_class() {
                offer(entity, CandidateOrigin::Replicated(class));
            }
        }
    }
    if !movers.is_empty() {
        for (entity, _) in registry.iter_with_kind(ComponentKind::KinematicMover) {
            offer(entity, CandidateOrigin::Local);
        }
    }
}

#[cfg(test)]
#[path = "eligibility_tests.rs"]
mod opt_in_tests;

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
