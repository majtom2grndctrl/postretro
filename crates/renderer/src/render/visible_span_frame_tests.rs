//! Adapter-backed proofs through the production offscreen scene recorder.
//! See: context/lib/rendering_pipeline.md (§5, §7.1–7.3).

use super::{ClearColor, Renderer, ShSampleRegionSets};
use crate::compute_cull::IndirectDrawCommand;
use glam::{Mat4, Vec3};
use postretro_level_loader::{MapLight, ShDrainBatch};
use postretro_render_data::geometry::BucketRange;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

#[path = "visible_span_frame_test_fixtures.rs"]
mod fixtures;
use fixtures::World;

const PORTAL: VisibilityPath = VisibilityPath::PrlPortal { walk_reach: 1 };
const CLEAR: ClearColor = ClearColor {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(32, 32) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[VisibleSpanFrameProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

fn ranges(rows: &[(u32, u32, u32)]) -> Vec<BucketRange> {
    rows.iter()
        .map(
            |&(material_bucket_id, first_leaf, leaf_count)| BucketRange {
                material_bucket_id,
                first_leaf,
                leaf_count,
            },
        )
        .collect()
}

fn drawn_slots(commands: &[IndirectDrawCommand]) -> Vec<u32> {
    commands
        .iter()
        .flat_map(|command| match *command {
            IndirectDrawCommand::BindMaterial(_) => 0..0,
            IndirectDrawCommand::MultiDraw { byte_offset, count } => {
                assert_eq!(byte_offset % 20, 0);
                let start = (byte_offset / 20) as u32;
                start..start + count
            }
            IndirectDrawCommand::Draw { byte_offset } => {
                assert_eq!(byte_offset % 20, 0);
                let start = (byte_offset / 20) as u32;
                start..start + 1
            }
        })
        .collect()
}

fn frame(
    renderer: &mut Renderer,
    cells: VisibleCells,
    path: VisibilityPath,
    fog: &[u32],
    expected: &[BucketRange],
    candidate: bool,
) -> Vec<IndirectDrawCommand> {
    let before = renderer.full().compute_cull.as_ref().unwrap().range_builds;
    renderer
        .capture_measurement_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path,
            },
            &[],
            &[],
            fog,
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: fog,
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            Vec3::ZERO,
            &[],
            &[],
            1.0,
            CLEAR,
            true,
            ShDrainBatch::default(),
        )
        .unwrap()
        .frame
        .unwrap();
    let cull = renderer.full().compute_cull.as_ref().unwrap();
    assert_eq!(
        cull.range_builds,
        before + 1,
        "one build per recorded frame"
    );
    assert_eq!(cull.camera_ranges(), expected);
    let trace = cull.draw_trace.borrow();
    assert_eq!(trace.len(), 2, "actual prepass and forward draws");
    assert!(
        !trace[0]
            .iter()
            .any(|c| matches!(c, IndirectDrawCommand::BindMaterial(_)))
    );
    let forward_draws: Vec<_> = trace[1]
        .iter()
        .copied()
        .filter(|c| !matches!(c, IndirectDrawCommand::BindMaterial(_)))
        .collect();
    assert_eq!(
        trace[0], forward_draws,
        "both passes issue identical ranges"
    );
    let expected_slots: Vec<_> = expected
        .iter()
        .flat_map(|r| r.first_leaf..r.first_leaf + r.leaf_count)
        .collect();
    assert_eq!(drawn_slots(&trace[0]), expected_slots);
    let binds: Vec<_> = trace[1]
        .iter()
        .filter_map(|c| match c {
            IndirectDrawCommand::BindMaterial(bucket) => Some(*bucket),
            _ => None,
        })
        .collect();
    let mut expected_binds: Vec<_> = expected.iter().map(|r| r.material_bucket_id).collect();
    expected_binds.dedup();
    assert_eq!(
        binds, expected_binds,
        "one material bind per occupied forward bucket"
    );
    assert_eq!(
        matches!(
            renderer.full().camera_cull_diagnostics.path,
            super::CameraCullPath::Candidate { .. }
        ),
        candidate
    );
    forward_draws
}

