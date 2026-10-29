// Behind-the-wall leak guard (C18): sample points in the space behind a
// free-standing wall that no eye in front of it can see, eyes on a lattice in
// front of it, and the camera walk's visible set. A hidden sample in a
// walk-visible cell is a leak.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bvh::aabb::{Aabb, Bounded};
use bvh::bounding_hierarchy::BHShape;
use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::{DVec3, Mat4, Vec3};
use nalgebra::{Point3, Vector3};
use postretro_level_loader::{CellLocatorChild, LevelWorld};
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::classify;
use super::test_fixtures::{MapText, Untiered, compile, engine, load};
use crate::map_data::{BrushVolume, MapData};
use crate::map_format::MapFormat;
use crate::partition::occluder_classify::OccluderVerdict;

// ---- the metric ----

/// Eye yaws, degrees counter-clockwise from Quake +X; 90° faces the wall.
const YAWS_DEG: [f64; 5] = [45.0, 70.0, 90.0, 110.0, 135.0];

/// The engine camera's projection (`crates/postretro/src/camera.rs`): 100°
/// horizontal field of view at 16:9, near 0.1 m, far 4096 m.
const HFOV_DEG: f32 = 100.0;
const ASPECT: f32 = 16.0 / 9.0;
const NEAR_M: f32 = 0.1;
const FAR_M: f32 = 4096.0;

/// Eye height above the floor, Quake units: 1.5 m.
const EYE_Z_Q: f64 = 59.0;

/// Pitch of the cell-centred sample lattice behind the wall, Quake units.
const SAMPLE_STEP_Q: f64 = 32.0;

/// The lattice's nudge off its cell centres along every axis, Quake units.
/// Brush faces sit on whole units, so no sample lies on one and locates
/// ambiguously.
const SAMPLE_NUDGE_Q: f64 = 0.5;

/// Segment ends stay this far off a triangle before it counts as a blocker,
/// metres, so a sample or eye resting on a face does not block itself.
const RAY_EPSILON_M: f32 = 1.0e-3;

/// A free-standing wall on a floor at z = 0, its front face at its low y,
/// with the space behind it sampled out to `far_y`. Quake units.
struct BehindWall {
    wall_min: [f64; 3],
    wall_max: [f64; 3],
    far_y: f64,
    eye_xs: Vec<f64>,
    eye_ys: Vec<f64>,
}

/// One eye pose of the lattice, Quake units and degrees.
#[derive(Clone, Copy, Debug)]
struct Pose {
    eye: [f64; 3],
    yaw_deg: f64,
}

impl BehindWall {
    /// Lattice eyes standing inside a brush: the walk takes its solid-cell
    /// fallback there, so they hold no pose.
    fn eyes_in_solid(&self, brushes: &[BrushVolume]) -> Vec<[f64; 3]> {
        self.lattice_eyes()
            .into_iter()
            .filter(|&eye| inside_any_brush(brushes, engine(eye)))
            .collect()
    }

    fn lattice_eyes(&self) -> Vec<[f64; 3]> {
        let mut eyes = Vec::new();
        for &x in &self.eye_xs {
            for &y in &self.eye_ys {
                eyes.push([x, y, EYE_Z_Q]);
            }
        }
        eyes
    }

    /// The lattice eyes in open space.
    fn eyes(&self, brushes: &[BrushVolume]) -> Vec<[f64; 3]> {
        self.lattice_eyes()
            .into_iter()
            .filter(|&eye| !inside_any_brush(brushes, engine(eye)))
            .collect()
    }

    /// Every open-space eye at every yaw.
    fn poses(&self, brushes: &[BrushVolume]) -> Vec<Pose> {
        self.eyes(brushes)
            .into_iter()
            .flat_map(|eye| YAWS_DEG.map(|yaw_deg| Pose { eye, yaw_deg }))
            .collect()
    }

