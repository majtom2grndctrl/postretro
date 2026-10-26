// Real-map mirror sweep: the seeded walk from water seeds on baked PRLs, under the shared cap.
// See: context/lib/rendering_pipeline.md §2, §7.3a
//
// Ignored. Reads the same POSTRETRO_WALK_SWEEP_PRL list as the camera sweep
// and runs in its invocation (this test's name matches the filter):
//
//   POSTRETRO_WALK_SWEEP_PRL=content/dev/maps/reflection-lake-test.prl,content/dev/maps/e24-environment-test.prl,content/dev/maps/reflection-probes-test.prl \
//     cargo test --release -p postretro-visibility real_map_sweep -- --ignored --nocapture --test-threads=1
//
// Water planes: the flat, up-facing (n.y > 0.9999) surface faces of the
// PRL's environment volumes (id 52), grouped by height within 0.01 m, as
// planar selection groups them. Every reflecting fluid counts, probe-bound
// or not, and no area floor applies, so the sweep covers more planes than
// selection can pick. Poses are the camera sweep's; each plane the eye stands
// more than the fog band above gets a mirror pose: the eye reflected across
// the plane, pitch negated, near plane the water plane. Seeds: non-solid,
// non-exterior cells straddling the plane (within the fog band) over its XZ
// bounds, in that pose's camera reach, and inside the mirror frustum. The
// walk and its CPU time (best of 2) run uncropped; `Crop` is where the
// cropped mode composes its view-projection.

use std::time::Instant;

use glam::{DVec3, Mat4, Vec2, Vec3};

use super::real_map_sweep::{Pose, load_world, map_name, pct, poses, sweep_paths};
use super::test_fixtures::camera_view_proj;
use super::{
    MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkView, WalkWorld, seeded_view_planes,
    walk_from_camera, walk_from_seeds,
};
use crate::visibility::{FrustumPlane, is_aabb_outside_planes};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumesSection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::LevelWorld;

/// render-cpu's `FLUID_FOG_BAND`: an eye this close above the water selects
/// no plane, and seed cells straddle the plane within it.
const FLUID_FOG_BAND: f32 = 0.08;
/// A face joins a plane within this height of the plane's first face.
const PLANE_HEIGHT_TOLERANCE: f32 = 0.01;
const UP_FACING_MIN_Y: f64 = 0.9999;

/// One horizontal water plane: every flat up-facing surface face at its
/// height.
pub(super) struct WaterPlane {
    pub(super) h: f32,
    pub(super) xz_min: Vec2,
    pub(super) xz_max: Vec2,
    pub(super) area: f32,
    /// Each face's polygon, for a crop built from the faces in view.
    pub(super) faces: Vec<Vec<Vec3>>,
    /// Non-solid, non-exterior cells straddling the plane over its XZ
    /// bounds: the seed candidates before the per-pose filters.
    seed_cells: Vec<usize>,
}

/// Forms the mirror walk's view-projection for a plane from the camera's
/// view-projection, the mirror eye and the mirror's uncropped
/// view-projection; `None` drops the pose (the plane projects to nothing).
pub(super) type Crop<'a> = &'a dyn Fn(&WaterPlane, Mat4, Vec3, Mat4) -> Option<Mat4>;

/// The uncropped mode: the mirror's view-projection, unchanged.
fn uncropped(_: &WaterPlane, _camera: Mat4, _eye: Vec3, mirror: Mat4) -> Option<Mat4> {
    Some(mirror)
}

/// A flat, up-facing face's height, area and XZ bounds. The normal is
/// Newell's, as render-cpu's `face_normal` computes it.
fn up_facing_face(face: &EnvironmentSurfaceFace) -> Option<(f32, f32, Vec2, Vec2, Vec<Vec3>)> {
    let points: Vec<DVec3> = face
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position).as_dvec3())
        .collect();
    if points.len() < 3 {
        return None;
    }
    let first = points[0];
    let mut newell = DVec3::ZERO;
    for (i, point) in points.iter().enumerate() {
        let a = *point - first;
        let b = points[(i + 1) % points.len()] - first;
        newell.x += (a.y - b.y) * (a.z + b.z);
        newell.y += (a.z - b.z) * (a.x + b.x);
        newell.z += (a.x - b.x) * (a.y + b.y);
    }
    let length = newell.length();
    if !(length.is_finite() && length > 1.0e-9) || newell.y / length <= UP_FACING_MIN_Y {
        return None;
    }
    let lo = points
        .iter()
        .copied()
        .fold(DVec3::splat(f64::INFINITY), DVec3::min);
    let hi = points
        .iter()
        .copied()
        .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
    if hi.y - lo.y > f64::from(PLANE_HEIGHT_TOLERANCE) {
        return None;
    }
    let h = points.iter().map(|p| p.y).sum::<f64>() / points.len() as f64;
    Some((
        h as f32,
        (length * 0.5) as f32,
        Vec2::new(lo.x as f32, lo.z as f32),
        Vec2::new(hi.x as f32, hi.z as f32),
        points.iter().map(|p| p.as_vec3()).collect(),
    ))
}

