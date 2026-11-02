//! Per-map structure counts read back from a PRL, and the cells and portals
//! inside a named region grouped by plane.
//! See: context/lib/build_pipeline.md §PRL section IDs · context/lib/rendering_pipeline.md §2

// Counts the compiler logs and the PRL does not hold (classifier output,
// lightmap charts, chunk light-list drops, id-51 cameras without an eye
// point, leak and door-sealing lines) come from the prl-build log.

use std::collections::BTreeMap;

use glam::Vec3;
use postretro_level_format::cluster_directory::CLUSTER_FLAG_INDIVISIBLE_OVERSIZE;
use postretro_level_loader::{LevelWorld, PortalData};

use super::is_interior;

pub(super) fn parse_region(value: &str) -> Option<(Vec3, Vec3)> {
    let v: Vec<f32> = value
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x0, y0, z0, x1, y1, z1] = v[..] else {
        return None;
    };
    let (a, b) = (Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1));
    Some((a.min(b), a.max(b)))
}

/// A portal polygon's area, unit normal, and its diameter over its
/// narrowest width across any edge direction.
pub(super) fn portal_shape(polygon: &[Vec3]) -> (f32, Vec3, f32) {
    let mut sum = Vec3::ZERO;
    for (i, &a) in polygon.iter().enumerate() {
        sum += a.cross(polygon[(i + 1) % polygon.len()]);
    }
    let normal = sum.normalize_or_zero();
    let mut diameter = 0.0f32;
    for (i, &a) in polygon.iter().enumerate() {
        for &b in &polygon[i + 1..] {
            diameter = diameter.max(a.distance(b));
        }
    }
    let mut width = f32::MAX;
    for (i, &a) in polygon.iter().enumerate() {
        let edge = (polygon[(i + 1) % polygon.len()] - a).normalize_or_zero();
        if edge == Vec3::ZERO {
            continue;
        }
        let across = normal.cross(edge);
        let (lo, hi) = polygon.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            let d = v.dot(across);
            (lo.min(d), hi.max(d))
        });
        width = width.min(hi - lo);
    }
    (0.5 * sum.length(), normal, diameter / width.max(1.0e-6))
}

/// Counts per bucket: below `edges[0]`, between consecutive edges, at or
/// above the last.
fn histogram(values: impl IntoIterator<Item = f32>, edges: &[f32]) -> Vec<usize> {
    let mut counts = vec![0usize; edges.len() + 1];
    for v in values {
        counts[edges.iter().take_while(|&&e| v >= e).count()] += 1;
    }
    counts
}

fn print_histogram(title: &str, unit: &str, edges: &[f32], counts: &[usize]) {
    let mut line = format!("{title}:");
    for (k, count) in counts.iter().enumerate() {
        let label = match k {
            0 => format!("<{}", edges[0]),
            k if k == edges.len() => format!(">={}", edges[k - 1]),
            k => format!("{}-{}", edges[k - 1], edges[k]),
        };
        line += &format!(" [{label}{unit}] {count}");
    }
    println!("{line}");
}

