// What a lightmap or mask pool's layers cost, and how many it starts with. One
// source, so the compiler reports the bytes the runtime allocates.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use crate::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapHeader};

/// What a pool holds over the demand it was sized from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolMargin {
    pub numerator: u32,
    pub denominator: u32,
}

impl PoolMargin {
    /// `layers × self`, rounded up.
    pub fn of(self, layers: u32) -> u32 {
        let scaled =
            (u64::from(layers) * u64::from(self.numerator)).div_ceil(u64::from(self.denominator));
        u32::try_from(scaled).unwrap_or(u32::MAX)
    }
}

/// Holds drawn demand the bound does not model, and keeps band blocks.
pub const LIGHTMAP_POOL_MARGIN: PoolMargin = PoolMargin {
    numerator: 5,
    denominator: 4,
};

/// The fewest layers a pool's first generation takes, short of its cap. A
/// small level's bound leaves a margin of a layer or two, and its walks miss
/// where a pool at the cap would not: campaign-test's completed far-point tour
/// has 8 late blocks at its 10 margin layers and none at its 14 capped ones.
/// Below this floor a pool takes its cap, at most 17 × 6 + 17 × 4 = 170 MiB
/// with the spares, well under the budget warning. Set by late blocks measured
/// on camera walks, never by the budget (`context/lib/rendering_pipeline.md`
/// §4, Pool size).
pub const FIRST_GENERATION_FLOOR_LAYERS: u32 = 16;

/// One pool's first generation: its compiled `bound` times the margin, raised
/// to [`FIRST_GENERATION_FLOOR_LAYERS`], capped at what the level needs
/// `all_resident` times the same margin. The cap carries the margin because the
/// all-resident count is a whole-level pack from empty that the runtime's
/// incremental placement cannot reach: at exactly that count a pool fragments
/// and refuses band pairs, and campaign-test's realtime far-point tour rises
/// from 43 late blocks to 47. A pool with anything to hold therefore takes at
/// least two layers even at a bound of 0, so band pairs can land: a level can
/// hold data no camera cell mandates. A pool with nothing to hold has none.
/// Shelf packing is not monotone, so a cell's set can pack worse than the
/// whole level: the cap never drops the pool below the bound.
pub fn first_generation_layers(bound: u32, all_resident: u32) -> u32 {
    if all_resident == 0 {
        return 0;
    }
    LIGHTMAP_POOL_MARGIN
        .of(bound)
        .max(FIRST_GENERATION_FLOOR_LAYERS)
        .min(LIGHTMAP_POOL_MARGIN.of(all_resident).max(bound))
}

/// One mask pool layer: a BC5 plane at the pool edge.
pub const MASK_POOL_LAYER_BYTES: u64 =
    (LIGHTMAP_POOL_LAYER_EDGE as u64 / 4) * (LIGHTMAP_POOL_LAYER_EDGE as u64 / 4) * 16;

/// One lightmap pool layer: irradiance and direction at the pool edge, in the
/// level's own formats. `None` for a format no pool can allocate.
pub fn lightmap_pool_layer_bytes(header: &LightmapHeader) -> Option<u64> {
    let edge = LIGHTMAP_POOL_LAYER_EDGE;
    header
        .irradiance_len(edge, edge)?
        .checked_add(header.direction_len(edge, edge)?)
}

/// A streamed pool's bytes: its layers and the one spare a repack stages
/// through. A pool with no layers is no pool, so it has no spare.
pub fn streamed_pool_bytes(layers: u32, layer_bytes: u64) -> u64 {
    match layers {
        0 => 0,
        _ => (u64::from(layers) + 1) * layer_bytes,
    }
}

