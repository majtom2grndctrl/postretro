//! Physics owns collision, player movement, and deterministic kinematic movers.
//! See: context/lib/entity_model.md §7; context/lib/movement.md §1.

#![deny(unsafe_code)]

pub mod collision;
pub mod kinematic_mover;
pub mod movement;
