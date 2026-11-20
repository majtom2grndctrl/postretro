// Camera-route playback's GPU timing: its own timestamp query ring, one slot
// group per marked frame, bracketing the drain encoders and the whole frame.
// See: context/lib/rendering_pipeline.md §12

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use super::MarkSite;
use super::playback_ring::{GroupState, Ring, Span, decode_group};
use crate::render::gpu_create::GpuCreate;

/// Frames in flight before a marked frame finds no free slot group. Readbacks
/// return a frame or two after submission, so eight leaves no row untimed in
/// practice.
pub(crate) const RING_FRAMES: usize = 8;

/// Three spans per frame, a begin and an end slot each.
pub(super) const QUERIES_PER_FRAME: u32 = 6;
/// One timestamp is a little-endian `u64`.
const QUERY_SIZE: wgpu::BufferAddress = size_of::<u64>() as wgpu::BufferAddress;
const GROUP_BYTES: wgpu::BufferAddress = QUERIES_PER_FRAME as wgpu::BufferAddress * QUERY_SIZE;
/// Each group resolves to its own aligned offset of one resolve buffer.
const RESOLVE_STRIDE: wgpu::BufferAddress = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;

const MAP_PENDING: u8 = 0;
const MAP_READY: u8 = 1;
const MAP_FAILED: u8 = 2;

/// One span's decoded GPU time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackGpuSpan {
    /// The span's encoder did not run that frame.
    Absent,
    /// An endpoint read back zero, the end preceded the start, or the span
    /// passed one second.
    Malformed,
    Nanos(u64),
}

/// One marked frame's GPU times, returned once its readback lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackGpuSample {
    /// The frame mark's index.
    pub frame: u64,
    /// Begins at the frame's first marked encoder, which on a drain frame is
    /// the lightmap drain submitted in the streaming step. It includes
    /// CPU-side idle time and unmarked command buffers submitted between that
    /// encoder and the Frame Encoder.
    pub whole_frame: PlaybackGpuSpan,
    /// The "Streamed Lightmap Drain" encoder.
    pub lightmap_drain: PlaybackGpuSpan,
    /// The SH drain's marked encoders ("Streamed SH Upload Copies" and the
    /// SH and mover-channel pool growth copies): the first one's first
    /// command to the last one's last. A span, not a busy-time union: it
    /// includes the gaps between the separate submits, so it reads longer than
    /// the Mac reducer's union. The copies include mover depth-tile flushes
    /// made at the SH drain.
    pub sh_upload: PlaybackGpuSpan,
}

/// The playback ring on the GPU. Built when a run enables it, with every
/// buffer it uses, so a timed frame creates nothing.
pub(crate) struct PlaybackTiming {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readbacks: Vec<wgpu::Buffer>,
    /// Set by each group's map callback, which captures no buffer.
    maps: Vec<Arc<AtomicU8>>,
    ring: Ring,
    ns_per_tick: f32,
    /// Decoded samples not yet taken; reserved so a steady run never grows it.
    samples: Vec<PlaybackGpuSample>,
    #[cfg(test)]
    timestamp_writes: u64,
}

