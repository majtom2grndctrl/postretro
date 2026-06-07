// DirectShVolume section (id 35): baked static-direct octahedral irradiance for dynamic objects.
// See: context/lib/build_pipeline.md

use crate::FormatError;
use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, RUNTIME_SUPPORTED_TILE_DIMENSION, irradiance_atlas_dimensions,
    irradiance_atlas_tiles_per_row,
};

/// Section-internal version written as the first u32 of the `DirectShVolume`
/// section payload. Bumped any time the on-disk layout changes so the loader can
/// reject stale `.prl` files with a clear error rather than silently misread
/// them. Starts at 1 — this section is new and shares no history with the
/// indirect `OctahedralShVolume` section's version line.
pub const DIRECT_SH_VOLUME_VERSION: u32 = 1;

/// Direct-light octahedral irradiance volume section (ID 35).
///
/// Carries a dense per-probe octahedral irradiance atlas holding DIRECT light
/// from STATIC lights, sampled at runtime by dynamic objects (entities and
/// billboards). It is a sibling to [`crate::sh_volume::OctahedralShVolumeSection`]
/// (the INDIRECT atlas) and mirrors it EXACTLY except:
///   - it carries direct (not indirect) coefficients, and
///   - it has NO depth moments and NO animation data.
///
/// Direct light is static and dense, so no animation table is needed. Depth
/// moments and per-probe validity are NOT duplicated here: the direct probe grid
/// is byte-identical in position to the indirect grid, so the runtime reads them
/// from the existing `OctahedralShVolumeSection`. This section still carries its
/// own grid header (byte-identical to the indirect grid) for round-trip and
/// validation.
///
/// The per-probe tile geometry (`tile_dimension` / `tile_border`,
/// `atlas_dimensions` / `atlas_tiles_per_row` derivation) and the dense
/// x-fastest linear probe order are IDENTICAL to the indirect atlas so the
/// runtime sampler (`sh_sample.wgsl`) is reused verbatim. The atlas texel byte
/// layout mirrors `OctahedralShVolumeSection`'s atlas block (see below).
///
/// The atlas payload is stored BC6H-compressed at rest (`irradiance_format ==
/// IRRADIANCE_FORMAT_BC6H`); the actual BC6H encode happens at emit time. The
/// uncompressed-debug variant (`IRRADIANCE_FORMAT_RGBA16F`) carries row-major
/// `Rgba16Float` texels. The format tag mirrors the lightmap section's
/// `irradiance_format` discipline exactly.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (68 bytes):
///     u32      version                (= DIRECT_SH_VOLUME_VERSION)
///     f32 × 3  grid_origin
///     f32 × 3  cell_size
///     u32 × 3  grid_dimensions
///     u32      tile_dimension         (default 6, border included)
///     u32      tile_border            (default 1)
///     u32      atlas_width            (texels)
///     u32      atlas_height           (texels)
///     u32      atlas_tiles_per_row    (near-square linear tile packing)
///     u32      irradiance_format      (IRRADIANCE_FORMAT_BC6H / _RGBA16F)
///     u32      irradiance_len         (byte length of the atlas blob)
///
///   Atlas blob (irradiance_len bytes):
///     IRRADIANCE_FORMAT_RGBA16F: row-major atlas_width × atlas_height texels,
///                                f16 × 4 RGBA per texel (BYTE-IDENTICAL to the
///                                `OctahedralShVolumeSection` atlas texel block).
///     IRRADIANCE_FORMAT_BC6H:    4×4 `Bc6hRgbUfloat` blocks, 16 bytes each
///                                (ceil(w/4)·ceil(h/4)·16 bytes total).
/// ```
///
/// The atlas blob is read by its stored `irradiance_len`, not by recomputing the
/// texel count, exactly as the lightmap section does — that keeps the compressed
/// and uncompressed variants on the same parsing path.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectShVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_dimensions: [u32; 2],
    pub atlas_tiles_per_row: u32,
    /// Format tag for `atlas`: `IRRADIANCE_FORMAT_BC6H` (compressed at rest) or
    /// `IRRADIANCE_FORMAT_RGBA16F` (uncompressed-debug variant). Mirrors the
    /// lightmap section's `irradiance_format`.
    pub irradiance_format: u32,
    /// Raw atlas bytes in the encoding named by `irradiance_format`.
    pub atlas: Vec<u8>,
}

