use super::*;
use glam::Vec3;
use postretro_render_data::material::{Material, SurfaceDepth};

// -- Fixtures --

/// Authored `_h.png` values.
const BLACK: u8 = 0;
const PLANE: u8 = 128;
const WHITE: u8 = 255;

/// What `textureLoad(..).g` returns for each authored value: the bake stores
/// `G = 255 − h`.
fn stored(authored: &[u8]) -> Vec<f32> {
    authored
        .iter()
        .map(|&h| f32::from(255 - h) / 255.0)
        .collect()
}

fn field(width: i32, height: i32, stored_g: &[f32], levels: u32) -> SurfaceDepthField<'_> {
    SurfaceDepthField {
        width,
        height,
        stored_g,
        quantize_levels: levels,
    }
}

/// The quantized band the uniform would carry for these authored values: the
/// load-time extraction over one level, then the material's quantization.
fn band(authored: &[u8], levels: u32) -> SurfaceRelief {
    let rg: Vec<u8> = authored.iter().flat_map(|&h| [0u8, 255 - h]).collect();
    surface_relief_from_rg8_levels(&[(authored.len() as u32, 1, &rg)]).quantized(levels as f32)
}

/// Deterministic authored bytes, no libm involved.
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state & 0xFF) as u8
        })
        .collect()
}

/// Unit azimuths from integer vectors (sqrt is correctly rounded everywhere).
fn azimuths() -> Vec<[f32; 2]> {
    [
        (1, 0),
        (3, 1),
        (1, 1),
        (1, 3),
        (0, 1),
        (-1, 2),
        (-1, 1),
        (-2, 1),
        (-1, 0),
        (-3, -1),
        (-1, -1),
        (-1, -3),
        (0, -1),
        (1, -2),
        (1, -1),
        (2, -1),
    ]
    .iter()
    .map(|&(x, y): &(i32, i32)| {
        let len = ((x * x + y * y) as f32).sqrt();
        [x as f32 / len, y as f32 / len]
    })
    .collect()
}

// -- Packing --

fn march_fields(max_steps: u32, base_mip: u32, has_depth: bool, budget: u32) -> SurfaceDepthMarch {
    SurfaceDepthMarch {
        max_steps,
        base_mip,
        has_depth,
        shadow_light_budget: budget,
    }
}

#[test]
fn march_word_round_trips_every_packed_field() {
    for fields in [
        march_fields(24, 0, true, 2),
        march_fields(1, 7, true, 0),
        march_fields(255, 15, false, 15),
    ] {
        assert_eq!(
            unpack_surface_depth_march(pack_surface_depth_march(fields)),
            fields
        );
    }
}

#[test]
fn march_word_clamps_rather_than_wrapping() {
    let fields =
        unpack_surface_depth_march(pack_surface_depth_march(march_fields(9999, 99, true, 9999)));
    assert_eq!(fields.max_steps, SURFACE_DEPTH_MAX_STEPS);
    assert_eq!(fields.base_mip, SURFACE_DEPTH_BASE_MIP_MASK);
    assert_eq!(fields.shadow_light_budget, SURFACE_DEPTH_SHADOW_BUDGET_MASK);
    assert!(fields.has_depth);
    // A clamped field must never bleed into its neighbours.
    assert_eq!(
        pack_surface_depth_march(march_fields(9999, 99, true, 9999)),
        SURFACE_DEPTH_MAX_STEPS
            | (SURFACE_DEPTH_BASE_MIP_MASK << SURFACE_DEPTH_BASE_MIP_SHIFT)
            | SURFACE_DEPTH_HAS_DEPTH_BIT
            | (SURFACE_DEPTH_SHADOW_BUDGET_MASK << SURFACE_DEPTH_SHADOW_BUDGET_SHIFT),
    );
}

#[test]
fn every_material_step_cap_fits_the_packed_byte() {
    for material in [
        Material::Concrete,
        Material::Metal,
        Material::Grate,
        Material::Wood,
        Material::Glass,
        Material::Neon,
        Material::Default,
    ] {
        assert!(
            material.surface_depth().max_steps <= SURFACE_DEPTH_MAX_STEPS,
            "{material:?}: max_steps would be clamped by the packed word",
        );
    }
}

// -- Uniform resolve --

fn resolve_on(carving: SurfaceDepth, mip_count: u32, requested: u32) -> SurfaceDepthUniform {
    SurfaceDepthUniform::resolve(
        carving,
        SurfaceDepthQuality::On,
        true,
        mip_count,
        requested,
        SurfaceRelief::FULL_RANGE,
    )
}

#[test]
fn a_material_without_a_surface_map_resolves_flat() {
    let carving = Material::Concrete.surface_depth();
    assert!(carving.is_enabled());
    let resolved = SurfaceDepthUniform::resolve(
        carving,
        SurfaceDepthQuality::On,
        false,
        11,
        SURFACE_DEPTH_RESIDENT_BASE_MIP,
        SurfaceRelief::FULL_RANGE,
    );
    assert_eq!(resolved, SurfaceDepthUniform::FLAT);
    assert!(!resolved.has_depth);
    assert_eq!(resolved.march_word() & SURFACE_DEPTH_HAS_DEPTH_BIT, 0);
}

/// The has-depth bit is the ONLY guard against the R8 placeholder, because
/// the placeholder's `g = 0` now means maximum raise, not flat.
#[test]
fn the_march_never_runs_without_the_has_depth_bit() {
    // What the placeholder would do if marched: lift the face by its full depth.
    let placeholder = [0.0f32; 4];
    let f = field(2, 2, &placeholder, 0);
    assert_eq!(f.texel_height(0, 0), SURFACE_HEIGHT_MAX_RAISE);

    for quality in SurfaceDepthQuality::ALL {
        for relief in [
            SurfaceRelief::FULL_RANGE,
            SurfaceRelief {
                peak_raise: SURFACE_HEIGHT_MAX_RAISE,
                trough: 0.0,
            },
            SurfaceRelief::FLAT,
        ] {
            let resolved = SurfaceDepthUniform::resolve(
                Material::Concrete.surface_depth(),
                quality,
                false,
                1,
                SURFACE_DEPTH_RESIDENT_BASE_MIP,
                relief,
            );
            assert!(
                !surface_depth_has_map(resolved.march_word(), resolved.depth.depth_meters),
                "{quality:?}/{relief:?}: a non-surface-map slot must never pass the march gate",
            );
            // Belt and braces: even a shader that ignored the bit would find
            // an empty band and march nothing.
            assert!(resolved.relief.is_flat());
        }
    }
    // The gate reads the bit, not the depth: a positive depth alone is not enough.
    assert!(!surface_depth_has_map(0, 6.0));
    assert!(surface_depth_has_map(SURFACE_DEPTH_HAS_DEPTH_BIT, 6.0));
    assert!(!surface_depth_has_map(SURFACE_DEPTH_HAS_DEPTH_BIT, 0.0));
}