    /// Cell centres of a `SAMPLE_STEP_Q` lattice, nudged by `SAMPLE_NUDGE_Q`,
    /// over the wall's x span, from its back face to `far_y`, and from the
    /// floor to its top. Every
    /// eye stands within the wall's x span below its top and in front of it,
    /// so a straight line from any eye to any sample crosses the wall's slab
    /// inside its footprint below its top: the samples are hidden by
    /// construction, and the ray check against the compiled geometry
    /// confirms it.
    fn sample_lattice(&self) -> Vec<[f64; 3]> {
        let axis = |lo: f64, hi: f64| -> Vec<f64> {
            let n = ((hi - lo) / SAMPLE_STEP_Q).floor() as usize;
            (0..n)
                .map(|i| lo + (i as f64 + 0.5) * SAMPLE_STEP_Q + SAMPLE_NUDGE_Q)
                .collect()
        };
        let mut samples = Vec::new();
        for x in axis(self.wall_min[0], self.wall_max[0]) {
            for y in axis(self.wall_max[1], self.far_y) {
                for z in axis(0.0, self.wall_max[2]) {
                    samples.push([x, y, z]);
                }
            }
        }
        samples
    }

    fn describe(&self) -> String {
        format!(
            "wall {:?}..{:?}; eyes x {:?} × y {:?}, z {EYE_Z_Q} u, yaws {YAWS_DEG:?}°, \
             {HFOV_DEG}° HFOV at 16:9; samples: {SAMPLE_STEP_Q} u cell-centred lattice, \
             nudged {SAMPLE_NUDGE_Q} u, over x [{}, {}], y [{}, {}], z [0, {}]",
            self.wall_min,
            self.wall_max,
            self.eye_xs,
            self.eye_ys,
            self.wall_min[0],
            self.wall_max[0],
            self.wall_max[1],
            self.far_y,
            self.wall_max[2],
        )
    }
}

/// The hidden samples: lattice points in open space that no eye's segment
/// reaches without crossing a compiled triangle.
struct HiddenSamples {
    lattice: usize,
    /// Lattice points inside a brush: no space to draw, so not counted.
    in_solid: usize,
    /// Lattice points some eye's segment reaches unblocked.
    seen: Vec<[f64; 3]>,
    hidden: Vec<[f64; 3]>,
}

impl HiddenSamples {
    fn of(setup: &BehindWall, brushes: &[BrushVolume], triangles: &Triangles) -> Self {
        let lattice = setup.sample_lattice();
        let eyes: Vec<Vec3> = setup
            .eyes(brushes)
            .into_iter()
            .map(|q| engine(q).as_vec3())
            .collect();
        let (mut in_solid, mut seen, mut hidden) = (0, Vec::new(), Vec::new());
        for &q in &lattice {
            if inside_any_brush(brushes, engine(q)) {
                in_solid += 1;
                continue;
            }
            let point = engine(q).as_vec3();
            if eyes
                .iter()
                .all(|&eye| triangles.segment_blocked(eye, point))
            {
                hidden.push(q);
            } else {
                seen.push(q);
            }
        }
        Self {
            lattice: lattice.len(),
            in_solid,
            seen,
            hidden,
        }
    }

    fn describe(&self) -> String {
        format!(
            "{} lattice points: {} inside brushes, {} seen by some eye (dropped), {} hidden",
            self.lattice,
            self.in_solid,
            self.seen.len(),
            self.hidden.len()
        )
    }
}

fn inside_any_brush(brushes: &[BrushVolume], point: DVec3) -> bool {
    brushes.iter().any(|brush| {
        brush
            .planes
            .iter()
            .all(|plane| point.dot(plane.normal) - plane.distance < 0.0)
    })
}

/// One pose's walk, read against the hidden samples.
struct PoseLeak {
    pose: Pose,
    path: VisibilityPath,
    visible_cells: usize,
    /// Indices into the hidden samples that lie in walk-visible cells.
    leaked: Vec<usize>,
}

