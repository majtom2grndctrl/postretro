// GPU timestamp query helper for per-pass frame timing.
// Gated on `POSTRETRO_GPU_TIMING=1` and the timestamp features used below.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How many completed readback samples to accumulate before logging an average.
const AVG_WINDOW_SAMPLES: u32 = 120;

/// Longest pass span decoded as a real sample: one second of GPU time. A longer
/// span means an endpoint is garbage, e.g. a counter with fewer than 64 valid
/// bits wrapped between the prefill's end and start writes of a skipped pass.
const MAX_PASS_NS: f64 = 1.0e9;

/// One completed averaging window, retained for the debug UI and consumed by
/// capture measurement.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct FrameTimingSnapshot {
    /// Readbacks in the window. Every pass's `sampled_readbacks` is at most this.
    pub readbacks: u32,
    pub passes: Vec<PassTiming>,
}

/// One pass's result over a completed window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassTiming {
    pub label: &'static str,
    /// Mean over only the readbacks the pass was sampled in; `None` when it
    /// was sampled in none, so an unsampled pass never reads as zero cost. A
    /// conditional pass's mean is per run, not per readback: summing passes
    /// over-counts.
    pub average_ms: Option<f32>,
    /// Readbacks in which the pass ran and decoded a sample.
    pub sampled_readbacks: u32,
    /// Readbacks in which the pass ran but decoded malformed.
    pub malformed_readbacks: u32,
}

const QUERY_SIZE: wgpu::BufferAddress = 8;

const QUERIES_PER_PASS: u32 = 2;

/// Resolve-buffer offset for the frame's single resolve. wgpu requires it to be
/// a multiple of `QUERY_RESOLVE_BUFFER_ALIGNMENT`.
const RESOLVE_OFFSET: wgpu::BufferAddress = 0;

/// Order in which `begin_frame` writes every query slot: all end slots, then
/// all start slots.
///
/// Vulkan resolves with a wait on each query's availability, and a query that
/// was never written never becomes available — the copy hangs until the driver
/// times out and loses the device. Writing every slot at frame start makes the
/// whole range available. Writing ends first means a pair whose pass never
/// overwrote it reads `end <= start` (timestamps on one queue do not
/// decrease), which `decode_pair` reports as absent rather than a stale sample.
///
/// Each slot is its own encoder-level write. On Metal, wgpu defers such a write
/// until the next encoder opens, but wgpu-core resets each slot before writing
/// it, and that reset opens a blit encoder that flushes the deferred write.
/// Every pass also opens a fresh command buffer, and closing the previous one
/// flushes too. So no pass descriptor inherits prefill samples. Do not swap in
/// empty compute passes covering two slots each: wgpu-core resets every index
/// between a compute pass's two write indices, which unwrites any slot in that
/// span that an earlier prefill pass wrote.
fn prefill_order(num_queries: u32) -> impl Iterator<Item = u32> {
    let ends = (1..num_queries).step_by(QUERIES_PER_PASS as usize);
    let starts = (0..num_queries).step_by(QUERIES_PER_PASS as usize);
    ends.chain(starts)
}

/// Query range resolved this frame, or `None` when no pass requested
/// timestamps (nothing to measure, so nothing is resolved or sampled). Only
/// valid after `begin_frame` prefilled `0..num_queries` in the same encoder.
fn resolve_range(written_mask: u64, num_queries: u32) -> Option<Range<u32>> {
    (written_mask != 0).then_some(0..num_queries)
}

/// One pass's outcome decoded from a frame's resolved ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairSample {
    /// The pass did not run this frame: it never requested timestamps, or it
    /// requested them but skipped encoding, leaving the prefill in place.
    Absent,
    /// The pass ran; GPU ticks from its start to its end timestamp.
    Ticks(u64),
    /// The pass ran, but an endpoint read back as zero or out of range, or
    /// the span exceeds `max_ticks`.
    Malformed,
}

