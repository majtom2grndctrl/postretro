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

/// Opens every time stage of `S` nested under its parent, as the owning crate
/// does, so each substage's time stays inside its parent's. No allocation:
/// the guards live on the recursion's stack.
fn scope_subtree<S: StageSet>(frame: &postretro_stage_timing::StageFrame<S>, parent: Option<S>) {
    for &stage in S::ALL {
        if stage.parent() != parent {
            continue;
        }
        match stage.kind() {
            StageKind::Time => {
                let _scope = frame.scope(stage);
                scope_subtree(frame, Some(stage));
            }
            StageKind::Count => frame.add_count(stage, 3),
            StageKind::Marker => frame.mark(stage),
        }
    }
}

fn every_stage<S: StageSet>(gate: TimingGate) -> postretro_stage_timing::StageFrame<S> {
    let frame = postretro_stage_timing::StageFrame::<S>::new(gate);
    scope_subtree(&frame, None);
    frame
}

/// Drives every operation the timer performs per frame: open, every stage
/// scope of every set the engine folds (each built inside its anchor's scope),
/// per-tick absorb, nested placement by anchor, wait attribution, commit and
/// window close.
fn drive_timer_frames(timer: &mut CpuFrameTimer, frames: u32) {
    use postretro_renderer::cpu_stages::RenderStage;
    use postretro_sim::sim::cpu_stages::SimStage;
    use postretro_visibility::VisibilityStage;

    let gate = timer.gate();
    for _ in 0..frames {
        timer.begin_frame(Instant::now());
        let stages = timer.stages();
        for &stage in FrameStage::ALL {
            let scope = (stage != FrameStage::Ticks).then(|| stages.scope(stage));
            let anchor = Some(stage.label());
            match stage {
                FrameStage::Ticks => stages.add_count(stage, 2),
                FrameStage::FixedStep => {
                    let sim = postretro_stage_timing::StageFrame::<SimStage>::new(gate);
                    for _ in 0..2 {
                        sim.absorb(&every_stage::<SimStage>(gate));
                    }
                    let prediction = every_stage::<super::PredictionStage>(gate);
                    let nested = timer.nested_mut();
                    nested.extend_from(&sim, anchor);
                    nested.extend_from(&prediction, anchor);
                }
                FrameStage::Visibility => {
                    let visibility = every_stage::<VisibilityStage>(gate);
                    timer.nested_mut().extend_from(&visibility, anchor);
                }
                FrameStage::RenderPrep => {
                    let streaming = every_stage::<super::StreamingStage>(gate);
                    timer.nested_mut().extend_from(&streaming, anchor);
                    let particles = every_stage::<super::ParticleStage>(gate);
                    timer.nested_mut().extend_from(&particles, anchor);
                }
                FrameStage::Render => {
                    let render = every_stage::<RenderStage>(gate);
                    timer.nested_mut().extend_from(&render, anchor);
                    timer.add_wait_within(FrameStage::Render, WaitSource::Acquire, 0);
                    timer.add_wait_within(FrameStage::Render, WaitSource::Present, 0);
                }
                _ => {}
            }
            drop(scope);
        }
        drop(stages);
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

/// A stage set that exists only in this test. Nothing in the binary names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeStage {
    Outer,
    Inner,
}

impl StageSet for ProbeStage {
    const ALL: &'static [Self] = &[Self::Outer, Self::Inner];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Outer => "probe_outer",
            Self::Inner => "probe_inner",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::Outer => None,
            Self::Inner => Some(Self::Outer),
        }
    }
}