/// The map's environment volumes: the full loader's, or read straight from
/// the file when only the visibility sections loaded.
fn environment_volumes(
    path: &str,
    world: &LevelWorld,
    full_load: bool,
) -> Option<EnvironmentVolumesSection> {
    if full_load {
        return Some(world.environment_volumes.clone());
    }
    let mut file = std::fs::File::open(path).ok()?;
    let meta = read_container(&mut file).ok()?;
    let data = read_section_data(&mut file, &meta, SectionId::EnvironmentVolumes as u32).ok()??;
    EnvironmentVolumesSection::from_bytes(&data).ok()
}

fn water_planes(world: &LevelWorld, environment: &EnvironmentVolumesSection) -> Vec<WaterPlane> {
    let mut faces: Vec<_> = environment
        .volumes
        .iter()
        .flat_map(|volume| volume.surface_faces.iter())
        .filter_map(up_facing_face)
        .collect();
    faces.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut planes: Vec<WaterPlane> = Vec::new();
    for (h, area, xz_min, xz_max, polygon) in faces {
        match planes.last_mut() {
            Some(plane) if (plane.h - h).abs() <= PLANE_HEIGHT_TOLERANCE => {
                plane.xz_min = plane.xz_min.min(xz_min);
                plane.xz_max = plane.xz_max.max(xz_max);
                plane.area += area;
                plane.faces.push(polygon);
            }
            _ => planes.push(WaterPlane {
                h,
                xz_min,
                xz_max,
                area,
                faces: vec![polygon],
                seed_cells: Vec::new(),
            }),
        }
    }
    for plane in &mut planes {
        plane.seed_cells = (0..world.cell_count())
            .filter(|&c| {
                let cell = &world.cells[c];
                !cell.is_solid
                    && !cell.is_exterior
                    && cell.bounds_max.y >= plane.h - FLUID_FOG_BAND
                    && cell.bounds_min.y <= plane.h + FLUID_FOG_BAND
                    && cell.bounds_max.x >= plane.xz_min.x
                    && cell.bounds_min.x <= plane.xz_max.x
                    && cell.bounds_max.z >= plane.xz_min.y
                    && cell.bounds_min.z <= plane.xz_max.y
            })
            .collect();
    }
    planes
}

/// The water plane, facing the reflected content above it.
fn water_plane(h: f32) -> FrustumPlane {
    FrustumPlane {
        normal: Vec3::Y,
        dist: -h,
    }
}

#[derive(Debug, Clone, Copy)]
struct MirrorRecord {
    pose: Pose,
    plane: usize,
    seeds: usize,
    considered: u32,
    tripped: bool,
    nanos: u64,
    reached: usize,
    /// Drawable reached cells outside the camera walk's reach.
    mirror_only: usize,
}

pub(super) struct MirrorMapResult {
    pub(super) worst_tests: u32,
    pub(super) trips: usize,
    pub(super) walks: usize,
}