impl DirectShVolumeSection {
    pub const HEADER_SIZE: usize = 68;

    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert_eq!(
            self.atlas_tiles_per_row,
            irradiance_atlas_tiles_per_row(self.grid_dimensions).unwrap_or(0)
        );
        debug_assert_eq!(
            self.atlas_dimensions,
            irradiance_atlas_dimensions(self.grid_dimensions, self.tile_dimension)
        );

        let mut buf = Vec::with_capacity(Self::HEADER_SIZE + self.atlas.len());

        buf.extend_from_slice(&DIRECT_SH_VOLUME_VERSION.to_le_bytes());
        for v in &self.grid_origin {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.cell_size {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.grid_dimensions {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.tile_dimension.to_le_bytes());
        buf.extend_from_slice(&self.tile_border.to_le_bytes());
        buf.extend_from_slice(&self.atlas_dimensions[0].to_le_bytes());
        buf.extend_from_slice(&self.atlas_dimensions[1].to_le_bytes());
        buf.extend_from_slice(&self.atlas_tiles_per_row.to_le_bytes());
        buf.extend_from_slice(&self.irradiance_format.to_le_bytes());
        buf.extend_from_slice(&(self.atlas.len() as u32).to_le_bytes());

        buf.extend_from_slice(&self.atlas);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("direct sh volume header"));
        }

        let mut o = 0;
        let version = read_u32(data, o);
        o += 4;
        if version != DIRECT_SH_VOLUME_VERSION {
            return Err(invalid_data(format!(
                "direct sh volume section version {version}, expected {DIRECT_SH_VOLUME_VERSION} — \
                 recompile the .prl with the current `prl-build`"
            )));
        }
        let grid_origin = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let cell_size = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let grid_dimensions = [
            read_u32(data, o),
            read_u32(data, o + 4),
            read_u32(data, o + 8),
        ];
        o += 12;
        let tile_dimension = read_u32(data, o);
        o += 4;
        let tile_border = read_u32(data, o);
        o += 4;
        let atlas_dimensions = [read_u32(data, o), read_u32(data, o + 4)];
        o += 8;
        let atlas_tiles_per_row = read_u32(data, o);
        o += 4;
        let irradiance_format = read_u32(data, o);
        o += 4;
        let irradiance_len = read_u32(data, o) as usize;
        o += 4;
        debug_assert_eq!(o, Self::HEADER_SIZE);

        if irradiance_format != IRRADIANCE_FORMAT_BC6H
            && irradiance_format != IRRADIANCE_FORMAT_RGBA16F
        {
            return Err(invalid_data(format!(
                "direct sh volume irradiance_format {irradiance_format} is not a known tag \
                 (expected {IRRADIANCE_FORMAT_BC6H} BC6H or {IRRADIANCE_FORMAT_RGBA16F} RGBA16F)"
            )));
        }

        validate_tile_geometry(tile_dimension, tile_border)?;
        validate_grid_and_atlas(
            grid_dimensions,
            tile_dimension,
            atlas_dimensions,
            atlas_tiles_per_row,
        )?;

        if data.len() < o + irradiance_len {
            return Err(truncated("direct sh volume atlas blob"));
        }
        let atlas = data[o..o + irradiance_len].to_vec();
        o += irradiance_len;

        if o != data.len() {
            return Err(invalid_data(format!(
                "direct sh volume has {} trailing byte(s) after the atlas blob",
                data.len() - o,
            )));
        }

        Ok(Self {
            grid_origin,
            cell_size,
            grid_dimensions,
            tile_dimension,
            tile_border,
            atlas_dimensions,
            atlas_tiles_per_row,
            irradiance_format,
            atlas,
        })
    }
}

