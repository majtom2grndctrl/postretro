//! Fixtures for the glossy overlay's lifecycle and gate proofs: the floor
//! level installed with a cell draw index and fluid volumes, frames through
//! culled cells, movers over the floor, D_static's guard mark read back, and
//! the composite row checked per pixel.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::{Mat4, UVec2, Vec3};
use postretro_level_format::cell_draw_index::Span;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::geometry::Vertex;
use postretro_level_format::reflection_probes::{
    ReflectionProbeFluidBinding, ReflectionProbeWorldBinding, ReflectionProbesSection,
};
use postretro_level_loader::{CellDrawIndex, KinematicGeometry, ShDrainBatch};
use postretro_render_cpu::fluid_surface::reflection::reflection_fresnel;
use postretro_render_cpu::planar_reflection::MirrorCamera;
use postretro_render_cpu::sh_compose::f16_bits_to_f32;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::fluid_reflection_frame_test_fixtures::{
    FRAME, MAGENTA_UV, View, centre, on_plane, pixels, read_back, read_scene,
};
use super::glossy_overlay_frame_test_fixtures::{
    FACES, FloorMaterial, GlossyFrame, PROBE_COLOUR, face_at, floor_probe, half_step,
    near_any_face, read_static_diffuse, set_floor_material, with_floor_indexed,
};
use super::{ClearColor, Renderer, ShSampleRegionSets};

/// The CPU edge margin, metres: pixels this close to a face edge are left out
/// of per-face expectations.
pub(super) const MARGIN: f32 = 0.05;

/// The mover the guard proofs draw.
pub(super) const MOVER: u32 = 1;

/// The floor's probe binding faces 0 and 1, so the second glossy face is
/// bound as well (a coplanar mover lies over it).
pub(super) fn both_glossy_faces_bound() -> ReflectionProbesSection {
    let mut probes = floor_probe(true);
    probes
        .world_bindings
        .push(ReflectionProbeWorldBinding { face: 1, probe: 0 });
    probes
}

/// A cell draw index whose one cell holds only the floor's non-glossy face
/// (leaf 2): a frame culled to that cell has camera ranges over no glossy
/// bucket, while a draw-all frame keeps every bucket.
pub(super) fn matte_only_cell_index() -> CellDrawIndex {
    CellDrawIndex {
        cell_count: 1,
        span_count: 1,
        cell_span_offset: vec![0, 1],
        spans: vec![Span {
            leaf_start: 2,
            leaf_count: 1,
        }],
    }
}

/// Install the floor level with `probes` as its id 54 and `index` as its
/// cell draw index, then `volumes` and the probes, with
/// [`FloorMaterial::WHITE_SHARP`] as the glossy faces' material.
pub(super) fn install_floor_with(
    renderer: &mut Renderer,
    probes: &ReflectionProbesSection,
    index: Option<&CellDrawIndex>,
    volumes: &[EnvironmentVolumeRecord],
) {
    with_floor_indexed(Some(probes), index, |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    renderer.install_fluid_surfaces(volumes, Some(probes));
    set_floor_material(renderer, FloorMaterial::WHITE_SHARP);
}

/// The draw-all visibility every frame of the floor proofs sees by default.
pub(super) const ALL: VisibleCells = VisibleCells::DrawAll;

/// One frame through `view` at time 0 seeing `cells`, inside a validation
/// error scope, read back.
pub(super) fn frame_seeing(
    renderer: &mut Renderer,
    view: View,
    cells: &VisibleCells,
) -> GlossyFrame {
    let path = match cells {
        VisibleCells::DrawAll => VisibilityPath::EmptyWorldFallback,
        VisibleCells::Culled(cells) => VisibilityPath::PrlPortal {
            walk_reach: cells.len() as u32,
        },
    };
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    renderer
        .capture_frame_indirect(
            CameraCullVisibility { cells, path },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: cells,
                fog_cells: &[],
                movers: &[],
            },
            None,
            view.view_proj,
            view.eye,
            &[],
            &[],
            0.0,
            ClearColor {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            true,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records");
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "the frame raised a validation error: {error:?}"
    );
    GlossyFrame {
        scene: read_scene(renderer),
        static_diffuse: read_static_diffuse(renderer),
    }
}

/// D_static's alpha, the coplanar-mover guard's mark, or `None` when the
/// target is not allocated.
pub(super) fn read_guard_marks(renderer: &Renderer) -> Option<Vec<f32>> {
    let texture = renderer
        .level()
        .glossy
        .as_ref()?
        .static_diffuse_texture()?
        .clone();
    Some(
        read_back(renderer, &texture, 8, wgpu::TextureAspect::All)
            .as_chunks::<8>()
            .0
            .iter()
            .map(|texel| f16_bits_to_f32(u16::from_le_bytes([texel[6], texel[7]])))
            .collect(),
    )
}

/// The value at `pixel` of a per-pixel grid at [`FRAME`]².
pub(super) fn at<T: Copy>(grid: &[T], pixel: UVec2) -> T {
    grid[(pixel.y * FRAME + pixel.x) as usize]
}

/// A mover whose one quad is floor face `face` exactly (both windings, its
/// corners and order), so at the identity it is coplanar with that face and
/// rasterizes to the same depth.
pub(super) fn coplanar_mover(face: usize) -> KinematicGeometry {
    let (x, z) = FACES[face];
    let corners = [
        [x[0], 0.0, z[0]],
        [x[1], 0.0, z[0]],
        [x[1], 0.0, z[1]],
        [x[0], 0.0, z[1]],
    ];
    mover_of(&corners, [0.0, 1.0, 0.0], [1.0, 0.0, 0.0])
}

/// A mover standing upright on the floor: a two-sided quad 1.4 m wide and
/// 1.8 m tall about its bottom centre, at its origin, facing ±Z.
pub(super) fn upright_mover() -> KinematicGeometry {
    let corners = [
        [-0.7, 0.0, 0.0],
        [0.7, 0.0, 0.0],
        [0.7, 1.8, 0.0],
        [-0.7, 1.8, 0.0],
    ];
    mover_of(&corners, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0])
}

