// The playback record: header fields, row columns, GPU patching, and the
// allocation-free fill and single write.
// See: context/lib/rendering_pipeline.md §12

use std::path::PathBuf;
use std::time::{Duration, Instant};

use postretro_renderer::{
    GpuTimingSupport, PlaybackDrainCounters, PlaybackGpuSample, PlaybackGpuSpan,
};
use postretro_stage_timing::{StageFrame, StageSet};

use super::row::{CPU_COLUMNS, cpu_labels};
use super::*;
use crate::alloc_probe::AllocSnapshot;
use crate::cpu_timing::{CpuFrameTimer, FrameStage, StreamingStage, TimingGate};
use crate::playback::args::Pacing;
use crate::playback::route::tests::pose_file;
use crate::session::level_streaming::DrainAdmission;
use crate::streaming::shared_drain::ResourceAdmission;

fn args(hold: u32) -> PlaybackArgs {
    PlaybackArgs {
        poses: PathBuf::from("routes/cp.txt"),
        hold,
        report: PathBuf::from("cp.playback.json"),
        pacing: Pacing::FrameLocked,
        stall_timeout: crate::playback::args::DEFAULT_STALL_TIMEOUT,
        present: postretro_renderer::PresentChoice::Vsync,
    }
}

fn plan(kind: RowKind, pose: usize) -> FramePlan {
    FramePlan::for_test(kind, pose)
}

const UNMARKED: (ChangeSet, bool) = (ChangeSet::EMPTY, false);

fn no_fill() -> RowFill {
    RowFill::default()
}

fn context() -> RunContext {
    RunContext {
        adapter: AdapterReport {
            name: "GPU".into(),
            backend: "Dx12".into(),
            device_type: "DiscreteGpu".into(),
        },
        surface_extent: [1920, 1080],
        scene_extent: [960, 540],
        render_divisor: 2,
        window_mode: WindowModeReport {
            mode: "windowed",
            display: None,
        },
        present_mode: "vsync",
    }
}

/// One committed frame: a fixed step of `ticks` ticks and a lightmap drain
/// inside render prep; the timer composes its record at commit.
fn committed_frame(timer: &mut CpuFrameTimer, ticks: u64) {
    let start = Instant::now();
    timer.begin_frame(start);
    let stages = timer.stages();
    stages.add_nanos(FrameStage::FixedStep, 4_000);
    stages.add_count(FrameStage::Ticks, ticks);
    stages.add_nanos(FrameStage::RenderPrep, 3_000);
    drop(stages);
    let streaming = StageFrame::<StreamingStage>::new(timer.gate());
    streaming.add_nanos(StreamingStage::LightmapResidency, 1_000);
    streaming.add_nanos(StreamingStage::LightmapDrain, 500);
    timer
        .nested_mut()
        .extend_from(&streaming, Some(FrameStage::RenderPrep.label()));
    timer.commit_frame(start + Duration::from_nanos(10_000));
}

fn admission(items: usize, bytes: u64) -> ResourceAdmission {
    ResourceAdmission {
        admitted: items,
        bytes,
        left_behind: false,
    }
}

