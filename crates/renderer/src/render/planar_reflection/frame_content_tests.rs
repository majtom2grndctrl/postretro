//! Adapter-backed mirror content proofs: billboards turned to the mirror, mirror-only meshes,
//! ghosts and the viewmodel kept out, and the main view untouched by mirror content.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::frame_test_fixtures::*;
use super::frame_tests::{MOVER, mover_geometry};
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, View, capture_checked, pixels, rgb,
};
use crate::render::fluid_surface_frame_test::look_draw;
use crate::render::ghost_depth_frame_test::install_skinned_model;
use glam::{Mat4, UVec2, Vec3};
use postretro_level_format::geometry::Vertex;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::ghost_instance::{
    GhostInstanceLook, KinematicGhostInstance, pack_ghost_sprite_instance,
};
use postretro_render_cpu::mesh_instances::{
    MeshInstanceInput, MeshPaletteCacheKey, MeshPresentation, MirrorReach,
};
use postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE;

/// The skinned twin of the AC 12 mover: a two-sided magenta quad, 1.4 m wide
/// and 1.8 m tall about its bottom centre, at its model origin.
const SKINNED: &str = "planar-content/quad";

/// The two sprite collections: a projectile-style billboard and a pool
/// particle, over one isotropic smoke sheet, both emissive so they read
/// bright against the black wall they reflect over.
const BILLBOARD: &str = "planar-content/billboard";
const PARTICLE: &str = "planar-content/particle";

