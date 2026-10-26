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
// planar selection groups them. Every fluid volume's up-facing faces count,
// probe-bound or not, and no area floor applies, so the sweep covers more
// planes than selection can pick. Poses are the camera sweep's; each plane
// the eye stands more than the fog band above gets a mirror pose: the eye
// reflected across the plane, pitch negated, near plane the water plane
// lowered by the clip bias, as the app's mirror walk takes it. Seeds:
// non-solid, non-exterior cells straddling the plane (within the fog band)
// over one of its faces' XZ bounds, as the app's seed rule takes them, in
// that pose's camera reach, and inside the mirror frustum. Each pose walks
// twice, CPU time best of 2 each: uncropped, and cropped as the
// app crops it (the plane's faces in the camera's view, clipped, projected
// and padded by the crop pad), seeds filtered by the cropped frustum.

use std::time::Instant;

use glam::{DVec3, Mat4, Vec2, Vec3};

use super::real_map_sweep::{Pose, load_world, map_name, pct, poses, sweep_paths};
use super::test_fixtures::camera_view_proj;
use super::{
    MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkView, WalkWorld, seeded_view_planes,
    walk_from_camera, walk_from_seeds,
};
use crate::mirror_crop::{
    MIRROR_CROP_PAD_RADIANS, MirrorCropScratch, crop_mirror_view_proj, mirror_crop_bounds,
};
use crate::mirror_visibility::{MIRROR_CLIP_BIAS, MIRROR_FOG_BAND};
use crate::visibility::{FrustumPlane, frustum_plane_array, is_aabb_outside_planes};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumesSection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::LevelWorld;

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
    /// Each face's XZ bounds, in `faces` order: what the seed rule tests.
    face_xz: Vec<(Vec2, Vec2)>,
    /// Non-solid, non-exterior cells straddling the plane over one of its
    /// faces' XZ bounds: the seed candidates before the per-pose filters.
    seed_cells: Vec<usize>,
}

impl WaterPlane {
    /// The faces whose bounds pass `camera`'s frustum, as the app's crop
    /// takes them: a coplanar pool out of view never widens the crop.
    fn faces_in_view<'a>(
        &'a self,
        camera: &'a [FrustumPlane; 6],
    ) -> impl Iterator<Item = &'a [Vec3]> + 'a {
        self.faces
            .iter()
            .filter(|face| {
                let lo = face.iter().copied().fold(Vec3::INFINITY, Vec3::min);
                let hi = face.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max);
                !is_aabb_outside_planes(lo, hi, camera)
            })
            .map(Vec::as_slice)
    }
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
                plane.face_xz.push((xz_min, xz_max));
            }
            _ => planes.push(WaterPlane {
                h,
                xz_min,
                xz_max,
                area,
                faces: vec![polygon],
                face_xz: vec![(xz_min, xz_max)],
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
                    && cell.bounds_max.y >= plane.h - MIRROR_FOG_BAND
                    && cell.bounds_min.y <= plane.h + MIRROR_FOG_BAND
                    && plane.face_xz.iter().any(|(lo, hi)| {
                        cell.bounds_max.x >= lo.x
                            && cell.bounds_min.x <= hi.x
                            && cell.bounds_max.z >= lo.y
                            && cell.bounds_min.z <= hi.y
                    })
            })
            .collect();
    }
    planes
}

