//! Always-on I/O and decode counters for the SH worker threads, with
//! fixed-bucket latency histograms (no per-sample storage).
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::time::Duration;

/// Bucket upper bounds in microseconds, roughly doubling. A sample above the
/// last bound lands in an overflow bucket reported as the observed maximum.
const LATENCY_BUCKET_UPPER_MICROS: [u64; 20] = [
    50, 100, 250, 500, 1_000, 2_000, 3_000, 5_000, 7_500, 10_000, 15_000, 25_000, 50_000, 75_000,
    100_000, 150_000, 250_000, 500_000, 1_000_000, 2_500_000,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LatencyHistogram {
    counts: [u64; LATENCY_BUCKET_UPPER_MICROS.len() + 1],
    samples: u64,
    max_micros: u64,
}

impl LatencyHistogram {
    pub(crate) fn record(&mut self, elapsed: Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        let bucket = LATENCY_BUCKET_UPPER_MICROS
            .iter()
            .position(|&upper| micros <= upper)
            .unwrap_or(LATENCY_BUCKET_UPPER_MICROS.len());
        self.counts[bucket] = self.counts[bucket].saturating_add(1);
        self.samples = self.samples.saturating_add(1);
        self.max_micros = self.max_micros.max(micros);
    }

    /// Upper bound of the bucket holding the `fraction` quantile, never above
    /// the observed maximum. Zero before the first sample.
    pub(crate) fn quantile_ms(&self, fraction: f64) -> f32 {
        if self.samples == 0 {
            return 0.0;
        }
        let rank = ((self.samples as f64) * fraction).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (bucket, &count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(count);
            if seen >= rank {
                let upper = LATENCY_BUCKET_UPPER_MICROS
                    .get(bucket)
                    .copied()
                    .unwrap_or(self.max_micros);
                return micros_to_ms(upper.min(self.max_micros));
            }
        }
        micros_to_ms(self.max_micros)
    }

    pub(crate) fn max_ms(&self) -> f32 {
        micros_to_ms(self.max_micros)
    }
}

/// Cumulative since the worker set started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShWorkerStats {
    /// Physical reads issued.
    pub(crate) reads_issued: u64,
    /// Physical reads that served more than one chunk.
    pub(crate) coalesced_reads: u64,
    /// Bytes physically read, gap bytes included.
    pub(crate) read_bytes: u64,
    /// Bytes read only to bridge chunks and then discarded.
    pub(crate) gap_bytes: u64,
    /// Per chunk: frame-thread submission to encoded bytes in hand.
    pub(crate) read_latency: LatencyHistogram,
    /// Per chunk: decode and verification time on a pool thread.
    pub(crate) decode_latency: LatencyHistogram,
}

impl ShWorkerStats {
    pub(crate) fn record_read(&mut self, span_bytes: u64, chunk_count: usize, gap_bytes: u64) {
        self.reads_issued = self.reads_issued.saturating_add(1);
        if chunk_count > 1 {
            self.coalesced_reads = self.coalesced_reads.saturating_add(1);
        }
        self.read_bytes = self.read_bytes.saturating_add(span_bytes);
        self.gap_bytes = self.gap_bytes.saturating_add(gap_bytes);
    }
}

fn micros_to_ms(micros: u64) -> f32 {
    (micros as f64 / 1000.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn histogram(samples_micros: &[u64]) -> LatencyHistogram {
        let mut histogram = LatencyHistogram::default();
        for &micros in samples_micros {
            histogram.record(Duration::from_micros(micros));
        }
        histogram
    }

    #[test]
    fn empty_histogram_reports_zero() {
        let empty = LatencyHistogram::default();
        assert_eq!(empty.quantile_ms(0.5), 0.0);
        assert_eq!(empty.max_ms(), 0.0);
    }

    #[test]
    fn quantiles_report_bucket_upper_bounds_capped_at_the_maximum() {
        // 90 fast samples in the <=1 ms bucket, 10 slow ones near 40 ms.
        let mut samples = vec![900; 90];
        samples.extend([40_000; 10]);
        let histogram = histogram(&samples);
        assert_eq!(histogram.quantile_ms(0.5), 1.0);
        assert_eq!(
            histogram.quantile_ms(0.95),
            40.0,
            "50 ms bucket capped at max"
        );
        assert_eq!(histogram.max_ms(), 40.0);
    }

    #[test]
    fn samples_beyond_the_last_bucket_report_the_observed_maximum() {
        let histogram = histogram(&[9_000_000]);
        assert_eq!(histogram.quantile_ms(0.5), 9000.0);
        assert_eq!(histogram.max_ms(), 9000.0);
    }

    #[test]
    fn read_counters_count_coalesced_reads_and_gap_bytes() {
        let mut stats = ShWorkerStats::default();
        stats.record_read(100, 1, 0);
        stats.record_read(300, 3, 40);
        assert_eq!(stats.reads_issued, 2);
        assert_eq!(stats.coalesced_reads, 1);
        assert_eq!(stats.read_bytes, 400);
        assert_eq!(stats.gap_bytes, 40);
    }
}