#[test]
fn headless_frames_use_their_own_drawable_spans_across_all_cull_paths() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let world = World::new(false);
    world.install(&mut renderer, true);
    let cell0 = ranges(&[(0, 0, 1), (0, 3, 1)]);
    let cell1 = ranges(&[(0, 1, 1), (1, 5, 1)]);
    let whole = world.bvh.derive_bucket_ranges();
    assert!(
        cell0.iter().map(|r| r.leaf_count).sum::<u32>() < whole.iter().map(|r| r.leaf_count).sum()
    );
    let first = frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[1, 2, 3, 4],
        &cell0,
        true,
    );
    assert_eq!(
        first,
        frame(
            &mut renderer,
            VisibleCells::Culled(vec![0, 0]),
            PORTAL,
            &[1],
            &cell0,
            true
        )
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![1]),
        PORTAL,
        &[],
        &cell1,
        true,
    );
    // Consecutive candidate/tree/candidate frames must never inherit prior ranges.
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        VisibilityPath::SolidCellFallback,
        &[],
        &cell0,
        false,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![1]),
        PORTAL,
        &[],
        &cell1,
        true,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        VisibilityPath::SolidCellFallback,
        &[],
        &cell0,
        false,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![1]),
        VisibilityPath::ExteriorCellFallback,
        &[],
        &cell1,
        false,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![1]),
        VisibilityPath::ExteriorCellFallback,
        &[],
        &cell1,
        false,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![]),
        PORTAL,
        &[0, 1],
        &[],
        true,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &cell0,
        true,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![3]),
        PORTAL,
        &[0],
        &[],
        true,
    );
    frame(
        &mut renderer,
        VisibleCells::DrawAll,
        VisibilityPath::EmptyWorldFallback,
        &[],
        &whole,
        false,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![4]),
        PORTAL,
        &[],
        &ranges(&[(1, 4, 1)]),
        true,
    );
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![4, 3, 2, 1, 0]),
        PORTAL,
        &[],
        &whole,
        true,
    );
    for path in [
        PORTAL,
        VisibilityPath::SolidCellFallback,
        VisibilityPath::ExteriorCellFallback,
    ] {
        for ids in [vec![5, 0], vec![0, 5]] {
            frame(
                &mut renderer,
                VisibleCells::Culled(ids),
                path,
                &[],
                &whole,
                false,
            );
            frame(
                &mut renderer,
                VisibleCells::Culled(vec![0]),
                path,
                &[],
                &cell0,
                matches!(path, VisibilityPath::PrlPortal { .. }),
            );
        }
    }
    for (path, candidate) in [
        (VisibilityPath::NoPortalsFallback, false),
        (
            VisibilityPath::PortalStepLimitFallback {
                considered: 2,
                accepted: 1,
            },
            true,
        ),
    ] {
        frame(
            &mut renderer,
            VisibleCells::Culled(vec![1]),
            path,
            &[],
            &cell1,
            candidate,
        );
    }
    world.install(&mut renderer, false);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &whole,
        false,
    );
    world.install(&mut renderer, true);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &cell0,
        true,
    );
    eprintln!("[VisibleSpanFrameProof] all cull paths: 30 adapter frames executed");
}

#[test]
fn headless_level_reinstall_and_occupied_shadow_preserve_current_camera_ranges() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let world = World::new(false);
    world.install(&mut renderer, true);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &ranges(&[(0, 0, 1), (0, 3, 1)]),
        true,
    );
    let mut replacement = World::new(true);
    replacement.install(&mut renderer, true);
    let new_cell0 = ranges(&[(0, 1, 2)]);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &new_cell0,
        true,
    );
    replacement.install(&mut renderer, false);
    frame(
        &mut renderer,
        VisibleCells::DrawAll,
        VisibilityPath::EmptyWorldFallback,
        &[],
        &replacement.bvh.derive_bucket_ranges(),
        false,
    );
    replacement.install(&mut renderer, true);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &new_cell0,
        true,
    );
    replacement.lights.push(MapLight {
        origin: [0.0, 0.0, 1.5],
        light_type: postretro_level_loader::LightType::Spot,
        intensity: 10.0,
        color: [1.0; 3],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: 10.0,
        cone_angle_inner: 0.3,
        cone_angle_outer: 0.7,
        cone_direction: [0.0, 0.0, -1.0],
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    });
    replacement.install(&mut renderer, true);
    frame(
        &mut renderer,
        VisibleCells::Culled(vec![0]),
        PORTAL,
        &[],
        &new_cell0,
        true,
    );
    assert!(
        renderer
            .full()
            .spot_shadow_pool
            .slot_cone_matrices
            .iter()
            .any(Option::is_some),
        "real frame must occupy a shadow slot"
    );
    let shadow = renderer.full().shadow_cull.as_ref().unwrap();
    let shadow_trace = shadow.draw_trace.borrow();
    assert!(
        !shadow_trace.is_empty(),
        "real frame must record shadow world depth"
    );
    let expected_slots: Vec<_> = (0..replacement.bvh.leaves.len() as u32).collect();
    for (slot, commands) in shadow_trace.iter() {
        let stride = (replacement.bvh.leaves.len() as u64 * 20).next_multiple_of(256);
        let base = u64::from(*slot) * stride;
        let relative: Vec<_> = commands
            .iter()
            .map(|command| match *command {
                IndirectDrawCommand::MultiDraw { byte_offset, count } => {
                    IndirectDrawCommand::MultiDraw {
                        byte_offset: byte_offset - base,
                        count,
                    }
                }
                IndirectDrawCommand::Draw { byte_offset } => IndirectDrawCommand::Draw {
                    byte_offset: byte_offset - base,
                },
                IndirectDrawCommand::BindMaterial(_) => {
                    panic!("depth-only shadow must not bind materials")
                }
            })
            .collect();
        assert_eq!(
            drawn_slots(&relative),
            expected_slots,
            "shadow still draws every whole bucket"
        );
    }
    eprintln!("[VisibleSpanFrameProof] reinstall and occupied shadow: 5 adapter frames executed");
}
