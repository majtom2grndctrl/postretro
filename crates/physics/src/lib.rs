//! Physics owns collision, player movement, and deterministic kinematic movers.
//! See: context/lib/entity_model.md §7; context/lib/movement.md §1.

#![deny(unsafe_code)]

pub mod collision;
pub mod environment_volumes;
pub mod kinematic_mover;
pub mod movement;

pub mod teleporters;

// Test-only: makes `alloc_probe` count allocations in this test binary.
// See: context/lib/testing_guide.md §Resource bounds
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: postretro_sim::alloc_probe::CountingAllocator =
    postretro_sim::alloc_probe::CountingAllocator;

#[cfg(test)]
mod alloc_probe_tests {
    use postretro_sim::alloc_probe::AllocSnapshot;

    /// Positive control: the probe must see a real allocation, or every
    /// zero-allocation assertion in this crate is dead.
    #[test]
    fn probe_counts_a_real_allocation() {
        let snapshot = AllocSnapshot::arm();
        let v: Vec<u64> = std::hint::black_box(Vec::with_capacity(16));
        let allocs = snapshot.allocs_since();
        drop(v);
        assert!(allocs > 0, "probe must observe a Vec allocation");
    }
}