/// The mirror's near plane: the water plane lowered by the clip bias, facing
/// the reflected content above it. The app's mirror walk takes
/// `MirrorCamera`'s clip plane, this same plane, so the sweep measures that
/// walk.
fn water_plane(h: f32) -> FrustumPlane {
    FrustumPlane {
        normal: Vec3::Y,
        dist: -(h - MIRROR_CLIP_BIAS),
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

/// One mode's sweep over a map.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct MirrorModeResult {
    pub(super) worst_tests: u32,
    pub(super) trips: usize,
    pub(super) walks: usize,
}

impl MirrorModeResult {
    fn of(records: &[MirrorRecord]) -> Self {
        Self {
            worst_tests: records.iter().map(|r| r.considered).max().unwrap_or(0),
            trips: records.iter().filter(|r| r.tripped).count(),
            walks: records.len(),
        }
    }
}

pub(super) struct MirrorMapResult {
    pub(super) uncropped: MirrorModeResult,
    pub(super) cropped: MirrorModeResult,
    /// Cropped walks whose plane covers under half the screen, and of those,
    /// the ones that reach fewer cells than the same pose uncropped.
    pub(super) under_half: usize,
    pub(super) under_half_fewer: usize,
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

/// One mirror walk from `seeds` under `view`, timed best of 2; the reach
/// stays in `scratch`.
fn timed_walk(
    input: WalkWorld,
    view: &WalkView,
    seeds: &[usize],
    scratch: &mut PortalWalkScratch,
) -> (super::RegionWalkStats, u64) {
    let mut nanos = u64::MAX;
    let mut stats = None;
    for _ in 0..2 {
        let t = Instant::now();
        stats = Some(walk_from_seeds(
            input,
            view,
            seeds,
            MAX_PORTAL_WALK_STEPS,
            scratch,
        ));
        nanos = nanos.min(t.elapsed().as_nanos() as u64);
    }
    (stats.expect("walked"), nanos)
}

fn report_mode(name: &str, mode: &str, planes: &[WaterPlane], records: &[MirrorRecord]) {
    let all: Vec<&MirrorRecord> = records.iter().collect();
    report_mirror_row(&format!("{name} mirror ({mode})"), &all);
    for (i, plane) in planes.iter().enumerate() {
        let sel: Vec<&MirrorRecord> = records.iter().filter(|r| r.plane == i).collect();
        report_mirror_row(
            &format!("{name} mirror ({mode}) plane {i} h={:.3}", plane.h),
            &sel,
        );
    }
    if let Some(w) = records.iter().max_by_key(|r| r.considered) {
        eprintln!(
            "  {name} ({mode}): worst mirror portal tests {} at camera eye ({:.2}, {:.2}, {:.2}) \
             yaw {} pitch {} cell {} plane {} h={:.3} seeds {}",
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
            "  {name} ({mode}) MIRROR TRIP: camera eye ({:.3}, {:.3}, {:.3}) yaw {} pitch {} \
             plane h={:.3} seeds {} tests {}",
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
}

pub(super) fn sweep_mirror_map(path: &str) -> MirrorMapResult {
    let (world, full_load) = load_world(path);
    let name = map_name(path);
    let none = MirrorMapResult {
        uncropped: MirrorModeResult::default(),
        cropped: MirrorModeResult::default(),
        under_half: 0,
        under_half_fewer: 0,
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
    let mut cropped_scratch = PortalWalkScratch::new();
    let mut crop_scratch = MirrorCropScratch::default();
    let mut seeds = Vec::new();
    let (mut uncropped, mut cropped) = (Vec::new(), Vec::new());
    let (mut above, mut seedless, mut camera_trips) = (0, 0, 0);
    let (mut cropped_away, mut cropped_seedless, mut fewer) = (0, 0, 0);
    let (mut under_half, mut under_half_fewer) = (0, 0);
    // Portal tests and reached cells summed over every uncropped walk, and
    // over the same poses cropped (zero where the crop leaves no walk).
    let (mut tests_sum, mut cropped_tests_sum) = (0u64, 0u64);
    let (mut reached_sum, mut cropped_reached_sum) = (0usize, 0usize);
    let started = Instant::now();
    for pose in &poses {
        let (yaw, pitch) = (pose.yaw_deg.to_radians(), pose.pitch_deg.to_radians());
        let main_view_proj = camera_view_proj(pose.eye, yaw, pitch);
        let camera_planes = frustum_plane_array(main_view_proj);
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
            if pose.eye.y - plane.h <= MIRROR_FOG_BAND {
                continue;
            }
            above += 1;
            let eye = Vec3::new(pose.eye.x, 2.0 * plane.h - pose.eye.y, pose.eye.z);
            let near = water_plane(plane.h);
            let full_view_proj = camera_view_proj(eye, yaw, -pitch);
            let in_view = |view_proj: Mat4, seeds: &mut Vec<usize>| {
                let mirror_planes = seeded_view_planes(view_proj, near);
                seeds.clear();
                seeds.extend(plane.seed_cells.iter().copied().filter(|&c| {
                    let cell = &world.cells[c];
                    camera_scratch.is_reached(c)
                        && !is_aabb_outside_planes(cell.bounds_min, cell.bounds_max, &mirror_planes)
                }));
            };
            in_view(full_view_proj, &mut seeds);
            if seeds.is_empty() {
                seedless += 1;
                continue;
            }
            let view = WalkView::seeded(eye, full_view_proj, near);
            let (stats, nanos) = timed_walk(input, &view, &seeds, &mut mirror_scratch);
            let record = |stats: super::RegionWalkStats, nanos, seeds: usize, reached: &[u32]| {
                MirrorRecord {
                    pose: *pose,
                    plane: plane_index,
                    seeds,
                    considered: stats.considered,
                    tripped: stats.step_limit_hit,
                    nanos,
                    reached: reached.len(),
                    mirror_only: reached
                        .iter()
                        .filter(|&&c| {
                            world.cells[c as usize].is_drawable
                                && !camera_scratch.is_reached(c as usize)
                        })
                        .count(),
                }
            };
            let whole = record(stats, nanos, seeds.len(), mirror_scratch.reached());
            uncropped.push(whole);

            // The cropped mode, from the faces in the camera's view.
            let coverage = mirror_crop_bounds(
                full_view_proj,
                eye,
                plane.faces_in_view(&camera_planes),
                0.0,
                &mut crop_scratch,
            )
            .map_or(0.0, |rect| rect.screen_fraction());
            let crop = crop_mirror_view_proj(
                full_view_proj,
                eye,
                plane.faces_in_view(&camera_planes),
                MIRROR_CROP_PAD_RADIANS,
                &mut crop_scratch,
            );
            let walked = match crop {
                None => {
                    // Nothing of the plane on screen: the app keeps no mirror.
                    cropped_away += 1;
                    None
                }
                Some(view_proj) => {
                    in_view(view_proj, &mut seeds);
                    if seeds.is_empty() {
                        cropped_seedless += 1;
                        None
                    } else {
                        let view = WalkView::seeded(eye, view_proj, near);
                        let (stats, nanos) = timed_walk(input, &view, &seeds, &mut cropped_scratch);
                        let stray: Vec<u32> = cropped_scratch
                            .reached()
                            .iter()
                            .copied()
                            .filter(|&c| !mirror_scratch.is_reached(c as usize))
                            .collect();
                        assert!(
                            stray.is_empty(),
                            "{name}: the cropped mirror reached {stray:?} the uncropped one \
                             does not, at {pose:?} plane h={:.3}",
                            plane.h
                        );
                        let entry = record(stats, nanos, seeds.len(), cropped_scratch.reached());
                        cropped.push(entry);
                        Some(entry)
                    }
                }
            };
            tests_sum += u64::from(whole.considered);
            reached_sum += whole.reached;
            if let Some(entry) = walked {
                cropped_tests_sum += u64::from(entry.considered);
                cropped_reached_sum += entry.reached;
                let cut = entry.reached < whole.reached;
                fewer += usize::from(cut);
                if coverage < 0.5 {
                    under_half += 1;
                    under_half_fewer += usize::from(cut);
                }
            }
        }
    }
    eprintln!(
        "[real_map_sweep mirror] {name}: camera poses={} plane poses above water={above} \
         mirror walks uncropped={} cropped={} (no usable seed {seedless}; cropped: nothing on \
         screen {cropped_away}, no seed in the crop {cropped_seedless}) camera trips={camera_trips} \
         in {:.1?} cap={MAX_PORTAL_WALK_STEPS}",
        poses.len(),
        uncropped.len(),
        cropped.len(),
        started.elapsed(),
    );
    eprintln!(
        "[real_map_sweep mirror] {name}: of {} cropped walks, {fewer} reach fewer cells than \
         uncropped; the plane covers under half the screen at {under_half}, of which \
         {under_half_fewer} reach fewer cells. Summed over the {} uncropped walks: portal \
         tests {tests_sum} -> {cropped_tests_sum} cropped, reached cells {reached_sum} -> \
         {cropped_reached_sum}",
        cropped.len(),
        uncropped.len(),
    );
    report_mode(&name, "uncropped", &planes, &uncropped);
    report_mode(&name, "cropped", &planes, &cropped);
    MirrorMapResult {
        uncropped: MirrorModeResult::of(&uncropped),
        cropped: MirrorModeResult::of(&cropped),
        under_half,
        under_half_fewer,
    }
}

#[test]
#[ignore = "manual mirror sweep over baked PRLs named in POSTRETRO_WALK_SWEEP_PRL"]
fn real_map_sweep_mirror() {
    let paths = sweep_paths();
    let (mut uncropped, mut cropped) = (MirrorModeResult::default(), MirrorModeResult::default());
    let (mut under_half, mut under_half_fewer) = (0, 0);
    for path in &paths {
        let result = sweep_mirror_map(path);
        for (total, map) in [
            (&mut uncropped, result.uncropped),
            (&mut cropped, result.cropped),
        ] {
            total.worst_tests = total.worst_tests.max(map.worst_tests);
            total.trips += map.trips;
            total.walks += map.walks;
        }
        under_half += result.under_half;
        under_half_fewer += result.under_half_fewer;
    }
    for (mode, total) in [("uncropped", uncropped), ("cropped", cropped)] {
        let headroom = MAX_PORTAL_WALK_STEPS as f64 / total.worst_tests.max(1) as f64;
        eprintln!(
            "[real_map_sweep mirror] over {} map(s), {mode}: mirror walks {}; worst portal \
             tests {}; cap {MAX_PORTAL_WALK_STEPS} = {headroom:.2}x (needs >= 2x: {}); cap \
             trips {}",
            paths.len(),
            total.walks,
            total.worst_tests,
            if headroom >= 2.0 { "ok" } else { "FAIL" },
            total.trips,
        );
    }
    eprintln!(
        "[real_map_sweep mirror] plane under half the screen at {under_half} poses; the cropped \
         walk reaches fewer cells at {under_half_fewer}"
    );
    if uncropped.walks == 0 {
        eprintln!("[real_map_sweep mirror] no map named holds a reachable water plane");
    }
    for (mode, total) in [("uncropped", uncropped), ("cropped", cropped)] {
        assert_eq!(total.trips, 0, "{mode} mirror walks tripped the step cap");
        assert!(
            MAX_PORTAL_WALK_STEPS >= 2 * total.worst_tests,
            "cap {MAX_PORTAL_WALK_STEPS} needs 2x headroom over the {mode} mirror worst {}",
            total.worst_tests
        );
    }
}
