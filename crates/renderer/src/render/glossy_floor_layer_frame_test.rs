//! Adapter-backed proofs of the glossy floor layer's march and composite: a
//! mover and a character standing on a bound glossy floor reflect in it
//! through the layered form on the frame they move, under every material
//! weight and tint; a coplanar mover takes nothing from either layer; the
//! floor layer is the same at every capture-shadow weight; a monument
//! reflects from the first frame after a live low → medium switch and not at
//! Low.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{Mat4, UVec2, Vec3};
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{FRAME, View, centre, on_plane, pixels};
use super::ghost_depth_frame_test::install_skinned_model;
use super::glossy_floor_frame_test_fixtures::{
    FloorLayer, SPECULAR_WALL, install_faces, read_floor_layer, room_faces, room_view,
};
use super::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, GlossyFrame, floor_probe, floor_view, renderer,
    set_bucket_material, set_floor_material, with_probe_term_alone, with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{
    ALL, Layers, assert_layered_composite, at, coplanar_mover, frame_seeing, install_floor_with,
    install_mover, read_guard_marks, set_mover, upright_mover,
};
use super::glossy_overlay_guard_frame_test::{CHARACTER, character_at, character_quads};
use FluidReflectionQuality::{High, Low, Medium};

/// Where the mover and the character stand on the bound face, and where
/// they move to.
const MOVER_AT: [Vec3; 2] = [Vec3::new(-0.8, 0.0, 3.5), Vec3::new(-1.6, 0.0, 4.5)];
const CHARACTER_AT: [Vec3; 2] = [Vec3::new(-2.2, 0.0, 6.5), Vec3::new(-1.2, 0.0, 6.0)];

/// The floor level with a mover and a character installed, lit by the
/// ambient floor so both read apart from the floor; `probes` as id 54. In
/// smooth style, whose read the reference's bilinear tap mirrors (pixel
/// style's per-texel read is `glossy_overlay_style_frame_test`'s).
fn install_standing(renderer: &mut Renderer, probes: &ReflectionProbesSection) {
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_floor_with(renderer, probes, None, &[]);
    renderer.set_ambient_floor(1.0);
    install_mover(renderer, &upright_mover());
    install_skinned_model(renderer, CHARACTER, &character_quads());
}

/// Draw the mover and the character at pose `pose`, or neither.
fn stand(renderer: &mut Renderer, pose: Option<usize>) {
    set_mover(
        renderer,
        pose.map(|pose| Mat4::from_translation(MOVER_AT[pose])),
    );
    let draws: Vec<_> = pose
        .map(|pose| character_at(CHARACTER_AT[pose]))
        .into_iter()
        .collect();
    renderer.set_mesh_draws(&draws);
}

/// One pose's frames: production, the floor layer it left, the probe term
/// alone, forward's colour and D_static (the zero-adding variant), and the
/// coplanar-mover guard's marks.
struct Frames {
    production: GlossyFrame,
    floor: Option<FloorLayer>,
    probe_alone: GlossyFrame,
    forward: GlossyFrame,
    marks: Vec<f32>,
}

fn frames(renderer: &mut Renderer, view: View) -> Frames {
    let production = frame_seeing(renderer, view, &ALL);
    let floor = read_floor_layer(renderer);
    let marks = read_guard_marks(renderer).expect("D_static is allocated");
    let probe_alone =
        with_probe_term_alone(renderer, |renderer| frame_seeing(renderer, view, &ALL));
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    Frames {
        production,
        floor,
        probe_alone,
        forward,
        marks,
    }
}

/// The pixels a mover or character covers: the guard marks them, or
/// forward's colour there differs from forward's without them.
fn covered<'a>(with: &'a Frames, without: &'a GlossyFrame) -> impl Fn(UVec2) -> bool + 'a {
    move |pixel| {
        at(&with.marks, pixel) == 1.0 || with.forward.colour(pixel) != without.colour(pixel)
    }
}

/// Pixels of the bound face whose floor-layer read is mostly confident.
fn reflecting(view: View, floor: &FloorLayer, skip: &dyn Fn(UVec2) -> bool) -> usize {
    pixels()
        .filter(|&pixel| !skip(pixel))
        .filter(|&pixel| {
            super::glossy_overlay_frame_test_fixtures::face_at(view, pixel, 0.05) == Some(0)
                && floor.bilinear(pixel, UVec2::splat(FRAME)).0.w > 0.5
        })
        .count()
}

