// DirectShVolume section (id 35): baked static-direct octahedral irradiance for dynamic objects.
// See: context/lib/build_pipeline.md

use crate::FormatError;
use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, MAX_SH_ATLAS_LAYERS, RUNTIME_SUPPORTED_TILE_DIMENSION,
};

/// Section-internal version written as the first u32 of the `DirectShVolume`
/// section payload. Bumped any time the on-disk layout changes so the loader can
/// reject stale `.prl` files with a clear error rather than silently misread
/// them. Starts at 1 — this section is new and shares no history with the
/// indirect `OctahedralShVolume` section's version line. Version 2 adds
/// layer-aware atlas metadata for 2D array texture uploads. Version 3 changes
/// that geometry from the dense grid to id-34's metadata-derived stored tiles.
pub const DIRECT_SH_VOLUME_VERSION: u32 = 3;

/// Direct-light octahedral irradiance volume section (ID 35).
///
/// Carries a stored-tile octahedral irradiance atlas holding DIRECT light from
/// STATIC lights, sampled at runtime by dynamic objects (entities and
/// billboards). It is a sibling to [`crate::sh_volume::OctahedralShVolumeSection`]
/// (the INDIRECT atlas) and mirrors its stored atlas geometry exactly except:
///   - it carries direct (not indirect) coefficients, and
///   - it has NO depth moments and NO animation data.
///
/// Direct light is static, so no animation table is needed. Depth
/// moments and per-probe validity are NOT duplicated here: the direct probe grid
/// is byte-identical in position to the indirect grid, so the runtime reads them
/// from the existing `OctahedralShVolumeSection`. This section still carries its
/// own grid header (byte-identical to the indirect grid) for round-trip and
/// validation.
///
/// The stored-tile geometry (`tile_dimension` / `tile_border`,
/// shared per-layer `atlas_dimensions`, `layer_count` / `tiles_per_layer`
/// derivation, `atlas_tiles_per_row`) and brick-major stored slot order are
/// IDENTICAL to the indirect atlas. This section has no metadata, so the
/// loader cross-checks it against id 34 before runtime use.
///
/// The atlas payload is stored BC6H-compressed at rest (`irradiance_format ==
/// IRRADIANCE_FORMAT_BC6H`); the actual BC6H encode happens at emit time. The
/// uncompressed-debug variant (`IRRADIANCE_FORMAT_RGBA16F`) carries layer-major
/// `Rgba16Float` texels. The format tag mirrors the lightmap section's
/// `irradiance_format` discipline exactly.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (76 bytes):
///     u32      version                (= DIRECT_SH_VOLUME_VERSION)
///     f32 × 3  grid_origin
///     f32 × 3  cell_size
///     u32 × 3  grid_dimensions
///     u32      tile_dimension         (default 6, border included)
///     u32      tile_border            (default 1)
///     u32      atlas_width            (stored-tile atlas, per-layer texels)
///     u32      atlas_height           (stored-tile atlas, per-layer texels)
///     u32      atlas_tiles_per_row    (per-layer tile columns)
///     u32      layer_count            (2D array layers)
///     u32      tiles_per_layer        (whole probe tiles per layer)
///     u32      irradiance_format      (IRRADIANCE_FORMAT_BC6H / _RGBA16F)
///     u32      irradiance_len         (byte length of the atlas blob)
///
///   Atlas blob (irradiance_len bytes):
///     IRRADIANCE_FORMAT_RGBA16F: layer-major, then row-major
///                                atlas_width × atlas_height texels per layer,
///                                f16 × 4 RGBA per texel (BYTE-IDENTICAL to the
///                                `OctahedralShVolumeSection` atlas texel block).
///     IRRADIANCE_FORMAT_BC6H:    layer-major 4×4 `Bc6hRgbUfloat` blocks,
///                                16 bytes each
///                                (layer_count·ceil(w/4)·ceil(h/4)·16 bytes total).
/// ```
///
/// The atlas blob length must match the declared format, per-layer atlas
/// dimensions, and layer count exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectShVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_dimensions: [u32; 2],
    pub layer_count: u32,
    pub tiles_per_layer: u32,
    pub atlas_tiles_per_row: u32,
    /// Format tag for `atlas`: `IRRADIANCE_FORMAT_BC6H` (compressed at rest) or
    /// `IRRADIANCE_FORMAT_RGBA16F` (uncompressed-debug variant). Mirrors the
    /// lightmap section's `irradiance_format`.
    pub irradiance_format: u32,
    /// Raw stored-atlas bytes in the encoding named by `irradiance_format`.
    pub atlas: Vec<u8>,
}

