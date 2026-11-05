// What the streamed SH pools charge per dense slot and sparse unit. One source
// so the compiler prices the mandatory bound as the renderer sizes the pool.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};

/// One 8×8 physical tile of RGBA16F: a composed atlas's cell per slot.
pub const SH_DENSE_COMPOSED_CELL_BYTES: u64 = 8 * 8 * 8;
/// One 8×8 physical tile of BC6H: four 4×4 blocks of 16 bytes.
const SH_DENSE_BC6H_CELL_BYTES: u64 = 64;
/// One compacted sparse entry.
pub const SH_SPARSE_ENTRY_BYTES: u64 = 8;
/// One word of the sparse tile pool, holding a pair of f16 values.
pub const SH_SPARSE_F16_WORD_BYTES: u64 = 4;

/// The dense pool's texture set, which varies by level: every slot pays once
/// for each texture the level's SH sections allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShDenseSlotTextures {
    /// Id-34 atlas format.
    pub indirect_format: u32,
    /// Id-35 atlas format, when the level has one.
    pub direct_format: Option<u32>,
    /// Id 41 or id 45 present: direct compose keeps an intermediate atlas.
    pub direct_compose: bool,
    /// Id 45 present: animated direct compose keeps its own atlas.
    pub animated_direct_compose: bool,
}

/// Bytes of one dense slot across its level's textures. `None` for an atlas
/// format the pool cannot allocate.
pub fn sh_dense_slot_bytes(textures: ShDenseSlotTextures) -> Option<u64> {
    let mut bytes = sh_atlas_cell_bytes(textures.indirect_format)? + SH_DENSE_COMPOSED_CELL_BYTES;
    if let Some(direct_format) = textures.direct_format {
        bytes += sh_atlas_cell_bytes(direct_format)?;
    }
    if textures.direct_compose {
        bytes += SH_DENSE_COMPOSED_CELL_BYTES;
    }
    if textures.animated_direct_compose {
        bytes += SH_DENSE_COMPOSED_CELL_BYTES;
    }
    Some(bytes)
}

/// Bytes of a sparse family's pools at `entries` and `tile_f16` capacity. The
/// tile pool rounds up to whole words.
pub fn sh_sparse_capacity_bytes(entries: u32, tile_f16: u32) -> u64 {
    u64::from(entries) * SH_SPARSE_ENTRY_BYTES
        + u64::from(tile_f16.div_ceil(2)) * SH_SPARSE_F16_WORD_BYTES
}

fn sh_atlas_cell_bytes(format: u32) -> Option<u64> {
    match format {
        IRRADIANCE_FORMAT_BC6H => Some(SH_DENSE_BC6H_CELL_BYTES),
        IRRADIANCE_FORMAT_RGBA16F => Some(SH_DENSE_COMPOSED_CELL_BYTES),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The per-slot figure research.md measured on the hallway, whose texture
    // set is every one the pool can allocate.
    #[test]
    fn dense_slot_bytes_prices_every_texture_a_level_allocates() {
        let full = ShDenseSlotTextures {
            indirect_format: IRRADIANCE_FORMAT_BC6H,
            direct_format: Some(IRRADIANCE_FORMAT_BC6H),
            direct_compose: true,
            animated_direct_compose: true,
        };
        assert_eq!(sh_dense_slot_bytes(full), Some(1664));
    }

    #[test]
    fn dense_slot_bytes_charges_only_the_textures_a_level_has() {
        let indirect_only = ShDenseSlotTextures {
            indirect_format: IRRADIANCE_FORMAT_RGBA16F,
            direct_format: None,
            direct_compose: false,
            animated_direct_compose: false,
        };
        assert_eq!(sh_dense_slot_bytes(indirect_only), Some(1024));
        assert_eq!(
            sh_dense_slot_bytes(ShDenseSlotTextures {
                direct_format: Some(IRRADIANCE_FORMAT_BC6H),
                direct_compose: true,
                ..indirect_only
            }),
            Some(1024 + 64 + 512)
        );
    }

    #[test]
    fn dense_slot_bytes_refuses_an_unknown_atlas_format() {
        let unknown = ShDenseSlotTextures {
            indirect_format: u32::MAX,
            direct_format: None,
            direct_compose: false,
            animated_direct_compose: false,
        };
        assert_eq!(sh_dense_slot_bytes(unknown), None);
        assert_eq!(
            sh_dense_slot_bytes(ShDenseSlotTextures {
                indirect_format: IRRADIANCE_FORMAT_BC6H,
                direct_format: Some(u32::MAX),
                ..unknown
            }),
            None
        );
    }

    #[test]
    fn sparse_capacity_rounds_tiles_up_to_whole_words() {
        assert_eq!(sh_sparse_capacity_bytes(3, 4), 3 * 8 + 2 * 4);
        assert_eq!(sh_sparse_capacity_bytes(3, 5), 3 * 8 + 3 * 4);
    }
}
