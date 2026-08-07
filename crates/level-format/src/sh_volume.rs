// OctahedralShVolume section (id 34): the live baked-irradiance section.
//
// See: context/lib/build_pipeline.md

use crate::FormatError;
use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, RUNTIME_SUPPORTED_TILE_DIMENSION, irradiance_atlas_array_layout,
};

/// Section-internal version written as the first u32 of the `OctahedralShVolume`
/// section payload. Bumped any time the on-disk layout changes so the loader can
/// reject stale `.prl` files with a clear error rather than silently misread
/// them.
///
/// The SH *coefficient* layout has been frozen since v5; v6+ describe the
/// octahedral atlas, not the SH record. History: version 1 (pre-animated-flag) —
/// no `start_active` in the descriptor table; version 2 — `start_active: u32`
/// lives alongside the brightness/color counts; version 3 — direction channel
/// samples serialized after color samples, with a `direction_count` field in the
/// descriptor header; version 4 — two f16 depth moments (`mean_distance`,
/// `mean_sq_distance`) appended inside the per-probe record after `validity`;
/// version 5 — trailing `map-light-index → animated-light section slot` table
/// (Task 2c of `sdf-static-occluder-shadows`), `u32::MAX` = no slot; version 6 —
/// base irradiance replaced the per-probe SH coefficients with a 2D octahedral
/// `Rgba16Float` atlas (per-probe validity/depth moments retained); version 7 —
/// octahedral atlas packing changed from z-stacked grid rows to near-square
/// linear tile rows and stores `atlas_tiles_per_row` in the header; version 8
/// — atlas metadata became layer-aware for 2D array texture uploads, storing
/// shared per-layer dimensions plus `layer_count` and `tiles_per_layer`; version
/// 9 (current) — the base atlas became a valid-probe-only compact payload with
/// its own geometry and a BC6H/RGBA16F format tag. The original atlas geometry
/// remains dense for the composed runtime atlas and sampler tile math.
pub const SH_VOLUME_VERSION: u32 = 9;

/// Sentinel for "this map light has no animated-light section slot" in
/// `OctahedralShVolumeSection.slot_for_map_light`. Non-animated lights and any
/// light the bake excluded from the animated-baked namespace use this value.
pub const ANIMATED_SLOT_NONE: u32 = u32::MAX;

/// Byte stride of one serialized octahedral probe metadata record:
/// `u8 validity` + two f16 depth moments + `u8 density_level` + 2 bytes of
/// padding.
pub const OCTAHEDRAL_PROBE_STRIDE: u32 = 8;

/// Serialized atlas texel stride for `Rgba16Float`: 4 f16 channels.
pub const OCTAHEDRAL_ATLAS_TEXEL_STRIDE: u32 = 8;

/// One probe's non-atlas metadata in the octahedral irradiance volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OctahedralShProbe {
    /// 0 = invalid (inside solid), 1 = valid (usable by runtime).
    pub validity: u8,
    /// Mean ray distance `E[d]`, f16 bits.
    pub mean_distance: u16,
    /// Mean squared ray distance `E[d²]`, f16 bits.
    pub mean_sq_distance: u16,
    /// Reserved for the adaptive-density follow-up. Every v9 bake writes zero;
    /// v9 parsing rejects nonzero values rather than assigning them semantics.
    pub density_level: u8,
}

/// One `Rgba16Float` atlas texel, stored as raw f16 channel bits.
///
/// This remains the compiler's uncompressed tile-packing representation. The
/// v9 section payload itself is the tagged byte blob on
/// [`OctahedralShVolumeSection::compact_atlas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OctahedralAtlasTexel {
    pub rgba: [u16; 4],
}

/// Animation curves for one animated light, stored once per light (not per
/// probe). Brightness and color channels are uniformly-sampled over the
/// light's period; the runtime linearly interpolates between samples.
///
/// A `brightness_count` / `color_count` of 0 means the channel holds constant
/// over the cycle (use `base_color` or unit brightness, respectively).
///
/// `start_active` is the initial runtime on/off state. 1 = active at map load
/// (the default — lights light); 0 = spawned dark, typically because the
/// entity carried `_start_inactive = 1`. Scripting toggles the GPU mirror of
/// this flag at runtime; only the initial value lives on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationDescriptor {
    pub period: f32,
    pub phase: f32,
    pub base_color: [f32; 3],
    pub brightness: Vec<f32>,
    pub color: Vec<[f32; 3]>,
    /// Animated cone-direction samples for spot lights (Plan 2 Sub-plan 1).
    /// Samples must be unit-length — enforced by the scripting primitive
    /// `set_light_animation` and the FGD `direction_curve` parser. The GPU
    /// evaluator does not re-normalize per frame; a `debug_assert` in the
    /// GPU writer checks the invariant in debug builds.
    pub direction: Vec<[f32; 3]>,
    pub start_active: u32,
}

