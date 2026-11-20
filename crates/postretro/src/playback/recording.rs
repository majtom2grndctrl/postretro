// `--record-camera-route`: holds each presented world frame's pre-view-feel
// eye, yaw and pitch in memory, then writes one pose file resampled to the
// generator's 60 Hz cadence. Pure: no App, window or GPU; time is passed in.
// See: context/lib/rendering_pipeline.md §12

use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::Vec3;
use postretro_camera_route::{
    CADENCE_HZ, PoseFile, Projection, RouteHeader, RouteOrigin, RoutePose, format_pose_file,
};

use super::pacing;

/// Samples held before growth: a minute at 240 Hz. Growth past it is
/// allowed, since a recording frame is not a measured one.
const INITIAL_SAMPLES: usize = 60 * 240;

/// One presented world frame's view, before view feel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    /// The frame's presented-time stamp: the instant its redraw began.
    pub(crate) at: Instant,
    pub(crate) eye: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

/// Where a recording stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordState {
    /// No gameplay world frame has presented yet.
    Waiting,
    /// The first gameplay level's world frames are being held.
    Recording,
    /// The level ended or the session exited: nothing more is recorded.
    Ended,
}

pub(crate) struct Recorder {
    path: PathBuf,
    state: RecordState,
    /// The bake identity of the gameplay level installed last, as playback
    /// computes it at install. Fixed once the first world frame records.
    bake: Option<String>,
    /// The projection the first recorded frame drew at, before view feel.
    projection: Option<Projection>,
    /// The current frame's view, until its present says whether it drew.
    pending: Option<(Sample, Projection)>,
    samples: Vec<Sample>,
}

impl Recorder {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: RecordState::Waiting,
            bake: None,
            projection: None,
            pending: None,
            samples: Vec::with_capacity(INITIAL_SAMPLES),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn state(&self) -> RecordState {
        self.state
    }

    pub(crate) fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// Whether a gameplay level installed while nothing has recorded yet:
    /// its bake would name the recording.
    pub(crate) fn wants_level(&self) -> bool {
        self.state == RecordState::Waiting
    }

    /// A gameplay level installed with `bake`: until the first world frame
    /// records, the latest such level is the one a recording would cover.
    pub(crate) fn note_level(&mut self, bake: String) {
        if self.state == RecordState::Waiting {
            self.bake = Some(bake);
        }
    }

    /// This frame's pre-feel view, held until its present. `None` declines
    /// the frame (a frontend backdrop or menu frame); either way an earlier
    /// frame's unpresented view never carries over.
    pub(crate) fn offer(&mut self, view: Option<(Sample, Projection)>) {
        self.pending = match self.state {
            RecordState::Ended => None,
            RecordState::Waiting | RecordState::Recording => view,
        };
    }

    /// The current frame's present: a frame that drew the world records its
    /// held view. Returns true when that was the recording's first frame.
    pub(crate) fn note_presented(&mut self, presented: bool) -> bool {
        let Some((sample, projection)) = self.pending.take() else {
            return false;
        };
        if !presented || self.state == RecordState::Ended {
            return false;
        }
        self.samples.push(sample);
        if self.state == RecordState::Waiting {
            self.state = RecordState::Recording;
            self.projection = Some(projection);
            return true;
        }
        false
    }

    /// The frame ended: a view its present never claimed is dropped.
    pub(crate) fn end_frame(&mut self) {
        self.pending = None;
    }

    /// Ends the recording. Returns its file when any world frame recorded;
    /// `None` when it had already ended or never recorded a frame.
    pub(crate) fn finish(&mut self) -> Option<PoseFile> {
        let state = std::mem::replace(&mut self.state, RecordState::Ended);
        self.pending = None;
        if state != RecordState::Recording {
            return None;
        }
        Some(recorded_file(
            &self.samples,
            self.projection?,
            self.bake.take().unwrap_or_default(),
        ))
    }
}

/// The pose file for `samples`: origin recorded, resampled to 60 Hz, no
/// spawn, poses with no walk step or cell.
pub(crate) fn recorded_file(samples: &[Sample], projection: Projection, bake: String) -> PoseFile {
    PoseFile {
        header: RouteHeader {
            origin: RouteOrigin::Recorded,
            params: vec![
                ("source_hz".into(), source_hz(samples)),
                ("frames".into(), samples.len().to_string()),
            ],
            projection,
            bake,
            cadence_hz: CADENCE_HZ,
            spawn: None,
        },
        poses: resample(samples, f64::from(CADENCE_HZ)),
    }
}

/// Writes `file` to `path`: the recording's one write.
pub(crate) fn write(path: &Path, file: &PoseFile) -> Result<(), String> {
    std::fs::write(path, format_pose_file(file))
        .map_err(|err| format!("writing the camera route {}: {err}", path.display()))
}

/// The mean recorded frame rate, `none` with fewer than two frames or no
/// elapsed time.
fn source_hz(samples: &[Sample]) -> String {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return "none".into();
    };
    let span = last.at.saturating_duration_since(first.at).as_secs_f64();
    if samples.len() < 2 || span <= 0.0 {
        return "none".into();
    }
    format!("{:.3}", (samples.len() - 1) as f64 / span)
}

/// Pose k sits at t0 + k / `cadence_hz`, up to the last sample's time: eye
/// and pitch linear between the two samples bracketing it, yaw the short way
/// round. Interpolation is by time, never by frame, so a slow frame bridges
/// the camera's own motion and the source rate drops out.
pub(crate) fn resample(samples: &[Sample], cadence_hz: f64) -> Vec<RoutePose> {
    let Some(first) = samples.first() else {
        return Vec::new();
    };
    let seconds = |sample: &Sample| sample.at.saturating_duration_since(first.at).as_secs_f64();
    let span = samples.last().map_or(0.0, seconds);
    // Stamps are whole nanoseconds: the epsilon (17 ns at 60 Hz) keeps a
    // pose that lands on the last sample's time from rounding out. Such a
    // pose draws the last sample.
    let count = (span * cadence_hz + 1.0e-6).floor() as usize + 1;
    let mut poses = Vec::with_capacity(count);
    let mut from = 0;
    for k in 0..count {
        let t = k as f64 / cadence_hz;
        while from + 1 < samples.len() && seconds(&samples[from + 1]) <= t {
            from += 1;
        }
        let a = &samples[from];
        let (eye, yaw, pitch) = match samples.get(from + 1) {
            Some(b) => {
                let (ta, tb) = (seconds(a), seconds(b));
                let blend = ((t - ta) / (tb - ta)) as f32;
                pacing::interpolate(&route_pose(a), &route_pose(b), blend)
            }
            None => (a.eye, a.yaw, a.pitch),
        };
        poses.push(RoutePose {
            eye,
            yaw,
            pitch,
            walk: None,
            cut: false,
        });
    }
    poses
}

fn route_pose(sample: &Sample) -> RoutePose {
    RoutePose {
        eye: sample.eye,
        yaw: sample.yaw,
        pitch: sample.pitch,
        walk: None,
        cut: false,
    }
}