impl PlaybackTiming {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Playback Timing Query Set"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERIES_PER_FRAME * RING_FRAMES as u32,
        });
        let resolve = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Playback Timing Resolve Buffer"),
            size: RESOLVE_STRIDE * RING_FRAMES as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::QUERY_RESOLVE,
            mapped_at_creation: false,
        });
        let readbacks = (0..RING_FRAMES)
            .map(|_| {
                device.new_buffer(&wgpu::BufferDescriptor {
                    label: Some("Playback Timing Readback Buffer"),
                    size: GROUP_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            query_set,
            resolve,
            readbacks,
            maps: (0..RING_FRAMES)
                .map(|_| Arc::new(AtomicU8::new(MAP_PENDING)))
                .collect(),
            ring: Ring::new(),
            ns_per_tick: queue.get_timestamp_period(),
            samples: Vec::with_capacity(RING_FRAMES * 2),
            #[cfg(test)]
            timestamp_writes: 0,
        }
    }

    pub(crate) fn begin_frame(&mut self, frame: u64) {
        self.ring.begin_frame(frame);
    }

    pub(crate) fn end_frame(&mut self) {
        self.ring.end_frame();
    }

    /// Marked frames that went untimed because every group was busy.
    pub(crate) fn untimed_frames(&self) -> u64 {
        self.ring.untimed_frames
    }

    fn write(&mut self, encoder: &mut wgpu::CommandEncoder, group: usize, slot: u32) {
        encoder.write_timestamp(&self.query_set, group as u32 * QUERIES_PER_FRAME + slot);
        #[cfg(test)]
        {
            self.timestamp_writes += 1;
        }
    }

    /// A marked encoder opened: its spans' begin timestamps, first in it.
    pub(crate) fn open(&mut self, encoder: &mut wgpu::CommandEncoder, site: MarkSite) {
        let Some((group, begin)) = self.ring.open(site) else {
            return;
        };
        for span in begin.into_iter().flatten() {
            self.write(encoder, group, span.begin_slot());
        }
    }

    /// A marked encoder closes. A drain encoder ends its span; every later
    /// one of the frame moves that end. The frame encoder writes every slot
    /// the frame left unwritten, ends the whole frame, and resolves the
    /// group into its readback.
    pub(crate) fn close(&mut self, encoder: &mut wgpu::CommandEncoder, site: MarkSite) {
        if site != MarkSite::Frame {
            if let Some((group, span)) = self.ring.close_drain(site) {
                self.write(encoder, group, span.end_slot());
            }
            return;
        }
        let Some((group, unwritten)) = self.ring.close_frame() else {
            return;
        };
        for span in unwritten.into_iter().flatten() {
            self.write(encoder, group, span.begin_slot());
            self.write(encoder, group, span.end_slot());
        }
        self.write(encoder, group, Span::WholeFrame.end_slot());
        let first = group as u32 * QUERIES_PER_FRAME;
        let offset = group as wgpu::BufferAddress * RESOLVE_STRIDE;
        encoder.resolve_query_set(
            &self.query_set,
            first..first + QUERIES_PER_FRAME,
            &self.resolve,
            offset,
        );
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            offset,
            &self.readbacks[group],
            0,
            GROUP_BYTES,
        );
    }

    /// After a frame's submit: decode every landed readback, then start the
    /// map of every group resolved since. Never blocks.
    pub(crate) fn post_submit(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        self.harvest();
        self.start_maps();
    }

    fn harvest(&mut self) {
        for group in 0..RING_FRAMES {
            if self.ring.states[group] != GroupState::Mapping {
                continue;
            }
            match self.maps[group].load(Ordering::Acquire) {
                MAP_READY => {
                    let mut ticks = [0u64; QUERIES_PER_FRAME as usize];
                    {
                        let view = self.readbacks[group]
                            .slice(..)
                            .get_mapped_range()
                            .expect("a ready map's range is mapped; fails only after device loss");
                        for (tick, chunk) in ticks
                            .iter_mut()
                            .zip(view.as_chunks::<{ size_of::<u64>() }>().0)
                        {
                            *tick = u64::from_le_bytes(*chunk);
                        }
                    }
                    self.readbacks[group].unmap();
                    self.samples.push(decode_group(
                        self.ring.frames[group],
                        &ticks,
                        self.ring.ran[group],
                        self.ns_per_tick,
                    ));
                }
                // A failed map is a readback that never returned: the frame
                // has no sample, and its row reads absent.
                MAP_FAILED => {}
                _ => continue,
            }
            self.maps[group].store(MAP_PENDING, Ordering::Release);
            self.ring.states[group] = GroupState::Idle;
        }
    }

    fn start_maps(&mut self) {
        for group in 0..RING_FRAMES {
            if self.ring.states[group] != GroupState::Copied {
                continue;
            }
            self.ring.states[group] = GroupState::Mapping;
            let state = Arc::clone(&self.maps[group]);
            self.readbacks[group]
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let value = if result.is_ok() {
                        MAP_READY
                    } else {
                        MAP_FAILED
                    };
                    state.store(value, Ordering::Release);
                });
        }
    }

    /// Hands every decoded sample to `sink`, oldest first.
    pub(crate) fn take_samples(&mut self, sink: &mut impl FnMut(PlaybackGpuSample)) {
        for sample in self.samples.drain(..) {
            sink(sample);
        }
    }

    /// The run's end: waits up to `timeout` for every resolved group's
    /// readback, then hands over every sample. Returns the frames whose
    /// readback is still out.
    pub(crate) fn finish(
        &mut self,
        device: &wgpu::Device,
        timeout: Duration,
        sink: &mut impl FnMut(PlaybackGpuSample),
    ) -> usize {
        self.ring.end_frame();
        let deadline = Instant::now() + timeout;
        loop {
            self.harvest();
            self.start_maps();
            let outstanding = self.outstanding();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if outstanding == 0 || remaining.is_zero() {
                self.take_samples(sink);
                return outstanding;
            }
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(remaining),
            });
        }
    }

    fn outstanding(&self) -> usize {
        self.ring
            .states
            .iter()
            .filter(|state| matches!(state, GroupState::Copied | GroupState::Mapping))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn timestamp_writes(&self) -> u64 {
        self.timestamp_writes
    }
}

#[cfg(test)]
#[path = "playback_timing_tests.rs"]
mod tests;