/// The materials the layered composite is checked under: uncoloured at full
/// weight; a yellow tint-1 metal, whose reflection takes its albedo in both
/// layers; a yellow texel without `_t`, coloured by raw albedo; half `_s`;
/// and `_s` 0, which draws no term in either layer.
const MATERIALS: [(&str, FloorMaterial); 5] = [
    ("white", FloorMaterial::WHITE_SHARP),
    (
        "tint-1 yellow",
        FloorMaterial {
            albedo: [255, 255, 0, 255],
            slot: [255, 0, 255, 254],
            glow: None,
        },
    ),
    (
        "untinted yellow",
        FloorMaterial {
            albedo: [255, 255, 0, 255],
            slot: [255, 0, 255, 255],
            glow: None,
        },
    ),
    (
        "half _s",
        FloorMaterial {
            slot: [128, 0, 255, 0],
            ..FloorMaterial::WHITE_SHARP
        },
    ),
    (
        "_s 0",
        FloorMaterial {
            slot: [0, 0, 255, 0],
            ..FloorMaterial::WHITE_SHARP
        },
    ),
];

// Acceptance rows 37, 4, 27, 28 (both layers) and 45 (GPU half). At High a
// mover and a character standing on a bound glossy floor reflect in its
// floor layer; every bound pixel they leave uncovered is the layered form of
// the CPU reference over that frame's floor layer (F·colour·[c·X + (1 −
// c)·R] − W·D, s = 1), within one f16 step, under every material: the tint
// colours both layers, half `_s` halves both, `_s` 0 draws nothing from
// either. The unbound glossy face and the matte face take nothing (forward's
// colour bit for bit, the floor layer clear there). On the frame they move
// the floor layer is the one a fresh frame at the new pose leaves, bit for
// bit, and differs from the last pose's; at Low the floor layer is gone and
// the frame is the probe term's.
#[test]
fn movers_and_characters_reflect_through_the_layered_form_on_the_frame_they_move() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_standing(&mut renderer, &floor_probe(true));
    renderer.set_reflection_quality(High);
    let view = floor_view();
    for (name, material) in MATERIALS {
        set_floor_material(&mut renderer, material);
        stand(&mut renderer, None);
        let without = frames(&mut renderer, view);
        stand(&mut renderer, Some(0));
        let with = frames(&mut renderer, view);
        let floor = with.floor.as_ref().expect("High runs the floor layer");
        let skip = covered(&with, &without.forward);
        let reflecting = reflecting(view, floor, &skip);
        assert!(
            reflecting > 20,
            "{name}: the floor reflects them over {reflecting} pixels"
        );
        let layers = Layers {
            floor: Some(floor),
            material,
            shadow: 1.0,
        };
        let checked = assert_layered_composite(
            view,
            &with.production,
            &with.forward,
            &[0],
            &layers,
            &skip,
            name,
        );
        assert!(checked > 200, "{name}: {checked} bound pixels");
        // The floor layer moves the frame where it is confident, but for a
        // texel of no weight.
        let moved = pixels()
            .filter(|&pixel| with.production.colour(pixel) != with.probe_alone.colour(pixel))
            .count();
        if material.slot[0] == 0 {
            assert_eq!(moved, 0, "{name}: no term in either layer");
            assert_eq!(with.production.scene.colour, with.forward.scene.colour);
        } else {
            assert!(
                moved >= reflecting / 2,
                "{name}: {moved} pixels take the floor layer"
            );
        }
        // Off the bound face the floor layer is clear: the unbound glossy
        // face and the matte face never take it.
        let scene = UVec2::splat(FRAME);
        for pixel in pixels() {
            if let Some(face @ (1 | 2)) =
                super::glossy_overlay_frame_test_fixtures::face_at(view, pixel, 0.3)
            {
                assert_eq!(
                    floor.nearest(centre(pixel), scene).w,
                    0.0,
                    "{name}: face {face} at {pixel} takes the floor layer"
                );
            }
        }
    }

    // The frame they move: the floor layer is the new pose's alone.
    set_floor_material(&mut renderer, FloorMaterial::WHITE_SHARP);
    stand(&mut renderer, Some(0));
    let before = frames(&mut renderer, view);
    stand(&mut renderer, Some(1));
    let moved = frames(&mut renderer, view);
    stand(&mut renderer, None);
    frames(&mut renderer, view);
    stand(&mut renderer, Some(1));
    let fresh = frames(&mut renderer, view);
    assert_ne!(moved.floor, before.floor, "the reflection moves with them");
    assert_eq!(
        moved.floor, fresh.floor,
        "the frame they move holds no trace"
    );
    assert_eq!(moved.production.scene.colour, fresh.production.scene.colour);

    // At Low no floor layer exists and the frame is the probe term's.
    renderer.set_reflection_quality(Low);
    let low = frames(&mut renderer, view);
    assert!(low.floor.is_none(), "no floor layer at Low");
    assert_eq!(low.production.scene.colour, low.probe_alone.scene.colour);
    renderer.release_level_resources();
}