fn validate_tile_geometry(tile_dimension: u32, tile_border: u32) -> crate::Result<()> {
    // The header stores N so a re-bake can change tile resolution without a
    // format break; reject only what *this runtime* cannot sample yet. Mirrors
    // `OctahedralShVolumeSection` so the shared sampler stays valid.
    if tile_dimension != RUNTIME_SUPPORTED_TILE_DIMENSION {
        return Err(invalid_data(format!(
            "direct sh volume tile_dimension {tile_dimension} is not supported by this runtime, which is pinned to N={RUNTIME_SUPPORTED_TILE_DIMENSION}"
        )));
    }
    if tile_border != DEFAULT_IRRADIANCE_TILE_BORDER {
        return Err(invalid_data(format!(
            "direct sh volume tile_border {tile_border}, expected {DEFAULT_IRRADIANCE_TILE_BORDER}"
        )));
    }
    if tile_dimension <= tile_border.saturating_mul(2) {
        return Err(invalid_data(format!(
            "direct sh volume tile_dimension {tile_dimension} leaves no interior texels with border {tile_border}"
        )));
    }
    Ok(())
}

fn validate_grid_and_atlas(
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
    atlas_tiles_per_row: u32,
) -> crate::Result<()> {
    let zero_axes = grid_dimensions.iter().filter(|&&d| d == 0).count();
    if zero_axes > 0 {
        if zero_axes != 3 {
            return Err(invalid_data(format!(
                "direct sh volume grid_dimensions {grid_dimensions:?} are malformed: empty grids must be [0, 0, 0]"
            )));
        }
        if atlas_dimensions != [0, 0] || atlas_tiles_per_row != 0 {
            return Err(invalid_data(format!(
                "direct sh volume empty grid must use atlas_dimensions [0, 0] and atlas_tiles_per_row 0, got atlas_dimensions {atlas_dimensions:?}, atlas_tiles_per_row {atlas_tiles_per_row}"
            )));
        }
        return Ok(());
    }

    let expected_tiles_per_row = irradiance_atlas_tiles_per_row(grid_dimensions).ok_or_else(|| {
        invalid_data(format!(
            "direct sh volume grid_dimensions {grid_dimensions:?} overflow while computing atlas_tiles_per_row"
        ))
    })?;
    if atlas_tiles_per_row != expected_tiles_per_row {
        return Err(invalid_data(format!(
            "direct sh volume atlas_tiles_per_row {atlas_tiles_per_row}, expected {expected_tiles_per_row} for grid_dimensions {grid_dimensions:?}"
        )));
    }

    let expected_atlas_dimensions = irradiance_atlas_dimensions(grid_dimensions, tile_dimension);
    if atlas_dimensions != expected_atlas_dimensions {
        return Err(invalid_data(format!(
            "direct sh volume atlas_dimensions {atlas_dimensions:?}, expected {expected_atlas_dimensions:?} for grid_dimensions {grid_dimensions:?}, tile_dimension {tile_dimension}, atlas_tiles_per_row {atlas_tiles_per_row}"
        )));
    }

    // `atlas_dimensions` are the LOGICAL near-square tile-packed dimensions
    // (byte-identical to the indirect atlas), which are multiples of
    // `tile_dimension` (6) and so are NOT guaranteed to be 4-aligned. BC6H
    // operates on 4×4 blocks, so the emitter (Task 3) rounds the encoded buffer
    // up to the next multiple of 4 on each axis before calling
    // `encode_bc6h_rgb_from_f32_rgba` — mirroring the lightmap atlas builder's
    // power-of-two ≥64 rounding. The stored blob length is therefore decoupled
    // from `atlas_dimensions` and carried verbatim in `irradiance_len`; this
    // section validates the logical geometry, not the padded BC6H block count.
    Ok(())
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("direct sh volume section truncated: {what}"),
    ))
}

