//! Adapter-backed proof of the fluid's per-vertex baked light on the install-time SH grid.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::Renderer;
use super::fluid_baked_frame_test_fixtures::{
    FrameScales, UniformLight, direct_section, dynamic_directional, f16_round, install_level,
    sh_section, todays_lighting, without, zero_dimension_sh_section,
};
use super::fluid_surface_frame_test::{
    GRAZING, ReferenceProbes, SIDE, capture_encode, capture_from, expected_pixel, f16_rgb,
    fluid_quad, look_draw, magenta_fluid, pixel_world, renderer, uniform_probe_section,
    worst_deviation,
};
use glam::{DVec3, Mat4, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_render_cpu::fluid_surface::fog::{FluidFogFragment, fog_fragment};
use postretro_render_cpu::fluid_surface::grid::choose_grid_planes;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::fluid_surface::{
    FLUID_GRID_VERTEX_CAP, FluidGridLattice, FluidGridPlanes, FluidShaderVariant, FluidSurfaceLook,
};
use postretro_render_cpu::frame_uniforms::LightTermMask;

/// The placeholder checkerboard's magenta square under [`magenta`]'s UVs.
const ALBEDO: Vec3 = Vec3::new(1.0, 0.0, 1.0);
const OPACITY: f32 = 0.5;
/// Straight on from outside every test face (outward −Z).
const FRONT: Vec3 = Vec3::new(0.0, 0.0, -1.0);
/// Under the faces, inside their volume.
const BELOW: Vec3 = Vec3::new(0.0, 3.0, 1.5);

/// The SH lattice under every test face: probes at x, y ∈ {−1.5, −0.5, 0.5,
/// 1.5} and z ∈ {−1, 0, 1, 2}. The faces at z 0.5 lie between the z 0 and
/// z 1 layers, and the half-cell grid's planes at x, y ∈ {−0.5, 0, 0.5}
/// cross them.
const ORIGIN: [f32; 3] = [-1.5, -1.5, -1.0];
const CELL: [f32; 3] = [1.0; 3];
const DIMENSIONS: [u32; 3] = [4; 3];

/// `volume` with every face's UVs inside the placeholder's first magenta
/// square, as [`magenta_fluid`] maps a full-screen face: albedo [`ALBEDO`].
fn magenta(mut volume: EnvironmentVolumeRecord) -> EnvironmentVolumeRecord {
    for vertex in volume
        .surface_faces
        .iter_mut()
        .flat_map(|face| &mut face.vertices)
    {
        vertex.uv = [vertex.position[0], vertex.position[1]].map(|clip| 3.5 + (clip + 1.0) * 0.5);
    }
    volume
}

/// The capture's encoding of a straight-alpha surface over the white clear.
fn over_white(rgb: Vec3, alpha: f32) -> [u8; 3] {
    capture_encode(rgb * alpha + Vec3::ONE * (1.0 - alpha))
}

fn draw(renderer: &mut Renderer, look: FluidSurfaceLook, camera: Vec3) -> Vec<u8> {
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    capture_from(renderer, camera)
}

fn installed_grid(renderer: &Renderer) -> Option<FluidGridPlanes> {
    renderer.full().fluid_surface.installed_grid()
}

/// The planes the packer chooses for `volumes` on the lattice at `origin`,
/// `cell_size`: what a level whose SH sampler reads that lattice must install.
fn planes_on(
    volumes: &[EnvironmentVolumeRecord],
    origin: [f32; 3],
    cell_size: [f32; 3],
) -> Option<FluidGridPlanes> {
    let lattice = FluidGridLattice::new(origin, cell_size);
    choose_grid_planes(volumes, lattice.as_ref(), FLUID_GRID_VERTEX_CAP)
}

fn pixel(pixels: &[u8], column: u32, row: u32) -> [u8; 3] {
    let at = ((row * SIDE + column) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

// Acceptance A7, uniform field. Every probe holds one irradiance, the level
// carries a uniform baked direct SH section and one dynamic directional
// light, and the full-screen face is cut by the half-cell grid on both axes.
// At the default and at changed frame scales, under the all-terms mask and
// with each of ambient floor, static and animated indirect (alone and
// together), static baked direct and dynamic direct off, every water pixel
// reads the capture's encoding of today's formula, `albedo × (ambient ·
// [ambient] + sh_indirect · indirect_scale · [indirect] + sh_direct ·
// direct_scale + dynamic · [dynamic])` blended at the surface opacity,
// within 1/255. The same frames drawn as a fogged fluid seen from outside
// match the CPU fog reference (`fog_fragment`), whose murk takes
// `fog_albedo × lighting`, within 1/255.
#[test]
fn water_on_a_uniform_baked_field_reads_todays_formula_for_every_term_and_scale() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let light = UniformLight {
        indirect: Vec3::new(0.5, 0.5, 0.25),
        direct: Vec3::new(0.125, 0.125, 0.5),
        dynamic: Vec3::new(0.25, 0.25, 0.125),
    };
    assert_eq!(f16_round(light.indirect), light.indirect);
    assert_eq!(f16_round(light.direct), light.direct);
    let sh = sh_section(ORIGIN, CELL, DIMENSIONS, |_| light.indirect);
    let direct = direct_section(&sh, light.direct);
    // Aimed along +Z: it lights the face's camera side (−Z) at full cosine.
    let sun = dynamic_directional(light.dynamic, Vec3::Z);
    install_level(&mut renderer, Some(&sh), Some(&direct), &[sun]);
    let pool = magenta_fluid();
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), None);
    let grid = installed_grid(&renderer).expect("the lattice cuts the face");
    assert_eq!(grid.spacing(), DVec3::splat(0.5));
    assert_eq!(grid.coarsening, 0);

    let changed = FrameScales {
        ambient_floor: 0.125,
        indirect_scale: 0.75,
        direct_scale: 0.5,
    };
    let all = LightTermMask::ALL;
    let masks = [
        all,
        without(all, LightTermMask::AMBIENT_FLOOR),
        without(all, LightTermMask::INDIRECT_STATIC),
        without(all, LightTermMask::INDIRECT_ANIMATED),
        without(
            without(all, LightTermMask::INDIRECT_STATIC),
            LightTermMask::INDIRECT_ANIMATED,
        ),
        without(all, LightTermMask::BAKED_DIRECT_STATIC),
        without(all, LightTermMask::DYNAMIC_DIRECT),
    ];
    // Changed scales give every term a nonzero share, so each mask above
    // names a distinct expected value: the test sees every term switch.
    let changed_values: Vec<[u8; 3]> = masks
        .iter()
        .map(|&mask| over_white(ALBEDO * todays_lighting(mask, changed, light), OPACITY))
        .collect();
    for (index, value) in changed_values.iter().enumerate().skip(1) {
        if index == 3 {
            // Animated indirect off leaves the static base: no animated
            // lights, so the frame equals the all-terms frame.
            assert_eq!(*value, changed_values[0]);
        } else {
            assert_ne!(
                *value, changed_values[0],
                "mask {index} must move the frame"
            );
        }
    }

    let water = FluidSurfaceLook::surface_only(OPACITY);
    let fogged = FluidSurfaceLook {
        fog_distance: Some(1.0),
        fog_color: [0.5, 0.75, 0.25],
        ..FluidSurfaceLook::surface_only(0.25)
    };
    let fog_row = fogged.pack();
    for scales in [FrameScales::DEFAULT, changed] {
        scales.apply(&mut renderer);
        for mask in masks {
            renderer.set_light_term_mask(mask);
            let lighting = todays_lighting(mask, scales, light);

            let pixels = draw(&mut renderer, water, FRONT);
            let want = over_white(ALBEDO * lighting, OPACITY);
            let (worst, at) = worst_deviation(&pixels, 0..SIDE, |_, _| want);
            eprintln!(
                "[FluidBakedProof] uniform field: scales {scales:?} mask {:#x} lighting \
                 {lighting} expected {want:?} centre {:?} worst {worst} at {at:?}",
                mask.bits(),
                pixel(&pixels, SIDE / 2, SIDE / 2),
            );
            assert!(
                worst <= 1,
                "mask {:#x} at {scales:?}: water off today's formula {want:?} by {worst} at \
                 {at:?}",
                mask.bits()
            );

            let pixels = draw(&mut renderer, fogged, FRONT);
            let expected = |column, row| {
                let out = fog_fragment(
                    ALBEDO * lighting,
                    fog_row.surface[0],
                    &FluidFogFragment {
                        view_proj: Mat4::IDENTITY,
                        camera: FRONT,
                        world_position: pixel_world(column, row),
                        outward: Vec3::NEG_Z,
                        // The capture clears depth to the far plane.
                        scene_depth: 1.0,
                        extinction: fog_row.surface[1],
                        fog_albedo: Vec3::from_slice(&fog_row.fog_albedo[..3]),
                        lighting,
                        weight: 1.0,
                    },
                );
                over_white(out.truncate(), out.w)
            };
            let (worst, at) = worst_deviation(&pixels, 0..SIDE, expected);
            eprintln!(
                "[FluidBakedProof] uniform field fog: scales {scales:?} mask {:#x} centre {:?} \
                 expected {:?} worst {worst} at {at:?}",
                mask.bits(),
                pixel(&pixels, SIDE / 2, SIDE / 2),
                expected(SIDE / 2, SIDE / 2),
            );
            assert!(
                worst <= 1,
                "mask {:#x} at {scales:?}: fog layer off the CPU reference by {worst} at {at:?}",
                mask.bits()
            );
        }
    }
    renderer.set_light_term_mask(LightTermMask::ALL);
    renderer.release_level_resources();
}

