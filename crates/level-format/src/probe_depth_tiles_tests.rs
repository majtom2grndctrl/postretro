use super::*;
use crate::SectionId;
use crate::lightmap::f32_to_f16_bits;

fn error_message(result: crate::Result<ProbeDepthTilesSection>) -> String {
    format!("{:?}", result.expect_err("expected a rejection"))
}

fn tile_filled(mean: f32, mean_sq: f32) -> Vec<[u16; 2]> {
    vec![[f32_to_f16_bits(mean), f32_to_f16_bits(mean_sq)]; PROBE_DEPTH_TILE_TEXELS]
}

fn two_tiles() -> ProbeDepthTilesSection {
    let mut texels = tile_filled(1.0, 2.0);
    texels.extend(tile_filled(3.0, 10.0));
    ProbeDepthTilesSection {
        probe_indices: vec![4, 9],
        texels,
    }
}

#[test]
fn section_id_is_pinned() {
    assert_eq!(SectionId::ProbeDepthTiles as u32, 59);
    assert_eq!(SectionId::from_u32(59), Some(SectionId::ProbeDepthTiles));
}

#[test]
fn tiles_round_trip_with_the_header_indices_then_texels_in_index_order() {
    let section = two_tiles();
    let bytes = section.to_bytes();
    assert_eq!(
        &bytes[0..16],
        &[1, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(&bytes[16..24], &[4, 0, 0, 0, 9, 0, 0, 0]);
    assert_eq!(bytes.len(), 16 + 8 + 2 * 100 * 4);
    assert_eq!(section.encoded_len(), bytes.len());
    let first = f32_to_f16_bits(1.0).to_le_bytes();
    assert_eq!(
        &bytes[24..26],
        &first,
        "first texel's E[d] follows the indices"
    );
    assert_eq!(ProbeDepthTilesSection::from_bytes(&bytes).unwrap(), section);
}

#[test]
fn zero_tiles_is_the_header_alone() {
    let bytes = ProbeDepthTilesSection::default().to_bytes();
    assert_eq!(bytes.len(), 16);
    assert_eq!(ProbeDepthTilesSection::default().encoded_len(), 16);
    let section = ProbeDepthTilesSection::from_bytes(&bytes).unwrap();
    assert_eq!(section.tile_count(), 0);
    assert_eq!(section.texture_layout(), None);
}

fn with_word(mut bytes: Vec<u8>, at: usize, value: u32) -> Vec<u8> {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    bytes
}

#[test]
fn malformed_sections_refuse_by_name() {
    let good = two_tiles().to_bytes();
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (
            with_word(good.clone(), 0, 2),
            "ProbeDepthTiles version 2 is unknown",
        ),
        (
            with_word(good.clone(), 4, 6),
            "ProbeDepthTiles tile edge 6 is unsupported",
        ),
        (
            with_word(good.clone(), 8, 0),
            "ProbeDepthTiles border 0 is unsupported",
        ),
        (
            with_word(good.clone(), 12, MAX_PROBE_DEPTH_TILES + 1),
            "past the 32768-tile cap",
        ),
        (
            with_word(good.clone(), 20, 4),
            "must ascend without repeats; 4 follows 4",
        ),
        (
            with_word(good.clone(), 20, 3),
            "must ascend without repeats; 3 follows 4",
        ),
        (
            good[..good.len() - 1].to_vec(),
            "ProbeDepthTiles section is 823 bytes",
        ),
        (
            [good.as_slice(), &[0, 0, 0, 0]].concat(),
            "ProbeDepthTiles section is 828 bytes",
        ),
        (good[..10].to_vec(), "shorter than its 16-byte header"),
        (good[..2].to_vec(), "too short for its version"),
    ];
    for (bytes, expected) in cases {
        let message = error_message(ProbeDepthTilesSection::from_bytes(&bytes));
        assert!(message.contains(expected), "{expected:?} not in {message}");
    }
}

#[test]
fn unknown_version_is_named_at_any_length() {
    let bytes = with_word(two_tiles().to_bytes(), 0, 7);
    for len in [bytes.len(), 4] {
        let message = error_message(ProbeDepthTilesSection::from_bytes(&bytes[..len]));
        assert!(
            message.contains("ProbeDepthTiles version 7 is unknown") && message.contains("re-bake"),
            "{message}"
        );
    }
}

#[test]
fn lattice_validation_refuses_an_index_outside_or_naming_an_invalid_probe() {
    let section = two_tiles();
    // 2×3×2 = 12 probes; 9 invalid.
    let valid = |index: u32| index != 9;
    let message = format!(
        "{:?}",
        section
            .validate_against_lattice([2, 3, 2], valid)
            .expect_err("invalid probe")
    );
    assert!(
        message.contains("probe index 9 names an invalid probe"),
        "{message}"
    );
    let message = format!(
        "{:?}",
        section
            .validate_against_lattice([2, 2, 2], |_| true)
            .expect_err("outside")
    );
    assert!(
        message.contains("probe index 9 is outside the 2×2×2 probe lattice"),
        "{message}"
    );
    section
        .validate_against_lattice([2, 3, 2], |_| true)
        .expect("in range and valid");
}

#[test]
fn reference_flag_reads_the_raw_sign_bit() {
    assert_eq!(tile_reference_bits(0), 0x8000, "tile 0 is f16 -0");
    assert_eq!(
        tile_reference_bits(32_767),
        0xffff,
        "the top tile is an f16 NaN"
    );
    assert_eq!(
        decode_moment_second_channel(0x8000),
        MomentSecondChannel::Tile(0)
    );
    assert_eq!(
        decode_moment_second_channel(0xffff),
        MomentSecondChannel::Tile(32_767)
    );
    assert_eq!(
        decode_moment_second_channel(0x0000),
        MomentSecondChannel::Averaged(0),
        "+0 E[d²] stays untiled"
    );
    assert_eq!(
        decode_moment_second_channel(0x7c00),
        MomentSecondChannel::Averaged(0x7c00)
    );
    assert!(moment_sign_bit_set(0x8000, 0));
    assert!(moment_sign_bit_set(0, 0xbc00));
    assert!(!moment_sign_bit_set(0x7bff, 0x7bff));
}

#[test]
#[should_panic(expected = "past the 32768-tile reference range")]
fn a_reference_past_the_cap_panics() {
    tile_reference_bits(32_768);
}

#[test]
fn texture_layout_rows_are_a_power_of_two_wide_and_only_as_tall_as_needed() {
    let layout = |n| ProbeDepthTileTextureLayout::for_tile_count(n).unwrap();
    assert_eq!(layout(1).tiles_per_row(), 1);
    assert_eq!(layout(1).extent(), [10, 10]);
    assert_eq!(layout(5).tiles_per_row(), 4);
    assert_eq!(layout(5).rows, 2);
    assert_eq!(layout(16).extent(), [40, 40]);
    // The hallway bake's 13,451 tiles at 8×8: 128 per row, 106 rows.
    let hallway = layout(13_451);
    assert_eq!((hallway.tiles_per_row(), hallway.rows), (128, 106));
    assert_eq!(hallway.texture_bytes(), 1280 * 1060 * 4);
    let cap = layout(MAX_PROBE_DEPTH_TILES);
    assert_eq!(cap.extent(), [2560, 1280]);
    assert_eq!(cap.tile_origin(MAX_PROBE_DEPTH_TILES - 1), [2550, 1270]);
    assert_eq!(layout(5).tile_origin(4), [0, 10]);
    assert_eq!(probe_depth_tile_texture_bytes(0), 0);
}

#[test]
fn bordered_tile_wraps_like_irradiance_tiles_and_sampling_reads_its_interior() {
    let interior: Vec<[u16; 2]> = (0..PROBE_DEPTH_TILE_INTERIOR_TEXELS)
        .map(|i| [f32_to_f16_bits(i as f32), f32_to_f16_bits(2.0 * i as f32)])
        .collect();
    let tile = bordered_tile(&interior);
    assert_eq!(tile.len(), PROBE_DEPTH_TILE_TEXELS);
    // Interior texel (0, 0) lands at bordered (1, 1).
    assert_eq!(tile[11], interior[0]);
    // The left border of row 1 mirrors the interior's left edge about its midpoint.
    assert_eq!(tile[10], interior[7 * 8]);
    // A direction at an interior texel centre samples that texel exactly.
    for (x, y) in [(0u32, 0u32), (3, 5), (7, 7)] {
        let dir = crate::octahedral::irradiance_interior_texel_direction(
            x,
            y,
            PROBE_DEPTH_TILE_BORDERED_EDGE,
            PROBE_DEPTH_TILE_BORDER,
        );
        let i = (y * 8 + x) as usize;
        assert_eq!(interior_texel_of_direction(dir), i);
        let [mean, mean_sq] = sample_tile_moments(&tile, dir);
        assert!((mean - i as f32).abs() < 1e-3, "{mean} vs {i}");
        assert!((mean_sq - 2.0 * i as f32).abs() < 2e-3);
    }
}

// A shaded point at the probe hands the lookup a zero direction. The shader
// floors its L1 divide, so it reads the tile centre; the mirror must too.
#[test]
fn a_zero_direction_samples_the_tile_centre_without_nan() {
    let interior: Vec<[u16; 2]> = (0..PROBE_DEPTH_TILE_INTERIOR_TEXELS)
        .map(|i| [f32_to_f16_bits(i as f32), f32_to_f16_bits(2.0 * i as f32)])
        .collect();
    let tile = bordered_tile(&interior);
    for dir in [[0.0f32; 3], [-0.0; 3], [0.0, 0.0, -0.0]] {
        let [mean, mean_sq] = sample_tile_moments(&tile, dir);
        assert!(mean.is_finite() && mean_sq.is_finite(), "{dir:?}");
        // The centre sits between interior texels 27, 28, 35 and 36.
        assert_eq!((mean, mean_sq), (31.5, 63.0), "{dir:?}");
        assert_eq!(interior_texel_of_direction(dir), 36, "{dir:?}");
    }
}

#[test]
fn f16_decode_inverts_the_encoder() {
    for v in [0.0f32, 0.25, 1.0, 3.5, 1.0e-6, 65504.0, 0.000_061_035_156] {
        assert_eq!(f16_bits_to_f32(f32_to_f16_bits(v)), {
            let bits = f32_to_f16_bits(v);
            // Re-encode the decode: exact round trip on representable values.
            assert_eq!(f32_to_f16_bits(f16_bits_to_f32(bits)), bits);
            f16_bits_to_f32(bits)
        });
    }
    assert_eq!(f16_bits_to_f32(0x3c00), 1.0);
    assert_eq!(f16_bits_to_f32(0x8000), -0.0);
    assert!(f16_bits_to_f32(0xffff).is_nan());
    assert_eq!(f16_bits_to_f32(0xfc00), f32::NEG_INFINITY);
}
