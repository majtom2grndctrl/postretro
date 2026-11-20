//! Live lightmap streaming diagnostics: one plain-data view assembled from the
//! controller, the read route, the level's file-read counters, and the
//! renderer's pool, plus the periodic `[Lightmap streaming]` log line.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::SectionId;
use postretro_level_loader::PrlReadCounters;
use postretro_renderer::{LightmapStreamCounters, LightmapStreamingLiveDiagnostics};

use super::sh_streaming_diagnostics::DIAGNOSTICS_LOG_INTERVAL_SECONDS;
use crate::lightmap_streaming::controller::LightmapResidencyController;
use crate::lightmap_streaming::route::LightmapRouteLedger;

/// Bytes read per PRL section. The level's [`PrlReadCounters`] in the game
/// and capture; tests substitute fixed values.
pub(crate) trait SectionReadBytes {
    fn section_bytes(&self, section_id: u32) -> u64;
}

impl SectionReadBytes for PrlReadCounters {
    fn section_bytes(&self, section_id: u32) -> u64 {
        PrlReadCounters::section_bytes(self, section_id)
    }
}

/// Refreshes `live` in place from every source. Field writes only: a steady
/// frame allocates nothing. A `None` renderer or read source leaves those
/// fields as they were.
pub(crate) fn assemble_live_diagnostics(
    live: &mut LightmapStreamingLiveDiagnostics,
    controller: &LightmapResidencyController,
    ledger: &LightmapRouteLedger,
    renderer: Option<&LightmapStreamCounters>,
    reads: Option<&dyn SectionReadBytes>,
) {
    live.lead_metres = controller.lead_metres();
    live.max_lead_metres = controller.max_lead_metres();

    let residency = controller.residency_bytes();
    live.block_count = controller.block_count() as u64;
    live.resident_blocks = u64::from(residency.resident_blocks);
    live.resident_lightmap_bytes = residency.resident_lightmap_bytes;
    live.resident_shadowmask_bytes = residency.resident_shadowmask_bytes;
    live.mandatory_blocks = u64::from(residency.mandatory_blocks);
    live.mandatory_lightmap_bytes = residency.mandatory_lightmap_bytes;
    live.mandatory_shadowmask_bytes = residency.mandatory_shadowmask_bytes;
    let report = controller.pool_report();
    live.band_headroom_texels = report.map(|pool| pool.band_headroom_texels);
    live.in_flight_read_bytes = ledger.in_memory_bytes();
    live.physical_reads = ledger.physical_reads();

    let counters = controller.counters();
    live.reads_requested = counters.reads_requested;
    live.installs = counters.installs;
    live.evictions = counters.evictions;
    live.refusals = counters.refusals;
    live.deferrals = counters.deferrals;
    live.failed_reads = counters.failed_reads;
    live.failed_installs = counters.failed_installs;
    live.cancelled_reads = counters.cancelled_reads;
    live.drawn_outside_baked_set = counters.drawn_outside_baked_set;
    live.drawn_not_resident = counters.drawn_not_resident;
    live.last_frame_drawn_outside_baked_set = counters.last_frame_drawn_outside_baked_set;
    live.last_frame_drawn_not_resident = counters.last_frame_drawn_not_resident;
    live.miss_buckets = counters.miss_buckets;

    if let Some(reads) = reads {
        live.lightmap_bytes_read = reads.section_bytes(SectionId::Lightmap as u32);
        live.shadowmask_bytes_read = reads.section_bytes(SectionId::ShadowmaskAtlas as u32);
    }
    if let Some(renderer) = renderer {
        live.record_renderer_counters(renderer);
    }
}