/// One mover over the quad `corners`, drawn with both windings (the culled
/// one never shades).
pub(super) fn mover_of(
    corners: &[[f32; 3]; 4],
    normal: [f32; 3],
    tangent: [f32; 3],
) -> KinematicGeometry {
    let vertices = corners
        .iter()
        .map(|&corner| Vertex::new(corner, MAGENTA_UV, normal, tangent, true, [0.0, 0.0], 0))
        .collect();
    KinematicGeometry {
        movers: vec![postretro_level_loader::LoadedKinematicMover {
            mover_id: MOVER,
            name: "glossy-guard-mover".into(),
            tags: Vec::new(),
            origin: Vec3::ZERO,
            path: String::new(),
            speed_mps: 1.0,
            wait_ms: 0.0,
            move_mode: 1,
            start_on_spawn: false,
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2],
            face_meta: Vec::new(),
            spin_axis: Vec3::ZERO,
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".into(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            open_sound: None,
            close_sound: None,
            blocked_sound: None,
            crush_sound: None,
            sealed_portal_ids: Vec::new(),
            carried_lights: Vec::new(),
            crush_damage_type: None,
            light_ramp: Default::default(),
        }],
        waypoints: Vec::new(),
    }
}

/// Replace the level's mover geometry with `geometry`.
pub(super) fn install_mover(renderer: &mut Renderer, geometry: &KinematicGeometry) {
    let device = renderer.device.clone();
    renderer
        .level_mut()
        .kinematic
        .replace_geometry(&device, geometry, 1);
}

/// Draw the mover at `transform` this frame, or no mover.
pub(super) fn set_mover(renderer: &mut Renderer, transform: Option<Mat4>) {
    let movers: Vec<_> = transform
        .map(|transform| crate::render::KinematicMoverInstance {
            mover_id: MOVER,
            transform,
        })
        .into_iter()
        .collect();
    renderer.set_kinematic_mover_draws(&movers, &movers);
}

/// Mover guard passes recorded so far.
pub(super) fn guards(renderer: &Renderer) -> u32 {
    renderer.full().glossy_overlay.guard().recorded()
}

/// Assert the composite row on every pixel of `view` the CPU can call and
/// `skip` leaves: on a floor face in `bound` ([`FACES`] numbering),
/// production is forward + W·(R − D_static) within one half-float step, W
/// the head-on-0.04 Schlick weight (capture-shadow weight 1: zero
/// distances); everywhere else it is forward's, bit for bit. Returns the
/// bound pixels checked.
pub(super) fn assert_composite(
    view: View,
    production: &GlossyFrame,
    forward: &GlossyFrame,
    bound: &[usize],
    skip: &dyn Fn(UVec2) -> bool,
    what: &str,
) -> usize {
    let mut checked = 0;
    for pixel in pixels().filter(|&pixel| !skip(pixel)) {
        match face_at(view, pixel, MARGIN) {
            Some(face) if bound.contains(&face) => {
                let point = on_plane(view, centre(pixel), 0.0).expect("on the floor");
                let weight = reflection_fresnel((view.eye - point).normalize().y, 2.0);
                let expected =
                    forward.colour(pixel) + weight * (PROBE_COLOUR - forward.static_diffuse(pixel));
                let actual = production.colour(pixel);
                for c in 0..3 {
                    // f32 rasterization against the CPU's ray-plane point
                    // moves W by about 1e-6.
                    let tolerance = half_step(expected[c]) + 1.0e-5;
                    assert!(
                        (actual[c] - expected[c]).abs() <= tolerance,
                        "{what}: {pixel} channel {c} is {} against {}",
                        actual[c],
                        expected[c]
                    );
                }
                checked += 1;
            }
            Some(_) => assert_eq!(
                production.colour(pixel),
                forward.colour(pixel),
                "{what}: W = 0 off a bound face leaves forward's colour at {pixel}"
            ),
            None if !near_any_face(view, pixel, MARGIN) => assert_eq!(
                production.colour(pixel),
                forward.colour(pixel),
                "{what}: off the floor at {pixel}"
            ),
            None => {}
        }
    }
    checked
}

/// A fluid volume of one up-facing face at height `y` over `x` × `z`, its
/// UVs one texel per metre.
pub(super) fn pool(x: [f32; 2], z: [f32; 2], y: f32) -> EnvironmentVolumeRecord {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, y, z],
        uv: [x, z],
    };
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: vec![
                corner(x[0], z[0]),
                corner(x[0], z[1]),
                corner(x[1], z[1]),
                corner(x[1], z[0]),
            ],
        }],
    }
}

/// `probes` with the first fluid volume's one face bound to probe 0 too.
pub(super) fn binding_the_pool(mut probes: ReflectionProbesSection) -> ReflectionProbesSection {
    probes.fluid_bindings = vec![ReflectionProbeFluidBinding {
        volume: 0,
        face: 0,
        probe: 0,
    }];
    probes
}

/// The mirror camera of the plane at height `h` through `view`, built as
/// the planar proofs build it.
pub(super) fn mirror_camera(view: View, h: f32) -> MirrorCamera {
    let projection =
        glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
    let main_view = projection.inverse() * view.view_proj;
    MirrorCamera::new(view.eye, main_view, projection, h, None).expect("the eye is above the plane")
}
