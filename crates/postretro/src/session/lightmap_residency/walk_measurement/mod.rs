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
// frame at a fixed pitch. Paths and eyes are seeded, so frame i is the same
// pose in every run, mode and build over one cell layout.
//
// Read modes (`POSTRETRO_LIGHTMAP_WALK_READS`):
// - `realtime` (default): one drain per frame; a frame with reads in flight
//   sleeps to its 16.7 ms deadline, so misses race disk reads. The pool
//   mirror retires a grown-out generation per drain call.
// - `completed`: the residency gate's mode (O17, O29). Every read in flight
//   completes before each of the frame's two drains, so frame N's reads land
//   in its second drain; no wall clock is involved. Misses are counted once
//   per drawn block, after the last drain, and retirement advances once per
//   frame.
//
// After the last drain each drawn, non-resident block counts as deferred
// behind a retiring generation when the frame's latest drain carrying it
// deferred it, and as left behind by admission otherwise, undemanded
// included. The mirror's pool has no device limit, so every deferral is
// behind a retiring generation (O18).
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
// `POSTRETRO_LIGHTMAP_STREAMING` unset or `stream`. Camera:
// `POSTRETRO_LIGHTMAP_WALK_HFOV` (degrees, default 100),
// `POSTRETRO_LIGHTMAP_WALK_ASPECT` (`W:H`, default `16:9`; horizontal FOV
// stays fixed) and `POSTRETRO_LIGHTMAP_WALK_PITCH` (degrees, up positive,
// default 0).
//
// Residency gate, per projection and pitch: run the pre-change build with
// `POSTRETRO_LIGHTMAP_WALK_READS=completed` and
// `POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_OUT=<file>`, which writes each run's
// per-frame trip record; then the rect build, on its own bake, with the same
// modes and `POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_IN=<file>`. The rect run
// splits its frames by the recorded trips (O28, O38) and prints both builds'
// totals in and outside the gate. A pair whose poses or locate mismatches
// differ prints INVALID. Each run also prints the bake it read (O14).

mod baked_set;
mod completed_reads;
mod frame_driver;
mod modes;
mod paths;
mod pool_mirror;
mod report;
#[cfg(test)]
mod tests;
mod trip_record;

use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_level_format::SectionId;
use postretro_level_loader::{LevelWorld, PrlReadCounters};
use postretro_visibility::{TimingGate, VisibilityPath, VisibleCells, determine_visible_cells};

use super::{LightmapLevelView, LightmapStreamingSession};
use crate::session::level_streaming::LevelStreaming;
use crate::streaming::cell_demand::CellDemand;
use crate::streaming::cell_demand::LEAD_UNITS_PER_METRE;
use crate::streaming::cluster_hints::decode_level_hints;
use baked_set::{
    bake_identity, bake_token, cell_block_ranges, mandatory_blocks, mandatory_cells,
    mandatory_set_sizes,
};
use frame_driver::{FrameDriver, FrameInput};
use modes::{ReadsMode, WalkModes, view_projection};
use paths::{SplitMix64, WALK_SEED, WalkKind, camera_adjacency, walk_path};
use pool_mirror::{PoolMirror, RetireClock};
use report::{RunReport, mib, print_run, print_split};
use trip_record::{FrameRecord, RunRecord, pose_fingerprint};

const PRL_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_PRL";
const STEPS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_STEPS";
const RUNS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_RUNS";
const DEFAULT_STEPS: usize = 2_000;
const FRAME_SECONDS: f64 = 1.0 / 60.0;
/// A brisk boomer-shooter run speed, metres per second.
const RUN_SPEED: f32 = 8.0;
const MAX_DWELL_FRAMES: u32 = 60;
const EYE_TRIES: usize = 32;

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

fn section_reads(counters: &PrlReadCounters) -> (u64, u64) {
    (
        counters.section_bytes(SectionId::Lightmap as u32),
        counters.section_bytes(SectionId::ShadowmaskAtlas as u32),
    )
}

