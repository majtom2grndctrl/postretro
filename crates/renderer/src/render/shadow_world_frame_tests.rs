//! Adapter-backed shadow world reach through the production offscreen recorder.
//! See: context/lib/rendering_pipeline.md §7.1 steps 6–8.

use super::shadow_world_draws::ShadowRegion;
use super::{ClearColor, LevelGeometry, LevelGeometryShStorage, Renderer, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
use postretro_level_loader::{
    CellData, FalloffModel, LightType, MapLight, ShDrainBatch, ShadowType,
};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};
use std::ops::Range;

const PORTAL: VisibilityPath = VisibilityPath::PrlPortal { walk_reach: 1 };

fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(32, 32) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[ShadowWorldFrameProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// One unit-box cell per centre, one triangle per cell, cell-major indices.
struct World {
    bvh: BvhTree,
    index: CellDrawIndexSection,
    vertices: Vec<WorldVertex>,
    indices: Vec<u32>,
    cells: Vec<CellData>,
    lights: Vec<MapLight>,
}

impl World {
    fn new(centres: &[Vec3], with_bvh: bool) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut leaves = Vec::new();
        for (cell, centre) in centres.iter().enumerate() {
            let base = vertices.len() as u32;
            for corner in [
                Vec3::new(-0.4, -0.4, 0.0),
                Vec3::new(0.4, -0.4, 0.0),
                Vec3::new(0.0, 0.4, 0.0),
            ] {
                vertices.push(WorldVertex {
                    position: (*centre + corner).to_array(),
                    base_uv: [0.0; 2],
                    normal_oct: [32768; 2],
                    tangent_packed: [0; 2],
                    lightmap_uv: [0; 2],
                    lightmap_block: 0,
                    animated_block: 0,
                });
            }
            indices.extend([base, base + 1, base + 2]);
            leaves.push(BvhLeaf {
                aabb_min: (*centre - 0.5).to_array(),
                material_bucket_id: 0,
                aabb_max: (*centre + 0.5).to_array(),
                index_offset: cell as u32 * 3,
                index_count: 3,
                cell_id: cell as u32,
                chunk_range_start: 0,
                chunk_range_count: 0,
            });
        }
        let (min, max) =
            leaves
                .iter()
                .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(min, max), leaf| {
                    (
                        min.min(Vec3::from(leaf.aabb_min)),
                        max.max(Vec3::from(leaf.aabb_max)),
                    )
                });
        let mut nodes = vec![BvhNode {
            aabb_min: min.to_array(),
            skip_index: leaves.len() as u32 + 1,
            aabb_max: max.to_array(),
            left_child_or_leaf_index: 0,
            flags: 0,
        }];
        nodes.extend(leaves.iter().enumerate().map(|(slot, leaf)| BvhNode {
            aabb_min: leaf.aabb_min,
            skip_index: slot as u32 + 2,
            aabb_max: leaf.aabb_max,
            left_child_or_leaf_index: slot as u32,
            flags: BVH_NODE_FLAG_LEAF,
        }));
        let index = CellDrawIndexSection {
            cell_count: centres.len() as u32,
            span_count: centres.len() as u32,
            cell_span_offset: (0..=centres.len() as u32).collect(),
            spans: (0..centres.len() as u32)
                .map(|leaf_start| Span {
                    leaf_start,
                    leaf_count: 1,
                })
                .collect(),
        };
        let index = CellDrawIndexSection::from_bytes(&index.to_bytes()).unwrap();
        crate::render::validate_level_geometry_ranges(&leaves, indices.len()).unwrap();
        let cells = centres
            .iter()
            .map(|centre| CellData {
                bounds_min: *centre - 0.5,
                bounds_max: *centre + 0.5,
                face_start: 0,
                face_count: 1,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: true,
            })
            .collect();
        let bvh = if with_bvh {
            BvhTree {
                nodes,
                leaves,
                root_node_index: 0,
            }
        } else {
            BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            }
        };
        Self {
            bvh,
            index,
            vertices,
            indices,
            cells,
            lights: vec![],
        }
    }

    fn with_spot(mut self, origin: Vec3, direction: Vec3) -> Self {
        self.lights.push(MapLight {
            origin: [origin.x as f64, origin.y as f64, origin.z as f64],
            light_type: LightType::Spot,
            intensity: 10.0,
            color: [1.0; 3],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.7,
            cone_direction: direction.to_array(),
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        });
        self
    }

    fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    fn install(&self, renderer: &mut Renderer, indexed: bool) {
        renderer.install_level_geometry(
            &LevelGeometry {
                vertices: &self.vertices,
                indices: &self.indices,
                bvh: &self.bvh,
                lights: &self.lights,
                light_influences: &[],
                sh_volume: None,
                sh_storage: LevelGeometryShStorage::Legacy,
                lightmap: None,
                lightmap_streaming: None,
                chunk_light_list: None,
                animated_light_chunks: None,
                animated_light_weight_maps: None,
                delta_sh_volumes: None,
                direct_sh_volume: None,
                direct_sh_delta_volumes: None,
                animated_direct_sh_delta_volumes: None,
                billboard_direct_scatter_volume: None,
                animated_billboard_direct_scatter_delta_volumes: None,
                entity_shadow_lights: &[],
                shadowmask_atlas: None,
                sdf_atlas: None,
                lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
                cell_draw_index: (indexed && !self.bvh.leaves.is_empty()).then_some(&self.index),
                kinematic_geometry: None,
                cells: &self.cells,
                texture_materials: &[],
            },
            Default::default(),
        );
    }
}