impl Default for AnimationDescriptor {
    fn default() -> Self {
        Self {
            period: 0.0,
            phase: 0.0,
            base_color: [0.0; 3],
            brightness: Vec::new(),
            color: Vec::new(),
            direction: Vec::new(),
            start_active: 1,
        }
    }
}

/// Octahedral irradiance volume section (ID 34).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (104 bytes):
///     u32      version                (= SH_VOLUME_VERSION)
///     f32 × 3  grid_origin
///     f32 × 3  cell_size
///     u32 × 3  grid_dimensions
///     u32      probe_stride           (= OCTAHEDRAL_PROBE_STRIDE = 8)
///     u32      animated_light_count
///     u32      tile_dimension         (default 6, border included)
///     u32      tile_border            (default 1)
///     u32      atlas_width            (shared per-layer texels)
///     u32      atlas_height           (shared per-layer texels)
///     u32      atlas_tiles_per_row    (per-layer tile columns)
///     u32      layer_count            (2D array layers)
///     u32      tiles_per_layer        (whole probe tiles per layer)
///
///     The preceding atlas fields retain their v8 meanings: they describe the
///     dense per-grid-probe composed atlas and sampler tile geometry.
///
///     u32      compact_atlas_width     (valid-probe payload, per layer)
///     u32      compact_atlas_height    (valid-probe payload, per layer)
///     u32      compact_atlas_tiles_per_row
///     u32      compact_atlas_tiles_per_layer
///     u32      compact_atlas_layer_count
///     u32      irradiance_format       (BC6H / RGBA16F tag from `lightmap`)
///     u32      compact_atlas_len       (byte length of compact atlas blob)
///
///   Probe metadata records (probe_stride bytes each, x-fastest order):
///     u8       validity
///     f16      mean_distance          (E[d])
///     f16      mean_sq_distance       (E[d²])
///     u8       density_level          (= 0 in v9; nonzero is rejected)
///     u8 × 2   padding
///
///   Compact atlas blob (compact_atlas_len bytes), carrying tiles only for
///   metadata-valid probes in x-fastest probe order:
///     IRRADIANCE_FORMAT_RGBA16F: layer-major, then row-major
///                                compact_atlas_width × compact_atlas_height
///                                `f16 × 4` texels per layer.
///     IRRADIANCE_FORMAT_BC6H:    layer-major 4×4 `Bc6hRgbUfloat` blocks,
///                                `compact_atlas_layer_count ×
///                                ceil(width / 4) × ceil(height / 4) × 16`
///                                bytes total.
///
///   Animation descriptor table and map-light slot table:
///     written by `write_animation_descriptors` / `write_slot_table`
///     (period, phase, base_color, sample counts, then samples; followed by a
///     u32-prefixed `slot_for_map_light` table)
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct OctahedralShVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub probe_stride: u32,
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_dimensions: [u32; 2],
    pub layer_count: u32,
    pub tiles_per_layer: u32,
    pub atlas_tiles_per_row: u32,
    pub probes: Vec<OctahedralShProbe>,
    /// Per-layer dimensions for the compact valid-probe-only payload. These
    /// never replace the dense `atlas_dimensions` sampler contract above.
    pub compact_atlas_dimensions: [u32; 2],
    pub compact_atlas_tiles_per_row: u32,
    pub compact_atlas_tiles_per_layer: u32,
    pub compact_atlas_layer_count: u32,
    /// Format tag for `compact_atlas`: `IRRADIANCE_FORMAT_BC6H` (default at
    /// rest) or `IRRADIANCE_FORMAT_RGBA16F` (uncompressed debug path).
    pub irradiance_format: u32,
    /// Raw compact atlas bytes in the encoding named by `irradiance_format`.
    /// Tiles cover metadata-valid probes only, in x-fastest probe order.
    pub compact_atlas: Vec<u8>,
    pub animation_descriptors: Vec<AnimationDescriptor>,
    pub slot_for_map_light: Vec<u32>,
}

impl OctahedralShVolumeSection {
    pub const HEADER_SIZE: usize = 104;