/// Throttles the periodic log line, on [`ShStreamingLogWindow`]'s cadence: a
/// line needs a full interval since the previous one and a change since then,
/// so an idle level stays silent. The last-frame miss gauges alone are not a
/// change; the cumulative miss counts behind them are.
///
/// [`ShStreamingLogWindow`]: super::sh_streaming_diagnostics::ShStreamingLogWindow
///
/// Each line is also a telemetry window; the session's end closes a final
/// partial one, which has no line.
#[derive(Debug)]
pub(crate) struct LightmapStreamingLogWindow {
    session: u64,
    window_start: Option<f64>,
    /// The latest observed time and snapshot, which a session's end closes
    /// its partial window at. Plain copies: observing allocates nothing.
    last_seconds: f64,
    last_live: LightmapStreamingLiveDiagnostics,
    baseline: LightmapStreamingLiveDiagnostics,
}

impl Default for LightmapStreamingLogWindow {
    fn default() -> Self {
        Self {
            session: super::streaming_window_events::next_streaming_session(),
            window_start: None,
            last_seconds: 0.0,
            last_live: LightmapStreamingLiveDiagnostics::default(),
            baseline: LightmapStreamingLiveDiagnostics::default(),
        }
    }
}

impl Drop for LightmapStreamingLogWindow {
    fn drop(&mut self) {
        self.finish();
    }
}

impl LightmapStreamingLogWindow {
    pub(crate) fn observe(&mut self, now_seconds: f64, live: &LightmapStreamingLiveDiagnostics) {
        self.last_seconds = now_seconds;
        self.last_live = *live;
        if let Some(line) = self.line_if_due(now_seconds, live) {
            log::info!("{line}");
        }
    }

    /// The session ends (it drops with the window): a window that has
    /// counted something since its last line closes as a partial, with no
    /// line. An idle one emits nothing.
    pub(crate) fn finish(&mut self) {
        let Some(start) = self.window_start.take() else {
            return;
        };
        if without_frame_gauges(&self.last_live) == without_frame_gauges(&self.baseline) {
            return;
        }
        super::streaming_window_events::emit_lightmap_window(
            self.session,
            true,
            self.last_seconds - start,
            &self.baseline,
            &self.last_live,
        );
    }

    fn line_if_due(
        &mut self,
        now_seconds: f64,
        live: &LightmapStreamingLiveDiagnostics,
    ) -> Option<String> {
        let start = *self.window_start.get_or_insert(now_seconds);
        let elapsed = now_seconds - start;
        if elapsed < DIAGNOSTICS_LOG_INTERVAL_SECONDS
            || without_frame_gauges(live) == without_frame_gauges(&self.baseline)
        {
            return None;
        }
        let line = format_line(elapsed, &self.baseline, live);
        super::streaming_window_events::emit_lightmap_window(
            self.session,
            false,
            elapsed,
            &self.baseline,
            live,
        );
        self.baseline = *live;
        self.window_start = Some(now_seconds);
        Some(line)
    }
}

fn without_frame_gauges(
    live: &LightmapStreamingLiveDiagnostics,
) -> LightmapStreamingLiveDiagnostics {
    LightmapStreamingLiveDiagnostics {
        last_frame_drawn_outside_baked_set: 0,
        last_frame_drawn_not_resident: 0,
        ..*live
    }
}

