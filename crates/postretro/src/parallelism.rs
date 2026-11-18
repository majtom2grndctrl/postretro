//! The core count every engine thread pool sizes itself from.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/boot_sequence.md §2

// One seam, so a measurement can size every pool as a smaller machine would
// (macOS cannot pin a process to cores). Tests set an emulated count on the
// calling thread: pools are sized on the thread that builds them, so an
// emulation never leaks into another test running in parallel.

#[cfg(test)]
use std::cell::Cell;

/// The cores this machine offers, or the count a test on this thread
/// emulates. At least 1.
pub(crate) fn available_parallelism() -> usize {
    #[cfg(test)]
    if let Some(cores) = EMULATED_CORES.with(Cell::get) {
        return cores;
    }
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

#[cfg(test)]
thread_local! {
    static EMULATED_CORES: Cell<Option<usize>> = const { Cell::new(None) };
}

/// While alive, [`available_parallelism`] on this thread returns the emulated
/// count; dropping it restores the previous one.
#[cfg(test)]
#[must_use = "the emulation ends when this guard drops"]
pub(crate) struct EmulatedCores {
    previous: Option<usize>,
}

#[cfg(test)]
impl EmulatedCores {
    pub(crate) fn new(cores: usize) -> Self {
        assert!(cores >= 1, "an emulated machine has at least one core");
        Self {
            previous: EMULATED_CORES.with(|cell| cell.replace(Some(cores))),
        }
    }
}

#[cfg(test)]
impl Drop for EmulatedCores {
    fn drop(&mut self) {
        EMULATED_CORES.with(|cell| cell.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulation_overrides_on_this_thread_only_and_restores() {
        let machine = available_parallelism();
        {
            let _one = EmulatedCores::new(1);
            assert_eq!(available_parallelism(), 1);
            {
                let _two = EmulatedCores::new(2);
                assert_eq!(available_parallelism(), 2);
            }
            assert_eq!(available_parallelism(), 1);
            let other = std::thread::spawn(available_parallelism).join().unwrap();
            assert_eq!(other, machine, "another thread sees the machine");
        }
        assert_eq!(available_parallelism(), machine);
    }
}