    pub fn placeholder() -> Self {
        Self {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [0, 0, 0],
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: RUNTIME_SUPPORTED_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let total_probes = self.total_probes();
        let valid_probe_count = valid_probe_count(&self.probes);
        debug_assert_eq!(self.probes.len(), total_probes);
        debug_assert!(self.probes.iter().all(|probe| probe.density_level == 0));
        debug_assert!(
            expected_array_layout(
                self.grid_dimensions,
                self.tile_dimension,
                self.atlas_dimensions,
            )
            .is_some_and(|layout| {
                self.layer_count == layout.layer_count
                    && self.tiles_per_layer == layout.tiles_per_layer
                    && self.atlas_tiles_per_row == layout.atlas_tiles_per_row
                    && self.atlas_dimensions == [layout.atlas_width, layout.atlas_height]
            })
        );
        debug_assert!(
            expected_compact_array_layout(
                valid_probe_count,
                self.tile_dimension,
                self.compact_atlas_dimensions,
            )
            .is_some_and(|layout| {
                self.compact_atlas_layer_count == layout.layer_count
                    && self.compact_atlas_tiles_per_layer == layout.tiles_per_layer
                    && self.compact_atlas_tiles_per_row == layout.atlas_tiles_per_row
                    && self.compact_atlas_dimensions == [layout.atlas_width, layout.atlas_height]
            })
        );
        debug_assert_eq!(
            expected_compact_atlas_len(
                self.irradiance_format,
                self.compact_atlas_dimensions,
                self.compact_atlas_layer_count,
            )
            .ok(),
            Some(self.compact_atlas.len())
        );

        let mut buf = Vec::with_capacity(
            Self::HEADER_SIZE
                + total_probes * OCTAHEDRAL_PROBE_STRIDE as usize
                + self.compact_atlas.len(),
        );

        buf.extend_from_slice(&SH_VOLUME_VERSION.to_le_bytes());
        for v in &self.grid_origin {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.cell_size {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.grid_dimensions {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.probe_stride.to_le_bytes());
        buf.extend_from_slice(&(self.animation_descriptors.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.tile_dimension.to_le_bytes());
        buf.extend_from_slice(&self.tile_border.to_le_bytes());
        buf.extend_from_slice(&self.atlas_dimensions[0].to_le_bytes());
        buf.extend_from_slice(&self.atlas_dimensions[1].to_le_bytes());
        buf.extend_from_slice(&self.atlas_tiles_per_row.to_le_bytes());
        buf.extend_from_slice(&self.layer_count.to_le_bytes());
        buf.extend_from_slice(&self.tiles_per_layer.to_le_bytes());
        buf.extend_from_slice(&self.compact_atlas_dimensions[0].to_le_bytes());
        buf.extend_from_slice(&self.compact_atlas_dimensions[1].to_le_bytes());
        buf.extend_from_slice(&self.compact_atlas_tiles_per_row.to_le_bytes());
        buf.extend_from_slice(&self.compact_atlas_tiles_per_layer.to_le_bytes());
        buf.extend_from_slice(&self.compact_atlas_layer_count.to_le_bytes());
        buf.extend_from_slice(&self.irradiance_format.to_le_bytes());
        buf.extend_from_slice(&(self.compact_atlas.len() as u32).to_le_bytes());

        for probe in &self.probes {
            buf.push(probe.validity);
            buf.extend_from_slice(&probe.mean_distance.to_le_bytes());
            buf.extend_from_slice(&probe.mean_sq_distance.to_le_bytes());
            buf.push(probe.density_level);
            buf.extend_from_slice(&[0u8; 2]);
        }

        buf.extend_from_slice(&self.compact_atlas);

        write_animation_descriptors(&mut buf, &self.animation_descriptors);
        write_slot_table(&mut buf, &self.slot_for_map_light);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(truncated("octahedral header"));
        }

        let mut o = 0;
        let version = read_u32(data, o);
        o += 4;
        if version != SH_VOLUME_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "octahedral sh volume section version {version}, expected {SH_VOLUME_VERSION} — \
                     recompile the .prl with the current `prl-build` for the v9 compact-atlas format"
                ),
            )));
        }
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("octahedral header"));
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
        let probe_stride = read_u32(data, o);
        o += 4;
        let animated_light_count = read_u32(data, o) as usize;
        o += 4;
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
        let compact_atlas_dimensions = [read_u32(data, o), read_u32(data, o + 4)];
        o += 8;
        let compact_atlas_tiles_per_row = read_u32(data, o);
        o += 4;
        let compact_atlas_tiles_per_layer = read_u32(data, o);
        o += 4;
        let compact_atlas_layer_count = read_u32(data, o);
        o += 4;
        let irradiance_format = read_u32(data, o);
        o += 4;
        let compact_atlas_len = read_u32(data, o) as usize;
        o += 4;
        debug_assert_eq!(o, Self::HEADER_SIZE);

        if probe_stride < OCTAHEDRAL_PROBE_STRIDE {
            return Err(invalid_data(format!(
                "octahedral sh volume probe_stride {probe_stride} is smaller than the minimum {OCTAHEDRAL_PROBE_STRIDE}"
            )));
        }

        validate_irradiance_format(irradiance_format)?;

        validate_octahedral_tile_geometry(tile_dimension, tile_border)?;
        validate_octahedral_grid_and_atlas(
            grid_dimensions,
            tile_dimension,
            atlas_dimensions,
            layer_count,
            tiles_per_layer,
            atlas_tiles_per_row,
        )?;

        let total_probes = (grid_dimensions[0] as usize)
            .checked_mul(grid_dimensions[1] as usize)
            .and_then(|n| n.checked_mul(grid_dimensions[2] as usize))
            .ok_or_else(|| {
                invalid_data(format!(
                    "octahedral sh volume grid_dimensions {:?} overflow",
                    grid_dimensions,
                ))
            })?;
        let probe_bytes = total_probes
            .checked_mul(probe_stride as usize)
            .ok_or_else(|| {
                invalid_data("octahedral sh volume probe byte count overflow".to_string())
            })?;
        if data.len() < o + probe_bytes {
            return Err(truncated("octahedral probe metadata records"));
        }

        let mut probes = Vec::with_capacity(total_probes);
        for _ in 0..total_probes {
            probes.push(OctahedralShProbe {
                validity: data[o],
                mean_distance: read_u16(data, o + 1),
                mean_sq_distance: read_u16(data, o + 3),
                density_level: data[o + 5],
            });
            o += probe_stride as usize;
        }

        if let Some((probe_index, density_level)) = probes
            .iter()
            .enumerate()
            .find(|(_, probe)| probe.density_level != 0)
            .map(|(probe_index, probe)| (probe_index, probe.density_level))
        {
            return Err(invalid_data(format!(
                "octahedral sh volume probe {probe_index} density_level {density_level} is reserved for adaptive density; v9 requires 0"
            )));
        }

        let valid_probe_count = valid_probe_count(&probes);
        validate_compact_atlas_geometry(
            valid_probe_count,
            tile_dimension,
            compact_atlas_dimensions,
            compact_atlas_layer_count,
            compact_atlas_tiles_per_layer,
            compact_atlas_tiles_per_row,
        )?;
        let expected_payload_len = expected_compact_atlas_len(
            irradiance_format,
            compact_atlas_dimensions,
            compact_atlas_layer_count,
        )?;
        if compact_atlas_len != expected_payload_len {
            return Err(invalid_data(format!(
                "octahedral sh volume compact_atlas_len {compact_atlas_len}, expected {expected_payload_len} for irradiance_format {irradiance_format} over {valid_probe_count} metadata-valid probe(s)"
            )));
        }
        let compact_atlas_end = o.checked_add(compact_atlas_len).ok_or_else(|| {
            invalid_data("octahedral sh volume compact atlas byte range overflow".to_string())
        })?;
        if data.len() < compact_atlas_end {
            return Err(truncated("compact atlas blob"));
        }
        let compact_atlas = data[o..compact_atlas_end].to_vec();
        o = compact_atlas_end;

        let (animation_descriptors, after_anim) =
            read_animation_descriptors(data, o, animated_light_count)?;
        let (slot_for_map_light, after_slots) = read_slot_table(data, after_anim)?;
        if after_slots != data.len() {
            return Err(invalid_data(format!(
                "octahedral sh volume has {} trailing byte(s) after the map-light slot table",
                data.len() - after_slots,
            )));
        }

        Ok(Self {
            grid_origin,
            cell_size,
            grid_dimensions,
            probe_stride,
            tile_dimension,
            tile_border,
            atlas_dimensions,
            layer_count,
            tiles_per_layer,
            atlas_tiles_per_row,
            probes,
            compact_atlas_dimensions,
            compact_atlas_tiles_per_row,
            compact_atlas_tiles_per_layer,
            compact_atlas_layer_count,
            irradiance_format,
            compact_atlas,
            animation_descriptors,
            slot_for_map_light,
        })
    }
}

