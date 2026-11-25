// Colour mip prefilter tests: spread per mip, per-mip borders, the probe's hemisphere alone, the chain's layout and mip 0's bytes.
// See: context/lib/resource_management.md §5.4 (Colour mips) · testing_guide.md

use super::*;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_level_format::reflection_probes::{
    reflection_lobe_half_width, reflection_probe_colour_chain_bytes,
    reflection_probe_mip_half_width,
};

const MIPS: u32 = 6;

/// Hemispheres of every kind a probe faces: up, a wall, a ceiling, a slope.
fn hemispheres() -> [ShareHemisphere; 4] {
    [
        ShareHemisphere::Up,
        ShareHemisphere::Around(Vec3::X),
        ShareHemisphere::Around(Vec3::NEG_Y),
        ShareHemisphere::Around(Vec3::new(0.0, 1.0, 1.0).normalize()),
    ]
}

/// Interior rows of the mip-0 capture from `radiance(x, y)`.
fn capture(radiance: impl Fn(u32, u32) -> Vec3) -> Vec<Vec<Vec3>> {
    (0..TILE_INTERIOR)
        .map(|y| (0..TILE_INTERIOR).map(|x| radiance(x, y)).collect())
        .collect()
}

fn as_rows(rows: &[Vec<Vec3>]) -> Vec<&[Vec3]> {
    rows.iter().map(Vec::as_slice).collect()
}

/// A smooth sky: bright toward +Y and +X, dim below, never zero.
fn smooth(x: u32, y: u32) -> Vec3 {
    let d = texel_direction(x, y, TILE_INTERIOR);
    Vec3::new(
        1.0 + d.y.max(0.0) * 3.0,
        0.5 + d.x.max(0.0),
        0.25 + 0.5 * (d.z + 1.0),
    )
}

/// The same mean as [`prefilter_mip`], over every source texel in
/// `hemisphere`: no cull.
fn brute_force(rows: &[&[Vec3]], mip: u32, hemisphere: ShareHemisphere) -> Vec<Vec3> {
    let exponent = reflection_probe_mip_exponent(mip, CAPTURE_MIP0_TEXEL_ANGLE);
    let interior = mip_interior(mip);
    let mut out = Vec::new();
    for y in 0..interior {
        for x in 0..interior {
            let lookup = texel_direction(x, y, interior);
            let (mut sum, mut weight) = (DVec3::ZERO, 0.0f64);
            for sy in 0..TILE_INTERIOR {
                for sx in 0..TILE_INTERIOR {
                    let d = texel_direction(sx, sy, TILE_INTERIOR);
                    if !hemisphere.holds(d) {
                        continue;
                    }
                    let w = f64::from(
                        prefilter_weight(lookup.dot(d), exponent)
                            * texel_solid_angle(sx, sy, TILE_INTERIOR),
                    );
                    sum += rows[sy as usize][sx as usize].as_dvec3() * w;
                    weight += w;
                }
            }
            out.push(if weight > 0.0 {
                (sum / weight).as_vec3()
            } else {
                Vec3::ZERO
            });
        }
    }
    out
}

#[test]
fn the_prefilter_lobe_has_the_mapped_half_width() {
    for mip in 1..MIPS {
        let exponent = reflection_probe_mip_exponent(mip, CAPTURE_MIP0_TEXEL_ANGLE);
        let theta = reflection_lobe_half_width(exponent);
        assert!(
            (prefilter_weight(theta.cos(), exponent) - 0.5).abs() < 1e-4,
            "mip {mip}"
        );
        assert_eq!(prefilter_weight(1.0, exponent), 1.0);
        assert_eq!(prefilter_weight(-1.0, exponent), 0.0);
    }
}

#[test]
fn the_tabulated_lobe_follows_the_exact_lobe_and_cuts_at_the_cutoff() {
    for mip in 1..MIPS {
        let exponent = reflection_probe_mip_exponent(mip, CAPTURE_MIP0_TEXEL_ANGLE);
        let lobe = PrefilterLobe::new(exponent);
        for step in 0..=20_000 {
            let cos_angle = 1.0 - 2.0 * step as f32 / 20_000.0;
            let exact = prefilter_weight(cos_angle, exponent);
            let tabled = lobe.weight(cos_angle);
            if exact >= 2.0 * PREFILTER_CUTOFF {
                assert!(
                    (tabled - exact).abs() < 1e-4,
                    "mip {mip} cos {cos_angle}: {tabled} vs {exact}"
                );
            } else {
                assert!(
                    tabled <= 2.0 * PREFILTER_CUTOFF,
                    "mip {mip} cos {cos_angle}"
                );
            }
        }
        assert_eq!(lobe.weight(1.0), 1.0);
    }
}

