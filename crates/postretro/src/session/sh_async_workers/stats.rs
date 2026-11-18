//! Always-on I/O and decode counters for SH's issuer route and decode jobs.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use crate::streaming::latency_histogram::LatencyHistogram;

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
    /// Per chunk: submission to the decode pool to its decode starting.
    pub(crate) decode_queue_wait: LatencyHistogram,
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

#[cfg(test)]
mod tests {
    use super::*;

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
