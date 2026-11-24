//! Adapter-backed proofs of what the glossy overlay leaves to later passes and
//! to the planar mirror: translucent passes draw over its result undimmed,
//! the mirror view records no overlay and matches the level without glossy
//! faces, plane selection's renderer side is unchanged by a glossy floor, and
//! the mirror's frames stay valid across tier changes and a resize.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Planar layer)

use glam::{Mat4, UVec2, Vec3};
use postretro_level_format::reflection_probes::ReflectionProbeWorldBinding;
use postretro_render_cpu::fluid_surface::FluidSurfaceLook;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::ghost_instance::{GhostInstanceLook, KinematicGhostInstance};
use postretro_render_cpu::planar_reflection::{PlanarPreset, planar_preset};
use postretro_visibility::VisibleCells;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{
    FRAME, View, centre, mirror, on_plane, pixels, read_back, view_from,
};
use super::fluid_reflection_sums_frame_test_fixtures::frame_at;
use super::fluid_surface_frame_test::look_draw;
use super::glossy_floor_frame_test_fixtures::read_floor_layer;
use super::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, GlossyFrame, face_at, floor_faces, floor_probe, floor_view,
    overlays, renderer, set_floor_material, with_faces, with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{
    ALL, Layers, MARGIN, MOVER, assert_layered_composite, binding_the_pool, frame_seeing,
    install_floor_with, install_mover, mirror_camera, pool, upright_mover,
};
use FluidReflectionQuality::{Low, Off, VeryHigh};

/// The pool's extent and height: over the floor's non-glossy face, larger
/// than the planar area floor and smaller than the two glossy floor faces
/// together, so the glossy floor outsizes every pool.
const POOL_X: [f32; 2] = [-3.0, 3.0];
const POOL_Z: [f32; 2] = [10.0, 15.0];
const POOL_Y: f32 = 0.3;

/// The glossy wall the pool reflects: upright at the floor's far edge,
/// facing the camera, in the glossy bucket.
const WALL_Z: f32 = 16.0;

