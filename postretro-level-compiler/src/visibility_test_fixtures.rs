// Visibility test fixtures: geometry builders and pipeline helpers for visibility tests.
// See: context/lib/testing_guide.md §4

use glam::Vec3;

use crate::map_data::{BrushVolume, EntityInfo, Face};
use crate::partition::{Aabb, Cluster};
use crate::test_fixtures::{build_voxel_grid_from_faces, make_box_brush_volume, make_box_faces};
use crate::voxel_grid;
use crate::visibility::{VisibilityResult, compute_visibility, pvs};

/// Build a VoxelGrid covering clusters and faces, matching the main pipeline logic.
pub fn build_test_voxel_grid(
    clusters: &[Cluster],
    faces: &[Face],
    brush_volumes: &[BrushVolume],
) -> voxel_grid::VoxelGrid {
    let mut world_bounds = Aabb::empty();
    for c in clusters {
        world_bounds.expand_aabb(&c.bounds);
    }
    for face in faces {
        for &v in &face.vertices {
            world_bounds.expand_point(v);
        }
    }
    if !world_bounds.is_valid() {
        world_bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(1.0),
        };
    }
    let pad = Vec3::splat(voxel_grid::DEFAULT_VOXEL_SIZE);
    world_bounds.min -= pad;
    world_bounds.max += pad;
    voxel_grid::VoxelGrid::from_brushes(
        brush_volumes,
        &world_bounds,
        voxel_grid::DEFAULT_VOXEL_SIZE,
    )
}

/// Convert grid cells to Cluster type for test compatibility.
pub fn grid_cells_to_clusters(cells: &[crate::spatial_grid::GridCell]) -> Vec<Cluster> {
    cells
        .iter()
        .filter(|c| {
            !c.face_indices.is_empty()
                || c.cell_type.map_or(false, |t| {
                    t != crate::spatial_grid::CellType::Solid
                })
        })
        .enumerate()
        .map(|(new_id, cell)| Cluster {
            id: new_id,
            bounds: cell.bounds.clone(),
            face_indices: cell.face_indices.clone(),
        })
        .collect()
}

/// Decompress all PVS rows from a VisibilityResult into a flat Vec<Vec<u8>>.
pub fn decompress_all_pvs_rows(vis: &VisibilityResult) -> Vec<Vec<u8>> {
    let bytes_per_row = pvs::bytes_for_clusters(vis.cluster_count);
    (0..vis.cluster_count)
        .map(|i| {
            let ci = &vis.section.clusters[i];
            let compressed = &vis.section.pvs_data
                [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize];
            postretro_level_format::visibility::decompress_pvs(compressed, bytes_per_row)
        })
        .collect()
}

/// Find all cluster indices whose centroid falls within a Y-axis range.
///
/// Room discrimination uses a single axis because clusters contain wall
/// brush faces whose centroids spread across X/Z well beyond the air volume.
/// The Y axis (corridor direction in most test geometries) cleanly separates rooms.
pub fn clusters_in_y_range(clusters: &[Cluster], y_min: f32, y_max: f32) -> Vec<usize> {
    clusters
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let cy = c.bounds.centroid().y;
            if cy >= y_min && cy <= y_max {
                Some(i)
            } else {
                None
            }
        })
        .collect()
}

/// Find all cluster indices whose centroid falls within an X-axis range.
pub fn clusters_in_x_range(clusters: &[Cluster], x_min: f32, x_max: f32) -> Vec<usize> {
    clusters
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let cx = c.bounds.centroid().x;
            if cx >= x_min && cx <= x_max {
                Some(i)
            } else {
                None
            }
        })
        .collect()
}

/// Run the full spatial-grid-to-visibility pipeline on faces and brush volumes.
///
/// When `voxel_aware` is true, builds a VoxelGrid and passes it to
/// assign_to_grid for cell classification (solid/air/boundary).
pub fn run_visibility_pipeline_inner(
    faces: Vec<Face>,
    brush_volumes: &[BrushVolume],
    voxel_aware: bool,
) -> (Vec<Cluster>, VisibilityResult) {
    let vg = build_voxel_grid_from_faces(&faces, brush_volumes);
    let grid_opt = if voxel_aware { Some(&vg) } else { None };
    let grid_result = crate::spatial_grid::assign_to_grid(faces, grid_opt);
    let clusters = grid_cells_to_clusters(&grid_result.cells);
    let entities = vec![EntityInfo {
        classname: "info_player_start".to_string(),
        origin: Some(Vec3::ZERO),
    }];
    let min_cell_dim = grid_result
        .cell_size
        .x
        .min(grid_result.cell_size.y)
        .min(grid_result.cell_size.z)
        .max(1.0);
    let vis = compute_visibility(
        &clusters,
        &entities,
        &vg,
        min_cell_dim,
        &grid_result.faces,
        false,
    );
    (clusters, vis)
}

