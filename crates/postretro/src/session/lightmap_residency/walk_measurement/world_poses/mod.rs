//! World-space pose sweeps over a real PRL, paired across cell partitions:
//! pose record and replay, the ray-grid visibility guard, the bake-gap
//! replay, and structure counts.
//! See: context/lib/rendering_pipeline.md §2, §4

// The residency walks pair two builds frame by frame over one cell layout
// (`trip_record`). A change to the cell partition breaks that pairing: walk
// paths and eye points derive from cell ids. Every measurement here instead
// pairs by world pose alone. The before build writes eye, yaw and pitch per
// pose or frame; the after build replays the same world positions and looks
// every cell up in its own partition. No cell id crosses between builds.
//
// Run from the workspace root on a fresh bake. Each entry is an ignored test
// in the `postretro` bin target:
//
//   cargo test -p postretro --bin postretro <entry> -- --ignored --nocapture
//
// Entries (`POSTRETRO_WORLD_POSES_*` env vars; PRL is always required except
// for `world_poses_diff`):
//
// - `world_poses_record`: writes the pose set to FILE. Standing eye points
//   1.5 m above every floor of a grid of columns over the interior cells'
//   footprint, at yaws 0/90/180/270 and pitch 0. COLUMNS (default 600) caps
//   the grid; IMPORT appends a `measurement/frame-cost/poses.txt`-style list
//   (`label x,y,z,yaw_deg,pitch_deg`, y the pawn's capsule centre). Record
//   once, in the before build; replay the same file in the after build.
// - `world_poses_ray_sweep`: per pose in FILE, the portal walk plus a ray
//   grid from the eye (RAYS = `WxH`, default 160x90, over the walk's camera:
//   100° HFOV at 16:9). Each ray's hit point is pulled 1 cm back toward the
//   eye and located in this build's partition; that cell must be in the
//   walk's reach. Fails on any step-cap trip and any other miss. Reported,
//   not failed: a hit in an unreached cell with no faces, and one the ray
//   entered across a boundary no portal spans (an opening the unchanged
//   portal-area cull removed). With BEFORE (a before build's sweep record),
//   a miss on a ray that also missed before is reported, not failed. OUT
//   writes this run's record.
// - `world_poses_bake_gap`: the bake-gap gate in the completed-reads mode at
//   the default levers. Without BEFORE it materialises the random walk and
//   far-point tour (STEPS, default 2000) exactly as the residency walks do,
//   plus a tour of FILE's poses when given, and runs them; with BEFORE it
//   replays BEFORE's frames instead. Per frame: deferred blocks and their
//   texel area. Fails when a sequence's deferred texel area exceeds
//   BEFORE's, or when BEFORE deferred nothing and this run defers anything.
//   OUT writes this run's record. Camera: the residency walks'
//   `POSTRETRO_LIGHTMAP_WALK_HFOV` / `_ASPECT` / `_PITCH`; planar off.
// - `world_poses_diff`: BEFORE against AFTER, two sweep records or two
//   bake-gap records, without loading a PRL. Same verdicts.
// - `world_poses_structure`: per-map structure counts. REGION =
//   `x0,y0,z0,x1,y1,z1` (engine metres) also lists the interior cells and
//   portals inside that box and groups the portals by plane.

mod bake_gap;
mod poses;
mod ray_sweep;
mod structure;
#[cfg(test)]
mod tests;

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use super::baked_set::bake_identity;

const PRL_ENV: &str = "POSTRETRO_WORLD_POSES_PRL";
const FILE_ENV: &str = "POSTRETRO_WORLD_POSES_FILE";
const IMPORT_ENV: &str = "POSTRETRO_WORLD_POSES_IMPORT";
const COLUMNS_ENV: &str = "POSTRETRO_WORLD_POSES_COLUMNS";
const RAYS_ENV: &str = "POSTRETRO_WORLD_POSES_RAYS";
const OUT_ENV: &str = "POSTRETRO_WORLD_POSES_OUT";
const BEFORE_ENV: &str = "POSTRETRO_WORLD_POSES_BEFORE";
const AFTER_ENV: &str = "POSTRETRO_WORLD_POSES_AFTER";
const STEPS_ENV: &str = "POSTRETRO_WORLD_POSES_STEPS";
const REGION_ENV: &str = "POSTRETRO_WORLD_POSES_REGION";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn required(name: &str) -> String {
    env(name).unwrap_or_else(|| panic!("{name} must be set"))
}

/// Loads the PRL and prints which bake it is.
fn load(path: &str) -> LevelWorld {
    let started = std::time::Instant::now();
    let world = postretro_level_loader::load_prl(path).expect("load PRL");
    println!(
        "PRL: {path}\nloaded in {:.1} s\nbake read: {}",
        started.elapsed().as_secs_f64(),
        bake_identity(path, &world)
    );
    world
}

/// A cell the camera can stand in: neither solid nor exterior.
fn is_interior(world: &LevelWorld, cell: usize) -> bool {
    world
        .cells
        .get(cell)
        .is_some_and(|data| !data.is_solid && !data.is_exterior)
}