fn validate_octahedral_tile_geometry(tile_dimension: u32, tile_border: u32) -> crate::Result<()> {
    // The header stores N so a re-bake can change tile resolution without a
    // format break; reject only what *this runtime* cannot sample yet.
    if tile_dimension != RUNTIME_SUPPORTED_TILE_DIMENSION {
        return Err(invalid_data(format!(
            "octahedral sh volume tile_dimension {tile_dimension} is not supported by this runtime, which is pinned to N={RUNTIME_SUPPORTED_TILE_DIMENSION}"
        )));
    }
    if tile_border != DEFAULT_IRRADIANCE_TILE_BORDER {
        return Err(invalid_data(format!(
            "octahedral sh volume tile_border {tile_border}, expected {DEFAULT_IRRADIANCE_TILE_BORDER}"
        )));
    }
    if tile_dimension <= tile_border.saturating_mul(2) {
        return Err(invalid_data(format!(
            "octahedral sh volume tile_dimension {tile_dimension} leaves no interior texels with border {tile_border}"
        )));
    }
    Ok(())
}

fn validate_octahedral_grid_and_atlas(
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
                "octahedral sh volume grid_dimensions {grid_dimensions:?} are malformed: empty grids must be [0, 0, 0]"
            )));
        }
        if atlas_dimensions != [0, 0]
            || layer_count != 0
            || tiles_per_layer != 0
            || atlas_tiles_per_row != 0
        {
            return Err(invalid_data(format!(
                "octahedral sh volume empty grid must use atlas_dimensions [0, 0], layer_count 0, tiles_per_layer 0, and atlas_tiles_per_row 0, got atlas_dimensions {atlas_dimensions:?}, layer_count {layer_count}, tiles_per_layer {tiles_per_layer}, atlas_tiles_per_row {atlas_tiles_per_row}"
            )));
        }
        return Ok(());
    }

    let layout = expected_array_layout(grid_dimensions, tile_dimension, atlas_dimensions).ok_or_else(|| {
        invalid_data(format!(
            "octahedral sh volume grid_dimensions {grid_dimensions:?}, tile_dimension {tile_dimension}, and atlas_dimensions {atlas_dimensions:?} do not describe a valid layer-aware atlas layout"
        ))
    })?;
    let expected_atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    if atlas_dimensions != expected_atlas_dimensions {
        return Err(invalid_data(format!(
            "octahedral sh volume atlas_dimensions {atlas_dimensions:?}, expected {expected_atlas_dimensions:?} for grid_dimensions {grid_dimensions:?}, tile_dimension {tile_dimension}"
        )));
    }
    if layer_count != layout.layer_count {
        return Err(invalid_data(format!(
            "octahedral sh volume layer_count {layer_count}, expected {} for grid_dimensions {grid_dimensions:?}, atlas_dimensions {atlas_dimensions:?}",
            layout.layer_count
        )));
    }
    if tiles_per_layer != layout.tiles_per_layer {
        return Err(invalid_data(format!(
            "octahedral sh volume tiles_per_layer {tiles_per_layer}, expected {} for grid_dimensions {grid_dimensions:?}, atlas_dimensions {atlas_dimensions:?}",
            layout.tiles_per_layer
        )));
    }
    if atlas_tiles_per_row != layout.atlas_tiles_per_row {
        return Err(invalid_data(format!(
            "octahedral sh volume atlas_tiles_per_row {atlas_tiles_per_row}, expected {} for grid_dimensions {grid_dimensions:?}, atlas_dimensions {atlas_dimensions:?}",
            layout.atlas_tiles_per_row
        )));
    }
    Ok(())
}