#[test]
fn texel_solid_angles_cover_the_sphere_at_every_mip() {
    for mip in 0..MIPS {
        let n = mip_interior(mip);
        let total: f64 = (0..n)
            .flat_map(|y| (0..n).map(move |x| f64::from(texel_solid_angle(x, y, n))))
            .sum();
        let sphere = 4.0 * std::f64::consts::PI;
        // Centre-point quadrature: exact in the limit, a few percent at 2².
        let tolerance = if n >= 14 { 0.01 } else { 0.25 };
        assert!(
            (total - sphere).abs() / sphere < tolerance,
            "mip {mip}: {total}"
        );
    }
}

// A one-texel bright source, prefiltered, spreads at each colour mip
// to the reflected-direction half-width of the exponent that maps to that
// mip, within one texel of that mip (α₀·2^m). The half-maximum contour lies
// between θ − α and θ + α: every texel at half the peak or more lies within
// θ + α of the source, and every texel under half lies at least θ − α away.
// The probe faces the source, and only lookups in its hemisphere count, as
// only they are read.
#[test]
fn a_one_texel_source_spreads_to_each_mips_mapped_half_width() {
    // Off every axis and fold, in both hemispheres of the octahedral square.
    for (sx, sy) in [(40, 70), (100, 20), (63, 62)] {
        let rows = capture(|x, y| {
            if (x, y) == (sx, sy) {
                Vec3::splat(1000.0)
            } else {
                Vec3::ZERO
            }
        });
        let rows = as_rows(&rows);
        let source_direction = texel_direction(sx, sy, TILE_INTERIOR);
        let hemisphere = ShareHemisphere::Around(source_direction);
        let source = PrefilterSource::new(&rows, hemisphere);
        for mip in 1..MIPS {
            let theta = reflection_lobe_half_width(reflection_probe_mip_exponent(
                mip,
                CAPTURE_MIP0_TEXEL_ANGLE,
            ));
            assert!(
                (theta - reflection_probe_mip_half_width(mip, CAPTURE_MIP0_TEXEL_ANGLE)).abs()
                    < 1e-3
            );
            let texel = reflection_probe_mip_half_width(mip, CAPTURE_MIP0_TEXEL_ANGLE);
            let interior = mip_interior(mip);
            let filtered = prefilter_mip(&source, mip);
            let read = |x: u32, y: u32| hemisphere.holds(texel_direction(x, y, interior));
            let peak = (0..interior)
                .flat_map(|y| (0..interior).map(move |x| (x, y)))
                .filter(|&(x, y)| read(x, y))
                .map(|(x, y)| filtered[(y * interior + x) as usize].x)
                .fold(0.0f32, f32::max);
            assert!(peak > 0.0, "mip {mip}: the source reaches the mip");
            let (mut widest_bright, mut nearest_dim) = (0.0f32, f32::INFINITY);
            for y in 0..interior {
                for x in 0..interior {
                    if !read(x, y) {
                        continue;
                    }
                    let angle = texel_direction(x, y, interior)
                        .dot(source_direction)
                        .clamp(-1.0, 1.0)
                        .acos();
                    if filtered[(y * interior + x) as usize].x >= 0.5 * peak {
                        widest_bright = widest_bright.max(angle);
                    } else {
                        nearest_dim = nearest_dim.min(angle);
                    }
                }
            }
            eprintln!(
                "[prefilter spread] source ({sx},{sy}) mip {mip}: θ {:.2}°, half-max set reaches \
                 {:.2}°, first dim texel {:.2}°, texel {:.2}°",
                theta.to_degrees(),
                widest_bright.to_degrees(),
                nearest_dim.to_degrees(),
                texel.to_degrees()
            );
            assert!(
                widest_bright <= theta + texel,
                "({sx},{sy}) mip {mip}: bright to {}° past θ {}° + {}°",
                widest_bright.to_degrees(),
                theta.to_degrees(),
                texel.to_degrees()
            );
            assert!(
                nearest_dim >= theta - texel,
                "({sx},{sy}) mip {mip}: dim at {}° inside θ {}° − {}°",
                nearest_dim.to_degrees(),
                theta.to_degrees(),
                texel.to_degrees()
            );
        }
    }
}

