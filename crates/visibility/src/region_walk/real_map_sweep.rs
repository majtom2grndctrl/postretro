// Real-map sweep (R1): the bounded-region walk against the exact oracle on baked PRLs.
// See: context/plans/in-progress/portal-walk-bounded-regions/research.md §Sweep setup, §Report sets
//
// Ignored. Reads `.prl` paths from POSTRETRO_WALK_SWEEP_PRL (one path or a
// comma-separated list). Rerun from the workspace root, release, single-threaded:
//
//   POSTRETRO_WALK_SWEEP_PRL=content/dev/maps/stress-warren-hallway-inspection.prl,content/dev/maps/reflection-lake-test.prl,content/dev/maps/occlusion-test.prl \
//     cargo test --release -p postretro-visibility real_map_sweep -- --ignored --nocapture --test-threads=1
//
// Poses follow the research prototype (`prototype/src/main.rs`): every empty
// interior cell at its centre and, where tall enough, at floor + 1.6 m, each
// located through the map's own locator; 8 yaws × pitch 0/+45/−45; HFOV 100°
// 16:9, near 0.1, far 4096. On the hallway, poses inside the shafts also look
// straight up and down (pitch ±89.9°). Walk CPU is best of 2 on one thread
// and includes the walk's `#[cfg(test)]` counters.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use glam::Vec3;

use super::oracle::{OracleWalk, oracle_from_camera};
use super::test_fixtures::camera_view_proj;
use super::tests::unclassified_misses;
use super::{MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkView, WalkWorld, walk_from_camera};
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::cell_locator::{CellLocatorChild as FormatChild, CellLocatorSection};
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData, load_prl,
};

const SWEEP_ENV: &str = "POSTRETRO_WALK_SWEEP_PRL";
const EYE_HEIGHT: f32 = 1.6;
const YAWS: usize = 8;
const PITCHES_DEG: [f32; 3] = [0.0, 45.0, -45.0];
const VERTICAL_PITCH_DEG: f32 = 89.9;

/// Named hallway regions, from the prototype's `analyze_alt.py`: shaft boxes
/// with an (8, 2, 8) m margin, and the arena's ground layer.
const SHAFT_A: u8 = 1;
const SHAFT_B: u8 = 2;
const SHAFT_C: u8 = 4;
const ARENA: u8 = 8;
const SHAFTS: u8 = SHAFT_A | SHAFT_B | SHAFT_C;

fn in_box(p: Vec3, lo: [f32; 3], hi: [f32; 3]) -> bool {
    let margin = [8.0, 2.0, 8.0];
    (0..3).all(|i| lo[i] - margin[i] <= p[i] && p[i] <= hi[i] + margin[i])
}

fn hallway_regions(p: Vec3) -> u8 {
    let mut groups = 0;
    if in_box(p, [-46.13, 18.49, 17.27], [-38.4, 33.73, 24.99]) {
        groups |= SHAFT_A;
    }
    if in_box(p, [80.67, 18.49, -109.52], [88.39, 33.73, -101.8]) {
        groups |= SHAFT_B;
    }
    if in_box(p, [-46.13, 18.49, -109.52], [-38.4, 33.73, -101.8]) {
        groups |= SHAFT_C;
    }
    if (20.0..=100.0).contains(&p.x) && p.y <= 16.0 && (-60.0..=60.0).contains(&p.z) {
        groups |= ARENA;
    }
    groups
}

#[derive(Debug, Clone, Copy)]
struct Pose {
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    cell: usize,
    groups: u8,
}