/// Every pose's leak on one world, the cell of each hidden sample read from
/// `cell_of`.
fn pose_leaks(
    world: &LevelWorld,
    setup: &BehindWall,
    brushes: &[BrushVolume],
    hidden: &[[f64; 3]],
    cell_of: impl Fn([f64; 3]) -> u32,
) -> Vec<PoseLeak> {
    let sample_cells: Vec<u32> = hidden.iter().map(|&q| cell_of(q)).collect();
    let mut scratch = postretro_visibility::PortalWalkScratch::new();
    setup
        .poses(brushes)
        .into_iter()
        .map(|pose| {
            let (visible, path) = camera_walk(world, pose, &mut scratch);
            let visible_set: BTreeSet<u32> = visible.iter().copied().collect();
            let leaked = (0..hidden.len())
                .filter(|&i| visible_set.contains(&sample_cells[i]))
                .collect();
            PoseLeak {
                pose,
                path,
                visible_cells: visible.len(),
                leaked,
            }
        })
        .collect()
}

/// The camera entry the engine runs each frame, from a level pose with the
/// engine camera's projection.
fn camera_walk(
    world: &LevelWorld,
    pose: Pose,
    scratch: &mut postretro_visibility::PortalWalkScratch,
) -> (Vec<u32>, VisibilityPath) {
    let eye = engine(pose.eye).as_vec3();
    let yaw = pose.yaw_deg.to_radians();
    let dir_q = [yaw.cos(), yaw.sin(), 0.0];
    let dir = (engine(dir_q) - engine([0.0; 3])).as_vec3().normalize();
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
    let vfov = 2.0 * ((HFOV_DEG.to_radians() / 2.0).tan() / ASPECT).atan();
    let projection: Mat4 =
        glam::camera::rh::proj::directx::perspective(vfov, ASPECT, NEAR_M, FAR_M);
    let blocked = vec![false; world.portals.len()];
    let (result, _) = postretro_visibility::determine_visible_cells(
        eye,
        projection * view,
        world,
        &blocked,
        false,
        scratch,
        postretro_visibility::TimingGate::OFF,
    );
    let cells = match result.visible_cells {
        VisibleCells::Culled(ids) => ids,
        VisibleCells::DrawAll => (0..world.cells.len() as u32).collect(),
    };
    (cells, result.stats.path)
}

/// Mean and worst leak fraction over the poses.
fn leak_summary(leaks: &[PoseLeak], hidden: usize) -> (f64, &PoseLeak) {
    let fraction = |leak: &PoseLeak| leak.leaked.len() as f64 / hidden.max(1) as f64;
    let mean = leaks.iter().map(fraction).sum::<f64>() / leaks.len() as f64;
    let worst = leaks
        .iter()
        .max_by_key(|leak| leak.leaked.len())
        .expect("at least one pose");
    (mean, worst)
}

// ---- compiled geometry ----

/// One compiled world triangle, engine space.
struct Triangle {
    corners: [Vec3; 3],
    node: usize,
}

impl Bounded<f32, 3> for Triangle {
    fn aabb(&self) -> Aabb<f32, 3> {
        let [a, b, c] = self.corners;
        let (min, max) = (a.min(b).min(c), a.max(b).max(c));
        Aabb::with_bounds(
            Point3::new(min.x, min.y, min.z),
            Point3::new(max.x, max.y, max.z),
        )
    }
}

impl BHShape<f32, 3> for Triangle {
    fn set_bh_node_index(&mut self, index: usize) {
        self.node = index;
    }

    fn bh_node_index(&self) -> usize {
        self.node
    }
}

/// The world's static triangles, as the PRL carries them, under a BVH.
struct Triangles {
    triangles: Vec<Triangle>,
    bvh: Bvh<f32, 3>,
}