#[test]
fn a_flat_prefix_with_a_surface_map_still_resolves_flat() {
    let flat = Material::Glass.surface_depth();
    assert_eq!(
        resolve_on(flat, 11, SURFACE_DEPTH_RESIDENT_BASE_MIP),
        SurfaceDepthUniform::FLAT
    );
}

#[test]
fn an_all_mid_gray_map_uploads_exactly_what_no_map_does() {
    // P4: the band is empty, so the material marches nothing.
    for levels in 0..=16u32 {
        let depth = SurfaceDepth {
            quantize_levels: levels,
            ..Material::Concrete.surface_depth()
        };
        let resolved = SurfaceDepthUniform::resolve(
            depth,
            SurfaceDepthQuality::On,
            true,
            11,
            SURFACE_DEPTH_RESIDENT_BASE_MIP,
            band(&[PLANE; 16], levels),
        );
        assert_eq!(resolved, SurfaceDepthUniform::FLAT, "levels {levels}");
    }
}

#[test]
fn the_relief_band_is_quantized_with_the_material_levels() {
    let depth = Material::Concrete.surface_depth();
    let resolved = SurfaceDepthUniform::resolve(
        depth,
        SurfaceDepthQuality::On,
        true,
        11,
        SURFACE_DEPTH_RESIDENT_BASE_MIP,
        SurfaceRelief {
            peak_raise: 0.3,
            trough: -0.25,
        },
    );
    let levels = depth.quantize_levels as f32;
    assert_eq!(
        resolved.relief.peak_raise,
        surface_height_quantize(0.3, levels)
    );
    assert_eq!(
        resolved.relief.trough,
        surface_height_quantize(-0.25, levels)
    );
}

#[test]
fn a_nonsense_relief_folds_into_the_legal_band() {
    let folded = SurfaceRelief {
        peak_raise: f32::NAN,
        trough: 7.0,
    }
    .quantized(0.0);
    assert_eq!(folded, SurfaceRelief::FLAT);
    let wide = SurfaceRelief {
        peak_raise: 9.0,
        trough: -9.0,
    }
    .quantized(0.0);
    assert_eq!(
        wide,
        SurfaceRelief {
            peak_raise: 1.0,
            trough: -1.0
        }
    );
}

#[test]
fn base_mip_never_exceeds_the_uploaded_chain() {
    let carving = Material::Concrete.surface_depth();
    // A single-level chain (the 1x1 placeholder shape) must still be a legal
    // textureLoad level, whatever residency asks for.
    assert_eq!(resolve_on(carving, 1, 0).base_mip, 0);
    assert_eq!(resolve_on(carving, 0, 0).base_mip, 0);
    assert_eq!(resolve_on(carving, 1, 9).base_mip, 0);
    // A streamed-down chain keeps the level streaming asked for.
    assert_eq!(resolve_on(carving, 11, 3).base_mip, 3);
    assert_eq!(resolve_on(carving, 4, 9).base_mip, 3);
}

#[test]
fn a_requested_base_mip_survives_the_packed_word() {
    let carving = Material::Concrete.surface_depth();
    let fields = unpack_surface_depth_march(resolve_on(carving, 11, 3).march_word());
    assert_eq!(fields.base_mip, 3);
    assert!(fields.has_depth);
}

// -- Player on/off switch (D5) --

#[test]
fn the_switch_defaults_to_on_because_the_feature_ships_enabled() {
    assert_eq!(SurfaceDepthQuality::default(), SurfaceDepthQuality::On);
}

#[test]
fn the_switch_has_exactly_two_states() {
    // D5 is a cost lever, not a quality ladder: a third state would have to
    // earn its keep visually, and the one that existed never changed the
    // carve depth at all.
    assert_eq!(
        SurfaceDepthQuality::ALL,
        [SurfaceDepthQuality::Off, SurfaceDepthQuality::On]
    );
}

const CARVING: [Material; 5] = [
    Material::Concrete,
    Material::Metal,
    Material::Grate,
    Material::Wood,
    Material::Default,
];

#[test]
fn off_forces_every_carving_material_flat() {
    for material in CARVING {
        let carving = material.surface_depth();
        assert!(carving.is_enabled(), "{material:?} must carve at On");
        assert_eq!(
            SurfaceDepthQuality::Off.apply(carving),
            SurfaceDepth::FLAT,
            "{material:?} must be forced flat at Off"
        );
        let resolved = SurfaceDepthUniform::resolve(
            carving,
            SurfaceDepthQuality::Off,
            true,
            11,
            SURFACE_DEPTH_RESIDENT_BASE_MIP,
            SurfaceRelief::FULL_RANGE,
        );
        assert_eq!(resolved, SurfaceDepthUniform::FLAT);
        assert_eq!(
            resolved.march_word(),
            0,
            "{material:?}: Off must pack the historical all-zero march word"
        );
    }
}

#[test]
fn on_is_exactly_the_material_prefix_values() {
    for material in CARVING {
        let carving = material.surface_depth();
        assert_eq!(
            SurfaceDepthQuality::On.apply(carving),
            carving,
            "{material:?}: On must pass the per-prefix tuning through unmodified"
        );
        let resolved = resolve_on(carving, 11, SURFACE_DEPTH_RESIDENT_BASE_MIP);
        assert_eq!(resolved.depth, carving);
        assert_eq!(
            resolved.shadow_light_budget, SURFACE_DEPTH_SHADOW_LIGHT_BUDGET,
            "{material:?}: On gets the full self-shadow budget"
        );
        assert!(resolved.has_depth);
    }
}

#[test]
fn only_on_budgets_a_self_shadow_march() {
    assert_eq!(SurfaceDepthQuality::Off.shadow_light_budget(), 0);
    assert_eq!(
        SurfaceDepthQuality::On.shadow_light_budget(),
        SURFACE_DEPTH_SHADOW_LIGHT_BUDGET
    );
    // The budget must survive its packed field without clamping.
    const { assert!(SURFACE_DEPTH_SHADOW_LIGHT_BUDGET <= SURFACE_DEPTH_SHADOW_BUDGET_MASK) };
}

