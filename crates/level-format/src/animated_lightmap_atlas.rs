// Shared sizing and budget rules for the runtime animated-lightmap atlas.
// See: context/plans/in-progress/animated-lightmap-array-atlas/index.md

/// Bytes occupied by one animated-lightmap texel: `Rgba16Float` irradiance
/// (8 bytes) plus `Rgba8Unorm` dominant direction (4 bytes).
pub const ANIMATED_ATLAS_BYTES_PER_TEXEL: u64 = 12;

/// Maximum VRAM reserved for the animated-lightmap irradiance and direction
/// atlas pair. The compiler rejects an over-budget bake; the renderer uses the
/// same limit to degrade safely when loading externally-produced content.
pub const ANIMATED_ATLAS_VRAM_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

/// Return the combined byte requirement for an animated atlas with one array
/// layer per animated slot. Arithmetic is widened before multiplication because
/// the supported static-atlas dimensions and layer ceiling exceed `u32`.
pub fn animated_atlas_byte_estimate(width: u32, height: u32, slot_count: u32) -> u64 {
    u64::from(width) * u64::from(height) * u64::from(slot_count) * ANIMATED_ATLAS_BYTES_PER_TEXEL
}

/// Whether an animated atlas fits `budget_bytes`.
pub fn animated_atlas_fits_budget(
    width: u32,
    height: u32,
    slot_count: u32,
    budget_bytes: u64,
) -> bool {
    animated_atlas_byte_estimate(width, height, slot_count) <= budget_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_estimate_scales_with_slots_without_u32_overflow() {
        let one_slot = animated_atlas_byte_estimate(8_192, 8_192, 1);
        let many_slots = animated_atlas_byte_estimate(8_192, 8_192, 256);

        assert_eq!(many_slots, one_slot * 256);
        assert!(many_slots > u64::from(u32::MAX));
        assert!(!animated_atlas_fits_budget(
            8_192,
            8_192,
            256,
            ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
        ));
    }
}
