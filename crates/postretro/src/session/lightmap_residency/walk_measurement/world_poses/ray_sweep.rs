//! The ray-grid visibility guard: every cell a ray from the eye reaches is in
//! the walk's reach, and no pose trips the step cap; plus per-pose walk,
//! draw and residency counts.
//! See: context/lib/rendering_pipeline.md §2

use std::collections::HashSet;
use std::fmt::Write as _;

use glam::Vec3;
use postretro_level_loader::LevelWorld;
use postretro_physics::collision::{CollisionWorld, cast_ray};
use postretro_visibility::{
    Frustum, PortalWalkScratch, TimingGate, VisibilityPath, VisibilityStage, VisibleCells,
    determine_visible_cells,
};

use super::super::modes::{Projection, render_camera};
use super::is_interior;
use super::poses::{WorldPose, format_pose, parse_pose};

const MAGIC: &str = "postretro-world-pose-sweep 1";
pub(super) const DEFAULT_RAYS: (u32, u32) = (160, 90);
/// A hit point moves this far back toward the eye before it is located, so
/// it lands in the open cell in front of the surface, metres.
const HIT_PULLBACK: f32 = 0.01;
/// `render_camera`'s near and far planes.
const NEAR: f32 = 0.1;
const FAR: f32 = 4096.0;
/// Misses listed per pose in the record; the count is always exact.
const MISSES_KEPT: usize = 64;
/// Locator step along a missed ray when finding the boundary it crossed,
/// metres: finer than the thinnest sliver cell a sweep has met.
const MARCH_STEP: f32 = 0.005;

/// One pose's sweep: its pose, `key=value` counts in a fixed order, and the
/// rays whose located hit cell the walk did not reach.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PoseRow {
    pub(super) pose: WorldPose,
    pub(super) fields: Vec<(String, String)>,
    /// Up to `MISSES_KEPT` rays of each kind; the counts are exact.
    pub(super) misses: Vec<RayMiss>,
}

/// Why a located hit cell was outside the walk's reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissKind {
    /// A drawable cell the walk missed although every boundary the ray
    /// crossed carries a portal: the guard's failure.
    Drawn,
    /// A drawable cell the ray entered across a boundary no portal spans:
    /// an opening the unchanged portal-area cull removed. Reported.
    Culled,
    /// A cell with no faces, so nothing of it draws. Reported.
    Faceless,
}

const KINDS: [MissKind; 3] = [MissKind::Drawn, MissKind::Culled, MissKind::Faceless];

impl MissKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Drawn => "drawn",
            Self::Culled => "culled",
            Self::Faceless => "faceless",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        KINDS.into_iter().find(|kind| kind.label() == label)
    }

    /// The pose field holding this kind's exact count.
    fn field(self) -> &'static str {
        match self {
            Self::Drawn => "misses",
            Self::Culled => "culled_misses",
            Self::Faceless => "faceless_misses",
        }
    }
}

/// One ray whose located hit cell the walk did not reach.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RayMiss {
    /// Row-major over the grid.
    pub(super) ray: u32,
    /// The located hit point and its cell, for triage.
    pub(super) point: Vec3,
    pub(super) cell: u32,
    pub(super) kind: MissKind,
}

