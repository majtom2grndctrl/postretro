//! Measurement harness: streamed lightmap residency walks over a real PRL.
//! See: context/lib/experimental_spikes.md · context/lib/rendering_pipeline.md §4

// Drives the real runtime path over the dry run's seeded camera walks: the
// lightmap session inside `LevelStreaming`, whose issuer reads real block
// pairs through the level's manifest. The real `LightmapPoolModel` plans each
// drain (see `pool_mirror`) without GPU work. Visibility is the runtime
// portal walk from a jittered eye in each step's cell. SH does not stream.
//
// Time model: the camera dwells in each walk cell for the frames it takes to
// reach the next cell's centre at `RUN_SPEED` m/s at 60 Hz, turning 90° per
// frame. A frame with reads in flight sleeps to its 16.7 ms deadline, so disk
// reads race real time. Paths and eyes are seeded; misses depend on latency.
//
// Run from the workspace root (debug is fine; the heavy part is the walk):
//
//   POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/campaign-test.prl \
//     cargo test -p postretro --bin postretro lightmap_residency_walks_from_prl \
//     -- --ignored --nocapture
//
// Optional: `POSTRETRO_LIGHTMAP_WALK_STEPS` (default 2000),
// `POSTRETRO_LIGHTMAP_WALK_RUNS` = `default` | `all` (default `all`: the
// default levers plus the cap and lead sweeps). Leave
// `POSTRETRO_LIGHTMAP_STREAMING` unset or `stream`.

mod paths;
mod pool_mirror;

use std::ops::Range;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use postretro_level_format::SectionId;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{LevelWorld, PrlReadCounters};
use postretro_stage_timing::StageFrame;
use postretro_visibility::{TimingGate, VisibilityPath, VisibleCells, determine_visible_cells};

use super::{LightmapLevelView, LightmapStreamingSession};
use crate::cpu_timing::StreamingStage;
use crate::lightmap_streaming::levers::LEAD_UNITS_PER_METRE;
use crate::session::level_streaming::{LevelStreaming, StreamingFrame};
use crate::streaming::cluster_hints::decode_level_hints;
use paths::{SplitMix64, WALK_SEED, WalkKind, camera_adjacency, walk_path};
use pool_mirror::{PoolMirror, PoolStats};

const PRL_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_PRL";
const STEPS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_STEPS";
const RUNS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_RUNS";
const DEFAULT_STEPS: usize = 2_000;
const FRAME_SECONDS: f64 = 1.0 / 60.0;
/// A brisk boomer-shooter run speed, metres per second.
const RUN_SPEED: f32 = 8.0;
const MAX_DWELL_FRAMES: u32 = 60;
/// Default camera: `camera.rs` HFOV at 16:9.
const HFOV_DEGREES: f32 = 100.0;
const ASPECT: f32 = 16.0 / 9.0;
const EYE_TRIES: usize = 32;
const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, Copy)]
struct Levers {
    lead_metres: f32,
    cap_layers: u32,
}

/// One walk, resolved to cell ids, with its teleports marked and each step's
/// dwell in frames.
struct Walk {
    kind: WalkKind,
    cells: Vec<u32>,
    /// Step `i` arrived by teleport: not portal-adjacent to step `i - 1`.
    teleport: Vec<bool>,
    dwell: Vec<u32>,
    travel_yaw: Vec<f32>,
}

impl Walk {
    fn build(world: &LevelWorld, kind: WalkKind, steps: usize) -> Self {
        let camera_cells: Vec<u32> = (0..world.cells.len() as u32)
            .filter(|&cell| {
                let data = &world.cells[cell as usize];
                !data.is_solid && !data.is_exterior
            })
            .collect();
        let adjacency = camera_adjacency(
            world.cells.len(),
            world.portals.iter().map(|p| (p.front_cell, p.back_cell)),
            &camera_cells,
        );
        let (path, _) = walk_path(kind, &adjacency, steps, WALK_SEED);
        let teleport = (0..path.len())
            .map(|i| {
                i > 0
                    && path[i] != path[i - 1]
                    && adjacency[path[i - 1] as usize]
                        .binary_search(&path[i])
                        .is_err()
            })
            .collect();
        let cells: Vec<u32> = path.iter().map(|&i| camera_cells[i as usize]).collect();
        let centre = |cell: u32| {
            let data = &world.cells[cell as usize];
            (data.bounds_min + data.bounds_max) * 0.5
        };
        let mut dwell = Vec::with_capacity(cells.len());
        let mut travel_yaw = Vec::with_capacity(cells.len());
        let mut yaw = 0.0f32;
        for i in 0..cells.len() {
            let Some(&next) = cells.get(i + 1) else {
                dwell.push(1);
                travel_yaw.push(yaw);
                continue;
            };
            let delta = centre(next) - centre(cells[i]);
            let frames = (delta.length() / (RUN_SPEED * FRAME_SECONDS as f32)).ceil() as u32;
            dwell.push(frames.clamp(1, MAX_DWELL_FRAMES));
            if delta.x != 0.0 || delta.z != 0.0 {
                yaw = (-delta.x).atan2(-delta.z);
            }
            travel_yaw.push(yaw);
        }
        Self {
            kind,
            cells,
            teleport,
            dwell,
            travel_yaw,
        }
    }

