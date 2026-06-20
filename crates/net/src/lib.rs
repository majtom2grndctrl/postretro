// Netcode crate root: wire codec, transport, and dev harness for co-op multiplayer.
// See: context/research/netcode/
//
// This crate is glam-free and postretro-free: wire-mirror types use plain
// `[f32; N]`, and the dependency arrow only ever points postretro -> postretro-net.

pub mod transport;
pub mod wire;

#[cfg(any(test, feature = "dev-tools"))]
pub mod harness;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Smoke test so CI compiles the crate even while the modules are stubs.
        assert_eq!(2 + 2, 4);
    }
}
