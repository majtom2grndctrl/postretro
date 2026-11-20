// The playback record: a header and one row per recorded frame, filled
// without per-frame allocation and written once when the run ends.
// See: context/lib/rendering_pipeline.md §12

use std::collections::BTreeMap;
use std::path::Path;

use postretro_camera_route::{PoseFile, Projection, RouteOrigin};
use serde::Serialize;

use super::args::{Pacing, PlaybackArgs};
use super::marks::{ChangeSet, LifeKey};
use super::route::Identity;
use super::run::{FramePlan, RowKind};

pub(crate) const SCHEMA: &str = "postretro.camera_route_playback.v1";

/// Rows reserved past the planned ones, so no-draw rows still fill without
/// allocating.
const ROW_SLACK: usize = 256;

/// Wall-clock route rows are planned at this frame rate over the route's
/// duration; a faster run grows the row store rarely, never per frame.
const WALL_CLOCK_PLANNED_HZ: f64 = 240.0;

/// Life-state entries reserved: a run logs one per death, respawn or
/// teleport, so the steady state never allocates.
const LIFE_STATE_SLACK: usize = 32;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlaybackRecord {
    header: RecordHeader,
    rows: Vec<Row>,
    /// Local pawn life-state changes, each on the row of the frame that saw it.
    life_states: Vec<LifeStateEntry>,
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
    /// How Settling ended: `settled` or `timed_out`.
    settle: Option<&'static str>,
    /// `complete` or `aborted`; `null` only in a record never written.
    outcome: Option<&'static str>,
    /// Why an aborted run ended.
    abort_reason: Option<String>,
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
    /// The pose drawn; under wall-clock pacing, the pose the frame's route
    /// time falls after.
    pub(crate) pose: usize,
    /// Wall-clock route and no-draw rows: seconds since route frame 0 began.
    pub(crate) route_time_s: Option<f64>,
    /// Microseconds since the previous world frame began; `null` for the
    /// first, which has none.
    pub(crate) interval_us: Option<u64>,
    /// Every mid-run change from the first row it touched on.
    pub(crate) changes: ChangeSet,
    /// The interval spans a hidden window or a frame that presented nothing.
    pub(crate) gap: bool,
}

/// One life-state change the local pawn went through.
#[derive(Debug, Clone, Serialize)]
struct LifeStateEntry {
    /// The row of the frame that saw it.
    frame: u64,
    /// The pawn now followed; `null` when none is.
    pawn: Option<u32>,
    life_state: Option<u8>,
    /// The life state's declared name, when the table names it.
    life_state_name: Option<String>,
    life_state_entry: Option<u32>,
    teleport_generation: Option<u8>,
    table_revision: Option<u8>,
    /// The life state the previous frame saw.
    previous_life_state: Option<u8>,
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
                settle: None,
                outcome: None,
                abort_reason: None,
                rendered_projection: None,
                adapter: None,
            },
            rows: Vec::with_capacity(args.hold as usize + planned_route_rows(args, file) + ROW_SLACK),
            life_states: Vec::with_capacity(LIFE_STATE_SLACK),
        }
    }

    pub(crate) fn set_settle(&mut self, timed_out: bool) {
        self.header.settle = Some(if timed_out { "timed_out" } else { "settled" });
    }

    /// Marks the record aborted with `reason`.
    pub(crate) fn set_aborted(&mut self, reason: String) {
        self.header.outcome = Some("aborted");
        self.header.abort_reason = Some(reason);
    }

    pub(crate) fn set_complete(&mut self) {
        self.header.outcome = Some("complete");
    }

    /// Logs a life-state change on the row the current frame takes.
    pub(crate) fn push_life_state(
        &mut self,
        now: Option<LifeKey>,
        name: Option<String>,
        previous: Option<LifeKey>,
    ) {
        self.life_states.push(LifeStateEntry {
            frame: self.next_frame(),
            pawn: now.map(|key| key.pawn),
            life_state: now.map(|key| key.life_state),
            life_state_name: name,
            life_state_entry: now.map(|key| key.life_state_entry),
            teleport_generation: now.map(|key| key.teleport_generation),
            table_revision: now.map(|key| key.table_revision),
            previous_life_state: previous.map(|key| key.life_state),
        });
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

    pub(crate) fn push_row(
        &mut self,
        plan: FramePlan,
        interval_us: Option<u64>,
        (changes, gap): (ChangeSet, bool),
    ) {
        self.rows.push(Row {
            frame: self.next_frame(),
            phase: plan.kind,
            pose: plan.pose,
            route_time_s: plan.route_time_s,
            interval_us,
            changes,
            gap,
        });
    }

    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg(test)]
    pub(crate) fn header_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.header).expect("the header serializes")
    }

    /// The record's one write.
    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("serializing the playback record: {err}"))?;
        std::fs::write(path, text)
            .map_err(|err| format!("writing the playback record {}: {err}", path.display()))
    }
}