fn two_sided_quad() -> [[Vertex; 4]; 2] {
    let corners = [
        Vec3::new(-0.7, 0.0, 0.0),
        Vec3::new(0.7, 0.0, 0.0),
        Vec3::new(0.7, 1.8, 0.0),
        Vec3::new(-0.7, 1.8, 0.0),
    ];
    let vertex = |corner: Vec3, normal: [f32; 3]| {
        Vertex::new(
            corner.to_array(),
            fixture::MAGENTA_UV,
            normal,
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    };
    let mut back = corners;
    back.reverse();
    [
        corners.map(|corner| vertex(corner, [0.0, 0.0, 1.0])),
        back.map(|corner| vertex(corner, [0.0, 0.0, -1.0])),
    ]
}

/// A world instance of [`SKINNED`] with its bottom centre at `at`.
fn skinned_at(
    seed: u32,
    at: Vec3,
    forward_visible: bool,
    mirror: MirrorReach,
) -> MeshInstanceInput {
    MeshInstanceInput {
        model: postretro_model::ModelHandle::from(SKINNED),
        transform: Mat4::from_translation(at),
        shadow_bias_scale: 1.0,
        phase_seed: seed,
        palette_cache_key: MeshPaletteCacheKey::Entity(seed),
        sample: MeshSampleParams::rigid(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible,
        dynamic_shadow_visible: forward_visible,
        mirror,
        presentation: MeshPresentation::World,
    }
}

const OPAQUE_GHOST: GhostInstanceLook = GhostInstanceLook {
    opacity: 1.0,
    saturation: 1.0,
    tint: [1.0, 1.0, 1.0],
};

/// Register both sprite collections; first, while the upload queue is empty.
fn register_sprites(renderer: &mut Renderer) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    for collection in [BILLBOARD, PARTICLE] {
        renderer.register_smoke_collection(
            collection,
            "smoke_puff_isotropic",
            &root.join("textures"),
            &root.join("cache"),
            crate::render::SpriteCollectionRegistration {
                baked_sidecar_eligible: false,
                spec_intensity: 0.0,
                spec_exponent: 16.0,
                lifetime: 1.0,
                emissive: 4.0,
            },
        );
    }
}

/// One packed billboard instance, upright, at age 0 and full opacity.
fn sprite(position: Vec3, size: f32) -> Vec<u8> {
    let bytes: Vec<u8> = [position.x, position.y, position.z, 0.0, size, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect();
    assert_eq!(bytes.len(), SPRITE_INSTANCE_SIZE);
    bytes
}

/// The billboard scene: the base pool under a tall black wall, at very high
/// with the planar layer at full scale, seen from 4 m up looking 45° down,
/// the base pool's plane selected. The wall is tall enough that every
/// reflected sprite lands on it in the mirror.
fn billboard_renderer() -> Option<(Renderer, View)> {
    let mut renderer = fixture::ssr_renderer()?;
    register_sprites(&mut renderer);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    fixture::install_world_quads(
        &mut renderer,
        &[fixture::upright(
            [-6.0, 6.0],
            [0.0, 8.0],
            fixture::POOL_Z[1],
            fixture::BLACK_UV,
        )],
    );
    renderer.set_ambient_floor(1.0);
    // Bloom is a post-pass over the main view: it would spread the bright
    // reflected sprites past the water and blur the off-plane comparison.
    renderer.full_mut().bloom.set_enabled(false);
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    renderer.set_planar_preset_override(Some(full_scale()));
    let view = fixture::view_from(Vec3::new(0.0, 4.0, 0.0), Vec3::new(0.0, 0.0, 4.0));
    select(&mut renderer, view, 0.0);
    Some((renderer, view))
}

/// Pixels near the selected plane's water, outset by 0.3 m so the layer's
/// filtered edge stays inside: the only pixels mirror content may change.
fn near_the_plane(view: View, pixel: UVec2, x: [f32; 2]) -> bool {
    on_extent(view, pixel, 0.0, x, -0.3).is_some()
}

/// The rgb sum `frame` adds over `base` at `pixel`.
fn brightening(frame: &[u8], base: &[u8], pixel: UVec2) -> i32 {
    let (a, b) = (rgb(frame, pixel), rgb(base, pixel));
    (0..3).map(|c| i32::from(a[c]) - i32::from(b[c])).sum()
}

/// Rows and columns, within `radius` pixels of `centre`, where `frame`
/// brightens over `base` by at least `threshold`.
fn brightened_extent(
    frame: &[u8],
    base: &[u8],
    centre: UVec2,
    radius: u32,
    threshold: i32,
) -> (u32, u32) {
    let (mut rows, mut columns) = (Vec::new(), Vec::new());
    for pixel in pixels() {
        if pixel.x.abs_diff(centre.x) > radius || pixel.y.abs_diff(centre.y) > radius {
            continue;
        }
        if brightening(frame, base, pixel) >= threshold {
            rows.push(pixel.y);
            columns.push(pixel.x);
        }
    }
    let span = |values: &[u32]| match (values.iter().min(), values.iter().max()) {
        (Some(low), Some(high)) => high - low + 1,
        _ => 0,
    };
    (span(&rows), span(&columns))
}

// AC 13. A particle on the mirror camera's axis and a billboard over the top
// of the main view, both above the selected plane, land on their mirrored
// pixels in the water. Each faces the mirror camera: the axis particle,
// which a quad turned to the main camera would show edge-on from the mirror
// at this 45° view, covers several rows and columns there. Nothing off the
// plane changes, and the main view draws no sprite of its own here.
#[test]
fn planar_gpu_billboards_and_particles_face_the_mirror() {
    let Some((mut renderer, view)) = billboard_renderer() else {
        return;
    };
    // On the mirror camera's axis: its reflection is the screen centre.
    let particle = Vec3::new(0.0, 1.0, 5.0);
    // Over the top of the main view, its whole quad off screen.
    let billboard = Vec3::new(1.0, 4.5, 6.0);
    let (top, _) = fixture::project(view, billboard - Vec3::Y * 0.6);
    assert!(
        top.y < 0.0,
        "the billboard's lowest corner is over the top: {top}"
    );

    renderer.set_planar_particle_draws(std::iter::empty());
    let base = capture_checked(&mut renderer, view);
    let (particle_bytes, billboard_bytes) = (sprite(particle, 1.0), sprite(billboard, 0.8));
    renderer.set_planar_particle_draws([
        (PARTICLE, particle_bytes.as_slice()),
        (BILLBOARD, billboard_bytes.as_slice()),
    ]);
    let records = renderer.planar_reflection_records();
    let frame = capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records + 1);

    let reflected = |point: Vec3| Vec3::new(point.x, -point.y, point.z);
    for (name, point) in [("particle", particle), ("billboard", billboard)] {
        let pixel = pixel_of(view, reflected(point)).expect("its reflection is on screen");
        assert!(
            on_extent(view, pixel, 0.0, fixture::POOL_X, 0.05).is_some(),
            "{name}: its reflection lies on the pool"
        );
        let gain = brightening(&frame, &base, pixel);
        eprintln!(
            "[PlanarProof] {name} reflection at {pixel}: {:?} over {:?} (+{gain})",
            rgb(&frame, pixel),
            rgb(&base, pixel)
        );
        assert!(gain >= 30, "{name}: lands on its mirrored pixel");
    }

    let centre = pixel_of(view, reflected(particle)).expect("on screen");
    let (rows, columns) = brightened_extent(&frame, &base, centre, 8, 30);
    eprintln!("[PlanarProof] axis particle covers {rows} rows × {columns} columns");
    assert!(
        rows >= 3 && columns >= 3,
        "the axis particle faces the mirror camera: {rows} × {columns}"
    );

    let mut compared = 0;
    for pixel in pixels() {
        if near_the_plane(view, pixel, fixture::POOL_X) {
            continue;
        }
        compared += 1;
        assert_eq!(
            rgb(&frame, pixel),
            rgb(&base, pixel),
            "off-plane pixel {pixel} is the main view's alone"
        );
    }
    assert!(compared > 200, "{compared} off-plane pixels compared");
    renderer.release_level_resources();
}

/// The AC 12 scene with the mover and the skinned quad installed, the large
/// plane selected through the base view.
fn mesh_renderer() -> Option<(Renderer, View)> {
    let mut renderer = two_plane_renderer(
        FluidReflectionQuality::VeryHigh,
        FluidReflectionStyle::Smooth,
        &[standing_object(), fixture::back_wall()],
    )?;
    let device = renderer.device.clone();
    renderer
        .full_mut()
        .kinematic_brush
        .install_geometry(&device, Some(&mover_geometry()), 1);
    install_skinned_model(&mut renderer, SKINNED, &two_sided_quad());
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    Some((renderer, view))
}

/// Above the large plane and over the top of the main view: the mover's
/// pose and the skinned quad's, each reflecting at its own x.
const MOVER_AT: Vec3 = Vec3::new(-1.3, 4.2, 7.5);
const SKINNED_AT: Vec3 = Vec3::new(1.2, 4.2, 7.5);

/// Where a quad standing at `at` reflects: its centre, mirrored in height 0.
fn reflected_centre(at: Vec3) -> Vec3 {
    Vec3::new(at.x, -(at.y + 0.9), at.z)
}

fn set_mover(renderer: &mut Renderer, at: Option<Vec3>) {
    let movers: Vec<_> = at
        .into_iter()
        .map(|at| crate::render::KinematicMoverInstance {
            mover_id: MOVER,
            transform: Mat4::from_translation(at),
        })
        .collect();
    renderer.set_kinematic_mover_draws(&movers, &movers);
}

// AC 14. A mover and a skinned mesh only the mirror reaches, both over the
// top of the main view, land on their mirrored pixels on the selected
// plane. The main forward pass draws no mirror-only mesh: one standing in
// plain view leaves every off-plane pixel as the frame without it, while the
// same instance drawn forward changes them.
#[test]
fn planar_gpu_mirror_only_mover_and_skinned_mesh_reflect() {
    let Some((mut renderer, view)) = mesh_renderer() else {
        return;
    };
    for at in [MOVER_AT, SKINNED_AT] {
        let (bottom, _) = fixture::project(view, at);
        assert!(bottom.y < 0.0, "{at} is over the top of the main view");
    }
    let in_view = Vec3::new(2.5, 1.2, 7.0);

    set_mover(&mut renderer, None);
    renderer.set_mesh_draws(&[]);
    let base = capture_checked(&mut renderer, view);

    set_mover(&mut renderer, Some(MOVER_AT));
    renderer.set_mesh_draws(&[
        skinned_at(1, SKINNED_AT, false, MirrorReach::Only),
        skinned_at(2, in_view, false, MirrorReach::Only),
    ]);
    let frame = capture_checked(&mut renderer, view);
    for (name, at) in [("mover", MOVER_AT), ("skinned mesh", SKINNED_AT)] {
        let pixel = pixel_of(view, reflected_centre(at)).expect("its reflection is on screen");
        assert!(
            on_extent(view, pixel, 0.0, LARGE_X, 0.05).is_some(),
            "{name}: its reflection lies on the large plane"
        );
        eprintln!(
            "[PlanarProof] mirror-only {name} at {pixel}: {:?} over {:?}",
            rgb(&frame, pixel),
            rgb(&base, pixel)
        );
        assert!(magenta(rgb(&frame, pixel)), "{name} reflects");
        assert!(
            !magenta(rgb(&base, pixel)),
            "{name}: nothing else does there"
        );
    }
    let mut compared = 0;
    for pixel in pixels() {
        if near_the_plane(view, pixel, LARGE_X) {
            continue;
        }
        compared += 1;
        assert_eq!(
            rgb(&frame, pixel),
            rgb(&base, pixel),
            "pixel {pixel}: the main view draws no mirror-only mesh"
        );
    }
    assert!(compared > 200, "{compared} off-plane pixels compared");

    // The control: the in-view quad drawn forward shows off the plane.
    renderer.set_mesh_draws(&[skinned_at(2, in_view, true, MirrorReach::AlsoMain)]);
    let drawn = capture_checked(&mut renderer, view);
    let shown = pixels()
        .filter(|&pixel| !near_the_plane(view, pixel, LARGE_X))
        .filter(|&pixel| magenta(rgb(&drawn, pixel)) && !magenta(rgb(&base, pixel)))
        .count();
    assert!(
        shown >= 8,
        "a forward instance in view draws ({shown} pixels)"
    );
    renderer.release_level_resources();
}

// AC 15. Ghosts (skinned, mover and billboard) and the viewmodel at the
// skinned quad's pose above the selected plane, each even marked as the
// mirror's, add no texel to the planar layer; a real mirror-only mesh at that
// pose lands on its mirrored pixel in the same setup.
#[test]
fn planar_gpu_ghosts_and_viewmodel_stay_out_of_the_mirror() {
    let Some((mut renderer, view)) = mesh_renderer() else {
        return;
    };
    set_mover(&mut renderer, None);
    renderer.set_mesh_draws(&[]);
    renderer.set_ghost_mesh_draws(&[]);
    renderer.set_kinematic_ghost_draws(&[]);
    capture_checked(&mut renderer, view);
    let empty = read_planar_layer(&renderer);

    let ghost = MeshInstanceInput {
        palette_cache_key: MeshPaletteCacheKey::Ghost {
            link: 0,
            holder: 1,
            attachment_index: None,
        },
        dynamic_shadow_visible: false,
        presentation: MeshPresentation::Ghost(OPAQUE_GHOST),
        ..skinned_at(1, SKINNED_AT, true, MirrorReach::AlsoMain)
    };
    let viewmodel = MeshInstanceInput {
        presentation: MeshPresentation::Viewmodel,
        ..skinned_at(3, SKINNED_AT, true, MirrorReach::Only)
    };
    renderer.set_mesh_draws(&[viewmodel]);
    renderer.set_ghost_mesh_draws(&[ghost]);
    renderer.set_kinematic_ghost_draws(&[KinematicGhostInstance {
        mover_id: MOVER,
        transform: Mat4::from_translation(SKINNED_AT),
        look: OPAQUE_GHOST,
    }]);
    let mut ghost_sprites = Vec::new();
    pack_ghost_sprite_instance(
        SKINNED_AT + Vec3::Y * 0.9,
        0.0,
        1.0,
        0.0,
        OPAQUE_GHOST,
        &mut ghost_sprites,
    );
    renderer.set_billboard_ghost_draws([(BILLBOARD, ghost_sprites.as_slice())]);
    capture_checked(&mut renderer, view);
    assert!(renderer.full().kinematic_ghost.has_draws());
    let ghosts = read_planar_layer(&renderer);
    assert_eq!(ghosts.extent, empty.extent);
    let differing = ghosts
        .texels
        .iter()
        .zip(&empty.texels)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "no ghost or viewmodel texel enters the mirror"
    );

    renderer.set_mesh_draws(&[skinned_at(1, SKINNED_AT, false, MirrorReach::Only)]);
    renderer.set_ghost_mesh_draws(&[]);
    renderer.set_kinematic_ghost_draws(&[]);
    renderer.set_billboard_ghost_draws(std::iter::empty());
    let frame = capture_checked(&mut renderer, view);
    let pixel = pixel_of(view, reflected_centre(SKINNED_AT)).expect("on screen");
    eprintln!(
        "[PlanarProof] real mesh at the ghost's pose: {pixel} reads {:?}",
        rgb(&frame, pixel)
    );
    assert!(magenta(rgb(&frame, pixel)), "the real mesh reflects there");
    renderer.release_level_resources();
}

