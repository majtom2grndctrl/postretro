// Netcode crate root: wire codec, transport, and dev harness for co-op multiplayer.
// See: context/lib/networking.md
//
// This crate is glam-free and postretro-free: wire-mirror types use plain
// `[f32; N]`, and the dependency arrow only ever points postretro -> postretro-net.

pub mod replication;
pub mod slots;
pub mod state_replication;
pub mod state_slots;
pub mod timesync;
pub mod transport;
pub mod wire;

#[cfg(any(test, feature = "dev-tools"))]
pub mod harness;