// Acceptance row 49 and the guard (plan Corrections): a mover face coplanar
// with the bound glossy face takes nothing from either layer. Where the
// floor under it reflects a character standing behind, the floor layer is
// confident, and still every pixel the guard marks is the mover pass's
// output bit for bit.
#[test]
fn a_coplanar_mover_takes_nothing_from_either_layer() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor_with(&mut renderer, &floor_probe(true), None, &[]);
    renderer.set_ambient_floor(1.0);
    renderer.set_reflection_quality(High);
    install_mover(&mut renderer, &coplanar_mover(0));
    install_skinned_model(&mut renderer, CHARACTER, &character_quads());
    set_mover(&mut renderer, Some(Mat4::IDENTITY));
    renderer.set_mesh_draws(&[character_at(Vec3::new(-1.5, 0.0, 7.5))]);
    let view = floor_view();
    let with = frames(&mut renderer, view);
    let floor = with.floor.as_ref().expect("High runs the floor layer");
    let (mut marked, mut confident) = (0, 0);
    for pixel in pixels() {
        if at(&with.marks, pixel) != 1.0 {
            continue;
        }
        marked += 1;
        if floor.bilinear(pixel, UVec2::splat(FRAME)).0.w > 0.0 {
            confident += 1;
        }
        assert_eq!(
            with.production.colour(pixel),
            with.forward.colour(pixel),
            "{pixel}: the coplanar mover takes no term"
        );
    }
    assert!(marked > 100, "the mover covers the bound face: {marked}");
    assert!(
        confident > 10,
        "the floor layer is confident under the mover over {confident} pixels"
    );
    renderer.release_level_resources();
}

/// [`floor_probe`] with every distance texel at inverse L1 distance 2: a
/// surface half a metre (L1) from the capture point all round, so every
/// floor point further out lies in capture shadow (s = 0).
fn shadowing_probe() -> ReflectionProbesSection {
    let mut probes = floor_probe(true);
    probes.distances = [0x00, 0x40].repeat(probes.distances.len() / 2);
    probes
}

// Acceptance rows 12 (GPU half) and 40 (fence B1): the floor layer's target
// is bit for bit the same with every floor point in capture shadow (s = 0)
// as with none (s = 1), and at s = 0 every bound pixel away from the capture
// point is the layered form with no probe share, F·colour·c·X − F·c·D: the
// screen-space layer neither widens, renormalises nor fills where the probe
// term is withheld, and its contribution is the same at every s.
#[test]
fn the_floor_layer_is_the_same_at_every_capture_shadow_weight() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let view = floor_view();
    let mut at_shadow = Vec::new();
    for probes in [floor_probe(true), shadowing_probe()] {
        install_standing(&mut renderer, &probes);
        renderer.set_reflection_quality(High);
        stand(&mut renderer, None);
        let without = frames(&mut renderer, view);
        stand(&mut renderer, Some(0));
        let with = frames(&mut renderer, view);
        at_shadow.push((with, without));
        renderer.release_level_resources();
    }
    let [(lit, _), (shadowed, without)] = &at_shadow[..] else {
        unreachable!()
    };
    let floor = shadowed.floor.as_ref().expect("the floor layer");
    assert!(floor.confident() > 20);
    assert_eq!(lit.floor, shadowed.floor, "the floor layer reads no s");
    assert_ne!(
        lit.production.scene.colour, shadowed.production.scene.colour,
        "the fixture withholds the probe term"
    );
    let capture = Vec3::from(floor_probe(true).probes[0].capture_point);
    let skip = |pixel: UVec2| {
        covered(shadowed, &without.forward)(pixel)
            || on_plane(view, centre(pixel), 0.0)
                .is_none_or(|point| (point - capture).abs().element_sum() < 2.0)
    };
    let layers = Layers {
        floor: Some(floor),
        material: FloorMaterial::WHITE_SHARP,
        shadow: 0.0,
    };
    let checked = assert_layered_composite(
        view,
        &shadowed.production,
        &shadowed.forward,
        &[0],
        &layers,
        &skip,
        "s = 0",
    );
    assert!(checked > 100, "{checked} bound pixels in capture shadow");
    assert!(
        reflecting(view, floor, &skip) > 10,
        "the floor reflects them inside the capture shadow"
    );
}

