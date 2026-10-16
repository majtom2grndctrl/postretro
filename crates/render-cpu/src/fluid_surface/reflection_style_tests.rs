// Tests: the reflection style reference. Acceptance row 26: every fragment
// of one pixel-style water texel cell reads one reflection sample, cells with
// different displayed offsets read different directions, smooth varies per
// fragment, and drift and a declared wave move no cell's reflection.

use std::cell::Cell;

use glam::{IVec2, Vec2, Vec3, Vec4};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, ReflectionProbeLightShare, ReflectionProbeRecord,
};

use super::*;
use crate::fluid_surface::motion::{FluidWaveParams, displayed_offset, warped_texel_uv};
use crate::fluid_surface::reflection::{
    FluidReflectionFragment, FluidReflectionLights, reflection_fragment,
};
use crate::frame_uniforms::LightTermMask;

const TILE: [u32; 2] = [128, 1];
/// One texel of the 128² tile, in normalized tile UV.
const TILE_TEXEL: f32 = 1.0 / 128.0;
/// Below the fade threshold's floor (0.75), so no cell flattens.
const NEAR_FOOTPRINT: f32 = 0.5;

/// A distance tile holding no surface: every search ends at the box hit,
/// plain box projection.
fn no_surface(_layer: u32, _uv: Vec2) -> f32 {
    0.0
}

/// A room-sized probe over the pool, capture point 2 mm above the water.
fn room_probe() -> ReflectionProbeRecord {
    ReflectionProbeRecord {
        source_entity: 0,
        box_min: [-6.0, -1.0, -6.0],
        box_max: [6.0, 3.5, 6.0],
        capture_point: [0.0, 0.002, 0.0],
        layer: 0,
        lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    }
}

/// [`room_probe`]'s box, min and max.
fn probe_box() -> (Vec3, Vec3) {
    let probe = room_probe();
    (Vec3::from(probe.box_min), Vec3::from(probe.box_max))
}

/// The pool's texture map, world `(x, z)` to texel UV: rotated, sheared and
/// offset, so no test leans on axis-aligned texels. About 10 texels a metre.
fn texel_of(world: Vec3) -> Vec2 {
    Vec2::new(
        9.0 * world.x + 3.0 * world.z + 0.37,
        -2.0 * world.x + 10.0 * world.z - 0.61,
    )
}

/// The world point at texel UV `texel` on the pool plane (`texel_of`'s
/// inverse).
fn world_of(texel: Vec2) -> Vec3 {
    let t = texel - Vec2::new(0.37, -0.61);
    // [[9, 3], [-2, 10]]⁻¹ = [[10, -3], [2, 9]] / 96.
    Vec3::new(
        (10.0 * t.x - 3.0 * t.y) / 96.0,
        0.0,
        (2.0 * t.x + 9.0 * t.y) / 96.0,
    )
}

/// A fragment at `world` with screen-step world vectors `dx`, `dy` (in the
/// pool plane; they differ per fragment under perspective), its texel
/// derivatives following from the texture map, as the GPU's would.
fn fragment(style: u32, world: Vec3, dx: Vec3, dy: Vec3, time: f32) -> FluidReflectionFragment {
    let texel_step = |step: Vec3| texel_of(step) - texel_of(Vec3::ZERO);
    FluidReflectionFragment {
        probe: 0,
        camera: Vec3::new(-1.0, 1.7, -4.0),
        world_position: world,
        outward: Vec3::Y,
        light_term_mask: LightTermMask::ALL.bits(),
        reflectivity: 1.0,
        style,
        texel_uv: texel_of(world),
        axes: reflection_texel_axes(texel_step(dx), texel_step(dy), dx, dy),
        time,
        texels_per_pixel: NEAR_FOOTPRINT,
    }
}

