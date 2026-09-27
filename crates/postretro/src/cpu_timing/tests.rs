// Frame accounting and allocation proofs for the binary's CPU frame timer.
// See: context/lib/rendering_pipeline.md §12

use std::time::{Duration, Instant};

use postretro_stage_timing::{StageKind, StageSet, WINDOW_FRAMES};
use postretro_test_log_capture::LogCapture;

use super::{CpuFrameTimer, FrameStage, TimingGate, WaitSource, derived};
use crate::alloc_probe::AllocSnapshot;

const MS: u64 = 1_000_000;

/// Runs one counted frame: `stages` in nanoseconds, then commit at `total`.
fn counted_frame(timer: &mut CpuFrameTimer, stages: &[(FrameStage, u64)], total: u64) -> bool {
    let start = Instant::now();
    timer.begin_frame(start);
    let handle = timer.stages();
    for &(stage, nanos) in stages {
        handle.add_nanos(stage, nanos);
    }
    handle.add_count(FrameStage::Ticks, 1);
    timer.commit_frame(start + Duration::from_nanos(total))
}

fn value(timer: &CpuFrameTimer, label: &str) -> Option<u64> {
    timer.last_record().value(label)
}

#[test]
fn unattributed_is_exactly_total_minus_top_level_stages_minus_wait() {
    // P-wait: acquire and present block inside the render call.
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    let start = Instant::now();
    timer.begin_frame(start);
    let stages = timer.stages();
    stages.add_nanos(FrameStage::Input, 400);
    stages.add_nanos(FrameStage::Render, 3_000);
    stages.add_count(FrameStage::Ticks, 0);
    timer.add_wait_within(FrameStage::Render, WaitSource::Acquire, 1_200);
    timer.add_wait_within(FrameStage::Render, WaitSource::Present, 300);
    timer.commit_frame(start + Duration::from_nanos(5_000));

    assert_eq!(value(&timer, "render"), Some(1_500), "block leaves render");
    assert_eq!(
        value(&timer, derived::WAIT),
        Some(1_500),
        "block counted once"
    );
    assert_eq!(value(&timer, derived::TOTAL), Some(5_000));
    assert_eq!(value(&timer, derived::WORK), Some(3_500));
    assert_eq!(
        value(&timer, derived::UNATTRIBUTED),
        Some(5_000 - 400 - 1_500 - 1_500)
    );
    assert_eq!(value(&timer, derived::WAIT_ACQUIRE), Some(1_200));
    assert_eq!(value(&timer, derived::WAIT_PRESENT), Some(300));
}

#[test]
fn zero_tick_frame_records_zero_ticks_and_still_counts() {
    // P-zero-tick
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    let start = Instant::now();
    timer.begin_frame(start);
    timer.stages().add_count(FrameStage::Ticks, 0);
    timer.commit_frame(start + Duration::from_nanos(MS));
    let ticks = timer
        .last_record()
        .samples()
        .iter()
        .find(|sample| sample.label == "ticks")
        .copied()
        .expect("tick count present on a zero-tick frame");
    assert_eq!((ticks.kind, ticks.value), (StageKind::Count, 0));
    assert_eq!(timer.partial_frames(), 1);
}

#[test]
fn only_committed_in_level_frames_advance_the_window() {
    // P-close, P-count, P-early-fallback, P-surface-skip.
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for index in 1..=WINDOW_FRAMES {
        // A frontend or early-returned frame: opened, staged, never committed.
        timer.begin_frame(Instant::now());
        timer.stages().add_nanos(FrameStage::Visibility, 50 * MS);
        timer
            .nested_mut()
            .push_marker("portal_fallback", Some("visibility"));

        // A frame whose acquire yielded no surface.
        let start = Instant::now();
        timer.begin_frame(start);
        timer.stages().add_nanos(FrameStage::Render, 40 * MS);
        timer.add_wait_within(FrameStage::Render, WaitSource::Acquire, 30 * MS);
        timer.exclude_frame();
        assert!(!timer.commit_frame(start + Duration::from_nanos(60 * MS)));

        let closed = counted_frame(&mut timer, &[(FrameStage::Render, MS)], 2 * MS);
        assert_eq!(closed, index == WINDOW_FRAMES, "frame {index}");
    }
    let window = timer.last_window().expect("window one closed");
    assert_eq!(window.frames, WINDOW_FRAMES);
    assert_eq!(window.row("render").unwrap().max, MS);
    assert_eq!(window.row(derived::WAIT).unwrap().max, 0);
    assert!(window.row("portal_fallback").is_none());
    assert!(window.row("visibility").is_none());

    counted_frame(&mut timer, &[(FrameStage::Render, 7 * MS)], 8 * MS);
    assert_eq!(timer.partial_frames(), 1, "frame 121 opens window two");
    assert_eq!(timer.last_window().unwrap().row("render").unwrap().max, MS);
}

