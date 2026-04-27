// GPU timestamp query helper for per-pass frame timing.
//
// Records begin/end timestamps around a fixed list of render / compute
// passes, resolves them into a GPU-side buffer, then asynchronously maps
// the copy into host memory. Results are averaged over a window of
// frames and logged at the same cadence.
//
// Gated on the `POSTRETRO_GPU_TIMING=1` environment variable and on the
// adapter supporting `Features::TIMESTAMP_QUERY`. When either is false,
// `Renderer::frame_timing` is `None` and no `timestamp_writes` are
// attached to any pass — the runtime cost is zero.
//
// Lifecycle per frame:
//   1. `encode_resolve(encoder)` — emit resolve_query_set + copy into
//      the readback buffer. Skipped internally when a map is pending so
//      we never `copy_buffer_to_buffer` into a still-mapped buffer.
//   2. `queue.submit(..)`
//   3. `post_submit(device)` — poll non-blocking; if a map completed,
//      accumulate into the averaging window (and log on boundary);
//      then kick off a fresh `map_async` if none is in flight.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How many frames to accumulate before logging an averaged line.
const AVG_WINDOW_FRAMES: u32 = 120;

/// Bytes per timestamp query result (u64).
const QUERY_SIZE: wgpu::BufferAddress = 8;

/// Two query indices per pass — beginning and end.
const QUERIES_PER_PASS: u32 = 2;

/// Owns the query set plus resolve / readback buffers and the averaging
/// accumulator. One `FrameTiming` covers a fixed list of pass labels
/// chosen at construction — pair index `i` maps to query indices
/// `[2*i, 2*i + 1]`.
pub struct FrameTiming {
    query_set: wgpu::QuerySet,
    /// Slot count matches `pass_labels.len()` × 2. Always padded up to
    /// the requested minimum of 16 slots so callers have room for future
    /// passes without reallocating the query set.
    num_queries: u32,
    resolve_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
    /// True between `map_async` submission and the callback firing.
    /// While pending, `encode_resolve` must not overwrite the readback
    /// buffer (it's still mapped on the host).
    map_pending: Arc<AtomicBool>,
    /// Populated by the `map_async` callback with a snapshot of the
    /// readback buffer as raw ticks. Consumed in `post_submit`.
    map_result: Arc<Mutex<Option<Vec<u64>>>>,
    /// Nanoseconds per timestamp tick, cached from `queue.get_timestamp_period()`.
    ns_per_tick: f32,
    /// Human-readable labels, one per pair (index = pair_idx).
    pass_labels: Vec<&'static str>,
    /// Accumulated frame-time sums per pass (in nanoseconds).
    accum_ns: Vec<f64>,
    /// Number of frames summed into `accum_ns` so far.
    accum_frames: u32,
    /// Per-pair count of frames where the pair *was* marked-written by
    /// the caller but the resolved ticks looked malformed (zero endpoint,
    /// end ≤ start). Surfaced in the averaged log line so mis-wired pair
    /// indices or driver anomalies don't silently report 0.00ms.
    accum_skipped: Vec<u32>,
    /// Bitmask of pair indices whose `render_pass_writes` /
    /// `compute_pass_writes` was called this frame. `fetch_or` from the
    /// accessor methods; swapped to zero at `encode_resolve` time. Used
    /// to distinguish "pass didn't run this frame" (silent skip) from
    /// "pass ran but tick values are anomalous" (counted skip).
    ///
    /// `AtomicU64` so the accessor methods stay `&self` and don't force
    /// `FrameTiming` to become `!Sync`; the bitmask is single-threaded
    /// in practice (main render thread) but the atomic is free here.
    pairs_written: AtomicU64,
    /// Snapshot of `pairs_written` taken when `encode_resolve` actually
    /// performed the `copy_buffer_to_buffer` (not when the copy was
    /// skipped because a map was in flight). Paired with the tick
    /// snapshot that eventually arrives via `map_async`, so `accumulate`
    /// only trusts ticks for pairs that were actually written in the
    /// frame those ticks came from.
    pairs_written_in_flight: AtomicU64,
}