#[test]
fn a_flat_material_is_switch_independent() {
    // Glass and Neon are flat by intent; neither state may make them march,
    // and both must produce the identical all-zero row.
    let flat = Material::Glass.surface_depth();
    for quality in SurfaceDepthQuality::ALL {
        assert_eq!(quality.apply(flat), SurfaceDepth::FLAT);
        assert_eq!(
            SurfaceDepthUniform::resolve(
                flat,
                quality,
                true,
                11,
                SURFACE_DEPTH_RESIDENT_BASE_MIP,
                SurfaceRelief::FULL_RANGE,
            ),
            SurfaceDepthUniform::FLAT
        );
    }
}

/// A non-finite authored depth must never reach the GPU.
#[test]
fn a_non_finite_authored_depth_resolves_flat() {
    for bad in [f32::INFINITY, f32::NAN] {
        let depth = SurfaceDepth {
            depth_meters: bad,
            quantize_levels: 8,
            max_steps: 16,
            fade_distance_meters: 10.0,
        };
        let resolved = SurfaceDepthUniform::resolve(
            depth,
            SurfaceDepthQuality::On,
            true,
            4,
            0,
            SurfaceRelief::FULL_RANGE,
        );
        assert_eq!(
            resolved,
            SurfaceDepthUniform::FLAT,
            "an authored depth of {bad} must resolve flat, not reach the shader",
        );
    }
}

/// The ceiling is a real constraint on the value that reaches the GPU, not
/// an assertion about one hardcoded table.
#[test]
fn an_over_deep_authored_depth_is_clamped_to_the_ceiling() {
    let ceiling = surface_depth_max_authored();
    let depth = SurfaceDepth {
        depth_meters: ceiling * 10.0,
        quantize_levels: 8,
        max_steps: 16,
        fade_distance_meters: 10.0,
    };
    let resolved = SurfaceDepthUniform::resolve(
        depth,
        SurfaceDepthQuality::On,
        true,
        4,
        0,
        SurfaceRelief::FULL_RANGE,
    );
    assert!(resolved.depth.depth_meters <= ceiling);
    // Still marching — the clamp bounds the depth, it does not disable it.
    assert!(resolved.has_depth);
}

// -- Encoding and quantization --

#[test]
fn mid_gray_is_exactly_the_plane_at_every_level_count() {
    let g = stored(&[PLANE])[0];
    assert_eq!(surface_height_fraction(g).to_bits(), 0.0f32.to_bits());
    assert_eq!(surface_height_fraction_from_byte(255 - PLANE), 0.0);
    for levels in 0..=255u32 {
        assert_eq!(
            surface_height_quantize(0.0, levels as f32).to_bits(),
            0.0f32.to_bits(),
            "mid-gray must quantize to +0.0 at {levels} levels",
        );
    }
}

#[test]
fn the_signed_fraction_is_linear_in_the_authored_byte() {
    assert_eq!(surface_height_fraction(stored(&[BLACK])[0]), -1.0);
    assert_eq!(
        surface_height_fraction(stored(&[WHITE])[0]),
        SURFACE_HEIGHT_MAX_RAISE
    );
    for h in 0..=255u8 {
        let expected = (f32::from(h) - 128.0) / 128.0;
        assert_eq!(surface_height_fraction(stored(&[h])[0]), expected, "h {h}");
        assert_eq!(
            surface_height_fraction_from_byte(255 - h),
            expected,
            "h {h}"
        );
    }
}

/// The byte recovery must tolerate a unorm decode that is a hair off `G/255`.
#[test]
fn the_stored_byte_is_recovered_from_a_slightly_inexact_unorm() {
    for byte in 0..=255u8 {
        let exact = f32::from(byte) / 255.0;
        for jitter in [-1.0e-4f32, 0.0, 1.0e-4] {
            assert_eq!(
                surface_height_fraction(exact + jitter),
                surface_height_fraction_from_byte(byte),
                "byte {byte} jitter {jitter}",
            );
        }
    }
}

/// `floor(x + 0.5)` and Rust's `f32::round` disagree at a negative exact half step, and
/// WGSL's half-to-even disagrees with both elsewhere. The rule is floor.
#[test]
fn quantization_at_an_exact_half_step_follows_floor_plus_half() {
    // Authored 96 is s = −32/128 = −0.25 exactly.
    let s = surface_height_fraction(stored(&[96])[0]);
    assert_eq!(s, -0.25);
    // 6 levels: −1.5 → floor(−1.0) = −1 → −1/6. Half-away gives −2; half-even −2.
    assert_eq!(surface_height_quantize(s, 6.0), -1.0 / 6.0);
    // 2 levels: −0.5 → floor(0.0) = 0. Half-away gives −1; half-even gives 0.
    assert_eq!(surface_height_quantize(s, 2.0), 0.0);
    // Positive half step: 0.25 · 6 = 1.5 → 2 → 1/3. Half-even would also give 2.
    assert_eq!(surface_height_quantize(0.25, 6.0), 2.0 / 6.0);
    // 0.25 · 2 = 0.5 → 1 → 0.5. Half-even would give 0.
    assert_eq!(surface_height_quantize(0.25, 2.0), 0.5);
}

#[test]
fn quantization_is_per_direction_and_saturates_at_the_full_depth() {
    for levels in 1..=16u32 {
        let l = levels as f32;
        assert_eq!(surface_height_quantize(-1.0, l), -1.0);
        // White is 127/128, which every level count up to 64 rounds to 1.
        assert_eq!(surface_height_quantize(SURFACE_HEIGHT_MAX_RAISE, l), 1.0);
    }
    assert_eq!(
        surface_height_quantize(SURFACE_HEIGHT_MAX_RAISE, 0.0),
        SURFACE_HEIGHT_MAX_RAISE
    );
}

#[test]
fn sampling_wraps_like_address_mode_repeat() {
    let g = stored(&[PLANE, 160, 96, BLACK]);
    let f = field(2, 2, &g, 0);
    assert_eq!(f.texel_height(0, 0), 0.0);
    assert_eq!(f.texel_height(2, 2), 0.0);
    assert_eq!(f.texel_height(-1, -1), -1.0);
    assert_eq!(f.texel_height(-2, 3), -0.25);
    assert_eq!(f.texel_height(1, 0), 0.25);
}

#[test]
fn quantization_snaps_neighbours_onto_shared_plateaus() {
    // 140 and 144 are both nearest the 1/6 terrace; 60 and 64 nearest −3/6.
    let g = stored(&[140, 144, 60, 64]);
    let f = field(2, 2, &g, 6);
    assert_eq!(f.texel_height(0, 0), f.texel_height(1, 0));
    assert_eq!(f.texel_height(0, 1), f.texel_height(1, 1));
    assert_eq!(f.texel_height(0, 0), 1.0 / 6.0);
    assert_eq!(f.texel_height(0, 1), -3.0 / 6.0);
}

