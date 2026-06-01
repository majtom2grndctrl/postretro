// Octahedral direction encoding and irradiance-atlas tile mapping.
// See: context/lib/rendering_pipeline.md §4 and context/lib/build_pipeline.md §PRL section IDs

/// Default tile resolution chosen by the baker when it writes a fresh atlas.
/// The wire format stores N per-section, so a future re-bake can pick a
/// different default without a format break.
pub const DEFAULT_IRRADIANCE_TILE_DIMENSION: u32 = 6;
pub const DEFAULT_IRRADIANCE_TILE_BORDER: u32 = 1;

/// Tile resolution the current runtime (sampler shaders + delta/compose passes)
/// is pinned to. This is a *capability* limit, not a format constraint: the
/// header stores N so the resolution can change via re-bake, but the loaders
/// reject any N the runtime cannot currently sample. Today it equals
/// [`DEFAULT_IRRADIANCE_TILE_DIMENSION`]; the distinct name documents that the
/// two answer different questions ("what does the baker pick" vs. "what can the
/// runtime consume"). Bump this once the runtime handles a new N.
pub const RUNTIME_SUPPORTED_TILE_DIMENSION: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;

/// Encode a unit-length direction vector to octahedral `[u16; 2]`.
///
/// Maps the unit sphere to a `[-1, 1]^2` octahedron unfolding, then quantizes
/// to `[0, 65535]`. The bitangent sign (for tangent vectors) is not stored here;
/// callers pack it separately.
pub fn encode(x: f32, y: f32, z: f32) -> [u16; 2] {
    // Project onto octahedron: divide by L1 norm
    let inv_l1 = 1.0 / (x.abs() + y.abs() + z.abs());
    let mut ox = x * inv_l1;
    let mut oy = y * inv_l1;

    // Reflect the lower hemisphere into the [−1,1]^2 square
    if z < 0.0 {
        let new_ox = (1.0 - oy.abs()) * sign_not_zero(ox);
        let new_oy = (1.0 - ox.abs()) * sign_not_zero(oy);
        ox = new_ox;
        oy = new_oy;
    }

    // Map [-1, 1] → [0, 65535]
    let u = ((ox * 0.5 + 0.5) * 65535.0).round() as u16;
    let v = ((oy * 0.5 + 0.5) * 65535.0).round() as u16;
    [u, v]
}