/// `MAX_PASS_NS` in GPU ticks. An unusable period disables the ceiling.
fn max_pass_ticks(ns_per_tick: f32) -> u64 {
    if ns_per_tick.is_finite() && ns_per_tick > 0.0 {
        // Float-to-int `as` saturates, so a tiny period cannot overflow. A
        // period over one second would truncate to 0 and reject every sample.
        ((MAX_PASS_NS / f64::from(ns_per_tick)) as u64).max(1)
    } else {
        u64::MAX
    }
}

fn decode_pair(written_mask: u64, pair_idx: usize, ticks: &[u64], max_ticks: u64) -> PairSample {
    if pair_idx >= 64 || (written_mask & (1u64 << pair_idx)) == 0 {
        return PairSample::Absent;
    }
    let base = pair_idx * (QUERIES_PER_PASS as usize);
    let (Some(&start), Some(&end)) = (ticks.get(base), ticks.get(base + 1)) else {
        return PairSample::Malformed;
    };
    if start == 0 || end == 0 {
        return PairSample::Malformed;
    }
    if end <= start {
        // Prefill signature: marked when the descriptor was built, but the
        // pass's dispatch was skipped (e.g. no visible animated-lightmap tiles).
        return PairSample::Absent;
    }
    let delta = end - start;
    if delta > max_ticks {
        return PairSample::Malformed;
    }
    PairSample::Ticks(delta)
}

/// Per-pass sums over one averaging window of readbacks.
struct TimingWindow {
    sum_ns: Vec<f64>,
    /// Readbacks in which the pass ran. Each pass averages over its own
    /// count, so a readback it was absent from weighs nothing, not zero.
    present: Vec<u32>,
    /// Readbacks in which the pass was marked but decoded malformed.
    /// Surfaced in the averaged log line so mis-wired pair indices or
    /// driver anomalies don't silently report 0.00ms.
    malformed: Vec<u32>,
    readbacks: u32,
}

impl TimingWindow {
    fn new(pairs: usize) -> Self {
        Self {
            sum_ns: vec![0.0; pairs],
            present: vec![0; pairs],
            malformed: vec![0; pairs],
            readbacks: 0,
        }
    }

    fn record(&mut self, written: u64, ticks: &[u64], ns_per_tick: f64, max_ticks: u64) {
        for pair_idx in 0..self.sum_ns.len() {
            match decode_pair(written, pair_idx, ticks, max_ticks) {
                PairSample::Absent => {}
                PairSample::Malformed => self.malformed[pair_idx] += 1,
                PairSample::Ticks(delta) => {
                    self.sum_ns[pair_idx] += delta as f64 * ns_per_tick;
                    self.present[pair_idx] += 1;
                }
            }
        }
        self.readbacks += 1;
    }

    /// Mean over the readbacks the pass ran in; `None` when it ran in none.
    fn average_ms(&self, pair_idx: usize) -> Option<f64> {
        match self.present[pair_idx] {
            0 => None,
            n => Some(self.sum_ns[pair_idx] / f64::from(n) / 1.0e6),
        }
    }

    fn snapshot(&self, labels: &[&'static str]) -> FrameTimingSnapshot {
        FrameTimingSnapshot {
            readbacks: self.readbacks,
            passes: labels
                .iter()
                .enumerate()
                .map(|(i, &label)| PassTiming {
                    label,
                    average_ms: self.average_ms(i).map(|ms| ms as f32),
                    sampled_readbacks: self.present[i],
                    malformed_readbacks: self.malformed[i],
                })
                .collect(),
        }
    }

    /// Per-pass averages; a pass absent from some readbacks shows
    /// `(present/readbacks)`, and one absent from all shows `n/a`.
    fn log_line(&self, labels: &[&'static str]) -> String {
        let parts: Vec<String> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let avg = match self.average_ms(i) {
                    Some(ms) => format!("{ms:.2}ms"),
                    None => "n/a".to_string(),
                };
                let present = self.present[i];
                if present < self.readbacks {
                    format!("{label} {avg} ({present}/{})", self.readbacks)
                } else {
                    format!("{label} {avg}")
                }
            })
            .collect();
        let malformed_parts: Vec<String> = labels
            .iter()
            .zip(&self.malformed)
            .filter(|&(_, &n)| n > 0)
            .map(|(label, n)| format!("{label} {n}"))
            .collect();
        let mut line = format!(
            "[gpu-timing] {} (avg over {} readbacks",
            parts.join(" | "),
            self.readbacks
        );
        if !malformed_parts.is_empty() {
            line.push_str("; malformed: ");
            line.push_str(&malformed_parts.join(", "));
        }
        line.push(')');
        line
    }

