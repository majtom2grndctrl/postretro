//! Adapter-backed proof that a probe binding glossy floors at two heights
//! tests the lower floor's capture shadow from each fragment's true position:
//! where the capture saw a surface between the capture point and the lower
//! floor, the overlay adds exactly nothing there, while the floor it was
//! captured over keeps its term.
//! See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::{UVec2, Vec3};
use postretro_level_format::reflection_probes::{
    ReflectionProbeWorldBinding, ReflectionProbesSection,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{View, centre, on_plane, pixels, view_from};
use super::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, GlossyFrame, PROBE_COLOUR, floor_probe_of, glossy_frame, renderer,
    set_floor_material, with_faces, with_variant,
};

/// The lower floor's height; the upper floor lies at y = 0, the capture
/// point 2 mm over it.
const LOWER: f32 = -2.0;
/// The floors' extents, `(x, z)`: the upper west of x = 0, the lower east.
const UPPER_X: [f32; 2] = [-3.0, 0.0];
const LOWER_X: [f32; 2] = [0.0, 3.0];
const FLOOR_Z: [f32; 2] = [1.0, 9.0];
/// A tile fine enough that the horizon reads none of the below-horizon
/// texels this proof fills (about 3° a texel).
const TILE: u32 = 64;
/// Below-horizon directions steeper than this (sine) hold the captured
/// surface; every lower-floor fragment in view lies further below.
const BLOCKED_BELOW: f32 = -0.2;
/// The CPU edge margin, metres.
const MARGIN: f32 = 0.1;

/// The camera, 1.5 m over the upper floor looking down the room: the upper
/// floor fills the left of the frame, the lower floor the right.
fn view() -> View {
    view_from(Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.0, 0.0, 7.0))
}

/// The upper floor (face 0) and the lower floor (face 1), both glossy.
fn faces() -> Vec<FixtureFace> {
    let up = postretro_level_format::octahedral::encode(0.0, 1.0, 0.0);
    let quad = |x: [f32; 2], y: f32| FixtureFace {
        corners: [
            [x[0], y, FLOOR_Z[0]],
            [x[1], y, FLOOR_Z[0]],
            [x[1], y, FLOOR_Z[1]],
            [x[0], y, FLOOR_Z[1]],
        ],
        normal_oct: up,
        bucket: 0,
    };
    vec![quad(UPPER_X, 0.0), quad(LOWER_X, LOWER)]
}

/// One probe binding both floors, captured 2 mm over the upper one, its box
/// holding both. With `blocked`, its distance tile holds a surface 0.1 m
/// from the capture point in every direction steeper below the horizon than
/// [`BLOCKED_BELOW`], and nothing elsewhere: something the capture point saw
/// between it and the whole lower floor, and nothing it saw over either
/// floor's plane. Without, it holds nothing anywhere.
fn two_height_probe(blocked: bool) -> ReflectionProbesSection {
    use postretro_level_format::octahedral::{
        irradiance_interior_texel_direction, irradiance_tile_source_texel,
    };
    use postretro_render_cpu::sh_volume::f32_to_f16_bits;
    let mut section = floor_probe_of(true, PROBE_COLOUR);
    let texel = section.tiles[..8].to_vec();
    section.tile_dimension = TILE;
    section.tiles = texel.repeat((TILE * TILE) as usize);
    section.world_bindings = vec![
        ReflectionProbeWorldBinding { face: 0, probe: 0 },
        ReflectionProbeWorldBinding { face: 1, probe: 0 },
    ];
    section.probes[0].box_min = [-20.0, -5.0, -20.0];
    section.probes[0].box_max = [20.0, 10.0, 20.0];
    section.probes[0].capture_point = [-1.5, 0.002, 5.0];
    section.distances = (0..TILE)
        .flat_map(|y| (0..TILE).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let [ix, iy] = irradiance_tile_source_texel(x, y, TILE, 1);
            let direction = Vec3::from(irradiance_interior_texel_direction(ix, iy, TILE, 1));
            let stored = if blocked && direction.y < BLOCKED_BELOW {
                1.0 / (0.1 * direction.abs().element_sum())
            } else {
                0.0
            };
            f32_to_f16_bits(stored).to_le_bytes()
        })
        .collect();
    section
}

/// The two-height level at Low (the probe term alone), smooth style.
fn two_height_renderer(blocked: bool) -> Option<Renderer> {
    let mut renderer = renderer()?;
    let probes = two_height_probe(blocked);
    with_faces(&faces(), Some(&probes), None, |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    renderer.install_fluid_surfaces(&[], Some(&probes));
    set_floor_material(&mut renderer, FloorMaterial::WHITE_SHARP);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    Some(renderer)
}

/// Production, then the zero-adding variant (forward's colour).
fn production_and_forward(renderer: &mut Renderer) -> (GlossyFrame, GlossyFrame) {
    let production = glossy_frame(renderer, view());
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view())
    });
    (production, forward)
}

/// Whether `pixel` sees the floor of extent `x` at height `y`, `MARGIN`
/// inside it.
fn on_floor(pixel: UVec2, x: [f32; 2], y: f32) -> bool {
    on_plane(view(), centre(pixel), y).is_some_and(|point| {
        point.x > x[0] + MARGIN
            && point.x < x[1] - MARGIN
            && point.z > FLOOR_Z[0] + MARGIN
            && point.z < FLOOR_Z[1] - MARGIN
    })
}

/// Per floor, how many of its pixels the overlay added to and left alone.
fn added_and_left(
    production: &GlossyFrame,
    forward: &GlossyFrame,
    x: [f32; 2],
    y: f32,
) -> [u32; 2] {
    let mut counts = [0, 0];
    for pixel in pixels().filter(|&pixel| on_floor(pixel, x, y)) {
        let added = production.colour(pixel) != forward.colour(pixel);
        counts[usize::from(!added)] += 1;
    }
    counts
}

// Owner ruling: one probe may bind floors at two heights, capturing 2 mm over
// the upper. A lower-floor fragment tests its capture shadow from its true
// position, below the capture point's plane: with a surface captured between
// the capture point and the lower floor, every lower-floor pixel takes
// exactly forward's colour (the term withheld, nothing filled), while the
// upper floor, tested at its own plane where the capture saw nothing, keeps
// its term. With nothing captured, both floors take it, so the lower floor
// is bound and drawn.
#[test]
fn a_lower_floor_behind_a_captured_surface_takes_no_probe_term() {
    let Some(mut blocked) = two_height_renderer(true) else {
        return;
    };
    let mut open = two_height_renderer(false).expect("a second renderer");

    let (production, forward) = production_and_forward(&mut blocked);
    let [upper_added, upper_left] = added_and_left(&production, &forward, UPPER_X, 0.0);
    let [lower_added, lower_left] = added_and_left(&production, &forward, LOWER_X, LOWER);
    assert!(
        upper_added > 100 && upper_left == 0,
        "upper floor: {upper_added} added, {upper_left} left"
    );
    assert!(
        lower_left > 100 && lower_added == 0,
        "lower floor: {lower_added} added, {lower_left} left"
    );

    let (production, forward) = production_and_forward(&mut open);
    let [upper_added, upper_left] = added_and_left(&production, &forward, UPPER_X, 0.0);
    let [lower_added, lower_left] = added_and_left(&production, &forward, LOWER_X, LOWER);
    assert!(
        upper_added > 100 && upper_left == 0,
        "open, upper floor: {upper_added} added, {upper_left} left"
    );
    assert!(
        lower_added > 100 && lower_left == 0,
        "open, lower floor: {lower_added} added, {lower_left} left"
    );
}