/// Run the full visibility pipeline with voxel-aware grid classification.
pub fn run_visibility_pipeline(
    faces: Vec<Face>,
    brush_volumes: &[BrushVolume],
) -> (Vec<Cluster>, VisibilityResult) {
    run_visibility_pipeline_inner(faces, brush_volumes, true)
}

/// Find the cluster index whose AABB contains the given point.
///
/// Returns None if no cluster contains the point (e.g., point is in
/// solid space or outside the world).
pub fn cluster_containing_point(clusters: &[Cluster], point: Vec3) -> Option<usize> {
    clusters.iter().enumerate().find_map(|(i, c)| {
        let b = &c.bounds;
        if point.x >= b.min.x
            && point.x <= b.max.x
            && point.y >= b.min.y
            && point.y <= b.max.y
            && point.z >= b.min.z
            && point.z <= b.max.z
        {
            Some(i)
        } else {
            None
        }
    })
}

/// Build a sealed two-room level with a solid wall between them (no opening).
///
/// Layout (top-down, Z-up):
///   Room A air: (-60, -28, 4) to (-8, 28, 60)
///   Room B air: (8, -28, 4) to (60, 28, 60)
///   Solid wall: (-8, -32, 0) to (8, 32, 64) -- separates the rooms
///
/// Each room is enclosed by 5 wall/floor/ceiling brushes plus the shared wall.
pub fn build_two_room_sealed_level() -> (Vec<Face>, Vec<BrushVolume>, Vec<EntityInfo>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();

    let mut add_brush = |min: Vec3, max: Vec3| {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    };

    // Middle wall (solid divider between rooms)
    add_brush(Vec3::new(-8.0, -32.0, 0.0), Vec3::new(8.0, 32.0, 64.0));

    // Room A walls
    add_brush(
        Vec3::new(-64.0, -32.0, 0.0),
        Vec3::new(-60.0, 32.0, 64.0),
    ); // left wall
    add_brush(
        Vec3::new(-64.0, -32.0, 0.0),
        Vec3::new(-8.0, -28.0, 64.0),
    ); // back wall
    add_brush(
        Vec3::new(-64.0, 28.0, 0.0),
        Vec3::new(-8.0, 32.0, 64.0),
    ); // front wall
    add_brush(
        Vec3::new(-64.0, -32.0, 0.0),
        Vec3::new(-8.0, 32.0, 4.0),
    ); // floor
    add_brush(
        Vec3::new(-64.0, -32.0, 60.0),
        Vec3::new(-8.0, 32.0, 64.0),
    ); // ceiling

    // Room B walls
    add_brush(
        Vec3::new(60.0, -32.0, 0.0),
        Vec3::new(64.0, 32.0, 64.0),
    ); // right wall
    add_brush(
        Vec3::new(8.0, -32.0, 0.0),
        Vec3::new(64.0, -28.0, 64.0),
    ); // back wall
    add_brush(Vec3::new(8.0, 28.0, 0.0), Vec3::new(64.0, 32.0, 64.0)); // front wall
    add_brush(Vec3::new(8.0, -32.0, 0.0), Vec3::new(64.0, 32.0, 4.0)); // floor
    add_brush(
        Vec3::new(8.0, -32.0, 60.0),
        Vec3::new(64.0, 32.0, 64.0),
    ); // ceiling

    // Player start in Room A
    let entities = vec![EntityInfo {
        classname: "info_player_start".to_string(),
        origin: Some(Vec3::new(-32.0, 0.0, 32.0)),
    }];

    (faces, volumes, entities)
}