fn report_mirror_row(label: &str, records: &[&MirrorRecord]) {
    if records.is_empty() {
        eprintln!("  {label}: no mirror walks");
        return;
    }
    let trips = records.iter().filter(|r| r.tripped).count();
    let mut tests: Vec<u32> = records.iter().map(|r| r.considered).collect();
    let mut micros: Vec<f64> = records.iter().map(|r| r.nanos as f64 / 1000.0).collect();
    let mut seeds: Vec<usize> = records.iter().map(|r| r.seeds).collect();
    let mut reached: Vec<usize> = records.iter().map(|r| r.reached).collect();
    let mut mirror_only: Vec<usize> = records.iter().map(|r| r.mirror_only).collect();
    eprintln!(
        "  {label}: walks={} cap_trips={trips} tests p50/p95/p99/max={}/{}/{}/{} \
         us p50/p99/p99.9/max={:.1}/{:.0}/{:.0}/{:.0} seeds p50/max={}/{} \
         reached p50/max={}/{} mirror_only_drawable p50/p95/max={}/{}/{}",
        records.len(),
        pct(&mut tests, 0.5),
        pct(&mut tests, 0.95),
        pct(&mut tests, 0.99),
        pct(&mut tests, 1.0),
        pct(&mut micros, 0.5),
        pct(&mut micros, 0.99),
        pct(&mut micros, 0.999),
        pct(&mut micros, 1.0),
        pct(&mut seeds, 0.5),
        pct(&mut seeds, 1.0),
        pct(&mut reached, 0.5),
        pct(&mut reached, 1.0),
        pct(&mut mirror_only, 0.5),
        pct(&mut mirror_only, 0.95),
        pct(&mut mirror_only, 1.0),
    );
}

pub(super) fn sweep_mirror_map(path: &str, mode: &str, crop: Crop<'_>) -> MirrorMapResult {
    let (world, full_load) = load_world(path);
    let name = map_name(path);
    let none = MirrorMapResult {
        worst_tests: 0,
        trips: 0,
        walks: 0,
    };
    let Some(environment) = environment_volumes(path, &world, full_load) else {
        eprintln!("[real_map_sweep mirror] {name}: no readable environment volumes; skipped");
        return none;
    };
    let planes = water_planes(&world, &environment);
    if planes.is_empty() {
        eprintln!("[real_map_sweep mirror] {name}: no up-facing fluid surface; skipped");
        return none;
    }
    let (poses, _) = poses(&world, name.starts_with("stress-warren-hallway"));
    for (i, plane) in planes.iter().enumerate() {
        eprintln!(
            "[real_map_sweep mirror] {name}: plane {i} h={:.3} area={:.1} m2 faces={} \
             xz=({:.1}, {:.1})..({:.1}, {:.1}) seed_cells={}",
            plane.h,
            plane.area,
            plane.faces.len(),
            plane.xz_min.x,
            plane.xz_min.y,
            plane.xz_max.x,
            plane.xz_max.y,
            plane.seed_cells.len(),
        );
    }

    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let mut camera_scratch = PortalWalkScratch::new();
    let mut mirror_scratch = PortalWalkScratch::new();
    let mut seeds = Vec::new();
    let mut records = Vec::new();
    let (mut above, mut seedless, mut cropped_away, mut camera_trips) = (0, 0, 0, 0);
    let started = Instant::now();
    for pose in &poses {
        let (yaw, pitch) = (pose.yaw_deg.to_radians(), pose.pitch_deg.to_radians());
        let main_view_proj = camera_view_proj(pose.eye, yaw, pitch);
        let camera = WalkView::camera(pose.eye, main_view_proj);
        // The camera's reach this pose; a trip's partial reach stands in for
        // its drawn set (the camera sweep reports trips; ordinary maps have none).
        let camera_stats = walk_from_camera(
            input,
            &camera,
            pose.cell,
            MAX_PORTAL_WALK_STEPS,
            &mut camera_scratch,
        );
        camera_trips += usize::from(camera_stats.step_limit_hit);
        for (plane_index, plane) in planes.iter().enumerate() {
            if pose.eye.y - plane.h <= FLUID_FOG_BAND {
                continue;
            }
            above += 1;
            let eye = Vec3::new(pose.eye.x, 2.0 * plane.h - pose.eye.y, pose.eye.z);
            let mirror_view_proj = camera_view_proj(eye, yaw, -pitch);
            let Some(view_proj) = crop(plane, main_view_proj, eye, mirror_view_proj) else {
                cropped_away += 1;
                continue;
            };
            let near = water_plane(plane.h);
            let mirror_planes = seeded_view_planes(view_proj, near);
            seeds.clear();
            seeds.extend(plane.seed_cells.iter().copied().filter(|&c| {
                let cell = &world.cells[c];
                camera_scratch.is_reached(c)
                    && !is_aabb_outside_planes(cell.bounds_min, cell.bounds_max, &mirror_planes)
            }));
            if seeds.is_empty() {
                seedless += 1;
                continue;
            }
            let view = WalkView::seeded(eye, view_proj, near);
            let mut nanos = u64::MAX;
            let mut stats = None;
            for _ in 0..2 {
                let t = Instant::now();
                let s = walk_from_seeds(
                    input,
                    &view,
                    &seeds,
                    MAX_PORTAL_WALK_STEPS,
                    &mut mirror_scratch,
                );
                nanos = nanos.min(t.elapsed().as_nanos() as u64);
                stats = Some(s);
            }
            let stats = stats.expect("walked");
            let reached = mirror_scratch.reached();
            let mirror_only = reached
                .iter()
                .filter(|&&c| {
                    world.cells[c as usize].is_drawable && !camera_scratch.is_reached(c as usize)
                })
                .count();
            records.push(MirrorRecord {
                pose: *pose,
                plane: plane_index,
                seeds: seeds.len(),
                considered: stats.considered,
                tripped: stats.step_limit_hit,
                nanos,
                reached: reached.len(),
                mirror_only,
            });
        }
    }
    eprintln!(
        "[real_map_sweep mirror] {name} ({mode}): camera poses={} plane poses above water={above} \
         mirror walks={} (no usable seed {seedless}, cropped away {cropped_away}) \
         camera trips={camera_trips} in {:.1?} cap={MAX_PORTAL_WALK_STEPS}",
        poses.len(),
        records.len(),
        started.elapsed(),
    );

    let all: Vec<&MirrorRecord> = records.iter().collect();
    report_mirror_row(&format!("{name} mirror ({mode})"), &all);
    for (i, plane) in planes.iter().enumerate() {
        let sel: Vec<&MirrorRecord> = records.iter().filter(|r| r.plane == i).collect();
        report_mirror_row(
            &format!("{name} mirror ({mode}) plane {i} h={:.3}", plane.h),
            &sel,
        );
    }
    let worst = records.iter().max_by_key(|r| r.considered);
    if let Some(w) = worst {
        eprintln!(
            "  {name}: worst mirror portal tests {} at camera eye ({:.2}, {:.2}, {:.2}) yaw {} \
             pitch {} cell {} plane {} h={:.3} seeds {}",
            w.considered,
            w.pose.eye.x,
            w.pose.eye.y,
            w.pose.eye.z,
            w.pose.yaw_deg,
            w.pose.pitch_deg,
            w.pose.cell,
            w.plane,
            planes[w.plane].h,
            w.seeds,
        );
    }
    for r in records.iter().filter(|r| r.tripped) {
        eprintln!(
            "  {name} MIRROR TRIP: camera eye ({:.3}, {:.3}, {:.3}) yaw {} pitch {} plane h={:.3} \
             seeds {} tests {}",
            r.pose.eye.x,
            r.pose.eye.y,
            r.pose.eye.z,
            r.pose.yaw_deg,
            r.pose.pitch_deg,
            planes[r.plane].h,
            r.seeds,
            r.considered,
        );
    }
    MirrorMapResult {
        worst_tests: worst.map(|w| w.considered).unwrap_or(0),
        trips: records.iter().filter(|r| r.tripped).count(),
        walks: records.len(),
    }
}

