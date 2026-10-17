// postretro-net crate root: wire codec, transport, and the dev latency harness
// and lag proxy for co-op multiplayer.
// See: context/lib/networking.md
//
// This crate is glam-free and postretro-free: wire-mirror types use plain
// `[f32; N]`, and the dependency arrow only ever points postretro -> postretro-net.

pub mod handshake;
pub mod replication;
pub use replication::JOIN_BASELINE_BUDGET_BYTES;
pub mod slots;
pub mod state_replication;
pub mod state_slots;
pub mod timesync;
pub mod transport;
pub mod wire;

#[cfg(any(test, feature = "dev-tools"))]
pub mod harness;
#[cfg(feature = "dev-tools")]
pub mod lag_proxy;