// Over the lookups an up-facing probe reads: past the horizon a lookup's
// weight can be all lobe tail, which the cutoff drops by design.
#[test]
fn the_block_cull_changes_no_mip_by_more_than_its_cutoff() {
    let rows = capture(smooth);
    let rows = as_rows(&rows);
    let hemisphere = ShareHemisphere::Up;
    let source = PrefilterSource::new(&rows, hemisphere);
    for mip in 1..MIPS {
        let culled = prefilter_mip(&source, mip);
        let exact = brute_force(&rows, mip, hemisphere);
        let interior = mip_interior(mip);
        for (index, (a, b)) in culled.iter().zip(&exact).enumerate() {
            let (x, y) = (index as u32 % interior, index as u32 / interior);
            if !hemisphere.holds(texel_direction(x, y, interior)) {
                continue;
            }
            let error = (*a - *b).abs().max_element() / b.max_element();
            assert!(
                error < 10.0 * PREFILTER_CUTOFF,
                "mip {mip} texel {index}: {a} vs {b}"
            );
        }
    }
}

// Every lookup the probe reads keeps the colour; a texel behind its horizon
// keeps it too, or stores 0 where its lobe reaches no texel in front.
#[test]
fn a_uniform_capture_stays_uniform_at_every_mip() {
    let colour = Vec3::new(0.3, 1.5, 4.0);
    let rows = capture(|_, _| colour);
    let rows = as_rows(&rows);
    for hemisphere in hemispheres() {
        let source = PrefilterSource::new(&rows, hemisphere);
        for mip in 1..MIPS {
            let interior = mip_interior(mip);
            for (index, texel) in prefilter_mip(&source, mip).into_iter().enumerate() {
                let (x, y) = (index as u32 % interior, index as u32 / interior);
                let uniform = (texel - colour).abs().max_element() < 1e-5;
                let read = hemisphere.holds(texel_direction(x, y, interior));
                assert!(
                    uniform || (!read && texel == Vec3::ZERO),
                    "{hemisphere:?} mip {mip} ({x}, {y}): {texel}"
                );
            }
        }
    }
}

// An up-facing probe over a glossy floor: its rays pass through the floor's
// plane and miss, so the lower hemisphere is black. No mip past 0 darkens
// toward the horizon, where Fresnel weight is highest: every lookup it reads
// keeps the upper hemisphere's colour.
#[test]
fn an_up_probes_mips_never_darken_toward_a_black_lower_hemisphere() {
    let colour = Vec3::new(2.0, 1.0, 0.5);
    let rows = capture(|x, y| {
        if texel_direction(x, y, TILE_INTERIOR).y >= 0.0 {
            colour
        } else {
            Vec3::ZERO
        }
    });
    let rows = as_rows(&rows);
    let hemisphere = ShareHemisphere::Up;
    let source = PrefilterSource::new(&rows, hemisphere);
    for mip in 1..MIPS {
        let interior = mip_interior(mip);
        let filtered = prefilter_mip(&source, mip);
        for y in 0..interior {
            for x in 0..interior {
                let lookup = texel_direction(x, y, interior);
                if !hemisphere.holds(lookup) {
                    continue;
                }
                let texel = filtered[(y * interior + x) as usize];
                assert!(
                    (texel - colour).abs().max_element() < 1e-5,
                    "mip {mip} ({x}, {y}) at height {}: {texel}",
                    lookup.y
                );
            }
        }
    }
}

