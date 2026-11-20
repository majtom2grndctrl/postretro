#[allow(unused_imports)]
pub use postretro_foundation::{DashPrograms, MovementState, PlayerMovementComponent};

/// `pawn`'s life counter: its respawn count, or 0 for an entity that is not a
/// player pawn (an NPC never respawns).
pub fn life_counter(
    registry: &crate::registry::EntityRegistry,
    pawn: crate::registry::EntityId,
) -> u32 {
    pawn_life_counter(registry, pawn).unwrap_or(0)
}

/// `pawn`'s life counter while it is a player pawn; `None` for any other
/// entity, a despawned pawn included, whose life cannot have moved on.
pub fn pawn_life_counter(
    registry: &crate::registry::EntityRegistry,
    pawn: crate::registry::EntityId,
) -> Option<u32> {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()
        .map(|movement| movement.life_counter)
}