fn expected_array_layout(
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
) -> Option<crate::octahedral::IrradianceAtlasArrayLayout> {
    let max_dim = atlas_dimensions[0].max(atlas_dimensions[1]);
    irradiance_atlas_array_layout(grid_dimensions, tile_dimension, max_dim)
}

fn valid_probe_count(probes: &[OctahedralShProbe]) -> usize {
    probes.iter().filter(|probe| probe.validity != 0).count()
}

fn expected_compact_array_layout(
    valid_probe_count: usize,
    tile_dimension: u32,
    compact_atlas_dimensions: [u32; 2],
) -> Option<crate::octahedral::IrradianceAtlasArrayLayout> {
    let valid_probe_count = u32::try_from(valid_probe_count).ok()?;
    let max_dim = compact_atlas_dimensions[0].max(compact_atlas_dimensions[1]);
    irradiance_atlas_array_layout([valid_probe_count, 1, 1], tile_dimension, max_dim)
}

fn validate_irradiance_format(irradiance_format: u32) -> crate::Result<()> {
    match irradiance_format {
        IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F => Ok(()),
        _ => Err(invalid_data(format!(
            "octahedral sh volume irradiance_format {irradiance_format} is not a known tag \
             (expected {IRRADIANCE_FORMAT_BC6H} BC6H or {IRRADIANCE_FORMAT_RGBA16F} RGBA16F)"
        ))),
    }
}

fn validate_compact_atlas_geometry(
    valid_probe_count: usize,
    tile_dimension: u32,
    compact_atlas_dimensions: [u32; 2],
    compact_atlas_layer_count: u32,
    compact_atlas_tiles_per_layer: u32,
    compact_atlas_tiles_per_row: u32,
) -> crate::Result<()> {
    let layout = expected_compact_array_layout(
        valid_probe_count,
        tile_dimension,
        compact_atlas_dimensions,
    )
    .ok_or_else(|| {
        invalid_data(format!(
            "octahedral sh volume compact_atlas geometry cannot be derived for {valid_probe_count} metadata-valid probe(s), tile_dimension {tile_dimension}, and compact_atlas_dimensions {compact_atlas_dimensions:?}"
        ))
    })?;
    let expected_dimensions = [layout.atlas_width, layout.atlas_height];
    if compact_atlas_dimensions != expected_dimensions
        || compact_atlas_layer_count != layout.layer_count
        || compact_atlas_tiles_per_layer != layout.tiles_per_layer
        || compact_atlas_tiles_per_row != layout.atlas_tiles_per_row
    {
        return Err(invalid_data(format!(
            "octahedral sh volume compact_atlas geometry {compact_atlas_dimensions:?}, tiles_per_row {compact_atlas_tiles_per_row}, tiles_per_layer {compact_atlas_tiles_per_layer}, layer_count {compact_atlas_layer_count}; expected dimensions {expected_dimensions:?}, tiles_per_row {}, tiles_per_layer {}, layer_count {} for {valid_probe_count} metadata-valid probe(s)",
            layout.atlas_tiles_per_row, layout.tiles_per_layer, layout.layer_count,
        )));
    }
    Ok(())
}

