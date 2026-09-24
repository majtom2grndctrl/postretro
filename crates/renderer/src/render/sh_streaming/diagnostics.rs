//! Always-on streaming counters and the plain-data live diagnostics view.
//!
//! Nothing here is feature-gated: the application's periodic log line reads
//! these values in every build, and the dev-tools panel only renders them.

use std::time::Duration;

use super::ShResidencySnapshot;

/// Cumulative CPU time the renderer spent installing ready clusters, measured
/// once per drain that carried at least one ready cluster.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct InstallCpuCounters {
    pub(super) total_micros: u64,
    pub(super) max_drain_micros: u64,
    pub(super) last_drain_micros: u64,
}

impl InstallCpuCounters {
    pub(super) fn record_drain(&mut self, elapsed: Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.total_micros = self.total_micros.saturating_add(micros);
        self.max_drain_micros = self.max_drain_micros.max(micros);
        self.last_drain_micros = micros;
    }
}

/// Cumulative physical growth of the streamed pools. One event is one pool
/// family (the coupled dense atlas, or one sparse CSR family) whose capacity
/// grew; bytes are the active physical capacity each growth transaction added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PoolGrowthCounters {
    pub(super) events: u64,
    pub(super) bytes: u64,
}

impl PoolGrowthCounters {
    pub(super) fn record(&mut self, pools_grown: usize, previous_bytes: u64, grown_bytes: u64) {
        self.events = self
            .events
            .saturating_add(u64::try_from(pools_grown).unwrap_or(u64::MAX));
        self.bytes = self
            .bytes
            .saturating_add(grown_bytes.saturating_sub(previous_bytes));
    }
}

/// One frame's view of SH streaming for the dev-tools Streaming tab and the
/// periodic log. Plain data: gauges are current values, every other field is a
/// cumulative count since the controller or renderer state was created.
///
/// The application fills controller and worker fields; the renderer fields
/// come from [`ShResidencySnapshot`] via [`Self::record_renderer_snapshot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShStreamingLiveDiagnostics {
    // Gauges.
    pub target_clusters: u64,
    pub warm_clusters: u64,
    pub sampleable_clusters: u64,
    pub queued_clusters: u64,
    pub ready_clusters: u64,
    pub permits_in_use: u64,
    pub active_capacity_bytes: u64,
    pub logical_occupancy_bytes: u64,
    // Controller counters.
    pub misses: u64,
    pub installs: u64,
    pub evictions: u64,
    pub retries: u64,
    pub cancelled_requests: u64,
    pub discarded_reads: u64,
    pub discarded_read_bytes: u64,
    pub decoded_bytes_installed: u64,
    pub last_drain_decoded_bytes: u64,
    pub max_drain_decoded_bytes: u64,
    pub budget_limited_drains: u64,
    // Worker counters. `reads_issued` counts physical reads;
    // `coalesced_reads` counts physical reads that covered more than one chunk.
    pub reads_issued: u64,
    pub coalesced_reads: u64,
    pub read_bytes: u64,
    pub gap_bytes: u64,
    pub read_latency_p50_ms: f32,
    pub read_latency_p95_ms: f32,
    pub read_latency_max_ms: f32,
    pub decode_latency_max_ms: f32,
    // Renderer counters.
    pub install_cpu_total_micros: u64,
    pub install_cpu_max_drain_micros: u64,
    pub install_cpu_last_drain_micros: u64,
    pub pool_growth_events: u64,
    pub pool_growth_bytes: u64,
}

impl ShStreamingLiveDiagnostics {
    /// Copy the renderer-owned gauges and counters from a residency snapshot.
    /// Controller and worker fields are left untouched.
    pub fn record_renderer_snapshot(&mut self, snapshot: &ShResidencySnapshot) {
        self.sampleable_clusters = u64::try_from(snapshot.sampleable_clusters).unwrap_or(u64::MAX);
        self.active_capacity_bytes = snapshot.active_capacity_bytes;
        self.logical_occupancy_bytes = snapshot.logical_occupancy_bytes;
        self.install_cpu_total_micros = snapshot.install_cpu_total_micros;
        self.install_cpu_max_drain_micros = snapshot.install_cpu_max_drain_micros;
        self.install_cpu_last_drain_micros = snapshot.install_cpu_last_drain_micros;
        self.pool_growth_events = snapshot.pool_growth_events;
        self.pool_growth_bytes = snapshot.pool_growth_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_cpu_counters_track_total_max_and_last_drain() {
        let mut counters = InstallCpuCounters::default();
        counters.record_drain(Duration::from_micros(40));
        counters.record_drain(Duration::from_micros(90));
        counters.record_drain(Duration::from_micros(15));
        assert_eq!(
            counters,
            InstallCpuCounters {
                total_micros: 145,
                max_drain_micros: 90,
                last_drain_micros: 15,
            }
        );
    }

    #[test]
    fn install_cpu_counters_saturate_instead_of_wrapping() {
        let mut counters = InstallCpuCounters {
            total_micros: u64::MAX - 1,
            ..InstallCpuCounters::default()
        };
        counters.record_drain(Duration::from_micros(10));
        assert_eq!(counters.total_micros, u64::MAX);
    }

    #[test]
    fn pool_growth_counts_each_family_and_the_capacity_added() {
        let mut counters = PoolGrowthCounters::default();
        counters.record(1, 1024, 4096);
        counters.record(2, 4096, 6144);
        assert_eq!(
            counters,
            PoolGrowthCounters {
                events: 3,
                bytes: 3072 + 2048,
            }
        );
    }

    #[test]
    fn renderer_snapshot_fills_only_renderer_owned_fields() {
        let mut live = ShStreamingLiveDiagnostics {
            target_clusters: 9,
            reads_issued: 4,
            ..ShStreamingLiveDiagnostics::default()
        };
        live.record_renderer_snapshot(&ShResidencySnapshot {
            sampleable_clusters: 3,
            active_capacity_bytes: 2048,
            logical_occupancy_bytes: 512,
            install_cpu_total_micros: 700,
            install_cpu_max_drain_micros: 300,
            install_cpu_last_drain_micros: 100,
            pool_growth_events: 2,
            pool_growth_bytes: 4096,
            ..ShResidencySnapshot::default()
        });
        assert_eq!(live.target_clusters, 9);
        assert_eq!(live.reads_issued, 4);
        assert_eq!(live.sampleable_clusters, 3);
        assert_eq!(live.active_capacity_bytes, 2048);
        assert_eq!(live.logical_occupancy_bytes, 512);
        assert_eq!(live.install_cpu_total_micros, 700);
        assert_eq!(live.install_cpu_max_drain_micros, 300);
        assert_eq!(live.install_cpu_last_drain_micros, 100);
        assert_eq!(live.pool_growth_events, 2);
        assert_eq!(live.pool_growth_bytes, 4096);
    }
}
