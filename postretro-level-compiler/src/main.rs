// postretro-level-compiler: level compiler entry point.
// See: context/lib/index.md

pub mod geometry;
pub mod map_data;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod spatial_grid;
pub mod visibility;
pub mod voxel_grid;

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod spatial_grid_test_fixtures;
#[cfg(test)]
mod visibility_test_fixtures;

use crate::partition::{Aabb, Cluster};
use crate::voxel_grid::VoxelGrid;

use glam::Vec3;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let started = Instant::now();
    let args = parse_args()?;

    log::info!("[Compiler] Input: {}", args.input.display());
    log::info!("[Compiler] Output: {}", args.output.display());
    if args.diagnostics {
        log::info!("[Compiler] Diagnostics mode enabled (computing visibility confidence)");
    }
    if args.bsp {
        log::info!("[Compiler] BSP partitioning enabled");
    }

    let map_data = parse::parse_map_file(&args.input)?;

    log::info!("[Compiler] Parsing complete.");

    // Build voxel grid from brush volumes (shared between spatial grid and PVS)
    let mut voxel_grid = build_voxel_grid(&map_data.world_faces, &map_data.brush_volumes);

    // Seal exterior void: flood-fill from player spawn to find interior air,
    // then mark all unreached empty voxels as solid so rays can't travel
    // through the void outside the map.
    if let Some(seed) = find_player_start_origin(&map_data.entities) {
        voxel_grid.seal_exterior(seed);
    } else {
        log::warn!(
            "[Compiler] No info_player_start with origin found; \
             skipping exterior void seal"
        );
    }

    let (clusters, world_faces, min_cell_dim) = if args.bsp {
        let res = partition::partition(map_data.world_faces, &map_data.brush_volumes)?;
        log::info!("[Compiler] BSP partitioning complete.");
        // Use a default min_cell_dim for BSP since it doesn't have fixed cells
        (res.clusters, res.faces, 32.0f32)
    } else {
        let res = spatial_grid::assign_to_grid(map_data.world_faces, Some(&voxel_grid));
        log::info!("[Compiler] Spatial grid assignment complete.");
        let min_dim = res
            .cell_size
            .x
            .min(res.cell_size.y)
            .min(res.cell_size.z)
            .max(1.0);
        (grid_cells_to_clusters(&res.cells), res.faces, min_dim)
    };

    let geometry_section = geometry::extract_geometry(&world_faces, &clusters);
    geometry::log_stats(&geometry_section, clusters.len());

    log::info!("[Compiler] Geometry extraction complete.");

    let vis_result = visibility::compute_visibility(
        &clusters,
        &map_data.entities,
        &voxel_grid,
        min_cell_dim,
        &world_faces,
        args.diagnostics,
    );
    visibility::log_stats(&vis_result);

    log::info!("[Compiler] Visibility computation complete.");

    pack::pack_and_write(
        &args.output,
        &geometry_section,
        &vis_result.section,
        vis_result.confidence.as_deref(),
    )?;

    let elapsed = started.elapsed();
    log::info!("[Compiler] Done in {elapsed:.2?}.");

    Ok(())
}

/// Build a VoxelGrid covering all face geometry, padded by one voxel.
///
/// Computes the world AABB from face vertices, then voxelizes brush volumes.
/// The resulting grid is shared between the spatial grid (cell classification)
/// and PVS (ray-cast occlusion).
fn build_voxel_grid(
    faces: &[crate::map_data::Face],
    brush_volumes: &[crate::map_data::BrushVolume],
) -> VoxelGrid {
    let mut world_bounds = Aabb::empty();
    for face in faces {
        for &v in &face.vertices {
            world_bounds.expand_point(v);
        }
    }
    let pad = Vec3::splat(voxel_grid::DEFAULT_VOXEL_SIZE);
    world_bounds.min -= pad;
    world_bounds.max += pad;

    VoxelGrid::from_brushes(brush_volumes, &world_bounds, voxel_grid::DEFAULT_VOXEL_SIZE)
}

/// Convert spatial grid cells to Cluster type for downstream pipeline stages.
///
/// When voxel-aware classification is active, keeps air cells even with no faces
/// (they provide camera containment). Without classification, filters out empty
/// cells. Reassigns sequential IDs for dense cluster lists.
fn grid_cells_to_clusters(cells: &[spatial_grid::GridCell]) -> Vec<Cluster> {
    cells
        .iter()
        .filter(|c| {
            // Keep cells that have faces OR are classified as air/boundary
            // (air cells with no faces still need cluster bounds for camera containment)
            !c.face_indices.is_empty()
                || c.cell_type
                    .map_or(false, |t| t != spatial_grid::CellType::Solid)
        })
        .enumerate()
        .map(|(new_id, cell)| Cluster {
            id: new_id,
            bounds: cell.bounds.clone(),
            face_indices: cell.face_indices.clone(),
        })
        .collect()
}

/// Find the `info_player_start` entity origin for exterior void sealing.
///
/// Entity origins are in Quake coordinates (Z-up), which matches the voxel
/// grid's coordinate space. No transform needed.
fn find_player_start_origin(entities: &[crate::map_data::EntityInfo]) -> Option<Vec3> {
    entities
        .iter()
        .find(|e| e.classname == "info_player_start")
        .and_then(|e| e.origin)
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    /// Compute per-pair visibility confidence (slower, for diagnostics).
    diagnostics: bool,
    /// Use BSP partitioning instead of spatial grid.
    bsp: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I>(mut args: I) -> anyhow::Result<Args>
where
    I: Iterator<Item = String>,
{
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut diagnostics = false;
    let mut bsp = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("-o requires an output path"))?;
                output = Some(PathBuf::from(path));
            }
            "--diagnostics" => {
                diagnostics = true;
            }
            "--bsp" => {
                bsp = true;
            }
            _ if input.is_none() => {
                input = Some(PathBuf::from(arg));
            }
            _ => {
                anyhow::bail!("unexpected argument: {arg}");
            }
        }
    }

    let input = input.ok_or_else(|| {
        anyhow::anyhow!("usage: prl-build <input.map> [-o <output.prl>] [--diagnostics] [--bsp]")
    })?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args {
        input,
        output,
        diagnostics,
        bsp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_enables_bsp_flag() {
        let args = vec!["input.map".to_string(), "--bsp".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.bsp);
    }
}