/// Two rooms connected by a corridor with configurable room dimensions.
///
/// Layout (top-down, Z-up, looking down -Z):
/// ```text
/// +----------+              +----------+
/// |          |              |          |
/// |  Room A  +--corridor---+  Room B  |
/// |          |              |          |
/// +----------+              +----------+
/// ```
///
/// Room A air: (0, 0, 0) to (rx, ry, rz)
/// Room B air: (0, ry+64, 0) to (rx, 2*ry+64, rz)
/// Corridor air: centered on X, from y=ry to y=ry+64, z=0 to corridor_h
///
/// The corridor connects along the Y axis. Wall brushes fill in the
/// boundary between rooms except for the corridor opening.
/// Corridor width is min(rx/2, 64), corridor height is min(rz, 48).
pub fn build_two_rooms_with_corridor_sized(rx: f32, ry: f32, rz: f32) -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0; // wall thickness
    let corridor_len = 64.0; // corridor length along Y
    let corridor_w = (rx / 2.0).min(64.0); // corridor width: half of room X, capped at 64
    let corridor_h = rz.min(48.0); // corridor height

    // Corridor opening centered on X axis of room
    let cor_x0 = (rx - corridor_w) / 2.0;
    let cor_x1 = (rx + corridor_w) / 2.0;

    let mut add_brush = |min: Vec3, max: Vec3| {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    };

    // --- Room A enclosure (air: 0,0,0 to rx,ry,rz) ---
    // -X wall
    add_brush(
        Vec3::new(-wt, -wt, -wt),
        Vec3::new(0.0, ry + wt, rz + wt),
    );
    // +X wall
    add_brush(
        Vec3::new(rx, -wt, -wt),
        Vec3::new(rx + wt, ry + wt, rz + wt),
    );
    // -Y wall (back of Room A)
    add_brush(Vec3::new(0.0, -wt, -wt), Vec3::new(rx, 0.0, rz + wt));
    // Floor
    add_brush(Vec3::new(0.0, 0.0, -wt), Vec3::new(rx, ry, 0.0));
    // Ceiling
    add_brush(
        Vec3::new(0.0, 0.0, rz),
        Vec3::new(rx, ry, rz + wt),
    );

    // +Y wall of Room A — has a corridor opening from x=cor_x0..cor_x1, z=0..corridor_h
    // Left section of wall (x: 0 to cor_x0)
    add_brush(
        Vec3::new(0.0, ry, -wt),
        Vec3::new(cor_x0, ry + wt, rz + wt),
    );
    // Right section of wall (x: cor_x1 to rx)
    add_brush(
        Vec3::new(cor_x1, ry, -wt),
        Vec3::new(rx, ry + wt, rz + wt),
    );
    // Top section above corridor opening (x: cor_x0 to cor_x1, z: corridor_h to rz)
    add_brush(
        Vec3::new(cor_x0, ry, corridor_h),
        Vec3::new(cor_x1, ry + wt, rz + wt),
    );

    // --- Corridor enclosure (air: cor_x0,ry,0 to cor_x1,ry+corridor_len,corridor_h) ---
    let cor_y1 = ry + corridor_len;
    // Corridor -X wall
    add_brush(
        Vec3::new(cor_x0 - wt, ry, -wt),
        Vec3::new(cor_x0, cor_y1, corridor_h + wt),
    );
    // Corridor +X wall
    add_brush(
        Vec3::new(cor_x1, ry, -wt),
        Vec3::new(cor_x1 + wt, cor_y1, corridor_h + wt),
    );
    // Corridor floor
    add_brush(Vec3::new(cor_x0, ry, -wt), Vec3::new(cor_x1, cor_y1, 0.0));
    // Corridor ceiling
    add_brush(
        Vec3::new(cor_x0, ry, corridor_h),
        Vec3::new(cor_x1, cor_y1, corridor_h + wt),
    );

    // --- Room B enclosure (air: 0,cor_y1,0 to rx,cor_y1+ry,rz) ---
    let rb_y1 = cor_y1 + ry;
    // -X wall
    add_brush(
        Vec3::new(-wt, cor_y1 - wt, -wt),
        Vec3::new(0.0, rb_y1 + wt, rz + wt),
    );
    // +X wall
    add_brush(
        Vec3::new(rx, cor_y1 - wt, -wt),
        Vec3::new(rx + wt, rb_y1 + wt, rz + wt),
    );
    // +Y wall (far end of Room B)
    add_brush(
        Vec3::new(0.0, rb_y1, -wt),
        Vec3::new(rx, rb_y1 + wt, rz + wt),
    );
    // Floor
    add_brush(Vec3::new(0.0, cor_y1, -wt), Vec3::new(rx, rb_y1, 0.0));
    // Ceiling
    add_brush(
        Vec3::new(0.0, cor_y1, rz),
        Vec3::new(rx, rb_y1, rz + wt),
    );

    // -Y wall of Room B — has a corridor opening from x=cor_x0..cor_x1, z=0..corridor_h
    // Left section
    add_brush(
        Vec3::new(0.0, cor_y1 - wt, -wt),
        Vec3::new(cor_x0, cor_y1, rz + wt),
    );
    // Right section
    add_brush(
        Vec3::new(cor_x1, cor_y1 - wt, -wt),
        Vec3::new(rx, cor_y1, rz + wt),
    );
    // Top section above opening
    add_brush(
        Vec3::new(cor_x0, cor_y1 - wt, corridor_h),
        Vec3::new(cor_x1, cor_y1, rz + wt),
    );

    (faces, volumes)
}

