// Planar mirror fixture: a water room whose ceiling portals only the mirror sees.
// See: context/lib/testing_guide.md

use glam::{Vec3, camera::rh};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, PortalData};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::planar_preset;
use postretro_visibility::{PortalWalkScratch, TimingGate, VisibleCells};

use super::PlanarRenderPreparation;
use crate::camera::RenderCamera;
use crate::render_preparation::VisibleRenderPreparation;

/// The water room: it holds the camera and the water at y = 0.
pub(crate) const WATER_ROOM: u32 = 0;
/// Rooms above the water room's ceiling portals: behind the main camera's
/// frustum, inside the mirror's. Their ids sit outside the lightmap corridor's
/// camera-0 baked set, so their blocks are demanded only as drawn cells.
pub(crate) const MIRROR_ROOMS: [u32; 2] = [5, 6];
/// An empty (faceless, non-solid) cell above room 6: the mirror reaches it,
/// draws nothing in it, and an entity may still stand there.
pub(crate) const EMPTY_ATTIC: u32 = 8;
/// A solid cell; a locator answering it puts the main view on the solid-cell
/// fallback at the same pose.
pub(crate) const SOLID_CAMERA_CELL: u32 = 7;

fn cell(min: Vec3, max: Vec3, drawable: bool, solid: bool) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: u32::from(drawable),
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: solid,
        is_exterior: false,
        is_drawable: drawable,
    }
}

/// Nine cells. Cell 0 (x, z ∈ [−4, 4], y ∈ [−1, 4]) straddles the water and
/// opens through two ceiling portals at y = 4 into rooms 5 (x < 0) and 6
/// (x > 0); room 6 opens at y = 8 into the empty attic 8. Cells 1–4 and 7 are
/// solid filler far away, so cell ids line up with the lightmap corridor's.
/// The locator answers `locator_cell` everywhere.
pub(crate) fn world(locator_cell: u32) -> LevelWorld {
    let filler = |x: f32| {
        cell(
            Vec3::new(x, -1.0, 100.0),
            Vec3::new(x + 1.0, 0.0, 101.0),
            false,
            true,
        )
    };
    let mut cells = vec![
        cell(
            Vec3::new(-4.0, -1.0, -4.0),
            Vec3::new(4.0, 4.0, 4.0),
            true,
            false,
        ),
        filler(100.0),
        filler(102.0),
        filler(104.0),
        filler(106.0),
        cell(
            Vec3::new(-4.0, 4.0, -4.0),
            Vec3::new(0.0, 8.0, 4.0),
            true,
            false,
        ),
        cell(
            Vec3::new(0.0, 4.0, -4.0),
            Vec3::new(4.0, 8.0, 4.0),
            true,
            false,
        ),
        filler(108.0),
        cell(
            Vec3::new(0.0, 8.0, -4.0),
            Vec3::new(4.0, 10.0, 4.0),
            false,
            false,
        ),
    ];
    let cell_refs: [&[u32]; 9] = [&[0, 1], &[], &[], &[], &[], &[0], &[1, 2], &[], &[2]];
    let mut refs = Vec::new();
    for (cell, cell_refs) in cells.iter_mut().zip(cell_refs) {
        cell.portal_ref_start = refs.len() as u32;
        cell.portal_ref_count = cell_refs.len() as u32;
        refs.extend_from_slice(cell_refs);
    }
    let ceiling = |y: f32, x0: f32, x1: f32, front_cell, back_cell| PortalData {
        polygon: vec![
            Vec3::new(x0, y, -4.0),
            Vec3::new(x1, y, -4.0),
            Vec3::new(x1, y, 4.0),
            Vec3::new(x0, y, 4.0),
        ],
        front_cell,
        back_cell,
    };
    let mut world = LevelWorld::new_visibility_only(
        cells,
        refs,
        CellLocatorChild::Cell(locator_cell as usize),
        Vec::new(),
        vec![
            ceiling(4.0, -4.0, 0.0, 0, 5),
            ceiling(4.0, 0.0, 4.0, 0, 6),
            ceiling(8.0, 0.0, 4.0, 6, 8),
        ],
        true,
    )
    .expect("planar fixture world is valid");
    world.environment_volumes.volumes = vec![water_volume()];
    world
}

/// One water volume whose surface covers cell 0's floor at y = 0, up-facing.
fn water_volume() -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: [
                [-4.0, 0.0, -4.0],
                [-4.0, 0.0, 4.0],
                [4.0, 0.0, 4.0],
                [4.0, 0.0, -4.0],
            ]
            .into_iter()
            .map(|position| EnvironmentSurfaceVertex {
                position,
                uv: [0.0; 2],
            })
            .collect(),
        }],
    }
}

/// The main camera: 2 m above the water, pitched down at it, so the ceiling
/// portals lie above its frustum while their reflections fill the mirror's.
pub(crate) fn camera() -> RenderCamera {
    let eye = Vec3::new(0.0, 2.0, 3.0);
    let target = Vec3::new(0.0, 0.0, -1.0);
    let view_matrix = rh::view::look_at_mat4(eye, target, Vec3::Y);
    RenderCamera {
        eye_position: eye,
        forward: (target - eye).normalize(),
        view_matrix,
        view_projection: rh::proj::directx::perspective(75.0_f32.to_radians(), 1.5, 0.1, 128.0)
            * view_matrix,
    }
}

/// The app's camera preparation at the fixture pose, from the real walk.
pub(crate) fn main_view(world: &LevelWorld) -> VisibleRenderPreparation {
    let camera = camera();
    VisibleRenderPreparation::for_level(
        world,
        camera.eye_position,
        camera.view_projection,
        &[],
        false,
        &mut PortalWalkScratch::new(),
        TimingGate::ON,
    )
}

/// A preparation with the fixture's water installed as a candidate.
pub(crate) fn installed(world: &LevelWorld) -> PlanarRenderPreparation {
    let mut mirror = PlanarRenderPreparation::default();
    mirror.install(world, &[0]);
    mirror
}

/// One frame's mirror at `quality`'s preset, after `main`.
pub(crate) fn prepare(
    mirror: &mut PlanarRenderPreparation,
    world: &LevelWorld,
    main: &VisibleRenderPreparation,
    quality: FluidReflectionQuality,
) {
    mirror.prepare(
        world,
        &camera(),
        main,
        planar_preset(quality),
        &[],
        &main.stats.cpu,
    );
}

pub(crate) fn cells(visible: &VisibleCells) -> &[u32] {
    match visible {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => panic!("the fixture culls explicit cells"),
    }
}