// A glossy wall's probe: its rays pass through the wall's plane into the
// bright room beyond it. None of that room reaches any mip past 0, at a
// lookup the probe reads or behind its horizon.
#[test]
fn a_wall_probes_mips_hold_none_of_the_room_behind_its_plane() {
    let (lit, beyond) = (Vec3::new(0.2, 0.3, 0.4), Vec3::splat(1000.0));
    let rows = capture(|x, y| {
        if texel_direction(x, y, TILE_INTERIOR).x >= 0.0 {
            lit
        } else {
            beyond
        }
    });
    let rows = as_rows(&rows);
    let hemisphere = ShareHemisphere::Around(Vec3::X);
    let source = PrefilterSource::new(&rows, hemisphere);
    for mip in 1..MIPS {
        let interior = mip_interior(mip);
        let filtered = prefilter_mip(&source, mip);
        for y in 0..interior {
            for x in 0..interior {
                let texel = filtered[(y * interior + x) as usize];
                let read = hemisphere.holds(texel_direction(x, y, interior));
                let kept = (texel - lit).abs().max_element() < 1e-5;
                assert!(
                    kept || (!read && texel == Vec3::ZERO),
                    "mip {mip} ({x}, {y}): {texel}"
                );
            }
        }
    }
}

/// The wrap rule restated: an edge border mirrors its own edge, a corner
/// takes the diagonal (interior coordinates of the source texel).
fn wrapped_source(x: u32, y: u32, dimension: u32) -> [u32; 2] {
    let n = dimension as i64 - 2;
    let (ix, iy) = (x as i64 - 1, y as i64 - 1);
    let inside = |v: i64| (0..n).contains(&v);
    let source = match (inside(ix), inside(iy)) {
        (true, true) => (ix, iy),
        (false, true) => (if ix < 0 { 0 } else { n - 1 }, n - 1 - iy),
        (true, false) => (n - 1 - ix, if iy < 0 { 0 } else { n - 1 }),
        (false, false) => (
            if ix < 0 { n - 1 } else { 0 },
            if iy < 0 { n - 1 } else { 0 },
        ),
    };
    [source.0 as u32, source.1 as u32]
}

// At every colour mip past 0 (mip 0's border is the capture's own,
// `capture::tests`), each tile border copies its wrapped interior, and the
// stored chain is exactly those bordered tiles, BC6H-encoded mip by mip.
#[test]
fn every_mips_border_copies_its_wrapped_interior() {
    let rows = capture(smooth);
    let rows = as_rows(&rows);
    let hemisphere = ShareHemisphere::Around(Vec3::X);
    let mips = prefiltered_mips(&rows, MIPS, hemisphere);
    assert_eq!(mips.len(), MIPS as usize - 1);
    let mut stored = encode_tile(&rows);
    for (mip, rgba) in (1..MIPS).zip(&mips) {
        let dimension = mip_dimension(mip);
        assert_eq!(
            rgba.len(),
            (dimension * dimension * 4) as usize,
            "mip {mip}"
        );
        let at = |x: u32, y: u32| {
            let i = ((y * dimension + x) * 4) as usize;
            Vec3::new(rgba[i], rgba[i + 1], rgba[i + 2])
        };
        let mut borders = 0;
        for y in 0..dimension {
            for x in 0..dimension {
                if (1..dimension - 1).contains(&x) && (1..dimension - 1).contains(&y) {
                    continue;
                }
                let [ix, iy] = wrapped_source(x, y, dimension);
                assert_eq!(at(x, y), at(ix + 1, iy + 1), "mip {mip} border ({x}, {y})");
                borders += 1;
            }
        }
        assert_eq!(borders, 4 * (dimension - 1), "mip {mip}");
        stored.extend(crate::bc6h::encode_bc6h_rgb_from_f32_rgba(
            rgba, dimension, dimension,
        ));
    }
    assert_eq!(encode_colour_chain(&rows, MIPS, hemisphere), stored);
}

#[test]
fn the_chain_keeps_mip_zero_byte_identical_and_has_the_formats_length() {
    let rows = capture(smooth);
    let rows = as_rows(&rows);
    let mip0 = encode_tile(&rows);
    for hemisphere in hemispheres() {
        assert_eq!(
            encode_colour_chain(&rows, 1, hemisphere),
            mip0,
            "one mip is the capture alone"
        );
        // Mip 0 holds the whole sphere as captured, whatever the facing.
        let chain = encode_colour_chain(&rows, MIPS, hemisphere);
        assert_eq!(&chain[..mip0.len()], mip0.as_slice());
        assert_eq!(
            Some(chain.len()),
            reflection_probe_colour_chain_bytes(
                IRRADIANCE_FORMAT_BC6H,
                REFLECTION_PROBE_TILE_DIMENSION,
                MIPS
            )
        );
    }
}