/// Build two rooms connected by a corridor (opening on the Y axis).
///
/// Uses default dimensions: 64x64x64 rooms with a 32-wide, 48-tall corridor.
pub fn build_two_rooms_with_corridor() -> (Vec<Face>, Vec<BrushVolume>) {
    build_two_rooms_with_corridor_sized(64.0, 64.0, 64.0)
}

/// Build two rooms separated by a solid wall (no opening).
///
/// Same dimensions as build_two_rooms_with_corridor but the wall between
/// rooms has no gap. Room A and Room B should have zero cross-visibility.
pub fn build_two_rooms_solid_wall() -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0;

    let mut add_brush = |min: Vec3, max: Vec3| {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    };

    // --- Room A (air: 0,0,0 to 64,64,64) ---
    add_brush(
        Vec3::new(-wt, -wt, -wt),
        Vec3::new(0.0, 64.0 + wt, 64.0 + wt),
    );
    add_brush(
        Vec3::new(64.0, -wt, -wt),
        Vec3::new(64.0 + wt, 64.0 + wt, 64.0 + wt),
    );
    add_brush(Vec3::new(0.0, -wt, -wt), Vec3::new(64.0, 0.0, 64.0 + wt));
    add_brush(Vec3::new(0.0, 0.0, -wt), Vec3::new(64.0, 64.0, 0.0));
    add_brush(
        Vec3::new(0.0, 0.0, 64.0),
        Vec3::new(64.0, 64.0, 64.0 + wt),
    );
    // +Y wall — SOLID, no opening
    add_brush(
        Vec3::new(0.0, 64.0, -wt),
        Vec3::new(64.0, 64.0 + wt, 64.0 + wt),
    );

    // --- Room B (air: 0,72,0 to 64,136,64) — gap is filled by the solid wall ---
    add_brush(
        Vec3::new(-wt, 72.0 - wt, -wt),
        Vec3::new(0.0, 136.0 + wt, 64.0 + wt),
    );
    add_brush(
        Vec3::new(64.0, 72.0 - wt, -wt),
        Vec3::new(64.0 + wt, 136.0 + wt, 64.0 + wt),
    );
    add_brush(
        Vec3::new(0.0, 136.0, -wt),
        Vec3::new(64.0, 136.0 + wt, 64.0 + wt),
    );
    add_brush(Vec3::new(0.0, 72.0, -wt), Vec3::new(64.0, 136.0, 0.0));
    add_brush(
        Vec3::new(0.0, 72.0, 64.0),
        Vec3::new(64.0, 136.0, 64.0 + wt),
    );
    // -Y wall — SOLID, no opening
    add_brush(
        Vec3::new(0.0, 72.0 - wt, -wt),
        Vec3::new(64.0, 72.0, 64.0 + wt),
    );

    (faces, volumes)
}