/// The tile UV a fragment samples, and its output over a fixed surface with
/// a capture whose colour varies with the tile UV.
fn sampled(fragment: &FluidReflectionFragment) -> (Vec2, Vec4) {
    let seen = Cell::new(None);
    let out = reflection_fragment(
        Vec4::new(0.2, 0.3, 0.4, 0.6),
        fragment,
        &[room_probe()],
        TILE,
        &FluidReflectionLights::NONE,
        |layer, uv| {
            assert_eq!(layer, 0);
            seen.set(Some(uv));
            Vec3::new(uv.x * 4.0, uv.y * 4.0, 1.0)
        },
        no_surface,
    );
    (
        seen.get().expect("a bound face seen from above samples"),
        out,
    )
}

/// Fragments spread over the cell at texel cell `cell`, each with its own
/// perspective footprint.
fn cell_fragments(style: u32, cell: IVec2, time: f32) -> Vec<FluidReflectionFragment> {
    [
        (Vec2::new(0.05, 0.05), 1.0),
        (Vec2::new(0.95, 0.07), 1.7),
        (Vec2::new(0.5, 0.5), 0.6),
        (Vec2::new(0.1, 0.92), 2.3),
        (Vec2::new(0.97, 0.96), 1.2),
    ]
    .into_iter()
    .map(|(inside, scale)| {
        let world = world_of(cell.as_vec2() + inside);
        let dx = Vec3::new(0.011, 0.0, 0.002) * scale;
        let dy = Vec3::new(-0.003, 0.0, 0.017) * scale * (1.0 + inside.y);
        fragment(style, world, dx, dy, time)
    })
    .collect()
}

/// A cell and a time at which its displayed offset is `wanted(offset)`.
fn cell_where(time: f32, wanted: impl Fn(IVec2) -> bool) -> IVec2 {
    (0..64)
        .flat_map(|x| (0..64).map(move |y| IVec2::new(x, y)))
        .find(|&cell| wanted(displayed_offset(cell, time, NEAR_FOOTPRINT)))
        .expect("some cell displays the wanted offset")
}

#[test]
fn every_fragment_of_one_pixel_cell_reads_one_reflection_sample() {
    let time = 3.3;
    for wanted in [
        (|offset: IVec2| offset == IVec2::ZERO) as fn(IVec2) -> bool,
        |offset| offset != IVec2::ZERO,
        |offset| offset.x != 0 && offset.y != 0,
    ] {
        let cell = cell_where(time, wanted);
        let fragments = cell_fragments(FLUID_REFLECTION_STYLE_PIXEL, cell, time);
        let (first_uv, first_out) = sampled(&fragments[0]);
        for fragment in &fragments[1..] {
            assert_eq!(
                motion_cell(fragment),
                cell,
                "the fixture's fragments lie in one cell"
            );
            let (uv, out) = sampled(fragment);
            assert!(
                uv.abs_diff_eq(first_uv, 1.0e-5),
                "cell {cell}: {uv} vs {first_uv}"
            );
            assert!(
                out.abs_diff_eq(first_out, 1.0e-4),
                "cell {cell}: one sample at one weight: {out} vs {first_out}"
            );
        }
    }
}

fn motion_cell(fragment: &FluidReflectionFragment) -> IVec2 {
    crate::fluid_surface::motion::texel_cell(fragment.texel_uv)
}

#[test]
fn smooth_style_varies_per_fragment_within_one_cell() {
    let time = 3.3;
    let cell = cell_where(time, |offset| offset != IVec2::ZERO);
    let fragments = cell_fragments(FLUID_REFLECTION_STYLE_SMOOTH, cell, time);
    let uvs: Vec<Vec2> = fragments.iter().map(|f| sampled(f).0).collect();
    for (i, a) in uvs.iter().enumerate() {
        for b in &uvs[i + 1..] {
            assert!(
                a.distance(*b) > 1.0e-4,
                "smooth reads per fragment: {a} vs {b}"
            );
        }
    }
    // Smooth reflects from the fragment about the flat normal, whatever the
    // cell displays.
    for fragment in &fragments {
        let point = reflection_point(
            fragment.style,
            fragment.texel_uv,
            fragment.world_position,
            Vec3::Y,
            fragment.axes,
            fragment.time,
            fragment.texels_per_pixel,
            probe_box().0,
            probe_box().1,
        );
        assert_eq!(point.position, fragment.world_position);
        assert_eq!(point.normal, Vec3::Y);
    }
}

