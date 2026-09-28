// Shared sizing, paging and budget rules for the compact animated-lightmap atlas.
// PRL lightmap pipeline: `context/lib/build_pipeline.md`.

/// Bytes occupied by one animated-lightmap texel: `Rgba16Float` irradiance
/// (8 bytes) plus `Rgba8Unorm` dominant direction (4 bytes).
pub const ANIMATED_ATLAS_BYTES_PER_TEXEL: u64 = 12;

/// Maximum VRAM reserved for the animated-lightmap irradiance and direction
/// atlas pair. The compiler rejects an over-budget bake; the renderer uses the
/// same limit to degrade safely when loading externally-produced content.
pub const ANIMATED_ATLAS_VRAM_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

/// Smallest page side the compiler picks, unless the static lightmap layer is
/// itself smaller. Keeps page quantization from fragmenting into many tiny
/// pages while staying a natural load-and-evict unit.
pub const ANIMATED_PAGE_MIN_SIZE: u32 = 1024;

/// Uniform binding size the renderer requests for the forward pass's group-4
/// binding-7 block table: wgpu's default `max_uniform_buffer_binding_size`.
pub const ANIMATED_BLOCK_TABLE_UNIFORM_BYTES: u32 = 64 << 10;

/// Block-table header: static layer size, page size, block count, padding.
pub const ANIMATED_BLOCK_TABLE_HEADER_BYTES: u32 = 16;

/// One block-table entry: packed `(i16 dx, i16 dy)` static→compact texel
/// offset, then the page. Two entries share one 16-byte uniform array element.
pub const ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK: u32 = 8;

/// The one block cap the compiler enforces and the forward shader's block
/// table holds. A vertex names block `n` as `n + 1` in a `u16`, so the cap
/// must stay below `u16::MAX`.
pub const ANIMATED_BLOCK_CAP: u32 = (ANIMATED_BLOCK_TABLE_UNIFORM_BYTES
    - ANIMATED_BLOCK_TABLE_HEADER_BYTES)
    / ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK;

const _: () = assert!(ANIMATED_BLOCK_CAP < u16::MAX as u32);

/// Return the combined byte requirement for an animated atlas of `layers`
/// array layers. Arithmetic is widened before multiplication because the
/// supported dimensions and layer ceiling exceed `u32`.
pub fn animated_atlas_byte_estimate(width: u32, height: u32, layers: u32) -> u64 {
    u64::from(width) * u64::from(height) * u64::from(layers) * ANIMATED_ATLAS_BYTES_PER_TEXEL
}

/// Whether an animated atlas fits `budget_bytes`.
pub fn animated_atlas_fits_budget(width: u32, height: u32, layers: u32, budget_bytes: u64) -> bool {
    animated_atlas_byte_estimate(width, height, layers) <= budget_bytes
}

/// Smallest legal page side: at least the largest block side, and at least
/// [`ANIMATED_PAGE_MIN_SIZE`] unless the static layer is smaller. `None` for
/// the static layer skips that clause (placeholder static lightmap).
pub fn animated_page_size_lower_bound(
    largest_block_side: u32,
    static_layer_size: Option<u32>,
) -> u32 {
    let floor = static_layer_size.map_or(0, |size| size.min(ANIMATED_PAGE_MIN_SIZE));
    largest_block_side.max(floor).max(1)
}

/// Check a page size against the paging rules: a power of two, at least
/// [`animated_page_size_lower_bound`], and at most the static layer size. With
/// a placeholder static lightmap (`None`) only the power-of-two and
/// largest-block rules apply; that level takes the no-animated-light path.
pub fn validate_animated_page_size(
    page_size: u32,
    largest_block_side: u32,
    static_layer_size: Option<u32>,
) -> Result<(), String> {
    if !page_size.is_power_of_two() {
        return Err(format!(
            "animated atlas page size {page_size} is not a power of two"
        ));
    }
    let lower = animated_page_size_lower_bound(largest_block_side, static_layer_size);
    if page_size < lower {
        return Err(format!(
            "animated atlas page size {page_size} is below the lower bound {lower} \
             (largest block side {largest_block_side})"
        ));
    }
    if let Some(upper) = static_layer_size
        && page_size > upper
    {
        return Err(format!(
            "animated atlas page size {page_size} exceeds the static lightmap layer size {upper}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_estimate_scales_with_layers_without_u32_overflow() {
        let one_layer = animated_atlas_byte_estimate(8_192, 8_192, 1);
        let many_layers = animated_atlas_byte_estimate(8_192, 8_192, 256);

        assert_eq!(many_layers, one_layer * 256);
        assert!(many_layers > u64::from(u32::MAX));
        assert!(!animated_atlas_fits_budget(
            8_192,
            8_192,
            256,
            ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
        ));
    }

    #[test]
    fn block_cap_fills_the_requested_uniform_and_fits_a_u16_vertex_id() {
        let table_bytes = ANIMATED_BLOCK_TABLE_HEADER_BYTES
            + ANIMATED_BLOCK_CAP * ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK;
        assert!(table_bytes <= ANIMATED_BLOCK_TABLE_UNIFORM_BYTES);
        assert!(
            table_bytes + ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK > ANIMATED_BLOCK_TABLE_UNIFORM_BYTES,
            "the cap is the largest block count the uniform holds"
        );
        assert!(ANIMATED_BLOCK_CAP < u32::from(u16::MAX));
    }

    #[test]
    fn page_size_bounds_follow_the_static_layer_and_largest_block() {
        assert!(validate_animated_page_size(1024, 645, Some(2048)).is_ok());
        assert!(validate_animated_page_size(2048, 645, Some(2048)).is_ok());
        assert!(validate_animated_page_size(1000, 645, Some(2048)).is_err());
        assert!(validate_animated_page_size(512, 300, Some(2048)).is_err());
        assert!(validate_animated_page_size(4096, 645, Some(2048)).is_err());
        assert!(validate_animated_page_size(1024, 1025, Some(2048)).is_err());
        // A static layer below the minimum lowers the floor to its own size.
        assert!(validate_animated_page_size(128, 78, Some(128)).is_ok());
        assert!(validate_animated_page_size(64, 60, Some(128)).is_err());
    }

    #[test]
    fn placeholder_static_layer_skips_the_static_bounds() {
        assert!(validate_animated_page_size(8192, 645, None).is_ok());
        assert!(validate_animated_page_size(1024, 645, None).is_ok());
        assert!(validate_animated_page_size(512, 645, None).is_err());
        assert!(validate_animated_page_size(1000, 645, None).is_err());
    }
}