/// Build two rooms connected by an L-shaped corridor.
///
/// Layout (top-down, Z-up):
/// ```text
///              +----------+
///              |  Room A  |
///              +----+-----+
///                   |
///                   | vert corridor
///                   |
///         +---------+
///         | horiz corridor
///    +----+-----+
///    |  Room B   |
///    +-----------+
/// ```
///
/// Room A air: (64, 96, 0) to (128, 160, 64)
/// Vertical corridor air: (80, 64, 0) to (112, 96, 48)
/// Horizontal corridor air: (32, 64, 0) to (80, 96, 48)
/// Room B air: (0, 0, 0) to (64, 64, 64)
pub fn build_l_shaped_corridor() -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0;

    fn add(faces: &mut Vec<Face>, volumes: &mut Vec<BrushVolume>, min: Vec3, max: Vec3) {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    }

    // --- Room A (air: 64,96,0 to 128,160,64) ---
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(64.0 - wt, 96.0 - wt, -wt),
        Vec3::new(64.0, 160.0 + wt, 64.0 + wt),
    ); // -X wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(128.0, 96.0 - wt, -wt),
        Vec3::new(128.0 + wt, 160.0 + wt, 64.0 + wt),
    ); // +X wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(64.0, 160.0, -wt),
        Vec3::new(128.0, 160.0 + wt, 64.0 + wt),
    ); // +Y wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(64.0, 96.0, -wt),
        Vec3::new(128.0, 160.0, 0.0),
    ); // floor
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(64.0, 96.0, 64.0),
        Vec3::new(128.0, 160.0, 64.0 + wt),
    ); // ceiling
    // -Y wall with corridor opening at x:80..112, z:0..48
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(64.0, 96.0 - wt, -wt),
        Vec3::new(80.0, 96.0, 64.0 + wt),
    ); // left of opening
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(112.0, 96.0 - wt, -wt),
        Vec3::new(128.0, 96.0, 64.0 + wt),
    ); // right of opening
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(80.0, 96.0 - wt, 48.0),
        Vec3::new(112.0, 96.0, 64.0 + wt),
    ); // above opening

    // --- Vertical corridor (air: 80,64,0 to 112,96,48) ---
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(80.0 - wt, 64.0, -wt),
        Vec3::new(80.0, 96.0, 48.0 + wt),
    ); // -X wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(112.0, 64.0, -wt),
        Vec3::new(112.0 + wt, 96.0, 48.0 + wt),
    ); // +X wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(80.0, 64.0, -wt),
        Vec3::new(112.0, 96.0, 0.0),
    ); // floor
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(80.0, 64.0, 48.0),
        Vec3::new(112.0, 96.0, 48.0 + wt),
    ); // ceiling

    // --- Horizontal corridor (air: 32,64,0 to 80,96,48) ---
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 64.0 - wt, -wt),
        Vec3::new(80.0, 64.0, 48.0 + wt),
    ); // -Y wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 96.0, -wt),
        Vec3::new(112.0, 96.0 + wt, 48.0 + wt),
    ); // +Y wall (extends to cover vert corridor -Y end)
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 64.0, -wt),
        Vec3::new(80.0, 96.0, 0.0),
    ); // floor
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 64.0, 48.0),
        Vec3::new(80.0, 96.0, 48.0 + wt),
    ); // ceiling

    // --- Room B (air: 0,32,0 to 32,96,64) ---
    // Corridor's -X end at x=32 connects to Room B's +X wall.
    // Opening on Room B +X wall at y:64..96, z:0..48.
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(0.0 - wt, 32.0 - wt, -wt),
        Vec3::new(0.0, 96.0 + wt, 64.0 + wt),
    ); // -X wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(0.0, 32.0 - wt, -wt),
        Vec3::new(32.0, 32.0, 64.0 + wt),
    ); // -Y wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(0.0, 96.0, -wt),
        Vec3::new(32.0, 96.0 + wt, 64.0 + wt),
    ); // +Y wall
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(0.0, 32.0, -wt),
        Vec3::new(32.0, 96.0, 0.0),
    ); // floor
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(0.0, 32.0, 64.0),
        Vec3::new(32.0, 96.0, 64.0 + wt),
    ); // ceiling
    // +X wall with corridor opening at y:64..96, z:0..48
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 32.0, -wt),
        Vec3::new(32.0 + wt, 64.0, 64.0 + wt),
    ); // below opening
    add(
        &mut faces,
        &mut volumes,
        Vec3::new(32.0, 64.0, 48.0),
        Vec3::new(32.0 + wt, 96.0, 64.0 + wt),
    ); // above opening (z)

    (faces, volumes)
}

