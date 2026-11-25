// Colour mip selection and per-mip read reference tests.
// See: context/lib/testing_guide.md

use super::*;
use postretro_level_format::octahedral::{
    encode_unquantized_uv, irradiance_interior_texel_direction, irradiance_tile_source_texel,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
    reflection_probe_full_colour_mip_count, reflection_probe_mip_exponent,
};
use postretro_level_format::specular_smoothness::{SPECULAR_EXPONENT_MAX, SPECULAR_EXPONENT_MIN};

const TILE: u32 = REFLECTION_PROBE_TILE_DIMENSION;
const BORDER: u32 = REFLECTION_PROBE_TILE_BORDER;

fn mip_count() -> u32 {
    reflection_probe_full_colour_mip_count(TILE)
}

/// α₀ of the engine's 128² tile.
fn angle() -> f32 {
    reflection_probe_mip0_texel_angle(TILE)
}

/// A value naming interior texel `(x, y)` of mip `mip`, exact in f16.
pub(crate) fn texel_value(mip: u32, x: u32, y: u32) -> Vec3 {
    Vec3::new(mip as f32, x as f32, y as f32)
}

/// Every mip's tile, border included, holding [`texel_value`] of its
/// interior texels, bordered by the SH atlas's wrap rule as the compiler
/// writes it.
pub(crate) fn labelled_chain(tile: u32, mips: u32) -> Vec<Vec<Vec3>> {
    (0..mips)
        .map(|mip| {
            let dimension = tile >> mip;
            (0..dimension)
                .flat_map(|y| (0..dimension).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, BORDER);
                    texel_value(mip, ix, iy)
                })
                .collect()
        })
        .collect()
}

/// Interior texels near the octahedral folds and corners, and a few inside.
pub(crate) fn fold_texels(interior: u32) -> Vec<(u32, u32)> {
    let last = interior - 1;
    let mut texels = vec![
        (0, 0),
        (last, 0),
        (0, last),
        (last, last),
        (interior / 2, 0),
        (0, interior / 2),
        (last, interior / 2 - 1),
        (interior / 2 - 1, last),
        (interior / 2, interior / 2),
        (interior / 4, interior / 4 + 1),
    ];
    texels.sort_unstable();
    texels.dedup();
    texels
}

#[test]
fn selection_blends_the_mapped_mip_between_its_floor_and_the_next() {
    let count = mip_count();
    for n in [8.0f32, 16.0, 40.0, 64.0, 100.0] {
        let mip = reflection_probe_exponent_mip(n, angle(), count);
        let mips = reflection_probe_mips(n, TILE, count);
        assert_eq!(mips.lower, mip.floor() as u32, "n = {n}");
        assert_eq!(mips.upper, mips.lower + 1, "n = {n}");
        assert!((mips.lower as f32 + mips.blend - mip).abs() < 1e-6);
        assert!((0.0..1.0).contains(&mips.blend));
    }
    // The cap reads the minimum blur alone; the minimum exponent the last mip.
    let cap = reflection_probe_mips(SPECULAR_EXPONENT_MAX, TILE, count);
    assert_eq!(
        (cap.lower, cap.blend),
        (REFLECTION_PROBE_MIN_BLUR_MIP as u32, 0.0)
    );
    let rough = reflection_probe_mips(SPECULAR_EXPONENT_MIN, TILE, count);
    assert_eq!(
        (rough.lower, rough.upper, rough.blend),
        (count - 1, count - 1, 0.0)
    );
    // A one-mip chain reads mip 0 alone.
    let one = reflection_probe_mips(64.0, TILE, 1);
    assert_eq!((one.lower, one.upper, one.blend), (0, 0, 0.0));
}

// The selection reads the section's own tile dimension: at 64² a texel's
// blur angle falls one mip lower than at 128², at 256² one higher, each
// clamped to its own chain; a one-mip section of any size reads mip 0 alone.
#[test]
fn selection_follows_the_sections_tile_dimension() {
    for (tile, offset) in [(64u32, -1.0f32), (256, 1.0)] {
        let count = reflection_probe_full_colour_mip_count(tile);
        for n in [16.0f32, 40.0, 64.0, 100.0] {
            let engine = reflection_probe_exponent_mip(n, angle(), mip_count());
            let expected = (engine + offset)
                .max(REFLECTION_PROBE_MIN_BLUR_MIP)
                .min((count - 1) as f32);
            let mips = reflection_probe_mips(n, tile, count);
            assert!(
                (mips.lower as f32 + mips.blend - expected).abs() < 1e-4,
                "{tile}²: n = {n} reads {mips:?}, expected {expected}"
            );
            assert!(mips.upper < count);
        }
        let one = reflection_probe_mips(64.0, tile, 1);
        assert_eq!((one.lower, one.upper, one.blend), (0, 0, 0.0), "{tile}²");
    }
}