// One cell over time: each displayed offset tilts its normal by
// atan(tilt × |offset|) toward the offset along the texture's axes, and
// different offsets read different tile texels. Seen at this eye height
// (about 22° above the water) a one-texel offset turns the lookup by one to
// a few tile texels, most along the view and least across it.
#[test]
fn cells_with_different_displayed_offsets_read_different_directions() {
    let cell = IVec2::new(11, 7);
    let mut by_offset: Vec<(IVec2, Vec2)> = Vec::new();
    for step in 0..200 {
        let time = step as f32 * 0.4 + 0.05;
        let offset = displayed_offset(cell, time, NEAR_FOOTPRINT);
        if by_offset.iter().any(|(seen, _)| *seen == offset) {
            continue;
        }
        let fragment = &cell_fragments(FLUID_REFLECTION_STYLE_PIXEL, cell, time)[2];
        let point = reflection_point(
            fragment.style,
            fragment.texel_uv,
            fragment.world_position,
            Vec3::Y,
            fragment.axes,
            time,
            NEAR_FOOTPRINT,
            probe_box().0,
            probe_box().1,
        );
        let along = reflection_unit_or_zero(fragment.axes.u) * offset.x as f32
            + reflection_unit_or_zero(fragment.axes.v) * offset.y as f32;
        let expected = (Vec3::Y + along * FLUID_REFLECTION_PIXEL_TILT).normalize();
        assert!(point.normal.abs_diff_eq(expected, 1.0e-6), "{offset}");
        if offset != IVec2::ZERO {
            assert!(
                point.normal.angle_between(Vec3::Y) > 0.04,
                "{offset}: tilted"
            );
        }
        by_offset.push((offset, sampled(fragment).0));
    }
    assert!(
        by_offset.len() >= 3,
        "the cell shows several offsets: {by_offset:?}"
    );
    for (i, (offset_a, uv_a)) in by_offset.iter().enumerate() {
        for (offset_b, uv_b) in &by_offset[i + 1..] {
            assert!(
                uv_a.distance(*uv_b) > 0.75 * TILE_TEXEL,
                "offsets {offset_a} and {offset_b} read {uv_a} and {uv_b}"
            );
        }
    }
}

// Drift scrolls the albedo beneath the fixed grid and the wave shifts it
// sideways, both continuously; neither is an input of the reflection, which
// keys on the unwarped cell and its displayed offset alone. Within one cell
// step the albedo of a drifting, waving pool moves while the reflection
// holds.
#[test]
fn drift_and_a_declared_wave_move_no_cells_reflection_direction() {
    let dims = Vec2::splat(64.0);
    let wave = FluidWaveParams {
        inv_period: 1.0 / 6.0,
        amplitude: 16.0,
        length_min: 128.0,
        length_max: 128.0,
    };
    let time = 3.3;
    let cell = cell_where(time, |offset| offset != IVec2::ZERO);
    let fragment = cell_fragments(FLUID_REFLECTION_STYLE_PIXEL, cell, time)[2];
    let (uv, out) = sampled(&fragment);
    let still = warped_texel_uv(
        fragment.texel_uv,
        Vec2::ZERO,
        dims,
        FluidWaveParams::NONE,
        time,
        NEAR_FOOTPRINT,
    );
    let moving = warped_texel_uv(
        fragment.texel_uv,
        Vec2::new(0.5, -0.25),
        dims,
        wave,
        time,
        NEAR_FOOTPRINT,
    );
    assert!(
        still.distance(moving) >= 1.0,
        "drift and wave move the albedo"
    );
    // Later in the same step the displayed offset holds, drift has scrolled
    // on, and the reflection reads the same sample.
    let step = crate::fluid_surface::motion::cell_step(cell, time);
    let later = (1..40)
        .map(|i| time + i as f32 * 0.01)
        .rfind(|&t| crate::fluid_surface::motion::cell_step(cell, t) == step)
        .expect("time left in the step");
    assert!(later > time);
    let drifted = warped_texel_uv(
        fragment.texel_uv,
        Vec2::new(0.5, -0.25),
        dims,
        wave,
        later,
        NEAR_FOOTPRINT,
    );
    assert_ne!(
        drifted, moving,
        "the albedo keeps scrolling within the step"
    );
    let held = FluidReflectionFragment {
        time: later,
        ..fragment
    };
    let (held_uv, held_out) = sampled(&held);
    assert!(held_uv.abs_diff_eq(uv, 1.0e-6), "{held_uv} vs {uv}");
    assert!(held_out.abs_diff_eq(out, 1.0e-6));
}

