use super::*;
use glam::{Vec3, camera::rh};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_loader::{CellData, CellLocatorChild};
use postretro_visibility::{VisibilityPath, VisibilityStats};

fn fixture() -> (LevelWorld, RenderCamera, VisibleRenderPreparation) {
    let cell = |min, max, drawable| CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: u32::from(drawable),
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: drawable,
    };
    let mut world = LevelWorld::new_visibility_only(
        vec![
            cell(Vec3::new(-2.0, -0.1, -6.0), Vec3::new(2.0, 2.0, -4.0), true),
            cell(Vec3::new(-1.0, 0.0, -10.0), Vec3::new(1.0, 2.0, -7.0), true),
            cell(
                Vec3::new(-1.0, 2.0, -10.0),
                Vec3::new(1.0, 3.0, -7.0),
                false,
            ),
        ],
        Vec::new(),
        CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    world.environment_volumes.volumes = vec![EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: [
                [-2.0, 0.0, -8.0],
                [-2.0, 0.0, -4.0],
                [2.0, 0.0, -4.0],
                [2.0, 0.0, -8.0],
            ]
            .into_iter()
            .map(|position| EnvironmentSurfaceVertex {
                position,
                uv: [0.0; 2],
            })
            .collect(),
        }],
    }];
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let view = rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, -6.0), Vec3::Y);
    let camera = RenderCamera {
        eye_position: eye,
        forward: (Vec3::new(0.0, 0.0, -6.0) - eye).normalize(),
        view_matrix: view,
        view_projection: rh::proj::directx::perspective(75.0_f32.to_radians(), 1.5, 0.1, 128.0)
            * view,
    };
    let main = VisibleRenderPreparation {
        visible_cells: VisibleCells::Culled(vec![0]),
        fog_reachable: vec![0],
        light_reachable_cell_mask: vec![true, false, false],
        reachable_cell_aabbs: vec![world.cell_bounds(0).unwrap()],
        stats: VisibilityStats {
            camera_cell: 0,
            total_faces: 2,
            drawn_faces: 1,
            path: VisibilityPath::NoPortalsFallback,
            cpu: StageFrame::new(TimingGate::OFF),
        },
    };
    (world, camera, main)
}

fn prepare(
    mirror: &mut PlanarRenderPreparation,
    world: &LevelWorld,
    camera: RenderCamera,
    main: &VisibleRenderPreparation,
    on: bool,
) {
    mirror.prepare(
        world,
        camera,
        main,
        PlanarReflectionSettings {
            on,
            ..Default::default()
        },
        &[],
        TimingGate::OFF,
    );
}

fn cells(visible: &VisibleCells) -> &[u32] {
    match visible {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => panic!("explicit fixture cells"),
    }
}

#[test]
fn planar_preparation_unions_empty_mirror_cells_for_entities_residency_and_sh() {
    let (world, camera, mut main) = fixture();
    let mut mirror = PlanarRenderPreparation::default();
    mirror.install(&world, &[0]);
    prepare(&mut mirror, &world, camera, &main, true);
    assert!(mirror.camera().is_some());
    assert_eq!(cells(mirror.visible()), &[0, 1]);
    assert_eq!(
        cells(mirror.collected_cells(&main.visible_cells)),
        &[0, 1, 2]
    );
    assert_eq!(mirror.fog_cells(&main.fog_reachable), &[0, 1, 2]);
    main.include_mirror_lighting(&world, &mirror);
    assert_eq!(main.light_reachable_cell_mask, vec![true, true, true]);
    assert_eq!(main.reachable_cell_aabbs.len(), 3);
    assert_eq!(cells(&main.visible_cells), &[0]);
    assert_eq!(main.fog_reachable, vec![0]);
}

#[test]
fn planar_preparation_off_restores_main_inputs_and_reinstall_drops_selection() {
    let (world, camera, main) = fixture();
    let mut mirror = PlanarRenderPreparation::default();
    mirror.install(&world, &[0]);
    prepare(&mut mirror, &world, camera, &main, true);
    prepare(&mut mirror, &world, camera, &main, false);
    assert!(mirror.camera().is_none());
    assert!(mirror.plane().is_none());
    assert!(std::ptr::eq(
        mirror.collected_cells(&main.visible_cells),
        &main.visible_cells
    ));
    assert_eq!(mirror.fog_cells(&main.fog_reachable), &[0]);
    mirror.install(&world, &[]);
    prepare(&mut mirror, &world, camera, &main, true);
    assert!(mirror.camera().is_none());
    assert!(cells(mirror.visible()).is_empty());
}

#[test]
fn planar_preparation_preserves_unisolated_fog_sentinel_on_main_fallback() {
    let (world, camera, mut main) = fixture();
    main.fog_reachable.clear();
    main.light_reachable_cell_mask.clear();
    main.reachable_cell_aabbs.clear();
    let mut mirror = PlanarRenderPreparation::default();
    mirror.install(&world, &[0]);
    prepare(&mut mirror, &world, camera, &main, true);
    assert!(mirror.camera().is_some());
    assert!(mirror.fog_cells(&[]).is_empty());
    main.include_mirror_lighting(&world, &mirror);
    assert!(main.light_reachable_cell_mask.is_empty());
    assert!(main.reachable_cell_aabbs.is_empty());
}
