// What a mid pose holds for a near-field probe from its own posed gather:
// its state where that clears the probe's bars against id 34, the base where
// it stays within them, and nothing, so it blends across, where the pose
// encloses the probe.

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
// floor; reads the base where it stays within either; and stores nothing,
// so it blends across, where it encloses the probe (AC 12), whatever its
// gather read.
#[test]
fn a_mid_pose_stores_its_gather_past_the_bars_reads_the_base_within_and_blends_across_enclosed() {
    let base = tile(&dc(0.5));
    let bright = dc(4.0);
    assert_eq!(
        classify(&bright, false, &base, 0.0),
        Some(MidState::Stored(compact_rgb(&tile(&bright)))),
        "past both bars: its own gather, packed"
    );
    assert_eq!(
        classify(&bright, false, &base, 10.0),
        Some(MidState::Base),
        "within the round-trip bar"
    );
    let near = dc(0.505);
    assert!(
        max_abs_difference(&tile(&near), &base) > 0.0
            && display_levels(&tile(&near), &base) <= BOUNCE_FLOOR_LEVELS,
        "the near state leaves the f16 tile but not the floor"
    );
    assert_eq!(
        classify(&near, false, &base, 0.0),
        Some(MidState::Base),
        "past the round-trip bar, within the perceptual floor"
    );
    assert_eq!(
        classify(&bright, true, &base, 0.0),
        None,
        "enclosed: blends across"
    );
}