// -- Relief extraction (D6, P1) --

fn rg_level(width: u32, height: u32, stored_g: &[u8]) -> Vec<u8> {
    assert_eq!(stored_g.len(), (width * height) as usize);
    stored_g.iter().flat_map(|&g| [0x7Fu8, g]).collect()
}

#[test]
fn relief_extraction_takes_the_extremes_across_every_mip() {
    // Base level: plane and sinks only. Mip 1: one overshooting raise (a
    // Mitchell-Netravali lobe can do this). Mip 2: the deepest sink.
    let base = rg_level(2, 2, &[127, 127, 200, 150]);
    let mip1 = rg_level(1, 1, &[27]);
    let mip2 = rg_level(1, 1, &[240]);
    let relief =
        surface_relief_from_rg8_levels(&[(2, 2, &base[..]), (1, 1, &mip1[..]), (1, 1, &mip2[..])]);
    assert_eq!(relief.peak_raise, (127.0 - 27.0) / 128.0);
    assert_eq!(relief.trough, (127.0 - 240.0) / 128.0);
}

#[test]
fn relief_extraction_of_an_all_sink_map_has_no_peak() {
    let base = rg_level(2, 2, &[127, 255, 200, 150]);
    let mip1 = rg_level(1, 1, &[180]);
    let relief = surface_relief_from_rg8_levels(&[(2, 2, &base[..]), (1, 1, &mip1[..])]);
    assert_eq!(relief.peak_raise, 0.0);
    assert_eq!(relief.trough, -1.0);
}

#[test]
fn relief_extraction_of_an_all_raise_map_has_no_trough() {
    let base = rg_level(1, 2, &[0, 100]);
    let relief = surface_relief_from_rg8_levels(&[(1, 2, &base[..])]);
    assert_eq!(relief.peak_raise, SURFACE_HEIGHT_MAX_RAISE);
    assert_eq!(relief.trough, 0.0);
}

#[test]
fn relief_extraction_of_nothing_is_flat() {
    assert_eq!(surface_relief_from_rg8_levels(&[]), SurfaceRelief::FLAT);
    let mid = rg_level(2, 1, &[127, 127]);
    assert_eq!(
        surface_relief_from_rg8_levels(&[(2, 1, &mid[..])]),
        SurfaceRelief::FLAT
    );
}

// -- The view march --

#[test]
fn an_all_mid_gray_field_is_an_exact_no_op_at_every_level_count() {
    let authored = [PLANE; 16];
    let g = stored(&authored);
    for levels in 0..=16u32 {
        let f = field(4, 4, &g, levels);
        let relief = band(&authored, levels);
        assert!(relief.is_flat());
        for dir in [[0.0, 0.0], [0.4, -0.2], [400.0, 90.0]] {
            let hit = march_surface_depth(&f, [0.3, 0.7], dir, 0.02, relief, 48);
            assert_eq!(hit, SurfaceDepthHit::flat([0.3, 0.7]), "levels {levels}");
        }
    }
}

/// In a map that does rise, a mid-gray texel's top is still exactly the plane.
#[test]
fn a_mid_gray_texel_top_is_exactly_height_zero_in_a_raised_map() {
    let authored = [PLANE, WHITE, BLACK, 200];
    let g = stored(&authored);
    for levels in 0..=16u32 {
        let f = field(2, 2, &g, levels);
        let hit = march_surface_depth(
            &f,
            [0.25, 0.25],
            [0.0, 0.0],
            0.03,
            band(&authored, levels),
            48,
        );
        assert_eq!(hit.face, SurfaceDepthFace::Top);
        assert_eq!(hit.height_meters, 0.0, "levels {levels}");
    }
}

#[test]
fn an_all_black_field_sinks_exactly_the_full_depth() {
    let authored = [BLACK; 9];
    let g = stored(&authored);
    let scale = 0.06;
    for levels in 0..=16u32 {
        let f = field(3, 3, &g, levels);
        let relief = band(&authored, levels);
        assert_eq!(relief.peak_raise, 0.0);
        assert_eq!(relief.trough, -1.0);
        for dir in [[0.0, 0.0], [3.0, 1.0], [-5.0, 2.0]] {
            let hit = march_surface_depth(&f, [0.4, 0.6], dir, scale, relief, 48);
            assert_eq!(hit.height_meters, -scale, "levels {levels} dir {dir:?}");
            assert_eq!(hit.face, SurfaceDepthFace::Top);
        }
    }
}

#[test]
fn an_all_white_field_rises_by_its_quantized_peak() {
    let authored = [WHITE; 9];
    let g = stored(&authored);
    let scale = 0.06;
    for levels in 0..=16u32 {
        let f = field(3, 3, &g, levels);
        let relief = band(&authored, levels);
        // Unquantized white is 127/128 of the depth; any level count rounds it
        // up onto the full-depth terrace.
        let expected_peak = if levels == 0 {
            SURFACE_HEIGHT_MAX_RAISE
        } else {
            1.0
        };
        assert_eq!(relief.peak_raise, expected_peak, "levels {levels}");
        assert_eq!(relief.trough, 0.0);
        for dir in [[0.0, 0.0], [3.0, 1.0], [-5.0, 2.0]] {
            let hit = march_surface_depth(&f, [0.4, 0.6], dir, scale, relief, 48);
            assert_eq!(hit.height_meters, expected_peak * scale, "levels {levels}");
            assert_eq!(hit.face, SurfaceDepthFace::Top);
            // A plateau at the peak is met where the view ray enters the band.
            assert_eq!(hit.steps, 0);
        }
    }
}