    fn teleports(&self) -> usize {
        self.teleport.iter().filter(|&&t| t).count()
    }

    fn frames(&self) -> u64 {
        self.dwell.iter().map(|&d| u64::from(d)).sum()
    }
}

/// A seeded eye point inside `cell` (10–90% of its AABB per axis, off the
/// bake's sampling lattice), accepted only where the locator agrees; the
/// AABB centre after `EYE_TRIES` misses.
fn eye_point(world: &LevelWorld, cell: u32, rng: &mut SplitMix64) -> Vec3 {
    let data = &world.cells[cell as usize];
    let extent = data.bounds_max - data.bounds_min;
    for _ in 0..EYE_TRIES {
        let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.8 + Vec3::splat(0.1);
        let point = data.bounds_min + extent * fraction;
        if world.locate_cell(point) == cell as usize {
            return point;
        }
    }
    (data.bounds_min + data.bounds_max) * 0.5
}

fn view_projection(eye: Vec3, yaw: f32) -> Mat4 {
    let look = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, Vec3::Y);
    let vfov = 2.0 * ((HFOV_DEGREES.to_radians() / 2.0).tan() / ASPECT).atan();
    glam::camera::rh::proj::directx::perspective(vfov, ASPECT, 0.1, 4096.0) * view
}

/// Visible-miss block-frames, one bucket pair per step kind.
#[derive(Debug, Default, Clone, Copy)]
struct Misses {
    frames: u64,
    outside: u64,
    not_resident: u64,
    frames_outside: u64,
    frames_not_resident: u64,
    max_outside: u32,
    max_not_resident: u32,
    /// Of `outside`: drawn while not resident after the frame's drain, so
    /// sampled SH-only (the harness's own count, over the same drawn set).
    outside_missing: u64,
    frames_outside_missing: u64,
}

impl Misses {
    fn add(&mut self, outside: u32, not_resident: u32, outside_missing: u32) {
        self.frames += 1;
        self.outside += u64::from(outside);
        self.not_resident += u64::from(not_resident);
        self.frames_outside += u64::from(outside > 0);
        self.frames_not_resident += u64::from(not_resident > 0);
        self.max_outside = self.max_outside.max(outside);
        self.max_not_resident = self.max_not_resident.max(not_resident);
        self.outside_missing += u64::from(outside_missing);
        self.frames_outside_missing += u64::from(outside_missing > 0);
    }

    fn line(&self) -> String {
        format!(
            "{} frames: outside baked set {} block-frames in {} frames (max {}/frame), of which \
             drawn non-resident {} in {} frames; not resident {} block-frames in {} frames \
             (max {}/frame)",
            self.frames,
            self.outside,
            self.frames_outside,
            self.max_outside,
            self.outside_missing,
            self.frames_outside_missing,
            self.not_resident,
            self.frames_not_resident,
            self.max_not_resident,
        )
    }
}

