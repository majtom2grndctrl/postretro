//! Adapter-backed proof of acceptance row 38 in the form the plan's
//! correction gives it: water's screen-space layer reflects an on-screen
//! glossy wall's composited result, at a tier that runs both screen-space
//! layers, and water's screen-space term is the same at every water
//! capture-shadow weight.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors; Screen-space layer)

use glam::{UVec2, Vec3, Vec4};
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::reflection_probes::{
    ReflectionProbeFluidBinding, ReflectionProbeRecord, ReflectionProbeWorldBinding,
    ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{
    FRAME, PROBE_TILE, TargetReadback, View, centre, mirror, on_plane, pixels,
    read_reflection_target, view_from,
};
use super::fluid_surface_frame_test::look_draw;
use super::glossy_floor_frame_test_fixtures::read_floor_layer;
use super::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, GlossyFrame, PROBE_COLOUR, floor_faces, floor_probe, half_step,
    renderer, set_floor_material, with_faces, with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{ALL, frame_seeing, pool};
use super::glossy_overlay_room_frame_test_fixtures::ray_through;

/// The pool over the floor's matte face, and its height.
const POOL_X: [f32; 2] = [-3.0, 3.0];
const POOL_Z: [f32; 2] = [10.0, 15.0];
const POOL_Y: f32 = 0.3;
/// The glossy wall behind the pool: upright at the floor's far edge, facing
/// the camera.
const WALL_Z: f32 = 16.0;
/// Pixel style's water cells a metre, as the water proofs' placeholder
/// texture gives them.
const WATER_TEXELS_PER_METRE: f32 = 64.0;
/// The water's probe capture, apart from the wall's (exact in f16).
const WATER_PROBE_COLOUR: Vec3 = Vec3::new(0.5, 0.25, 0.125);

/// Tint 1 at full `_s` and the exponent cap: W = 1 at every angle, so the
/// wall's composited colour is its probe's capture alone, the same at every
/// pixel, and its overlay delta is that capture less forward's colour.
const MIRROR_WALL: FloorMaterial = FloorMaterial {
    albedo: [255; 4],
    slot: [255, 0, 255, 254],
    glow: None,
};

/// Low over the pool's near edge, looking at the wall over the water: water
/// fills the lower screen and its mirrored rays reach the wall's interior.
fn view() -> View {
    view_from(Vec3::new(0.0, 2.0, 6.0), Vec3::new(0.0, 0.8, WALL_Z))
}

/// Id 54: probe 0 (the floor's, [`PROBE_COLOUR`]) bound to the glossy floor
/// face 0 and the wall, face 2; probe 1 ([`WATER_PROBE_COLOUR`]) over the
/// pool, bound to its face. With `water_shadowed`, every probe-1 distance
/// texel holds inverse L1 distance 2: a surface half a metre from its
/// capture point all round, so the water's probe term is withheld (s = 0)
/// over nearly all the pool; probe 0 is the same either way.
fn probes(water_shadowed: bool) -> ReflectionProbesSection {
    let mut probes = floor_probe(true);
    probes
        .world_bindings
        .push(ReflectionProbeWorldBinding { face: 2, probe: 0 });
    probes.fluid_bindings = vec![ReflectionProbeFluidBinding {
        volume: 0,
        face: 0,
        probe: 1,
    }];
    probes.probes.push(ReflectionProbeRecord {
        source_entity: 1,
        capture_point: [0.0, POOL_Y + 0.002, 12.5],
        layer: 1,
        ..probes.probes[0]
    });
    let texel: Vec<u8> = WATER_PROBE_COLOUR
        .to_array()
        .map(f32_to_f16_bits)
        .into_iter()
        .chain([0x3C00])
        .flat_map(u16::to_le_bytes)
        .collect();
    probes
        .tiles
        .extend(texel.repeat((PROBE_TILE * PROBE_TILE) as usize));
    let per_probe = reflection_probe_zero_distances(1, PROBE_TILE);
    let water = if water_shadowed {
        [0x00, 0x40].repeat(per_probe.len() / 2)
    } else {
        per_probe.clone()
    };
    probes.distances = [per_probe, water].concat();
    probes
}

/// The floor, the wall (face 2, so the matte floor face is face 3) and the
/// pool, under `probes`; the glossy faces in [`MIRROR_WALL`], every face lit
/// by the ambient floor alone, and the pool drawn as a mirror (F = 1,
/// opacity 0: each water pixel is its reflection alone).
fn install(renderer: &mut Renderer, probes: &ReflectionProbesSection) {
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
    with_faces(&faces, Some(probes), None, |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    // The water's texture is the glossy bucket's one texel: its UVs run
    // [`WATER_TEXELS_PER_METRE`], as fine as the cells of the 64-texel
    // placeholder the water proofs run on.
    let mut water = pool(POOL_X, POOL_Z, POOL_Y);
    for vertex in &mut water.surface_faces[0].vertices {
        vertex.uv = vertex.uv.map(|uv| uv * WATER_TEXELS_PER_METRE);
    }
    renderer.install_fluid_surfaces(&[water], Some(probes));
    set_floor_material(renderer, MIRROR_WALL);
    renderer.set_ambient_floor(1.0);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
}

/// One frame, water's reflection target as it left it, and whether the
/// floor layer ran.
fn frame(renderer: &mut Renderer) -> (GlossyFrame, TargetReadback) {
    let frame = frame_seeing(renderer, view(), &ALL);
    assert!(
        read_floor_layer(renderer).is_some(),
        "High runs the floor layer on a level with a bound glossy floor"
    );
    (frame, read_reflection_target(renderer))
}

/// Whether `pixel` shows the wall's interior, clear of its edges and of the
/// water and floor in front of it.
fn on_wall(pixel: UVec2) -> bool {
    let view = view();
    let floor_hit = on_plane(view, centre(pixel), POOL_Y).is_some_and(|p| p.z < WALL_Z + 0.5);
    let (origin, direction) = ray_through(view, centre(pixel));
    let t = (WALL_Z - origin.z) / direction.z;
    let point = origin + direction * t;
    !floor_hit && point.x.abs() < 2.8 && point.y > 0.2 && point.y < 2.8
}

/// Every wall-interior pixel's colour in `frame`, which W = 1 keeps one
/// value (each pixel within a half-float step of the first).
fn wall_colour(frame: &GlossyFrame, what: &str) -> Vec3 {
    let wall: Vec<Vec3> = pixels()
        .filter(|&pixel| on_wall(pixel))
        .map(|pixel| frame.colour(pixel))
        .collect();
    assert!(
        wall.len() > 100,
        "{what}: the wall shows over {}",
        wall.len()
    );
    for colour in &wall {
        for c in 0..3 {
            assert!(
                (colour[c] - wall[0][c]).abs() <= half_step(wall[0][c]),
                "{what}: the wall is one colour: {colour} against {}",
                wall[0]
            );
        }
    }
    wall[0]
}

/// The water target's reflected colour X at a texel of full confidence.
fn reflected(texel: Vec4) -> Vec3 {
    texel.truncate() / texel.w
}

fn near(a: Vec3, b: Vec3, slack: f32) -> bool {
    (a - b).abs().max_element() <= slack
}

// Acceptance row 38 (plan Correction "Row 38 is unreachable as written for
// floors"): at High, which runs water's screen-space layer and the floor
// layer, water reflects a glossy wall standing behind the pool. The wall is
// a tint-1 mirror (W = 1), so the overlay turns it from forward's colour
// into its probe's capture. Water's march reads scene depth alone, so its
// confidence is the same bits with the overlay and with a test-only overlay
// that adds nothing; where it hits the wall at full confidence, the colour
// it holds is the wall's composited colour, and differs from the no-overlay
// frame's by the wall's overlay delta, within one half-float step of each
// colour: the target stores c·X in f16 (one rounding, exact at c = 1), and
// a hit inside the wall reads texels of one f16 colour, so its filter adds
// none. In smooth style each water pixel whose whole bilinear footprint is
// such a hit shows the same delta (F = 1 at zero opacity), within twice that:
// the fluid pass stores its own f16 sum over the read. Then, with the
// water's own probe withholding its term over the pool (s = 0), water's
// target is bit for bit the same in both styles, while the frame's water
// differs: the screen-space term never gains water's s, though it shows a
// glossy face's reflection.
#[test]
fn water_reflects_a_glossy_walls_composited_result_and_its_ssr_term_reads_no_s() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    for style in [FluidReflectionStyle::Pixel, FluidReflectionStyle::Smooth] {
        install(&mut renderer, &probes(false));
        renderer.set_fluid_reflection_style(style);
        let (production, target) = frame(&mut renderer);
        let (bare, bare_target) = with_variant(&mut renderer, "fs_test_zero", frame);
        let case = format!("{style:?}");
        let wall = wall_colour(&production, &case);
        let bare_wall = wall_colour(&bare, &case);
        assert!(
            near(wall, PROBE_COLOUR, half_step(1.0)),
            "{case}: the wall composites to its capture: {wall}"
        );
        assert!(
            !near(wall, bare_wall, 0.1),
            "{case}: the overlay moves the wall: {wall} against {bare_wall}"
        );
        assert_eq!(target.extent, bare_target.extent);
        let slack = |a: Vec3, b: Vec3| {
            (0..3)
                .map(|c| half_step(a[c]) + half_step(b[c]))
                .fold(0.0, f32::max)
        };
        let mut hits = 0;
        for (index, (&with, &without)) in target.texels.iter().zip(&bare_target.texels).enumerate()
        {
            assert_eq!(
                with.w, without.w,
                "{case}: texel {index}'s confidence reads no overlay"
            );
            if with.w < 1.0 || !near(reflected(without), bare_wall, slack(bare_wall, bare_wall)) {
                continue;
            }
            let delta = reflected(with) - reflected(without);
            assert!(
                near(delta, wall - bare_wall, slack(wall, bare_wall)),
                "{case}: texel {index} reflects the wall's delta: {delta} against {}",
                wall - bare_wall
            );
            hits += 1;
        }
        assert!(
            hits > 40,
            "{case}: water reflects the wall at {hits} texels"
        );
        if style == FluidReflectionStyle::Smooth {
            let scale = target.extent.as_vec2() / FRAME as f32;
            let mut water = 0;
            for pixel in pixels() {
                let on_water = on_plane(view(), centre(pixel), POOL_Y).is_some_and(|p| {
                    p.x > POOL_X[0] && p.x < POOL_X[1] && p.z > POOL_Z[0] && p.z < POOL_Z[1]
                });
                let base = (centre(pixel) * scale - 0.5).floor().as_ivec2();
                let footprint = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .map(|(x, y)| bare_target.at(base + glam::IVec2::new(x, y)));
                let all_wall = footprint.iter().all(|&texel| {
                    texel.w == 1.0 && near(reflected(texel), bare_wall, slack(bare_wall, bare_wall))
                });
                if !on_water || !all_wall {
                    continue;
                }
                let delta = production.colour(pixel) - bare.colour(pixel);
                assert!(
                    near(delta, wall - bare_wall, 2.0 * slack(wall, bare_wall)),
                    "{pixel}: the water shows the wall's delta: {delta} against {}",
                    wall - bare_wall
                );
                water += 1;
            }
            assert!(water > 20, "the water shows the wall over {water} pixels");
        }

        renderer.release_level_resources();
        install(&mut renderer, &probes(true));
        renderer.set_fluid_reflection_style(style);
        let (shadowed, shadowed_target) = frame(&mut renderer);
        assert_eq!(
            shadowed_target.texels, target.texels,
            "{case}: water's screen-space term is the same at every capture-shadow weight"
        );
        let withheld = pixels()
            .filter(|&pixel| shadowed.colour(pixel) != production.colour(pixel))
            .count();
        assert!(
            withheld > 20,
            "{case}: the water's probe term is withheld over {withheld} pixels"
        );
        renderer.release_level_resources();
    }
}