impl Triangles {
    fn of(world: &LevelWorld) -> Self {
        let mut triangles: Vec<Triangle> = world
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|tri| Triangle {
                corners: tri.map(|i| Vec3::from(world.vertices[i as usize].position)),
                node: 0,
            })
            .collect();
        let bvh = Bvh::build(&mut triangles);
        Self { triangles, bvh }
    }

    /// True when a triangle crosses the open segment `from`..`to`.
    fn segment_blocked(&self, from: Vec3, to: Vec3) -> bool {
        let delta = to - from;
        let length = delta.length();
        let direction = delta / length;
        let ray = Ray::new(
            Point3::new(from.x, from.y, from.z),
            Vector3::new(direction.x, direction.y, direction.z),
        );
        self.bvh
            .traverse(&ray, &self.triangles)
            .into_iter()
            .any(|triangle| {
                ray_triangle_distance(from, direction, triangle.corners)
                    .is_some_and(|t| t > RAY_EPSILON_M && t < length - RAY_EPSILON_M)
            })
    }
}

/// Möller–Trumbore, both faces, edges inclusive: a segment through a shared
/// edge of two fragments of one face hits it.
fn ray_triangle_distance(origin: Vec3, direction: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let (edge_one, edge_two) = (b - a, c - a);
    let h = direction.cross(edge_two);
    let determinant = edge_one.dot(h);
    if determinant.abs() < 1.0e-12 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let s = origin - a;
    let u = inverse * s.dot(h);
    const EDGE: f32 = 1.0e-5;
    if !(-EDGE..=1.0 + EDGE).contains(&u) {
        return None;
    }
    let q = s.cross(edge_one);
    let v = inverse * direction.dot(q);
    if v < -EDGE || u + v > 1.0 + EDGE {
        return None;
    }
    Some(inverse * edge_two.dot(q))
}

// ---- the leaking cells ----

/// Engine bounds as Quake-unit `(min, max)`.
fn quake_bounds(min: Vec3, max: Vec3) -> ([f64; 3], [f64; 3]) {
    let s = MapFormat::IdTech2.units_to_meters();
    let q = |e: Vec3| -> DVec3 {
        let e = e.as_dvec3() / s;
        DVec3::new(-e.z, -e.x, e.y)
    };
    let (a, b) = (q(min), q(max));
    let round = |v: DVec3| v.to_array().map(|c| (c * 10.0).round() / 10.0);
    (round(a.min(b)), round(a.max(b)))
}

/// A locator plane in Quake terms, `x=1024`-style when axis-aligned, with the
/// brushes that have a face on it and any cap world-grid line it lies on.
fn describe_plane(normal: Vec3, distance: f32, brushes: &[BrushVolume]) -> String {
    let s = MapFormat::IdTech2.units_to_meters();
    let n_e = normal.as_dvec3();
    let d_e = f64::from(distance);
    // Engine (x, y, z) = s·(-qy, qz, -qx): n_e·p_e = d_e becomes n_q·p_q = d_e / s.
    let n_q = DVec3::new(-n_e.z, -n_e.x, n_e.y);
    let d_q = d_e / s;
    let axis = (0..3).find(|&axis| (n_q[axis].abs() - 1.0).abs() < 1e-6);
    let mut text = match axis {
        Some(axis) => format!(
            "{}={}",
            ["x", "y", "z"][axis],
            (d_q * n_q[axis].signum() * 10.0).round() / 10.0
        ),
        None => format!("{:.3?}·p={:.1}", n_q.to_array(), d_q),
    };
    let owners: Vec<usize> = brushes
        .iter()
        .enumerate()
        .filter(|(_, brush)| {
            brush.planes.iter().any(|plane| {
                let same =
                    (plane.normal - n_e).length() < 1e-6 && (plane.distance - d_e).abs() < 1e-4;
                let flipped =
                    (plane.normal + n_e).length() < 1e-6 && (plane.distance + d_e).abs() < 1e-4;
                same || flipped
            })
        })
        .map(|(index, _)| index)
        .collect();
    // The lowest-numbered few name the plane; a floor or ceiling plane is
    // shared by most of the map.
    const LISTED: usize = 6;
    match owners.len() {
        0 => {}
        n if n <= LISTED => text.push_str(&format!(" brushes {owners:?}")),
        n => text.push_str(&format!(" brushes {:?} +{}", &owners[..LISTED], n - LISTED)),
    }
    if let Some(axis) = axis.filter(|&axis| axis < 2) {
        let coordinate = (d_q * n_q[axis].signum()).round();
        for grid in [768.0, 1024.0] {
            if (d_q.abs() - coordinate.abs()).abs() < 1e-3 && coordinate % grid == 0.0 {
                text.push_str(&format!(" grid{grid}"));
            }
        }
    }
    text
}