/// Union of the interior cells' bounds, or `None` when the level has none.
fn interior_bounds(world: &LevelWorld) -> Option<(Vec3, Vec3)> {
    world
        .cells
        .iter()
        .filter(|data| !data.is_solid && !data.is_exterior)
        .map(|data| (data.bounds_min, data.bounds_max))
        .reduce(|(lo, hi), (min, max)| (lo.min(min), hi.max(max)))
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_WORLD_POSES_PRL and _FILE"]
fn world_poses_record() {
    let path = required(PRL_ENV);
    let file = required(FILE_ENV);
    let max_columns = env(COLUMNS_ENV).map_or(poses::DEFAULT_MAX_COLUMNS, |v| {
        v.trim().parse().expect("COLUMNS is a column count")
    });
    let world = load(&path);
    let mut set = poses::grid_poses(&world, max_columns);
    if let Some(import) = env(IMPORT_ENV) {
        let imported = poses::import_frame_cost(&import).unwrap_or_else(|why| panic!("{why}"));
        set.notes
            .push(format!("imported={import} ({} poses)", imported.len()));
        set.poses.extend(imported);
    }
    set.notes
        .push(format!("bake={}", bake_identity(&path, &world)));
    poses::write_poses(&file, &set).expect("write the pose file");
    for note in &set.notes {
        println!("{note}");
    }
    println!("{} poses written: {file}", set.poses.len());
    assert!(!set.poses.is_empty(), "no standing pose found");
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_WORLD_POSES_PRL and _FILE"]
fn world_poses_ray_sweep() {
    let path = required(PRL_ENV);
    let set = poses::read_poses(&required(FILE_ENV)).unwrap_or_else(|why| panic!("{why}"));
    let rays = env(RAYS_ENV).map_or(ray_sweep::DEFAULT_RAYS, |v| {
        ray_sweep::parse_rays(&v).unwrap_or_else(|| panic!("{RAYS_ENV} = WxH, e.g. 160x90"))
    });
    let world = load(&path);
    let record = ray_sweep::sweep(&world, &bake_identity(&path, &world), &set.poses, rays);
    if let Some(out) = env(OUT_ENV) {
        ray_sweep::write_record(&out, &record).expect("write the sweep record");
        println!("sweep record written: {out}");
    }
    ray_sweep::print_summary("this build", &record);
    let before = env(BEFORE_ENV)
        .map(|file| ray_sweep::read_record(&file).unwrap_or_else(|why| panic!("{why}")));
    let verdict = ray_sweep::verdict(before.as_ref(), &record);
    verdict.print();
    assert!(verdict.passed(), "ray-grid guard failed");
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_WORLD_POSES_PRL"]
fn world_poses_bake_gap() {
    let path = required(PRL_ENV);
    let before = env(BEFORE_ENV).map(|file| {
        let record = bake_gap::read_record(&file).unwrap_or_else(|why| panic!("{why}"));
        println!("replaying {file}: {} sequences", record.sequences.len());
        record
    });
    let world = load(&path);
    let Some(record) = bake_gap::run(&world, &path, before.as_ref()) else {
        println!("this level does not stream its lightmap: the bake-gap guard does not apply");
        return;
    };
    if let Some(out) = env(OUT_ENV) {
        bake_gap::write_record(&out, &record).expect("write the bake-gap record");
        println!("bake-gap record written: {out}");
    }
    let verdict = bake_gap::verdict(before.as_ref(), &record);
    verdict.print();
    assert!(verdict.passed(), "bake-gap guard failed");
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_WORLD_POSES_BEFORE and _AFTER"]
fn world_poses_diff() {
    let before = required(BEFORE_ENV);
    let after = required(AFTER_ENV);
    let passed = if ray_sweep::is_record(&before) {
        let before = ray_sweep::read_record(&before).unwrap_or_else(|why| panic!("{why}"));
        let after = ray_sweep::read_record(&after).unwrap_or_else(|why| panic!("{why}"));
        ray_sweep::print_summary("before", &before);
        ray_sweep::print_summary("after", &after);
        let verdict = ray_sweep::verdict(Some(&before), &after);
        verdict.print();
        verdict.passed()
    } else {
        let before = bake_gap::read_record(&before).unwrap_or_else(|why| panic!("{why}"));
        let after = bake_gap::read_record(&after).unwrap_or_else(|why| panic!("{why}"));
        let verdict = bake_gap::verdict(Some(&before), &after);
        verdict.print();
        verdict.passed()
    };
    assert!(passed, "guard failed");
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_WORLD_POSES_PRL"]
fn world_poses_structure() {
    let path = required(PRL_ENV);
    let region = env(REGION_ENV).map(|v| {
        structure::parse_region(&v)
            .unwrap_or_else(|| panic!("{REGION_ENV} = x0,y0,z0,x1,y1,z1 in engine metres"))
    });
    let world = load(&path);
    structure::print_structure(&world);
    if let Some((lo, hi)) = region {
        structure::print_region(&world, lo, hi);
    }
}