impl FrameTiming {
    /// Create a new timing helper. `pass_labels.len()` pairs of query
    /// slots will be allocated (plus padding to 16 slots minimum), and
    /// `encode_resolve` copies all of them out each frame.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, pass_labels: Vec<&'static str>) -> Self {
        let pair_count = pass_labels.len() as u32;
        // 16 slots = 8 pairs, per the calling convention — pad up so
        // callers can slot in future passes without resizing.
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

        // With a u64 bitmask for `pairs_written`, we can track up to 64
        // pairs. The query set is already capped at 8 pairs via the
        // 16-slot minimum, so 64 is well beyond anything we'd wire up,
        // but assert the contract here so a future expansion past 64
        // pairs fails loudly at construction rather than silently
        // losing the high bits.
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
            map_result: Arc::new(Mutex::new(None)),
            ns_per_tick,
            pass_labels,
            accum_ns: vec![0.0; pair_usize],
            accum_frames: 0,
            accum_skipped: vec![0; pair_usize],
            pairs_written: AtomicU64::new(0),
            pairs_written_in_flight: AtomicU64::new(0),
        }
    }

    /// Render-pass timestamp writes for pair `pair_idx`. The returned
    /// struct borrows the query set, so the caller must keep `self`
    /// alive for the duration of the pass descriptor. Also marks pair
    /// `pair_idx` as written-this-frame so `accumulate` can distinguish
    /// a pass that ran from a pass that was conditionally skipped.
    pub fn render_pass_writes(&self, pair_idx: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        self.mark_pair_written(pair_idx);
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        }
    }

    /// Compute-pass timestamp writes for pair `pair_idx`. Also marks
    /// pair `pair_idx` as written-this-frame.
    pub fn compute_pass_writes(&self, pair_idx: usize) -> wgpu::ComputePassTimestampWrites<'_> {
        self.mark_pair_written(pair_idx);
        let base = (pair_idx as u32) * QUERIES_PER_PASS;
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        }
    }

    /// Set bit `pair_idx` in the `pairs_written` bitmask. No-op when
    /// the pair index is out of range (the accessor methods guard this
    /// but we defend here too).
    fn mark_pair_written(&self, pair_idx: usize) {
        if pair_idx < 64 {
            self.pairs_written
                .fetch_or(1u64 << pair_idx, Ordering::Relaxed);
        }
    }

    /// Resolve the query set into the resolve buffer, then copy into the
    /// host-mappable readback buffer. Skips the copy (but always
    /// resolves, so the query set stays valid) when a previous
    /// `map_async` is still in flight — missing a single frame's data
    /// is preferable to stalling the frame.
    ///
    /// Always drains the `pairs_written` bitmask for this frame. When
    /// the copy runs, the drained bitmask is stored into
    /// `pairs_written_in_flight` so it travels alongside the tick
    /// snapshot that will eventually arrive via `map_async`. This is
    /// how `accumulate` avoids reading stale ticks for pairs whose pass
    /// didn't run this frame — `resolve_query_set` snapshots whatever
    /// the query set currently holds, including values written several
    /// frames ago, so filtering at accumulate time is the only honest
    /// way to report per-frame timings.
    pub fn encode_resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let written_this_frame = self.pairs_written.swap(0, Ordering::Relaxed);
        encoder.resolve_query_set(
            &self.query_set,
            0..self.num_queries,
            &self.resolve_buffer,
            0,
        );
        if !self.map_pending.load(Ordering::Acquire) {
            encoder.copy_buffer_to_buffer(
                &self.resolve_buffer,
                0,
                &self.readback_buffer,
                0,
                self.num_queries as wgpu::BufferAddress * QUERY_SIZE,
            );
            self.pairs_written_in_flight
                .store(written_this_frame, Ordering::Relaxed);
        }
    }

    /// Drive the async map state machine. Called once per frame AFTER
    /// `queue.submit`. Non-blocking poll drives any ready map callbacks
    /// on native; if a completed map is waiting, we consume + accumulate
    /// it, unmap, and kick off a new `map_async`.
    pub fn post_submit(&mut self, device: &wgpu::Device) {
        // Non-blocking poll so mapping callbacks can fire on native.
        let _ = device.poll(wgpu::PollType::Poll);

        // Consume a completed map, if any.
        let snapshot = self.map_result.lock().unwrap().take();
        if let Some(ticks) = snapshot {
            self.accumulate(&ticks);
            self.readback_buffer.unmap();
            self.map_pending.store(false, Ordering::Release);
        }

        // Kick off a new map if the readback buffer isn't mapped.
        if !self.map_pending.load(Ordering::Acquire) {
            self.map_pending.store(true, Ordering::Release);
            let result_slot = Arc::clone(&self.map_result);
            let pending = Arc::clone(&self.map_pending);
            let buffer_size = self.num_queries as wgpu::BufferAddress * QUERY_SIZE;
            let buf = self.readback_buffer.clone();
            self.readback_buffer
                .slice(0..buffer_size)
                .map_async(wgpu::MapMode::Read, move |res| match res {
                    Ok(()) => {
                        let view = buf.slice(0..buffer_size).get_mapped_range();
                        let mut ticks = Vec::with_capacity(view.len() / 8);
                        for chunk in view.chunks_exact(8) {
                            ticks.push(u64::from_le_bytes(chunk.try_into().unwrap()));
                        }
                        drop(view);
                        // Buffer stays mapped; the main thread unmaps it
                        // during the next `post_submit` call after
                        // observing the result. Unmapping inside the
                        // callback would race with any outstanding view.
                        *result_slot.lock().unwrap() = Some(ticks);
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
        for pair_idx in 0..self.pass_labels.len() {
            // Pair not marked written this frame → pass was
            // conditionally omitted; silently skip so we don't pick up
            // stale ticks lingering in the query set.
            if pair_idx >= 64 || (written & (1u64 << pair_idx)) == 0 {
                continue;
            }
            let base = pair_idx * (QUERIES_PER_PASS as usize);
            if base + 1 >= ticks.len() {
                self.accum_skipped[pair_idx] += 1;
                continue;
            }
            let start = ticks[base];
            let end = ticks[base + 1];
            // Pair was marked written but the resolved ticks look
            // malformed — driver/adapter anomaly or a mis-wired pair
            // index. Count it so the averaged log line surfaces the
            // issue instead of reporting 0.00ms.
            if start == 0 || end == 0 || end <= start {
                self.accum_skipped[pair_idx] += 1;
                continue;
            }
            let delta = end - start;
            self.accum_ns[pair_idx] += delta as f64 * self.ns_per_tick as f64;
        }
        self.accum_frames += 1;

        if self.accum_frames >= AVG_WINDOW_FRAMES {
            let mut parts: Vec<String> = Vec::with_capacity(self.pass_labels.len());
            for (i, label) in self.pass_labels.iter().enumerate() {
                let avg_ms = self.accum_ns[i] / (self.accum_frames as f64) / 1.0e6;
                parts.push(format!("{label} {avg_ms:.2}ms"));
            }
            // Surface any pair with a non-zero skip count in this
            // window. Common case (no skips) keeps the log line terse.
            let skip_parts: Vec<String> = self
                .pass_labels
                .iter()
                .zip(self.accum_skipped.iter())
                .filter_map(|(label, &n)| {
                    if n > 0 {
                        Some(format!("{label} {n}"))
                    } else {
                        None
                    }
                })
                .collect();
            if skip_parts.is_empty() {
                log::info!(
                    "[gpu-timing] {} (avg over {} frames)",
                    parts.join(" | "),
                    self.accum_frames,
                );
            } else {
                log::info!(
                    "[gpu-timing] {} (avg over {} frames; skipped: {})",
                    parts.join(" | "),
                    self.accum_frames,
                    skip_parts.join(", "),
                );
            }
            for v in self.accum_ns.iter_mut() {
                *v = 0.0;
            }
            for v in self.accum_skipped.iter_mut() {
                *v = 0;
            }
            self.accum_frames = 0;
        }
    }
}