/// Each cell's locator paths from the root: (node, took the front child).
fn locator_paths(world: &LevelWorld) -> BTreeMap<usize, Vec<Vec<(usize, bool)>>> {
    let mut paths: BTreeMap<usize, Vec<Vec<(usize, bool)>>> = BTreeMap::new();
    let mut stack = vec![(world.cell_locator_root, Vec::new())];
    while let Some((child, path)) = stack.pop() {
        match child {
            CellLocatorChild::Cell(cell) => paths.entry(cell).or_default().push(path),
            CellLocatorChild::Node(node) => {
                let data = &world.cell_locator_nodes[node];
                let mut front = path.clone();
                front.push((node, true));
                let mut back = path;
                back.push((node, false));
                stack.push((data.front, front));
                stack.push((data.back, back));
            }
        }
    }
    paths
}

/// The planes bounding `cell`: those on the locator path down to the deepest
/// node holding every leaf that names it, kept when the cell's bounds touch
/// them. Listed in descent order, root first.
fn bounding_planes(
    world: &LevelWorld,
    paths: &BTreeMap<usize, Vec<Vec<(usize, bool)>>>,
    cell: usize,
    brushes: &[BrushVolume],
) -> Vec<String> {
    let Some(cell_paths) = paths.get(&cell) else {
        return Vec::new();
    };
    let first = &cell_paths[0];
    let common = (0..first.len())
        .take_while(|&depth| {
            cell_paths
                .iter()
                .all(|path| path.get(depth) == Some(&first[depth]))
        })
        .count();
    let data = &world.cells[cell];
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 {
                    data.bounds_min.x
                } else {
                    data.bounds_max.x
                },
                if i & 2 == 0 {
                    data.bounds_min.y
                } else {
                    data.bounds_max.y
                },
                if i & 4 == 0 {
                    data.bounds_min.z
                } else {
                    data.bounds_max.z
                },
            )
        })
        .collect();
    first[..common]
        .iter()
        .filter_map(|&(node, _)| {
            let plane = &world.cell_locator_nodes[node];
            let side = |p: &Vec3| plane.plane_normal.dot(*p) - plane.plane_distance;
            let (lo, hi) = corners
                .iter()
                .map(side)
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), d| {
                    (lo.min(d), hi.max(d))
                });
            (lo.abs() < 1e-3 || hi.abs() < 1e-3)
                .then(|| describe_plane(plane.plane_normal, plane.plane_distance, brushes))
        })
        .collect()
}

