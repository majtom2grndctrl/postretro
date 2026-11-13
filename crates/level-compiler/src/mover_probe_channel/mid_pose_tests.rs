// What a mid pose holds for a near-field probe from its own posed gather:
// its state where that clears the probe's bars against id 34 and the base
// where it stays within them, the probe enclosed or not.

use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::classify;
use crate::mover_probe_channel::domain::{BOUNCE_FLOOR_LEVELS, display_levels, max_abs_difference};
use crate::mover_probe_channel::states::{MidState, compact_rgb};
use crate::sh_bake::pack_octahedral_irradiance_tile;

/// L0-only coefficients: a tile constant over every direction.
fn dc(value: f32) -> [f32; 27] {
    let mut coefficients = [0.0; 27];
    coefficients[..3].copy_from_slice(&[value; 3]);
    coefficients
}

fn tile(coefficients: &[f32; 27]) -> Vec<OctahedralAtlasTexel> {
    pack_octahedral_irradiance_tile(
        coefficients,
        true,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    )
}

// A mid pose stores its own gathered state, packed as id 34 packs a tile,
// where it leaves id 34 by more than the round-trip bar and the perceptual
// floor, and reads the base where it stays within either.
#[test]
fn a_mid_pose_stores_its_gather_past_the_bars_and_reads_the_base_within() {
    let base = tile(&dc(0.5));
    let bright = dc(4.0);
    assert_eq!(
        classify(&bright, &base, 0.0),
        MidState::Stored(compact_rgb(&tile(&bright))),
        "past both bars: its own gather, packed"
    );
    assert_eq!(
        classify(&bright, &base, 10.0),
        MidState::Base,
        "within the round-trip bar"
    );
    let near = dc(0.505);
    assert!(
        max_abs_difference(&tile(&near), &base) > 0.0
            && display_levels(&tile(&near), &base) <= BOUNCE_FLOOR_LEVELS,
        "the near state leaves the f16 tile but not the floor"
    );
    assert_eq!(
        classify(&near, &base, 0.0),
        MidState::Base,
        "past the round-trip bar, within the perceptual floor"
    );
}

// A probe the mid pose encloses gathers zero radiance (every lifted ray
// starts in solid; the gather side is pinned by
// `an_enclosing_terminus_reads_the_base_and_an_enclosing_mid_pose_stores_its_dark_state`), and that
// state classifies like any other (Amendment 2, Ruling B): stored as the
// zero tile where id 34 is lit past the bars, the base where id 34 is
// already within them of zero. Nothing blends across.
#[test]
fn an_enclosed_mid_pose_stores_its_zero_bounce_past_the_bars_and_reads_a_dark_base_within() {
    let dark = [0.0; 27];
    let lit = tile(&dc(0.5));
    assert_eq!(
        classify(&dark, &lit, 0.0),
        MidState::Stored(compact_rgb(&tile(&dark))),
        "a lit base: the dark state is stored"
    );
    assert!(
        compact_rgb(&tile(&dark)).iter().all(|&half| half == 0),
        "the stored state is zero radiance"
    );
    assert_eq!(
        classify(&dark, &tile(&dark), 0.0),
        MidState::Base,
        "a base already dark: it reads the base"
    );
}