/// The prototype's generator, plus straight up and down at shaft poses.
fn poses(world: &LevelWorld, hallway: bool) -> (Vec<Pose>, usize) {
    let mut positions = Vec::new();
    let mut skipped = 0;
    for c in world.cells.iter() {
        if c.is_solid || c.is_exterior {
            continue;
        }
        let centre = (c.bounds_min + c.bounds_max) * 0.5;
        let mut candidates = vec![centre];
        if c.bounds_max.y - c.bounds_min.y > EYE_HEIGHT + 0.1 {
            candidates.push(Vec3::new(centre.x, c.bounds_min.y + EYE_HEIGHT, centre.z));
        }
        for p in candidates {
            let cell = world.locate_cell(p);
            if cell >= world.cell_count()
                || world.cells[cell].is_solid
                || world.cells[cell].is_exterior
            {
                skipped += 1;
                continue;
            }
            positions.push((p, cell));
        }
    }
    let mut out = Vec::new();
    for &(eye, cell) in &positions {
        let groups = if hallway { hallway_regions(eye) } else { 0 };
        let mut pitches = PITCHES_DEG.to_vec();
        if groups & SHAFTS != 0 {
            pitches.extend([VERTICAL_PITCH_DEG, -VERTICAL_PITCH_DEG]);
        }
        for &pitch_deg in &pitches {
            for k in 0..YAWS {
                out.push(Pose {
                    eye,
                    yaw_deg: k as f32 * 45.0,
                    pitch_deg,
                    cell,
                    groups,
                });
            }
        }
    }
    (out, skipped)
}

/// Light and SH-cluster data for R1's consumer deltas, when the map has it.
struct ConsumerData {
    /// Runtime cell → id-49 cluster.
    cell_to_cluster: Option<Vec<u32>>,
    /// Approximate shadow-candidate roster: dynamic-tier, compiler-selected
    /// static (`entity_shadow_lights`) and SH-animated lights, cell-assigned.
    candidates: Vec<(Vec3, f32)>,
    /// Every cell-assigned light.
    all_lights: Vec<(Vec3, f32)>,
}