#[test]
fn vsync_toggle_discards_the_partial_window_only() {
    // P-reset (vsync toggle between redraws).
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES {
        counted_frame(&mut timer, &[(FrameStage::Render, MS)], 2 * MS);
    }
    for _ in 0..40 {
        counted_frame(&mut timer, &[(FrameStage::Render, 9 * MS)], 10 * MS);
    }
    timer.discard_partial();
    assert_eq!(timer.partial_frames(), 0);
    assert!(
        timer.last_window().is_some(),
        "last closed window stays visible"
    );
    for _ in 0..WINDOW_FRAMES {
        counted_frame(&mut timer, &[(FrameStage::Render, 3 * MS)], 4 * MS);
    }
    assert_eq!(
        timer.last_window().unwrap().row("render").unwrap().max,
        3 * MS
    );
}

#[test]
fn level_change_clears_every_window_and_skips_the_install_frame() {
    // P-reset (install/unload), and no surface shows the previous level.
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES + 10 {
        counted_frame(&mut timer, &[(FrameStage::Render, MS)], 2 * MS);
    }
    let start = Instant::now();
    timer.begin_frame(start);
    timer.stages().add_nanos(FrameStage::Housekeeping, 500 * MS);
    timer.level_changed();
    assert!(!timer.commit_frame(start + Duration::from_nanos(600 * MS)));
    assert!(timer.last_window().is_none());
    assert_eq!(timer.partial_frames(), 0);

    for _ in 0..WINDOW_FRAMES {
        counted_frame(&mut timer, &[(FrameStage::Render, 2 * MS)], 3 * MS);
    }
    let window = timer.last_window().unwrap();
    assert!(
        window.row("housekeeping").is_none(),
        "install frame not counted"
    );
    assert_eq!(window.row("render").unwrap().max, 2 * MS);
}

#[test]
fn reload_commit_frame_is_discarded_with_the_partial_window() {
    // P-reload: the commit lands after present, before the frame-end fold.
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..30 {
        counted_frame(&mut timer, &[(FrameStage::Render, MS)], 2 * MS);
    }
    let start = Instant::now();
    timer.begin_frame(start);
    timer.stages().add_nanos(FrameStage::FrameTail, 90 * MS);
    timer.discard_partial();
    timer.exclude_frame();
    assert!(!timer.commit_frame(start + Duration::from_nanos(100 * MS)));
    assert_eq!(timer.partial_frames(), 0);
    counted_frame(&mut timer, &[(FrameStage::Render, MS)], 2 * MS);
    assert_eq!(timer.partial_frames(), 1);
}

#[test]
fn log_line_appears_once_per_closed_window_and_never_when_off() {
    let capture = LogCapture::start();
    let mut off = CpuFrameTimer::new(TimingGate::OFF);
    for _ in 0..3 * WINDOW_FRAMES {
        off.begin_frame(Instant::now());
        drop(off.stages().scope(FrameStage::Render));
        off.finish_frame(Instant::now());
    }
    capture.assert_not_logged(log::Level::Info, "[CpuTiming]");

    let mut on = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..2 * WINDOW_FRAMES + 30 {
        on.begin_frame(Instant::now());
        drop(on.stages().scope(FrameStage::Render));
        on.stages().add_count(FrameStage::Ticks, 1);
        on.finish_frame(Instant::now());
    }
    let lines = capture
        .records()
        .into_iter()
        .filter(|record| record.message.contains("[CpuTiming]"))
        .count();
    assert_eq!(lines, 2);
    capture.assert_logged(log::Level::Info, "[CpuTiming] frames=120 total=");
}

#[test]
fn absent_stage_is_missing_from_the_log_line_not_zero() {
    let capture = LogCapture::start();
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES {
        timer.begin_frame(Instant::now());
        drop(timer.stages().scope(FrameStage::Render));
        timer.finish_frame(Instant::now());
    }
    capture.assert_logged_once(log::Level::Info, "render=");
    capture.assert_not_logged(log::Level::Info, "snapshot_apply=");
}