#[derive(Debug, Default)]
struct RunReport {
    frames: u64,
    wall: Duration,
    paced_frames: u64,
    locate_mismatch_frames: u64,
    path_portal: u64,
    path_step_limit: u64,
    path_other: u64,
    preload_pairs: u32,
    preload_bytes: u64,
    preload_millis: f64,
    /// Per step end: (resident id 22, resident id 42, mandatory id 22,
    /// mandatory id 42) bytes.
    step_bytes: Vec<[u64; 4]>,
    /// Per step: id-22 and id-42 bytes read at the positional reader.
    step_reads: Vec<(u64, u64)>,
    entering_blocks: u64,
    entering_resident: u64,
    spawn: Misses,
    adjacent: Misses,
    /// Adjacent-step frames split: the frame the camera cell changed, and
    /// the rest.
    adjacent_change_frames: Misses,
    teleport: Misses,
    residency_nanos_change: Vec<u64>,
    residency_nanos_steady: Vec<u64>,
    /// Steady frames' wall time per part: `prepare_drains` (demand,
    /// completions, batch, reads), `apply_outcome`, `finish_frame`.
    steady_parts: [Vec<u64>; 3],
    /// Steady frames' `prepare_drains` time, split by whether the frame
    /// admitted delivered read bytes (completions), and the most admitted.
    prepare_with_completions: Vec<u64>,
    prepare_without_completions: Vec<u64>,
    max_admitted_bytes: u64,
    counters: Option<crate::lightmap_streaming::controller::LightmapResidencyCounters>,
    physical_reads: u64,
    pool: PoolStats,
    layer_bytes: u64,
    first_not_resident: Vec<(usize, u32, u32)>,
    first_outside: Vec<(usize, u32, u32)>,
}

fn section_reads(counters: &PrlReadCounters) -> (u64, u64) {
    (
        counters.section_bytes(SectionId::Lightmap as u32),
        counters.section_bytes(SectionId::ShadowmaskAtlas as u32),
    )
}

/// Each cell's blocks from the owning cell of every block in block order: a
/// contiguous run, as the loader validates, empty for a cell without charts.
fn cell_block_ranges(
    block_cells: impl IntoIterator<Item = u32>,
    cell_count: usize,
) -> Vec<Range<u32>> {
    let mut ranges = vec![0..0; cell_count];
    for (block, cell) in (0u32..).zip(block_cells) {
        let run = &mut ranges[cell as usize];
        if run.start == run.end {
            *run = block..block + 1;
        } else {
            assert_eq!(run.end, block, "cell {cell}'s blocks are contiguous");
            run.end += 1;
        }
    }
    ranges
}

/// Every block of the camera cell's mandatory cells at `lead` (fixed point),
/// sorted.
fn mandatory_blocks(
    residency_set: &CellResidencySetSection,
    cell_blocks: &[Range<u32>],
    camera_cell: u32,
    lead: u32,
    out: &mut Vec<u32>,
) {
    out.clear();
    out.extend(
        residency_set
            .entries_for(camera_cell as usize)
            .iter()
            .filter(|entry| entry.lead <= lead)
            .flat_map(|entry| {
                cell_blocks
                    .get(entry.cell_id as usize)
                    .cloned()
                    .unwrap_or(0..0)
            }),
    );
    out.sort_unstable();
    out.dedup();
}

