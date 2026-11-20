// The playback timestamp ring's bookkeeping apart from its GPU objects:
// which slot group each marked frame records into, which spans it began, and
// how a readback decodes.
// See: context/lib/rendering_pipeline.md §12

use super::MarkSite;
use super::playback_timing::{PlaybackGpuSample, PlaybackGpuSpan, QUERIES_PER_FRAME, RING_FRAMES};

/// Longest span decoded as a sample: a longer one has a garbage endpoint, as
/// a counter narrower than 64 bits wrapping mid-frame leaves.
const MAX_SPAN_NS: f64 = 1.0e9;

/// One span of a frame's GPU work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Span {
    LightmapDrain = 0,
    /// First SH upload copy's first command to the last one's last; includes
    /// gaps between the separate submits and mover depth-tile flushes made at
    /// the SH drain.
    ShUploads = 1,
    /// The first command of the frame's earliest marked encoder to the last
    /// command of its frame encoder. On a drain frame the earliest is the
    /// lightmap drain, so the span includes CPU-side idle time and unmarked
    /// command buffers submitted before the Frame Encoder.
    WholeFrame = 2,
}

impl Span {
    pub(super) fn begin_slot(self) -> u32 {
        self as u32 * 2
    }

    pub(super) fn end_slot(self) -> u32 {
        self as u32 * 2 + 1
    }
}

/// Where a slot group stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupState {
    Idle,
    /// A marked frame records into it.
    Recording,
    /// Resolved and copied in a frame encoder; mapped after its submit.
    Copied,
    /// Its readback map is in flight.
    Mapping,
}

/// The ring's bookkeeping, apart from the GPU objects: which group each
/// marked frame records into, which spans it began, and what each group's
/// readback stands for.
#[derive(Debug)]
pub(super) struct Ring {
    pub(super) states: [GroupState; RING_FRAMES],
    pub(super) frames: [u64; RING_FRAMES],
    /// The drain spans each group's frame ran: lightmap, SH.
    pub(super) ran: [[bool; 2]; RING_FRAMES],
    /// The group the current marked frame records into, and the spans it began.
    pub(super) recording: Option<(usize, [bool; 3])>,
    pub(super) next: usize,
    /// Marked frames that found every group busy and went untimed.
    pub(super) untimed_frames: u64,
}

impl Ring {
    pub(super) fn new() -> Self {
        Self {
            states: [GroupState::Idle; RING_FRAMES],
            frames: [0; RING_FRAMES],
            ran: [[false; 2]; RING_FRAMES],
            recording: None,
            next: 0,
            untimed_frames: 0,
        }
    }

    /// A marked frame begins: it takes the next idle group, or goes untimed.
    /// A previous frame that never resolved (no surface) gives its group back.
    pub(super) fn begin_frame(&mut self, frame: u64) {
        self.end_frame();
        let free = (0..RING_FRAMES)
            .map(|offset| (self.next + offset) % RING_FRAMES)
            .find(|&group| self.states[group] == GroupState::Idle);
        match free {
            Some(group) => {
                self.states[group] = GroupState::Recording;
                self.frames[group] = frame;
                self.recording = Some((group, [false; 3]));
            }
            None => self.untimed_frames += 1,
        }
    }

    /// The mark cleared: a frame still recording resolves nothing.
    pub(super) fn end_frame(&mut self) {
        if let Some((group, _)) = self.recording.take() {
            self.states[group] = GroupState::Idle;
        }
    }

    /// Spans to begin as `site` opens: the whole frame's on the frame's first
    /// marked encoder, and the site's own drain span on its first encoder.
    pub(super) fn open(&mut self, site: MarkSite) -> Option<(usize, [Option<Span>; 2])> {
        let (group, began) = self.recording.as_mut()?;
        let mut begin = [None; 2];
        if !std::mem::replace(&mut began[Span::WholeFrame as usize], true) {
            begin[0] = Some(Span::WholeFrame);
        }
        if let Some(span) = drain_span(site)
            && !std::mem::replace(&mut began[span as usize], true)
        {
            begin[1] = Some(span);
        }
        Some((*group, begin))
    }

    /// The group a closing drain encoder ends its span in.
    pub(super) fn close_drain(&self, site: MarkSite) -> Option<(usize, Span)> {
        let (group, _) = self.recording?;
        Some((group, drain_span(site)?))
    }

    /// The frame encoder closes: the drain spans it never began, which take
    /// a zero-length pair so every slot is written before the resolve, and
    /// the group to resolve. The group waits for its map.
    pub(super) fn close_frame(&mut self) -> Option<(usize, [Option<Span>; 2])> {
        let (group, began) = self.recording.take()?;
        let ran = [
            began[Span::LightmapDrain as usize],
            began[Span::ShUploads as usize],
        ];
        self.ran[group] = ran;
        self.states[group] = GroupState::Copied;
        self.next = (group + 1) % RING_FRAMES;
        let unwritten = [
            (!ran[0]).then_some(Span::LightmapDrain),
            (!ran[1]).then_some(Span::ShUploads),
        ];
        Some((group, unwritten))
    }
}

fn drain_span(site: MarkSite) -> Option<Span> {
    match site {
        MarkSite::LightmapDrain => Some(Span::LightmapDrain),
        MarkSite::ShUploads => Some(Span::ShUploads),
        MarkSite::Frame => None,
    }
}

/// `MAX_SPAN_NS` in ticks of `ns_per_tick`; an unusable period disables it.
fn max_span_ticks(ns_per_tick: f32) -> u64 {
    if ns_per_tick.is_finite() && ns_per_tick > 0.0 {
        ((MAX_SPAN_NS / f64::from(ns_per_tick)) as u64).max(1)
    } else {
        u64::MAX
    }
}

fn decode_span(ticks: &[u64; QUERIES_PER_FRAME as usize], span: Span, ran: bool, ns_per_tick: f32) -> PlaybackGpuSpan {
    if !ran {
        return PlaybackGpuSpan::Absent;
    }
    let start = ticks[span.begin_slot() as usize];
    let end = ticks[span.end_slot() as usize];
    if start == 0 || end == 0 || end < start || end - start > max_span_ticks(ns_per_tick) {
        return PlaybackGpuSpan::Malformed;
    }
    PlaybackGpuSpan::Nanos(((end - start) as f64 * f64::from(ns_per_tick)).round() as u64)
}

pub(super) fn decode_group(
    frame: u64,
    ticks: &[u64; QUERIES_PER_FRAME as usize],
    ran: [bool; 2],
    ns_per_tick: f32,
) -> PlaybackGpuSample {
    PlaybackGpuSample {
        frame,
        whole_frame: decode_span(ticks, Span::WholeFrame, true, ns_per_tick),
        lightmap_drain: decode_span(ticks, Span::LightmapDrain, ran[0], ns_per_tick),
        sh_upload: decode_span(ticks, Span::ShUploads, ran[1], ns_per_tick),
    }
}