fn expected_compact_atlas_len(
    irradiance_format: u32,
    compact_atlas_dimensions: [u32; 2],
    compact_atlas_layer_count: u32,
) -> crate::Result<usize> {
    let layer_count = compact_atlas_layer_count as usize;
    let width = compact_atlas_dimensions[0] as usize;
    let height = compact_atlas_dimensions[1] as usize;

    match irradiance_format {
        IRRADIANCE_FORMAT_BC6H => checked_len(
            &[
                layer_count,
                compact_atlas_dimensions[0].div_ceil(4) as usize,
                compact_atlas_dimensions[1].div_ceil(4) as usize,
                16,
            ],
            "octahedral sh volume compact BC6H atlas byte length overflows usize",
        ),
        IRRADIANCE_FORMAT_RGBA16F => checked_len(
            &[
                layer_count,
                width,
                height,
                OCTAHEDRAL_ATLAS_TEXEL_STRIDE as usize,
            ],
            "octahedral sh volume compact RGBA16F atlas byte length overflows usize",
        ),
        _ => Err(invalid_data(format!(
            "octahedral sh volume irradiance_format {irradiance_format} is not a known tag \
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

fn write_animation_descriptors(buf: &mut Vec<u8>, descriptors: &[AnimationDescriptor]) {
    for desc in descriptors {
        buf.extend_from_slice(&desc.period.to_le_bytes());
        buf.extend_from_slice(&desc.phase.to_le_bytes());
        for c in &desc.base_color {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&(desc.brightness.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(desc.color.len() as u32).to_le_bytes());
        buf.extend_from_slice(&desc.start_active.to_le_bytes());
        buf.extend_from_slice(&(desc.direction.len() as u32).to_le_bytes());
        for b in &desc.brightness {
            buf.extend_from_slice(&b.to_le_bytes());
        }
        for c in &desc.color {
            for ch in c {
                buf.extend_from_slice(&ch.to_le_bytes());
            }
        }
        for d in &desc.direction {
            for ch in d {
                buf.extend_from_slice(&ch.to_le_bytes());
            }
        }
    }
}

fn read_animation_descriptors(
    data: &[u8],
    mut o: usize,
    animated_light_count: usize,
) -> crate::Result<(Vec<AnimationDescriptor>, usize)> {
    let mut animation_descriptors = Vec::with_capacity(animated_light_count);
    for _ in 0..animated_light_count {
        if data.len() < o + 20 {
            return Err(truncated("animation descriptor header"));
        }
        let period = read_f32(data, o);
        let phase = read_f32(data, o + 4);
        let base_color = [
            read_f32(data, o + 8),
            read_f32(data, o + 12),
            read_f32(data, o + 16),
        ];
        o += 20;

        if data.len() < o + 16 {
            return Err(truncated("animation descriptor sample counts"));
        }
        let brightness_count = read_u32(data, o) as usize;
        let color_count = read_u32(data, o + 4) as usize;
        let start_active = read_u32(data, o + 8);
        let direction_count = read_u32(data, o + 12) as usize;
        o += 16;

        let brightness_bytes = brightness_count * 4;
        let color_bytes = color_count * 12;
        let direction_bytes = direction_count * 12;
        if data.len() < o + brightness_bytes + color_bytes + direction_bytes {
            return Err(truncated("animation descriptor samples"));
        }

        let mut brightness = Vec::with_capacity(brightness_count);
        for i in 0..brightness_count {
            brightness.push(read_f32(data, o + i * 4));
        }
        o += brightness_bytes;

        let mut color = Vec::with_capacity(color_count);
        for i in 0..color_count {
            color.push([
                read_f32(data, o + i * 12),
                read_f32(data, o + i * 12 + 4),
                read_f32(data, o + i * 12 + 8),
            ]);
        }
        o += color_bytes;

        let mut direction = Vec::with_capacity(direction_count);
        for i in 0..direction_count {
            direction.push([
                read_f32(data, o + i * 12),
                read_f32(data, o + i * 12 + 4),
                read_f32(data, o + i * 12 + 8),
            ]);
        }
        o += direction_bytes;

        animation_descriptors.push(AnimationDescriptor {
            period,
            phase,
            base_color,
            brightness,
            color,
            direction,
            start_active,
        });
    }
    Ok((animation_descriptors, o))
}

fn write_slot_table(buf: &mut Vec<u8>, slots: &[u32]) {
    buf.extend_from_slice(&(slots.len() as u32).to_le_bytes());
    for slot in slots {
        buf.extend_from_slice(&slot.to_le_bytes());
    }
}

fn read_slot_table(data: &[u8], mut o: usize) -> crate::Result<(Vec<u32>, usize)> {
    if data.len() < o + 4 {
        return Ok((Vec::new(), o));
    }
    let map_light_count = read_u32(data, o) as usize;
    o += 4;
    if data.len() < o + map_light_count * 4 {
        return Err(truncated("map-light slot table"));
    }
    let mut slots = Vec::with_capacity(map_light_count);
    for i in 0..map_light_count {
        slots.push(read_u32(data, o + i * 4));
    }
    o += map_light_count * 4;
    Ok((slots, o))
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("sh volume section truncated: {what}"),
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

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lightmap::f32_to_f16_bits;
    use crate::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION;

    fn oct_section(grid: [u32; 3]) -> OctahedralShVolumeSection {
        oct_section_with_max_dim(grid, 8192)
    }

    fn oct_section_with_max_dim(grid: [u32; 3], max_dim: u32) -> OctahedralShVolumeSection {
        oct_section_with_format(grid, max_dim, IRRADIANCE_FORMAT_BC6H)
    }

    fn oct_section_with_format(
        grid: [u32; 3],
        max_dim: u32,
        irradiance_format: u32,
    ) -> OctahedralShVolumeSection {
        let total = (grid[0] * grid[1] * grid[2]) as usize;
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let tile_border = DEFAULT_IRRADIANCE_TILE_BORDER;
        let layout = irradiance_atlas_array_layout(grid, tile_dimension, max_dim).unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        let probes: Vec<_> = (0..total)
            .map(|i| OctahedralShProbe {
                validity: (i % 2) as u8,
                mean_distance: f32_to_f16_bits(i as f32 + 0.5),
                mean_sq_distance: f32_to_f16_bits(i as f32 + 1.0),
                density_level: 0,
            })
            .collect();
        let valid_probe_count = valid_probe_count(&probes);
        let compact_layout = irradiance_atlas_array_layout(
            [valid_probe_count as u32, 1, 1],
            tile_dimension,
            max_dim,
        )
        .unwrap();
        let compact_atlas_dimensions = [compact_layout.atlas_width, compact_layout.atlas_height];
        let compact_atlas_len = expected_compact_atlas_len(
            irradiance_format,
            compact_atlas_dimensions,
            compact_layout.layer_count,
        )
        .unwrap();
        OctahedralShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension,
            tile_border,
            atlas_dimensions,
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            compact_atlas_dimensions,
            compact_atlas_tiles_per_row: compact_layout.atlas_tiles_per_row,
            compact_atlas_tiles_per_layer: compact_layout.tiles_per_layer,
            compact_atlas_layer_count: compact_layout.layer_count,
            irradiance_format,
            compact_atlas: (0..compact_atlas_len).map(|i| (i % 256) as u8).collect(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn octahedral_round_trip_preserves_dense_metadata_and_compact_bc6h_atlas() {
        let section = oct_section([2, 2, 1]);
        assert_eq!(section.layer_count, 1);
        assert_eq!(section.compact_atlas_dimensions, [12, 6]);
        assert_eq!(section.compact_atlas_layer_count, 1);
        assert_eq!(section.compact_atlas_tiles_per_layer, 2);
        assert_eq!(section.compact_atlas_tiles_per_row, 2);
        let bytes = section.to_bytes();
        assert_eq!(
            &bytes[64..68],
            section.atlas_tiles_per_row.to_le_bytes().as_slice()
        );
        assert_eq!(&bytes[68..72], section.layer_count.to_le_bytes().as_slice());
        assert_eq!(
            &bytes[72..76],
            section.tiles_per_layer.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[76..80],
            section.compact_atlas_dimensions[0].to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[80..84],
            section.compact_atlas_dimensions[1].to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[84..88],
            section.compact_atlas_tiles_per_row.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[88..92],
            section
                .compact_atlas_tiles_per_layer
                .to_le_bytes()
                .as_slice()
        );
        assert_eq!(
            &bytes[92..96],
            section.compact_atlas_layer_count.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[96..100],
            IRRADIANCE_FORMAT_BC6H.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[100..104],
            (section.compact_atlas.len() as u32)
                .to_le_bytes()
                .as_slice()
        );
        let expected_len =
            OctahedralShVolumeSection::HEADER_SIZE + 4 * OCTAHEDRAL_PROBE_STRIDE as usize + 96 + 4;
        assert_eq!(bytes.len(), expected_len);

        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn octahedral_round_trip_preserves_multi_layer_compact_geometry() {
        let section = oct_section_with_max_dim([20, 1, 1], 20);
        assert_eq!(section.layer_count, 3);
        assert_eq!(section.tiles_per_layer, 9);
        assert_eq!(section.atlas_tiles_per_row, 3);
        assert_eq!(section.atlas_dimensions, [18, 18]);
        assert_eq!(section.compact_atlas_layer_count, 2);
        assert_eq!(section.compact_atlas_tiles_per_layer, 9);
        assert_eq!(section.compact_atlas_tiles_per_row, 3);
        assert_eq!(section.compact_atlas_dimensions, [18, 18]);

        let bytes = section.to_bytes();
        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn octahedral_round_trip_preserves_uncompressed_compact_atlas_bits() {
        let section = oct_section_with_format([3, 2, 1], 8192, IRRADIANCE_FORMAT_RGBA16F);
        let bytes = section.to_bytes();
        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
        assert_eq!(restored.compact_atlas, section.compact_atlas);
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn octahedral_empty_volume_round_trips() {
        let section = OctahedralShVolumeSection::placeholder();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), OctahedralShVolumeSection::HEADER_SIZE + 4);
        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn octahedral_round_trip_serializes_near_square_tiles_per_row() {
        let section = oct_section([3, 2, 4]);
        assert_eq!(section.atlas_tiles_per_row, 5);
        assert_eq!(section.atlas_dimensions, [30, 30]);
        assert_eq!(section.layer_count, 1);
        assert_eq!(section.tiles_per_layer, 25);

        let restored = OctahedralShVolumeSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.atlas_tiles_per_row, 5);
        assert_eq!(restored.atlas_dimensions, [30, 30]);
        assert_eq!(restored.layer_count, 1);
        assert_eq!(restored.tiles_per_layer, 25);
    }

    #[test]
    fn octahedral_metadata_keeps_eight_byte_stride_with_zero_density_level() {
        let section = oct_section([1, 1, 1]);
        let bytes = section.to_bytes();
        let metadata = &bytes[OctahedralShVolumeSection::HEADER_SIZE
            ..OctahedralShVolumeSection::HEADER_SIZE + OCTAHEDRAL_PROBE_STRIDE as usize];

        assert_eq!(metadata.len(), OCTAHEDRAL_PROBE_STRIDE as usize);
        assert_eq!(metadata[5], 0, "v9 density_level must serialize as zero");
        assert_eq!(&metadata[6..8], &[0, 0]);
    }

    #[test]
    fn octahedral_rejects_unknown_compact_atlas_format_tag() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[96..100].copy_from_slice(&7u32.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("irradiance_format") && msg.contains("known tag"),
            "expected unknown-format-tag error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_nonzero_density_level_reserved_for_adaptive_density() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[OctahedralShVolumeSection::HEADER_SIZE + 5] = 1;

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("density_level") && msg.contains("reserved for adaptive density"),
            "expected reserved-density-level error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_compact_payload_length_mismatched_to_format_tag() {
        let section = oct_section([2, 2, 1]);
        let mut bytes = section.to_bytes();
        // The payload remains a BC6H-sized blob, but the known RGBA16F tag has
        // a different exact length identity for the same valid-probe count.
        bytes[96..100].copy_from_slice(&IRRADIANCE_FORMAT_RGBA16F.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("compact_atlas_len") && msg.contains("metadata-valid probe"),
            "expected compact-payload-length error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_compact_geometry_mismatched_to_valid_probe_count() {
        let section = oct_section([2, 2, 1]);
        let mut bytes = section.to_bytes();
        bytes[84..88].copy_from_slice(&1u32.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("compact_atlas geometry") && msg.contains("metadata-valid probe"),
            "expected compact-geometry error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_malformed_tiles_per_row() {
        let section = oct_section([3, 2, 4]);
        let mut bytes = section.to_bytes();
        bytes[64..68].copy_from_slice(&3u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("atlas_tiles_per_row"),
            "expected tiles-per-row error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_runtime_unsupported_tile_dimension() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // tile_dimension is the u32 after version[0..4] origin[4..16]
        // cell[16..28] dims[28..40] probe_stride[40..44] animated_count[44..48].
        bytes[48..52].copy_from_slice(&8u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        // A re-baked atlas at a different N is a format-legal value the runtime
        // cannot yet sample — the error must read as a capability limit.
        assert!(
            msg.contains("tile_dimension") && msg.contains("not supported by this runtime"),
            "expected runtime-capability tile-dimension error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_z_stacked_atlas_dimensions() {
        let section = oct_section([3, 2, 4]);
        let mut bytes = section.to_bytes();
        bytes[56..60].copy_from_slice(&18u32.to_le_bytes());
        bytes[60..64].copy_from_slice(&48u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("atlas_dimensions"),
            "expected atlas-dimensions error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_partial_empty_grid() {
        let section = OctahedralShVolumeSection::placeholder();
        let mut bytes = section.to_bytes();
        bytes[28..32].copy_from_slice(&0u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&2u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&1u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty grids must be [0, 0, 0]"),
            "expected partial-empty-grid error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_extra_trailing_bytes() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes.push(99);
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trailing byte"),
            "expected trailing-byte error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_previous_section_version() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[0..4].copy_from_slice(&7u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version 7")
                && msg.contains("expected 9")
                && msg.contains("recompile")
                && msg.contains("v9 compact-atlas format"),
            "expected version-mismatch error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_section_id_is_thirty_four() {
        use crate::SectionId;

        assert_eq!(SectionId::OctahedralShVolume as u32, 34);
        assert_eq!(SectionId::from_u32(34), Some(SectionId::OctahedralShVolume));
    }

    /// Loader-side degradation contract: a PRL with the ShVolume section
    /// absent from its section table must read without error and yield
    /// `None` for the section lookup. This matches the spec's "missing
    /// section is not an error" rule for the SH volume.
    #[test]
    fn prl_container_returns_none_for_missing_sh_volume_section() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};

        // Pack a single unrelated section — no ShVolume — and read back.
        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0xAA, 0xBB, 0xCC],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = std::io::Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        assert!(meta.find_section(SectionId::ShVolume as u32).is_none());
        let result = read_section_data(&mut cursor, &meta, SectionId::ShVolume as u32).unwrap();
        assert!(result.is_none(), "missing SH volume must return None");
    }
}