/// Route rows a run plans: one per pose frame-locked; the route's duration
/// at [`WALL_CLOCK_PLANNED_HZ`] under wall-clock pacing.
fn planned_route_rows(args: &PlaybackArgs, file: &PoseFile) -> usize {
    match args.pacing {
        Pacing::FrameLocked => file.poses.len(),
        Pacing::WallClock => {
            let seconds = file.poses.len() as f64 / f64::from(file.header.cadence_hz.max(1));
            (seconds * WALL_CLOCK_PLANNED_HZ).ceil() as usize + 1
        }
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
            stall_timeout: crate::playback::args::DEFAULT_STALL_TIMEOUT,
        }
    }

    fn plan(kind: RowKind, pose: usize) -> FramePlan {
        FramePlan::for_test(kind, pose)
    }

    const UNMARKED: (ChangeSet, bool) = (ChangeSet::EMPTY, false);

    #[test]
    fn playback_record_header_and_rows_serialize_in_order() {
        let file = pose_file(100.0, 2);
        let mut record = PlaybackRecord::new(&args(1), &file);
        record.set_level(file.header.bake.clone(), Identity::Proven);
        record.push_row(plan(RowKind::Hold, 0), None, UNMARKED);
        record.set_rendered(100.0, 1.5, Some(("GPU".into(), "Dx12".into())));
        record.push_row(plan(RowKind::Route, 0), Some(16_667), UNMARKED);
        record.push_row(plan(RowKind::Route, 1), Some(16_600), UNMARKED);
        let json: serde_json::Value = serde_json::to_value(&record).unwrap();

        let header = &json["header"];
        assert_eq!(header["schema"], SCHEMA);
        assert!(header["revision"].is_null());
        assert!(header["settle"].is_null(), "no settle outcome until the reveal");
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
            record.push_row(plan(RowKind::Hold, 0), Some(16_000), UNMARKED);
        }
        for pose in 0..500 {
            record.push_row(plan(RowKind::Route, pose), Some(16_000), UNMARKED);
        }
        assert_eq!(probe.allocs_since(), 0);
        assert_eq!(record.rows().len(), 1_100);
    }

    #[test]
    fn playback_record_writes_once_to_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.playback.json");
        let mut record = PlaybackRecord::new(&args(0), &pose_file(90.0, 1));
        record.push_row(plan(RowKind::Route, 0), None, UNMARKED);
        record.write(&path).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["rows"].as_array().unwrap().len(), 1);
        let missing_dir = dir.path().join("absent").join("run.json");
        assert!(record.write(&missing_dir).unwrap_err().contains("absent"));
    }

    #[test]
    fn playback_record_carries_marks_settle_outcome_and_life_states() {
        let file = pose_file(100.0, 3);
        let mut record = PlaybackRecord::new(&args(0), &file);
        record.set_settle(true);
        let mut changes = ChangeSet::EMPTY;
        record.push_row(plan(RowKind::Route, 0), None, UNMARKED);
        let key = LifeKey {
            pawn: 7,
            teleport_generation: 1,
            life_state: 2,
            life_state_entry: 5,
            table_revision: 1,
        };
        record.push_life_state(Some(key), Some("dead".into()), Some(LifeKey { life_state: 0, ..key }));
        changes.insert(crate::playback::marks::ChangeKind::Extents);
        record.push_row(plan(RowKind::NoDraw, 1), Some(9), (changes, true));
        record.set_aborted("level unload requested".into());
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["header"]["settle"], "timed_out");
        assert_eq!(json["header"]["outcome"], "aborted");
        assert_eq!(json["header"]["abort_reason"], "level unload requested");
        let rows = json["rows"].as_array().unwrap();
        assert_eq!(rows[0]["changes"], serde_json::json!([]));
        assert_eq!(rows[0]["gap"], false);
        assert_eq!(rows[1]["phase"], "no_draw");
        assert_eq!(rows[1]["changes"], serde_json::json!(["extents"]));
        assert_eq!(rows[1]["gap"], true);
        let life = &json["life_states"][0];
        assert_eq!(life["frame"], 1, "logged on the row its frame takes");
        assert_eq!(life["life_state_name"], "dead");
        assert_eq!(life["previous_life_state"], 0);
    }

    #[test]
    fn playback_record_plans_wall_clock_rows_over_the_route_duration() {
        let file = pose_file(100.0, 600);
        let mut wall = args(10);
        wall.pacing = Pacing::WallClock;
        let record = PlaybackRecord::new(&wall, &file);
        // 600 poses at 60 Hz is 10 s: 2,400 rows at 240 Hz, plus the hold.
        assert!(record.rows.capacity() >= 10 + 2_400 + ROW_SLACK);
    }
}