#[test]
fn playback_record_header_and_rows_serialize_in_order() {
    let file = pose_file(100.0, 2);
    let mut record = PlaybackRecord::new(&args(1), &file);
    record.set_level(file.header.bake.clone(), Identity::Proven);
    record.set_context(context());
    record.push_row(plan(RowKind::Hold, 0), None, UNMARKED, no_fill());
    record.set_rendered(100.0, 1.5);
    record.push_row(plan(RowKind::Route, 0), Some(16_667), UNMARKED, no_fill());
    record.push_row(plan(RowKind::Route, 1), Some(16_600), UNMARKED, no_fill());
    let json: serde_json::Value = serde_json::to_value(&record).unwrap();

    let header = &json["header"];
    assert_eq!(header["schema"], SCHEMA);
    assert!(
        header["settle"].is_null(),
        "no settle outcome until the reveal"
    );
    assert_eq!(header["pacing"], "frame-locked");
    assert_eq!(header["hold_frames"], 1);
    assert_eq!(header["pose_file"]["path"], "routes/cp.txt");
    assert_eq!(header["pose_file"]["origin"], "generated far-point");
    assert_eq!(header["pose_file"]["params"]["seed"], "1");
    assert_eq!(header["pose_file"]["projection"]["hfov_degrees"], 100.0);
    assert_eq!(header["pose_file"]["poses"], 2);
    assert_eq!(header["level"]["identity"], "proven");
    assert_eq!(header["rendered_projection"]["aspect"], 1.5);
    assert_eq!(header["machine"]["adapter"]["backend"], "Dx12");

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
fn playback_record_header_names_every_field_null_until_known() {
    let file = pose_file(100.0, 3);
    let mut record = PlaybackRecord::new(&args(2), &file);
    let header = record.header_json();
    for key in [
        "schema",
        "revision",
        "build",
        "pose_file",
        "level",
        "pacing",
        "hold_frames",
        "present_mode",
        "cap",
        "settle",
        "outcome",
        "abort_reason",
        "rendered_projection",
        "machine",
        "cpu_timing",
        "gpu_timing",
        "gpu_clock",
    ] {
        assert!(header.get(key).is_some(), "header names `{key}`");
    }
    for unknown in ["level", "settle", "rendered_projection", "machine"] {
        assert!(header[unknown].is_null(), "`{unknown}` is null until known");
    }
    assert!(header["gpu_timing"]["rows_missing"].is_null());
    assert_eq!(header["gpu_clock"], "unrecorded");
    assert_eq!(header["present_mode"], "vsync");
    assert_eq!(header["cap"]["source"], "production");
    assert_eq!(
        header["cap"]["decoded_bytes_per_drain"],
        crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN
    );
    assert_eq!(header["cpu_timing"]["state"], "pending");
    let build = &header["build"];
    assert!(build["profile"].is_string());
    assert_eq!(build["debug_assertions"], cfg!(debug_assertions));
    assert_eq!(
        build["features"]
            .as_array()
            .unwrap()
            .contains(&"dev-tools".into()),
        cfg!(feature = "dev-tools")
    );

    record.set_level(file.header.bake.clone(), Identity::Unproven);
    record.set_settle(false);
    record.set_rendered(100.0, 16.0 / 9.0);
    record.set_context(context());
    record.set_timing(false, GpuTimingSupport::Unsupported);
    let header = record.header_json();
    assert_eq!(header["level"]["identity"], "unproven");
    assert_eq!(header["level"]["bake"], file.header.bake.as_str());
    assert_eq!(header["pose_file"]["bake"], file.header.bake.as_str());
    assert_eq!(header["settle"], "settled");
    let machine = &header["machine"];
    for key in [
        "adapter",
        "surface_extent",
        "scene_extent",
        "render_divisor",
        "window_mode",
        "present_mode",
    ] {
        assert!(machine.get(key).is_some(), "machine names `{key}`");
    }
    assert_eq!(machine["adapter"]["device_type"], "DiscreteGpu");
    assert_eq!(machine["render_divisor"], 2);
    assert!(machine["window_mode"]["display"].is_null());
    assert_eq!(header["cpu_timing"]["state"], "disabled");
    assert_eq!(header["gpu_timing"]["state"], "unsupported");
    assert!(header["gpu_timing"]["reason"].is_string());
}

#[test]
fn playback_row_cpu_columns_cover_the_split_the_stages_and_the_streaming_stages() {
    let labels: Vec<&str> = cpu_labels().collect();
    assert_eq!(labels.len(), CPU_COLUMNS);
    let unique: std::collections::BTreeSet<&str> = labels.iter().copied().collect();
    assert_eq!(unique.len(), CPU_COLUMNS, "no column twice");
    for stage in FrameStage::ALL
        .iter()
        .filter(|stage| **stage != FrameStage::Ticks)
    {
        assert!(labels.contains(&stage.label()), "{}", stage.label());
    }
    for label in [
        "total",
        "work",
        "wait",
        "wait_acquire",
        "wait_present",
        "fixed_step",
        "lightmap_residency",
        "lightmap_drain",
        "render_sh_drain",
    ] {
        assert!(labels.contains(&label), "{label}");
    }
}

#[test]
fn playback_row_cpu_fields_are_absent_with_timing_off_and_carry_fixed_step_and_ticks_with_it_on() {
    // AC 13, 26.
    let mut off = CpuFrameTimer::new(TimingGate::OFF);
    committed_frame(&mut off, 2);
    let fill = RowFill::new(off.committed_record(), RowDrain::default());
    assert_eq!(fill.cpu_ns, None);
    assert_eq!(fill.ticks, None);
    let json = serde_json::to_value(fill).unwrap();
    assert!(json["cpu_ns"].is_null(), "absent, never zero");
    assert!(json["ticks"].is_null());
    assert!(json["drain"].is_object(), "drain columns stay present");

    let mut on = CpuFrameTimer::new(TimingGate::ON);
    committed_frame(&mut on, 3);
    let fill = RowFill::new(on.committed_record(), RowDrain::default());
    let cpu = fill.cpu_ns.expect("a committed frame carries its split");
    assert_eq!(cpu.value("fixed_step"), Some(4_000));
    assert_eq!(fill.ticks, Some(3));
    assert_eq!(cpu.value("total"), Some(10_000));
    assert_eq!(cpu.value("lightmap_drain"), Some(500));
    assert_eq!(
        cpu.value("render_sh_drain"),
        None,
        "a stage that did not run"
    );
    let json = serde_json::to_value(fill).unwrap();
    assert_eq!(json["cpu_ns"]["fixed_step"], 4_000);
    assert!(json["cpu_ns"]["render_sh_drain"].is_null());
    assert_eq!(json["ticks"], 3);

    // A frame the timer excludes (no surface) never composes: its row reads
    // absent rather than the previous frame's split.
    on.begin_frame(Instant::now());
    on.exclude_frame();
    on.commit_frame(Instant::now());
    assert!(on.committed_record().is_none());
}

#[test]
fn playback_row_drain_columns_carry_admission_cold_growth_and_late_counts() {
    let before = PlaybackDrainCounters {
        lightmap_staging_created: 2,
        sh_staging_created: 1,
        lightmap_pool_growths: 0,
        mask_pool_growths: 0,
    };
    let warm = RowDrain::new(
        DrainAdmission {
            sh: admission(2, 4_096),
            lightmap: admission(3, 65_536),
        },
        before,
        before,
        (Some(4), Some(0)),
    );
    assert_eq!(
        (warm.lightmap_admitted_items, warm.lightmap_admitted_bytes),
        (3, 65_536)
    );
    assert_eq!((warm.sh_admitted_items, warm.sh_admitted_bytes), (2, 4_096));
    assert!(!warm.cold && !warm.pool_grew);
    assert_eq!(warm.lightmap_late_block_frames, Some(4));

    let after = PlaybackDrainCounters {
        sh_staging_created: 2,
        lightmap_pool_growths: 1,
        ..before
    };
    let cold = RowDrain::new(DrainAdmission::default(), before, after, (None, None));
    assert!(cold.cold && cold.cold_sh && !cold.cold_lightmap);
    assert!(cold.pool_grew && cold.lightmap_pool_grew && !cold.mask_pool_grew);
    let json = serde_json::to_value(cold).unwrap();
    assert!(
        json["lightmap_late_block_frames"].is_null(),
        "a resource that does not stream reads null"
    );
    assert_eq!(json["sh_admitted_bytes"], 0);
}

fn sample(frame: u64, whole: PlaybackGpuSpan) -> PlaybackGpuSample {
    PlaybackGpuSample {
        frame,
        whole_frame: whole,
        lightmap_drain: PlaybackGpuSpan::Nanos(250_000),
        sh_upload: PlaybackGpuSpan::Absent,
    }
}

#[test]
fn playback_record_patches_gpu_rows_by_frame_index_and_counts_the_missing() {
    let file = pose_file(100.0, 3);
    let mut record = PlaybackRecord::new(&args(1), &file);
    record.set_timing(true, GpuTimingSupport::Enabled);
    record.push_row(plan(RowKind::Hold, 0), None, UNMARKED, no_fill());
    record.push_row(plan(RowKind::NoDraw, 0), None, UNMARKED, no_fill());
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    record.push_row(plan(RowKind::Route, 1), None, UNMARKED, no_fill());

    // Readbacks land out of order; a no-draw frame has no frame encoder, so
    // a sample naming it is counted apart, never placed.
    record.patch_gpu(sample(2, PlaybackGpuSpan::Malformed));
    record.patch_gpu(sample(0, PlaybackGpuSpan::Nanos(4_000_000)));
    record.patch_gpu(sample(1, PlaybackGpuSpan::Nanos(1)));
    record.patch_gpu(sample(9, PlaybackGpuSpan::Nanos(1)));
    record.finish_gpu(1, 0);

    let rows = record.rows();
    let hold = rows[0].gpu.expect("frame 0 landed");
    assert_eq!(hold.frame_us, Some(4_000.0));
    assert_eq!(hold.lightmap_drain_us, Some(250.0));
    assert_eq!(
        hold.sh_upload_us, None,
        "a drain that did not run is absent"
    );
    assert!(rows[1].gpu.is_none());
    let malformed = rows[2].gpu.expect("frame 2 landed");
    assert_eq!(
        malformed.frame_us, None,
        "malformed reads absent, never zero"
    );
    assert!(rows[3].gpu.is_none(), "frame 3's readback never returned");

    let header = record.header_json();
    let gpu = &header["gpu_timing"];
    assert_eq!(gpu["state"], "enabled");
    assert_eq!(
        gpu["rows_missing"], 1,
        "route row 3; the no-draw row expects none"
    );
    assert_eq!(gpu["malformed_spans"], 1);
    assert_eq!(gpu["unmatched_samples"], 2);
    assert_eq!(gpu["readbacks_outstanding"], 1);
    let json = serde_json::to_value(&record).unwrap();
    assert!(json["rows"][3]["gpu"].is_null());
    assert_eq!(json["rows"][0]["gpu"]["frame_us"], 4_000.0);
}

#[test]
fn playback_record_counts_no_missing_rows_without_gpu_timing() {
    let mut record = PlaybackRecord::new(&args(0), &pose_file(100.0, 1));
    record.set_timing(true, GpuTimingSupport::NotRequested);
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    record.finish_gpu(0, 0);
    let header = record.header_json();
    assert_eq!(header["gpu_timing"]["state"], "disabled");
    assert!(header["gpu_timing"]["rows_missing"].is_null());
}

#[test]
fn playback_record_fills_its_rows_without_allocating() {
    // AC 29: a row with its CPU split, drain columns and GPU patch.
    let file = pose_file(100.0, 500);
    let mut record = PlaybackRecord::new(&args(600), &file);
    record.set_timing(true, GpuTimingSupport::Enabled);
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    committed_frame(&mut timer, 1);
    let drain_admission = DrainAdmission {
        sh: admission(1, 1_024),
        lightmap: admission(2, 8_192),
    };
    let counters = PlaybackDrainCounters::default();

    let control = AllocSnapshot::arm();
    std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "probe must count in this binary"
    );

    let probe = AllocSnapshot::arm();
    for frame in 0..1_100u64 {
        let (kind, pose) = if frame < 600 {
            (RowKind::Hold, 0)
        } else {
            (RowKind::Route, frame as usize - 600)
        };
        let drain = RowDrain::new(drain_admission, counters, counters, (Some(1), Some(2)));
        let fill = RowFill::new(timer.committed_record(), drain);
        record.push_row(plan(kind, pose), Some(16_000), UNMARKED, fill);
        record.patch_gpu(sample(frame, PlaybackGpuSpan::Nanos(5_000_000)));
    }
    assert_eq!(probe.allocs_since(), 0);
    assert_eq!(record.rows().len(), 1_100);
    assert!(record.rows()[1_099].fill.cpu_ns.is_some());
}