fn run_label(walk: &Walk, levers: Levers, modes: &WalkModes) -> String {
    format!(
        "{} | L {} m | cap {} | {}",
        walk.kind.label(),
        levers.lead_metres,
        levers.cap_layers,
        modes.label()
    )
}

fn run(
    world: &LevelWorld,
    walk: &Walk,
    levers: Levers,
    modes: &WalkModes,
    bake: &str,
) -> RunReport {
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
    session
        .controller
        .levers_mut()
        .set_pool_cap_layers(levers.cap_layers);
    // L is the level's: set on the stage, which every frame hands the
    // lightmap demand.
    let mut stage = CellDemand::new(view.residency_set.max_lead);
    stage.set_lead_metres(levers.lead_metres);
    let lead = stage.lead();
    session.controller.set_lead(lead);
    let mirror = PoolMirror::new(view.manifest, levers.cap_layers, modes.reads.retire_clock());
    let mut level = LevelStreaming::default();
    level.install_cell_demand(stage);
    level.install_lightmap(session);
    let mut driver = FrameDriver::new(level, mirror, modes.reads, cell_blocks.clone());

    let mut report = RunReport {
        layer_bytes: driver.mirror().layer_bytes(),
        mandatory_set: mandatory_set_sizes(world, view.residency_set, &cell_blocks, lead),
        record: RunRecord {
            label: run_label(walk, levers, modes),
            bake: bake.to_string(),
            ..RunRecord::default()
        },
        ..RunReport::default()
    };
    let started = Instant::now();
    let mut rng = SplitMix64(WALK_SEED ^ 0xE7E5);

    // A synchronous preload approximating a settled entry: the spawn cell's
    // mandatory set, read and installed in one drain before the first frame.
    let spawn_eye = eye_point(world, walk.cells[0], &mut rng);
    let spawn_cell = world.locate_cell(spawn_eye) as u32;
    let before = section_reads(&read_counters);
    {
        let (session, mirror) = driver.session_and_mirror();
        session.update_camera_set(view.residency_set, spawn_cell);
        let summary = session
            .preload(&[], |batch| Ok(mirror.drain(batch)))
            .expect("settled-entry preload");
        assert_eq!(summary.deferred, 0, "a fresh pool never defers a preload");
        assert!(session.settled(), "spawn set resident after preload");
        report.preload_pairs = summary.reads.pairs;
        report.preload_millis = summary.elapsed.as_secs_f64() * 1000.0;
        mirror.begin_play();
    }
    let after = section_reads(&read_counters);
    report.preload_bytes = (after.0 - before.0) + (after.1 - before.1);

    let mut scratch = Vec::new();
    let mut mandatory_now = Vec::new();
    let mut mandatory_prev = Vec::new();
    let mut mandatory_cells_now = Vec::new();
    let mut previous_cell = spawn_cell;
    mandatory_blocks(
        view.residency_set,
        &cell_blocks,
        spawn_cell,
        lead,
        &mut mandatory_prev,
    );
    mandatory_cells(
        view.residency_set,
        spawn_cell,
        lead,
        &mut mandatory_cells_now,
    );
    let mut seconds = 0.0f64;
    let frame_budget = Duration::from_secs_f64(FRAME_SECONDS);
    let projection = (modes.projection.hfov_degrees, modes.projection.aspect);

    for (step, &cell) in walk.cells.iter().enumerate() {
        let step_reads_before = section_reads(&read_counters);
        for frame in 0..walk.dwell[step] {
            let frame_started = Instant::now();
            let eye = eye_point(world, cell, &mut rng);
            let yaw = walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2;
            let view_proj = view_projection(eye, yaw, modes.pitch, modes.projection);
            let (visibility, _) = determine_visible_cells(
                eye,
                view_proj,
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
                        report.entering_resident +=
                            u64::from(driver.mirror().model().is_resident(block));
                    }
                }
                std::mem::swap(&mut mandatory_now, &mut mandatory_prev);
                mandatory_cells(
                    view.residency_set,
                    camera_cell,
                    lead,
                    &mut mandatory_cells_now,
                );
                previous_cell = camera_cell;
            }

            let outcome = driver.frame(
                FrameInput {
                    visible_cells: &visibility.visible_cells,
                    camera_cell,
                    path: visibility.stats.path,
                    seconds,
                },
                view.residency_set,
            );
            // Drawn blocks outside M(c, L) that are still not resident after
            // this frame's drain: the frame samples them SH-only.
            let mut outside_missing = 0u32;
            if let (VisibilityPath::PrlPortal { .. }, VisibleCells::Culled(cells)) =
                (visibility.stats.path, &visibility.visible_cells)
            {
                for &drawn in cells {
                    for block in cell_blocks[drawn as usize].clone() {
                        if mandatory_prev.binary_search(&block).is_err()
                            && !driver.mirror().model().is_resident(block)
                        {
                            outside_missing += 1;
                        }
                    }
                }
            }
            if let VisibleCells::Culled(cells) = &visibility.visible_cells {
                for &drawn in cells {
                    let blocks = cell_blocks[drawn as usize].len() as u32;
                    if blocks > 0 && mandatory_cells_now.binary_search(&drawn).is_err() {
                        report.outside_cells.add(blocks);
                    }
                }
            }
            let (outside, not_resident) = (outcome.outside, outcome.not_resident);
            if step == 0 {
                report.spawn.add(outside, not_resident, outside_missing);
                report
                    .causes_spawn
                    .add(outcome.deferred, outcome.left_behind);
            } else {
                report
                    .causes_play
                    .add(outcome.deferred, outcome.left_behind);
                if walk.teleport[step] {
                    report.teleport.add(outside, not_resident, outside_missing);
                } else {
                    report.adjacent.add(outside, not_resident, outside_missing);
                    if cell_changed {
                        report
                            .adjacent_change_frames
                            .add(outside, not_resident, outside_missing);
                    }
                }
            }
            report.record.frames.push(FrameRecord::of(
                pose_fingerprint(step, cell, eye, yaw, modes.pitch, projection),
                step == 0,
                visibility.stats.path,
                &outcome,
            ));
            if outside > 0 && report.first_outside.len() < 8 {
                report.first_outside.push((step, camera_cell, outside));
            }
            if not_resident > 0 && report.first_not_resident.len() < 8 {
                report
                    .first_not_resident
                    .push((step, camera_cell, not_resident));
            }
            if cell_changed {
                report.residency_nanos_change.push(outcome.residency_nanos);
            } else {
                report.residency_nanos_steady.push(outcome.residency_nanos);
                if outcome.delivered > 0 {
                    report.prepare_with_completions.push(outcome.prepare_nanos);
                    report.max_admitted_bytes = report.max_admitted_bytes.max(outcome.delivered);
                } else {
                    report
                        .prepare_without_completions
                        .push(outcome.prepare_nanos);
                }
                for (parts, value) in report.steady_parts.iter_mut().zip([
                    outcome.prepare_nanos,
                    outcome.apply_nanos,
                    outcome.finish_nanos,
                ]) {
                    parts.push(value);
                }
            }
            report.frames += 1;
            seconds += FRAME_SECONDS;
            if let VisibleCells::Culled(cells) = visibility.visible_cells {
                scratch = cells;
            }
            if modes.reads == ReadsMode::RealTime && driver.permits_in_use() > 0 {
                report.paced_frames += 1;
                if let Some(rest) = frame_budget.checked_sub(frame_started.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        }
        let bytes = driver.session().controller.residency_bytes();
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

    report.record.locate_mismatches = report.locate_mismatch_frames;
    let (counters, physical_reads, mirror) = driver.finish();
    report.counters = Some(counters);
    report.physical_reads = physical_reads;
    report.wall = started.elapsed();
    report.pool = mirror.stats.clone();
    report
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
    let modes = WalkModes::from_env();
    let recorded = modes.record_in.as_deref().map(|record| {
        let runs = trip_record::read_records(record).unwrap_or_else(|why| panic!("{why}"));
        (record, runs)
    });

    let load_started = Instant::now();
    let world = postretro_level_loader::load_prl(&path).expect("load PRL");
    let bake = bake_identity(&path, &world);
    println!(
        "PRL: {path}\nloaded in {:.1} s\nbake read: {bake}",
        load_started.elapsed().as_secs_f64()
    );
    let view = LightmapLevelView::of(&world)
        .expect("the PRL must stream its lightmap: id 51, portals, POSTRETRO_LIGHTMAP_STREAMING");
    println!(
        "{} blocks, {} cells, max lead {:.1} m; steps {steps}, {RUN_SPEED} m/s at 60 Hz, \
         dwell capped at {MAX_DWELL_FRAMES} frames, 4 headings 90 deg apart, {}, seed \
         {WALK_SEED:#x}",
        view.manifest.block_count(),
        world.cells.len(),
        view.residency_set.max_lead as f32 / LEAD_UNITS_PER_METRE as f32,
        modes.label(),
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
    let mut records = Vec::new();
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
            let report = run(&world, &walk, levers, &modes, &bake);
            print_run(&walk, levers, &report);
            let own = &report.record;
            print_split("its own trip record", &trip_record::split(own, own));
            if let Some((file, runs)) = &recorded {
                let pair = runs
                    .iter()
                    .find(|run| run.label == own.label)
                    .ok_or_else(|| format!("{file} holds no run labelled {:?}", own.label))
                    .and_then(|run| {
                        println!("   recorded run read: {}", run.bake);
                        let token = |bake| bake_token(bake, "id51");
                        if token(&run.bake) == token(&own.bake)
                            && bake_token(&run.bake, "walk_epoch")
                                != bake_token(&own.bake, "walk_epoch")
                        {
                            println!(
                                "   WARNING: this build's walk epoch differs from the recorded \
                                 run's but both read the same id 51: a rect run on a pre-bump bake \
                                 is invalid (O14)"
                            );
                        }
                        trip_record::split(run, own)
                    });
                print_split(&format!("{file}'s trip record"), &pair);
            }
            records.push(report.record);
        }
    }
    if let Some(file) = &modes.record_out {
        trip_record::write_records(file, &records).expect("write the trip record");
        println!("\ntrip record written: {file} ({} runs)", records.len());
    }
}