/// The mirror level: the floor, a glossy wall standing at its far edge
/// (face 2, the matte floor face after it as face 3), and a reflecting pool
/// over the matte face whose plane the mirror reflects the wall in. `glossy`
/// binds the first floor face and the wall to the probe; without it the
/// level is the same level before glossy faces.
fn install_mirror_level(renderer: &mut Renderer, glossy: bool) {
    let mut faces = floor_faces();
    faces.insert(
        2,
        FixtureFace {
            corners: [
                [-3.0, 0.0, WALL_Z],
                [3.0, 0.0, WALL_Z],
                [3.0, 3.0, WALL_Z],
                [-3.0, 3.0, WALL_Z],
            ],
            normal_oct: postretro_level_format::octahedral::encode(0.0, 0.0, -1.0),
            bucket: 0,
        },
    );
    let mut probes = binding_the_pool(floor_probe(glossy));
    if glossy {
        probes
            .world_bindings
            .push(ReflectionProbeWorldBinding { face: 2, probe: 0 });
    }
    with_faces(&faces, Some(&probes), None, |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    renderer.install_fluid_surfaces(&[pool(POOL_X, POOL_Z, POOL_Y)], Some(&probes));
    set_floor_material(renderer, FloorMaterial::WHITE_SHARP);
    // Lit by the ambient floor, the wall reads in the mirror.
    renderer.set_ambient_floor(1.0);
}

/// [`install_mirror_level`] at very high, the pool's plane selected through
/// `view`.
fn mirror_renderer(glossy: bool, view: View) -> Option<Renderer> {
    let mut renderer = renderer()?;
    install_mirror_level(&mut renderer, glossy);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    renderer.set_reflection_quality(VeryHigh);
    // The planar layer at the scene's extent, so each water pixel holds its
    // own mirror texel.
    renderer.set_planar_preset_override(Some(PlanarPreset {
        scale: 1.0,
        ..planar_preset(VeryHigh).expect("a planar tier")
    }));
    select(&mut renderer, view);
    Some(renderer)
}

/// Pixels the composite row leaves to other proofs: the wall, and anything
/// past the floor's far edge.
fn past_the_floor(view: View) -> impl Fn(UVec2) -> bool {
    move |pixel| on_plane(view, centre(pixel), 0.0).is_none_or(|point| point.z > WALL_Z - 0.2)
}

fn select(renderer: &mut Renderer, view: View) {
    renderer.set_planar_reflection_view(Some(mirror_camera(view, POOL_Y)), &VisibleCells::DrawAll);
}

/// The planar layer the last frame left, raw bytes.
fn planar_layer(renderer: &Renderer) -> Vec<u8> {
    let texture = renderer
        .full()
        .fluid_surface
        .planar_layer_view()
        .expect("a planar layer")
        .texture()
        .clone();
    read_back(renderer, &texture, 8, wgpu::TextureAspect::All)
}

/// The mirror's depth the last frame left, raw bytes.
fn mirror_depth(renderer: &Renderer) -> Vec<u8> {
    let texture = renderer
        .planar_mirror_depth_texture()
        .expect("mirror depth");
    read_back(renderer, &texture, 4, wgpu::TextureAspect::DepthOnly)
}

/// Whether `pixel` sees the pool, `margin` metres inside it.
fn on_pool(view: View, pixel: UVec2, margin: f32) -> bool {
    on_plane(view, centre(pixel), POOL_Y).is_some_and(|point| {
        point.x > POOL_X[0] + margin
            && point.x < POOL_X[1] - margin
            && point.z > POOL_Z[0] + margin
            && point.z < POOL_Z[1] - margin
    })
}

/// The composite row on the bound face: the probe term with the floor
/// layer's share at very high (no planar share reaches a world face), the
/// probe term alone at Low, with no validation error, and at very high the
/// mirror recorded on both frames.
fn assert_bound_face_composite(renderer: &mut Renderer, view: View, mirror: bool, what: &str) {
    let records = renderer.planar_reflection_records();
    let production = frame_seeing(renderer, view, &ALL);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    assert_eq!(
        renderer.planar_reflection_records(),
        records + if mirror { 2 } else { 0 },
        "{what}: the mirror records each frame at very high"
    );
    let skip = past_the_floor(view);
    // At very high the floor runs its floor layer: the bound face takes the
    // layered form over the floor layer this frame left.
    let floor = read_floor_layer(renderer);
    assert_eq!(
        floor.is_some(),
        mirror,
        "{what}: the floor layer runs at very high alone"
    );
    let layers = Layers {
        floor: floor.as_ref(),
        ..Layers::PROBE_ALONE
    };
    let checked = assert_layered_composite(view, &production, &forward, &[0], &layers, &skip, what);
    assert!(checked > 100, "{what}: {checked} bound pixels");
}

/// The planar layer's texels with confidence: what the mirror resolved.
fn mirrored_texels(layer: &[u8]) -> usize {
    layer
        .as_chunks::<8>()
        .0
        .iter()
        .filter(|texel| texel[6..8] != [0, 0])
        .count()
}

// Acceptance rows 59 and 60, pins P10 and P22. At very high with the pool's
// plane selected, a glossy level records one overlay pass a frame, the main
// view's: the mirror records none. The mirror's world colour (the planar
// layer it resolves) and depth, and the water reading it, are bit for bit
// the level's without glossy faces. Every frame records with no validation
// error; the bound face takes the probe term alone beside the mirror, on the
// first frames after off → low, low → very high and a resize.
#[test]
fn the_mirror_records_no_overlay_and_matches_the_level_before_glossy_faces() {
    let view = floor_view();
    let Some(mut glossy) = mirror_renderer(true, view) else {
        return;
    };
    let mut before = mirror_renderer(false, view).expect("a second renderer");
    assert!(before.level().glossy.is_none());

    let overlays_before = overlays(&glossy);
    let records = glossy.planar_reflection_records();
    let with = frame_seeing(&mut glossy, view, &ALL);
    assert_eq!(
        glossy.planar_reflection_records(),
        records + 1,
        "the mirror records"
    );
    assert_eq!(
        overlays(&glossy),
        overlays_before + 1,
        "one overlay pass, the main view's: the mirror records none"
    );
    let without = frame_seeing(&mut before, view, &ALL);
    let layer = planar_layer(&glossy);
    let reflecting = mirrored_texels(&layer);
    assert!(
        reflecting >= 10,
        "the pool mirrors the glossy wall: {reflecting} texels"
    );
    assert_eq!(
        layer,
        planar_layer(&before),
        "the mirror's world colour is the level's before glossy faces"
    );
    assert_eq!(mirror_depth(&glossy), mirror_depth(&before));
    let water = pixels()
        .filter(|&pixel| on_pool(view, pixel, MARGIN))
        .inspect(|&pixel| assert_eq!(with.colour(pixel), without.colour(pixel), "{pixel}"))
        .count();
    assert!(water > 30, "the pool is on screen: {water}");

    assert_bound_face_composite(&mut glossy, view, true, "very high");
    glossy.set_reflection_quality(Off);
    frame_seeing(&mut glossy, view, &ALL);
    glossy.set_reflection_quality(Low);
    assert_bound_face_composite(&mut glossy, view, false, "the first low frame after off");
    glossy.set_reflection_quality(VeryHigh);
    assert_bound_face_composite(&mut glossy, view, true, "very high after low");
    glossy.record_surface_size(48, 40);
    frame_at(&mut glossy, view, 0.0);
    glossy.record_surface_size(FRAME, FRAME);
    assert_bound_face_composite(&mut glossy, view, true, "the first frame after a resize");
}

// Acceptance row 58 (renderer half). At very high, a level whose glossy
// floor outsizes its pool keeps the renderer's planar side as the level
// without glossy faces does, on every frame of a camera path with the pool's
// plane selected: the planar layer and mirror targets exist at the same
// extent, and the mirror resolves the same layer, bit for bit.
#[test]
fn a_glossy_floor_leaves_the_planar_side_as_before_along_a_camera_path() {
    let path = [
        floor_view(),
        view_from(Vec3::new(-1.0, 1.8, 1.0), Vec3::new(0.0, 0.3, 12.0)),
        view_from(Vec3::new(1.2, 1.2, -1.0), Vec3::new(0.5, 0.0, 9.0)),
    ];
    let Some(mut glossy) = mirror_renderer(true, path[0]) else {
        return;
    };
    let mut before = mirror_renderer(false, path[0]).expect("a second renderer");
    assert_eq!(
        glossy.planar_mirror_extent(),
        before.planar_mirror_extent(),
        "the same planar targets"
    );
    assert!(glossy.planar_mirror_extent().is_some());
    let mut mirrored = 0;
    for view in path {
        for renderer in [&mut glossy, &mut before] {
            select(renderer, view);
            let records = renderer.planar_reflection_records();
            frame_seeing(renderer, view, &ALL);
            assert_eq!(renderer.planar_reflection_records(), records + 1);
        }
        let layer = planar_layer(&glossy);
        mirrored += mirrored_texels(&layer);
        assert_eq!(layer, planar_layer(&before), "{view:?}");
        assert_eq!(mirror_depth(&glossy), mirror_depth(&before), "{view:?}");
    }
    assert!(
        mirrored >= 10,
        "the path mirrors the wall: {mirrored} texels"
    );
}

/// A fluid that hides what lies under it: full opacity, no reflection.
fn opaque_fluid() -> FluidSurfaceLook {
    FluidSurfaceLook::surface_only(1.0)
}

const OPAQUE_GHOST: GhostInstanceLook = GhostInstanceLook {
    opacity: 1.0,
    saturation: 1.0,
    tint: [1.0, 1.0, 1.0],
};

/// Production and the zero-adding variant through `view`.
fn both(renderer: &mut Renderer, view: View) -> (GlossyFrame, GlossyFrame) {
    let production = frame_seeing(renderer, view, &ALL);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    (production, forward)
}

// Acceptance row 46 (GPU half), pin P12. An opaque fluid surface and an
// opaque mover ghost drawn over a bound glossy face cover the overlay's
// result: their pixels are bit for bit the same with the production overlay
// and with one that adds nothing, though without them the overlay adds its
// term at those pixels. So the overlay is beneath them, and nothing they draw
// is scaled by (1 − W).
#[test]
fn a_fluid_and_a_ghost_draw_over_the_overlay_undimmed() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (pool_x, pool_z) = ([-2.5, -0.5], [2.5, 5.0]);
    install_floor_with(
        &mut renderer,
        &floor_probe(true),
        None,
        &[pool(pool_x, pool_z, POOL_Y)],
    );
    install_mover(&mut renderer, &upright_mover());
    let view = floor_view();

    renderer.set_fluid_surface_draws(&[]);
    renderer.set_kinematic_ghost_draws(&[]);
    let (bare, bare_forward) = both(&mut renderer, view);

    renderer.set_fluid_surface_draws(&[look_draw(0, opaque_fluid())]);
    renderer.set_kinematic_ghost_draws(&[KinematicGhostInstance {
        mover_id: MOVER,
        transform: Mat4::from_translation(Vec3::new(-1.5, 0.0, 7.0)),
        look: OPAQUE_GHOST,
    }]);
    let (covered, covered_forward) = both(&mut renderer, view);

    let under_fluid = |pixel: UVec2| {
        on_plane(view, centre(pixel), POOL_Y).is_some_and(|point| {
            point.x > pool_x[0] + MARGIN
                && point.x < pool_x[1] - MARGIN
                && point.z > pool_z[0] + MARGIN
                && point.z < pool_z[1] - MARGIN
        })
    };
    let (mut fluid, mut ghost) = (0, 0);
    for pixel in pixels() {
        let on_glossy = face_at(view, pixel, MARGIN) == Some(0);
        let termed = bare.colour(pixel) != bare_forward.colour(pixel);
        let ghosted = covered_forward.colour(pixel) != bare_forward.colour(pixel);
        if under_fluid(pixel) || ghosted {
            assert_eq!(
                covered.colour(pixel),
                covered_forward.colour(pixel),
                "{pixel}: drawn over the overlay, never dimmed by it"
            );
            if on_glossy && termed {
                if under_fluid(pixel) {
                    fluid += 1;
                } else {
                    ghost += 1;
                }
            }
        }
    }
    assert!(fluid > 30, "the fluid covers termed glossy pixels: {fluid}");
    assert!(ghost >= 8, "the ghost covers termed glossy pixels: {ghost}");
}
