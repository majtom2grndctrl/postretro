//! The read issuer's busy time over one walk, as p95 and max.
//! See: context/lib/rendering_pipeline.md §4 (one issuer and one budget)

// Samples come from the issuer's own busy log (`streaming::issuer_busy`): one
// per physical read, timed from the read's byte charge through its split and
// every delivery it completed. Idle time waiting for requests is never in it.
// The settled-entry preload reads on the calling thread, not the issuer, so
// it is not in it either.

use super::report::max_and_p95;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct BusyStats {
    /// Physical reads the issuer made.
    pub(super) reads: usize,
    pub(super) p95_ms: f64,
    pub(super) max_ms: f64,
    /// Every read's busy time summed.
    pub(super) total_ms: f64,
}

impl BusyStats {
    /// Nearest-rank p95 and the max of `nanos`, one sample per read.
    pub(super) fn of(nanos: &[u64]) -> Self {
        let (max, p95) = max_and_p95(nanos.to_vec());
        Self {
            reads: nanos.len(),
            p95_ms: p95 as f64 / 1e6,
            max_ms: max as f64 / 1e6,
            total_ms: nanos.iter().sum::<u64>() as f64 / 1e6,
        }
    }

    pub(super) fn line(&self) -> String {
        format!(
            "{} physical reads, busy p95 {:.3} ms, max {:.3} ms, total {:.1} ms",
            self.reads, self.p95_ms, self.max_ms, self.total_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn p95_and_max_of_a_known_sample() {
        // 1..=100 ms in shuffled order: the 95th of 100 is 95 ms.
        let mut nanos: Vec<u64> = (1..=100).map(|ms| ms * MS).collect();
        nanos.reverse();
        nanos.swap(3, 70);
        let stats = BusyStats::of(&nanos);
        assert_eq!(stats.reads, 100);
        assert_eq!(stats.p95_ms, 95.0);
        assert_eq!(stats.max_ms, 100.0);
        assert_eq!(stats.total_ms, 5050.0);
    }

    #[test]
    fn p95_rounds_its_rank_up() {
        // ceil(0.95 * 10) = 10th of 10: the max.
        let nanos: Vec<u64> = (1..=10).map(|ms| ms * MS).collect();
        assert_eq!(BusyStats::of(&nanos).p95_ms, 10.0);
        // ceil(0.95 * 21) = 20th of 21.
        let nanos: Vec<u64> = (1..=21).map(|ms| ms * MS).collect();
        assert_eq!(BusyStats::of(&nanos).p95_ms, 20.0);
    }

    #[test]
    fn no_reads_is_all_zero() {
        assert_eq!(BusyStats::of(&[]), BusyStats::default());
    }
}