// Acceptance A7, viewer side (P16). The probes on the camera's side of the
// water plane (z ≤ 0) hold one irradiance, those beyond it (z ≥ 1) another,
// so the backface-rejected lookup reads only its side's layer. On both
// shader variants the water reads its viewer side's baked term from above,
// from below, and on the frame the camera crosses the plane: one frame just
// above, the next just below, and that next frame already reads the
// below-side term. Seen from above, the reflecting variant reads its
// probe's composite over the above-side surface (CPU reference, 2/255);
// every other frame reads the bare surface within 1/255.
#[test]
fn water_takes_its_viewer_sides_baked_term_above_below_and_on_the_crossing_frame() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let above = Vec3::new(0.75, 0.5, 0.125);
    let below = Vec3::new(0.125, 0.5, 0.75);
    let sh = sh_section(
        ORIGIN,
        CELL,
        DIMENSIONS,
        |[_, _, z]| {
            if z <= 1 { above } else { below }
        },
    );
    install_level(&mut renderer, Some(&sh), None, &[]);
    FrameScales {
        ambient_floor: 0.0,
        indirect_scale: 1.0,
        direct_scale: 1.0,
    }
    .apply(&mut renderer);
    renderer.set_light_term_mask(LightTermMask::ALL);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let pool = magenta_fluid();
    let green = [0, 0x3800, 0];
    let section = uniform_probe_section(8, &[0], &[green], &[(0, 0)]);
    let colours = [f16_rgb(green)];
    let reference = ReferenceProbes::new(&section, &colours, 1.0);
    // The two sides' bare surfaces read far apart, so a 1/255 match names
    // the side.
    let (above_px, below_px) = (
        over_white(ALBEDO * above, OPACITY),
        over_white(ALBEDO * below, OPACITY),
    );
    assert!(above_px[0] > below_px[0] + 20 && below_px[2] > above_px[2] + 20);

    // Far above, far below, then consecutive frames across the plane.
    let views = [
        ("above", GRAZING),
        ("below", BELOW),
        ("just above", Vec3::new(0.0, 3.0, 0.46)),
        ("crossing", Vec3::new(0.0, 3.0, 0.54)),
    ];
    for variant in [
        FluidShaderVariant::ReflectionFree,
        FluidShaderVariant::Reflecting,
    ] {
        let reflecting = variant == FluidShaderVariant::Reflecting;
        let look = FluidSurfaceLook {
            has_bound_face: reflecting,
            ..FluidSurfaceLook::surface_only(OPACITY)
        };
        renderer
            .install_fluid_surfaces(std::slice::from_ref(&pool), reflecting.then_some(&section));
        assert!(installed_grid(&renderer).is_some());
        renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
        assert_eq!(
            renderer
                .full()
                .fluid_surface
                .frame_variants(LightTermMask::ALL),
            [variant]
        );
        for (name, camera) in views {
            let from_above = camera.z < 0.5;
            let lighting = if from_above { above } else { below };
            let pixels = draw(&mut renderer, look, camera);
            let mirrored = reflecting && from_above;
            let (worst, at) = worst_deviation(&pixels, 0..SIDE, |column, row| {
                if mirrored {
                    let surface = (ALBEDO * lighting).extend(OPACITY);
                    expected_pixel(camera, column, row, surface, 0, &reference)
                } else {
                    over_white(ALBEDO * lighting, OPACITY)
                }
            });
            eprintln!(
                "[FluidBakedProof] viewer side: {variant:?} {name} at {camera}: centre {:?}, \
                 above-side {above_px:?}, below-side {below_px:?}, worst {worst} at {at:?}",
                pixel(&pixels, SIDE / 2, SIDE / 2),
            );
            let tolerance = if mirrored { 2 } else { 1 };
            assert!(
                worst <= tolerance,
                "{variant:?} {name}: water off its viewer side's term by {worst} at {at:?}"
            );
        }
    }
    renderer.release_level_resources();
}

