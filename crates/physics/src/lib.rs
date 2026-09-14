//! Collision, player movement, and deterministic kinematic movers.
//!
//! Fixed-tick orchestration remains in `postretro-sim`; this crate is the
//! stable physical substrate shared by sim, AI, netcode, and the binary.

#![deny(unsafe_code)]

pub mod collision;
pub mod kinematic_mover;
pub mod movement;