impl PoseRow {
    pub(super) fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.field(key)?.parse().ok()
    }

    pub(super) fn tripped(&self) -> bool {
        self.field("path") == Some("step_limit")
    }

    pub(super) fn miss_count(&self, kind: MissKind) -> u64 {
        self.number(kind.field()).map_or(0, |n| n as u64)
    }

    fn missed_ray(&self, ray: u32) -> bool {
        self.misses.iter().any(|miss| miss.ray == ray)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SweepRecord {
    pub(super) bake: String,
    /// Ray grid and camera: a pair must match it.
    pub(super) camera: String,
    pub(super) rows: Vec<PoseRow>,
}

pub(super) fn parse_rays(value: &str) -> Option<(u32, u32)> {
    let (w, h) = value.trim().split_once('x')?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Per-level lookups the sweep reads at every pose.
struct Level<'w> {
    world: &'w LevelWorld,
    collision: CollisionWorld,
    /// Per cell, its lightmap blocks and their texel area.
    lightmap: Vec<(u32, u64)>,
    /// Per cell, its SH cluster, when the level has a cluster directory.
    sh_cluster: Option<Vec<u32>>,
    /// Every portal's cell pair, lower id first.
    portal_pairs: HashSet<(u32, u32)>,
}

impl<'w> Level<'w> {
    fn new(world: &'w LevelWorld) -> Self {
        let mut lightmap = vec![(0u32, 0u64); world.cells.len()];
        if let Some(index) = world.lightmap.as_ref() {
            for record in &index.records {
                if let Some(cell) = lightmap.get_mut(record.cell_id as usize) {
                    cell.0 += 1;
                    cell.1 += u64::from(record.width) * u64::from(record.height);
                }
            }
        }
        let sh_cluster = world.cluster_directory().map(|directory| {
            let mut of_cell = vec![u32::MAX; world.cells.len()];
            for (id, cluster) in directory.clusters.iter().enumerate() {
                let start = cluster.member_start as usize;
                for &cell in &directory.members[start..start + cluster.member_count as usize] {
                    if let Some(slot) = of_cell.get_mut(cell as usize) {
                        *slot = id as u32;
                    }
                }
            }
            of_cell
        });
        let portal_pairs = world
            .portals
            .iter()
            .map(|p| {
                let (a, b) = (p.front_cell as u32, p.back_cell as u32);
                (a.min(b), a.max(b))
            })
            .collect();
        Self {
            world,
            collision: CollisionWorld::from_level(world).expect("static collision mesh"),
            lightmap,
            sh_cluster,
            portal_pairs,
        }
    }
}

/// Sweeps every pose, spread over the machine's cores; rows keep pose order.
pub(super) fn sweep(
    world: &LevelWorld,
    bake: &str,
    poses: &[WorldPose],
    rays: (u32, u32),
) -> SweepRecord {
    let started = std::time::Instant::now();
    let level = Level::new(world);
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(poses.len().max(1));
    let mut rows: Vec<Option<PoseRow>> = vec![None; poses.len()];
    std::thread::scope(|scope| {
        let level = &level;
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                scope.spawn(move || {
                    let mut scratch = PortalWalkScratch::new();
                    (worker..poses.len())
                        .step_by(workers)
                        .map(|i| (i, sweep_pose(level, &poses[i], rays, &mut scratch)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (i, row) in handle.join().expect("sweep worker") {
                rows[i] = Some(row);
            }
        }
    });
    println!(
        "swept {} poses x {}x{} rays on {workers} threads in {:.1} s",
        poses.len(),
        rays.0,
        rays.1,
        started.elapsed().as_secs_f64()
    );
    SweepRecord {
        bake: bake.to_string(),
        camera: camera_label(rays),
        rows: rows.into_iter().map(|row| row.expect("swept")).collect(),
    }
}

fn camera_label(rays: (u32, u32)) -> String {
    format!(
        "rays {}x{} over {}, near {NEAR} m, far {FAR} m, hit pullback {HIT_PULLBACK} m",
        rays.0,
        rays.1,
        Projection::default().label()
    )
}

fn sweep_pose(
    level: &Level<'_>,
    pose: &WorldPose,
    rays: (u32, u32),
    scratch: &mut PortalWalkScratch,
) -> PoseRow {
    let world = level.world;
    let projection = Projection::default();
    let camera = render_camera(pose.eye, pose.yaw(), pose.pitch(), projection);
    let (visibility, frustum) = determine_visible_cells(
        pose.eye,
        camera.view_projection,
        world,
        &[],
        false,
        scratch,
        TimingGate::ON,
    );
    let stats = &visibility.stats;
    let cpu = |stage| stats.cpu.value(stage).unwrap_or(0);
    let path = match stats.path {
        VisibilityPath::PrlPortal { .. } => "portal",
        VisibilityPath::PortalStepLimitFallback { .. } => "step_limit",
        VisibilityPath::SolidCellFallback => "solid",
        VisibilityPath::ExteriorCellFallback => "exterior",
        VisibilityPath::NoPortalsFallback => "no_portals",
        VisibilityPath::EmptyWorldFallback => "empty",
    };
    let drawn: &[u32] = match &visibility.visible_cells {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => &[],
    };
    let reach = &visibility.fog_reachable;
    let camera_cell = stats.camera_cell;

    let mut fields = Vec::new();
    let mut put = |key: &str, value: String| fields.push((key.to_string(), value));
    put("path", path.to_string());
    put("camera_cell", camera_cell.to_string());
    put("considered", cpu(VisibilityStage::Considered).to_string());
    put("accepted", cpu(VisibilityStage::Accepted).to_string());
    put("drawn", drawn.len().to_string());
    put("reach", reach.len().to_string());
    put("drawn_faces", stats.drawn_faces.to_string());
    let (tris, tris_in_frustum) = triangles(world, drawn, &frustum);
    put("tris", tris.to_string());
    put("tris_in_frustum", tris_in_frustum.to_string());
    let (blocks, texels) = drawn.iter().fold((0u64, 0u64), |(b, t), &cell| {
        let (blocks, texels) = level.lightmap[cell as usize];
        (b + u64::from(blocks), t + texels)
    });
    put("lightmap_blocks", blocks.to_string());
    put("lightmap_texels", texels.to_string());
    if let Some(of_cell) = &level.sh_cluster {
        let mut clusters: Vec<u32> = drawn
            .iter()
            .map(|&cell| of_cell[cell as usize])
            .filter(|&c| c != u32::MAX)
            .collect();
        clusters.sort_unstable();
        clusters.dedup();
        put("sh_clusters", clusters.len().to_string());
    }
    let fog_bits = world.fog_cell_masks.as_ref().map_or(0, |masks| {
        reach
            .iter()
            .fold(0u32, |bits, &cell| bits | masks[cell as usize])
    });
    put("fog_volumes", fog_bits.count_ones().to_string());
    if let Some(set) = world
        .cell_residency_set
        .as_ref()
        .filter(|set| (camera_cell as usize) < set.camera_cell_count())
    {
        let baked = set.entries_for(camera_cell as usize);
        let outside = drawn
            .iter()
            .filter(|&&cell| !baked.iter().any(|entry| entry.cell_id == cell))
            .count();
        put("id51_outside", outside.to_string());
    }
    if let Some(data) = world.cells.get(camera_cell as usize) {
        let size = data.bounds_max - data.bounds_min;
        put(
            "camera_cell_size",
            format!("{:.2}x{:.2}x{:.2}", size.x, size.y, size.z),
        );
    }

    // Rays only on the portal path: a fallback draws through frustum culling
    // and reaches nothing, so there is no walk set to hold the hits to.
    let mut misses = Vec::new();
    let mut counts = RayCounts::default();
    if path == "portal" {
        cast_grid(level, pose, &camera, reach, rays, &mut counts, &mut misses);
    }
    put("rays_hit", counts.hit.to_string());
    put("rays_open", counts.open.to_string());
    put("hit_solid", counts.solid.to_string());
    put("hit_exterior", counts.exterior.to_string());
    put("seen", counts.seen.to_string());
    put("misses", counts.misses.to_string());
    put("missed_cells", counts.missed_cells.to_string());
    put("culled_misses", counts.culled_misses.to_string());
    put("faceless_misses", counts.faceless_misses.to_string());
    put("faceless_cells", counts.faceless_cells.to_string());
    PoseRow {
        pose: pose.clone(),
        fields,
        misses,
    }
}

#[derive(Debug, Default)]
struct RayCounts {
    hit: u64,
    /// No geometry within the far plane.
    open: u64,
    /// The located hit landed in a solid or exterior cell: no open cell to
    /// hold to the walk.
    solid: u64,
    exterior: u64,
    /// Distinct interior cells hit.
    seen: u64,
    /// Hits located in a drawable cell the walk did not reach: the guard.
    misses: u64,
    missed_cells: u64,
    /// Hits located in an unreached cell with no faces, so nothing of it
    /// can draw: a sliver whose every portal fell under the unchanged
    /// `MIN_PORTAL_AREA_M2` cull. Reported, never failed.
    faceless_misses: u64,
    faceless_cells: u64,
    /// Hits located in an unreached drawable cell that the ray entered
    /// across a boundary with no portal: an opening the unchanged portal-area
    /// cull removed. Reported, never failed.
    culled_misses: u64,
}

fn cast_grid(
    level: &Level<'_>,
    pose: &WorldPose,
    camera: &crate::camera::RenderCamera,
    reach: &[u32],
    (w, h): (u32, u32),
    counts: &mut RayCounts,
    misses: &mut Vec<RayMiss>,
) {
    let world = level.world;
    let projection = Projection::default();
    let tan_h = (projection.hfov_degrees.to_radians() / 2.0).tan();
    let tan_v = tan_h / projection.aspect;
    let forward = camera.forward;
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let mut seen = Vec::new();
    let mut missed = Vec::new();
    let mut faceless = Vec::new();
    for j in 0..h {
        for i in 0..w {
            let x = ((i as f32 + 0.5) / w as f32) * 2.0 - 1.0;
            let y = 1.0 - ((j as f32 + 0.5) / h as f32) * 2.0;
            let direction = (forward + right * (x * tan_h) + up * (y * tan_v)).normalize();
            // The near plane is perpendicular to the view axis.
            let start = NEAR / direction.dot(forward);
            let origin = pose.eye + direction * start;
            let Some(hit) = cast_ray(&level.collision, origin, direction, FAR - start) else {
                counts.open += 1;
                continue;
            };
            counts.hit += 1;
            let back = (hit.time_of_impact - HIT_PULLBACK).max(0.0);
            let point = origin + direction * back;
            let cell = world.locate_cell(point);
            if !is_interior(world, cell) {
                if world.cells.get(cell).is_some_and(|c| c.is_solid) {
                    counts.solid += 1;
                } else {
                    counts.exterior += 1;
                }
                continue;
            }
            seen.push(cell as u32);
            if reach.binary_search(&(cell as u32)).is_ok() {
                continue;
            }
            let (kind, count) = if !world.cells[cell].is_drawable {
                faceless.push(cell as u32);
                (MissKind::Faceless, &mut counts.faceless_misses)
            } else if entered_across_culled_opening(level, reach, origin, direction, back) {
                (MissKind::Culled, &mut counts.culled_misses)
            } else {
                missed.push(cell as u32);
                (MissKind::Drawn, &mut counts.misses)
            };
            if *count < MISSES_KEPT as u64 {
                misses.push(RayMiss {
                    ray: j * w + i,
                    point,
                    cell: cell as u32,
                    kind,
                });
            }
            *count += 1;
        }
    }
    let distinct = |cells: &mut Vec<u32>| {
        cells.sort_unstable();
        cells.dedup();
        cells.len() as u64
    };
    counts.seen = distinct(&mut seen);
    counts.missed_cells = distinct(&mut missed);
    counts.faceless_cells = distinct(&mut faceless);
}

/// Marches the ray to `length` through the locator and finds the first
/// interior cell the walk did not reach: true when no portal joins it to the
/// interior cell the ray left, so the ray crossed an opening no portal
/// spans. Every other boundary between two interior cells carries a portal.
fn entered_across_culled_opening(
    level: &Level<'_>,
    reach: &[u32],
    origin: Vec3,
    direction: Vec3,
    length: f32,
) -> bool {
    let world = level.world;
    let steps = (length / MARCH_STEP).ceil() as u32;
    let mut previous: Option<u32> = None;
    for k in 0..=steps {
        let cell = world.locate_cell(origin + direction * (k as f32 * MARCH_STEP).min(length));
        if !is_interior(world, cell) {
            continue;
        }
        let cell = cell as u32;
        if reach.binary_search(&cell).is_err() {
            return previous.is_some_and(|from| {
                !level
                    .portal_pairs
                    .contains(&(from.min(cell), from.max(cell)))
            });
        }
        previous = Some(cell);
    }
    false
}

/// Triangles in the drawn cells' BVH leaves, and in those whose bounds pass
/// the camera frustum: what the GPU cull is handed and what it keeps.
fn triangles(world: &LevelWorld, drawn: &[u32], frustum: &Frustum) -> (u64, u64) {
    let mut all = 0u64;
    let mut kept = 0u64;
    for leaf in &world.bvh.leaves {
        if drawn.binary_search(&leaf.cell_id).is_err() {
            continue;
        }
        let tris = u64::from(leaf.index_count / 3);
        all += tris;
        let (lo, hi) = (Vec3::from(leaf.aabb_min), Vec3::from(leaf.aabb_max));
        let outside = frustum.planes.iter().any(|plane| {
            let corner = Vec3::select(plane.normal.cmpge(Vec3::ZERO), hi, lo);
            plane.normal.dot(corner) + plane.dist < 0.0
        });
        if !outside {
            kept += tris;
        }
    }
    (all, kept)
}

// --- Record ---

pub(super) fn format_record(record: &SweepRecord) -> String {
    let mut text = String::new();
    writeln!(text, "{MAGIC}").expect("string write");
    writeln!(text, "bake {}", record.bake).expect("string write");
    writeln!(text, "camera {}", record.camera).expect("string write");
    for row in &record.rows {
        write!(text, "pose {}", format_pose(&row.pose)).expect("string write");
        for (key, value) in &row.fields {
            write!(text, " {key}={value}").expect("string write");
        }
        writeln!(text).expect("string write");
        for miss in &row.misses {
            let p = miss.point;
            writeln!(
                text,
                "miss {} {} {} {} {} {}",
                miss.ray,
                p.x,
                p.y,
                p.z,
                miss.cell,
                miss.kind.label()
            )
            .expect("string write");
        }
    }
    text
}

pub(super) fn parse_record(text: &str) -> Result<SweepRecord, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err("not a world-pose sweep record".to_string());
    }
    let field = |line: Option<&str>, key: &str| {
        line.and_then(|l| l.strip_prefix(key))
            .and_then(|l| l.strip_prefix(' '))
            .map(str::to_string)
            .ok_or_else(|| format!("expected `{key}`"))
    };
    let bake = field(lines.next(), "bake")?;
    let camera = field(lines.next(), "camera")?;
    let mut rows: Vec<PoseRow> = Vec::new();
    for line in lines {
        let bad = || format!("bad line {line:?}");
        if let Some(rest) = line.strip_prefix("pose ") {
            let mut parts = rest.split_ascii_whitespace();
            let pose = parse_pose(&mut parts).ok_or_else(bad)?;
            let fields = parts
                .map(|part| {
                    part.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .ok_or_else(bad)
                })
                .collect::<Result<_, _>>()?;
            rows.push(PoseRow {
                pose,
                fields,
                misses: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("miss ") {
            let row = rows.last_mut().ok_or_else(bad)?;
            let mut parts = rest.split_ascii_whitespace();
            let ray = parts.next().and_then(|v| v.parse().ok()).ok_or_else(bad)?;
            let mut float = || parts.next()?.parse::<f32>().ok();
            let point = Vec3::new(
                float().ok_or_else(bad)?,
                float().ok_or_else(bad)?,
                float().ok_or_else(bad)?,
            );
            let cell = parts.next().and_then(|v| v.parse().ok()).ok_or_else(bad)?;
            let kind = parts.next().and_then(MissKind::parse).ok_or_else(bad)?;
            row.misses.push(RayMiss {
                ray,
                point,
                cell,
                kind,
            });
        } else if !line.trim().is_empty() {
            return Err(bad());
        }
    }
    Ok(SweepRecord { bake, camera, rows })
}

pub(super) fn write_record(path: &str, record: &SweepRecord) -> std::io::Result<()> {
    std::fs::write(path, format_record(record))
}

pub(super) fn read_record(path: &str) -> Result<SweepRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse_record(&text).map_err(|why| format!("{path}: {why}"))
}

/// Whether `path` holds a sweep record rather than a bake-gap record.
pub(super) fn is_record(path: &str) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| text.lines().next() == Some(MAGIC))
}

// --- Summary and verdict ---

/// Mean, p95 and max of a numeric field over the poses that have it.
fn spread(rows: &[PoseRow], key: &str) -> Option<(f64, f64, f64)> {
    let mut values: Vec<f64> = rows.iter().filter_map(|row| row.number(key)).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let p95 = values[((values.len() as f64 * 0.95).ceil() as usize).clamp(1, values.len()) - 1];
    Some((mean, p95, *values.last().expect("non-empty")))
}

pub(super) fn print_summary(name: &str, record: &SweepRecord) {
    let rows = &record.rows;
    let mut paths: Vec<(&str, usize)> = Vec::new();
    for row in rows {
        let path = row.field("path").unwrap_or("?");
        match paths.iter_mut().find(|(p, _)| *p == path) {
            Some((_, n)) => *n += 1,
            None => paths.push((path, 1)),
        }
    }
    println!(
        "\n=== {name}: {} poses; {}\nbake {}\npaths {paths:?}",
        rows.len(),
        record.camera,
        record.bake
    );
    println!(
        "{:<18} {:>12} {:>12} {:>12}",
        "per pose", "mean", "p95", "max"
    );
    for key in [
        "considered",
        "accepted",
        "drawn",
        "reach",
        "seen",
        "drawn_faces",
        "tris",
        "tris_in_frustum",
        "lightmap_blocks",
        "lightmap_texels",
        "sh_clusters",
        "fog_volumes",
        "id51_outside",
        "misses",
        "faceless_misses",
        "culled_misses",
        "hit_solid",
        "hit_exterior",
    ] {
        if let Some((mean, p95, max)) = spread(rows, key) {
            println!("{key:<18} {mean:>12.1} {p95:>12.0} {max:>12.0}");
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct SweepVerdict {
    pub(super) invalid: Option<String>,
    /// Poses that trip the step cap in this run.
    pub(super) trips: Vec<String>,
    /// Per kind, in `KINDS` order, poses with misses of that kind: (label,
    /// rays that did not miss before, all rays). Unpaired, every ray is new.
    /// Only new `Drawn` misses fail.
    pub(super) by_kind: [Vec<(String, u64, u64)>; 3],
}

impl SweepVerdict {
    pub(super) fn passed(&self) -> bool {
        self.invalid.is_none()
            && self.trips.is_empty()
            && self.by_kind[0].iter().all(|(_, new, _)| *new == 0)
    }

    pub(super) fn print(&self) {
        println!("\n=== ray-grid guard");
        if let Some(why) = &self.invalid {
            println!("INVALID pair: {why}");
            return;
        }
        println!("step-cap trips: {}", self.trips.len());
        for label in self.trips.iter().take(20) {
            println!("  trip {label}");
        }
        for (kind, rows) in KINDS.iter().zip(&self.by_kind) {
            let what = match kind {
                MissKind::Drawn => "unreached drawable cells (fail when new)",
                MissKind::Culled => "cells entered across a culled opening (reported)",
                MissKind::Faceless => "unreached faceless cells (reported)",
            };
            let new = rows.iter().filter(|(_, new, _)| *new > 0).count();
            println!(
                "poses with hits in {what}: {}, {new} with rays that did not miss before",
                rows.len()
            );
            for (label, new, all) in rows.iter().take(30) {
                println!("  {label}: {new} new of {all} rays");
            }
        }
        println!("verdict: {}", if self.passed() { "PASS" } else { "FAIL" });
    }
}

/// The guard over `this` run, paired with `before` when given. Poses pair
/// by index and must hold bit-identical world poses; no cell id is read. A
/// miss pairs by ray index: it is new unless the same ray missed before, of
/// any kind.
pub(super) fn verdict(before: Option<&SweepRecord>, this: &SweepRecord) -> SweepVerdict {
    let mut out = SweepVerdict::default();
    if let Some(before) = before {
        if before.camera != this.camera {
            out.invalid = Some(format!(
                "cameras differ: before {:?}, this {:?}",
                before.camera, this.camera
            ));
            return out;
        }
        if before.rows.len() != this.rows.len() {
            out.invalid = Some(format!(
                "pose counts differ: before {}, this {}",
                before.rows.len(),
                this.rows.len()
            ));
            return out;
        }
        if let Some(i) =
            (0..this.rows.len()).find(|&i| !before.rows[i].pose.same_pose(&this.rows[i].pose))
        {
            out.invalid = Some(format!("world poses differ from pose {i}"));
            return out;
        }
    }
    for (i, row) in this.rows.iter().enumerate() {
        if row.tripped() {
            out.trips.push(row.pose.label.clone());
        }
        let earlier = before.map(|before| &before.rows[i]);
        for (slot, &kind) in out.by_kind.iter_mut().zip(&KINDS) {
            let all = row.miss_count(kind);
            if all == 0 {
                continue;
            }
            let new = earlier.map_or(all, |earlier| {
                let listed = row.misses.iter().filter(|m| m.kind == kind);
                let (listed, kept_new) = listed.fold((0u64, 0u64), |(n, new), m| {
                    (n + 1, new + u64::from(!earlier.missed_ray(m.ray)))
                });
                // Rays past the kept list cannot pair; they count as new
                // unless the before run missed at least as many rays here.
                let missed_before: u64 = KINDS.iter().map(|&k| earlier.miss_count(k)).sum();
                kept_new
                    + if missed_before >= all {
                        0
                    } else {
                        all - listed
                    }
            });
            slot.push((row.pose.label.clone(), new, all));
        }
    }
    out
}
