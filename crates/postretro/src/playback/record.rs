// The playback record: a header and one row per recorded frame, filled
// without per-frame allocation and written once when the run ends.
// See: context/lib/rendering_pipeline.md §12

use std::collections::BTreeMap;
use std::path::Path;

use postretro_camera_route::{PoseFile, Projection, RouteOrigin};
use serde::Serialize;

use super::args::PlaybackArgs;
use super::route::Identity;
use super::run::{FramePlan, RowKind};

pub(crate) const SCHEMA: &str = "postretro.camera_route_playback.v1";

/// Rows reserved past hold + poses, so frames the plan did not count
/// (later: no-draw rows) still fill without allocating.
const ROW_SLACK: usize = 256;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlaybackRecord {
    header: RecordHeader,
    rows: Vec<Row>,
}

#[derive(Debug, Clone, Serialize)]
struct RecordHeader {
    schema: &'static str,
    /// The source revision; `null` until a build-time source provides one.
    revision: Option<String>,
    pose_file: PoseFileReport,
    /// The loaded level's bake, once the first gameplay level installs.
    level: Option<LevelReport>,
    pacing: &'static str,
    hold_frames: u32,
    /// What route frame 0 rendered: the file's horizontal FOV at the
    /// window's aspect.
    rendered_projection: Option<ProjectionReport>,
    adapter: Option<AdapterReport>,
}