#[test]
#[ignore = "manual mirror sweep over baked PRLs named in POSTRETRO_WALK_SWEEP_PRL"]
fn real_map_sweep_mirror() {
    let paths = sweep_paths();
    let (mut worst_tests, mut trips, mut walks) = (0, 0, 0);
    for path in &paths {
        let result = sweep_mirror_map(path, "uncropped", &uncropped);
        worst_tests = worst_tests.max(result.worst_tests);
        trips += result.trips;
        walks += result.walks;
    }
    let headroom = MAX_PORTAL_WALK_STEPS as f64 / worst_tests.max(1) as f64;
    eprintln!(
        "[real_map_sweep mirror] over {} map(s), uncropped: mirror walks {walks}; worst portal \
         tests {worst_tests}; cap {MAX_PORTAL_WALK_STEPS} = {headroom:.2}x (needs >= 2x: {}); \
         cap trips {trips}",
        paths.len(),
        if headroom >= 2.0 { "ok" } else { "FAIL" },
    );
    if walks == 0 {
        eprintln!("[real_map_sweep mirror] no map named holds a reachable water plane");
    }
    assert_eq!(trips, 0, "mirror walks tripped the step cap");
    assert!(
        MAX_PORTAL_WALK_STEPS >= 2 * worst_tests,
        "cap {MAX_PORTAL_WALK_STEPS} needs 2x headroom over mirror worst {worst_tests}"
    );
}
