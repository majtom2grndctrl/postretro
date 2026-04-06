// postretro-level-compiler: level compiler entry point.
// See: context/lib/index.md

pub mod geometry;
pub mod map_data;
pub mod pack;
pub mod parse;
#[allow(dead_code)]
pub mod partition;
pub mod spatial_grid;
pub mod visibility;
pub mod voxel_grid;

use crate::partition::{Aabb, Cluster};
use crate::voxel_grid::VoxelGrid;

use glam::Vec3;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = parse_args()?;

    log::info!("[Compiler] Input: {}", args.input.display());
    log::info!("[Compiler] Output: {}", args.output.display());
    if args.diagnostics {
        log::info!("[Compiler] Diagnostics mode enabled (computing visibility confidence)");
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

    let grid_result =
        spatial_grid::assign_to_grid(map_data.world_faces, Some(&voxel_grid));
    let clusters = grid_cells_to_clusters(&grid_result.cells);

    log::info!("[Compiler] Spatial grid assignment complete.");

    let geometry_section = geometry::extract_geometry(&grid_result.faces, &clusters);
    geometry::log_stats(&geometry_section, clusters.len());

    log::info!("[Compiler] Geometry extraction complete.");

    let min_cell_dim = grid_result
        .cell_size
        .x
        .min(grid_result.cell_size.y)
        .min(grid_result.cell_size.z)
        .max(1.0);
    let vis_result = visibility::compute_visibility(
        &clusters,
        &map_data.entities,
        &voxel_grid,
        min_cell_dim,
        &grid_result.faces,
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

    log::info!("[Compiler] Done.");

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
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut diagnostics = false;

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
            _ if input.is_none() => {
                input = Some(PathBuf::from(arg));
            }
            _ => {
                anyhow::bail!("unexpected argument: {arg}");
            }
        }
    }

    let input = input.ok_or_else(|| {
        anyhow::anyhow!("usage: prl-build <input.map> [-o <output.prl>] [--diagnostics]")
    })?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args {
        input,
        output,
        diagnostics,
    })
}