/// Drives every operation the timer performs per frame: open, every stage
/// scope, a nested sample, wait attribution, commit.
fn drive_timer_frames(timer: &mut CpuFrameTimer, frames: u32) {
    for _ in 0..frames {
        timer.begin_frame(Instant::now());
        let stages = timer.stages();
        for &stage in FrameStage::ALL {
            if stage == FrameStage::Ticks {
                stages.add_count(stage, 2);
            } else {
                drop(stages.scope(stage));
            }
        }
        drop(stages);
        timer
            .nested_mut()
            .push_time("nested_probe", Some("render"), 0);
        timer.add_wait_within(FrameStage::Render, WaitSource::Present, 0);
        timer.commit_frame(Instant::now());
    }
}

#[test]
fn timing_off_frame_allocates_nothing_and_accumulates_no_window() {
    let mut timer = CpuFrameTimer::new(TimingGate::OFF);

    let control = AllocSnapshot::arm();
    drive_timer_frames(&mut timer, 1);
    std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "probe must count in this binary"
    );

    let probe = AllocSnapshot::arm();
    drive_timer_frames(&mut timer, 2 * WINDOW_FRAMES + 5);
    assert_eq!(probe.allocs_since(), 0);
    assert!(timer.last_window().is_none());
    assert_eq!(timer.partial_frames(), 0);
}

#[test]
fn timing_on_steady_state_allocates_nothing_across_window_closes() {
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    drive_timer_frames(&mut timer, WINDOW_FRAMES);
    assert!(timer.last_window().is_some(), "first window closed");

    let control = AllocSnapshot::arm();
    drive_timer_frames(&mut timer, 1);
    std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "probe must count in this binary"
    );

    let probe = AllocSnapshot::arm();
    drive_timer_frames(&mut timer, 2 * WINDOW_FRAMES);
    assert_eq!(probe.allocs_since(), 0);
    assert_eq!(timer.partial_frames(), 1, "two further windows closed");
}

/// One frame of the fixed-step fold as the redraw path runs it: each tick's
/// sim stage frame summed into a frame-level one, nested under `fixed_step`.
fn fixed_step_frame(timer: &mut CpuFrameTimer, tick_movement_nanos: &[u64]) {
    use postretro_sim::sim::cpu_stages::SimStage;
    use postretro_stage_timing::StageFrame;

    let start = Instant::now();
    timer.begin_frame(start);
    let sim_cpu = StageFrame::<SimStage>::new(timer.gate());
    let mut fixed_step = 0;
    for &movement in tick_movement_nanos {
        let tick = StageFrame::<SimStage>::new(timer.gate());
        tick.add_nanos(SimStage::Tick, movement + 100);
        tick.add_nanos(SimStage::Movement, movement);
        sim_cpu.absorb(&tick);
        fixed_step += movement + 150;
    }
    let stages = timer.stages();
    stages.add_nanos(FrameStage::FixedStep, fixed_step);
    stages.add_count(FrameStage::Ticks, tick_movement_nanos.len() as u64);
    drop(stages);
    timer
        .nested_mut()
        .extend_from(&sim_cpu, Some(FrameStage::FixedStep.label()));
    timer.commit_frame(start + Duration::from_nanos(fixed_step + 1_000));
}

#[test]
fn multi_tick_frame_sums_sim_substages_and_reports_the_tick_count() {
    // P-many-tick
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    fixed_step_frame(&mut timer, &[1_000, 2_000, 4_000]);
    let record = timer.last_record();
    assert_eq!(record.value("sim_movement"), Some(7_000));
    assert_eq!(record.value("sim_tick"), Some(7_300));
    assert_eq!(record.value("ticks"), Some(3));
    let movement = record
        .samples()
        .iter()
        .find(|sample| sample.label == "sim_movement")
        .unwrap();
    assert_eq!(movement.parent, Some("sim_tick"));
    let tick = record
        .samples()
        .iter()
        .find(|sample| sample.label == "sim_tick")
        .unwrap();
    assert_eq!(tick.parent, Some("fixed_step"));
    assert_eq!(record.substage_overruns().count(), 0);
}

#[test]
fn zero_tick_frame_has_no_sim_rows_and_sim_averages_skip_it() {
    // P-zero-tick
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    fixed_step_frame(&mut timer, &[]);
    assert_eq!(timer.last_record().value("sim_tick"), None);
    assert_eq!(timer.last_record().value("ticks"), Some(0));

    for index in 1..WINDOW_FRAMES {
        let ticks: &[u64] = if index % 2 == 0 { &[] } else { &[3_000] };
        fixed_step_frame(&mut timer, ticks);
    }
    let window = timer.last_window().expect("window closed");
    let movement = window.row("sim_movement").unwrap();
    assert_eq!(movement.frames, WINDOW_FRAMES / 2);
    assert!((movement.average - 3_000.0).abs() < 1e-9);
    assert_eq!(window.row("ticks").unwrap().frames, WINDOW_FRAMES);
}