fn invalid_data(msg: String) -> FormatError {
    FormatError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION;

    /// Build a populated section whose atlas is a stand-in BC6H blob sized to the
    /// 4×4-block count for the grid's atlas dimensions. (Task 1 only carries and
    /// round-trips the bytes; the real BC6H encode lands in Task 3.)
    fn direct_section(grid: [u32; 3], format: u32) -> DirectShVolumeSection {
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let tile_border = DEFAULT_IRRADIANCE_TILE_BORDER;
        let atlas_dimensions = irradiance_atlas_dimensions(grid, tile_dimension);
        let atlas_tiles_per_row = irradiance_atlas_tiles_per_row(grid).unwrap();
        let atlas_len = if format == IRRADIANCE_FORMAT_BC6H {
            // The emitter rounds each axis up to a multiple of 4 (BC6H block
            // size) before encoding; mirror that here so the blob length is the
            // real BC6H block-payload size for the padded atlas.
            let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
            let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
            let blocks_x = (padded_w / 4) as usize;
            let blocks_y = (padded_h / 4) as usize;
            blocks_x * blocks_y * 16
        } else {
            (atlas_dimensions[0] * atlas_dimensions[1]) as usize * 8
        };
        DirectShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            tile_dimension,
            tile_border,
            atlas_dimensions,
            atlas_tiles_per_row,
            irradiance_format: format,
            atlas: (0..atlas_len).map(|i| (i % 256) as u8).collect(),
        }
    }

    #[test]
    fn direct_sh_volume_round_trips_bc6h_atlas() {
        let section = direct_section([3, 2, 4], IRRADIANCE_FORMAT_BC6H);
        let bytes = section.to_bytes();
        let restored = DirectShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    }

    #[test]
    fn direct_sh_volume_round_trips_uncompressed_debug_atlas() {
        let section = direct_section([2, 2, 1], IRRADIANCE_FORMAT_RGBA16F);
        let bytes = section.to_bytes();
        assert_eq!(
            bytes.len(),
            DirectShVolumeSection::HEADER_SIZE + section.atlas.len()
        );
        let restored = DirectShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    }

    #[test]
    fn direct_sh_volume_round_trips_near_square_tiles_per_row() {
        let section = direct_section([3, 2, 4], IRRADIANCE_FORMAT_BC6H);
        assert_eq!(section.atlas_tiles_per_row, 5);
        assert_eq!(section.atlas_dimensions, [30, 30]);
        let restored = DirectShVolumeSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.atlas_tiles_per_row, 5);
        assert_eq!(restored.atlas_dimensions, [30, 30]);
    }

    #[test]
    fn direct_sh_volume_rejects_unknown_format_tag() {
        let section = direct_section([1, 1, 1], IRRADIANCE_FORMAT_BC6H);
        let mut bytes = section.to_bytes();
        // irradiance_format is the u32 at header offset 60 (version[0..4],
        // origin[4..16], cell[16..28], dims[28..40], tile_dim[40..44],
        // tile_border[44..48], atlas_w[48..52], atlas_h[52..56],
        // tiles_per_row[56..60]).
        bytes[60..64].copy_from_slice(&7u32.to_le_bytes());
        let err = DirectShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("irradiance_format"),
            "expected format-tag error, got: {err}",
        );
    }

    #[test]
    fn direct_sh_volume_rejects_trailing_bytes() {
        let section = direct_section([1, 1, 1], IRRADIANCE_FORMAT_BC6H);
        let mut bytes = section.to_bytes();
        bytes.push(0xAB);
        let err = DirectShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("trailing byte"),
            "expected trailing-byte error, got: {err}",
        );
    }

    #[test]
    fn direct_sh_volume_rejects_previous_section_version() {
        let section = direct_section([1, 1, 1], IRRADIANCE_FORMAT_BC6H);
        let mut bytes = section.to_bytes();
        bytes[0..4].copy_from_slice(&0u32.to_le_bytes());
        let err = DirectShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("version"),
            "expected version-mismatch error, got: {err}",
        );
    }

    #[test]
    fn direct_sh_volume_section_id_is_thirty_five() {
        use crate::SectionId;

        assert_eq!(SectionId::DirectShVolume as u32, 35);
        assert_eq!(SectionId::from_u32(35), Some(SectionId::DirectShVolume));
    }
}