// The axes recover the texture map's texel-to-world Jacobian from any
// perspective footprint; a degenerate footprint keeps the fragment's own
// position and flat normal.
#[test]
fn texel_axes_recover_the_faces_texel_to_world_map() {
    let expected_u = world_of(Vec2::new(1.0, 0.0)) - world_of(Vec2::ZERO);
    let expected_v = world_of(Vec2::new(0.0, 1.0)) - world_of(Vec2::ZERO);
    for fragment in cell_fragments(FLUID_REFLECTION_STYLE_PIXEL, IVec2::new(3, -4), 1.0) {
        assert!(
            fragment.axes.u.abs_diff_eq(expected_u, 1.0e-6),
            "{:?}",
            fragment.axes
        );
        assert!(
            fragment.axes.v.abs_diff_eq(expected_v, 1.0e-6),
            "{:?}",
            fragment.axes
        );
        let centre =
            reflection_cell_centre(fragment.texel_uv, fragment.world_position, fragment.axes);
        let expected = world_of(fragment.texel_uv.floor() + Vec2::splat(0.5));
        assert!(
            centre.abs_diff_eq(expected, 1.0e-5),
            "{centre} vs {expected}"
        );
    }
    let edge_on = reflection_texel_axes(Vec2::new(1.0, 2.0), Vec2::new(2.0, 4.0), Vec3::X, Vec3::Z);
    assert_eq!(edge_on, FluidTexelAxes::default());
    let vanished = reflection_texel_axes(Vec2::ZERO, Vec2::ZERO, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(vanished, FluidTexelAxes::default());
    let world = Vec3::new(0.3, 0.0, 0.4);
    let cell = cell_where(1.0, |offset| offset != IVec2::ZERO);
    let point = reflection_point(
        FLUID_REFLECTION_STYLE_PIXEL,
        cell.as_vec2() + Vec2::splat(0.25),
        world,
        Vec3::Y,
        edge_on,
        1.0,
        NEAR_FOOTPRINT,
        probe_box().0,
        probe_box().1,
    );
    assert_eq!(point.position, world);
    assert_eq!(point.normal, Vec3::Y);
}

// The pixel style holds a cell centre to the probe's box at a wall rim. A
// centre inside the box, or only below its floor (a derived box's floor is
// the capture plane 2 mm above the water), stays put; one past a wall by up
// to half the cell's diagonal moves onto the wall's face; past that the pull
// tapers off, so a centre one cell (a whole diagonal) or more out stays put.
// Only the horizontal overshoot counts, the held point moves continuously
// with the centre across both knees, and the smooth style never moves.
#[test]
fn a_pixel_cell_centre_past_the_box_is_pulled_back_at_most_half_a_cell_and_never_beyond_one() {
    let axes = FluidTexelAxes {
        u: Vec3::X / 16.0,
        v: Vec3::Z / 16.0,
    };
    let half_cell = 0.5 * 2.0f32.sqrt() / 16.0;
    let (box_min, box_max) = (Vec3::new(-2.0, 0.002, -2.0), Vec3::new(2.0, 3.0, 2.0));
    let held = |centre: Vec3| reflection_cell_within_box(centre, axes, box_min, box_max);
    for centre in [
        Vec3::new(0.5, 0.0, -1.0),
        Vec3::new(1.99, 0.0, 1.99),
        Vec3::new(-2.0, 0.0, 0.3),
    ] {
        assert_eq!(held(centre), centre, "inside, below the floor");
    }
    for past in [0.001, 0.02, 0.9 * half_cell] {
        let centre = Vec3::new(0.3, 0.0, 2.0 + past);
        assert!(
            held(centre).abs_diff_eq(Vec3::new(0.3, 0.0, 2.0), 1.0e-6),
            "{past} m past the wall: {}",
            held(centre)
        );
    }
    assert!(
        held(Vec3::new(2.02, 0.0, -2.02)).abs_diff_eq(Vec3::new(2.0, 0.0, -2.0), 1.0e-6),
        "past a corner within half a diagonal: onto the corner"
    );
    // On the taper, an overshoot of 1.5 h is pulled back 0.5 h.
    let tapered = Vec3::new(0.3, 0.0, 2.0 + 1.5 * half_cell);
    assert!(
        held(tapered).abs_diff_eq(Vec3::new(0.3, 0.0, 2.0 + half_cell), 1.0e-6),
        "on the taper: {}",
        held(tapered)
    );
    for far in [
        Vec3::new(0.3, 0.0, 2.0 + 2.0 * half_cell),
        Vec3::new(0.3, 0.0, 2.0 + 3.0 * half_cell),
        Vec3::new(0.3, 0.0, 5.0),
        Vec3::new(-4.0, 0.0, -3.0),
    ] {
        assert!(
            held(far).abs_diff_eq(far, 1.0e-6),
            "a cell or more outside stays put: {far} → {}",
            held(far)
        );
    }
    // Swept from inside across both knees (h at 2.044, 2h at 2.088) to well
    // past them: the held point moves at most twice the centre's step (the
    // taper's slope), never jumps, and the pull never exceeds h.
    let step = 0.0005;
    let mut last = held(Vec3::new(0.3, 0.0, 1.9));
    for i in 1..=500 {
        let centre = Vec3::new(0.3, 0.0, 1.9 + i as f32 * step);
        let now = held(centre);
        assert!(
            now.distance(last) <= 2.0 * step + 1.0e-5,
            "continuous: {last} → {now}"
        );
        assert!(
            centre.distance(now) <= half_cell + 1.0e-5,
            "{centre} → {now}"
        );
        last = now;
    }

    // Through the style switch: a cell centred 2.8 cm into the +z wall.
    let world = Vec3::new(0.3, 0.0, 1.998);
    let texel_uv = Vec2::new(world.x * 16.0, world.z * 16.0 + 0.05);
    let point = |style| {
        reflection_point(
            style,
            texel_uv,
            world,
            Vec3::Y,
            axes,
            0.0,
            NEAR_FOOTPRINT,
            box_min,
            box_max,
        )
    };
    let centre = reflection_cell_centre(texel_uv, world, axes);
    assert!(centre.z > 2.02, "{centre}");
    assert_eq!(
        point(FLUID_REFLECTION_STYLE_PIXEL).position,
        Vec3::new(centre.x, centre.y, 2.0)
    );
    assert_eq!(point(FLUID_REFLECTION_STYLE_SMOOTH).position, world);
}

// Pixel is the default and the uniform's zero; any value but smooth's reads
// as pixel.
#[test]
fn pixel_is_the_default_style_and_any_unknown_value_reads_as_pixel() {
    assert_eq!(FluidReflectionStyle::default(), FluidReflectionStyle::Pixel);
    assert_eq!(FLUID_REFLECTION_STYLE_PIXEL, 0);
    for style in FluidReflectionStyle::ALL {
        let expected = match style {
            FluidReflectionStyle::Pixel => FLUID_REFLECTION_STYLE_PIXEL,
            FluidReflectionStyle::Smooth => FLUID_REFLECTION_STYLE_SMOOTH,
        };
        assert_eq!(style.uniform_value(), expected);
    }
    let time = 3.3;
    let cell = cell_where(time, |offset| offset != IVec2::ZERO);
    let pixel = cell_fragments(FLUID_REFLECTION_STYLE_PIXEL, cell, time)[1];
    for unknown in [2, 7, u32::MAX] {
        let fragment = FluidReflectionFragment {
            style: unknown,
            ..pixel
        };
        assert_eq!(sampled(&fragment), sampled(&pixel), "style {unknown}");
    }
}