/// Build a long corridor with two rooms at either end.
///
/// Layout (top-down, Z-up):
/// ```text
/// +--------+-----------+--------+
/// | Room A |  corridor | Room B |
/// +--------+-----------+--------+
/// ```
///
/// Room A air: (0, 0, 0) to (64, 64, 64)
/// Corridor air: (64, 16, 0) to (192, 48, 48) -- 32 wide, 128 long, 48 tall
/// Room B air: (192, 0, 0) to (256, 64, 64)
pub fn build_long_corridor() -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0;

    let mut add_brush = |min: Vec3, max: Vec3| {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    };

    // --- Room A (air: 0,0,0 to 64,64,64) ---
    add_brush(
        Vec3::new(-wt, -wt, -wt),
        Vec3::new(0.0, 64.0 + wt, 64.0 + wt),
    ); // -X
    add_brush(Vec3::new(0.0, -wt, -wt), Vec3::new(64.0, 0.0, 64.0 + wt)); // -Y
    add_brush(
        Vec3::new(0.0, 64.0, -wt),
        Vec3::new(64.0, 64.0 + wt, 64.0 + wt),
    ); // +Y
    add_brush(Vec3::new(0.0, 0.0, -wt), Vec3::new(64.0, 64.0, 0.0)); // floor
    add_brush(
        Vec3::new(0.0, 0.0, 64.0),
        Vec3::new(64.0, 64.0, 64.0 + wt),
    ); // ceiling
    // +X wall with corridor opening at y:16..48, z:0..48
    add_brush(
        Vec3::new(64.0, 0.0, -wt),
        Vec3::new(64.0 + wt, 16.0, 64.0 + wt),
    ); // below opening
    add_brush(
        Vec3::new(64.0, 48.0, -wt),
        Vec3::new(64.0 + wt, 64.0, 64.0 + wt),
    ); // above opening
    add_brush(
        Vec3::new(64.0, 16.0, 48.0),
        Vec3::new(64.0 + wt, 48.0, 64.0 + wt),
    ); // above corridor height

    // --- Corridor (air: 64,16,0 to 192,48,48) ---
    add_brush(
        Vec3::new(64.0, 16.0 - wt, -wt),
        Vec3::new(192.0, 16.0, 48.0 + wt),
    ); // -Y wall
    add_brush(
        Vec3::new(64.0, 48.0, -wt),
        Vec3::new(192.0, 48.0 + wt, 48.0 + wt),
    ); // +Y wall
    add_brush(Vec3::new(64.0, 16.0, -wt), Vec3::new(192.0, 48.0, 0.0)); // floor
    add_brush(
        Vec3::new(64.0, 16.0, 48.0),
        Vec3::new(192.0, 48.0, 48.0 + wt),
    ); // ceiling

    // --- Room B (air: 192,0,0 to 256,64,64) ---
    add_brush(
        Vec3::new(256.0, -wt, -wt),
        Vec3::new(256.0 + wt, 64.0 + wt, 64.0 + wt),
    ); // +X
    add_brush(
        Vec3::new(192.0, -wt, -wt),
        Vec3::new(256.0, 0.0, 64.0 + wt),
    ); // -Y
    add_brush(
        Vec3::new(192.0, 64.0, -wt),
        Vec3::new(256.0, 64.0 + wt, 64.0 + wt),
    ); // +Y
    add_brush(Vec3::new(192.0, 0.0, -wt), Vec3::new(256.0, 64.0, 0.0)); // floor
    add_brush(
        Vec3::new(192.0, 0.0, 64.0),
        Vec3::new(256.0, 64.0, 64.0 + wt),
    ); // ceiling
    // -X wall with corridor opening at y:16..48, z:0..48
    add_brush(
        Vec3::new(192.0 - wt, 0.0, -wt),
        Vec3::new(192.0, 16.0, 64.0 + wt),
    );
    add_brush(
        Vec3::new(192.0 - wt, 48.0, -wt),
        Vec3::new(192.0, 64.0, 64.0 + wt),
    );
    add_brush(
        Vec3::new(192.0 - wt, 16.0, 48.0),
        Vec3::new(192.0, 48.0, 64.0 + wt),
    );

    (faces, volumes)
}

