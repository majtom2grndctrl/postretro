// GPU timestamp query helper for per-pass frame timing.
// Gated on `POSTRETRO_GPU_TIMING=1` and `Features::TIMESTAMP_QUERY`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How many frames to accumulate before logging an averaged line.
const AVG_WINDOW_FRAMES: u32 = 120;

const QUERY_SIZE: wgpu::BufferAddress = 8;

const QUERIES_PER_PASS: u32 = 2;

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
    map_result: Arc<Mutex<Option<Vec<u64>>>>,
    ns_per_tick: f32,
    pass_labels: Vec<&'static str>,
    accum_ns: Vec<f64>,
    accum_frames: u32,
    /// Per-pair count of frames where the pair was marked-written but
    /// resolved ticks were malformed (zero endpoint, end ≤ start).
    /// Surfaced in the averaged log line so mis-wired pair indices or
    /// driver anomalies don't silently report 0.00ms.
    accum_skipped: Vec<u32>,
    /// Bitmask of pair indices whose `render_pass_writes` /
    /// `compute_pass_writes` was called this frame. Swapped to zero at
    /// `encode_resolve` time. Used to distinguish "pass didn't run"
    /// (silent skip) from "pass ran but ticks are anomalous" (counted skip).
    ///
    /// `AtomicU64` so accessor methods stay `&self`; single-threaded in
    /// practice but the atomic is free here.
    pairs_written: AtomicU64,
    /// Snapshot of `pairs_written` taken when `encode_resolve` actually
    /// performed the `copy_buffer_to_buffer`. Paired with the tick
    /// snapshot that arrives via `map_async`, so `accumulate` only trusts
    /// ticks for pairs written in the frame those ticks came from.
    /// (`resolve_query_set` snapshots whatever the query set holds,
    /// including values written several frames ago.)
    pairs_written_in_flight: AtomicU64,
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

    fn mark_pair_written(&self, pair_idx: usize) {
        if pair_idx < 64 {
            self.pairs_written
                .fetch_or(1u64 << pair_idx, Ordering::Relaxed);
        }
    }

    /// Resolve the query set and copy into the readback buffer. Skips
    /// the copy when a previous `map_async` is still in flight — missing
    /// a single frame's data is preferable to stalling. Always drains
    /// `pairs_written` and, when the copy runs, stores the drained
    /// bitmask into `pairs_written_in_flight` so it travels with the
    /// tick snapshot arriving via `map_async`.
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
    /// on native; consumes a completed map if waiting, then kicks off a
    /// new `map_async`.
    pub fn post_submit(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);

        let snapshot = self.map_result.lock().unwrap().take();
        if let Some(ticks) = snapshot {
            self.accumulate(&ticks);
            self.readback_buffer.unmap();
            self.map_pending.store(false, Ordering::Release);
        }

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
                        // during the next `post_submit` after observing
                        // the result. Unmapping inside the callback would
                        // race with any outstanding view.
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
            // Pass was conditionally omitted this frame; skip so we
            // don't pick up stale ticks lingering in the query set.
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