// Reference half: at every colour mip down to 4², the
// direction the compiler wrote an interior texel for lands on that texel's
// centre in that mip's own tile, so a filtered read returns it exactly;
// across a selection between two mips, the other mip's read lands inside the
// interior texel the compiler assigned that direction there.
#[test]
fn a_direction_reads_its_own_interior_texel_at_every_mip() {
    let count = mip_count();
    let chain = labelled_chain(TILE, count);
    for mip in 0..count {
        let dimension = TILE >> mip;
        let interior = dimension - 2 * BORDER;
        for (x, y) in fold_texels(interior) {
            let direction =
                Vec3::from(irradiance_interior_texel_direction(x, y, dimension, BORDER));
            let uv = reflection_tile_mip_uv(direction, TILE, BORDER, mip);
            let texel = uv * dimension as f32;
            let centre = Vec2::new(x as f32 + 1.5, y as f32 + 1.5);
            assert!(
                (texel - centre).abs().max_element() < 1e-3,
                "mip {mip} ({x}, {y}): {texel} vs {centre}"
            );
            // Labels of neighbouring texels differ by 1: within 1% is that
            // texel alone, up to the centre's rounding.
            let read = reflection_probe_mip_sample(&chain[mip as usize], dimension, uv);
            assert!(
                (read - texel_value(mip, x, y)).abs().max_element() < 0.01,
                "mip {mip} ({x}, {y}): {read}"
            );
            // The neighbouring mips: the read centres inside the texel the
            // direction maps to there.
            for other in [mip.wrapping_sub(1), mip + 1] {
                if other >= count {
                    continue;
                }
                let other_dimension = TILE >> other;
                let other_interior = (other_dimension - 2 * BORDER) as f32;
                let oct = Vec2::from(encode_unquantized_uv(direction.x, direction.y, direction.z));
                let expected = (oct * other_interior)
                    .floor()
                    .min(Vec2::splat(other_interior - 1.0));
                let read = reflection_tile_mip_uv(direction, TILE, BORDER, other)
                    * other_dimension as f32
                    - Vec2::splat(BORDER as f32);
                assert_eq!(
                    read.floor(),
                    expected,
                    "mip {mip} ({x}, {y}) in mip {other}"
                );
            }
        }
    }
}

// The reference read blends the two mips' own reads by the selection.
#[test]
fn the_colour_read_blends_each_mips_own_read() {
    let count = mip_count();
    let chain = labelled_chain(TILE, count);
    let refs: Vec<&[Vec3]> = chain.iter().map(Vec::as_slice).collect();
    // An exponent halfway between mips 2 and 3, a direction at a mip-2
    // texel centre near the fold.
    let n = reflection_probe_mip_exponent(2, angle()) * 0.5
        + reflection_probe_mip_exponent(3, angle()) * 0.5;
    let mips = reflection_probe_mips(n, TILE, count);
    assert_eq!((mips.lower, mips.upper), (2, 3));
    let direction = Vec3::from(irradiance_interior_texel_direction(
        0,
        15,
        TILE >> 2,
        BORDER,
    ));
    let colour = reflection_probe_mip_colour(&refs, TILE, BORDER, direction, n);
    let lower = texel_value(2, 0, 15);
    let upper = reflection_probe_mip_sample(
        &chain[3],
        TILE >> 3,
        reflection_tile_mip_uv(direction, TILE, BORDER, 3),
    );
    let expected = lower * (1.0 - mips.blend) + upper * mips.blend;
    assert!(
        (colour - expected).abs().max_element() < 1e-5,
        "{colour} vs {expected}"
    );
    assert_eq!(
        colour.x,
        2.0 + mips.blend,
        "the mip label blends by the selection"
    );
}