fn run(world: &LevelWorld, walk: &Walk, levers: Levers) -> RunReport {
    let view = LightmapLevelView::of(world).expect("the PRL streams its lightmap");
    let read_counters = view.manifest.read_counters().clone();
    let cell_blocks = cell_block_ranges(
        view.manifest
            .lightmap_index()
            .records
            .iter()
            .map(|record| record.cell_id),
        world.cells.len(),
    );

    let hints = decode_level_hints(world.cluster_directory()).expect("id-49 hints");
    let mut session =
        LightmapStreamingSession::new(view, hints.as_deref()).expect("lightmap session");
    {
        let lever = session.controller.levers_mut();
        lever.set_pool_cap_layers(levers.cap_layers);
        lever.set_lead_metres(levers.lead_metres);
    }
    let lead = session.controller.levers().lead();
    let mut mirror = PoolMirror::new(view.manifest, levers.cap_layers);
    let mut level = LevelStreaming::default();
    level.install_lightmap(session);

    let mut report = RunReport {
        layer_bytes: mirror.layer_bytes(),
        ..RunReport::default()
    };
    let started = Instant::now();
    let mut rng = SplitMix64(WALK_SEED ^ 0xE7E5);

    // Level install: the spawn cell's mandatory set, read synchronously and
    // installed in one drain before the first frame.
    let spawn_eye = eye_point(world, walk.cells[0], &mut rng);
    let spawn_cell = world.locate_cell(spawn_eye) as u32;
    let before = section_reads(&read_counters);
    {
        let session = level.lightmap_mut().expect("installed");
        session.update_camera_set(view.residency_set, spawn_cell);
        let summary = session
            .preload(&[], |batch| Ok(mirror.drain(batch)))
            .expect("spawn preload");
        assert_eq!(summary.deferred, 0, "a fresh pool never defers a preload");
        assert!(session.settled(), "spawn set resident after preload");
        report.preload_pairs = summary.reads.pairs;
        report.preload_millis = summary.elapsed.as_secs_f64() * 1000.0;
    }
    let after = section_reads(&read_counters);
    report.preload_bytes = (after.0 - before.0) + (after.1 - before.1);
    mirror.begin_play();

    let mut no_sh = None;
    let mut scratch = Vec::new();
    let mut mandatory_now = Vec::new();
    let mut mandatory_prev = Vec::new();
    let mut previous_cell = spawn_cell;
    mandatory_blocks(
        view.residency_set,
        &cell_blocks,
        spawn_cell,
        lead,
        &mut mandatory_prev,
    );
    let mut seconds = 0.0f64;
    let frame_budget = Duration::from_secs_f64(FRAME_SECONDS);

    for (step, &cell) in walk.cells.iter().enumerate() {
        let step_reads_before = section_reads(&read_counters);
        for frame in 0..walk.dwell[step] {
            let frame_started = Instant::now();
            let eye = eye_point(world, cell, &mut rng);
            let yaw = walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2;
            let (visibility, _) = determine_visible_cells(
                eye,
                view_projection(eye, yaw),
                world,
                &[],
                false,
                &mut scratch,
                TimingGate::OFF,
            );
            let camera_cell = visibility.stats.camera_cell;
            report.locate_mismatch_frames += u64::from(camera_cell != cell);
            match visibility.stats.path {
                VisibilityPath::PrlPortal { .. } => report.path_portal += 1,
                VisibilityPath::PortalStepLimitFallback { .. } => report.path_step_limit += 1,
                _ => report.path_other += 1,
            }
            let cell_changed = camera_cell != previous_cell;
            if cell_changed {
                // Blocks joining M(c, L), and how many the pool already holds
                // before this frame's drain.
                mandatory_blocks(
                    view.residency_set,
                    &cell_blocks,
                    camera_cell,
                    lead,
                    &mut mandatory_now,
                );
                for &block in &mandatory_now {
                    if mandatory_prev.binary_search(&block).is_err() {
                        report.entering_blocks += 1;
                        report.entering_resident += u64::from(mirror.model().is_resident(block));
                    }
                }
                std::mem::swap(&mut mandatory_now, &mut mandatory_prev);
                previous_cell = camera_cell;
            }

            let cpu = StageFrame::<StreamingStage>::new(TimingGate::ON);
            let delivered = level
                .lightmap_mut()
                .expect("installed")
                .ledger()
                .in_memory_bytes();
            let part = Instant::now();
            level
                .prepare_drains(
                    &mut no_sh,
                    Some(view.residency_set),
                    StreamingFrame {
                        visible_cells: &visibility.visible_cells,
                        camera_cell: Some(camera_cell as usize),
                        path: visibility.stats.path,
                        monotonic_seconds: seconds,
                        cpu: &cpu,
                    },
                )
                .expect("prepare streaming drains");
            let prepare_nanos = elapsed_nanos(part);
            let session = level.lightmap_mut().expect("installed");
            let mut apply_nanos = 0;
            if let Some(batch) = session.take_drain_batch_for_renderer() {
                let outcome = {
                    let _scope = cpu.scope(StreamingStage::LightmapDrain);
                    mirror.drain(batch)
                };
                let part = Instant::now();
                let _scope = cpu.scope(StreamingStage::LightmapResidency);
                session.apply_outcome(outcome).expect("apply outcome");
                apply_nanos = elapsed_nanos(part);
            }
            let part = Instant::now();
            {
                let _scope = cpu.scope(StreamingStage::LightmapResidency);
                session.finish_frame(None, seconds);
            }
            let finish_nanos = elapsed_nanos(part);
            // Drawn blocks outside M(c, L) that are still not resident after
            // this frame's drain: the frame samples them SH-only.
            let mut outside_missing = 0u32;
            if let (VisibilityPath::PrlPortal { .. }, VisibleCells::Culled(cells)) =
                (visibility.stats.path, &visibility.visible_cells)
            {
                for &drawn in cells {
                    for block in cell_blocks[drawn as usize].clone() {
                        if mandatory_prev.binary_search(&block).is_err()
                            && !mirror.model().is_resident(block)
                        {
                            outside_missing += 1;
                        }
                    }
                }
            }
            let counters = session.controller.counters();
            let (outside, not_resident) = (
                counters.last_frame_drawn_outside_baked_set,
                counters.last_frame_drawn_not_resident,
            );
            if step == 0 {
                report.spawn.add(outside, not_resident, outside_missing);
            } else if walk.teleport[step] {
                report.teleport.add(outside, not_resident, outside_missing);
            } else {
                report.adjacent.add(outside, not_resident, outside_missing);
                if cell_changed {
                    report
                        .adjacent_change_frames
                        .add(outside, not_resident, outside_missing);
                }
            }
            if outside > 0 && report.first_outside.len() < 8 {
                report.first_outside.push((step, camera_cell, outside));
            }
            if not_resident > 0 && report.first_not_resident.len() < 8 {
                report
                    .first_not_resident
                    .push((step, camera_cell, not_resident));
            }
            let nanos = cpu.value(StreamingStage::LightmapResidency).unwrap_or(0);
            if cell_changed {
                report.residency_nanos_change.push(nanos);
            } else {
                report.residency_nanos_steady.push(nanos);
                if delivered > 0 {
                    report.prepare_with_completions.push(prepare_nanos);
                    report.max_admitted_bytes = report.max_admitted_bytes.max(delivered);
                } else {
                    report.prepare_without_completions.push(prepare_nanos);
                }
                for (parts, value) in
                    report
                        .steady_parts
                        .iter_mut()
                        .zip([prepare_nanos, apply_nanos, finish_nanos])
                {
                    parts.push(value);
                }
            }
            report.frames += 1;
            seconds += FRAME_SECONDS;
            if let VisibleCells::Culled(cells) = visibility.visible_cells {
                scratch = cells;
            }
            if session.controller.permits_in_use() > 0 {
                report.paced_frames += 1;
                if let Some(rest) = frame_budget.checked_sub(frame_started.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        }
        let session = level.lightmap_mut().expect("installed");
        let bytes = session.controller.residency_bytes();
        report.step_bytes.push([
            bytes.resident_lightmap_bytes,
            bytes.resident_shadowmask_bytes,
            bytes.mandatory_lightmap_bytes,
            bytes.mandatory_shadowmask_bytes,
        ]);
        let after = section_reads(&read_counters);
        report
            .step_reads
            .push((after.0 - step_reads_before.0, after.1 - step_reads_before.1));
    }

    let session = level.lightmap_mut().expect("installed");
    report.counters = Some(session.controller.counters());
    report.physical_reads = session.ledger().physical_reads();
    report.wall = started.elapsed();
    report.pool = mirror.stats.clone();
    level.retire(&mut no_sh);
    drop(level);
    report
}

fn elapsed_nanos(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn max_and_p95(mut values: Vec<u64>) -> (u64, u64) {
    if values.is_empty() {
        return (0, 0);
    }
    values.sort_unstable();
    let p95 = values[((values.len() as f64 * 0.95).ceil() as usize).clamp(1, values.len()) - 1];
    (*values.last().expect("non-empty"), p95)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB
}

fn micros_line(label: &str, nanos: &[u64]) -> String {
    if nanos.is_empty() {
        return format!("{label}: no frames");
    }
    let mut sorted = nanos.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| {
        sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1] as f64
            / 1000.0
    };
    let mean = sorted.iter().sum::<u64>() as f64 / sorted.len() as f64 / 1000.0;
    format!(
        "{label}: {} frames, mean {mean:.1} us, p50 {:.1}, p95 {:.1}, p99 {:.1}, max {:.1} us",
        sorted.len(),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0),
    )
}

fn print_run(walk: &Walk, levers: Levers, report: &RunReport) {
    let steps = report.step_bytes.len().max(1) as u64;
    let column = |i: usize| report.step_bytes.iter().map(|b| b[i]).collect::<Vec<_>>();
    let total = |a: usize, b: usize| {
        report
            .step_bytes
            .iter()
            .map(|v| v[a] + v[b])
            .collect::<Vec<_>>()
    };
    let (res_max, res_p95) = max_and_p95(total(0, 1));
    let (res22_max, res22_p95) = max_and_p95(column(0));
    let (res42_max, res42_p95) = max_and_p95(column(1));
    let (man_max, man_p95) = max_and_p95(total(2, 3));
    let (man22_max, man22_p95) = max_and_p95(column(2));
    let (man42_max, man42_p95) = max_and_p95(column(3));
    let reads22: u64 = report.step_reads.iter().map(|r| r.0).sum();
    let reads42: u64 = report.step_reads.iter().map(|r| r.1).sum();
    let (step_read_max, step_read_p95) =
        max_and_p95(report.step_reads.iter().map(|r| r.0 + r.1).collect());
    let pool = &report.pool;
    let c = report.counters.unwrap_or_default();
    let (drain_bytes_max, drain_bytes_p95) = max_and_p95(pool.drain_install_bytes.clone());
    println!(
        "== {} | L = {} m | cap {} | {} steps, {} teleports, {} frames ({} paced), wall {:.1} s",
        walk.kind.label(),
        levers.lead_metres,
        levers.cap_layers,
        report.step_bytes.len(),
        walk.teleports(),
        report.frames,
        report.paced_frames,
        report.wall.as_secs_f64(),
    );
    println!(
        "   spawn preload: {} pairs, {:.1} MiB read, {:.1} MiB uploaded in one drain, {:.1} ms \
         (positional reads + model plan)",
        report.preload_pairs,
        mib(report.preload_bytes),
        mib(pool.preload_install_bytes),
        report.preload_millis,
    );
    println!(
        "   resident MiB max / p95: total {:.1} / {:.1} (id 22 {:.1} / {:.1}, id 42 {:.1} / {:.1})",
        mib(res_max),
        mib(res_p95),
        mib(res22_max),
        mib(res22_p95),
        mib(res42_max),
        mib(res42_p95),
    );
    println!(
        "   mandatory MiB max / p95: total {:.1} / {:.1} (id 22 {:.1} / {:.1}, id 42 {:.1} / {:.1})",
        mib(man_max),
        mib(man_p95),
        mib(man22_max),
        mib(man22_p95),
        mib(man42_max),
        mib(man42_p95),
    );
    println!(
        "   pool: first {} layers, peak {} layers ({} occupied), layer {:.1} MiB; peak held \
         {:.1} MiB (spare and retiring included); growths {}, growth transient peak {:.1} MiB; \
         repacks {} ({:.2}% of steps, {} copies); deferred {}; evictions {}; refusals {}; \
         failed {}",
        pool.first_layers,
        pool.peak_layers,
        pool.peak_occupied_layers,
        mib(report.layer_bytes),
        mib(pool.peak_pool_bytes),
        pool.growths,
        mib(pool.growth_transient_peak_bytes),
        pool.repacks,
        pool.repacks as f64 * 100.0 / steps as f64,
        pool.repack_copies,
        pool.deferred,
        pool.evictions,
        pool.refusals,
        pool.failed,
    );
    println!(
        "   reads (after spawn): id 22 {:.1} MiB, id 42 {:.1} MiB; per step mean {:.2} MiB, \
         p95 {:.2}, max {:.2}; {} pairs requested, {} physical reads; installs {} ({:.1} MiB \
         uploaded over {} submitted drains, per drain p95 {:.2} MiB, max {:.2} MiB); \
         cancelled {}, departed {}, failed reads {}",
        mib(reads22),
        mib(reads42),
        mib(reads22 + reads42) / steps as f64,
        mib(step_read_p95),
        mib(step_read_max),
        c.reads_requested,
        report.physical_reads,
        pool.uploads,
        mib(pool.install_bytes),
        pool.submissions,
        mib(drain_bytes_p95),
        mib(drain_bytes_max),
        c.cancelled_reads,
        c.departed_reads,
        c.failed_reads,
    );
    println!(
        "   hit rate: {} of {} blocks joining M(c, L) already resident ({:.1}%)",
        report.entering_resident,
        report.entering_blocks,
        report.entering_resident as f64 * 100.0 / report.entering_blocks.max(1) as f64,
    );
    println!("   misses, spawn step: {}", report.spawn.line());
    println!("   misses, adjacent steps: {}", report.adjacent.line());
    println!(
        "   misses, adjacent steps, cell-change frames only: {}",
        report.adjacent_change_frames.line()
    );
    println!("   misses, teleport steps: {}", report.teleport.line());
    if !report.first_outside.is_empty() {
        println!(
            "   first outside-baked-set frames (step, camera cell, blocks): {:?}",
            report.first_outside
        );
    }
    if !report.first_not_resident.is_empty() {
        println!(
            "   first not-resident frames (step, camera cell, blocks): {:?}",
            report.first_not_resident
        );
    }
    println!(
        "   visibility paths: portal {}, step-limit {}, other {}; eye-locate mismatches {}",
        report.path_portal,
        report.path_step_limit,
        report.path_other,
        report.locate_mismatch_frames,
    );
    println!(
        "   cpu {}",
        micros_line(
            "lightmap_residency, camera-cell change",
            &report.residency_nanos_change
        )
    );
    println!(
        "   cpu {}",
        micros_line("lightmap_residency, steady", &report.residency_nanos_steady)
    );
    for (label, parts) in [
        "steady part prepare_drains",
        "steady part apply_outcome",
        "steady part finish_frame",
    ]
    .into_iter()
    .zip(&report.steady_parts)
    {
        println!("   wall {}", micros_line(label, parts));
    }
    println!(
        "   wall {}",
        micros_line(
            "steady prepare_drains, frames admitting completions",
            &report.prepare_with_completions
        )
    );
    println!(
        "   wall {} (most admitted in one frame {:.2} MiB)",
        micros_line(
            "steady prepare_drains, frames admitting none",
            &report.prepare_without_completions
        ),
        mib(report.max_admitted_bytes),
    );
    println!(
        "   cpu {}",
        micros_line("pool model plan per submitted drain", &pool.plan_nanos)
    );
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_LIGHTMAP_WALK_PRL"]
fn lightmap_residency_walks_from_prl() {
    let path = std::env::var(PRL_ENV).unwrap_or_else(|_| panic!("{PRL_ENV} must name a PRL"));
    let steps = std::env::var(STEPS_ENV)
        .ok()
        .map(|s| s.parse::<usize>().expect("steps"))
        .unwrap_or(DEFAULT_STEPS);
    let runs = std::env::var(RUNS_ENV).unwrap_or_else(|_| "all".to_string());

    let load_started = Instant::now();
    let world = postretro_level_loader::load_prl(&path).expect("load PRL");
    println!(
        "PRL: {path}\nloaded in {:.1} s",
        load_started.elapsed().as_secs_f64()
    );
    let view = LightmapLevelView::of(&world)
        .expect("the PRL must stream its lightmap: id 51, portals, POSTRETRO_LIGHTMAP_STREAMING");
    println!(
        "{} blocks, {} cells, max lead {:.1} m; steps {steps}, {RUN_SPEED} m/s at 60 Hz, \
         dwell capped at {MAX_DWELL_FRAMES} frames, 4 headings 90 deg apart, HFOV \
         {HFOV_DEGREES} deg at 16:9, seed {WALK_SEED:#x}",
        view.manifest.block_count(),
        world.cells.len(),
        view.residency_set.max_lead as f32 / LEAD_UNITS_PER_METRE as f32,
    );

    let default = Levers {
        lead_metres: 16.0,
        cap_layers: 15,
    };
    let mut configs = vec![default];
    if runs == "all" {
        for cap in [7, 12, 25] {
            configs.push(Levers {
                cap_layers: cap,
                ..default
            });
        }
        for lead_metres in [0.0, 32.0] {
            configs.push(Levers {
                lead_metres,
                ..default
            });
        }
    }
    for kind in WalkKind::ALL {
        let walk = Walk::build(&world, kind, steps);
        println!(
            "\n### {}: {} steps, {} teleports, {} frames ({:.0} s of play)",
            kind.label(),
            walk.cells.len(),
            walk.teleports(),
            walk.frames(),
            walk.frames() as f64 * FRAME_SECONDS,
        );
        for &levers in &configs {
            let report = run(&world, &walk, levers);
            print_run(&walk, levers, &report);
        }
    }
}

/// AC 24, CPU half: level load time under the process's
/// `POSTRETRO_LIGHTMAP_STREAMING` mode, then (streaming only) the spawn
/// preload at `POSTRETRO_LIGHTMAP_WALK_SPAWN` = `x,y,z`: positional reads plus
/// the pool model's plan, as level install runs it. GPU upload is not timed
/// here; headless captures carry it. Run once per mode:
///
/// ```text
/// POSTRETRO_LIGHTMAP_STREAMING=all-resident \
/// POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/campaign-test.prl \
/// POSTRETRO_LIGHTMAP_WALK_SPAWN=-65.84,2.6,-45.92 \
///   cargo test -p postretro --bin postretro lightmap_install_timing_from_prl \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement helper; set POSTRETRO_LIGHTMAP_WALK_PRL"]
fn lightmap_install_timing_from_prl() {
    const REPEATS: usize = 3;
    let path = std::env::var(PRL_ENV).unwrap_or_else(|_| panic!("{PRL_ENV} must name a PRL"));
    let spawn: Vec<f32> = std::env::var("POSTRETRO_LIGHTMAP_WALK_SPAWN")
        .expect("POSTRETRO_LIGHTMAP_WALK_SPAWN = x,y,z")
        .split(',')
        .map(|v| v.trim().parse().expect("spawn coordinate"))
        .collect();
    let spawn_eye = Vec3::new(spawn[0], spawn[1], spawn[2]);
    println!("PRL: {path}");
    for repeat in 0..REPEATS {
        let started = Instant::now();
        let mut world = postretro_level_loader::load_prl(&path).expect("load PRL");
        let load = started.elapsed();
        if LightmapLevelView::of(&world).is_none() {
            let payloads: u64 = world
                .take_gpu_lighting_payloads()
                .blocks
                .iter()
                .map(|block| {
                    (block.irradiance.len()
                        + block.direction.len()
                        + block
                            .shadowmask
                            .as_ref()
                            .map_or(0, |[a, b]| a.len() + b.len())) as u64
                })
                .sum();
            println!(
                "repeat {repeat}: all-resident load {:.1} ms; whole lightmap payloads held for \
                 upload {:.1} MiB",
                load.as_secs_f64() * 1000.0,
                mib(payloads),
            );
            continue;
        }
        let view = LightmapLevelView::of(&world).expect("streams");
        let hints = decode_level_hints(world.cluster_directory()).expect("id-49 hints");
        let mut session =
            LightmapStreamingSession::new(view, hints.as_deref()).expect("lightmap session");
        let mut mirror =
            PoolMirror::new(view.manifest, session.controller.levers().pool_cap_layers());
        let camera_cell = world.locate_cell(spawn_eye) as u32;
        let preload_started = Instant::now();
        session.update_camera_set(view.residency_set, camera_cell);
        let summary = session
            .preload(&[], |batch| Ok(mirror.drain(batch)))
            .expect("spawn preload");
        let preload = preload_started.elapsed();
        println!(
            "repeat {repeat}: stream load {:.1} ms; spawn preload (cell {camera_cell}) {} pairs, \
             {:.1} MiB, {:.1} ms; pool first generation {} layers",
            load.as_secs_f64() * 1000.0,
            summary.reads.pairs,
            mib(summary.reads.bytes),
            preload.as_secs_f64() * 1000.0,
            mirror.model().layers(),
        );
    }
}

#[test]
fn mandatory_blocks_include_every_block_of_a_multi_block_cell() {
    use crate::lightmap_streaming::test_fixtures::{M, residency_set};

    // Cell 0 owns blocks 0 and 1, cell 1 owns block 2, cell 2 owns none.
    let cell_blocks = cell_block_ranges([0, 0, 1], 3);
    assert_eq!(cell_blocks, [0..2, 2..3, 0..0]);

    // Camera 1 reaches cell 1 at 0 m, cell 0 at 8 m and cell 2 at 20 m.
    let set = residency_set(3, &[(1, 1, 0), (1, 0, 8), (1, 2, 20)], 32);
    let mut blocks = Vec::new();
    mandatory_blocks(&set, &cell_blocks, 1, 0, &mut blocks);
    assert_eq!(blocks, [2]);
    mandatory_blocks(&set, &cell_blocks, 1, 8 * M, &mut blocks);
    assert_eq!(blocks, [0, 1, 2], "cell 0 brings both of its blocks");
    mandatory_blocks(&set, &cell_blocks, 1, 32 * M, &mut blocks);
    assert_eq!(blocks, [0, 1, 2], "a chartless cell adds none");
}