#[test]
fn a_straight_down_ray_lands_on_its_own_texel_top() {
    // Texel (0,0) sinks half the depth.
    let authored = [64, PLANE, PLANE, PLANE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let hit = march_surface_depth(&f, [0.25, 0.25], [0.0, 0.0], 0.02, band(&authored, 0), 48);
    assert!((hit.height_meters + 0.01).abs() < 1e-6);
    assert_eq!(hit.face, SurfaceDepthFace::Top);
    assert_eq!(hit.uv, [0.25, 0.25]);
}

#[test]
fn a_neighbour_at_the_plane_is_hit_on_its_side_wall() {
    // Texel (0,0) is a full-depth pit, texel (1,0) sits on the plane. Marching
    // +u from inside the pit must hit (1,0)'s -u wall at the exact texel
    // boundary, not the top of the stone.
    let authored = [BLACK, PLANE, BLACK, PLANE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let scale = 0.02;
    // One texel of +u travel per 0.02 m of descent: the ray reaches the
    // boundary at u = 0.5 having descended the full scale.
    let hit = march_surface_depth(
        &f,
        [0.25, 0.25],
        [0.5 / scale, 0.0],
        scale,
        band(&authored, 0),
        48,
    );
    assert_eq!(hit.face, SurfaceDepthFace::NegU);
    assert!(
        (hit.march_uv[0] - 0.5).abs() < 1e-6,
        "side hit must land exactly on the texel boundary, got {}",
        hit.march_uv[0]
    );
    assert!(hit.height_meters > -scale && hit.height_meters < 0.0);
    // The sample UV is biased into the texel that was entered, so the
    // stone's own albedo reads on its wall.
    assert!((hit.uv[0] - (0.5 + 0.25)).abs() < 1e-6);
}

#[test]
fn a_raised_stone_is_hit_on_its_side_wall_above_the_plane() {
    // Plane, plane, stone, plane. The ray enters the band at the peak above
    // texel 0 and meets the stone's -u wall before it descends to the plane.
    let authored = [PLANE, PLANE, WHITE, PLANE];
    let g = stored(&authored);
    let f = field(4, 1, &g, 1);
    let scale = 0.02;
    let relief = band(&authored, 1);
    assert_eq!(relief.peak_raise, 1.0);
    // Two texels of travel per full depth, landing on the plane at u = 2.25
    // texels: it entered the band at 0.25 and reaches the stone at 2.0, 7/8 of
    // the way down.
    let hit = march_surface_depth(&f, [2.25 / 4.0, 0.5], [0.5 / scale, 0.0], scale, relief, 48);
    assert_eq!(hit.face, SurfaceDepthFace::NegU);
    assert!((hit.march_uv[0] - 0.5).abs() < 1e-6);
    assert!(
        (hit.height_meters - 0.125 * scale).abs() < 1e-7,
        "{}",
        hit.height_meters
    );

    // Straight down onto the stone: its top, a full depth above the plane.
    let top = march_surface_depth(&f, [2.25 / 4.0, 0.5], [0.0, 0.0], scale, relief, 48);
    assert_eq!(top.face, SurfaceDepthFace::Top);
    assert_eq!(top.height_meters, scale);
}

#[test]
fn a_side_hit_normal_opposes_the_direction_of_travel() {
    let right_stone = [BLACK, PLANE, BLACK, PLANE];
    let g = stored(&right_stone);
    let forward = march_surface_depth(
        &field(2, 2, &g, 0),
        [0.25, 0.25],
        [25.0, 0.0],
        0.02,
        band(&right_stone, 0),
        48,
    );
    assert_eq!(forward.face, SurfaceDepthFace::NegU);
    assert_eq!(forward.face.normal_texel(), [-1.0, 0.0, 0.0]);

    let left_stone = [PLANE, BLACK, PLANE, BLACK];
    let g = stored(&left_stone);
    let backward = march_surface_depth(
        &field(2, 2, &g, 0),
        [0.75, 0.25],
        [-25.0, 0.0],
        0.02,
        band(&left_stone, 0),
        48,
    );
    assert_eq!(backward.face, SurfaceDepthFace::PosU);
    assert_eq!(backward.face.normal_texel(), [1.0, 0.0, 0.0]);
}

#[test]
fn v_axis_crossings_produce_v_walls() {
    let authored = [BLACK, BLACK, PLANE, PLANE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let hit = march_surface_depth(&f, [0.25, 0.25], [0.0, 25.0], 0.02, band(&authored, 0), 48);
    assert_eq!(hit.face, SurfaceDepthFace::NegV);
    assert_eq!(hit.face.normal_texel(), [0.0, -1.0, 0.0]);
}

#[test]
fn a_flush_texel_stops_the_ray_at_the_plane() {
    // A plane texel in a carve-only map: the band's top IS the plane, so this
    // resolves as a geometric-normal top hit at height 0.
    let authored = [PLANE, BLACK, BLACK, BLACK];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let hit = march_surface_depth(&f, [0.25, 0.25], [25.0, 0.0], 0.02, band(&authored, 0), 48);
    assert_eq!(hit.height_meters, 0.0);
    assert_eq!(hit.face, SurfaceDepthFace::Top);
}

/// `|height| ≤ min(scale, SURFACE_DEPTH_MAX_METERS)` in each direction, for
/// any field, level count, scale and angle — including scales past the
/// ceiling the shader clamps to.
#[test]
fn the_height_never_exceeds_the_bound_in_either_direction() {
    let authored = noise(64, 7);
    let g = stored(&authored);
    let starts = [
        [0.37, 0.61],
        [0.05, 0.9],
        [0.52, 0.13],
        [0.8, 0.44],
        [0.21, 0.29],
    ];
    let mut raised = 0;
    let mut sunk = 0;
    for levels in [0u32, 1, 3, 6] {
        let f = field(8, 8, &g, levels);
        let relief = band(&authored, levels);
        for scale in [0.001f32, 0.05, 0.2, 0.5, 10.0] {
            let bound = scale.min(SURFACE_DEPTH_MAX_METERS);
            for tan in [0.0f32, 0.5, 2.0, 8.0] {
                for (az, uv0) in azimuths().into_iter().zip(starts.iter().cycle()) {
                    let per_m = tan / (bound / 6.0) / 8.0;
                    let dir = [az[0] * per_m, az[1] * per_m];
                    let hit = march_surface_depth(&f, *uv0, dir, scale, relief, 48);
                    // One ulp of slack for `peak·s − (peak − s(T))·s`.
                    assert!(
                        hit.height_meters.abs() <= bound * (1.0 + 1.0e-6),
                        "height {} past ±{bound} (scale {scale}, levels {levels})",
                        hit.height_meters,
                    );
                    if hit.height_meters > 0.0 {
                        raised += 1;
                    }
                    if hit.height_meters < 0.0 {
                        sunk += 1;
                    }
                }
            }
        }
    }
    assert!(
        raised > 0 && sunk > 0,
        "the sweep must reach both directions"
    );
}

/// D7: a starved march resolves flat at the true plane — original UV, height
/// 0, geometric normal, top — rather than at the last boundary it crossed.
#[test]
fn a_starved_march_resolves_flat_at_the_plane() {
    let authored = noise(9, 3);
    let g = stored(&authored);
    let f = field(3, 3, &g, 0);
    let relief = band(&authored, 0);
    let uv0 = [0.13, 0.77];
    let mut starved = 0;
    for az in azimuths() {
        // Grazing: thousands of texels per meter of descent.
        let hit = march_surface_depth(&f, uv0, [az[0] * 5000.0, az[1] * 5000.0], 0.02, relief, 8);
        if hit.starved {
            starved += 1;
            assert_eq!(hit.uv, uv0);
            assert_eq!(hit.march_uv, uv0);
            assert_eq!(hit.height_meters, 0.0);
            assert_eq!(hit.face, SurfaceDepthFace::Top);
            assert_eq!(hit.steps, 7);
        }
    }
    assert!(starved > 0, "no ray starved, so this proves nothing");
}

#[test]
fn the_march_terminates_within_the_step_budget_at_any_angle() {
    let authored = noise(9, 11);
    let g = stored(&authored);
    let f = field(3, 3, &g, 0);
    let relief = band(&authored, 0);
    for az in azimuths() {
        let hit = march_surface_depth(
            &f,
            [0.13, 0.77],
            [az[0] * 5000.0, az[1] * 5000.0],
            0.02,
            relief,
            12,
        );
        assert!(
            hit.steps < 12,
            "walked {} texels with a budget of 12",
            hit.steps
        );
        assert!(hit.height_meters.is_finite());
    }
}

/// A zero budget is clamped to one iteration, exactly as the shader's
/// `max(max_steps, 1u)` does: it resolves whatever the first texel resolves.
#[test]
fn a_zero_step_budget_marches_like_a_budget_of_one() {
    let authored = [64u8; 4];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let relief = band(&authored, 0);
    // Straight down: the first texel's top, half the depth below the plane.
    let down = march_surface_depth(&f, [0.25, 0.25], [0.0, 0.0], 0.02, relief, 0);
    assert!(!down.starved);
    assert_eq!(down.face, SurfaceDepthFace::Top);
    assert!((down.height_meters + 0.01).abs() < 1e-7);
    // Grazing enough to leave the first texel: one iteration starves it flat.
    for dir in [[0.0, 0.0], [25.0, 0.0], [400.0, -90.0]] {
        assert_eq!(
            march_surface_depth(&f, [0.25, 0.25], dir, 0.02, relief, 0),
            march_surface_depth(&f, [0.25, 0.25], dir, 0.02, relief, 1),
            "dir {dir:?}",
        );
    }
    assert!(march_surface_depth(&f, [0.25, 0.25], [400.0, -90.0], 0.02, relief, 0).starved);
}

/// P2 is exact: wherever the single-texel early-out fires it returns what
/// the full loop returns, bit for bit — and nowhere else does it change
/// anything.
#[test]
fn the_single_texel_early_out_is_exact() {
    let mut fired = 0;
    let mut looped = 0;
    for (seed, levels) in [(5u32, 0u32), (9, 3), (13, 6)] {
        let authored = noise(16 * 16, seed);
        let g = stored(&authored);
        let f = field(16, 16, &g, levels);
        let relief = band(&authored, levels);
        let scale = 0.06;
        let mut state = seed;
        for _ in 0..64 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let uv0 = [
                (state & 0xFFFF) as f32 / 65536.0,
                (state >> 16) as f32 / 65536.0,
            ];
            for tan in [0.0f32, 0.01, 0.05, 0.1, 0.5, 2.0] {
                for az in azimuths() {
                    let per_m = tan / 0.01 / 16.0;
                    let dir = [az[0] * per_m, az[1] * per_m];
                    let full = march_surface_depth_full_loop(&f, uv0, dir, scale, relief, 48);
                    let fast = march_surface_depth(&f, uv0, dir, scale, relief, 48);
                    assert_eq!(fast, full, "uv0 {uv0:?} dir {dir:?} levels {levels}");

                    let dims = [16.0f32, 16.0];
                    let d = [dir[0] * dims[0], dir[1] * dims[1]];
                    let peak_m = relief.peak_raise * scale;
                    let start = [
                        uv0[0] * dims[0] - d[0] * peak_m,
                        uv0[1] * dims[1] - d[1] * peak_m,
                    ];
                    let band_m = (relief.peak_raise - relief.trough) * scale;
                    if surface_depth_single_texel_band(&surface_depth_dda_setup(start, d), band_m) {
                        fired += 1;
                    } else {
                        looped += 1;
                    }
                }
            }
        }
    }
    assert!(fired > 0 && looped > 0, "fired {fired}, looped {looped}");
}

// -- Self-shadowing --

#[test]
fn a_pit_floor_is_shadowed_by_the_wall_beside_it() {
    // Texel (0,0) is a full-depth pit, (1,0) sits on the plane. A light
    // arriving from +u must be blocked by (1,0)'s wall.
    let authored = [BLACK, PLANE, BLACK, PLANE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let scale = 0.02;
    let visibility = surface_depth_light_visibility(
        &f,
        [0.25, 0.25],
        -scale,
        [0.5 / scale, 0.0],
        scale,
        0.0,
        12,
    );
    assert_eq!(visibility, 0.0);
}

#[test]
fn a_pit_floor_is_lit_when_the_light_clears_the_wall() {
    let authored = [BLACK, PLANE, BLACK, PLANE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let scale = 0.02;
    let visibility = surface_depth_light_visibility(
        &f,
        [0.25, 0.25],
        -scale,
        [0.05 / scale, 0.0],
        scale,
        0.0,
        12,
    );
    assert_eq!(visibility, 1.0);
}

/// P3: with relief above the plane, a hit ON the plane is not out of the band.
#[test]
fn the_shadow_march_does_not_exit_at_the_plane_when_relief_rises_above_it() {
    let authored = [PLANE, WHITE, PLANE, WHITE];
    let g = stored(&authored);
    let f = field(2, 2, &g, 1);
    let scale = 0.02;
    let peak = band(&authored, 1).peak_raise;
    assert_eq!(peak, 1.0);
    // The light reaches the stone's wall at half the depth above the plane,
    // below the stone's top: occluded.
    let blocked =
        surface_depth_light_visibility(&f, [0.25, 0.25], 0.0, [0.5 / scale, 0.0], scale, peak, 12);
    assert_eq!(
        blocked, 0.0,
        "the raised stone must shadow the plane beside it"
    );
    // Steep enough to clear the stone's top before reaching it: lit.
    let clear =
        surface_depth_light_visibility(&f, [0.25, 0.25], 0.0, [0.05 / scale, 0.0], scale, peak, 12);
    assert_eq!(clear, 1.0);
}

#[test]
fn a_top_hit_never_shadows_itself() {
    let authored = [64u8; 4];
    let g = stored(&authored);
    let f = field(2, 2, &g, 0);
    let scale = 0.02;
    let height = -0.5 * scale;
    for az in azimuths() {
        let visibility = surface_depth_light_visibility(
            &f,
            [0.25, 0.25],
            height,
            [az[0] * 30.0, az[1] * 30.0],
            scale,
            0.0,
            12,
        );
        assert_eq!(visibility, 1.0, "flat plateau must be fully lit");
    }
}

#[test]
fn a_top_hit_at_the_peak_skips_the_shadow_march() {
    // A uniform plateau at the peak: a hit there has nothing above it.
    let authored = [WHITE; 4];
    let g = stored(&authored);
    let f = field(2, 2, &g, 1);
    let scale = 0.02;
    assert_eq!(
        surface_depth_light_visibility(&f, [0.25, 0.25], scale, [30.0, 0.0], scale, 1.0, 12),
        1.0
    );
    // A carve-only map's plane is its peak, so a plane hit still skips.
    assert_eq!(
        surface_depth_light_visibility(&f, [0.25, 0.25], 0.0, [30.0, 0.0], scale, 0.0, 12),
        1.0
    );
}

// -- Ambient occlusion (D5) --

#[test]
fn ambient_occlusion_is_zero_on_a_texel_at_the_peak() {
    let authored = [WHITE, PLANE, PLANE, BLACK];
    let g = stored(&authored);
    let f = field(2, 2, &g, 1);
    let relief = band(&authored, 1);
    let scale = 0.02;
    let hit = march_surface_depth(&f, [0.25, 0.25], [0.0, 0.0], scale, relief, 48);
    assert_eq!(hit.height_meters, relief.peak_raise * scale);
    assert_eq!(
        surface_depth_ambient_occlusion(hit.height_meters, relief.peak_raise, scale, 1.0),
        1.0
    );
}

#[test]
fn ambient_occlusion_is_zero_across_an_all_mid_gray_map() {
    let authored = [PLANE; 4];
    let g = stored(&authored);
    let f = field(2, 2, &g, 6);
    let relief = band(&authored, 6);
    for az in azimuths() {
        let hit = march_surface_depth(
            &f,
            [0.3, 0.6],
            [az[0] * 40.0, az[1] * 40.0],
            0.02,
            relief,
            48,
        );
        assert_eq!(
            surface_depth_ambient_occlusion(hit.height_meters, relief.peak_raise, 0.02, 1.0),
            1.0
        );
    }
}

#[test]
fn ambient_occlusion_of_a_carve_only_map_is_the_pre_signed_formula() {
    let today = |depth_meters: f32, scale: f32, fade: f32| {
        1.0 - SURFACE_DEPTH_AO_STRENGTH * fade * (depth_meters / scale).clamp(0.0, 1.0)
    };
    // Authored values at and below the plane only: peak 0.
    let authored: Vec<u8> = noise(64, 21).iter().map(|&v| v / 2).collect();
    let g = stored(&authored);
    let f = field(8, 8, &g, 6);
    let relief = band(&authored, 6);
    assert_eq!(relief.peak_raise, 0.0);
    let scale = 0.06;
    let mut occluded = 0;
    for az in azimuths() {
        for tan in [0.0f32, 0.5, 1.0, 3.0] {
            let per_m = tan / 0.01 / 8.0;
            let hit = march_surface_depth(
                &f,
                [0.41, 0.17],
                [az[0] * per_m, az[1] * per_m],
                scale,
                relief,
                48,
            );
            for fade in [1.0f32, 0.5, 0.1] {
                let ao = surface_depth_ambient_occlusion(hit.height_meters, 0.0, scale, fade);
                assert_eq!(ao, today(-hit.height_meters, scale, fade));
                if ao < 1.0 {
                    occluded += 1;
                }
            }
        }
    }
    assert!(occluded > 0);
}

#[test]
fn ambient_occlusion_darkens_mid_gray_between_raised_texels() {
    let peak = 0.5;
    let ao = surface_depth_ambient_occlusion(0.0, peak, 0.02, 1.0);
    assert!(ao < 1.0);
    assert_eq!(ao, 1.0 - SURFACE_DEPTH_AO_STRENGTH * peak);
}

#[test]
fn ambient_occlusion_never_darkens_a_flat_material() {
    assert_eq!(surface_depth_ambient_occlusion(0.0, 0.0, 0.0, 1.0), 1.0);
    assert_eq!(
        surface_depth_ambient_occlusion(0.0, 1.0, f32::NAN, 1.0),
        1.0
    );
}

/// The occlusion must reach its flat value CONTINUOUSLY as the fade closes.
///
/// Both height arguments are post-fade, so their ratio is fade-invariant. Left
/// unscaled, occlusion stayed at full strength right up to the boundary and
/// then snapped to 1.0 when the resolve returned the flat result.
#[test]
fn ambient_occlusion_fades_out_with_the_relief() {
    let peak = 0.5;
    let full = surface_depth_ambient_occlusion(-0.02, peak, 0.02, 1.0);
    assert!(full < 1.0);
    let mut previous = full;
    for step in 1..=10u8 {
        let fade = 1.0 - f32::from(step) / 10.0_f32;
        let scale = 0.02 * fade;
        let ao = surface_depth_ambient_occlusion(-scale, peak, scale, fade);
        // STRICTLY weaker: `>=` passes on a constant function.
        assert!(ao > previous, "{ao} is not above {previous} at fade {fade}");
        previous = ao;
    }
    assert_eq!(surface_depth_ambient_occlusion(0.0, peak, 0.0, 0.0), 1.0);
}

// -- Fade, basis --

#[test]
fn distance_fade_reaches_zero_at_the_material_fade_distance() {
    assert_eq!(surface_depth_distance_fade(0.0, 10.0), 1.0);
    assert_eq!(surface_depth_distance_fade(7.5, 10.0), 1.0);
    assert!((surface_depth_distance_fade(8.75, 10.0) - 0.5).abs() < 1e-6);
    assert_eq!(surface_depth_distance_fade(10.0, 10.0), 0.0);
    assert_eq!(surface_depth_distance_fade(50.0, 10.0), 0.0);
}

#[test]
fn a_zero_fade_distance_is_always_flat() {
    assert_eq!(surface_depth_distance_fade(0.0, 0.0), 0.0);
}

#[test]
fn lod_fade_flattens_once_texels_go_sub_pixel() {
    assert_eq!(surface_depth_lod_fade(0.0), 1.0);
    assert_eq!(surface_depth_lod_fade(SURFACE_DEPTH_FADE_LOD_START), 1.0);
    assert_eq!(
        surface_depth_lod_fade(SURFACE_DEPTH_FADE_LOD_START + SURFACE_DEPTH_FADE_LOD_RANGE),
        0.0
    );
    assert!((surface_depth_lod_fade(2.0) - 0.5).abs() < 1e-6);
}

#[test]
fn lod_is_measured_in_base_mip_texels() {
    let lod_full = surface_depth_lod([1.0 / 1024.0, 0.0], [0.0, 1.0 / 1024.0], [1024.0, 1024.0]);
    assert!(lod_full.abs() < 1e-5);
    let lod_streamed = surface_depth_lod([1.0 / 1024.0, 0.0], [0.0, 1.0 / 1024.0], [512.0, 512.0]);
    assert!((lod_streamed + 1.0).abs() < 1e-5);
    assert!(surface_depth_lod_fade(lod_streamed) >= surface_depth_lod_fade(lod_full));
}

#[test]
fn the_combined_fade_takes_the_stricter_of_the_two() {
    assert_eq!(surface_depth_fade(0.0, 10.0, 9.0), 0.0);
    assert_eq!(surface_depth_fade(50.0, 10.0, 0.0), 0.0);
    assert_eq!(surface_depth_fade(0.0, 10.0, 0.0), 1.0);
}

#[test]
fn the_basis_recovers_world_units_per_uv_unit() {
    let basis = surface_depth_basis(
        [0.01, 0.0],
        [0.0, 0.005],
        Vec3::new(0.02, 0.0, 0.0),
        Vec3::new(0.0, 0.02, 0.0),
        Vec3::Z,
    )
    .expect("well-formed derivatives must yield a basis");
    assert!((basis.uv_per_meter[0] - 0.5).abs() < 1e-5);
    assert!((basis.uv_per_meter[1] - 0.25).abs() < 1e-5);
    assert!((basis.tangent - Vec3::X).length() < 1e-5);
    assert!((basis.bitangent - Vec3::Y).length() < 1e-5);
}

#[test]
fn a_close_range_fragment_keeps_its_basis() {
    // The determinant is a PRODUCT of two UV derivatives, so at close range on
    // a 1k texture it lands near 1e-9.
    let du = 5.0e-5_f32;
    let basis = surface_depth_basis(
        [du, 0.0],
        [0.0, du],
        Vec3::new(1.0e-4, 0.0, 0.0),
        Vec3::new(0.0, 1.0e-4, 0.0),
        Vec3::Z,
    );
    assert!(basis.is_some());
}

#[test]
fn a_nan_or_collapsed_chart_yields_no_basis() {
    let w = (Vec3::new(0.02, 0.0, 0.0), Vec3::new(0.0, 0.02, 0.0));
    assert!(surface_depth_basis([f32::NAN, 0.0], [0.0, 0.01], w.0, w.1, Vec3::Z).is_none());
    assert!(
        surface_depth_basis(
            [0.01, 0.0],
            [0.0, 0.01],
            Vec3::new(f32::NAN, 0.0, 0.0),
            w.1,
            Vec3::Z
        )
        .is_none()
    );
    assert!(surface_depth_basis([0.0, 0.0], [0.0, 0.0], w.0, w.1, Vec3::Z).is_none());
}

fn unit_basis(uv_per_meter: f32) -> SurfaceDepthBasis {
    SurfaceDepthBasis {
        tangent: Vec3::X,
        bitangent: Vec3::Y,
        normal: Vec3::Z,
        uv_per_meter: [uv_per_meter, uv_per_meter],
    }
}

#[test]
fn an_edge_on_fragment_has_no_view_ray() {
    let basis = unit_basis(1.0);
    assert!(surface_depth_view_ray(&basis, Vec3::X).is_none());
    assert!(surface_depth_view_ray(&basis, -Vec3::Z).is_none());
    assert!(surface_depth_view_ray(&basis, Vec3::Z).is_some());
    let sliver = Vec3::new(1.0, 0.0, SURFACE_DEPTH_MIN_DESCENT * 0.5).normalize();
    assert!(surface_depth_view_ray(&basis, sliver).is_none());
}

#[test]
fn a_head_on_view_ray_does_not_shift_uv() {
    let dir = surface_depth_view_ray(&unit_basis(2.0), Vec3::Z).expect("head-on ray");
    assert_eq!(dir, [0.0, 0.0]);
}

#[test]
fn a_grazing_view_ray_advances_uv_faster_than_a_steep_one() {
    let basis = unit_basis(1.0);
    let steep = surface_depth_view_ray(&basis, Vec3::new(0.2, 0.0, 1.0).normalize()).unwrap();
    let grazing = surface_depth_view_ray(&basis, Vec3::new(4.0, 0.0, 1.0).normalize()).unwrap();
    assert!(grazing[0].abs() > steep[0].abs());
}

#[test]
fn a_light_below_the_plane_has_no_shadow_ray() {
    let basis = unit_basis(1.0);
    assert!(surface_depth_light_ray(&basis, -Vec3::Z).is_none());
    assert!(surface_depth_light_ray(&basis, Vec3::Z).is_some());
}

// -- Texel→meters conversion (GPU-only; mirrored here for one assertion) --

/// Reproduces `surface_depth.wgsl`'s texel→meters conversion and its
/// `SURFACE_DEPTH_MAX_METERS` clamp. Test-local scaffolding, not the CPU
/// mirror the module header explains the owner declined to build.
fn shader_depth_scale_m(carve_request_texels: f32, texels_per_m: [f32; 2], fade: f32) -> f32 {
    let texel_rate = (texels_per_m[0] * texels_per_m[1])
        .max(SURFACE_DEPTH_EPS)
        .sqrt();
    let depth_scale_m = carve_request_texels / texel_rate;
    depth_scale_m.min(SURFACE_DEPTH_MAX_METERS * fade)
}

/// Regression: in texel mode the authored ceiling bounds a TEXEL COUNT, not
/// meters, so the shader must re-impose the meters ceiling after its divide.
#[test]
fn deepest_authored_texel_count_resolves_within_the_meters_ceiling_at_any_texel_rate() {
    assert!(surface_depth_is_texel_relative());
    let deepest_authored = surface_depth_max_authored();
    for texel_rate_axis in [1.0_f32, 4.0, 8.0, 16.0, 32.0, 128.0, 512.0, 4096.0] {
        let depth_scale_m =
            shader_depth_scale_m(deepest_authored, [texel_rate_axis, texel_rate_axis], 1.0);
        assert!(depth_scale_m <= SURFACE_DEPTH_MAX_METERS + 1e-6);
    }
    let depth_scale_m = shader_depth_scale_m(deepest_authored, [2.0, 4096.0], 1.0);
    assert!(depth_scale_m <= SURFACE_DEPTH_MAX_METERS + 1e-6);
}