/// Window deltas first, then current gauges and running maxima, on one line.
fn format_line(
    elapsed_seconds: f64,
    before: &LightmapStreamingLiveDiagnostics,
    now: &LightmapStreamingLiveDiagnostics,
) -> String {
    let delta =
        |pick: fn(&LightmapStreamingLiveDiagnostics) -> u64| pick(now).saturating_sub(pick(before));
    format!(
        "[Lightmap streaming] last {elapsed_seconds:.1} s: {requested} pairs requested \
         ({reads} reads, id 22 {read_22}, id 42 {read_42}), {installs} installs, \
         {evictions} evictions, {refusals} refusals, {deferrals} deferrals, \
         {failed_installs} failed installs, {failed_reads} failed reads, {repacks} repacks \
         ({mask_repacks} mask), {compactions} compactions ({mask_compactions} mask), \
         {rollbacks} compaction rollbacks ({mask_rollbacks} mask), {growths} growths \
         ({mask_growths} mask), visible misses in block-frames (may overlap) {outside} \
         outside baked set, {not_resident} not resident (reach {miss_reach}, in flight \
         {miss_in_flight}, permit {miss_permit}, in-hand cap {miss_in_hand}, budget \
         {miss_budget}, renderer drain {miss_renderer}, deferred {miss_deferred}, refused \
         {miss_refused}, failed {miss_failed}) \
         | now: lead {lead:.1} of {max_lead:.1} m, resident {resident_blocks} of {blocks} \
         blocks {resident} (id 22 {resident_22}, id 42 {resident_42}), mandatory \
         {mandatory_blocks} blocks {mandatory} (id 22 {mandatory_22}, id 42 {mandatory_42}), \
         pool {layers} layers (peak {peak}) {pool} + {retiring} retiring, \
         headroom {headroom} texels, mask pool {mask_layers} layers (peak {mask_peak}) \
         {mask_pool} + {mask_retiring} retiring, headroom {mask_headroom} texels, growth peak {growth_peak}, install last {install_last_ms:.2} ms / max \
         {install_max_ms:.2} ms",
        requested = delta(|d| d.reads_requested),
        reads = delta(|d| d.physical_reads),
        read_22 = format_bytes(delta(|d| d.lightmap_bytes_read)),
        read_42 = format_bytes(delta(|d| d.shadowmask_bytes_read)),
        installs = delta(|d| d.installs),
        evictions = delta(|d| d.evictions),
        refusals = delta(|d| d.refusals),
        deferrals = delta(|d| d.deferrals),
        failed_installs = delta(|d| d.failed_installs),
        failed_reads = delta(|d| d.failed_reads),
        repacks = delta(|d| d.repacks.lightmap),
        mask_repacks = delta(|d| d.repacks.mask),
        compactions = delta(|d| d.compactions.lightmap),
        mask_compactions = delta(|d| d.compactions.mask),
        rollbacks = delta(|d| d.compaction_rollbacks.lightmap),
        mask_rollbacks = delta(|d| d.compaction_rollbacks.mask),
        growths = delta(|d| d.growths.lightmap),
        mask_growths = delta(|d| d.growths.mask),
        outside = delta(|d| d.drawn_outside_baked_set),
        not_resident = delta(|d| d.drawn_not_resident),
        miss_reach = delta(|d| d.miss_buckets.outside_reach),
        miss_in_flight = delta(|d| d.miss_buckets.read_in_flight),
        miss_permit = delta(|d| d.miss_buckets.waiting_on_permit),
        miss_in_hand = delta(|d| d.miss_buckets.waiting_on_in_hand_cap),
        miss_budget = delta(|d| d.miss_buckets.held_by_drain_budget),
        miss_renderer = delta(|d| d.miss_buckets.awaiting_renderer_drain),
        miss_deferred = delta(|d| d.miss_buckets.deferred_by_pool),
        miss_refused = delta(|d| d.miss_buckets.refused),
        miss_failed = delta(|d| d.miss_buckets.failed),
        lead = now.lead_metres,
        max_lead = now.max_lead_metres,
        resident_blocks = now.resident_blocks,
        blocks = now.block_count,
        resident = format_bytes(now.resident_bytes()),
        resident_22 = format_bytes(now.resident_lightmap_bytes),
        resident_42 = format_bytes(now.resident_shadowmask_bytes),
        mandatory_blocks = now.mandatory_blocks,
        mandatory = format_bytes(now.mandatory_bytes()),
        mandatory_22 = format_bytes(now.mandatory_lightmap_bytes),
        mandatory_42 = format_bytes(now.mandatory_shadowmask_bytes),
        layers = now.pool_layers.lightmap,
        peak = now.peak_pool_layers.lightmap,
        pool = format_bytes(now.pool_bytes.lightmap),
        retiring = format_bytes(now.retiring_pool_bytes.lightmap),
        headroom = now.band_headroom_texels.lightmap,
        mask_layers = now.pool_layers.mask,
        mask_peak = now.peak_pool_layers.mask,
        mask_pool = format_bytes(now.pool_bytes.mask),
        mask_retiring = format_bytes(now.retiring_pool_bytes.mask),
        mask_headroom = now.band_headroom_texels.mask,
        growth_peak = format_bytes(now.growth_transient_peak_bytes),
        install_last_ms = now.last_drain_install_micros as f64 / 1000.0,
        install_max_ms = now.max_drain_install_micros as f64 / 1000.0,
    )
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use postretro_level_format::lightmap_pool::PerPool;
    use std::collections::BTreeMap;

    use postretro_level_loader::{LightmapPoolReport, PoolReport};
    use postretro_test_log_capture::LogCapture;
    use postretro_visibility::VisibleCells;

    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use crate::lightmap_streaming::test_fixtures::*;

    /// Fixed per-section read totals.
    struct FixedReads(BTreeMap<u32, u64>);

    impl SectionReadBytes for FixedReads {
        fn section_bytes(&self, section_id: u32) -> u64 {
            self.0.get(&section_id).copied().unwrap_or(0)
        }
    }

    fn lines(capture: &LogCapture) -> usize {
        capture
            .records()
            .iter()
            .filter(|record| record.message.starts_with("[Lightmap streaming] last"))
            .count()
    }

    // AC 22: bytes read come from ids 22 and 42 only; another section's reads
    // (SH's id 50 here) never reach the lightmap figures.
    #[test]
    fn bytes_read_are_attributed_to_ids_22_and_42() {
        let rig = Rig::corridor(None);
        let reads = FixedReads(BTreeMap::from([
            (SectionId::Lightmap as u32, 1_000),
            (SectionId::ShadowmaskAtlas as u32, 2_000),
            (50, 999_999),
        ]));
        let mut live = LightmapStreamingLiveDiagnostics::default();
        assemble_live_diagnostics(
            &mut live,
            &rig.controller,
            &LightmapRouteLedger::default(),
            None,
            Some(&reads),
        );
        assert_eq!(live.lightmap_bytes_read, 1_000);
        assert_eq!(live.shadowmask_bytes_read, 2_000);

        // A frame without a read source keeps the last values.
        assemble_live_diagnostics(
            &mut live,
            &rig.controller,
            &LightmapRouteLedger::default(),
            None,
            None,
        );
        assert_eq!(live.lightmap_bytes_read, 1_000);
    }

    // AC 22: visible misses in two overlapping buckets. Camera 0 draws block
    // 0 (mandatory, not yet resident), block 5 (outside the baked set, not
    // resident) and, after installs, block 1 (mandatory, resident).
    #[test]
    fn visible_misses_count_every_drawn_non_resident_block_and_every_block_outside_the_set() {
        let mut rig = Rig::corridor(None);
        let batch = rig.portal(0, &[0, 5]).unwrap();
        rig.controller.count_visible_misses();
        let counters = rig.controller.counters();
        assert_eq!(
            (
                counters.last_frame_drawn_outside_baked_set,
                counters.last_frame_drawn_not_resident
            ),
            (1, 2),
            "block 5 is outside the set and absent, so in both; block 0 is absent"
        );

        rig.install_all(&batch, headroom(8));
        rig.settle(0, &[1, 5], headroom(8));
        rig.controller.count_visible_misses();
        let counters = rig.controller.counters();
        assert_eq!(
            (
                counters.last_frame_drawn_outside_baked_set,
                counters.last_frame_drawn_not_resident
            ),
            (1, 0),
            "resident block 1 is no miss; resident block 5 is still outside the set"
        );

        // Counted once per frame: a second call without a new walk adds none.
        let before = rig.controller.counters();
        rig.controller.count_visible_misses();
        let after = rig.controller.counters();
        assert_eq!(
            after.drawn_outside_baked_set,
            before.drawn_outside_baked_set
        );
        assert_eq!(after.drawn_not_resident, before.drawn_not_resident);
        assert_eq!(after.last_frame_drawn_outside_baked_set, 0);

        let mut live = LightmapStreamingLiveDiagnostics::default();
        assemble_live_diagnostics(
            &mut live,
            &rig.controller,
            &LightmapRouteLedger::default(),
            None,
            None,
        );
        assert_eq!(live.drawn_outside_baked_set, after.drawn_outside_baked_set);
        assert_eq!(live.drawn_not_resident, after.drawn_not_resident);
    }

    // AC 22: resident and mandatory bytes follow installs, evictions and the
    // camera cell, split by section.
    #[test]
    fn resident_and_mandatory_bytes_track_installs_and_the_camera_cell() {
        let mut rig = Rig::corridor(None);
        rig.settle(0, &[], headroom(0));
        // Camera 0 at L = 16 m: blocks 0, 1 (0 m) and 2 (8 m) are mandatory;
        // each 64-byte half.
        let bytes = rig.controller.residency_bytes();
        assert_eq!(bytes.mandatory_blocks, 3);
        assert_eq!(bytes.mandatory_lightmap_bytes, 3 * 64);
        assert_eq!(bytes.mandatory_shadowmask_bytes, 3 * 64);
        assert_eq!(bytes.resident_blocks, 3, "no band room");
        assert_eq!(bytes.resident_lightmap_bytes, 3 * 64);

        // Camera 6: blocks 6 and 5 mandatory; 0..=2 leave demand and the
        // renderer frees them.
        rig.portal(6, &[]).unwrap();
        assert_eq!(rig.controller.residency_bytes().mandatory_blocks, 2);
        rig.controller
            .apply_outcome(postretro_level_loader::LightmapDrainOutcome {
                evicted: vec![0, 1, 2],
                pool: headroom(0),
                ..Default::default()
            })
            .unwrap();
        let bytes = rig.controller.residency_bytes();
        assert_eq!(bytes.resident_blocks, 0);
        assert_eq!(bytes.resident_shadowmask_bytes, 0);
    }

    // AC 22 hot path: steady frames assemble the diagnostics and offer them to
    // the log without a heap allocation.
    #[test]
    fn steady_frames_assemble_diagnostics_without_allocating() {
        let mut rig = Rig::corridor(None);
        rig.settle(2, &[2, 3], headroom(8));
        let visible = VisibleCells::Culled(vec![2, 3]);
        let ledger = LightmapRouteLedger::default();
        let reads = FixedReads(BTreeMap::from([(SectionId::Lightmap as u32, 7)]));
        let renderer = LightmapStreamCounters::default();
        let mut live = LightmapStreamingLiveDiagnostics::default();
        let mut window = LightmapStreamingLogWindow::default();
        // Prime the window so its first due line does not fall in the probe.
        window.observe(0.0, &live);
        let capacities = rig.controller.buffer_capacities();

        let probe = AllocSnapshot::arm();
        for frame in 0..64 {
            let batch = rig.frame(2, PORTAL, &visible).unwrap();
            rig.install_all(&batch, headroom(8));
            rig.controller.count_visible_misses();
            assemble_live_diagnostics(
                &mut live,
                &rig.controller,
                &ledger,
                Some(&renderer),
                Some(&reads),
            );
            window.observe(f64::from(frame) / 60.0, &live);
        }
        assert_eq!(
            probe.allocs_since(),
            0,
            "no heap allocation in steady frames"
        );
        assert_eq!(rig.controller.buffer_capacities(), capacities);
        assert_eq!(live.lightmap_bytes_read, 7);
    }

    // The periodic line carries the controller's miss causes, in
    // block-frames. Camera 6 draws block 6 while its read is in flight.
    #[test]
    fn log_line_reports_not_resident_misses_by_cause_in_block_frames() {
        let capture = LogCapture::start();
        let mut rig = Rig::corridor(None);
        let mut live = LightmapStreamingLiveDiagnostics::default();
        let mut window = LightmapStreamingLogWindow::default();
        let ledger = LightmapRouteLedger::default();
        window.observe(0.0, &live);
        rig.portal(6, &[6]);
        rig.controller.count_visible_misses();
        assemble_live_diagnostics(&mut live, &rig.controller, &ledger, None, None);
        assert_eq!(live.miss_buckets, rig.controller.counters().miss_buckets);
        window.observe(DIAGNOSTICS_LOG_INTERVAL_SECONDS, &live);
        capture.assert_logged_once(
            log::Level::Info,
            "visible misses in block-frames (may overlap) 0 outside baked set, 1 not resident \
             (reach 0, in flight 1, permit 0, in-hand cap 0, budget 0, renderer drain 0, \
             deferred 0, refused 0, failed 0)",
        );
    }

    #[test]
    fn log_line_fires_once_per_interval_while_activity_occurs_and_never_when_idle() {
        let capture = LogCapture::start();
        let mut window = LightmapStreamingLogWindow::default();
        let mut live = LightmapStreamingLiveDiagnostics::default();

        // 20 s at 60 Hz with nothing happening: silent.
        for frame in 0..=1200u64 {
            window.observe(frame as f64 / 60.0, &live);
        }
        assert_eq!(lines(&capture), 0, "an idle level never logs");

        // 20 s with an install every frame: the first changed frame logs at
        // once (the interval since the window opened has passed), then one
        // line per 5 s window, the last at the stretch's final frame.
        for frame in 1201..=2401u64 {
            live.installs += 1;
            window.observe(frame as f64 / 60.0, &live);
        }
        assert_eq!(lines(&capture), 5);

        // Frozen again, but the last-frame miss gauge flickers: still silent.
        for frame in 2402..=3600u64 {
            live.last_frame_drawn_not_resident = (frame % 2) as u32;
            window.observe(frame as f64 / 60.0, &live);
        }
        assert_eq!(lines(&capture), 5);

        // A gauge change alone (a new camera cell's mandatory set) logs.
        live.mandatory_blocks = 12;
        window.observe(3601.0 / 60.0, &live);
        assert_eq!(lines(&capture), 6);
        capture.assert_logged(log::Level::Info, "mandatory 12 blocks");
    }

    #[test]
    fn log_line_reports_window_deltas_and_current_gauges() {
        let before = LightmapStreamingLiveDiagnostics {
            installs: 10,
            lightmap_bytes_read: 1024 * 1024,
            drawn_not_resident: 4,
            miss_buckets: postretro_renderer::LightmapMissBuckets {
                read_in_flight: 3,
                waiting_on_in_hand_cap: 1,
                ..Default::default()
            },
            repacks: PerPool {
                lightmap: 2,
                mask: 1,
            },
            compactions: PerPool {
                lightmap: 1,
                mask: 0,
            },
            ..LightmapStreamingLiveDiagnostics::default()
        };
        let now = LightmapStreamingLiveDiagnostics {
            installs: 13,
            reads_requested: 3,
            physical_reads: 2,
            lightmap_bytes_read: 3 * 1024 * 1024,
            shadowmask_bytes_read: 4 * 1024 * 1024,
            drawn_outside_baked_set: 1,
            drawn_not_resident: 6,
            miss_buckets: postretro_renderer::LightmapMissBuckets {
                read_in_flight: 4,
                waiting_on_in_hand_cap: 2,
                ..Default::default()
            },
            lead_metres: 16.0,
            max_lead_metres: 32.0,
            block_count: 198,
            resident_blocks: 41,
            resident_lightmap_bytes: 1024 * 1024,
            resident_shadowmask_bytes: 2 * 1024 * 1024,
            mandatory_blocks: 38,
            pool_layers: PerPool {
                lightmap: 7,
                mask: 3,
            },
            peak_pool_layers: PerPool {
                lightmap: 9,
                mask: 4,
            },
            pool_bytes: PerPool {
                lightmap: 98 * 1024 * 1024,
                mask: 12 * 1024 * 1024,
            },
            band_headroom_texels: PerPool {
                lightmap: 64,
                mask: 32,
            },
            repacks: PerPool {
                lightmap: 5,
                mask: 3,
            },
            compactions: PerPool {
                lightmap: 2,
                mask: 1,
            },
            compaction_rollbacks: PerPool {
                lightmap: 1,
                mask: 0,
            },
            last_drain_install_micros: 420,
            max_drain_install_micros: 3_100,
            ..LightmapStreamingLiveDiagnostics::default()
        };
        let line = format_line(5.0, &before, &now);
        assert!(
            line.starts_with(
                "[Lightmap streaming] last 5.0 s: 3 pairs requested (2 reads, id 22 2.0 MiB, \
                 id 42 4.0 MiB), 3 installs"
            ),
            "{line}"
        );
        assert!(
            line.contains(
                "visible misses in block-frames (may overlap) 1 outside baked set, 2 not \
                 resident (reach 0, in flight 1, permit 0, in-hand cap 1, budget 0, renderer \
                 drain 0, deferred 0, refused 0, failed 0)"
            ),
            "{line}"
        );
        assert!(
            line.contains(
                "3 repacks (2 mask), 1 compactions (1 mask), 1 compaction rollbacks (0 mask)"
            ),
            "{line}"
        );
        assert!(line.contains("lead 16.0 of 32.0 m"), "{line}");
        assert!(
            line.contains("resident 41 of 198 blocks 3.0 MiB (id 22 1.0 MiB, id 42 2.0 MiB)"),
            "{line}"
        );
        assert!(line.contains("pool 7 layers (peak 9) 98.0 MiB"), "{line}");
        assert!(line.contains("headroom 64 texels"), "{line}");
        assert!(
            line.contains(
                "mask pool 3 layers (peak 4) 12.0 MiB + 0 B retiring, headroom 32 texels"
            ),
            "{line}"
        );
        assert!(
            line.contains("install last 0.42 ms / max 3.10 ms"),
            "{line}"
        );
        assert!(!line.contains('\n'));
    }

    // The band headroom gauge comes from the controller's latest pool report.
    #[test]
    fn band_headroom_comes_from_the_latest_outcome() {
        let mut rig = Rig::corridor(None);
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(
            &batch,
            LightmapPoolReport {
                lightmap: PoolReport {
                    band_headroom_texels: 4096,
                    ..PoolReport::default()
                },
                mask: PoolReport {
                    band_headroom_texels: 1024,
                    ..PoolReport::default()
                },
            },
        );
        let mut live = LightmapStreamingLiveDiagnostics::default();
        assemble_live_diagnostics(
            &mut live,
            &rig.controller,
            &LightmapRouteLedger::default(),
            None,
            None,
        );
        assert_eq!(live.band_headroom_texels.lightmap, 4096);
        assert_eq!(live.band_headroom_texels.mask, 1024);
        assert_eq!(live.block_count, 7);
        assert_eq!(live.lead_metres, 16.0);
    }

    fn lightmap_windows(hub: &postretro_telemetry::Hub) -> Vec<serde_json::Value> {
        hub.lines()
            .iter()
            .map(|line| postretro_telemetry::Envelope::parse_line(line).unwrap())
            .filter(|event| event.kind == postretro_telemetry::Kind::LightmapStreamingWindow)
            .map(|event| event.payload)
            .collect()
    }

    fn scoped_hub() -> (
        std::sync::Arc<postretro_telemetry::Hub>,
        postretro_telemetry::scoped::ScopedHub,
    ) {
        let hub = std::sync::Arc::new(postretro_telemetry::Hub::start(
            postretro_telemetry::HubConfig {
                memory: true,
                ring: Some(postretro_telemetry::RingCaps::DEFAULT),
                ..postretro_telemetry::HubConfig::new(postretro_telemetry::Source::Engine, "t")
            },
        ));
        let scope = postretro_telemetry::scoped::install(std::sync::Arc::clone(&hub));
        (hub, scope)
    }

    // Telemetry: the window event carries the line's not-resident cause
    // split, field for field, in block-frames.
    #[test]
    fn lightmap_window_event_and_line_agree_on_the_miss_causes() {
        let (hub, _scope) = scoped_hub();
        let capture = LogCapture::start();
        let mut window = LightmapStreamingLogWindow::default();
        window.observe(0.0, &LightmapStreamingLiveDiagnostics::default());
        let live = LightmapStreamingLiveDiagnostics {
            installs: 1,
            drawn_not_resident: 6,
            miss_buckets: postretro_renderer::LightmapMissBuckets {
                read_in_flight: 2,
                waiting_on_in_hand_cap: 3,
                held_by_drain_budget: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        window.observe(DIAGNOSTICS_LOG_INTERVAL_SECONDS, &live);
        capture.assert_logged_once(
            log::Level::Info,
            "6 not resident (reach 0, in flight 2, permit 0, in-hand cap 3, budget 1, \
             renderer drain 0, deferred 0, refused 0, failed 0)",
        );
        let events = lightmap_windows(&hub);
        assert_eq!(events.len(), 1);
        let delta = &events[0]["delta"];
        assert_eq!(delta["missed_not_resident"], 6);
        assert_eq!(delta["miss_reach"], 0);
        assert_eq!(delta["miss_in_flight"], 2);
        assert_eq!(delta["miss_permit"], 0);
        assert_eq!(delta["miss_in_hand_cap"], 3);
        assert_eq!(delta["miss_budget"], 1);
        assert_eq!(delta["miss_renderer_drain"], 0);
        assert_eq!(delta["miss_deferred"], 0);
        assert_eq!(delta["miss_refused"], 0);
        assert_eq!(delta["miss_failed"], 0);
    }

    // Telemetry: a lightmap window that has counted something emits one
    // partial when its session ends (the window drops with it), and no line.
    #[test]
    fn a_counted_lightmap_window_emits_one_partial_when_its_session_ends() {
        let (hub, _scope) = scoped_hub();
        let capture = LogCapture::start();
        let mut window = LightmapStreamingLogWindow::default();
        window.observe(1.0, &LightmapStreamingLiveDiagnostics::default());
        window.observe(
            2.5,
            &LightmapStreamingLiveDiagnostics {
                installs: 3,
                drawn_not_resident: 2,
                ..Default::default()
            },
        );
        drop(window);
        assert_eq!(lines(&capture), 0);
        let events = lightmap_windows(&hub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["partial"], true);
        assert_eq!(events[0]["elapsed_s"], 1.5);
        assert_eq!(events[0]["delta"]["installs"], 3);
        assert_eq!(events[0]["delta"]["missed_not_resident"], 2);
    }

    #[test]
    fn an_idle_lightmap_window_emits_nothing_when_its_session_ends() {
        let (hub, _scope) = scoped_hub();
        let mut window = LightmapStreamingLogWindow::default();
        // A last-frame gauge alone is not a count.
        window.observe(
            1.0,
            &LightmapStreamingLiveDiagnostics {
                last_frame_drawn_not_resident: 4,
                ..Default::default()
            },
        );
        drop(window);
        assert!(lightmap_windows(&hub).is_empty());
    }

    // Telemetry: with the ring and memory sink live, steady frames that close
    // no window allocate nothing.
    #[test]
    fn with_telemetry_on_steady_window_frames_allocate_nothing() {
        let (_hub, _scope) = scoped_hub();
        let mut window = LightmapStreamingLogWindow::default();
        let live = LightmapStreamingLiveDiagnostics {
            installs: 1,
            ..Default::default()
        };
        window.observe(0.0, &live);
        let probe = AllocSnapshot::arm();
        for frame in 0..64 {
            window.observe(f64::from(frame) / 60.0, &live);
        }
        assert_eq!(probe.allocs_since(), 0);
    }
}