#[test]
fn playback_record_writes_once_to_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.playback.json");
    let mut record = PlaybackRecord::new(&args(0), &pose_file(90.0, 1));
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    record.write(&path).unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written["rows"].as_array().unwrap().len(), 1);
    let missing_dir = dir.path().join("absent").join("run.json");
    assert!(record.write(&missing_dir).unwrap_err().contains("absent"));
    // The launch check identifies a written record by its opening header
    // schema: the next run may overwrite it.
    assert_eq!(
        crate::playback::args::check_report_path(
            std::path::Path::new("run.playback.json"),
            std::path::Path::new("cp.txt"),
            dir.path(),
        ),
        Ok(())
    );
}

#[test]
fn playback_record_carries_marks_settle_outcome_and_life_states() {
    let file = pose_file(100.0, 3);
    let mut record = PlaybackRecord::new(&args(0), &file);
    record.set_settle(true);
    let mut changes = ChangeSet::EMPTY;
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    let key = LifeKey {
        pawn: 7,
        teleport_generation: 1,
        life_state: 2,
        life_state_entry: 5,
        table_revision: 1,
    };
    record.push_life_state(
        Some(key),
        Some("dead".into()),
        Some(LifeKey {
            life_state: 0,
            ..key
        }),
    );
    changes.insert(crate::playback::marks::ChangeKind::Extents);
    record.push_row(
        plan(RowKind::NoDraw, 1),
        Some(9),
        (changes, true),
        no_fill(),
    );
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

#[test]
fn playback_record_caps_its_launch_reservation_whatever_the_pose_count() {
    // A pose file planning more rows than the cap reserves the cap, never
    // gigabytes; growth past it is gap-marked. Checked on the reservation
    // size, without allocating it.
    let mut file = pose_file(100.0, 5_000);
    file.header.cadence_hz = 1;
    let mut wall = args(crate::playback::args::MAX_HOLD_FRAMES);
    wall.pacing = Pacing::WallClock;
    // 5,000 poses at 1 Hz is 5,000 s: 1,200,001 rows planned at 240 Hz.
    assert!(planned_route_rows(&wall, &file) > MAX_RESERVED_ROWS);
    assert_eq!(reserved_rows(&wall, &file), MAX_RESERVED_ROWS);
    let small = pose_file(100.0, 600);
    assert_eq!(reserved_rows(&args(10), &small), 10 + 600 + ROW_SLACK);
}

#[test]
fn playback_record_gap_marks_the_row_after_its_row_store_grows() {
    // A push past the reservation copies every row inside that frame's end;
    // the next row's interval spans the copy, so it is gap-marked, never
    // read as a stutter. The growing push's own row is not.
    let mut record = PlaybackRecord::new(&args(0), &pose_file(100.0, 1));
    record.rows = Vec::with_capacity(2);
    let mut expected = Vec::new();
    let mut previous_grew = false;
    for pose in 0..9 {
        let grows = record.rows.len() == record.rows.capacity();
        expected.push(previous_grew);
        previous_grew = grows;
        record.push_row(
            plan(RowKind::Route, pose),
            Some(16_000),
            UNMARKED,
            no_fill(),
        );
    }
    let gaps: Vec<bool> = record.rows().iter().map(|row| row.gap).collect();
    assert_eq!(gaps, expected, "only each row after a growing push");
    assert!(!gaps[2], "the growing push's own row precedes the copy");
    assert!(gaps[3], "row 2's push outgrew the two reserved rows");
    // A growing push whose own row the marks already gap still marks the
    // next row.
    let mut record = PlaybackRecord::new(&args(0), &pose_file(100.0, 1));
    record.rows = Vec::with_capacity(1);
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    record.push_row(
        plan(RowKind::Route, 1),
        Some(1),
        (ChangeSet::EMPTY, true),
        no_fill(),
    );
    record.push_row(plan(RowKind::Route, 2), Some(1), UNMARKED, no_fill());
    let gaps: Vec<bool> = record.rows().iter().map(|row| row.gap).collect();
    assert_eq!(gaps, [false, true, true]);
}

#[test]
fn playback_record_life_state_entries_name_the_row_actually_written() {
    let key = LifeKey {
        pawn: 3,
        teleport_generation: 0,
        life_state: 1,
        life_state_entry: 2,
        table_revision: 1,
    };
    let mut record = PlaybackRecord::new(&args(0), &pose_file(100.0, 3));
    record.push_row(plan(RowKind::Route, 0), None, UNMARKED, no_fill());
    // Seen on a frame that wrote no row: the next row written names it.
    record.push_life_state(Some(key), None, None);
    record.push_row(plan(RowKind::Route, 1), Some(9), UNMARKED, no_fill());
    // Seen as the run ended: no row follows, so it names none.
    record.push_life_state(None, None, Some(key));
    let json = serde_json::to_value(&record).unwrap();
    let life = json["life_states"].as_array().unwrap();
    assert_eq!(life.len(), 2);
    assert_eq!(life[0]["frame"], 1);
    assert!(
        life[1]["frame"].is_null(),
        "never a row the record lacks: {}",
        life[1]
    );
    assert_eq!(json["rows"].as_array().unwrap().len(), 2);
}