fn quantiles(mut values: Vec<f32>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    values.sort_by(f32::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    format!("p50 {:.2} p95 {:.2} max {:.2}", at(0.5), at(0.95), at(1.0))
}

fn walkable(world: &LevelWorld, portal: &PortalData) -> bool {
    is_interior(world, portal.front_cell) && is_interior(world, portal.back_cell)
}

pub(super) fn print_structure(world: &LevelWorld) {
    let cells = &world.cells;
    let interior: Vec<usize> = (0..cells.len())
        .filter(|&c| is_interior(world, c))
        .collect();
    println!("\n=== structure");
    println!(
        "cells {} (interior {}, solid {}, exterior {}, drawable {})",
        cells.len(),
        interior.len(),
        cells.iter().filter(|c| c.is_solid).count(),
        cells.iter().filter(|c| c.is_exterior).count(),
        cells.iter().filter(|c| c.is_drawable).count(),
    );
    let extents: Vec<Vec3> = interior
        .iter()
        .map(|&c| cells[c].bounds_max - cells[c].bounds_min)
        .collect();
    print_histogram(
        "interior cell AABB volume",
        " m3",
        &[0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0],
        &histogram(
            extents.iter().map(|e| e.x * e.y * e.z),
            &[0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0],
        ),
    );
    let thin = [0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
    print_histogram(
        "interior cell smallest extent",
        " m",
        &thin,
        &histogram(extents.iter().map(|e| e.min_element()), &thin),
    );
    println!(
        "interior cell largest extent: {}",
        quantiles(extents.iter().map(|e| e.max_element()).collect())
    );

    let portals = &world.portals;
    let walk: Vec<&PortalData> = portals.iter().filter(|p| walkable(world, p)).collect();
    println!(
        "portals {} (walkable, both cells interior: {}; touching solid or exterior: {})",
        portals.len(),
        walk.len(),
        portals.len() - walk.len()
    );
    let shapes: Vec<(f32, Vec3, f32)> = walk.iter().map(|p| portal_shape(&p.polygon)).collect();
    let areas = [0.1, 1.0, 10.0, 100.0, 1000.0];
    print_histogram(
        "walkable portal area",
        " m2",
        &areas,
        &histogram(shapes.iter().map(|s| s.0), &areas),
    );
    let aspects = [1.5, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];
    print_histogram(
        "walkable portal aspect (diameter / narrowest width)",
        "",
        &aspects,
        &histogram(shapes.iter().map(|s| s.2), &aspects),
    );
    let (horizontal, vertical) = shapes.iter().fold((0, 0), |(h, v), s| {
        (
            h + usize::from(s.1.y.abs() > 0.99),
            v + usize::from(s.1.y.abs() < 0.01),
        )
    });
    println!(
        "walkable portal normals: horizontal {horizontal}, vertical {vertical}, oblique {}",
        shapes.len() - horizontal - vertical
    );

    println!(
        "faces {}, triangles {}, vertices {}, BVH leaves {}",
        cells.iter().map(|c| u64::from(c.face_count)).sum::<u64>(),
        world.indices.len() / 3,
        world.vertices.len(),
        world.bvh.leaves.len(),
    );
    match world.lightmap.as_ref() {
        Some(index) => {
            let mut per_cell: BTreeMap<u32, u32> = BTreeMap::new();
            for record in &index.records {
                *per_cell.entry(record.cell_id).or_default() += 1;
            }
            println!(
                "lightmap blocks {} over {} cells (max {} per cell), texel area {}",
                index.records.len(),
                per_cell.len(),
                per_cell.values().max().copied().unwrap_or(0),
                index
                    .records
                    .iter()
                    .map(|r| u64::from(r.width) * u64::from(r.height))
                    .sum::<u64>(),
            );
        }
        None => println!("lightmap: none"),
    }
    println!(
        "lightmap storage: {}",
        if world.lightmap_stream_manifest().is_some() {
            "streaming"
        } else {
            "all resident"
        }
    );
    let grid = world
        .sh_volume
        .as_ref()
        .map(|v| v.grid_dimensions)
        .or_else(|| world.sh_stream_manifest().map(|m| m.base().grid_dimensions));
    match grid {
        Some(g) => println!(
            "SH probe grid {}x{}x{} = {} probes",
            g[0],
            g[1],
            g[2],
            u64::from(g[0]) * u64::from(g[1]) * u64::from(g[2])
        ),
        None => println!("SH probe grid: none"),
    }
    if let Some(directory) = world.cluster_directory() {
        let oversize = directory
            .clusters
            .iter()
            .filter(|c| c.flags & CLUSTER_FLAG_INDIVISIBLE_OVERSIZE != 0)
            .count();
        println!(
            "SH clusters {} (indivisible oversize {oversize}), max {} cells per cluster",
            directory.clusters.len(),
            directory
                .clusters
                .iter()
                .map(|c| c.member_count)
                .max()
                .unwrap_or(0),
        );
    }
    if let Some(vis) = world.cell_visibility.as_ref() {
        let mut components = vis.component_ids().to_vec();
        components.sort_unstable();
        components.dedup();
        println!(
            "cell visibility: {} components, {} coupled pairs",
            components.len(),
            vis.coupled_pairs().count()
        );
    }
    if let Some(set) = world.cell_residency_set.as_ref() {
        let sizes: Vec<f32> = (0..set.camera_cell_count())
            .filter(|&c| is_interior(world, c))
            .map(|c| set.entries_for(c).len() as f32)
            .collect();
        println!(
            "id 51: {} camera cells, {} entries; entries per interior camera cell {}",
            set.camera_cell_count(),
            set.entries.len(),
            quantiles(sizes)
        );
    }
    println!(
        "lights {}, fog volumes {}",
        world.lights.len(),
        world.fog_volumes.len()
    );
}

/// The interior cells and portals inside `lo..hi`, and the portals grouped by
/// plane: on a stair, riser and tread boundaries show as runs of vertical and
/// horizontal planes a step apart.
pub(super) fn print_region(world: &LevelWorld, lo: Vec3, hi: Vec3) {
    let overlaps = |min: Vec3, max: Vec3| min.cmple(hi).all() && max.cmpge(lo).all();
    println!("\n=== region [{lo}]..[{hi}]");
    let cells: Vec<usize> = (0..world.cells.len())
        .filter(|&c| is_interior(world, c))
        .filter(|&c| overlaps(world.cells[c].bounds_min, world.cells[c].bounds_max))
        .collect();
    println!("interior cells overlapping: {}", cells.len());
    for &c in &cells {
        let data = &world.cells[c];
        println!(
            "  cell {c} [{:.2}, {:.2}, {:.2}]..[{:.2}, {:.2}, {:.2}] faces {}",
            data.bounds_min.x,
            data.bounds_min.y,
            data.bounds_min.z,
            data.bounds_max.x,
            data.bounds_max.y,
            data.bounds_max.z,
            data.face_count
        );
    }
    // Plane key: dominant axis and signed distance, both rounded to the
    // millimetre so coplanar portals group.
    let mut planes: BTreeMap<(String, i64), Vec<usize>> = BTreeMap::new();
    let mut inside = 0usize;
    for (id, portal) in world.portals.iter().enumerate() {
        let (min, max) = portal.polygon.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(a, b), &v| (a.min(v), b.max(v)),
        );
        if portal.polygon.is_empty() || !overlaps(min, max) || !walkable(world, portal) {
            continue;
        }
        inside += 1;
        let (_, normal, _) = portal_shape(&portal.polygon);
        let axis = if normal.y.abs() > 0.99 {
            "horizontal y".to_string()
        } else if normal.x.abs() > 0.99 {
            "vertical x".to_string()
        } else if normal.z.abs() > 0.99 {
            "vertical z".to_string()
        } else {
            format!(
                "oblique n=({:.2},{:.2},{:.2})",
                normal.x.abs(),
                normal.y.abs(),
                normal.z.abs()
            )
        };
        let along = if normal.y.abs() > 0.99 {
            min.y
        } else if normal.x.abs() > 0.99 {
            min.x
        } else if normal.z.abs() > 0.99 {
            min.z
        } else {
            normal.dot(portal.polygon[0]).abs()
        };
        planes
            .entry((axis, (along * 1000.0).round() as i64))
            .or_default()
            .push(id);
    }
    println!(
        "walkable portals overlapping: {inside}, on {} planes",
        planes.len()
    );
    for ((axis, mm), ids) in &planes {
        let area: f32 = ids
            .iter()
            .map(|&id| portal_shape(&world.portals[id].polygon).0)
            .sum();
        println!(
            "  {axis} = {:.3} m: {} portals, {:.2} m2, ids {:?}",
            *mm as f32 / 1000.0,
            ids.len(),
            area,
            &ids[..ids.len().min(12)]
        );
    }
}