/// Report one build's leak: mean and worst pose, every pose's count, and each
/// leaking cell's bounds, bounding planes and leak.
fn report(
    name: &str,
    world: &LevelWorld,
    leaks: &[PoseLeak],
    hidden: &[[f64; 3]],
    brushes: &[BrushVolume],
) -> String {
    let (mean, worst) = leak_summary(leaks, hidden.len());
    let mut out = format!(
        "== {name}: {} cells, {} portals\nleak mean {:.2}% over {} poses; worst {:.2}% \
         ({} of {} samples) at eye {:?} yaw {}°\n",
        world.cells.len(),
        world.portals.len(),
        100.0 * mean,
        leaks.len(),
        100.0 * worst.leaked.len() as f64 / hidden.len().max(1) as f64,
        worst.leaked.len(),
        hidden.len(),
        worst.pose.eye,
        worst.pose.yaw_deg,
    );
    let fallbacks: Vec<String> = leaks
        .iter()
        .filter(|leak| !matches!(leak.path, VisibilityPath::PrlPortal { .. }))
        .map(|leak| format!("{:?}@{}: {:?}", leak.pose.eye, leak.pose.yaw_deg, leak.path))
        .collect();
    out.push_str(&format!("non-portal paths: {fallbacks:?}\n"));
    out.push_str("per pose (eye x, eye y, yaw: leaked samples / visible cells):\n");
    for leak in leaks {
        out.push_str(&format!(
            "  {} {} {}: {} / {}\n",
            leak.pose.eye[0],
            leak.pose.eye[1],
            leak.pose.yaw_deg,
            leak.leaked.len(),
            leak.visible_cells
        ));
    }

    // Leaking cells over all poses: samples held, poses leaking them.
    let mut cells: BTreeMap<usize, (BTreeSet<usize>, usize)> = BTreeMap::new();
    for leak in leaks {
        let mut posed = BTreeSet::new();
        for &i in &leak.leaked {
            let cell = world.locate_cell(engine(hidden[i]).as_vec3());
            let entry = cells.entry(cell).or_default();
            entry.0.insert(i);
            if posed.insert(cell) {
                entry.1 += 1;
            }
        }
    }
    let worst_cells: BTreeSet<usize> = worst
        .leaked
        .iter()
        .map(|&i| world.locate_cell(engine(hidden[i]).as_vec3()))
        .collect();
    let paths = locator_paths(world);
    out.push_str(&format!(
        "leaking cells ({}; * = leaks at the worst pose):\n",
        cells.len()
    ));
    for (cell, (samples, poses)) in &cells {
        let data = &world.cells[*cell];
        let (min, max) = quake_bounds(data.bounds_min, data.bounds_max);
        out.push_str(&format!(
            "  {}cell {cell}: Quake bounds {min:?}..{max:?}; {} hidden samples, leaks at {poses} \
             poses\n    planes: {:?}\n",
            if worst_cells.contains(cell) { "*" } else { " " },
            samples.len(),
            bounding_planes(world, &paths, *cell, brushes),
        ));
    }

    // Every cell holding hidden samples, leaking or not: what bounds the
    // space behind the wall.
    let mut holding: BTreeMap<usize, usize> = BTreeMap::new();
    for &q in hidden {
        *holding
            .entry(world.locate_cell(engine(q).as_vec3()))
            .or_default() += 1;
    }
    out.push_str(&format!(
        "cells holding hidden samples ({}):\n",
        holding.len()
    ));
    for (cell, count) in &holding {
        let data = &world.cells[*cell];
        let (min, max) = quake_bounds(data.bounds_min, data.bounds_max);
        out.push_str(&format!(
            "  cell {cell}: Quake bounds {min:?}..{max:?}; {count} hidden samples\n    planes: \
             {:?}\n",
            bounding_planes(world, &paths, *cell, brushes),
        ));
    }
    out
}

// ---- the map sweep ----

/// occlusion-test.map's retaining wall (brush 37), its hall's far wall face,
/// and the review's eye lattice.
fn occlusion_test_setup() -> BehindWall {
    BehindWall {
        wall_min: [0.0, 1184.0, 0.0],
        wall_max: [1024.0, 1216.0, 256.0],
        far_y: 2720.0,
        eye_xs: vec![64.0, 300.0, 544.0, 800.0, 980.0],
        eye_ys: vec![700.0, 900.0, 1100.0],
    }
}

fn occlusion_test_map() -> MapData {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/maps/occlusion-test.map");
    crate::parse::parse_map_file(&path, MapFormat::IdTech2).expect("occlusion-test.map parses")
}