/// Record one production frame. Returns the frame's shadow world draws:
/// `(region, ranges)` for every region that walked reach.
fn frame(
    renderer: &mut Renderer,
    camera: VisibleCells,
    fog: &[u32],
) -> Vec<(ShadowRegion, Vec<Range<u32>>)> {
    renderer
        .capture_measurement_frame_indirect(
            CameraCullVisibility {
                cells: &camera,
                path: PORTAL,
            },
            &[],
            &[],
            fog,
            ShSampleRegionSets {
                visible_cells: &camera,
                fog_cells: fog,
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            Vec3::ZERO,
            &[],
            &[],
            1.0,
            ClearColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            true,
            ShDrainBatch::default(),
        )
        .unwrap()
        .frame
        .unwrap();
    let full = renderer.full();
    assert!(
        full.spot_shadow_pool
            .slot_cone_matrices
            .iter()
            .any(Option::is_some),
        "the frame must occupy a shadow slot"
    );
    // Every BVH-level world draw is one walk; a no-BVH draw walks nothing.
    assert!(full.shadow_world.walks as usize <= full.shadow_world.trace.len());
    full.shadow_world
        .trace
        .iter()
        .map(|entry| (entry.region, entry.ranges.clone()))
        .collect()
}

/// Cells 0–2 lie down -Z in a spot cone at the origin; cells 3 and 4 sit
/// behind the light, cell 5 far to the side.
fn cone_world(with_bvh: bool) -> World {
    World::new(
        &[
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::new(0.0, 0.0, -4.0),
            Vec3::new(0.0, 0.0, -6.0),
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::new(0.0, 0.0, 6.0),
            Vec3::new(30.0, 0.0, -4.0),
        ],
        with_bvh,
    )
    .with_spot(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z)
}

#[test]
fn recorded_frame_draws_reach_beyond_camera_and_fog_and_only_reach() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let world = cone_world(true);
    world.install(&mut renderer, true);
    // The camera sees cell 3 and fog reaches cell 4, both behind the light.
    // Cells 0–2 are in neither set but in the cone.
    let draws = frame(&mut renderer, VisibleCells::Culled(vec![3]), &[4]);
    assert_eq!(draws.len(), 1, "one dynamic cold fill: {draws:?}");
    let (region, ranges) = &draws[0];
    assert!(matches!(region, ShadowRegion::Spot(_)));
    assert_eq!(ranges, &vec![0..9], "cells 0–2 in one run, nothing else");
    assert_ne!(ranges, &vec![0..world.index_count()]);

    // The cold fill warmed its layer: the next frame walks nothing.
    assert!(frame(&mut renderer, VisibleCells::Culled(vec![3]), &[4]).is_empty());
    eprintln!("[ShadowWorldFrameProof] reach beyond camera and fog: 2 adapter frames executed");
}