#[test]
fn test_only_stage_set_reaches_window_log_line_and_capture_report() {
    use postretro_stage_timing::StageFrame;

    let capture = LogCapture::start();
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES {
        let start = Instant::now();
        timer.begin_frame(start);
        timer.stages().add_nanos(FrameStage::Render, 3 * MS);
        let probe = StageFrame::<ProbeStage>::new(timer.gate());
        probe.add_nanos(ProbeStage::Outer, 2 * MS);
        probe.add_nanos(ProbeStage::Inner, MS);
        timer
            .nested_mut()
            .extend_from(&probe, Some(FrameStage::Render.label()));
        timer.finish_frame(start + Duration::from_nanos(4 * MS));
    }

    let window = timer.last_window().expect("window closed");
    assert_eq!(window.row("probe_outer").unwrap().parent, Some("render"));
    assert_eq!(
        window.row("probe_inner").unwrap().parent,
        Some("probe_outer")
    );
    capture.assert_logged_once(log::Level::Info, "probe_inner=1.000/1.000ms");

    let report = serde_json::to_value(super::capture_stages_report(
        TimingGate::ON,
        std::slice::from_ref(window),
        0,
    ))
    .unwrap();
    let labels: Vec<_> = report["windows"][0]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stage| stage["label"].as_str().unwrap().to_string())
        .collect();
    assert!(labels.contains(&"probe_outer".to_string()));
    assert!(labels.contains(&"probe_inner".to_string()));
}

#[test]
fn shared_timing_crate_source_names_no_engine_stage() {
    // Drift guard derived from every stage set the binary folds.
    fn labels<S: StageSet>() -> impl Iterator<Item = &'static str> {
        S::ALL.iter().map(|stage| stage.label())
    }
    let stage_labels: Vec<&str> = labels::<FrameStage>()
        .chain(labels::<super::PredictionStage>())
        .chain(labels::<super::StreamingStage>())
        .chain(labels::<super::ParticleStage>())
        .chain(labels::<postretro_sim::sim::cpu_stages::SimStage>())
        .chain(labels::<postretro_visibility::VisibilityStage>())
        .chain(labels::<postretro_renderer::cpu_stages::RenderStage>())
        .chain([
            derived::TOTAL,
            derived::WORK,
            derived::WAIT,
            derived::UNATTRIBUTED,
            derived::WAIT_ACQUIRE,
            derived::WAIT_PRESENT,
        ])
        .collect();
    let sources = [
        include_str!("../../../stage-timing/src/lib.rs"),
        include_str!("../../../stage-timing/src/frame.rs"),
        include_str!("../../../stage-timing/src/record.rs"),
        include_str!("../../../stage-timing/src/window.rs"),
    ];
    for label in stage_labels {
        let quoted = format!("\"{label}\"");
        for source in sources {
            // Shipped code only: the leaf's own test fixtures may use any label.
            let shipped = source.split("#[cfg(test)]").next().unwrap_or(source);
            assert!(!shipped.contains(&quoted), "leaf crate names stage {label}");
        }
    }
}

#[test]
fn every_folded_stage_label_is_unique() {
    // Rows key by label: two sets sharing one would silently sum.
    fn labels<S: StageSet>() -> impl Iterator<Item = &'static str> {
        S::ALL.iter().map(|stage| stage.label())
    }
    let all: Vec<&str> = labels::<FrameStage>()
        .chain(labels::<super::PredictionStage>())
        .chain(labels::<super::StreamingStage>())
        .chain(labels::<super::ParticleStage>())
        .chain(labels::<postretro_sim::sim::cpu_stages::SimStage>())
        .chain(labels::<postretro_visibility::VisibilityStage>())
        .chain(labels::<postretro_renderer::cpu_stages::RenderStage>())
        .chain([
            derived::TOTAL,
            derived::WORK,
            derived::WAIT,
            derived::UNATTRIBUTED,
            derived::WAIT_ACQUIRE,
            derived::WAIT_PRESENT,
        ])
        .collect();
    let mut seen = std::collections::HashSet::new();
    for label in &all {
        assert!(seen.insert(*label), "stage label {label} is used twice");
    }
    assert!(all.len() <= postretro_stage_timing::MAX_FRAME_SAMPLES);
}