impl ConsumerData {
    fn new(world: &LevelWorld) -> Self {
        let cell_to_cluster = world.cluster_directory.as_ref().and_then(|dir| {
            if dir.runtime_cell_count as usize != world.cell_count() {
                return None;
            }
            let mut map = vec![u32::MAX; world.cell_count()];
            for (id, cluster) in dir.clusters.iter().enumerate() {
                let start = cluster.member_start as usize;
                let end = start + cluster.member_count as usize;
                for &cell in dir.members.get(start..end)? {
                    *map.get_mut(cell as usize)? = id as u32;
                }
            }
            Some(map)
        });
        let selected: HashSet<u32> = world.entity_shadow_lights.iter().copied().collect();
        let mut candidates = Vec::new();
        let mut all_lights = Vec::new();
        for (i, light) in world.lights.iter().enumerate() {
            if light.cell_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
                continue;
            }
            // Missing influence: uncullable, as the renderer treats it.
            let sphere = world
                .light_influences
                .get(i)
                .map(|inf| (inf.center, inf.radius))
                .unwrap_or((Vec3::ZERO, f32::MAX));
            all_lights.push(sphere);
            if light.is_dynamic || selected.contains(&(i as u32)) || light.animated_slot.is_some() {
                candidates.push(sphere);
            }
        }
        Self {
            cell_to_cluster,
            candidates,
            all_lights,
        }
    }

    fn clusters(&self, world: &LevelWorld, cells: &[usize]) -> Option<usize> {
        let map = self.cell_to_cluster.as_ref()?;
        let set: HashSet<u32> = cells
            .iter()
            .filter(|&&c| world.cells[c].is_drawable)
            .map(|&c| map[c])
            .collect();
        Some(set.len())
    }

    /// `light_reaches_visible_cell` over the reach set's AABBs.
    fn lights_reaching(world: &LevelWorld, lights: &[(Vec3, f32)], cells: &[usize]) -> usize {
        lights
            .iter()
            .filter(|&&(center, radius)| {
                let r = radius.max(0.0);
                let r_sq = r * r;
                cells.iter().any(|&c| {
                    let cell = &world.cells[c];
                    center
                        .clamp(cell.bounds_min, cell.bounds_max)
                        .distance_squared(center)
                        <= r_sq
                })
            })
            .count()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ConsumerDelta {
    /// Visible SH clusters (from the drawable part of the set): walk, the
    /// oracle's full set, and its clean set (leaks dropped).
    clusters: Option<[usize; 3]>,
    /// Shadow-eligible lights by reach, same three sets.
    candidates: [usize; 3],
    all_lights: [usize; 3],
}

#[derive(Debug, Clone, Copy)]
struct Record {
    pose: Pose,
    considered: u32,
    tripped: bool,
    nanos: u64,
    extra_cells: i64,
    classified: usize,
    oracle_complete: bool,
    worst_expansions: u32,
    tests_per_degree: f32,
    consumers: Option<ConsumerDelta>,
}

#[derive(Debug)]
struct Failure {
    pose: Pose,
    tripped: bool,
    considered: u32,
    missed: Vec<usize>,
}

fn pct<T: Copy + PartialOrd + Default>(values: &mut [T], p: f64) -> T {
    if values.is_empty() {
        return T::default();
    }
    values.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    values[((values.len() - 1) as f64 * p).round() as usize]
}

fn report_row(label: &str, records: &[&Record], with_consumers: bool) {
    if records.is_empty() {
        eprintln!("  {label}: no walks");
        return;
    }
    let trips = records.iter().filter(|r| r.tripped).count();
    let mut tests: Vec<u32> = records.iter().map(|r| r.considered).collect();
    let mut micros: Vec<f64> = records.iter().map(|r| r.nanos as f64 / 1000.0).collect();
    let mut extra: Vec<i64> = records.iter().map(|r| r.extra_cells).collect();
    let classified: usize = records.iter().map(|r| r.classified).sum();
    let classified_poses = records.iter().filter(|r| r.classified > 0).count();
    let incomplete = records.iter().filter(|r| !r.oracle_complete).count();
    let worst_expansions = records
        .iter()
        .map(|r| r.worst_expansions)
        .max()
        .unwrap_or(0);
    let worst_per_degree = records
        .iter()
        .map(|r| r.tests_per_degree)
        .fold(0.0f32, f32::max);
    eprintln!(
        "  {label}: walks={} cap_trips={trips} tests p50/p95/p99/max={}/{}/{}/{} \
         us p50/p99/p99.9/max={:.1}/{:.0}/{:.0}/{:.0} extra_cells p50/p95/max={}/{}/{} \
         classified_exceptions={classified} (poses {classified_poses}) oracle_incomplete={incomplete} \
         worst_expansions_per_cell={worst_expansions} worst_tests_per_degree={worst_per_degree:.2}",
        records.len(),
        pct(&mut tests, 0.5),
        pct(&mut tests, 0.95),
        pct(&mut tests, 0.99),
        pct(&mut tests, 1.0),
        pct(&mut micros, 0.5),
        pct(&mut micros, 0.99),
        pct(&mut micros, 0.999),
        pct(&mut micros, 1.0),
        pct(&mut extra, 0.5),
        pct(&mut extra, 0.95),
        pct(&mut extra, 1.0),
    );
    let deltas: Vec<ConsumerDelta> = records.iter().filter_map(|r| r.consumers).collect();
    if !with_consumers || deltas.is_empty() {
        return;
    }
    let summarize = |name: &str, sets: Vec<[usize; 3]>| {
        let n = sets.len() as f64;
        let mean = |i: usize| sets.iter().map(|s| s[i] as f64).sum::<f64>() / n;
        let mut vs_exact: Vec<i64> = sets.iter().map(|s| s[0] as i64 - s[1] as i64).collect();
        let mut vs_clean: Vec<i64> = sets.iter().map(|s| s[0] as i64 - s[2] as i64).collect();
        let changed = vs_exact.iter().filter(|&&d| d != 0).count();
        eprintln!(
            "    {name}: mean walk {:.2} exact {:.2} clean {:.2}; walk-exact p50/p95/max={}/{}/{} \
             min={} (poses changed {changed}/{}); walk-clean p50/p95/max={}/{}/{} min={}",
            mean(0),
            mean(1),
            mean(2),
            pct(&mut vs_exact, 0.5),
            pct(&mut vs_exact, 0.95),
            pct(&mut vs_exact, 1.0),
            vs_exact.first().copied().unwrap_or(0),
            sets.len(),
            pct(&mut vs_clean, 0.5),
            pct(&mut vs_clean, 0.95),
            pct(&mut vs_clean, 1.0),
            vs_clean.first().copied().unwrap_or(0),
        );
    };
    let clusters: Vec<[usize; 3]> = deltas.iter().filter_map(|d| d.clusters).collect();
    if clusters.is_empty() {
        eprintln!("    visible SH clusters: no usable id-49 cell map");
    } else {
        summarize("visible SH clusters", clusters);
    }
    summarize(
        "shadow-eligible lights (candidate roster approx.)",
        deltas.iter().map(|d| d.candidates).collect(),
    );
    if deltas.iter().any(|d| d.all_lights != d.candidates) {
        summarize(
            "lights reaching the set (all cell-assigned)",
            deltas.iter().map(|d| d.all_lights).collect(),
        );
    }
}

struct MapResult {
    worst_tests: u32,
    failures: Vec<Failure>,
}

/// The prototype's loader: the sections the walk reads, nothing else. For a
/// PRL whose other sections the current loader rejects (an older bake).
fn load_visibility_only(path: &str) -> LevelWorld {
    let mut file = std::fs::File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let meta = read_container(&mut file).expect("container");
    let mut section = |id: SectionId| {
        read_section_data(&mut file, &meta, id as u32)
            .expect("section read")
            .unwrap_or_else(|| panic!("{path}: no {id:?} section"))
    };
    let cells_raw = section(SectionId::Cells);
    let portals_raw = section(SectionId::Portals);
    let locator_raw = section(SectionId::CellLocator);
    let cells = CellsSection::from_bytes(&cells_raw).expect("cells");
    let portals = PortalsSection::from_bytes(&portals_raw).expect("portals");
    let locator =
        CellLocatorSection::from_bytes(&locator_raw, cells.cells.len() as u32).expect("locator");
    let child = |c: FormatChild| match c {
        FormatChild::Cell(i) => CellLocatorChild::Cell(i as usize),
        FormatChild::Node(i) => CellLocatorChild::Node(i as usize),
    };
    let cell_data = cells
        .cells
        .iter()
        .map(|c| CellData {
            bounds_min: Vec3::from(c.bounds_min),
            bounds_max: Vec3::from(c.bounds_max),
            face_start: c.face_start,
            face_count: c.face_count,
            portal_ref_start: c.portal_ref_start,
            portal_ref_count: c.portal_ref_count,
            is_solid: c.is_solid(),
            is_exterior: c.is_exterior(),
            is_drawable: c.is_drawable(),
        })
        .collect();
    let nodes = locator
        .nodes
        .iter()
        .map(|n| CellLocatorNodeData {
            plane_normal: Vec3::from(n.plane_normal),
            plane_distance: n.plane_distance,
            front: child(n.front),
            back: child(n.back),
        })
        .collect();
    let portal_data: Vec<PortalData> = portals
        .portals
        .iter()
        .map(|p| PortalData {
            polygon: portals.vertices
                [p.vertex_start as usize..(p.vertex_start + p.vertex_count) as usize]
                .iter()
                .map(|v| Vec3::from(*v))
                .collect(),
            front_cell: p.front_leaf as usize,
            back_cell: p.back_leaf as usize,
        })
        .collect();
    let has_portals = !portal_data.is_empty();
    LevelWorld::new_visibility_only(
        cell_data,
        cells.portal_refs,
        child(locator.root),
        nodes,
        portal_data,
        has_portals,
    )
    .expect("visibility-only world")
}

fn drawable_count(world: &LevelWorld, cells: impl Iterator<Item = usize>) -> i64 {
    cells.filter(|&c| world.cells[c].is_drawable).count() as i64
}

fn sweep_map(path: &str) -> MapResult {
    let started = Instant::now();
    let (world, full_load) = match load_prl(path) {
        Ok(world) => (world, true),
        Err(e) => {
            eprintln!(
                "[real_map_sweep] {path}: full load failed ({e:?}); loading cells, portals \
                 and locator only, so no light or SH-cluster deltas"
            );
            (load_visibility_only(path), false)
        }
    };
    let load_time = started.elapsed();
    let name = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let hallway = name.starts_with("stress-warren-hallway");
    let unoriented = world.portal_planes().iter().filter(|p| !p.oriented).count();
    let consumers = ConsumerData::new(&world);
    let (poses, skipped) = poses(&world, hallway);
    eprintln!(
        "[real_map_sweep] {name}: loaded in {load_time:.1?} (full_load={full_load}); cells={} portals={} unoriented_portals={unoriented} \
         lights={} candidates={} id49_cell_map={} poses={} (skipped {skipped} located solid/exterior) cap={}",
        world.cell_count(),
        world.portals.len(),
        consumers.all_lights.len(),
        consumers.candidates.len(),
        consumers.cell_to_cluster.is_some(),
        poses.len(),
        MAX_PORTAL_WALK_STEPS,
    );

    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let mut scratch = PortalWalkScratch::new();
    let mut records = Vec::with_capacity(poses.len());
    let mut failures = Vec::new();
    let sweep_started = Instant::now();
    for pose in &poses {
        let view_proj = camera_view_proj(
            pose.eye,
            pose.yaw_deg.to_radians(),
            pose.pitch_deg.to_radians(),
        );
        let view = WalkView::camera(pose.eye, view_proj);
        let mut nanos = u64::MAX;
        let mut stats = None;
        for _ in 0..2 {
            let t = Instant::now();
            let s = walk_from_camera(input, &view, pose.cell, MAX_PORTAL_WALK_STEPS, &mut scratch);
            nanos = nanos.min(t.elapsed().as_nanos() as u64);
            stats = Some(s);
        }
        let stats = stats.expect("walked");
        let exact: OracleWalk = oracle_from_camera(&world, &[], &view, pose.cell);

        let reached: Vec<usize> = scratch.reached().iter().map(|&c| c as usize).collect();
        let exact_cells: Vec<usize> = (0..exact.visible.len())
            .filter(|&c| exact.visible[c])
            .collect();
        let missed = unclassified_misses(&exact, &scratch);
        let classified = exact_cells
            .iter()
            .filter(|&&c| !exact.clean[c] && !scratch.is_reached(c))
            .count();
        let extra_cells = drawable_count(&world, reached.iter().copied())
            - drawable_count(&world, exact_cells.iter().copied());
        let degree: u32 = reached
            .iter()
            .map(|&c| world.cell_portal_count(c) as u32)
            .sum();
        let worst_expansions = reached
            .iter()
            .map(|&c| scratch.expansions(c))
            .max()
            .unwrap_or(0);
        let consumer_delta = (pose.groups & (SHAFTS | ARENA) != 0).then(|| {
            let clean_cells: Vec<usize> = exact_cells
                .iter()
                .copied()
                .filter(|&c| exact.clean[c])
                .collect();
            let sets = [&reached, &exact_cells, &clean_cells];
            let lights = |roster: &[(Vec3, f32)]| {
                sets.map(|cells| ConsumerData::lights_reaching(&world, roster, cells))
            };
            let clusters = sets.map(|cells| consumers.clusters(&world, cells));
            ConsumerDelta {
                clusters: match clusters {
                    [Some(w), Some(e), Some(c)] => Some([w, e, c]),
                    _ => None,
                },
                candidates: lights(&consumers.candidates),
                all_lights: lights(&consumers.all_lights),
            }
        });
        if stats.step_limit_hit || !missed.is_empty() {
            failures.push(Failure {
                pose: *pose,
                tripped: stats.step_limit_hit,
                considered: stats.considered,
                missed,
            });
        }
        records.push(Record {
            pose: *pose,
            considered: stats.considered,
            tripped: stats.step_limit_hit,
            nanos,
            extra_cells,
            classified,
            oracle_complete: exact.complete(),
            worst_expansions,
            tests_per_degree: stats.considered as f32 / degree.max(1) as f32,
            consumers: consumer_delta,
        });
    }
    eprintln!(
        "[real_map_sweep] {name}: swept {} walks in {:.1?}",
        records.len(),
        sweep_started.elapsed()
    );

    let all: Vec<&Record> = records.iter().collect();
    let base: Vec<&Record> = records
        .iter()
        .filter(|r| r.pose.pitch_deg.abs() < 50.0)
        .collect();
    report_row(&format!("{name} (8 yaws x 0/+45/-45)"), &base, false);
    for pitch in PITCHES_DEG {
        let sel: Vec<&Record> = records
            .iter()
            .filter(|r| r.pose.pitch_deg == pitch)
            .collect();
        report_row(&format!("{name} pitch {pitch:+}"), &sel, false);
    }
    if hallway {
        report_row(&format!("{name} all incl. vertical"), &all, false);
        for (label, mask) in [
            ("shaft A", SHAFT_A),
            ("shaft B", SHAFT_B),
            ("shaft C", SHAFT_C),
            ("arena", ARENA),
        ] {
            let sel: Vec<&Record> = base
                .iter()
                .copied()
                .filter(|r| r.pose.groups & mask != 0)
                .collect();
            report_row(&format!("{name} {label}"), &sel, true);
            if mask & SHAFTS != 0 {
                let vertical: Vec<&Record> = records
                    .iter()
                    .filter(|r| r.pose.groups & mask != 0 && r.pose.pitch_deg.abs() > 50.0)
                    .collect();
                report_row(&format!("{name} {label} straight up/down"), &vertical, true);
            }
        }
    }
    let worst = records.iter().max_by_key(|r| r.considered);
    if let Some(w) = worst {
        eprintln!(
            "  {name}: worst portal tests {} at eye ({:.2}, {:.2}, {:.2}) yaw {} pitch {} cell {}",
            w.considered,
            w.pose.eye.x,
            w.pose.eye.y,
            w.pose.eye.z,
            w.pose.yaw_deg,
            w.pose.pitch_deg,
            w.pose.cell
        );
    }
    for f in &failures {
        let p = f.pose;
        eprintln!(
            "  {name} FAIL: eye ({:.3}, {:.3}, {:.3}) yaw {} pitch {} cell {}: tripped={} tests={} unclassified_missed={:?}",
            p.eye.x,
            p.eye.y,
            p.eye.z,
            p.yaw_deg,
            p.pitch_deg,
            p.cell,
            f.tripped,
            f.considered,
            &f.missed[..f.missed.len().min(24)],
        );
    }
    MapResult {
        worst_tests: worst.map(|w| w.considered).unwrap_or(0),
        failures,
    }
}

/// Cargo runs tests from the crate directory; relative paths name files from
/// the workspace root.
fn resolve(path: &str) -> String {
    let given = Path::new(path);
    if given.is_absolute() || given.exists() {
        return path.to_string();
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(given)
        .to_string_lossy()
        .into_owned()
}

#[test]
#[ignore = "manual R1 sweep over baked PRLs named in POSTRETRO_WALK_SWEEP_PRL"]
fn real_map_sweep() {
    let paths = std::env::var(SWEEP_ENV)
        .unwrap_or_else(|_| panic!("set {SWEEP_ENV} to one .prl path or a comma-separated list"));
    let paths: Vec<&str> = paths
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    assert!(!paths.is_empty(), "{SWEEP_ENV} names no .prl");
    let mut worst_tests = 0;
    let mut trips = 0;
    let mut misses = 0;
    for path in &paths {
        let result = sweep_map(&resolve(path));
        worst_tests = worst_tests.max(result.worst_tests);
        trips += result.failures.iter().filter(|f| f.tripped).count();
        misses += result
            .failures
            .iter()
            .filter(|f| !f.missed.is_empty())
            .count();
    }
    let headroom = MAX_PORTAL_WALK_STEPS as f64 / worst_tests.max(1) as f64;
    eprintln!(
        "[real_map_sweep] over {} map(s): worst portal tests {worst_tests}; cap {MAX_PORTAL_WALK_STEPS} \
         = {headroom:.2}x (needs >= 2x: {}); cap trips {trips}; poses with unclassified misses {misses}",
        paths.len(),
        if headroom >= 2.0 { "ok" } else { "FAIL" },
    );
    assert_eq!(trips, 0, "walks tripped the step cap");
    assert_eq!(
        misses, 0,
        "walks missed cells the exact walk reaches cleanly"
    );
    assert!(
        MAX_PORTAL_WALK_STEPS >= 2 * worst_tests,
        "cap {MAX_PORTAL_WALK_STEPS} needs 2x headroom over {worst_tests}"
    );
}