impl DirectShVolumeSection {
    pub const HEADER_SIZE: usize = 76;

    pub fn placeholder() -> Self {
        Self {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [0, 0, 0],
            tile_dimension: RUNTIME_SUPPORTED_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            atlas: Vec::new(),
        }
    }

    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes()
            .expect("DirectShVolumeSection must satisfy its wire contract")
    }

    /// Encode only a canonical stored-atlas section whose payload fits the
    /// fixed `u32` length field.
    pub fn try_to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate_wire_contract()?;

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
        buf.extend_from_slice(&self.layer_count.to_le_bytes());
        buf.extend_from_slice(&self.tiles_per_layer.to_le_bytes());
        buf.extend_from_slice(&self.irradiance_format.to_le_bytes());
        buf.extend_from_slice(&(self.atlas.len() as u32).to_le_bytes());

        buf.extend_from_slice(&self.atlas);
        Ok(buf)
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(truncated("direct sh volume header"));
        }

        let mut o = 0;
        let version = read_u32(data, o);
        o += 4;
        if version != DIRECT_SH_VOLUME_VERSION {
            return Err(invalid_data(format!(
                "direct sh volume section version {version}, expected {DIRECT_SH_VOLUME_VERSION} — \
                 recompile the .prl with the current `prl-build` for the v3 stored-atlas format"
            )));
        }
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("direct sh volume header"));
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
        let layer_count = read_u32(data, o);
        o += 4;
        let tiles_per_layer = read_u32(data, o);
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
        validate_grid_and_stored_atlas(
            grid_dimensions,
            tile_dimension,
            atlas_dimensions,
            layer_count,
            tiles_per_layer,
            atlas_tiles_per_row,
        )?;

        let expected_len =
            expected_irradiance_len(irradiance_format, atlas_dimensions, layer_count)?;
        if irradiance_len != expected_len {
            return Err(invalid_data(format!(
                "direct sh volume irradiance_len {irradiance_len}, expected {expected_len} for irradiance_format {irradiance_format}, atlas_dimensions {atlas_dimensions:?}, layer_count {layer_count}"
            )));
        }

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
            layer_count,
            tiles_per_layer,
            atlas_tiles_per_row,
            irradiance_format,
            atlas,
        })
    }

    fn validate_wire_contract(&self) -> crate::Result<()> {
        validate_irradiance_format(self.irradiance_format)?;
        validate_tile_geometry(self.tile_dimension, self.tile_border)?;
        validate_grid_and_stored_atlas(
            self.grid_dimensions,
            self.tile_dimension,
            self.atlas_dimensions,
            self.layer_count,
            self.tiles_per_layer,
            self.atlas_tiles_per_row,
        )?;
        let expected_len = expected_irradiance_len(
            self.irradiance_format,
            self.atlas_dimensions,
            self.layer_count,
        )?;
        if self.atlas.len() != expected_len {
            return Err(invalid_data(format!(
                "direct sh volume atlas length {}, expected {expected_len} for irradiance_format {}, atlas_dimensions {:?}, layer_count {}",
                self.atlas.len(),
                self.irradiance_format,
                self.atlas_dimensions,
                self.layer_count,
            )));
        }
        atlas_len_for_header(self.atlas.len() as u64)?;
        Ok(())
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

fn validate_grid_and_stored_atlas(
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    atlas_tiles_per_row: u32,
) -> crate::Result<()> {
    let zero_axes = grid_dimensions.iter().filter(|&&d| d == 0).count();
    if zero_axes > 0 {
        if zero_axes != 3 {
            return Err(invalid_data(format!(
                "direct sh volume grid_dimensions {grid_dimensions:?} are malformed: empty grids must be [0, 0, 0]"
            )));
        }
        if atlas_dimensions != [0, 0]
            || layer_count != 0
            || tiles_per_layer != 0
            || atlas_tiles_per_row != 0
        {
            return Err(invalid_data(format!(
                "direct sh volume empty grid must use zero stored-atlas geometry, got atlas_dimensions {atlas_dimensions:?}, layer_count {layer_count}, tiles_per_layer {tiles_per_layer}, atlas_tiles_per_row {atlas_tiles_per_row}"
            )));
        }
        return Ok(());
    }

    validate_stored_atlas_shape(
        tile_dimension,
        atlas_dimensions,
        layer_count,
        tiles_per_layer,
        atlas_tiles_per_row,
    )
}

fn expected_irradiance_len(
    irradiance_format: u32,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
) -> crate::Result<usize> {
    let layer_count = layer_count as usize;
    let atlas_width = atlas_dimensions[0] as usize;
    let atlas_height = atlas_dimensions[1] as usize;

    match irradiance_format {
        IRRADIANCE_FORMAT_BC6H => checked_len(
            &[
                layer_count,
                atlas_dimensions[0].div_ceil(4) as usize,
                atlas_dimensions[1].div_ceil(4) as usize,
                16,
            ],
            "direct sh volume BC6H atlas byte length overflows usize",
        ),
        IRRADIANCE_FORMAT_RGBA16F => checked_len(
            &[layer_count, atlas_width, atlas_height, 8],
            "direct sh volume RGBA16F atlas byte length overflows usize",
        ),
        _ => Err(invalid_data(format!(
            "direct sh volume irradiance_format {irradiance_format} is not a known tag \
             (expected {IRRADIANCE_FORMAT_BC6H} BC6H or {IRRADIANCE_FORMAT_RGBA16F} RGBA16F)"
        ))),
    }
}

fn checked_len(factors: &[usize], overflow_msg: &str) -> crate::Result<usize> {
    factors.iter().try_fold(1usize, |acc, factor| {
        acc.checked_mul(*factor)
            .ok_or_else(|| invalid_data(overflow_msg.to_string()))
    })
}

fn validate_stored_atlas_shape(
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    atlas_tiles_per_row: u32,
) -> crate::Result<()> {
    if atlas_dimensions == [0, 0]
        && layer_count == 0
        && tiles_per_layer == 0
        && atlas_tiles_per_row == 0
    {
        return Ok(());
    }
    if atlas_dimensions.contains(&0)
        || layer_count == 0
        || tiles_per_layer == 0
        || atlas_tiles_per_row == 0
    {
        return Err(invalid_data(format!(
            "direct sh volume stored atlas geometry must be entirely zero or nonzero, got atlas_dimensions {atlas_dimensions:?}, layer_count {layer_count}, tiles_per_layer {tiles_per_layer}, atlas_tiles_per_row {atlas_tiles_per_row}"
        )));
    }
    if layer_count > MAX_SH_ATLAS_LAYERS {
        return Err(invalid_data(format!(
            "direct sh volume stored atlas layer_count {layer_count} exceeds maximum {MAX_SH_ATLAS_LAYERS}"
        )));
    }
    if atlas_dimensions[0] % tile_dimension != 0 || atlas_dimensions[1] % tile_dimension != 0 {
        return Err(invalid_data(format!(
            "direct sh volume stored atlas_dimensions {atlas_dimensions:?} must be multiples of tile_dimension {tile_dimension}"
        )));
    }
    let expected_tiles_per_row = atlas_dimensions[0] / tile_dimension;
    let tile_rows = atlas_dimensions[1] / tile_dimension;
    let expected_tiles_per_layer =
        expected_tiles_per_row
            .checked_mul(tile_rows)
            .ok_or_else(|| {
                invalid_data("direct sh volume stored atlas tile count overflows u32".into())
            })?;
    if atlas_tiles_per_row != expected_tiles_per_row || tiles_per_layer != expected_tiles_per_layer
    {
        return Err(invalid_data(format!(
            "direct sh volume stored atlas geometry has tiles_per_row {atlas_tiles_per_row}, tiles_per_layer {tiles_per_layer}; expected {expected_tiles_per_row} and {expected_tiles_per_layer} for atlas_dimensions {atlas_dimensions:?}"
        )));
    }
    Ok(())
}

fn validate_irradiance_format(irradiance_format: u32) -> crate::Result<()> {
    match irradiance_format {
        IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F => Ok(()),
        _ => Err(invalid_data(format!(
            "direct sh volume irradiance_format {irradiance_format} is not a known tag \
             (expected {IRRADIANCE_FORMAT_BC6H} BC6H or {IRRADIANCE_FORMAT_RGBA16F} RGBA16F)"
        ))),
    }
}

fn atlas_len_for_header(len: u64) -> crate::Result<u32> {
    u32::try_from(len).map_err(|_| {
        invalid_data(format!(
            "direct sh volume stored atlas byte length {len} exceeds the v3 u32 header maximum {}",
            u32::MAX,
        ))
    })
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
    use crate::octahedral::{DEFAULT_IRRADIANCE_TILE_DIMENSION, irradiance_atlas_array_layout};

    /// Build a populated section whose atlas is a stand-in BC6H blob sized to the
    /// 4×4-block count for the stored atlas dimensions.
    fn direct_section(grid: [u32; 3], format: u32) -> DirectShVolumeSection {
        direct_section_with_max_dim(grid, format, 8192)
    }

    fn direct_section_with_max_dim(
        grid: [u32; 3],
        format: u32,
        max_dim: u32,
    ) -> DirectShVolumeSection {
        let stored_tile_count = grid.iter().product();
        direct_section_with_stored_tiles(grid, stored_tile_count, format, max_dim)
    }

    fn direct_section_with_stored_tiles(
        grid: [u32; 3],
        stored_tile_count: u32,
        format: u32,
        max_dim: u32,
    ) -> DirectShVolumeSection {
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let tile_border = DEFAULT_IRRADIANCE_TILE_BORDER;
        let layout =
            irradiance_atlas_array_layout([stored_tile_count, 1, 1], tile_dimension, max_dim)
                .unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        let atlas_len = if format == IRRADIANCE_FORMAT_BC6H {
            // The emitter rounds each axis up to a multiple of 4 (BC6H block
            // size) before encoding; mirror that here so the blob length is the
            // real per-layer BC6H block-payload size for the padded atlas.
            let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
            let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
            let blocks_x = (padded_w / 4) as usize;
            let blocks_y = (padded_h / 4) as usize;
            layout.layer_count as usize * blocks_x * blocks_y * 16
        } else {
            (layout.layer_count * atlas_dimensions[0] * atlas_dimensions[1]) as usize * 8
        };
        DirectShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            tile_dimension,
            tile_border,
            atlas_dimensions,
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            irradiance_format: format,
            atlas: (0..atlas_len).map(|i| (i % 256) as u8).collect(),
        }
    }

    #[test]
    fn direct_sh_volume_round_trips_single_layer_bc6h_atlas() {
        let section = direct_section([3, 2, 4], IRRADIANCE_FORMAT_BC6H);
        assert_eq!(section.layer_count, 1);
        let bytes = section.to_bytes();
        assert_eq!(
            &bytes[56..60],
            section.atlas_tiles_per_row.to_le_bytes().as_slice()
        );
        assert_eq!(&bytes[60..64], section.layer_count.to_le_bytes().as_slice());
        assert_eq!(
            &bytes[64..68],
            section.tiles_per_layer.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[68..72],
            section.irradiance_format.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[72..76],
            (section.atlas.len() as u32).to_le_bytes().as_slice()
        );
        let restored = DirectShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    }

    #[test]
    fn direct_sh_volume_round_trips_stored_geometry_without_metadata() {
        let section = direct_section_with_stored_tiles([8, 4, 4], 10, IRRADIANCE_FORMAT_BC6H, 8192);
        assert_eq!(section.atlas_dimensions, [24, 18]);
        assert_eq!(section.atlas_tiles_per_row, 4);
        assert_eq!(section.tiles_per_layer, 12);
        assert_eq!(section.layer_count, 1);
        assert_eq!(
            DirectShVolumeSection::from_bytes(&section.to_bytes()).unwrap(),
            section
        );
    }

    #[test]
    fn direct_sh_volume_round_trips_multi_layer_bc6h_atlas() {
        let section = direct_section_with_max_dim([20, 1, 1], IRRADIANCE_FORMAT_BC6H, 20);
        assert_eq!(section.layer_count, 3);
        assert_eq!(section.tiles_per_layer, 9);
        assert_eq!(section.atlas_tiles_per_row, 3);
        assert_eq!(section.atlas_dimensions, [18, 18]);
        assert_eq!(section.atlas.len(), 3 * 5 * 5 * 16);

        let bytes = section.to_bytes();
        let restored = DirectShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    }

    #[test]
    fn direct_sh_volume_rejects_short_multi_layer_bc6h_atlas() {
        // Regression: a three-layer BC6H atlas could declare only one layer of
        // bytes, then fail later when renderer upload consumed the short blob.
        let section = direct_section_with_max_dim([20, 1, 1], IRRADIANCE_FORMAT_BC6H, 20);
        assert_eq!(section.layer_count, 3);
        let short_len = section.atlas.len() / section.layer_count as usize;
        let mut bytes = section.to_bytes();
        bytes[72..76].copy_from_slice(&(short_len as u32).to_le_bytes());
        bytes.truncate(DirectShVolumeSection::HEADER_SIZE + short_len);

        let err = DirectShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("irradiance_len") && msg.contains("expected 1200"),
            "expected irradiance length error, got: {msg}",
        );
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
        assert_eq!(section.layer_count, 1);
        assert_eq!(section.tiles_per_layer, 25);
        let restored = DirectShVolumeSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.atlas_tiles_per_row, 5);
        assert_eq!(restored.atlas_dimensions, [30, 30]);
        assert_eq!(restored.layer_count, 1);
        assert_eq!(restored.tiles_per_layer, 25);
    }

    #[test]
    fn direct_sh_volume_rejects_unknown_format_tag() {
        let section = direct_section([1, 1, 1], IRRADIANCE_FORMAT_BC6H);
        let mut bytes = section.to_bytes();
        // irradiance_format is the u32 at header offset 68 (version[0..4],
        // origin[4..16], cell[16..28], dims[28..40], tile_dim[40..44],
        // tile_border[44..48], atlas_w[48..52], atlas_h[52..56],
        // tiles_per_row[56..60], layer_count[60..64], tiles_per_layer[64..68]).
        bytes[68..72].copy_from_slice(&7u32.to_le_bytes());
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
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        let err = DirectShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version 2")
                && msg.contains("expected 3")
                && msg.contains("v3 stored-atlas"),
            "expected version-mismatch error, got: {msg}",
        );
    }

    #[test]
    fn direct_sh_volume_placeholder_round_trips() {
        let section = DirectShVolumeSection::placeholder();
        let bytes = section.to_bytes();
        assert_eq!(
            bytes.len(),
            DirectShVolumeSection::HEADER_SIZE + section.atlas.len()
        );
        let restored = DirectShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn direct_sh_volume_section_id_is_thirty_five() {
        use crate::SectionId;

        assert_eq!(SectionId::DirectShVolume as u32, 35);
        assert_eq!(SectionId::from_u32(35), Some(SectionId::DirectShVolume));
    }
}