/// Task 9h: the review's leak metric on real occlusion-test bakes. The test
/// runs in the crate directory, so the PRL paths are absolute:
/// `POSTRETRO_LEAK_SWEEP_PRL=</abs/a.prl,/abs/b.prl> cargo test --release -p
/// postretro-level-compiler --bin prl-build behind_wall_leak_sweep -- --ignored
/// --nocapture`.
#[test]
#[ignore = "on-demand sweep over baked PRLs named by POSTRETRO_LEAK_SWEEP_PRL"]
fn behind_wall_leak_sweep_reports_hidden_samples_in_walk_visible_cells() {
    let prls = std::env::var("POSTRETRO_LEAK_SWEEP_PRL")
        .expect("POSTRETRO_LEAK_SWEEP_PRL names comma-separated occlusion-test PRLs");
    let map = occlusion_test_map();
    let brushes = &map.brush_volumes;
    let setup = occlusion_test_setup();
    let wall = engine([512.0, 1200.0, 128.0]);
    assert!(
        inside_any_brush(&brushes[37..38], wall),
        "brush 37 is the retaining wall"
    );
    let verdicts = classify(brushes).expect("classifies").verdicts;
    println!("setup: {}", setup.describe());
    println!(
        "eyes inside a brush, dropped with their yaws: {:?}",
        setup.eyes_in_solid(brushes)
    );
    println!(
        "HEAD classification: b37 {:?}, b102 {:?}, yellow room b113..=b125 {:?}",
        verdicts[37],
        verdicts[102],
        &verdicts[113..=125]
    );

    for prl in prls.split(',') {
        let world = load(Path::new(prl)).expect("PRL loads");
        let triangles = Triangles::of(&world);
        let samples = HiddenSamples::of(&setup, brushes, &triangles);
        println!(
            "\n{prl}: {} triangles; {}",
            triangles.triangles.len(),
            samples.describe()
        );
        if !samples.seen.is_empty() {
            println!("  seen samples: {:?}", samples.seen);
        }
        let leaks = pose_leaks(&world, &setup, brushes, &samples.hidden, |q| {
            world.locate_cell(engine(q).as_vec3()) as u32
        });
        print!("{}", report(prl, &world, &leaks, &samples.hidden, brushes));
    }
}

// ---- the fixture row ----

/// Retaining wall of `laned_wall_hall`, 26 × 6.5 m, as occlusion-test's.
/// Its ends sit off the cap's 1024 u world grid, so no cap line lands on them
/// by accident and only the partition's own planes can bound the space behind
/// it.
const LANED_WALL_MIN: [f64; 3] = [256.0, 1184.0, 0.0];
const LANED_WALL_MAX: [f64; 3] = [1280.0, 1216.0, 256.0];

/// The second occluder: occlusion-test's brush 102, a full-height wall 100 u
/// thick and 1000 u (25.4 m) long standing in the left lane behind the
/// retaining wall and running through the hall's far wall. Its near end face,
/// extended across the hall, crosses the space behind the retaining wall.
const SECOND_OCCLUDER_MIN: [f64; 3] = [-144.0, 1984.0, 0.0];
const SECOND_OCCLUDER_MAX: [f64; 3] = [-44.0, 2984.0, 512.0];

/// Hall interior, Quake units: x −512..2048, y 672..2720, z 0..512 (65 × 52 ×
/// 13 m), so a 19.5 m lane runs past each end of the retaining wall. Wider
/// than deep, the wall's back face slices more of the hall than its end planes
/// do, so only the back face's shadow prism carries the end planes behind it;
/// without prisms the same layout in a hall deeper than wide passes and this
/// one leaks (task 9h).
const LANED_HALL_MIN: [f64; 3] = [-512.0, 672.0, 0.0];
const LANED_HALL_MAX: [f64; 3] = [2048.0, 2720.0, 512.0];

/// A hall crossed by a free-standing retaining wall with a lane past each
/// end, and a brush-102 stand-in behind it in the left lane.
fn laned_wall_hall() -> MapText {
    let (t, [x0, y0, z0], [x1, y1, z1]) = (16.0, LANED_HALL_MIN, LANED_HALL_MAX);
    let mut map = MapText::new();
    map.solid([x0 - t, y0 - t, z0 - t], [x1 + t, y1 + t, z0])
        .solid([x0 - t, y0 - t, z1], [x1 + t, y1 + t, z1 + t])
        .solid([x0 - t, y0 - t, z0], [x0, y1 + t, z1])
        .solid([x1, y0 - t, z0], [x1 + t, y1 + t, z1])
        .solid([x0, y0 - t, z0], [x1, y0, z1])
        .solid([x0, y1, z0], [x1, y1 + t, z1])
        .solid(LANED_WALL_MIN, LANED_WALL_MAX)
        .solid(SECOND_OCCLUDER_MIN, SECOND_OCCLUDER_MAX)
        .light([768.0, 900.0, 400.0], 2000.0);
    map
}

