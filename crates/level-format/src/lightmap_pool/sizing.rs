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

/// One pool's first generation: its compiled `bound` times the margin, capped
/// at what the level needs `all_resident`. A pool with anything to hold takes
/// at least one layer even at a bound of 0, so band pairs can land: a level
/// can hold data no camera cell mandates. A pool with nothing to hold has none.
/// Shelf packing is not monotone, so a cell's set can pack worse than the whole
/// level: the cap never drops the pool below the bound.
pub fn first_generation_layers(bound: u32, all_resident: u32) -> u32 {
    if all_resident == 0 {
        return 0;
    }
    LIGHTMAP_POOL_MARGIN.of(bound).min(all_resident.max(bound)).max(1)
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
        assert_eq!(first_generation_layers(12, 10), 12);
        assert_eq!(first_generation_layers(8, 9), 9);
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
        assert_eq!(first_generation_layers(8, 100), 10, "exact");
        assert_eq!(first_generation_layers(27, 100), 34, "33.75 rounds up");
        assert_eq!(first_generation_layers(1, 100), 2, "1.25 rounds up");
    }

    #[test]
    fn first_generation_stops_at_the_all_resident_layers() {
        assert_eq!(
            first_generation_layers(10, 13),
            13,
            "12.5 rounds up to the cap"
        );
        assert_eq!(first_generation_layers(27, 30), 30);
        assert_eq!(first_generation_layers(30, 30), 30);
    }

    // A level can hold blocks no camera cell mandates: its pool still needs a
    // layer for band pairs to land in.
    #[test]
    fn first_generation_floors_a_zero_bound_at_one_layer_only_with_something_to_hold() {
        assert_eq!(first_generation_layers(0, 5), 1);
        assert_eq!(first_generation_layers(0, 1), 1);
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