#[test]
fn totals_are_marked_as_aggregates_and_excluded_from_the_stage_sum() {
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    counted_frame(&mut timer, &[(FrameStage::Render, MS)], 3 * MS);
    let record = timer.last_record();
    let aggregate = |label: &str| {
        record
            .samples()
            .iter()
            .find(|sample| sample.label == label)
            .unwrap()
            .aggregate
    };
    assert!(aggregate(derived::TOTAL));
    assert!(aggregate(derived::WORK));
    assert!(!aggregate(derived::WAIT));
    assert!(!aggregate(derived::UNATTRIBUTED));
    assert!(!aggregate("render"));
    // Stages, wait and unattributed partition the total.
    assert_eq!(
        record.top_level_time(),
        record.value(derived::TOTAL).unwrap()
    );
}

// AC 23: the lightmap residency stages reach the `[CpuTiming]` line under
// `render_prep`, the controller's work apart from the renderer drain.
#[test]
fn lightmap_residency_stages_sit_under_render_prep_in_the_log_line() {
    use postretro_stage_timing::StageFrame;

    use super::StreamingStage;

    // Drift guard: a new variant must be placed here and given a label.
    for &stage in StreamingStage::ALL {
        let expected = match stage {
            StreamingStage::LightmapResidency => "lightmap_residency",
            StreamingStage::LightmapDrain => "lightmap_drain",
        };
        assert_eq!(stage.label(), expected);
        assert_eq!(stage.parent(), None, "{expected} is a root");
        assert_eq!(stage.kind(), StageKind::Time);
    }

    let capture = LogCapture::start();
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES {
        let start = Instant::now();
        timer.begin_frame(start);
        timer.stages().add_nanos(FrameStage::RenderPrep, 3 * MS);
        let streaming = StageFrame::<StreamingStage>::new(timer.gate());
        streaming.add_nanos(StreamingStage::LightmapResidency, MS);
        streaming.add_nanos(StreamingStage::LightmapDrain, 2 * MS);
        timer
            .nested_mut()
            .extend_from(&streaming, Some(FrameStage::RenderPrep.label()));
        timer.finish_frame(start + Duration::from_nanos(4 * MS));
    }
    let window = timer.last_window().expect("window closed");
    for label in ["lightmap_residency", "lightmap_drain"] {
        assert_eq!(window.row(label).unwrap().parent, Some("render_prep"));
    }
    capture.assert_logged_once(log::Level::Info, "lightmap_residency=1.000/1.000ms");
    capture.assert_logged_once(log::Level::Info, "lightmap_drain=2.000/2.000ms");
}

// The particle path (emitter bridge + particle sim) reaches the `[CpuTiming]`
// line under `render_prep`, so its cost is no longer unattributed.
#[test]
fn particle_stages_sit_under_render_prep_in_the_log_line() {
    use postretro_stage_timing::StageFrame;

    use super::ParticleStage;

    // Drift guard: a new variant must be placed here and given a label.
    for &stage in ParticleStage::ALL {
        let expected = match stage {
            ParticleStage::Emit => "particle_emit",
            ParticleStage::Sim => "particle_sim",
            ParticleStage::Collect => "particle_collect",
        };
        assert_eq!(stage.label(), expected);
        assert_eq!(stage.parent(), None, "{expected} is a root");
        assert_eq!(stage.kind(), StageKind::Time);
    }

    let capture = LogCapture::start();
    let mut timer = CpuFrameTimer::new(TimingGate::ON);
    for _ in 0..WINDOW_FRAMES {
        let start = Instant::now();
        timer.begin_frame(start);
        timer.stages().add_nanos(FrameStage::RenderPrep, 3 * MS);
        let particles = StageFrame::<ParticleStage>::new(timer.gate());
        particles.add_nanos(ParticleStage::Emit, MS);
        particles.add_nanos(ParticleStage::Sim, 2 * MS);
        timer
            .nested_mut()
            .extend_from(&particles, Some(FrameStage::RenderPrep.label()));
        timer.finish_frame(start + Duration::from_nanos(4 * MS));
    }
    let window = timer.last_window().expect("window closed");
    for label in ["particle_emit", "particle_sim"] {
        assert_eq!(window.row(label).unwrap().parent, Some("render_prep"));
    }
    capture.assert_logged_once(log::Level::Info, "particle_emit=1.000/1.000ms");
    capture.assert_logged_once(log::Level::Info, "particle_sim=2.000/2.000ms");
}
