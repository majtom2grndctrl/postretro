//! SH streaming beside the lightmap in the residency walks (`POSTRETRO_LIGHTMAP_WALK_SH`).
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency, one issuer and one budget)

// With SH on, the level's cluster SH streams as in play: its production
// controller in async mode, its route on the one issuer, its decode pool, and
// one shared drain budget with the lightmap. Only the renderer is stood in
// for: every drain's ready clusters are accepted whole (no pool capacity, no
// deferral, eviction or refusal), and compose counts as submitted once per
// frame, after the frame's last drain, so a cluster accepted in frame N
// samples from frame N+1, as in play. Late SH chunks are the controller's
// visible misses: one per episode of a drawn cluster that is not sampleable.
//
// In completed-reads mode a gate between the workers and the session holds
// each drain until every SH request in flight has delivered, as the lightmap
// gate does, so the SH-on run stays deterministic at any decode-pool size.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::Result;
use postretro_level_loader::{LevelWorld, ShDrainBatch, ShDrainOutcome, ShStreamingMode};

use crate::session::sh_async_workers::{ShWorkerCompletion, ShWorkerStats};
use crate::session::sh_residency::ShStreamingSession;
use crate::sh_streaming::controller::{ClusterResidencyState, ShResidencyCounters};
use crate::streaming::cluster_hints::ClusterHints;

const SH_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_SH";
/// A read this slow means the issuer or a decode thread stalled.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// Whether this run asks for SH streaming; `off` (the default) keeps the
/// harness's original lightmap-only walk.
pub(super) fn sh_requested_from_env() -> bool {
    match std::env::var(SH_ENV).ok().as_deref().map(str::trim) {
        None | Some("" | "off" | "0") => false,
        Some("on" | "1") => true,
        Some(other) => panic!("{SH_ENV} = on | off, not {other:?}"),
    }
}

/// An async SH session over `world`'s id 50, or `None` when the level has
/// none (or loaded with `POSTRETRO_SH_STREAMING=off`).
pub(super) fn sh_session(world: &LevelWorld) -> Option<ShStreamingSession> {
    let manifest = world.sh_stream_manifest()?;
    let hints = ClusterHints::decode(manifest.cluster_directory()).expect("id-49 hints");
    let mut session = ShStreamingSession::from_manifest(manifest.clone(), hints.into())
        .expect("SH streaming session");
    session.set_mode_for_test(ShStreamingMode::Async);
    Some(session)
}

/// The renderer's SH drain, stood in: every ready cluster accepted. Compose
/// submission is the caller's, once per frame.
pub(super) fn accept_all(session: &mut ShStreamingSession, batch: &ShDrainBatch) -> Result<()> {
    session.apply_controller_outcome(ShDrainOutcome {
        accepted: batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect(),
        ..ShDrainOutcome::default()
    })
}

/// Completed-reads mode for SH: interposes on the workers' completion queue
/// and forwards in delivery order once every request in flight has landed.
pub(super) struct ShCompletionGate {
    upstream: Receiver<ShWorkerCompletion>,
    forward: Sender<ShWorkerCompletion>,
    /// Clusters whose completion has not yet arrived; reused.
    owed: Vec<u32>,
}

impl ShCompletionGate {
    /// `None` until the session's workers attach (the first drain spawns
    /// them); call again before the next drain.
    pub(super) fn install(session: &mut ShStreamingSession) -> Option<Self> {
        let (forward, receiver) = channel();
        let upstream = session.interpose_worker_completions(receiver)?;
        Some(Self {
            upstream,
            forward,
            owed: Vec::new(),
        })
    }

    /// Blocks until every cluster the controller holds `Queued` (submitted,
    /// completion not yet admitted) has delivered, and forwards each for the
    /// next drain to admit.
    pub(super) fn await_in_flight(&mut self, session: &ShStreamingSession) {
        let controller = session.controller();
        self.owed.clear();
        self.owed.extend(
            (0u32..)
                .map_while(|cluster| controller.state(cluster).map(|state| (cluster, state)))
                .filter(|&(_, state)| state == ClusterResidencyState::Queued)
                .map(|(cluster, _)| cluster),
        );
        while !self.owed.is_empty() {
            let completion = self
                .upstream
                .recv_timeout(COMPLETION_TIMEOUT)
                .expect("every SH request in flight completes");
            if let Ok(at) = self.owed.binary_search(&completion.request.cluster_id) {
                self.owed.remove(at);
            }
            self.forward
                .send(completion)
                .expect("the workers hold the forward queue");
        }
    }
}

/// One run's SH outcome, read from the controller's cumulative counters.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ShRunStats {
    /// Visible misses by the end of the spawn step.
    pub(super) spawn_misses: u64,
    pub(super) counters: ShResidencyCounters,
    /// The workers' read and decode timings over the whole run.
    pub(super) workers: Option<ShWorkerStats>,
}

impl ShRunStats {
    /// Late SH chunks after the spawn step: visible-miss episodes.
    pub(super) fn late_after_spawn(&self) -> u64 {
        self.counters.misses - self.spawn_misses
    }

    pub(super) fn line(&self) -> String {
        let c = &self.counters;
        let b = &c.miss_buckets;
        let timings = self.workers.map_or_else(
            || "no worker timings".to_string(),
            |workers| {
                format!(
                    "read {}; decode queue wait {}; decode {}",
                    workers.read_latency.summary(),
                    workers.decode_queue_wait.summary(),
                    workers.decode_latency.summary(),
                )
            },
        );
        format!(
            "late chunks (visible-miss episodes) spawn step {}, after spawn {}; by cause (whole \
             run): outside reach {}, read in flight {}, held by drain budget {}, awaiting \
             compose {}, trimmed {}, failed {}; installs {}, evictions {}, re-reads {}, \
             retries {}, cancelled {}, discarded {}; budget-limited drains {}; max drain \
             decoded {:.2} MiB; {timings}",
            self.spawn_misses,
            self.late_after_spawn(),
            b.outside_reach,
            b.read_in_flight,
            b.held_by_drain_budget,
            b.awaiting_compose,
            b.trimmed_by_pressure,
            b.failed,
            c.installs,
            c.evictions,
            c.rereads,
            c.retries,
            c.cancelled_requests,
            c.discarded_reads,
            c.budget_limited_drains,
            super::report::mib(c.max_drain_decoded_bytes),
        )
    }
}