/// Index of the retaining wall and of the second occluder in
/// `laned_wall_hall`'s brushes.
const LANED_WALL: usize = 6;
const SECOND_OCCLUDER: usize = 7;

/// The review's eye lattice, carried with the wall: 64 to 980 u along it,
/// 84 to 484 u in front of it.
fn laned_wall_setup() -> BehindWall {
    let x0 = LANED_WALL_MIN[0];
    BehindWall {
        wall_min: LANED_WALL_MIN,
        wall_max: LANED_WALL_MAX,
        far_y: LANED_HALL_MAX[1],
        eye_xs: [64.0, 300.0, 544.0, 800.0, 980.0]
            .iter()
            .map(|x| x0 + x)
            .collect(),
        eye_ys: vec![700.0, 900.0, 1100.0],
    }
}

// Regression: the wall's back face split before its end planes, so the end
// planes never reached the space behind it; cap grid lines bounded that
// space inside the lanes, and lane eyes saw its cells (task 9h:
// measurement/9h/README.md). The back face's shadow prism now carries the
// wall's free end planes into that space.
#[test]
fn walk_from_in_front_of_a_laned_wall_draws_no_cell_holding_space_hidden_behind_it() {
    let map = laned_wall_hall();
    let map_data = map.parse();
    let verdicts = classify(&map_data.brush_volumes)
        .expect("classifies")
        .verdicts;
    assert_eq!(verdicts[LANED_WALL], OccluderVerdict::Occludes);
    assert_eq!(verdicts[SECOND_OCCLUDER], OccluderVerdict::Occludes);

    let setup = laned_wall_setup();
    // Ten times the default texel pitch: the row reads no texel.
    let compiled = compile(&map, &["--lightmap-density", "0.25"]);
    let world = &compiled.world;
    let triangles = Triangles::of(world);
    let samples = HiddenSamples::of(&setup, &map_data.brush_volumes, &triangles);
    assert!(
        samples.seen.is_empty(),
        "every lattice point behind the wall is hidden by construction: {:?}",
        samples.seen
    );
    assert!(!samples.hidden.is_empty());

    let leaks = pose_leaks(
        world,
        &setup,
        &map_data.brush_volumes,
        &samples.hidden,
        |q| world.locate_cell(engine(q).as_vec3()) as u32,
    );
    let text = report(
        "laned wall hall",
        world,
        &leaks,
        &samples.hidden,
        &map_data.brush_volumes,
    );

    // Today's build, for the record.
    let before = Untiered::of(&map_data);
    let before_world = before.visibility_world();
    let before_leaks = pose_leaks(
        &before_world,
        &setup,
        &map_data.brush_volumes,
        &samples.hidden,
        |q| before.cell_at(q) as u32,
    );
    let (before_mean, before_worst) = leak_summary(&before_leaks, samples.hidden.len());
    println!(
        "setup: {}\n{}\n{text}before: mean {:.2}%, worst {} samples at {:?} yaw {}°",
        setup.describe(),
        samples.describe(),
        100.0 * before_mean,
        before_worst.leaked.len(),
        before_worst.pose.eye,
        before_worst.pose.yaw_deg,
    );

    for leak in &leaks {
        assert!(
            matches!(leak.path, VisibilityPath::PrlPortal { .. }),
            "{:?}: {:?}",
            leak.pose,
            leak.path
        );
    }
    let leaked: usize = leaks.iter().map(|leak| leak.leaked.len()).sum();
    assert_eq!(
        leaked, 0,
        "walk-visible cells hold samples hidden behind the wall:\n{text}"
    );
}