/// A matte box 0.6 m square and 1.2 m tall standing on the room's floor at
/// x 1.5, z 5: its three faces toward [`room_view`], in the `_s`-only
/// wall's bucket.
fn monument_faces() -> Vec<FixtureFace> {
    let (x0, x1, z0, z1, h) = (1.2, 1.8, 4.7, 5.3, 1.2);
    let oct = |x: f32, y: f32, z: f32| postretro_level_format::octahedral::encode(x, y, z);
    vec![
        FixtureFace {
            corners: [[x0, 0.0, z0], [x1, 0.0, z0], [x1, h, z0], [x0, h, z0]],
            normal_oct: oct(0.0, 0.0, -1.0),
            bucket: 1,
        },
        FixtureFace {
            corners: [[x0, 0.0, z0], [x0, h, z0], [x0, h, z1], [x0, 0.0, z1]],
            normal_oct: oct(-1.0, 0.0, 0.0),
            bucket: 1,
        },
        FixtureFace {
            corners: [[x0, h, z0], [x1, h, z0], [x1, h, z1], [x0, h, z1]],
            normal_oct: oct(0.0, 1.0, 0.0),
            bucket: 1,
        },
    ]
}

/// The room with `faces`, at Low then on the first Medium frame after the
/// live switch: `(low, medium)` production frames, and Medium's floor layer.
fn low_then_medium(renderer: &mut Renderer, faces: &[FixtureFace]) -> (Frames, Frames) {
    renderer.set_reflection_quality(Low);
    install_faces(renderer, faces, &floor_probe(true));
    set_bucket_material(renderer, 0, FloorMaterial::WHITE_SHARP, true);
    set_bucket_material(renderer, 1, SPECULAR_WALL, false);
    renderer.set_ambient_floor(0.5);
    let view = room_view();
    let low = frames(renderer, view);
    renderer.set_reflection_quality(Medium);
    let medium = frames(renderer, view);
    renderer.release_level_resources();
    (low, medium)
}

// Acceptance row 17 (GPU half, pin P16): a freestanding monument on a glossy
// floor reflects in the floor layer on the first frame after a live low →
// medium switch, and at Low it is absent from the reflection: there every
// pixel it leaves uncovered is the monument-free room's, bit for bit.
#[test]
fn a_monument_reflects_from_the_first_medium_frame_and_not_at_low() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = room_faces();
    let mut with_monument = room.clone();
    with_monument.extend(monument_faces());
    let (low, medium) = low_then_medium(&mut renderer, &with_monument);
    let (bare_low, bare_medium) = low_then_medium(&mut renderer, &room);
    assert!(
        low.floor.is_none() && bare_low.floor.is_none(),
        "no floor layer at Low"
    );
    let uncovered = |pixel: UVec2| low.forward.colour(pixel) == bare_low.forward.colour(pixel);
    let monument_pixels = pixels().filter(|&pixel| !uncovered(pixel)).count();
    assert!(
        monument_pixels > 20,
        "the monument stands in view: {monument_pixels}"
    );
    for pixel in pixels().filter(|&pixel| uncovered(pixel)) {
        assert_eq!(
            low.production.colour(pixel),
            bare_low.production.colour(pixel),
            "Low, {pixel}: the monument is absent from the reflection"
        );
    }
    let floor = medium.floor.as_ref().expect("Medium runs the floor layer");
    let bare = bare_medium
        .floor
        .as_ref()
        .expect("Medium runs the floor layer");
    assert_eq!(floor.extent, UVec2::splat(FRAME / 4), "Medium's ¼ target");
    let reflected = pixels()
        .filter(|&pixel| uncovered(pixel))
        .filter(|&pixel| {
            medium.production.colour(pixel) != bare_medium.production.colour(pixel)
                && floor.bilinear(pixel, UVec2::splat(FRAME)).0
                    != bare.bilinear(pixel, UVec2::splat(FRAME)).0
        })
        .count();
    assert!(
        reflected > 8,
        "the monument reflects on the first Medium frame over {reflected} pixels"
    );
}
