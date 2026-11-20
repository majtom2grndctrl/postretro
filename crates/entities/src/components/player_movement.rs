#[allow(unused_imports)]
pub use postretro_foundation::{DashPrograms, MovementState, PlayerMovementComponent};

/// `pawn`'s life counter: its respawn count, or 0 for an entity that is not a
/// player pawn (an NPC never respawns).
pub fn life_counter(
    registry: &crate::registry::EntityRegistry,
    pawn: crate::registry::EntityId,
) -> u32 {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .map_or(0, |movement| movement.life_counter)
}