    fn reset(&mut self) {
        self.sum_ns.fill(0.0);
        self.present.fill(0);
        self.malformed.fill(0);
        self.readbacks = 0;
    }
}

/// Owns the query set plus resolve / readback buffers and the averaging
/// accumulator. One `FrameTiming` covers a fixed list of pass labels
/// chosen at construction — pair index `i` maps to query indices
/// `[2*i, 2*i + 1]`.
pub struct FrameTiming {
    query_set: wgpu::QuerySet,
    num_queries: u32,
    resolve_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
    /// While pending, `encode_resolve` must not overwrite the readback
    /// buffer (it's still mapped on the host).
    map_pending: Arc<AtomicBool>,
    /// A timing resolve was copied into the readback buffer and awaits its
    /// `map_async` kickoff in `post_submit`. Keeping this distinct from
    /// `map_pending` ensures every map observes a fresh copy rather than
    /// repeatedly mapping an already-consumed sample.
    copied_pending: bool,
    /// Set by the map callback when the readback buffer is ready to be read.
    /// The callback deliberately does not capture the buffer: the renderer can
    /// be torn down before a pending callback is dispatched.
    map_ready: Arc<AtomicBool>,
    ns_per_tick: f32,
    /// `MAX_PASS_NS` at this queue's timestamp period.
    max_pass_ticks: u64,
    pass_labels: Vec<&'static str>,
    window: TimingWindow,
    /// Bitmask of pair indices whose `render_pass_writes` /
    /// `compute_pass_writes` was called this frame. Cleared by `begin_frame`
    /// and swapped to zero at `encode_resolve` time. A marked pair may still
    /// have skipped encoding its pass; `decode_pair` detects that from the
    /// prefill ordering.
    ///
    /// `AtomicU64` so accessor methods stay `&self`; single-threaded in
    /// practice but the atomic is free here.
    pairs_written: AtomicU64,
    /// Snapshot of `pairs_written` taken when `encode_resolve` actually
    /// performed the `copy_buffer_to_buffer`. Paired with the tick
    /// snapshot that arrives via `map_async`, so `accumulate` decodes each
    /// sample against the frame it came from.
    pairs_written_in_flight: AtomicU64,
    /// `begin_frame` prefilled the query set in the encoder now being
    /// recorded. `encode_resolve` refuses to resolve without it, because an
    /// unprefilled slot may never have been written.
    frame_prefilled: bool,
    /// Most recent averaged-window snapshot. The debug UI reads this to show
    /// per-pass timing; overwritten each averaging boundary.
    last_window: Option<FrameTimingSnapshot>,
    /// Newly completed window not yet consumed by an offscreen measurement
    /// report. Kept distinct from `last_window` so diagnostics retain their
    /// most recent value while capture observes every window once.
    completed_window: Option<FrameTimingSnapshot>,
}