/// An all-resident pool's bytes: it never repacks, so it has no spare.
pub fn all_resident_pool_bytes(layers: u32, layer_bytes: u64) -> u64 {
    u64::from(layers) * layer_bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapMode};
    use crate::shadowmask_atlas::group_plane_len;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn the_cap_never_drops_a_pool_below_its_bound() {
        assert_eq!(first_generation_layers(20, 10), 20, "past the cap of 13");
        assert_eq!(first_generation_layers(14, 11), 14, "at the cap of 14");
    }

    fn header(irradiance_format: u32, direction_texel_scale: u32) -> LightmapHeader {
        LightmapHeader {
            block_count: 1,
            direction_texel_scale,
            irradiance_format,
            mode: LightmapMode::Shadowed,
        }
    }

    #[test]
    fn first_generation_applies_the_margin_rounded_up() {
        assert_eq!(first_generation_layers(80, 1000), 100, "exact");
        assert_eq!(first_generation_layers(181, 1000), 227, "226.25 rounds up");
        assert_eq!(first_generation_layers(74, 1000), 93, "92.5 rounds up");
    }

    #[test]
    fn first_generation_stops_at_the_all_resident_layers_times_the_margin() {
        assert_eq!(
            first_generation_layers(100, 90),
            113,
            "125 stops at the cap, 112.5 rounded up"
        );
        assert_eq!(first_generation_layers(100, 113), 125, "under the cap");
        assert_eq!(first_generation_layers(27, 24), 30);
        assert_eq!(first_generation_layers(30, 30), 38);
    }

    // A small bound's margin is a layer or two: the floor raises the pool to
    // its cap instead, and never past it.
    #[test]
    fn first_generation_rises_to_the_floor_short_of_the_cap() {
        let floor = FIRST_GENERATION_FLOOR_LAYERS;
        assert_eq!(
            first_generation_layers(8, 11),
            14,
            "campaign-test's lightmap pool: 11 × 5/4 rounded up"
        );
        assert_eq!(
            first_generation_layers(7, 7),
            9,
            "campaign-test's mask pool: 8.75 rounds up"
        );
        assert_eq!(
            first_generation_layers(8, 100),
            floor,
            "10 rises to the floor"
        );
        assert_eq!(first_generation_layers(1, 100), floor);
        assert_eq!(
            first_generation_layers(13, 100),
            17,
            "a margin past the floor stands"
        );
        assert_eq!(first_generation_layers(1, 12), 15, "a cap under the floor");
        assert_eq!(first_generation_layers(1, 13), floor, "a cap past it");
    }

    // A level can hold blocks no camera cell mandates: its pool still needs
    // room for band pairs to land in.
    #[test]
    fn first_generation_of_a_zero_bound_takes_the_floor_only_with_something_to_hold() {
        assert_eq!(first_generation_layers(0, 5), 7);
        assert_eq!(first_generation_layers(0, 1), 2);
        assert_eq!(
            first_generation_layers(0, 100),
            FIRST_GENERATION_FLOOR_LAYERS
        );
        assert_eq!(first_generation_layers(0, 0), 0);
        assert_eq!(first_generation_layers(3, 0), 0, "nothing to hold");
    }

    #[test]
    fn margin_never_overflows_at_the_top_of_the_range() {
        assert_eq!(LIGHTMAP_POOL_MARGIN.of(u32::MAX), u32::MAX);
    }

    #[test]
    fn pool_layers_price_at_the_shipped_formats() {
        assert_eq!(
            lightmap_pool_layer_bytes(&header(IRRADIANCE_FORMAT_BC6H, 2)),
            Some(6 * MIB),
            "BC6H 4 MiB plus Rg8 2 MiB"
        );
        assert_eq!(MASK_POOL_LAYER_BYTES, 4 * MIB, "one 2048² BC5 plane");
        assert_eq!(
            group_plane_len(LIGHTMAP_POOL_LAYER_EDGE, LIGHTMAP_POOL_LAYER_EDGE),
            Some(MASK_POOL_LAYER_BYTES)
        );
    }

    #[test]
    fn lightmap_layer_price_follows_the_levels_own_formats() {
        assert_eq!(
            lightmap_pool_layer_bytes(&header(IRRADIANCE_FORMAT_BC6H, 1)),
            Some(12 * MIB),
            "full-resolution direction"
        );
        assert_eq!(
            lightmap_pool_layer_bytes(&header(IRRADIANCE_FORMAT_RGBA16F, 2)),
            Some(34 * MIB),
            "uncompressed irradiance"
        );
        assert_eq!(lightmap_pool_layer_bytes(&header(99, 2)), None);
    }

    #[test]
    fn a_streamed_pool_pays_for_one_spare_and_an_all_resident_pool_for_none() {
        assert_eq!(streamed_pool_bytes(34, 6 * MIB), 35 * 6 * MIB);
        assert_eq!(streamed_pool_bytes(15, MASK_POOL_LAYER_BYTES), 16 * 4 * MIB);
        assert_eq!(all_resident_pool_bytes(13, 6 * MIB), 13 * 6 * MIB);
    }

    // A level with id 42 and no present plane has no mask pool.
    #[test]
    fn a_pool_with_no_layers_costs_nothing_and_carries_no_spare() {
        assert_eq!(streamed_pool_bytes(0, MASK_POOL_LAYER_BYTES), 0);
        assert_eq!(all_resident_pool_bytes(0, MASK_POOL_LAYER_BYTES), 0);
    }
}