#[derive(Debug, Clone, Serialize)]
struct PoseFileReport {
    path: String,
    origin: String,
    params: BTreeMap<String, String>,
    projection: ProjectionReport,
    bake: String,
    cadence_hz: u32,
    poses: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ProjectionReport {
    hfov_degrees: f32,
    aspect: f32,
}

impl From<Projection> for ProjectionReport {
    fn from(projection: Projection) -> Self {
        Self {
            hfov_degrees: projection.hfov_degrees,
            aspect: projection.aspect,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct LevelReport {
    bake: String,
    identity: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct AdapterReport {
    name: String,
    backend: String,
}

/// One recorded frame. Plain data, so filling one allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct Row {
    /// The record's frame index, and the frame mark's `k`.
    pub(crate) frame: u64,
    pub(crate) phase: RowKind,
    pub(crate) pose: usize,
    /// Microseconds since the previous world frame began; `null` for the
    /// first, which has none.
    pub(crate) interval_us: Option<u64>,
}

impl PlaybackRecord {
    pub(crate) fn new(args: &PlaybackArgs, file: &PoseFile) -> Self {
        let header = &file.header;
        let origin = match header.origin {
            RouteOrigin::Generated(kind) => format!("generated {}", kind.key()),
            RouteOrigin::Recorded => "recorded".to_string(),
        };
        Self {
            header: RecordHeader {
                schema: SCHEMA,
                revision: None,
                pose_file: PoseFileReport {
                    path: args.poses.display().to_string(),
                    origin,
                    params: header.params.iter().cloned().collect(),
                    projection: header.projection.into(),
                    bake: header.bake.clone(),
                    cadence_hz: header.cadence_hz,
                    poses: file.poses.len(),
                },
                level: None,
                pacing: args.pacing.key(),
                hold_frames: args.hold,
                rendered_projection: None,
                adapter: None,
            },
            rows: Vec::with_capacity(args.hold as usize + file.poses.len() + ROW_SLACK),
        }
    }

    pub(crate) fn set_level(&mut self, bake: String, identity: Identity) {
        self.header.level = Some(LevelReport {
            bake,
            identity: identity.key(),
        });
    }

    /// Route frame 0's projection and the adapter that drew it.
    pub(crate) fn set_rendered(
        &mut self,
        hfov_degrees: f32,
        aspect: f32,
        adapter: Option<(String, String)>,
    ) {
        self.header.rendered_projection = Some(ProjectionReport {
            hfov_degrees,
            aspect,
        });
        self.header.adapter = adapter.map(|(name, backend)| AdapterReport { name, backend });
    }

    pub(crate) fn has_rendered(&self) -> bool {
        self.header.rendered_projection.is_some()
    }

    /// The index the next row takes: the frame mark of the frame recording.
    pub(crate) fn next_frame(&self) -> u64 {
        self.rows.len() as u64
    }

    pub(crate) fn push_row(&mut self, plan: FramePlan, interval_us: Option<u64>) {
        self.rows.push(Row {
            frame: self.next_frame(),
            phase: plan.kind,
            pose: plan.pose,
            interval_us,
        });
    }

    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The record's one write.
    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("serializing the playback record: {err}"))?;
        std::fs::write(path, text)
            .map_err(|err| format!("writing the playback record {}: {err}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use crate::playback::args::Pacing;
    use crate::playback::route::tests::pose_file;
    use std::path::PathBuf;

    fn args(hold: u32) -> PlaybackArgs {
        PlaybackArgs {
            poses: PathBuf::from("routes/cp.txt"),
            hold,
            report: PathBuf::from("cp.playback.json"),
            pacing: Pacing::FrameLocked,
        }
    }

    fn plan(kind: RowKind, pose: usize) -> FramePlan {
        FramePlan { pose, kind }
    }

    #[test]
    fn playback_record_header_and_rows_serialize_in_order() {
        let file = pose_file(100.0, 2);
        let mut record = PlaybackRecord::new(&args(1), &file);
        record.set_level(file.header.bake.clone(), Identity::Proven);
        record.push_row(plan(RowKind::Hold, 0), None);
        record.set_rendered(100.0, 1.5, Some(("GPU".into(), "Dx12".into())));
        record.push_row(plan(RowKind::Route, 0), Some(16_667));
        record.push_row(plan(RowKind::Route, 1), Some(16_600));
        let json: serde_json::Value = serde_json::to_value(&record).unwrap();

        let header = &json["header"];
        assert_eq!(header["schema"], SCHEMA);
        assert!(header["revision"].is_null());
        assert_eq!(header["pacing"], "frame-locked");
        assert_eq!(header["hold_frames"], 1);
        assert_eq!(header["pose_file"]["path"], "routes/cp.txt");
        assert_eq!(header["pose_file"]["origin"], "generated far-point");
        assert_eq!(header["pose_file"]["params"]["seed"], "1");
        assert_eq!(header["pose_file"]["projection"]["hfov_degrees"], 100.0);
        assert_eq!(header["pose_file"]["poses"], 2);
        assert_eq!(header["level"]["identity"], "proven");
        assert_eq!(header["rendered_projection"]["aspect"], 1.5);
        assert_eq!(header["adapter"]["backend"], "Dx12");

        let rows = json["rows"].as_array().unwrap();
        let summary: Vec<(u64, &str, u64)> = rows
            .iter()
            .map(|row| {
                (
                    row["frame"].as_u64().unwrap(),
                    row["phase"].as_str().unwrap(),
                    row["pose"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![(0, "hold", 0), (1, "route", 0), (2, "route", 1)]
        );
        assert!(
            rows[0]["interval_us"].is_null(),
            "the first row has no interval"
        );
        assert_eq!(rows[1]["interval_us"], 16_667);
    }

    #[test]
    fn playback_record_fills_its_rows_without_allocating() {
        let file = pose_file(100.0, 500);
        let mut record = PlaybackRecord::new(&args(600), &file);

        let control = AllocSnapshot::arm();
        std::hint::black_box(Box::new(7_u64));
        assert!(
            control.allocs_since() >= 1,
            "probe must count in this binary"
        );

        let probe = AllocSnapshot::arm();
        for _ in 0..600 {
            record.push_row(plan(RowKind::Hold, 0), Some(16_000));
        }
        for pose in 0..500 {
            record.push_row(plan(RowKind::Route, pose), Some(16_000));
        }
        assert_eq!(probe.allocs_since(), 0);
        assert_eq!(record.rows().len(), 1_100);
    }

    #[test]
    fn playback_record_writes_once_to_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.playback.json");
        let mut record = PlaybackRecord::new(&args(0), &pose_file(90.0, 1));
        record.push_row(plan(RowKind::Route, 0), None);
        record.write(&path).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["rows"].as_array().unwrap().len(), 1);
        let missing_dir = dir.path().join("absent").join("run.json");
        assert!(record.write(&missing_dir).unwrap_err().contains("absent"));
    }
}
