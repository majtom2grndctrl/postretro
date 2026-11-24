//! Adapter-backed proofs of the overlay's coplanar-mover guard: a mover face
//! coplanar with a glossy face shows the mover pass's output with no term, a
//! mover or character in front of one takes no overlay fragment, and the next
//! frame's floor takes the term again, from that frame's own mark.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::{Mat4, Vec3};
use postretro_level_format::geometry::Vertex;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_cpu::mesh_instances::{
    MeshInstanceInput, MeshPaletteCacheKey, MeshPresentation, MirrorReach,
};

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{MAGENTA_UV, pixels};
use super::ghost_depth_frame_test::install_skinned_model;
use super::glossy_overlay_frame_test_fixtures::{
    GlossyFrame, face_at, floor_probe, floor_view, overlays, renderer, with_probe_term_alone,
    with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{
    ALL, MARGIN, at, both_glossy_faces_bound, coplanar_mover, frame_seeing, guards,
    install_floor_with, install_mover, read_guard_marks, set_mover, upright_mover,
};

/// The production frame, then the zero-adding and constant-adding variants,
/// at one pose and clock, with the guard marks each frame left.
struct Frames {
    /// Production, with the floor layer at the default tier.
    production: GlossyFrame,
    /// Production compositing the probe term alone: what the floor shows
    /// apart from what it reflects of the scene.
    probe_alone: GlossyFrame,
    marks: Vec<f32>,
    forward: GlossyFrame,
    counted: GlossyFrame,
}

fn frames(renderer: &mut Renderer) -> Frames {
    let view = floor_view();
    let probe_alone =
        with_probe_term_alone(renderer, |renderer| frame_seeing(renderer, view, &ALL));
    let production = frame_seeing(renderer, view, &ALL);
    let marks = read_guard_marks(renderer).expect("D_static is allocated");
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    let counted = with_variant(renderer, "fs_test_constant", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    Frames {
        production,
        probe_alone,
        marks,
        forward,
        counted,
    }
}

// Acceptance row 49, pins P15 and P23. A mover face coplanar with a bound
// glossy face (the face's own quad at the identity) shades over it, and the
// overlay's Equal lands there too (the constant variant counts a fragment
// on each such pixel): the guard alone withholds the term, so those pixels
// are the mover pass's output, bit for bit. The other bound face keeps its
// term. On the next frame, with the mover moved away (still drawn, off the
// floor) or culled (no mover draw at all), every pixel is the mover-free
// frame's bit for bit: the mark is that frame's alone, never an earlier one.
#[test]
fn a_coplanar_mover_takes_no_term_and_the_next_frame_takes_it_again() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor_with(&mut renderer, &both_glossy_faces_bound(), None, &[]);
    install_mover(&mut renderer, &coplanar_mover(1));
    let view = floor_view();

    set_mover(&mut renderer, None);
    let guards_before = guards(&renderer);
    let without = frame_seeing(&mut renderer, view, &ALL);
    assert_eq!(guards(&renderer), guards_before, "no mover: no guard pass");
    assert!(
        read_guard_marks(&renderer)
            .expect("D_static")
            .iter()
            .all(|&mark| mark == 0.0),
        "forward clears and writes the mark to 0"
    );

    set_mover(&mut renderer, Some(Mat4::IDENTITY));
    let overlays_before = overlays(&renderer);
    let with = frames(&mut renderer);
    assert_eq!(
        guards(&renderer),
        guards_before + 4,
        "one guard pass a frame"
    );
    assert_eq!(overlays(&renderer), overlays_before + 4);
    let constant = Vec3::splat(0.125);
    let (mut covered, mut kept) = (0, 0);
    for pixel in pixels() {
        match face_at(view, pixel, MARGIN) {
            Some(1) => {
                assert_eq!(at(&with.marks, pixel), 1.0, "the guard marks {pixel}");
                assert_eq!(
                    with.production.colour(pixel),
                    with.forward.colour(pixel),
                    "the coplanar mover's pixel {pixel} takes no term"
                );
                assert_eq!(
                    with.counted.colour(pixel),
                    with.forward.colour(pixel) + constant,
                    "the overlay's Equal lands on {pixel}: the guard withholds"
                );
                covered += 1;
            }
            Some(0) => {
                assert_eq!(at(&with.marks, pixel), 0.0, "{pixel} is unmarked");
                assert_eq!(
                    with.production.colour(pixel),
                    without.colour(pixel),
                    "the face beside the mover keeps its term at {pixel}"
                );
                if with.production.colour(pixel) != with.forward.colour(pixel) {
                    kept += 1;
                }
            }
            _ => {}
        }
    }
    assert!(covered > 100, "the mover covers its face: {covered}");
    assert!(kept > 100, "the other face draws the term: {kept}");

    // The next frame: moved away, still drawn; then culled, no mover draw.
    for (what, transform) in [
        (
            "moved away",
            Some(Mat4::from_translation(Vec3::new(0.0, 0.0, -40.0))),
        ),
        ("culled", None),
    ] {
        set_mover(&mut renderer, transform);
        let next = frame_seeing(&mut renderer, view, &ALL);
        assert!(
            read_guard_marks(&renderer)
                .expect("D_static")
                .iter()
                .all(|&mark| mark == 0.0),
            "{what}: no mark survives"
        );
        assert_eq!(
            next.scene.colour, without.scene.colour,
            "{what}: the floor takes the term again, as with no mover"
        );
        let returned = pixels()
            .filter(|&pixel| face_at(view, pixel, MARGIN) == Some(1))
            .filter(|&pixel| next.colour(pixel) != with.forward.colour(pixel))
            .count();
        assert!(
            returned > 100,
            "{what}: the term returns on {returned} pixels"
        );
    }
    renderer.release_level_resources();
}

/// A skinned model: a two-sided quad 1.4 m wide and 1.8 m tall about its
/// bottom centre, on the placeholder checkerboard.
pub(super) const CHARACTER: &str = "glossy-guard/character";

pub(super) fn character_quads() -> [[Vertex; 4]; 2] {
    let corners = [
        Vec3::new(-0.7, 0.0, 0.0),
        Vec3::new(0.7, 0.0, 0.0),
        Vec3::new(0.7, 1.8, 0.0),
        Vec3::new(-0.7, 1.8, 0.0),
    ];
    let vertex = |corner: Vec3, normal: [f32; 3]| {
        Vertex::new(
            corner.to_array(),
            MAGENTA_UV,
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

pub(super) fn character_at(at: Vec3) -> MeshInstanceInput {
    MeshInstanceInput {
        model: postretro_model::ModelHandle::from(CHARACTER),
        transform: Mat4::from_translation(at),
        shadow_bias_scale: 1.0,
        phase_seed: 1,
        palette_cache_key: MeshPaletteCacheKey::Entity(1),
        sample: MeshSampleParams::rigid(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible: true,
        dynamic_shadow_visible: false,
        mirror: MirrorReach::Hidden,
        presentation: MeshPresentation::World,
    }
}

// Acceptance row 49, pin P15: a mover and a character standing on a bound
// glossy face, in front of it, take no overlay fragment: their pixels are the
// same with the production overlay (the floor layer included) and with one
// that adds nothing, and with one that adds a constant per fragment. Every
// floor pixel they leave uncovered takes the probe term of the frame without
// them, bit for bit.
#[test]
fn movers_and_characters_in_front_of_a_glossy_face_take_no_term() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor_with(&mut renderer, &floor_probe(true), None, &[]);
    // Lit by the ambient floor, the character's checkerboard reads apart
    // from the white floor.
    renderer.set_ambient_floor(1.0);
    install_mover(&mut renderer, &upright_mover());
    install_skinned_model(&mut renderer, CHARACTER, &character_quads());
    let view = floor_view();

    set_mover(&mut renderer, None);
    renderer.set_mesh_draws(&[]);
    let without = frames(&mut renderer);

    set_mover(
        &mut renderer,
        Some(Mat4::from_translation(Vec3::new(-0.8, 0.0, 3.5))),
    );
    renderer.set_mesh_draws(&[character_at(Vec3::new(-2.2, 0.0, 6.5))]);
    let with = frames(&mut renderer);
    let (mut mover, mut character, mut floor) = (0, 0, 0);
    for pixel in pixels() {
        let marked = at(&with.marks, pixel) == 1.0;
        let drawn = with.forward.colour(pixel) != without.forward.colour(pixel);
        if marked || drawn {
            assert_eq!(
                with.production.colour(pixel),
                with.forward.colour(pixel),
                "{pixel}: no term over a mover or character"
            );
            assert_eq!(
                with.counted.colour(pixel),
                with.forward.colour(pixel),
                "{pixel}: no overlay fragment lands on it"
            );
            if marked {
                mover += 1;
            } else {
                character += 1;
            }
        } else {
            // The floor reflects them through its floor layer; its probe
            // term is unchanged.
            assert_eq!(
                with.probe_alone.colour(pixel),
                without.probe_alone.colour(pixel),
                "{pixel}: the floor they leave uncovered is unchanged"
            );
            if face_at(view, pixel, MARGIN) == Some(0)
                && with.production.colour(pixel) != with.forward.colour(pixel)
            {
                floor += 1;
            }
        }
    }
    assert!(mover >= 8, "the mover covers the floor: {mover}");
    assert!(
        character >= 8,
        "the character covers the floor: {character}"
    );
    assert!(floor > 100, "the rest of the face draws the term: {floor}");
    renderer.release_level_resources();
}