// Mirror recording stays on the frame encoder: the sprite store records into
// the pass it is handed and opens no encoder, bundle or thread of its own.
#[test]
fn planar_mirror_sprites_record_on_the_frame_encoder() {
    let sprites = include_str!("mirror_sprites.rs");
    for second_encoder in [
        "create_command_encoder",
        "RenderBundle",
        "thread::spawn",
        "rayon",
    ] {
        assert!(
            !sprites.contains(second_encoder),
            "mirror sprites stay on the frame encoder: `{second_encoder}`"
        );
    }
    let record = include_str!("record.rs");
    assert!(record.contains("full.mesh_pass.record_mirror_draws(&mut skinned, plan);"));
    assert!(record.contains("pass.sprites.record_draws(&mut sprites, &full.smoke_pass);"));
    for excluded in [
        "ghost_order",
        "kinematic_ghost",
        "billboard_ghost",
        "ghost_mesh_draws",
        ".viewmodel",
        ".ghost",
    ] {
        assert!(
            !record.contains(excluded),
            "no ghost or viewmodel source reaches the mirror: `{excluded}`"
        );
    }
}

// The mirror's group 0 is the main frame's with the mirror camera's raster
// view-projection and eye. The terms the world forward pass reads from the
// main view's group 5 at screen positions are off inside the mirror: the SDF
// factor flag clears and the SDF screen views shade as `On`, while the
// world-space shadowmask views and production modes pass through.
#[test]
fn planar_mirror_uniform_takes_the_mirror_camera_and_drops_screen_terms() {
    use postretro_render_cpu::frame_uniforms::{SdfShadowMode, UNIFORM_SIZE};
    let view = fixture::base_view();
    let camera = mirror_camera(view, 0.0);
    let word = |data: &[u8; UNIFORM_SIZE], at: usize| {
        u32::from_ne_bytes(data[at..at + 4].try_into().expect("a word"))
    };
    for (mode, mirrored) in [
        (SdfShadowMode::On, SdfShadowMode::On),
        (SdfShadowMode::Off, SdfShadowMode::Off),
        (SdfShadowMode::Visualize, SdfShadowMode::On),
        (SdfShadowMode::VisualizeDebugPaths, SdfShadowMode::On),
        (SdfShadowMode::VisualizeNormals, SdfShadowMode::On),
        (
            SdfShadowMode::ShadowmaskUnion,
            SdfShadowMode::ShadowmaskUnion,
        ),
        (
            SdfShadowMode::ShadowmaskRawPoolVisibility,
            SdfShadowMode::ShadowmaskRawPoolVisibility,
        ),
    ] {
        let mut main = [0xA5u8; UNIFORM_SIZE];
        main[96..100].copy_from_slice(&1u32.to_ne_bytes());
        main[100..104].copy_from_slice(&(mode as u32).to_ne_bytes());
        let data = super::mirror_camera_uniform(&main, &camera);
        let floats: Vec<f32> = (0..16)
            .map(|index| f32::from_bits(word(&data, index * 4)))
            .collect();
        assert_eq!(floats, camera.raster_vp.to_cols_array());
        let eye: Vec<f32> = (0..3)
            .map(|index| f32::from_bits(word(&data, 64 + index * 4)))
            .collect();
        assert_eq!(eye, camera.eye.to_array());
        assert_eq!(word(&data, 96), 0, "{mode:?}: no SDF factor in the mirror");
        assert_eq!(word(&data, 100), mirrored as u32, "{mode:?}");
        assert_eq!(&data[76..96], &main[76..96], "{mode:?}: the frame's terms");
        assert_eq!(&data[104..], &main[104..], "{mode:?}: the frame's tail");
    }
}