/// Build a Z-shaped three-room layout where Room A and Room C have no
/// direct line of sight.
///
/// Layout (top-down, Z-up):
/// ```text
///                   +----------+
///                   |  Room A  |
///                   +----+-----+
///                        |
///                        | corridor 1 (vertical, along Y)
///                        |
///              +---------+
///              |  Room B  |
///              +----+-----+
///                   |
///                   | corridor 2 (vertical, along Y)
///                   |
///              +----+-----+
///              |  Room C  |
///              +-----------+
/// ```
///
/// Room A air: (128, 208, 0) to (224, 304, 64)
/// Corridor 1 air: (192, 160, 0) to (224, 208, 48)
/// Room B air: (0, 96, 0) to (224, 160, 64)
/// Corridor 2 air: (16, 48, 0) to (48, 96, 48)
/// Room C air: (-32, -48, 0) to (64, 48, 64)
///
/// Key: corridors are offset so there is NO straight-line path from
/// Room A to Room C. Corridor 1 spans x:192..224, Corridor 2 spans
/// x:16..48 -- they share NO X range, so Room B's wall blocks the
/// sightline between x=48 and x=192.
pub fn build_z_shaped_three_rooms() -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0;

    fn add(faces: &mut Vec<Face>, volumes: &mut Vec<BrushVolume>, min: Vec3, max: Vec3) {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    }

    // --- Room A (air: 128,208,0 to 224,304,64) ---
    add(&mut faces, &mut volumes, Vec3::new(128.0 - wt, 208.0 - wt, -wt), Vec3::new(128.0, 304.0 + wt, 64.0 + wt)); // -X
    add(&mut faces, &mut volumes, Vec3::new(224.0, 208.0 - wt, -wt), Vec3::new(224.0 + wt, 304.0 + wt, 64.0 + wt)); // +X
    add(&mut faces, &mut volumes, Vec3::new(128.0, 304.0, -wt), Vec3::new(224.0, 304.0 + wt, 64.0 + wt)); // +Y
    add(&mut faces, &mut volumes, Vec3::new(128.0, 208.0, -wt), Vec3::new(224.0, 304.0, 0.0)); // floor
    add(&mut faces, &mut volumes, Vec3::new(128.0, 208.0, 64.0), Vec3::new(224.0, 304.0, 64.0 + wt)); // ceiling
    // -Y wall with corridor 1 opening at x:192..224, z:0..48
    add(&mut faces, &mut volumes, Vec3::new(128.0, 208.0 - wt, -wt), Vec3::new(192.0, 208.0, 64.0 + wt)); // left of opening
    add(&mut faces, &mut volumes, Vec3::new(192.0, 208.0 - wt, 48.0), Vec3::new(224.0, 208.0, 64.0 + wt)); // above opening

    // --- Corridor 1 (air: 192,160,0 to 224,208,48) -- 48 units deep ---
    add(&mut faces, &mut volumes, Vec3::new(192.0 - wt, 160.0, -wt), Vec3::new(192.0, 208.0, 48.0 + wt)); // -X
    add(&mut faces, &mut volumes, Vec3::new(224.0, 160.0, -wt), Vec3::new(224.0 + wt, 208.0, 48.0 + wt)); // +X
    add(&mut faces, &mut volumes, Vec3::new(192.0, 160.0, -wt), Vec3::new(224.0, 208.0, 0.0)); // floor
    add(&mut faces, &mut volumes, Vec3::new(192.0, 160.0, 48.0), Vec3::new(224.0, 208.0, 48.0 + wt)); // ceiling

    // --- Room B (air: 0,96,0 to 224,160,64) -- 224 wide ---
    add(&mut faces, &mut volumes, Vec3::new(0.0 - wt, 96.0 - wt, -wt), Vec3::new(0.0, 160.0 + wt, 64.0 + wt)); // -X
    add(&mut faces, &mut volumes, Vec3::new(224.0, 96.0 - wt, -wt), Vec3::new(224.0 + wt, 160.0 + wt, 64.0 + wt)); // +X
    add(&mut faces, &mut volumes, Vec3::new(0.0, 96.0, -wt), Vec3::new(224.0, 160.0, 0.0)); // floor
    add(&mut faces, &mut volumes, Vec3::new(0.0, 96.0, 64.0), Vec3::new(224.0, 160.0, 64.0 + wt)); // ceiling
    // +Y wall with corridor 1 opening at x:192..224, z:0..48
    add(&mut faces, &mut volumes, Vec3::new(0.0, 160.0, -wt), Vec3::new(192.0, 160.0 + wt, 64.0 + wt)); // left of opening
    add(&mut faces, &mut volumes, Vec3::new(192.0, 160.0, 48.0), Vec3::new(224.0, 160.0 + wt, 64.0 + wt)); // above opening
    // -Y wall with corridor 2 opening at x:16..48, z:0..48
    add(&mut faces, &mut volumes, Vec3::new(0.0, 96.0 - wt, -wt), Vec3::new(16.0, 96.0, 64.0 + wt)); // left of opening
    add(&mut faces, &mut volumes, Vec3::new(48.0, 96.0 - wt, -wt), Vec3::new(224.0, 96.0, 64.0 + wt)); // right of opening
    add(&mut faces, &mut volumes, Vec3::new(16.0, 96.0 - wt, 48.0), Vec3::new(48.0, 96.0, 64.0 + wt)); // above opening

    // --- Corridor 2 (air: 16,48,0 to 48,96,48) -- 48 units deep ---
    add(&mut faces, &mut volumes, Vec3::new(16.0 - wt, 48.0, -wt), Vec3::new(16.0, 96.0, 48.0 + wt)); // -X
    add(&mut faces, &mut volumes, Vec3::new(48.0, 48.0, -wt), Vec3::new(48.0 + wt, 96.0, 48.0 + wt)); // +X
    add(&mut faces, &mut volumes, Vec3::new(16.0, 48.0, -wt), Vec3::new(48.0, 96.0, 0.0)); // floor
    add(&mut faces, &mut volumes, Vec3::new(16.0, 48.0, 48.0), Vec3::new(48.0, 96.0, 48.0 + wt)); // ceiling

    // --- Room C (air: -32,-48,0 to 64,48,64) ---
    add(&mut faces, &mut volumes, Vec3::new(-32.0 - wt, -48.0 - wt, -wt), Vec3::new(-32.0, 48.0 + wt, 64.0 + wt)); // -X
    add(&mut faces, &mut volumes, Vec3::new(64.0, -48.0 - wt, -wt), Vec3::new(64.0 + wt, 48.0 + wt, 64.0 + wt)); // +X
    add(&mut faces, &mut volumes, Vec3::new(-32.0, -48.0 - wt, -wt), Vec3::new(64.0, -48.0, 64.0 + wt)); // -Y
    add(&mut faces, &mut volumes, Vec3::new(-32.0, -48.0, -wt), Vec3::new(64.0, 48.0, 0.0)); // floor
    add(&mut faces, &mut volumes, Vec3::new(-32.0, -48.0, 64.0), Vec3::new(64.0, 48.0, 64.0 + wt)); // ceiling
    // +Y wall with corridor 2 opening at x:16..48, z:0..48
    add(&mut faces, &mut volumes, Vec3::new(-32.0, 48.0, -wt), Vec3::new(16.0, 48.0 + wt, 64.0 + wt)); // left of opening
    add(&mut faces, &mut volumes, Vec3::new(48.0, 48.0, -wt), Vec3::new(64.0, 48.0 + wt, 64.0 + wt)); // right of opening
    add(&mut faces, &mut volumes, Vec3::new(16.0, 48.0, 48.0), Vec3::new(48.0, 48.0 + wt, 64.0 + wt)); // above opening

    (faces, volumes)
}