// Acceptance A7, variant parity. One volume of two faces over a baked field
// that varies across the faces, both cut by the grid: the left face binds a
// reflection probe, the right binds none. Drawn reflecting (its own pipeline,
// probe bound, reflectivity 1) and drawn reflection-free (no section 54),
// the frames match byte for byte wherever the reflecting gate is shut: over
// the unbound face seen from above, and everywhere seen from below. Not
// vacuous: the bound face differs from above, and the baked term is nonzero
// and varies across the frame.
#[test]
fn reflecting_and_reflection_free_variants_match_wherever_the_reflecting_gate_is_shut() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let sh = sh_section(ORIGIN, CELL, DIMENSIONS, |[x, y, z]| {
        Vec3::new(
            0.125 * (x + 1) as f32,
            0.0625 * (z + 1) as f32,
            0.125 * (y + 1) as f32,
        )
    });
    let direct = direct_section(&sh, Vec3::splat(0.125));
    install_level(&mut renderer, Some(&sh), Some(&direct), &[]);
    FrameScales {
        ambient_floor: 0.0625,
        indirect_scale: 1.0,
        direct_scale: 1.0,
    }
    .apply(&mut renderer);
    renderer.set_light_term_mask(LightTermMask::ALL);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let mut pool = magenta(fluid_quad(-1.0, 0.0));
    pool.surface_faces
        .extend(magenta(fluid_quad(0.0, 1.0)).surface_faces);
    let green = [0, 0x3800, 0];
    let section = uniform_probe_section(8, &[0], &[green], &[(0, 0)]);
    let free = FluidSurfaceLook::surface_only(OPACITY);
    let mirrored = FluidSurfaceLook {
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(OPACITY)
    };

    for (name, camera) in [("above", GRAZING), ("below", BELOW)] {
        renderer.install_fluid_surfaces(std::slice::from_ref(&pool), None);
        assert!(installed_grid(&renderer).is_some());
        renderer.set_fluid_surface_draws(&[look_draw(0, free)]);
        assert_eq!(
            renderer
                .full()
                .fluid_surface
                .frame_variants(LightTermMask::ALL),
            [FluidShaderVariant::ReflectionFree]
        );
        let today = draw(&mut renderer, free, camera);

        renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(&section));
        renderer.set_fluid_surface_draws(&[look_draw(0, mirrored)]);
        assert_eq!(
            renderer
                .full()
                .fluid_surface
                .frame_variants(LightTermMask::ALL),
            [FluidShaderVariant::Reflecting]
        );
        let reflecting = draw(&mut renderer, mirrored, camera);

        let today_at = |column, row| pixel(&today, column, row);
        let (unbound, unbound_at) = worst_deviation(&reflecting, SIDE / 2..SIDE, today_at);
        let (bound, _) = worst_deviation(&reflecting, 0..SIDE / 2, today_at);
        let reds: Vec<u8> = (0..SIDE)
            .map(|column| pixel(&today, column, 2)[0])
            .collect();
        let (low, high) = (reds.iter().min().unwrap(), reds.iter().max().unwrap());
        eprintln!(
            "[FluidBakedProof] variant parity {name}: unbound face worst {unbound} at \
             {unbound_at:?}, bound face worst {bound}, reflection-free red across row 2 \
             {low}..={high}"
        );
        // The unlit surface reads 188: a nonzero baked term lifts it, and the
        // field's x gradient spreads it across the frame.
        assert!(*low > 190 && high - low > 10, "baked term: {reds:?}");
        assert_eq!(unbound, 0, "{name}: the unbound face matches byte for byte");
        if camera.z < 0.5 {
            assert!(bound > 10, "the bound face reflects from above: {bound}");
        } else {
            assert!(
                reflecting == today,
                "from below the gate is shut everywhere: frames match byte for byte"
            );
        }
    }
    renderer.release_level_resources();
}