impl FrameTiming {
    /// Create a new timing helper. `pass_labels.len()` pairs of query
    /// slots will be allocated (plus padding to 16 slots minimum).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, pass_labels: Vec<&'static str>) -> Self {
        let pair_count = pass_labels.len() as u32;
        // Pad to 16 slots so callers can add future passes without
        // resizing the query set.
        let num_queries = (pair_count * QUERIES_PER_PASS).max(16);

        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Frame Timing Query Set"),
            ty: wgpu::QueryType::Timestamp,
            count: num_queries,
        });

        let buffer_size = num_queries as wgpu::BufferAddress * QUERY_SIZE;

        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Frame Timing Resolve Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::QUERY_RESOLVE,
            mapped_at_creation: false,
        });

        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Frame Timing Readback Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let ns_per_tick = queue.get_timestamp_period();
        let pair_usize = pass_labels.len();

        // u64 bitmask caps at 64 pairs; assert loudly so a future
        // expansion past 64 fails at construction rather than silently
        // losing high bits.
        assert!(
            pair_usize <= 64,
            "FrameTiming: pair count {pair_usize} exceeds the 64-bit \
             pairs_written bitmask capacity",
        );

        Self {
            query_set,
            num_queries,
            resolve_buffer,
            readback_buffer,
            map_pending: Arc::new(AtomicBool::new(false)),
            copied_pending: false,
            map_ready: Arc::new(AtomicBool::new(false)),
            ns_per_tick,
            max_pass_ticks: max_pass_ticks(ns_per_tick),
            pass_labels,
            window: TimingWindow::new(pair_usize),
            pairs_written: AtomicU64::new(0),
            pairs_written_in_flight: AtomicU64::new(0),
            frame_prefilled: false,
            last_window: None,
            completed_window: None,
        }
    }

    /// Write every query slot at the start of a frame's scene recording,
    /// before any pass requests timestamps. See `prefill_order` for why.
    pub fn begin_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        // Drop marks from a recording that never resolved (PNG capture).
        self.pairs_written.store(0, Ordering::Relaxed);
        for index in prefill_order(self.num_queries) {
            encoder.write_timestamp(&self.query_set, index);
        }
        self.frame_prefilled = true;
    }

    /// Most recent completed 120-readback window. `None` until the first
    /// averaging boundary has elapsed.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn last_window(&self) -> Option<&FrameTimingSnapshot> {
        self.last_window.as_ref()
    }

    /// Consume one newly completed 120-readback window, if it has not already
    /// been observed by a measurement report.
    pub fn take_completed_window(&mut self) -> Option<FrameTimingSnapshot> {
        self.completed_window.take()
    }

    /// Successful readbacks accumulated since the last completed window.
    /// Capture uses this instead of submitted frame count because a failed or
    /// delayed map does not contribute a timing sample.
    pub fn partial_window_samples(&self) -> u32 {
        self.window.readbacks
    }

    /// Discard timing state at the capture warmup/sample boundary. The caller
    /// must complete any in-flight readback before reset.
    pub fn reset_window_state(&mut self) {
        debug_assert!(
            !self.map_pending.load(Ordering::Acquire) && !self.copied_pending,
            "FrameTiming reset requires completed timing readback"
        );
        self.window.reset();
        self.pairs_written.store(0, Ordering::Relaxed);
        self.pairs_written_in_flight.store(0, Ordering::Relaxed);
        self.last_window = None;
        self.completed_window = None;
    }

    /// Render-pass timestamp writes for pair `pair_idx`. The returned
    /// struct borrows the query set, so the caller must keep `self`
    /// alive for the duration of the pass descriptor.
    pub fn render_pass_writes(&self, pair_idx: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        self.mark_pair_written(pair_idx);
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        }
    }

    /// Compute-pass timestamp writes for pair `pair_idx`.
    pub fn compute_pass_writes(&self, pair_idx: usize) -> wgpu::ComputePassTimestampWrites<'_> {
        self.mark_pair_written(pair_idx);
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        }
    }

    /// Encoder-level timestamp bracket for commands that are not inside a
    /// render or compute pass, such as texture copies.
    pub fn write_encoder_start(&self, encoder: &mut wgpu::CommandEncoder, pair_idx: usize) {
        self.mark_pair_written(pair_idx);
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        encoder.write_timestamp(&self.query_set, base);
    }

    pub fn write_encoder_end(&self, encoder: &mut wgpu::CommandEncoder, pair_idx: usize) {
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        encoder.write_timestamp(&self.query_set, base + 1);
    }

    fn mark_pair_written(&self, pair_idx: usize) {
        if pair_idx < 64 {
            self.pairs_written
                .fetch_or(1u64 << pair_idx, Ordering::Relaxed);
        }
    }

    /// Resolve the query set and copy into the readback buffer. Skips both
    /// when a previous copy/map cycle is still in flight — missing a frame's
    /// data is preferable to stalling — and when no pass requested timestamps.
    /// Always drains `pairs_written` and, when the copy runs, stores the
    /// drained bitmask into `pairs_written_in_flight` so it travels with the
    /// tick snapshot arriving via `map_async`.
    pub fn encode_resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let written_this_frame = self.pairs_written.swap(0, Ordering::Relaxed);
        let prefilled = std::mem::replace(&mut self.frame_prefilled, false);
        if !prefilled {
            debug_assert!(
                written_this_frame == 0,
                "FrameTiming: passes wrote timestamps without begin_frame"
            );
            return;
        }
        let Some(range) = resolve_range(written_this_frame, self.num_queries) else {
            return;
        };
        if self.copied_pending || self.map_pending.load(Ordering::Acquire) {
            return;
        }
        let bytes = (range.end - range.start) as wgpu::BufferAddress * QUERY_SIZE;
        encoder.resolve_query_set(&self.query_set, range, &self.resolve_buffer, RESOLVE_OFFSET);
        encoder.copy_buffer_to_buffer(
            &self.resolve_buffer,
            RESOLVE_OFFSET,
            &self.readback_buffer,
            0,
            bytes,
        );
        self.pairs_written_in_flight
            .store(written_this_frame, Ordering::Relaxed);
        self.copied_pending = true;
    }

    /// Drive the async map state machine. Called once per frame AFTER
    /// `queue.submit`. Non-blocking poll drives any ready map callbacks
    /// on native; consumes a completed map if waiting, then starts a map only
    /// for a newly submitted copy.
    pub fn post_submit(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);

        if self.map_ready.swap(false, Ordering::AcqRel) {
            let buffer_size = self.num_queries as wgpu::BufferAddress * QUERY_SIZE;
            let view = self
                .readback_buffer
                .slice(0..buffer_size)
                .get_mapped_range();
            let mut ticks = Vec::with_capacity(view.len() / 8);
            for chunk in view.chunks_exact(8) {
                ticks.push(u64::from_le_bytes(chunk.try_into().unwrap()));
            }
            drop(view);
            self.accumulate(&ticks);
            self.readback_buffer.unmap();
            self.map_pending.store(false, Ordering::Release);
        }

        if self.copied_pending && !self.map_pending.load(Ordering::Acquire) {
            self.copied_pending = false;
            self.map_pending.store(true, Ordering::Release);
            let ready = Arc::clone(&self.map_ready);
            let pending = Arc::clone(&self.map_pending);
            self.readback_buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |res| match res {
                    Ok(()) => {
                        // Regression: accessing the buffer from this callback
                        // panicked if the renderer/device was torn down before
                        // wgpu dispatched it. Only the live FrameTiming owner
                        // reads and unmaps the mapped range.
                        ready.store(true, Ordering::Release);
                    }
                    Err(err) => {
                        log::warn!("[gpu-timing] readback map failed: {err:?}");
                        pending.store(false, Ordering::Release);
                    }
                });
        }
    }

    fn accumulate(&mut self, ticks: &[u64]) {
        let written = self.pairs_written_in_flight.load(Ordering::Relaxed);
        self.window.record(
            written,
            ticks,
            f64::from(self.ns_per_tick),
            self.max_pass_ticks,
        );
        if self.window.readbacks < AVG_WINDOW_SAMPLES {
            return;
        }
        let snapshot = self.window.snapshot(&self.pass_labels);
        self.last_window = Some(snapshot.clone());
        self.completed_window = Some(snapshot);
        log::info!("{}", self.window.log_line(&self.pass_labels));
        self.window.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The renderer's live query count: 14 pairs, above the 16-slot floor.
    const LIVE_QUERIES: u32 = 28;

    /// `max_pass_ticks` at a 1 ns timestamp period.
    const ONE_SECOND_TICKS: u64 = 1_000_000_000;

    fn mask(pairs: &[usize]) -> u64 {
        pairs.iter().fold(0, |m, &p| m | (1u64 << p))
    }

    /// Model one frame's query-set contents as the GPU would leave them:
    /// `begin_frame` stamps every slot in `prefill_order` from a rising clock,
    /// then each pass in `ran` overwrites its pair later in the frame.
    fn simulate_frame(num_queries: u32, ran: &[(usize, u64)]) -> Vec<u64> {
        let mut ticks = vec![0u64; num_queries as usize];
        let mut clock = 1_000u64;
        for index in prefill_order(num_queries) {
            ticks[index as usize] = clock;
            clock += 1;
        }
        for &(pair, duration) in ran {
            clock += 50;
            ticks[pair * 2] = clock;
            clock += duration;
            ticks[pair * 2 + 1] = clock;
        }
        ticks
    }

    // Regression: resolving never-written timestamp queries (skipped passes)
    // device-lost NVIDIA/Vulkan on the first level frame under POSTRETRO_GPU_TIMING=1.
    #[test]
    fn resolve_range_never_covers_a_slot_the_prefill_did_not_write() {
        for num_queries in [16, LIVE_QUERIES] {
            let prefilled: BTreeSet<u32> = prefill_order(num_queries).collect();
            // Some passes skipped, a single pass, and every pass.
            for written in [
                mask(&[0, 4, 13]),
                mask(&[7]),
                mask(&(0..14).collect::<Vec<_>>()),
            ] {
                let range = resolve_range(written, num_queries).expect("passes ran");
                assert!(
                    range.clone().all(|slot| prefilled.contains(&slot)),
                    "resolve {range:?} includes a slot the prefill never wrote"
                );
            }
        }
    }

    #[test]
    fn prefill_order_writes_every_query_slot_exactly_once() {
        for num_queries in [16, LIVE_QUERIES] {
            let order: Vec<u32> = prefill_order(num_queries).collect();
            let unique: BTreeSet<u32> = order.iter().copied().collect();
            assert_eq!(order.len(), num_queries as usize, "no slot written twice");
            assert_eq!(unique, (0..num_queries).collect::<BTreeSet<_>>());
        }
    }

    #[test]
    fn prefill_order_writes_all_end_slots_before_any_start_slot() {
        let order: Vec<u32> = prefill_order(LIVE_QUERIES).collect();
        let last_end = order.iter().rposition(|slot| slot % 2 == 1).unwrap();
        let first_start = order.iter().position(|slot| slot % 2 == 0).unwrap();
        assert!(last_end < first_start, "order was {order:?}");
    }

    #[test]
    fn resolve_range_is_one_full_resolve_when_every_pass_runs() {
        let all = mask(&(0..14).collect::<Vec<_>>());
        assert_eq!(resolve_range(all, LIVE_QUERIES), Some(0..LIVE_QUERIES));
    }

    #[test]
    fn resolve_range_encodes_nothing_when_no_pass_requests_timestamps() {
        assert_eq!(resolve_range(0, LIVE_QUERIES), None);
    }

    #[test]
    fn resolve_offset_respects_query_resolve_buffer_alignment() {
        assert_eq!(RESOLVE_OFFSET % wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT, 0);
    }

    #[test]
    fn decode_pair_maps_each_ran_pass_to_its_own_duration() {
        let ticks = simulate_frame(LIVE_QUERIES, &[(0, 300), (4, 7_000), (13, 45)]);
        let written = mask(&[0, 4, 13]);
        assert_eq!(
            decode_pair(written, 0, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(300)
        );
        assert_eq!(
            decode_pair(written, 4, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(7_000)
        );
        assert_eq!(
            decode_pair(written, 13, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(45)
        );
    }

    #[test]
    fn decode_pair_reports_pass_that_requested_no_timestamps_as_absent() {
        let ticks = simulate_frame(LIVE_QUERIES, &[(4, 7_000)]);
        let written = mask(&[4]);
        for pair in (0..14).filter(|&p| p != 4) {
            assert_eq!(
                decode_pair(written, pair, &ticks, ONE_SECOND_TICKS),
                PairSample::Absent,
                "pair {pair}"
            );
        }
    }

    #[test]
    fn decode_pair_reports_marked_but_unencoded_pass_as_absent_not_zero() {
        // Pair 1 (animated LM compose) was marked when its descriptor was
        // built, then skipped because no animated tiles were visible.
        let ticks = simulate_frame(LIVE_QUERIES, &[(4, 7_000)]);
        let written = mask(&[1, 4]);
        assert_eq!(
            decode_pair(written, 1, &ticks, ONE_SECOND_TICKS),
            PairSample::Absent
        );
        assert_eq!(
            decode_pair(written, 4, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(7_000)
        );
    }

    #[test]
    fn decode_pair_counts_zero_or_missing_endpoint_as_malformed() {
        let mut ticks = simulate_frame(LIVE_QUERIES, &[(2, 100), (3, 100)]);
        ticks[2 * 2] = 0;
        ticks[3 * 2 + 1] = 0;
        let written = mask(&[2, 3, 20]);
        assert_eq!(
            decode_pair(written, 2, &ticks, ONE_SECOND_TICKS),
            PairSample::Malformed
        );
        assert_eq!(
            decode_pair(written, 3, &ticks, ONE_SECOND_TICKS),
            PairSample::Malformed
        );
        assert_eq!(
            decode_pair(written, 20, &ticks, ONE_SECOND_TICKS),
            PairSample::Malformed
        );
    }

    #[test]
    fn max_pass_ticks_is_one_second_at_the_timestamp_period() {
        assert_eq!(max_pass_ticks(1.0), ONE_SECOND_TICKS);
        assert_eq!(max_pass_ticks(10.0), 100_000_000);
        assert_eq!(
            max_pass_ticks(0.0),
            u64::MAX,
            "unusable period disables the ceiling"
        );
        assert_eq!(max_pass_ticks(f32::NAN), u64::MAX);
    }

    #[test]
    fn max_pass_ticks_never_truncates_to_zero_for_a_period_over_one_second() {
        // 2e9 ns/tick puts the one-second ceiling at half a tick.
        let ceiling = max_pass_ticks(2.0e9);
        assert_eq!(ceiling, 1);
        let ticks = simulate_frame(LIVE_QUERIES, &[(0, 1)]);
        assert_eq!(
            decode_pair(mask(&[0]), 0, &ticks, ceiling),
            PairSample::Ticks(1),
            "a one-tick span must still decode as a sample"
        );
    }

    #[test]
    fn decode_pair_counts_implausibly_long_span_as_malformed() {
        // A 48-bit counter wrapped between the prefill's end and start writes
        // of skipped pair 1: its end slot holds a pre-wrap tick, its start slot
        // a post-wrap tick, so `end > start` by nearly the whole counter range.
        let mut ticks = simulate_frame(LIVE_QUERIES, &[(4, 7_000)]);
        ticks[2] = 5;
        ticks[3] = (1u64 << 48) - 10;
        let written = mask(&[1, 4]);
        assert_eq!(
            decode_pair(written, 1, &ticks, ONE_SECOND_TICKS),
            PairSample::Malformed
        );
        assert_eq!(
            decode_pair(written, 4, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(7_000)
        );

        // The ceiling itself is still a sample; one tick past it is not.
        ticks[2] = 1_000;
        ticks[3] = 1_000 + ONE_SECOND_TICKS;
        assert_eq!(
            decode_pair(written, 1, &ticks, ONE_SECOND_TICKS),
            PairSample::Ticks(ONE_SECOND_TICKS)
        );
        ticks[3] += 1;
        assert_eq!(
            decode_pair(written, 1, &ticks, ONE_SECOND_TICKS),
            PairSample::Malformed
        );
    }

    #[test]
    fn timing_window_averages_a_pass_over_the_readbacks_it_ran_in() {
        let labels = ["always", "half", "never"];
        let mut window = TimingWindow::new(labels.len());
        for readback in 0..AVG_WINDOW_SAMPLES {
            // "half" is marked every frame but encodes only on even ones.
            let ran: &[(usize, u64)] = if readback % 2 == 0 {
                &[(0, 2_000_000), (1, 1_000_000)]
            } else {
                &[(0, 2_000_000)]
            };
            let ticks = simulate_frame(LIVE_QUERIES, ran);
            window.record(mask(&[0, 1]), &ticks, 1.0, ONE_SECOND_TICKS);
        }

        assert_eq!(window.readbacks, AVG_WINDOW_SAMPLES);
        assert_eq!(
            window.present,
            vec![AVG_WINDOW_SAMPLES, AVG_WINDOW_SAMPLES / 2, 0]
        );
        assert!((window.average_ms(0).unwrap() - 2.0).abs() < 1e-9);
        let half_ms = window.average_ms(1).unwrap();
        assert!(
            (half_ms - 1.0).abs() < 1e-9,
            "absent readbacks must not dilute the average: {half_ms}"
        );
        assert_eq!(window.average_ms(2), None, "unsampled pass is not zero");

        let snapshot = window.snapshot(&labels);
        assert_eq!(snapshot.readbacks, AVG_WINDOW_SAMPLES);
        let half = snapshot.passes[1];
        assert_eq!(half.label, "half");
        assert!((half.average_ms.unwrap() - 1.0).abs() < 1e-6, "{half:?}");
        assert_eq!(half.sampled_readbacks, AVG_WINDOW_SAMPLES / 2);
        assert_eq!(half.malformed_readbacks, 0);
        let never = snapshot.passes[2];
        assert_eq!(never.average_ms, None);
        assert_eq!(never.sampled_readbacks, 0);

        let line = window.log_line(&labels);
        assert!(line.contains("always 2.00ms |"), "{line}");
        assert!(line.contains("half 1.00ms (60/120)"), "{line}");
        assert!(line.contains("never n/a (0/120)"), "{line}");
        assert!(!line.contains("malformed"), "{line}");
    }

    #[test]
    fn timing_window_counts_malformed_readbacks_apart_from_absent_ones() {
        let labels = ["ok", "bad", "absent"];
        let mut window = TimingWindow::new(labels.len());
        let mut ticks = simulate_frame(LIVE_QUERIES, &[(0, 500), (1, 500)]);
        // Zero the start slot of pair 1.
        ticks[2] = 0;
        window.record(mask(&[0, 1]), &ticks, 1.0, ONE_SECOND_TICKS);

        assert_eq!(window.malformed, vec![0, 1, 0]);
        let snapshot = window.snapshot(&labels);
        assert_eq!(snapshot.passes[1].malformed_readbacks, 1);
        assert_eq!(snapshot.passes[1].sampled_readbacks, 0);
        assert_eq!(snapshot.passes[2].malformed_readbacks, 0);

        let line = window.log_line(&labels);
        assert!(line.ends_with("; malformed: bad 1)"), "{line}");
    }
}