/// Build two rooms separated by a thick solid wall with no opening.
///
/// The wall is 16 units thick -- wide enough that clusters whose AABBs
/// overlap the wall region will have random sample points landing inside
/// solid space. If filter_solid_samples weren't working, those in-wall
/// points would produce rays that originate on the wrong side of the
/// wall, creating false cross-room visibility.
///
/// Layout (top-down, Z-up):
///   Room A air: (-64, -32, 0) to (-8, 32, 64)
///   Thick wall: (-8, -40, -8) to (8, 40, 72)
///   Room B air: (8, -32, 0) to (64, 32, 64)
pub fn build_thick_wall_sealed_rooms() -> (Vec<Face>, Vec<BrushVolume>) {
    let mut faces = Vec::new();
    let mut volumes = Vec::new();
    let wt = 8.0;

    let mut add_brush = |min: Vec3, max: Vec3| {
        faces.extend(make_box_faces(min, max));
        volumes.push(make_box_brush_volume(min, max));
    };

    // Thick solid wall between rooms (16 units: x -8 to 8)
    add_brush(
        Vec3::new(-8.0, -40.0, -wt),
        Vec3::new(8.0, 40.0, 64.0 + wt),
    );

    // Room A enclosure (air: -64, -32, 0 to -8, 32, 64)
    add_brush(
        Vec3::new(-64.0 - wt, -32.0 - wt, -wt),
        Vec3::new(-64.0, 32.0 + wt, 64.0 + wt),
    ); // -X wall
    add_brush(
        Vec3::new(-64.0, -32.0 - wt, -wt),
        Vec3::new(-8.0, -32.0, 64.0 + wt),
    ); // -Y wall
    add_brush(
        Vec3::new(-64.0, 32.0, -wt),
        Vec3::new(-8.0, 32.0 + wt, 64.0 + wt),
    ); // +Y wall
    add_brush(
        Vec3::new(-64.0, -32.0, -wt),
        Vec3::new(-8.0, 32.0, 0.0),
    ); // floor
    add_brush(
        Vec3::new(-64.0, -32.0, 64.0),
        Vec3::new(-8.0, 32.0, 64.0 + wt),
    ); // ceiling

    // Room B enclosure (air: 8, -32, 0 to 64, 32, 64)
    add_brush(
        Vec3::new(64.0, -32.0 - wt, -wt),
        Vec3::new(64.0 + wt, 32.0 + wt, 64.0 + wt),
    ); // +X wall
    add_brush(
        Vec3::new(8.0, -32.0 - wt, -wt),
        Vec3::new(64.0, -32.0, 64.0 + wt),
    ); // -Y wall
    add_brush(
        Vec3::new(8.0, 32.0, -wt),
        Vec3::new(64.0, 32.0 + wt, 64.0 + wt),
    ); // +Y wall
    add_brush(
        Vec3::new(8.0, -32.0, -wt),
        Vec3::new(64.0, 32.0, 0.0),
    ); // floor
    add_brush(
        Vec3::new(8.0, -32.0, 64.0),
        Vec3::new(64.0, 32.0, 64.0 + wt),
    ); // ceiling

    (faces, volumes)
}
