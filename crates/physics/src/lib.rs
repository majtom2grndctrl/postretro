//! Physics owns collision, player movement, and deterministic kinematic movers.
//! See: context/lib/entity_model.md §7; context/lib/movement.md §1.

#![deny(unsafe_code)]

pub mod collision;
pub mod environment_volumes;
pub mod kinematic_mover;
pub mod movement;

pub mod teleporters;

// Test-only counting global allocator. Without it `alloc_probe`'s per-thread
// counter never moves in this crate's test binary and every zero-allocation
// assertion passes vacuously. `#[global_allocator]` must sit on a crate-root
// static; the approved `unsafe impl` stays in `postretro_sim::alloc_probe`.
// See: context/lib/development_guide.md §3.5
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: postretro_sim::alloc_probe::CountingAllocator =
    postretro_sim::alloc_probe::CountingAllocator;

#[cfg(test)]
mod alloc_probe_tests {
    use postretro_sim::alloc_probe::{AllocSnapshot, counting_allocator_is_installed};

    /// Positive control: the probe must see a real allocation, or every
    /// zero-allocation assertion in this crate is dead.
    #[test]
    fn alloc_probe_counts_a_real_allocation() {
        assert!(counting_allocator_is_installed());
        let snapshot = AllocSnapshot::arm();
        let v: Vec<u64> = std::hint::black_box(Vec::with_capacity(16));
        let allocs = snapshot.allocs_since();
        drop(v);
        assert!(allocs > 0, "probe must observe a Vec allocation");
    }
}