/// AC 24, CPU half: level load time under the process's
/// `POSTRETRO_LIGHTMAP_STREAMING` mode, then (streaming only) a synchronous
/// preload at `POSTRETRO_LIGHTMAP_WALK_SPAWN` = `x,y,z`, approximating a
/// settled entry: positional reads plus the pool model's plan. GPU upload is
/// not timed here; headless captures carry it. Run once per mode:
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
        let mut mirror = PoolMirror::new(
            view.manifest,
            session.controller.levers().pool_cap_layers(),
            RetireClock::Drains,
        );
        let camera_cell = world.locate_cell(spawn_eye) as u32;
        let preload_started = Instant::now();
        session.update_camera_set(view.residency_set, camera_cell);
        let summary = session
            .preload(&[], |batch| Ok(mirror.drain(batch)))
            .expect("settled-entry preload");
        let preload = preload_started.elapsed();
        println!(
            "repeat {repeat}: stream load {:.1} ms; settled-entry preload (cell {camera_cell}) {} pairs, \
             {:.1} MiB, {:.1} ms; pool first generation {} layers",
            load.as_secs_f64() * 1000.0,
            summary.reads.pairs,
            mib(summary.reads.bytes),
            preload.as_secs_f64() * 1000.0,
            mirror.model().layers(),
        );
    }
}
