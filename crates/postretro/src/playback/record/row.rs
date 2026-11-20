// A playback row's measured columns: the frame's CPU split, what its drains
// admitted and did, its late counts, and its GPU times. Fixed-size fields, so
// filling a row allocates nothing.
// See: context/lib/rendering_pipeline.md §12

use postretro_renderer::cpu_stages::RenderStage;
use postretro_renderer::{PlaybackDrainCounters, PlaybackGpuSample, PlaybackGpuSpan};
use postretro_stage_timing::{FrameRecord, StageKind, StageSet};
use serde::Serialize;
use serde::ser::SerializeMap;

use crate::cpu_timing::{FrameStage, StreamingStage, derived};
use crate::session::level_streaming::DrainAdmission;

/// The derived frame split, every top-level time stage but `ticks`, and the
/// three streaming stages.
pub(crate) const CPU_COLUMNS: usize = 6 + 14 + 3;

/// The CPU columns a row copies, in order.
pub(crate) fn cpu_labels() -> impl Iterator<Item = &'static str> {
    [
        derived::TOTAL,
        derived::WORK,
        derived::WAIT,
        derived::WAIT_ACQUIRE,
        derived::WAIT_PRESENT,
        derived::UNATTRIBUTED,
    ]
    .into_iter()
    .chain(
        FrameStage::ALL
            .iter()
            .filter(|stage| stage.kind() == StageKind::Time)
            .map(|stage| stage.label()),
    )
    .chain(StreamingStage::ALL.iter().map(|stage| stage.label()))
    .chain(std::iter::once(RenderStage::ShDrain.label()))
}

/// A fixed selection of the frame's composed CPU record, in nanoseconds;
/// a stage that did not run reads `null`, never zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowCpu([Option<u64>; CPU_COLUMNS]);

impl RowCpu {
    pub(crate) fn from_record(record: &FrameRecord) -> Self {
        let mut values = [None; CPU_COLUMNS];
        for (value, label) in values.iter_mut().zip(cpu_labels()) {
            *value = record.value(label);
        }
        Self(values)
    }

    /// A split holding only `values`, by label.
    #[cfg(test)]
    pub(crate) fn with_values(values: &[(&str, u64)]) -> Self {
        let mut out = [None; CPU_COLUMNS];
        for (label, value) in values {
            let index = cpu_labels()
                .position(|column| column == *label)
                .unwrap_or_else(|| panic!("no CPU column `{label}`"));
            out[index] = Some(*value);
        }
        Self(out)
    }

    /// One column by its label; `None` when the stage did not run or no
    /// column has that label.
    pub(crate) fn value(&self, label: &str) -> Option<u64> {
        cpu_labels()
            .position(|column| column == label)
            .and_then(|index| self.0[index])
    }
}

impl Serialize for RowCpu {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(CPU_COLUMNS))?;
        for (label, value) in cpu_labels().zip(self.0) {
            map.serialize_entry(label, &value)?;
        }
        map.end()
    }
}

/// What a frame's drains did: per-resource admission against the shared
/// budget, whether a drain's staging pool created a buffer (cold) or a
/// lightmap pool grew, and the frame's late counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RowDrain {
    pub(crate) lightmap_admitted_items: usize,
    pub(crate) lightmap_admitted_bytes: u64,
    pub(crate) sh_admitted_items: usize,
    pub(crate) sh_admitted_bytes: u64,
    /// Either drain's staging pool created a buffer.
    pub(crate) cold: bool,
    pub(crate) cold_lightmap: bool,
    /// The SH drain's staging pool created a buffer, the mover depth-tile
    /// flushes made at the SH drain included.
    pub(crate) cold_sh: bool,
    /// A lightmap drain grew the lightmap or the mask pool.
    pub(crate) pool_grew: bool,
    pub(crate) lightmap_pool_grew: bool,
    pub(crate) mask_pool_grew: bool,
    /// Lightmap blocks the frame drew while not installed; `null` when the
    /// level's lightmap does not stream.
    pub(crate) lightmap_late_block_frames: Option<u32>,
    /// SH clusters the frame drew while not Sampleable; `null` when the
    /// level's SH does not stream.
    pub(crate) sh_late_cluster_frames: Option<u32>,
}