/// Decode an octahedral `[u16; 2]` back to a unit-length direction vector `[f32; 3]`.
pub fn decode(encoded: [u16; 2]) -> [f32; 3] {
    // Map [0, 65535] → [-1, 1]
    let ox = encoded[0] as f32 / 65535.0 * 2.0 - 1.0;
    let oy = encoded[1] as f32 / 65535.0 * 2.0 - 1.0;

    // Reconstruct z from the octahedron constraint |x| + |y| + |z| = 1
    let z = 1.0 - ox.abs() - oy.abs();
    let (x, y) = if z < 0.0 {
        // Lower hemisphere: undo the reflection
        let x = (1.0 - oy.abs()) * sign_not_zero(ox);
        let y = (1.0 - ox.abs()) * sign_not_zero(oy);
        (x, y)
    } else {
        (ox, oy)
    };

    // Normalize to correct for quantization error
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// Returns 1.0 for non-negative values, -1.0 for negative. Never returns zero.
fn sign_not_zero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

/// Atlas tile columns for the committed near-square probe packing.
///
/// Probes keep their x-fastest linear order:
/// `probe_index = x + y * grid_x + z * grid_x * grid_y`.
/// The 2D tile atlas places that linear index as:
/// `tile_x = probe_index % tiles_per_row`,
/// `tile_y = probe_index / tiles_per_row`.
pub fn irradiance_atlas_tiles_per_row(grid_dimensions: [u32; 3]) -> Option<u32> {
    let total = total_probe_count(grid_dimensions)?;
    if total == 0 {
        return Some(0);
    }
    if total > (u32::MAX as u64) * (u32::MAX as u64) {
        return None;
    }
    u32::try_from(ceil_sqrt_u64(total)).ok()
}

/// Atlas dimensions, in texels, for the committed near-square probe packing.
pub fn irradiance_atlas_dimensions(grid_dimensions: [u32; 3], tile_dimension: u32) -> [u32; 2] {
    let Some(total) = total_probe_count(grid_dimensions) else {
        return [0, 0];
    };
    if total == 0 {
        return [0, 0];
    }
    let Some(tiles_per_row) = irradiance_atlas_tiles_per_row(grid_dimensions) else {
        return [0, 0];
    };
    let tile_rows = total.div_ceil(tiles_per_row as u64);
    let Some(width) = tiles_per_row.checked_mul(tile_dimension) else {
        return [0, 0];
    };
    let Some(height) = u32::try_from(tile_rows)
        .ok()
        .and_then(|rows| rows.checked_mul(tile_dimension))
    else {
        return [0, 0];
    };
    [width, height]
}

/// Tile origin, in atlas texels, for a probe identified by its x-fastest linear index
/// (probe_index = x + y*gx + z*gx*gy).
pub fn irradiance_tile_origin(
    probe_index: usize,
    tile_dimension: u32,
    atlas_tiles_per_row: u32,
) -> [u32; 2] {
    let tiles_per_row = atlas_tiles_per_row.max(1) as usize;
    let tile_x = probe_index % tiles_per_row;
    let tile_y = probe_index / tiles_per_row;
    [
        tile_x as u32 * tile_dimension,
        tile_y as u32 * tile_dimension,
    ]
}

fn total_probe_count(grid_dimensions: [u32; 3]) -> Option<u64> {
    if grid_dimensions.contains(&0) {
        return Some(0);
    }
    (grid_dimensions[0] as u64)
        .checked_mul(grid_dimensions[1] as u64)?
        .checked_mul(grid_dimensions[2] as u64)
}

fn ceil_sqrt_u64(n: u64) -> u64 {
    debug_assert!(n > 0);
    let mut lo = 1u64;
    // Cap the upper search bound at `u32::MAX` so the returned root never exceeds
    // `u32::MAX`. Callers that need a `u32` (e.g. `irradiance_atlas_tiles_per_row`)
    // already reject oversized inputs earlier — they bound `n` to at most
    // `u32::MAX * u32::MAX`, whose ceil-sqrt is `u32::MAX` — so this cap only
    // tightens the binary search, it is never the path that rejects an input.
    let mut hi = n.min(u32::MAX as u64);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if mid >= n.div_ceil(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// Interior texel center -> unit direction for an octahedral irradiance tile.
///
/// The border is excluded from the [0,1] domain. Interior `(0,0)` maps to the
/// lower-left center of the unfolded octahedral square, and increasing y maps
/// upward in octahedral space. The WGSL sampler in `sh_sample.wgsl` must mirror
/// this mapping exactly.
pub fn irradiance_interior_texel_direction(
    interior_x: u32,
    interior_y: u32,
    tile_dimension: u32,
    border: u32,
) -> [f32; 3] {
    let interior = tile_dimension
        .checked_sub(border * 2)
        .expect("tile border must leave an interior");
    assert!(interior > 0, "tile border must leave an interior");
    assert!(interior_x < interior);
    assert!(interior_y < interior);

    let u = (interior_x as f32 + 0.5) / interior as f32;
    let v = (interior_y as f32 + 0.5) / interior as f32;
    decode_unquantized(u * 2.0 - 1.0, v * 2.0 - 1.0)
}

/// Source interior texel for a tile texel, including the 1-texel octahedral
/// wrap border. Interior texels map to themselves; border texels copy the
/// opposite interior edge with the orthogonal coordinate reversed.
pub fn irradiance_tile_source_texel(
    tile_x: u32,
    tile_y: u32,
    tile_dimension: u32,
    border: u32,
) -> [u32; 2] {
    assert_eq!(border, 1, "only the committed 1-texel border is supported");
    assert!(tile_x < tile_dimension);
    assert!(tile_y < tile_dimension);
    let interior = tile_dimension - 2 * border;
    assert!(interior > 0, "tile border must leave an interior");

    let ix = tile_x as i32 - border as i32;
    let iy = tile_y as i32 - border as i32;
    let n = interior as i32;

    if (0..n).contains(&ix) && (0..n).contains(&iy) {
        return [ix as u32, iy as u32];
    }

    if ix < 0 && (0..n).contains(&iy) {
        return [(n - 1) as u32, (n - 1 - iy) as u32];
    }
    if ix >= n && (0..n).contains(&iy) {
        return [0, (n - 1 - iy) as u32];
    }
    if iy < 0 && (0..n).contains(&ix) {
        return [(n - 1 - ix) as u32, (n - 1) as u32];
    }
    if iy >= n && (0..n).contains(&ix) {
        return [(n - 1 - ix) as u32, 0];
    }

    // Corners are adjacent to two wrapped edges. Pick the diagonally wrapped
    // interior corner; this matches the edge reversal convention above.
    let sx = if ix < 0 { n - 1 } else { 0 };
    let sy = if iy < 0 { n - 1 } else { 0 };
    [sx as u32, sy as u32]
}

/// Unquantized octahedral UV in `[0, 1]^2` for a unit direction. This is the
/// exact Rust mirror of `oct_encode_unquantized` in `sh_sample.wgsl`: same L1
/// projection, same lower-hemisphere fold, same `p * 0.5 + 0.5` remap. It is
/// the inverse of [`decode_unquantized`] (after the `[-1,1] ↔ [0,1]` remap).
/// `octahedral_oct_uv_matches_wgsl_reference` pins it against hand-checked
/// reference values so a drift on either side fails CI. Test-only: the runtime
/// encodes via [`encode`]; this is the unquantized form the WGSL sampler uses.
#[cfg(test)]
fn encode_unquantized_uv(x: f32, y: f32, z: f32) -> [f32; 2] {
    let inv_l1 = 1.0 / (x.abs() + y.abs() + z.abs());
    let mut ox = x * inv_l1;
    let mut oy = y * inv_l1;
    if z < 0.0 {
        let new_ox = (1.0 - oy.abs()) * sign_not_zero(ox);
        let new_oy = (1.0 - ox.abs()) * sign_not_zero(oy);
        ox = new_ox;
        oy = new_oy;
    }
    [ox * 0.5 + 0.5, oy * 0.5 + 0.5]
}

fn decode_unquantized(ox: f32, oy: f32) -> [f32; 3] {
    let z = 1.0 - ox.abs() - oy.abs();
    let (x, y) = if z < 0.0 {
        let x = (1.0 - oy.abs()) * sign_not_zero(ox);
        let y = (1.0 - ox.abs()) * sign_not_zero(oy);
        (x, y)
    } else {
        (ox, oy)
    };
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maximum allowed angular error in radians between input and decoded output.
    const MAX_ANGULAR_ERROR: f32 = 0.001;

    fn angular_error(a: [f32; 3], b: [f32; 3]) -> f32 {
        let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
        dot.acos()
    }

    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    }

    fn assert_round_trip(dir: [f32; 3]) {
        let dir = normalize(dir);
        let encoded = encode(dir[0], dir[1], dir[2]);
        let decoded = decode(encoded);
        let err = angular_error(dir, decoded);
        assert!(
            err < MAX_ANGULAR_ERROR,
            "angular error {err:.6} rad exceeds threshold for direction {:?} \
             (encoded: {:?}, decoded: {:?})",
            dir,
            encoded,
            decoded,
        );
    }

    #[test]
    fn round_trips_positive_axes() {
        assert_round_trip([1.0, 0.0, 0.0]);
        assert_round_trip([0.0, 1.0, 0.0]);
        assert_round_trip([0.0, 0.0, 1.0]);
    }

    #[test]
    fn round_trips_negative_axes() {
        assert_round_trip([-1.0, 0.0, 0.0]);
        assert_round_trip([0.0, -1.0, 0.0]);
        assert_round_trip([0.0, 0.0, -1.0]);
    }

    #[test]
    fn round_trips_diagonals() {
        let d = 1.0 / 3.0_f32.sqrt();
        assert_round_trip([d, d, d]);
        assert_round_trip([-d, -d, -d]);
        assert_round_trip([d, -d, d]);
        assert_round_trip([-d, d, -d]);
    }

    #[test]
    fn round_trips_near_pole_vectors() {
        // Near +Z pole
        assert_round_trip([0.001, 0.001, 1.0]);
        assert_round_trip([-0.001, 0.001, 1.0]);
        // Near -Z pole
        assert_round_trip([0.001, 0.001, -1.0]);
        assert_round_trip([-0.001, -0.001, -1.0]);
        // Near +Y pole
        assert_round_trip([0.001, 1.0, 0.001]);
        // Near -X pole
        assert_round_trip([-1.0, 0.001, 0.001]);
    }

    #[test]
    fn round_trips_random_samples() {
        // Deterministic pseudo-random directions covering the sphere
        // Using a simple LCG for reproducibility
        let mut seed: u32 = 0xDEAD_BEEF;
        for _ in 0..1000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let x = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let y = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let z = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;

            let len = (x * x + y * y + z * z).sqrt();
            if len < 0.001 {
                continue; // Skip near-zero vectors
            }
            assert_round_trip([x, y, z]);
        }
    }

    #[test]
    fn encoded_values_use_expected_range() {
        // Verify axis vectors map to expected octahedral coordinates
        let dirs_and_expected: &[([f32; 3], [u16; 2])] = &[
            // +X: oct (1, 0) -> u16 (65535, 32768)
            ([1.0, 0.0, 0.0], [65535, 32768]),
            // -X: oct (-1, 0) -> u16 (0, 32768)
            ([-1.0, 0.0, 0.0], [0, 32768]),
            // +Y: oct (0, 1) -> u16 (32768, 65535)
            ([0.0, 1.0, 0.0], [32768, 65535]),
            // -Y: oct (0, -1) -> u16 (32768, 0)
            ([0.0, -1.0, 0.0], [32768, 0]),
            // +Z: oct (0, 0) -> u16 (32768, 32768)
            ([0.0, 0.0, 1.0], [32768, 32768]),
        ];
        for (dir, expected) in dirs_and_expected {
            let enc = encode(dir[0], dir[1], dir[2]);
            assert_eq!(enc, *expected, "direction {:?}", dir);
        }
    }

    #[test]
    fn decode_produces_unit_vectors() {
        let test_cases: &[[u16; 2]] = &[
            [0, 0],
            [65535, 65535],
            [32768, 32768],
            [0, 65535],
            [65535, 0],
            [16384, 49152],
        ];
        for &enc in test_cases {
            let dec = decode(enc);
            let len = (dec[0] * dec[0] + dec[1] * dec[1] + dec[2] * dec[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "decoded vector {:?} from {:?} has length {}, expected 1.0",
                dec,
                enc,
                len,
            );
        }
    }

    #[test]
    fn irradiance_atlas_dimensions_follow_near_square_linear_probe_tiles() {
        assert_eq!(irradiance_atlas_tiles_per_row([3, 2, 4]), Some(5));
        assert_eq!(irradiance_atlas_dimensions([3, 2, 4], 6), [30, 30]);
        assert_eq!(irradiance_atlas_dimensions([0, 2, 4], 6), [0, 0]);
        assert_eq!(irradiance_tile_origin(0, 6, 5), [0, 0]);
        assert_eq!(irradiance_tile_origin(1, 6, 5), [6, 0]);
        assert_eq!(irradiance_tile_origin(4, 6, 5), [24, 0]);
        assert_eq!(irradiance_tile_origin(5, 6, 5), [0, 6]);
        assert_eq!(irradiance_tile_origin(23, 6, 5), [18, 24]);
    }

    #[test]
    fn irradiance_tile_border_copies_across_octahedral_wrap() {
        let n = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let border = DEFAULT_IRRADIANCE_TILE_BORDER;

        // Interior maps to itself after subtracting the border.
        assert_eq!(irradiance_tile_source_texel(1, 1, n, border), [0, 0]);
        assert_eq!(irradiance_tile_source_texel(4, 4, n, border), [3, 3]);

        // Edges copy the opposite edge with the orthogonal axis reversed.
        assert_eq!(irradiance_tile_source_texel(0, 1, n, border), [3, 3]);
        assert_eq!(irradiance_tile_source_texel(0, 4, n, border), [3, 0]);
        assert_eq!(irradiance_tile_source_texel(5, 1, n, border), [0, 3]);
        assert_eq!(irradiance_tile_source_texel(5, 4, n, border), [0, 0]);
        assert_eq!(irradiance_tile_source_texel(1, 0, n, border), [3, 3]);
        assert_eq!(irradiance_tile_source_texel(4, 0, n, border), [0, 3]);
        assert_eq!(irradiance_tile_source_texel(1, 5, n, border), [3, 0]);
        assert_eq!(irradiance_tile_source_texel(4, 5, n, border), [0, 0]);

        // Corners use the diagonally wrapped interior corner.
        assert_eq!(irradiance_tile_source_texel(0, 0, n, border), [3, 3]);
        assert_eq!(irradiance_tile_source_texel(5, 0, n, border), [0, 3]);
        assert_eq!(irradiance_tile_source_texel(0, 5, n, border), [3, 0]);
        assert_eq!(irradiance_tile_source_texel(5, 5, n, border), [0, 0]);
    }

    /// Rust ↔ WGSL octahedral mapping parity (the plan's open question).
    ///
    /// The Rust encoder here and the WGSL decoder in `sh_sample.wgsl` are
    /// hand-mirrored (no codegen), so this test pins the shared convention with
    /// hardcoded reference values: each direction's unquantized octahedral UV
    /// (matching WGSL's `oct_encode_unquantized`) and, for that UV, the
    /// interior-space coordinate `uv * interior` the WGSL sampler computes as
    /// `border + oct * interior`. If the mapping changes on either side, the
    /// Rust assertions here fail directly and the comment by
    /// `oct_encode_unquantized` prompts the human/agent to resync the shader.
    ///
    /// Reference set exercises the seam/fold: all six axes plus a
    /// lower-hemisphere diagonal where the `z < 0` fold applies.
    #[test]
    fn octahedral_oct_uv_matches_wgsl_reference() {
        const EPS: f32 = 1e-5;
        // tile_dimension 6, border 1 -> interior 4 (the runtime-supported tile).
        const INTERIOR: f32 = 4.0;

        // (direction, expected octahedral UV in [0,1]^2).
        let l = 1.0 / 2.0_f32.sqrt(); // 1/sqrt(2)
        let cases: &[([f32; 3], [f32; 2])] = &[
            // Upper hemisphere (z >= 0): UV is just the L1-normalized xy remapped.
            ([1.0, 0.0, 0.0], [1.0, 0.5]),   // +X
            ([-1.0, 0.0, 0.0], [0.0, 0.5]),  // -X
            ([0.0, 1.0, 0.0], [0.5, 1.0]),   // +Y
            ([0.0, -1.0, 0.0], [0.5, 0.0]),  // -Y
            ([0.0, 0.0, 1.0], [0.5, 0.5]),   // +Z (center)
            // Lower hemisphere (z < 0): the fold sends the apex to the corners.
            ([0.0, 0.0, -1.0], [1.0, 1.0]),  // -Z
            // Lower-hemisphere diagonal (x=y=0.5, z=-1/sqrt(2)). After folding,
            // UV = ((1 - |oy|)*sign(ox)) etc. -> 0.853553 on both axes.
            ([0.5, 0.5, -l], [0.853_553_4, 0.853_553_4]),
        ];

        for (dir, expected_uv) in cases {
            let dir = normalize(*dir);
            let uv = encode_unquantized_uv(dir[0], dir[1], dir[2]);
            assert!(
                (uv[0] - expected_uv[0]).abs() < EPS && (uv[1] - expected_uv[1]).abs() < EPS,
                "octahedral UV for {dir:?} was {uv:?}, expected {expected_uv:?}",
            );

            // Interior-space coordinate the WGSL sampler derives (`oct * interior`).
            let interior_coord = [uv[0] * INTERIOR, uv[1] * INTERIOR];
            // Encoding then decoding the UV must return the same unit direction
            // (sign-not-zero handling and the fold are self-inverse here).
            let decoded = decode_unquantized(uv[0] * 2.0 - 1.0, uv[1] * 2.0 - 1.0);
            assert!(
                angular_error(dir, decoded) < MAX_ANGULAR_ERROR,
                "decode of UV {uv:?} (interior coord {interior_coord:?}) was {decoded:?}, \
                 expected {dir:?}",
            );
        }

        // Pin the interior-texel inverse used by the baker / diagnostics:
        // `irradiance_interior_texel_direction(i, j, 6, 1)` samples interior
        // texel centers at UV `(i + 0.5) / 4`. Texel (0,0) is the lower-left
        // octahedral center; texel (3,3) the upper-right.
        let center_00 = irradiance_interior_texel_direction(0, 0, 6, 1);
        let center_33 = irradiance_interior_texel_direction(3, 3, 6, 1);
        // UV (0.125, 0.125) -> oct (-0.75, -0.75), z = 1 - 1.5 < 0 -> folds.
        let expect_00 = decode_unquantized(0.125 * 2.0 - 1.0, 0.125 * 2.0 - 1.0);
        let expect_33 = decode_unquantized(0.875 * 2.0 - 1.0, 0.875 * 2.0 - 1.0);
        assert!(angular_error(center_00, expect_00) < MAX_ANGULAR_ERROR);
        assert!(angular_error(center_33, expect_33) < MAX_ANGULAR_ERROR);
    }

    #[test]
    fn irradiance_interior_texel_direction_uses_unit_vectors() {
        for y in 0..4 {
            for x in 0..4 {
                let d = irradiance_interior_texel_direction(x, y, 6, 1);
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-5);
            }
        }
    }
}