#[test]
fn recorded_cold_fill_with_empty_reach_still_records_its_cleared_pass() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // The light faces +X, where nothing lies within its range.
    let mut world = cone_world(true);
    world.lights.clear();
    let world = world.with_spot(Vec3::new(0.0, 0.0, 1.0), Vec3::X);
    world.install(&mut renderer, true);
    let draws = frame(&mut renderer, VisibleCells::Culled(vec![0]), &[]);
    // The cold fill walked once and drew nothing; its cache pass (LoadOp
    // Clear to far depth) still ran, and the layer is now warm.
    assert_eq!(draws.len(), 1, "{draws:?}");
    assert!(draws[0].1.is_empty());
    let cold = &renderer.full().dynamic_depth_cache_frame_plan;
    assert!(!cold.spot().is_empty() && cold.spot().iter().all(|plan| plan.needs_world_render));
    assert!(frame(&mut renderer, VisibleCells::Culled(vec![0]), &[]).is_empty());
    assert!(
        renderer
            .full()
            .dynamic_depth_cache_frame_plan
            .spot()
            .iter()
            .all(|plan| !plan.needs_world_render),
        "the empty cold fill warmed its layer"
    );
    eprintln!("[ShadowWorldFrameProof] empty reach cold fill: 2 adapter frames executed");
}

#[test]
fn recorded_level_switches_draw_each_installed_levels_own_world() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // O7 and the first fill after install: BVH level, no-BVH level, BVH level,
    // with the same light pose throughout.
    let first = cone_world(true);
    first.install(&mut renderer, true);
    let draws = frame(&mut renderer, VisibleCells::Culled(vec![0]), &[]);
    assert_eq!(draws[0].1, vec![0..9]);

    let flat = World::new(
        &[Vec3::new(0.0, 0.0, -2.0), Vec3::new(9.0, 0.0, 9.0)],
        false,
    )
    .with_spot(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z);
    flat.install(&mut renderer, true);
    let draws = frame(&mut renderer, VisibleCells::DrawAll, &[]);
    assert_eq!(draws.len(), 1, "{draws:?}");
    assert_eq!(draws[0].1, vec![0..flat.index_count()]);

    // A different BVH level: its first fill walks its own tree.
    let second = World::new(
        &[
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::new(0.0, 0.0, -5.0),
        ],
        true,
    )
    .with_spot(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z);
    second.install(&mut renderer, true);
    let draws = frame(&mut renderer, VisibleCells::Culled(vec![1]), &[]);
    assert_eq!(draws.len(), 1, "{draws:?}");
    assert_eq!(draws[0].1, vec![3..9]);
    for (_, ranges) in &draws {
        assert!(ranges.iter().all(|range| range.end <= second.index_count()));
    }
    eprintln!("[ShadowWorldFrameProof] level switches: 3 adapter frames executed");
}

#[test]
fn recorded_frame_without_cell_draw_index_draws_reach() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // O8: BVH leaves present, per-cell draw index absent.
    let world = cone_world(true);
    world.install(&mut renderer, false);
    let draws = frame(&mut renderer, VisibleCells::DrawAll, &[]);
    assert_eq!(draws.len(), 1, "{draws:?}");
    assert_eq!(draws[0].1, vec![0..9]);
    eprintln!("[ShadowWorldFrameProof] no cell draw index: 1 adapter frame executed");
}