impl RowDrain {
    pub(crate) fn new(
        admission: DrainAdmission,
        before: PlaybackDrainCounters,
        after: PlaybackDrainCounters,
        late: (Option<u32>, Option<u32>),
    ) -> Self {
        let cold_lightmap = after.lightmap_cold_since(before);
        let cold_sh = after.sh_cold_since(before);
        let lightmap_pool_grew = after.lightmap_pool_grew_since(before);
        let mask_pool_grew = after.mask_pool_grew_since(before);
        Self {
            lightmap_admitted_items: admission.lightmap.admitted,
            lightmap_admitted_bytes: admission.lightmap.bytes,
            sh_admitted_items: admission.sh.admitted,
            sh_admitted_bytes: admission.sh.bytes,
            cold: cold_lightmap || cold_sh,
            cold_lightmap,
            cold_sh,
            pool_grew: lightmap_pool_grew || mask_pool_grew,
            lightmap_pool_grew,
            mask_pool_grew,
            lightmap_late_block_frames: late.0,
            sh_late_cluster_frames: late.1,
        }
    }
}

/// Every measured column a row takes from its frame, gathered before the row
/// is pushed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct RowFill {
    /// `null` with CPU timing off, and for a frame the timer excluded (no
    /// surface): its record never composed.
    pub(crate) cpu_ns: Option<RowCpu>,
    /// Fixed ticks the frame ran; absent with `cpu_ns`.
    pub(crate) ticks: Option<u64>,
    pub(crate) drain: RowDrain,
}

impl RowFill {
    pub(crate) fn new(record: Option<&FrameRecord>, drain: RowDrain) -> Self {
        Self {
            cpu_ns: record.map(RowCpu::from_record),
            ticks: record.and_then(|record| record.value(FrameStage::Ticks.label())),
            drain,
        }
    }
}

/// A frame's GPU times, in microseconds; a span that did not run or
/// decoded malformed reads `null`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct RowGpu {
    /// The first command of the frame's earliest marked encoder (its
    /// lightmap drain, else its SH upload copies, else its frame encoder) to
    /// the last command of its frame encoder: a span, idle gaps between
    /// submissions included, not busy time. The per-frame upload copies
    /// before the first marked encoder and the debug UI after the frame
    /// encoder fall outside it.
    pub(crate) frame_us: Option<f64>,
    /// The lightmap drain's span: its first command to its last, idle gaps
    /// between its submissions included.
    pub(crate) lightmap_drain_us: Option<f64>,
    /// The SH upload copies' span, likewise, the mover depth-tile flushes
    /// made at the SH drain included.
    pub(crate) sh_upload_us: Option<f64>,
    /// Spans that decoded malformed.
    #[serde(skip)]
    pub(crate) malformed: u8,
}

impl From<PlaybackGpuSample> for RowGpu {
    fn from(sample: PlaybackGpuSample) -> Self {
        let spans = [sample.whole_frame, sample.lightmap_drain, sample.sh_upload];
        let micros = |span: PlaybackGpuSpan| match span {
            PlaybackGpuSpan::Nanos(ns) => Some(ns as f64 / 1_000.0),
            PlaybackGpuSpan::Absent | PlaybackGpuSpan::Malformed => None,
        };
        Self {
            frame_us: micros(sample.whole_frame),
            lightmap_drain_us: micros(sample.lightmap_drain),
            sh_upload_us: micros(sample.sh_upload),
            malformed: spans
                .iter()
                .filter(|span| **span == PlaybackGpuSpan::Malformed)
                .count() as u8,
        }
    }
}