// Acceptance A4 and A6 (P3, P4, P19, P20). Fluid install follows level
// geometry install, so the grid takes the lattice the SH sampler now reads.
// Level 1's legacy section (a skewed origin, unequal cells) packs on that
// lattice: half-cell spacing, planes through its origin, never the
// placeholder's. Its water reads its own uniform field. Level 2 has no SH
// volume, and level 2b's section has a zero-dimension grid the renderer
// refuses: both pack with no grid and draw water with the ambient floor
// alone. Level 3 brings another lattice and packs on it, reading its own
// field.
#[test]
fn the_fluid_grid_takes_each_levels_sh_lattice_and_a_level_without_one_draws_ambient_only() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let pool = magenta_fluid();
    let volumes = std::slice::from_ref(&pool);
    let placeholder = planes_on(volumes, [0.0; 3], [1.0; 3]);
    let water = FluidSurfaceLook::surface_only(OPACITY);
    renderer.set_light_term_mask(LightTermMask::ALL);
    let lit_by_sh = FrameScales {
        ambient_floor: 0.0,
        indirect_scale: 1.0,
        direct_scale: 1.0,
    };
    let check_water = |renderer: &mut Renderer, lighting: Vec3, level: &str| {
        let pixels = draw(renderer, water, FRONT);
        let want = over_white(ALBEDO * lighting, OPACITY);
        let (worst, at) = worst_deviation(&pixels, 0..SIDE, |_, _| want);
        eprintln!(
            "[FluidBakedProof] level change {level}: grid {:?}, expected {want:?}, worst {worst} \
             at {at:?}",
            installed_grid(renderer)
        );
        assert!(
            worst <= 1,
            "{level}: water off {want:?} by {worst} at {at:?}"
        );
    };
    let install = |renderer: &mut Renderer, sh: Option<_>| {
        renderer.release_level_resources();
        install_level(renderer, sh, None, &[]);
        renderer.install_fluid_surfaces(volumes, None);
    };

    // Level 1: probes at x −1.2..1.05, y −1.6..1.4, z −1.1..2.65.
    let (origin_1, cell_1) = ([-1.2, -1.6, -1.1], [0.75, 1.0, 1.25]);
    let colour_1 = Vec3::new(0.5, 0.25, 0.25);
    let level_1 = sh_section(origin_1, cell_1, DIMENSIONS, |_| colour_1);
    lit_by_sh.apply(&mut renderer);
    install(&mut renderer, Some(&level_1));
    let grid_1 = installed_grid(&renderer).expect("level 1 packs on its lattice");
    assert_eq!(grid_1.spacing(), Vec3::from(cell_1).as_dvec3() / 2.0);
    assert_eq!(Some(grid_1), planes_on(volumes, origin_1, cell_1));
    assert_ne!(Some(grid_1), placeholder);
    // The same spacing through another origin is another grid.
    let shifted = [origin_1[0] + 0.1, origin_1[1], origin_1[2]];
    assert_ne!(Some(grid_1), planes_on(volumes, shifted, cell_1));
    check_water(&mut renderer, colour_1, "level 1");

    // Levels 2 and 2b: no usable SH volume. The ambient floor alone lights
    // the water.
    let floor = FrameScales {
        ambient_floor: 0.5,
        ..lit_by_sh
    };
    floor.apply(&mut renderer);
    install(&mut renderer, None);
    assert_eq!(installed_grid(&renderer), None, "no SH volume, no grid");
    check_water(&mut renderer, Vec3::splat(0.5), "level 2 (no SH)");
    let zero = zero_dimension_sh_section();
    install(&mut renderer, Some(&zero));
    assert_eq!(installed_grid(&renderer), None, "a refused volume, no grid");
    check_water(
        &mut renderer,
        Vec3::splat(0.5),
        "level 2b (zero-dimension SH)",
    );

    // Degenerate lattices (zero cell size on one axis, NaN origin) are refused
    // renderer-wide: no SH volume, no grid, ambient floor alone.
    let bad_cell = sh_section([0.0; 3], [0.75, 0.0, 1.25], DIMENSIONS, |_| colour_1);
    let bad_origin = sh_section([f32::NAN, 0.0, 0.0], [1.0; 3], DIMENSIONS, |_| colour_1);
    for (section, name) in [(&bad_cell, "zero cell size"), (&bad_origin, "NaN origin")] {
        install(&mut renderer, Some(section));
        assert_eq!(installed_grid(&renderer), None, "{name}: no grid");
        assert!(
            !renderer.full().sh_volume_resources.present,
            "{name}: SH volume refused"
        );
        check_water(&mut renderer, Vec3::splat(0.5), name);
    }

    // Level 3: probes at x −1..2, y −1.25..1.15, z −0.75..3.75.
    let (origin_3, cell_3) = ([-1.0, -1.25, -0.75], [1.0, 0.8, 1.5]);
    let colour_3 = Vec3::new(0.25, 0.5, 0.5);
    let level_3 = sh_section(origin_3, cell_3, DIMENSIONS, |_| colour_3);
    lit_by_sh.apply(&mut renderer);
    install(&mut renderer, Some(&level_3));
    let grid_3 = installed_grid(&renderer).expect("level 3 packs on its lattice");
    assert_eq!(grid_3.spacing(), Vec3::from(cell_3).as_dvec3() / 2.0);
    assert_eq!(Some(grid_3), planes_on(volumes, origin_3, cell_3));
    assert_ne!(grid_3, grid_1);
    check_water(&mut renderer, colour_3, "level 3");

    renderer.release_level_resources();
    assert_eq!(installed_grid(&renderer), None);
}
