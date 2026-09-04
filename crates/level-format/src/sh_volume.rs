// OctahedralShVolume section (id 34): the live baked-irradiance section.
//
// See: context/lib/build_pipeline.md

use crate::FormatError;
use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, RUNTIME_SUPPORTED_TILE_DIMENSION, irradiance_atlas_array_layout,
};
use crate::sh_reconstruct::{Level, StoredBrickPrefixSum, corner_locals, stored_brick_prefix_sum};

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
/// 9 — the base atlas became a valid-probe-only compact payload with its own
/// geometry and a BC6H/RGBA16F format tag. Version 10 replaces the two v9 atlas
/// geometry blocks with one metadata-derived stored-tile geometry: L0 stores
/// valid probes, L1 reserves all corners, and L2 stores one brick mean.
pub const SH_VOLUME_VERSION: u32 = 10;

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
    /// Per-affinity-brick storage level: 0 = L0, 1 = L1, 2 = L2. v10 requires
    /// one shared value per full 4×4×4 brick and L0 on partial edge bricks.
    pub density_level: u8,
}

/// One `Rgba16Float` atlas texel, stored as raw f16 channel bits.
///
/// This remains the compiler's uncompressed tile-packing representation. The
/// v10 section payload itself is the tagged byte blob on
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
///   Header (84 bytes):
///     u32      version                (= SH_VOLUME_VERSION)
///     f32 × 3  grid_origin
///     f32 × 3  cell_size
///     u32 × 3  grid_dimensions
///     u32      probe_stride           (= OCTAHEDRAL_PROBE_STRIDE = 8)
///     u32      animated_light_count
///     u32      tile_dimension         (default 6, border included)
///     u32      tile_border            (default 1)
///     u32      atlas_width            (stored-tile atlas, per-layer texels)
///     u32      atlas_height           (stored-tile atlas, per-layer texels)
///     u32      atlas_tiles_per_row    (per-layer tile columns)
///     u32      layer_count            (2D array layers)
///     u32      tiles_per_layer        (whole probe tiles per layer)
///     u32      irradiance_format       (BC6H / RGBA16F tag from `lightmap`)
///     u32      compact_atlas_len       (byte length of stored atlas blob)
///
///   Probe metadata records (probe_stride bytes each, x-fastest order):
///     u8       validity
///     f16      mean_distance          (E[d])
///     f16      mean_sq_distance       (E[d²])
///     u8       density_level          (0 = L0, 1 = L1, 2 = L2)
///     u8 × 2   padding
///
///   Compact atlas blob (compact_atlas_len bytes), carrying the stored set in
///   brick-major order (affinity bricks x-fastest): L0 valid probes in local
///   x-fastest order; L1 all corners in `corner_locals` order with zero tiles
///   for invalid corners; L2 one valid-probe mean; all-invalid bricks none.
///     IRRADIANCE_FORMAT_RGBA16F: layer-major, then row-major
///                                atlas_width × atlas_height
///                                `f16 × 4` texels per layer.
///     IRRADIANCE_FORMAT_BC6H:    layer-major 4×4 `Bc6hRgbUfloat` blocks,
///                                `layer_count ×
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
    /// Format tag for `compact_atlas`: `IRRADIANCE_FORMAT_BC6H` (default at
    /// rest) or `IRRADIANCE_FORMAT_RGBA16F` (uncompressed debug path), over
    /// the metadata-derived stored-tile geometry above.
    pub irradiance_format: u32,
    /// Raw stored-atlas bytes in the encoding named by `irradiance_format`.
    pub compact_atlas: Vec<u8>,
    pub animation_descriptors: Vec<AnimationDescriptor>,
    pub slot_for_map_light: Vec<u32>,
}

impl OctahedralShVolumeSection {
    pub const HEADER_SIZE: usize = 84;

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
        self.try_to_bytes()
            .expect("OctahedralShVolumeSection must satisfy its wire contract")
    }

    /// Encode only a canonical section whose stored payload fits the v10
    /// `u32` byte-length field.
    pub fn try_to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate_wire_contract()?;

        let total_probes = checked_total_probe_count(self.grid_dimensions)?;
        let compact_atlas_len = compact_atlas_len_for_header(self.compact_atlas.len() as u64)?;

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
        buf.extend_from_slice(&self.irradiance_format.to_le_bytes());
        buf.extend_from_slice(&compact_atlas_len.to_le_bytes());

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
        Ok(buf)
    }

    fn validate_wire_contract(&self) -> crate::Result<()> {
        if self.probe_stride != OCTAHEDRAL_PROBE_STRIDE {
            return Err(invalid_data(format!(
                "octahedral sh volume probe_stride {}, expected exactly {OCTAHEDRAL_PROBE_STRIDE} for v10",
                self.probe_stride,
            )));
        }

        validate_octahedral_tile_geometry(self.tile_dimension, self.tile_border)?;
        let stored_prefix = validate_probe_metadata(self.grid_dimensions, &self.probes)?;
        validate_stored_atlas_geometry(
            stored_prefix.total_stored_tiles,
            self.tile_dimension,
            self.atlas_dimensions,
            self.layer_count,
            self.tiles_per_layer,
            self.atlas_tiles_per_row,
        )?;

        validate_irradiance_format(self.irradiance_format)?;
        let expected_payload_len = expected_compact_atlas_len(
            self.irradiance_format,
            self.atlas_dimensions,
            self.layer_count,
        )?;
        if self.compact_atlas.len() != expected_payload_len {
            return Err(invalid_data(format!(
                "octahedral sh volume compact_atlas length {}, expected {expected_payload_len} for irradiance_format {} over {} metadata-derived stored tile(s)",
                self.compact_atlas.len(),
                self.irradiance_format,
                stored_prefix.total_stored_tiles,
            )));
        }
        compact_atlas_len_for_header(self.compact_atlas.len() as u64)?;
        Ok(())
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
                     recompile the .prl with the current `prl-build` for the v10 stored-atlas format"
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
        let irradiance_format = read_u32(data, o);
        o += 4;
        let compact_atlas_len = read_u32(data, o) as usize;
        o += 4;
        debug_assert_eq!(o, Self::HEADER_SIZE);

        if probe_stride != OCTAHEDRAL_PROBE_STRIDE {
            return Err(invalid_data(format!(
                "octahedral sh volume probe_stride {probe_stride}, expected exactly {OCTAHEDRAL_PROBE_STRIDE} for v10"
            )));
        }

        validate_irradiance_format(irradiance_format)?;

        validate_octahedral_tile_geometry(tile_dimension, tile_border)?;
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

        let stored_prefix = validate_probe_metadata(grid_dimensions, &probes)?;
        validate_stored_atlas_geometry(
            stored_prefix.total_stored_tiles,
            tile_dimension,
            atlas_dimensions,
            layer_count,
            tiles_per_layer,
            atlas_tiles_per_row,
        )?;
        let expected_payload_len =
            expected_compact_atlas_len(irradiance_format, atlas_dimensions, layer_count)?;
        if compact_atlas_len != expected_payload_len {
            return Err(invalid_data(format!(
                "octahedral sh volume compact_atlas_len {compact_atlas_len}, expected {expected_payload_len} for irradiance_format {irradiance_format} over {} metadata-derived stored tile(s)",
                stored_prefix.total_stored_tiles,
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

fn validate_grid_dimensions(grid_dimensions: [u32; 3]) -> crate::Result<()> {
    let zero_axes = grid_dimensions.iter().filter(|&&d| d == 0).count();
    if zero_axes > 0 {
        if zero_axes != 3 {
            return Err(invalid_data(format!(
                "octahedral sh volume grid_dimensions {grid_dimensions:?} are malformed: empty grids must be [0, 0, 0]"
            )));
        }
    }
    Ok(())
}

fn validate_stored_atlas_geometry(
    stored_tile_count: u32,
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    tiles_per_layer: u32,
    atlas_tiles_per_row: u32,
) -> crate::Result<()> {
    let layout = expected_stored_array_layout(stored_tile_count, tile_dimension, atlas_dimensions)
        .ok_or_else(|| {
            invalid_data(format!(
                "octahedral sh volume stored atlas geometry cannot be derived for {stored_tile_count} metadata-derived tile(s), tile_dimension {tile_dimension}, and atlas_dimensions {atlas_dimensions:?}"
            ))
        })?;

    let expected_atlas_dimensions = [layout.atlas_width, layout.atlas_height];
    if atlas_dimensions != expected_atlas_dimensions
        || layer_count != layout.layer_count
        || tiles_per_layer != layout.tiles_per_layer
        || atlas_tiles_per_row != layout.atlas_tiles_per_row
    {
        return Err(invalid_data(format!(
            "octahedral sh volume stored atlas geometry {atlas_dimensions:?}, tiles_per_row {atlas_tiles_per_row}, tiles_per_layer {tiles_per_layer}, layer_count {layer_count}; expected dimensions {expected_atlas_dimensions:?}, tiles_per_row {}, tiles_per_layer {}, layer_count {} for {stored_tile_count} metadata-derived tile(s)",
            layout.atlas_tiles_per_row, layout.tiles_per_layer, layout.layer_count,
        )));
    }
    Ok(())
}

fn expected_stored_array_layout(
    stored_tile_count: u32,
    tile_dimension: u32,
    atlas_dimensions: [u32; 2],
) -> Option<crate::octahedral::IrradianceAtlasArrayLayout> {
    let max_dim = atlas_dimensions[0].max(atlas_dimensions[1]);
    irradiance_atlas_array_layout([stored_tile_count, 1, 1], tile_dimension, max_dim)
}

fn checked_total_probe_count(grid_dimensions: [u32; 3]) -> crate::Result<usize> {
    (grid_dimensions[0] as usize)
        .checked_mul(grid_dimensions[1] as usize)
        .and_then(|count| count.checked_mul(grid_dimensions[2] as usize))
        .ok_or_else(|| {
            invalid_data(format!(
                "octahedral sh volume grid_dimensions {grid_dimensions:?} overflow"
            ))
        })
}

fn validate_probe_metadata(
    grid_dimensions: [u32; 3],
    probes: &[OctahedralShProbe],
) -> crate::Result<StoredBrickPrefixSum> {
    validate_grid_dimensions(grid_dimensions)?;
    let total_probes = checked_total_probe_count(grid_dimensions)?;
    if probes.len() != total_probes {
        return Err(invalid_data(format!(
            "octahedral sh volume has {} probe metadata record(s), expected {total_probes} for grid_dimensions {:?}",
            probes.len(),
            grid_dimensions,
        )));
    }

    let affinity_dimensions = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    let brick_count = affinity_dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })
        .ok_or_else(|| {
            invalid_data(format!(
                "octahedral sh volume affinity dimensions {affinity_dimensions:?} overflow"
            ))
        })?;
    let mut brick_levels = Vec::with_capacity(brick_count);

    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let brick_index = brick_x
                    + brick_y * affinity_dimensions[0] as usize
                    + brick_z * affinity_dimensions[0] as usize * affinity_dimensions[1] as usize;
                let origin = [brick_x * 4, brick_y * 4, brick_z * 4];
                let full_brick = origin[0] + 4 <= grid_dimensions[0] as usize
                    && origin[1] + 4 <= grid_dimensions[1] as usize
                    && origin[2] + 4 <= grid_dimensions[2] as usize;
                let mut brick_level = None;
                let mut has_valid_corner = false;

                for local_z in 0..4usize {
                    for local_y in 0..4usize {
                        for local_x in 0..4usize {
                            let probe_x = origin[0] + local_x;
                            let probe_y = origin[1] + local_y;
                            let probe_z = origin[2] + local_z;
                            if probe_x >= grid_dimensions[0] as usize
                                || probe_y >= grid_dimensions[1] as usize
                                || probe_z >= grid_dimensions[2] as usize
                            {
                                continue;
                            }
                            let probe_index = probe_x
                                + probe_y * grid_dimensions[0] as usize
                                + probe_z
                                    * grid_dimensions[0] as usize
                                    * grid_dimensions[1] as usize;
                            let probe = probes[probe_index];
                            let level = Level::from_u8(probe.density_level).ok_or_else(|| {
                                invalid_data(format!(
                                    "octahedral sh volume probe {probe_index} density_level {} out of range: level must be 0..=2",
                                    probe.density_level
                                ))
                            })?;
                            if !full_brick && level != Level::L0 {
                                return Err(invalid_data(format!(
                                    "octahedral sh volume partial affinity brick {brick_index} density_level {} must be 0",
                                    probe.density_level
                                )));
                            }
                            if let Some(previous_level) = brick_level {
                                if full_brick && level != previous_level {
                                    return Err(invalid_data(format!(
                                        "octahedral sh volume affinity brick {brick_index} has disagreeing density_level values: {} and {}",
                                        previous_level.to_u8(),
                                        level.to_u8(),
                                    )));
                                }
                            } else {
                                brick_level = Some(level);
                            }

                            let local = local_x + local_y * 4 + local_z * 16;
                            if level == Level::L1
                                && corner_locals().contains(&local)
                                && probe.validity != 0
                            {
                                has_valid_corner = true;
                            }
                        }
                    }
                }

                let level = brick_level.expect("non-empty affinity brick has at least one probe");
                if level == Level::L1 && !has_valid_corner {
                    return Err(invalid_data(format!(
                        "octahedral sh volume affinity brick {brick_index} uses L1 without a valid corner probe"
                    )));
                }
                brick_levels.push(level);
            }
        }
    }

    let probe_validity: Vec<bool> = probes.iter().map(|probe| probe.validity != 0).collect();
    stored_brick_prefix_sum(grid_dimensions, &brick_levels, &probe_validity).ok_or_else(|| {
        invalid_data(format!(
            "octahedral sh volume stored-tile prefix sum over grid_dimensions {grid_dimensions:?} overflowed"
        ))
    })
}

/// Check the I2 storage ceiling against one parsed delta section's CSR shape.
///
/// The caller supplies a grid-matched delta section's `cell_levels` and
/// `affinity_offsets`. Only cells with at least one CSR entry constrain the
/// base stored level: `density_level <= cell_level`. This pure format-layer
/// check is shared by the compiler and loader; it does not depend on a delta
/// payload or renderer type.
pub fn validate_storage_levels_against_delta(
    grid_dimensions: [u32; 3],
    probes: &[OctahedralShProbe],
    cell_levels: &[u8],
    affinity_offsets: &[u32],
) -> crate::Result<()> {
    let prefix = validate_probe_metadata(grid_dimensions, probes)?;
    let cell_count = prefix.bricks.len();
    if cell_levels.len() != cell_count {
        return Err(invalid_data(format!(
            "octahedral sh volume delta cell_levels length {}, expected {cell_count}",
            cell_levels.len()
        )));
    }
    if affinity_offsets.len() != cell_count + 1 {
        return Err(invalid_data(format!(
            "octahedral sh volume delta affinity_offsets length {}, expected {}",
            affinity_offsets.len(),
            cell_count + 1
        )));
    }
    if affinity_offsets.first().copied() != Some(0) {
        return Err(invalid_data(
            "octahedral sh volume delta affinity_offsets[0] must be 0".into(),
        ));
    }
    for (cell, offsets) in affinity_offsets.windows(2).enumerate() {
        if offsets[0] > offsets[1] {
            return Err(invalid_data(format!(
                "octahedral sh volume delta affinity_offsets[{cell}] ({}) > affinity_offsets[{}] ({}): offsets must be non-decreasing",
                offsets[0],
                cell + 1,
                offsets[1],
            )));
        }
        let delta_level = Level::from_u8(cell_levels[cell]).ok_or_else(|| {
            invalid_data(format!(
                "octahedral sh volume delta cell_levels[{cell}] {} out of range: level must be 0..=2",
                cell_levels[cell]
            ))
        })?;
        if offsets[0] == offsets[1] {
            continue;
        }

        let brick_x = cell % prefix.affinity_dimensions[0] as usize;
        let brick_y = (cell / prefix.affinity_dimensions[0] as usize)
            % prefix.affinity_dimensions[1] as usize;
        let brick_z = cell
            / (prefix.affinity_dimensions[0] as usize * prefix.affinity_dimensions[1] as usize);
        let probe_index = brick_x * 4
            + brick_y * 4 * grid_dimensions[0] as usize
            + brick_z * 4 * grid_dimensions[0] as usize * grid_dimensions[1] as usize;
        let storage_level = Level::from_u8(probes[probe_index].density_level)
            .expect("metadata validation already checked every density level");
        if storage_level.to_u8() > delta_level.to_u8() {
            return Err(invalid_data(format!(
                "octahedral sh volume affinity brick {cell} density_level {} exceeds delta cell_level {} with {} CSR entr{}",
                storage_level.to_u8(),
                delta_level.to_u8(),
                offsets[1] - offsets[0],
                if offsets[1] - offsets[0] == 1 {
                    "y"
                } else {
                    "ies"
                },
            )));
        }
    }
    Ok(())
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

fn expected_compact_atlas_len(
    irradiance_format: u32,
    compact_atlas_dimensions: [u32; 2],
    compact_atlas_layer_count: u32,
) -> crate::Result<usize> {
    let layer_count = u64::from(compact_atlas_layer_count);
    let width = u64::from(compact_atlas_dimensions[0]);
    let height = u64::from(compact_atlas_dimensions[1]);

    let len = match irradiance_format {
        IRRADIANCE_FORMAT_BC6H => checked_len_u64(
            &[
                layer_count,
                u64::from(compact_atlas_dimensions[0].div_ceil(4)),
                u64::from(compact_atlas_dimensions[1].div_ceil(4)),
                16,
            ],
            "octahedral sh volume stored BC6H atlas byte length overflows u64",
        ),
        IRRADIANCE_FORMAT_RGBA16F => checked_len_u64(
            &[
                layer_count,
                width,
                height,
                u64::from(OCTAHEDRAL_ATLAS_TEXEL_STRIDE),
            ],
            "octahedral sh volume stored RGBA16F atlas byte length overflows u64",
        ),
        _ => Err(invalid_data(format!(
            "octahedral sh volume irradiance_format {irradiance_format} is not a known tag \
             (expected {IRRADIANCE_FORMAT_BC6H} BC6H or {IRRADIANCE_FORMAT_RGBA16F} RGBA16F)"
        ))),
    }?;
    let header_len = compact_atlas_len_for_header(len)?;
    Ok(header_len as usize)
}

fn compact_atlas_len_for_header(len: u64) -> crate::Result<u32> {
    u32::try_from(len).map_err(|_| {
        invalid_data(format!(
            "octahedral sh volume stored atlas byte length {len} exceeds the v10 u32 header maximum {}",
            u32::MAX,
        ))
    })
}

fn checked_len_u64(factors: &[u64], overflow_msg: &str) -> crate::Result<u64> {
    factors.iter().try_fold(1u64, |acc, factor| {
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
        return Err(truncated("map-light slot table"));
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
        let probes: Vec<_> = (0..total)
            .map(|i| OctahedralShProbe {
                validity: (i % 2) as u8,
                mean_distance: f32_to_f16_bits(i as f32 + 0.5),
                mean_sq_distance: f32_to_f16_bits(i as f32 + 1.0),
                density_level: 0,
            })
            .collect();
        let affinity_dimensions = grid.map(|dimension| dimension.div_ceil(4));
        let brick_count = affinity_dimensions.iter().product::<u32>() as usize;
        let levels = vec![Level::L0; brick_count];
        let validity: Vec<_> = probes.iter().map(|probe| probe.validity != 0).collect();
        let prefix = stored_brick_prefix_sum(grid, &levels, &validity).unwrap();
        let layout = irradiance_atlas_array_layout(
            [prefix.total_stored_tiles, 1, 1],
            tile_dimension,
            max_dim,
        )
        .unwrap();
        let compact_atlas_len = expected_compact_atlas_len(
            irradiance_format,
            [layout.atlas_width, layout.atlas_height],
            layout.layer_count,
        )
        .unwrap();
        OctahedralShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension,
            tile_border,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            irradiance_format,
            compact_atlas: (0..compact_atlas_len).map(|i| (i % 256) as u8).collect(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn repack_stored_geometry(section: &mut OctahedralShVolumeSection, max_dim: u32) {
        let prefix = validate_probe_metadata(section.grid_dimensions, &section.probes).unwrap();
        let layout = irradiance_atlas_array_layout(
            [prefix.total_stored_tiles, 1, 1],
            section.tile_dimension,
            max_dim,
        )
        .unwrap();
        section.atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        section.layer_count = layout.layer_count;
        section.tiles_per_layer = layout.tiles_per_layer;
        section.atlas_tiles_per_row = layout.atlas_tiles_per_row;
        section.compact_atlas = vec![
            0;
            expected_compact_atlas_len(
                section.irradiance_format,
                section.atlas_dimensions,
                section.layer_count,
            )
            .unwrap()
        ];
    }

    #[test]
    fn octahedral_round_trip_preserves_stored_metadata_and_bc6h_atlas() {
        let section = oct_section([2, 2, 1]);
        assert_eq!(section.layer_count, 1);
        assert_eq!(section.atlas_dimensions, [12, 6]);
        assert_eq!(section.tiles_per_layer, 2);
        assert_eq!(section.atlas_tiles_per_row, 2);
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
            IRRADIANCE_FORMAT_BC6H.to_le_bytes().as_slice()
        );
        assert_eq!(
            &bytes[80..84],
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
    fn octahedral_round_trip_preserves_multi_layer_stored_geometry() {
        let section = oct_section_with_max_dim([20, 1, 1], 20);
        assert_eq!(section.layer_count, 2);
        assert_eq!(section.tiles_per_layer, 9);
        assert_eq!(section.atlas_tiles_per_row, 3);
        assert_eq!(section.atlas_dimensions, [18, 18]);

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
        assert_eq!(section.atlas_tiles_per_row, 4);
        assert_eq!(section.atlas_dimensions, [24, 18]);
        assert_eq!(section.layer_count, 1);
        assert_eq!(section.tiles_per_layer, 12);

        let restored = OctahedralShVolumeSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.atlas_tiles_per_row, 4);
        assert_eq!(restored.atlas_dimensions, [24, 18]);
        assert_eq!(restored.layer_count, 1);
        assert_eq!(restored.tiles_per_layer, 12);
    }

    #[test]
    fn octahedral_metadata_keeps_eight_byte_stride_with_zero_density_level() {
        let section = oct_section([1, 1, 1]);
        let bytes = section.to_bytes();
        let metadata = &bytes[OctahedralShVolumeSection::HEADER_SIZE
            ..OctahedralShVolumeSection::HEADER_SIZE + OCTAHEDRAL_PROBE_STRIDE as usize];

        assert_eq!(metadata.len(), OCTAHEDRAL_PROBE_STRIDE as usize);
        assert_eq!(metadata[5], 0, "fixture uses the L0 density level");
        assert_eq!(&metadata[6..8], &[0, 0]);
    }

    // Regression: v9 could accept compact BC6H geometry whose byte length
    // exceeded the u32 payload-length field and would wrap during encoding.
    #[test]
    fn octahedral_rejects_compact_atlas_geometry_larger_than_u32_header_length() {
        let max_layer_tiles = (8192 / DEFAULT_IRRADIANCE_TILE_DIMENSION).pow(2);
        let valid_probe_count = max_layer_tiles as usize * 64 + 1;
        let layout = irradiance_atlas_array_layout(
            [valid_probe_count as u32, 1, 1],
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            8192,
        )
        .expect("65-layer geometry remains within the format's layer limit");
        assert_eq!(layout.layer_count, 65);

        let err = expected_compact_atlas_len(
            IRRADIANCE_FORMAT_BC6H,
            [layout.atlas_width, layout.atlas_height],
            layout.layer_count,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds the v10 u32 header maximum"),
            "expected compact-atlas header-length error, got: {msg}",
        );
    }

    // Regression: compact_atlas.len() was narrowed with `as u32`, allowing a
    // non-round-trippable section to be serialized.
    #[test]
    fn octahedral_serializer_rejects_compact_atlas_length_above_u32() {
        let err = compact_atlas_len_for_header(u64::from(u32::MAX) + 1).unwrap_err();
        assert!(
            err.to_string()
                .contains("exceeds the v10 u32 header maximum")
        );
    }

    // Regression: release serialization relied on debug assertions and could
    // emit a compact payload that disagreed with its geometry.
    #[test]
    fn octahedral_try_to_bytes_rejects_malformed_compact_payload() {
        let mut section = oct_section([2, 2, 1]);
        section.compact_atlas.push(0);

        let err = section.try_to_bytes().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("compact_atlas length") && msg.contains("expected"),
            "expected compact-payload serialization error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_unknown_compact_atlas_format_tag() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[76..80].copy_from_slice(&7u32.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("irradiance_format") && msg.contains("known tag"),
            "expected unknown-format-tag error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_out_of_range_density_level() {
        let section = oct_section([4, 4, 4]);
        let mut bytes = section.to_bytes();
        bytes[OctahedralShVolumeSection::HEADER_SIZE + 5] = 3;

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("density_level") && msg.contains("out of range"),
            "expected out-of-range density-level error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_disagreeing_full_brick_density_levels() {
        let section = oct_section([4, 4, 4]);
        let mut bytes = section.to_bytes();
        bytes[OctahedralShVolumeSection::HEADER_SIZE + 5] = Level::L1.to_u8();

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("disagreeing density_level"),
            "expected full-brick disagreement error, got: {err}",
        );
    }

    #[test]
    fn octahedral_rejects_nonzero_level_on_partial_edge_brick() {
        let section = oct_section([5, 4, 4]);
        let mut bytes = section.to_bytes();
        let partial_brick_first_probe = 4;
        bytes[OctahedralShVolumeSection::HEADER_SIZE
            + partial_brick_first_probe * OCTAHEDRAL_PROBE_STRIDE as usize
            + 5] = Level::L2.to_u8();

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("partial affinity brick")
                && err.to_string().contains("must be 0"),
            "expected partial-brick error, got: {err}",
        );
    }

    #[test]
    fn octahedral_rejects_l1_brick_without_a_valid_corner() {
        let section = oct_section([4, 4, 4]);
        let mut bytes = section.to_bytes();
        for local in 0..64 {
            let record =
                OctahedralShVolumeSection::HEADER_SIZE + local * OCTAHEDRAL_PROBE_STRIDE as usize;
            bytes[record + 5] = Level::L1.to_u8();
        }
        for local in corner_locals() {
            let record =
                OctahedralShVolumeSection::HEADER_SIZE + local * OCTAHEDRAL_PROBE_STRIDE as usize;
            bytes[record] = 0;
        }
        bytes[OctahedralShVolumeSection::HEADER_SIZE + OCTAHEDRAL_PROBE_STRIDE as usize] = 1;

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("L1 without a valid corner"),
            "expected L1-corner error, got: {err}",
        );
    }

    #[test]
    fn octahedral_round_trips_l1_stored_geometry_from_metadata_prefix_sum() {
        let mut section = oct_section([4, 4, 4]);
        for probe in &mut section.probes {
            probe.density_level = Level::L1.to_u8();
        }
        repack_stored_geometry(&mut section, 8192);

        let prefix = validate_probe_metadata(section.grid_dimensions, &section.probes).unwrap();
        assert_eq!(prefix.total_stored_tiles, 8);
        assert_eq!(section.atlas_dimensions, [18, 18]);
        assert_eq!(section.tiles_per_layer, 9);
        assert_eq!(section.atlas_tiles_per_row, 3);
        assert_eq!(
            OctahedralShVolumeSection::from_bytes(&section.to_bytes()).unwrap(),
            section
        );
    }

    #[test]
    fn storage_level_delta_validator_rejects_coarser_base_storage() {
        let mut section = oct_section([8, 4, 4]);
        for z in 0..4usize {
            for y in 0..4usize {
                for x in 4..8usize {
                    let index = x + y * 8 + z * 8 * 4;
                    section.probes[index].density_level = Level::L2.to_u8();
                }
            }
        }

        let err = validate_storage_levels_against_delta(
            section.grid_dimensions,
            &section.probes,
            &[Level::L0.to_u8(), Level::L1.to_u8()],
            &[0, 0, 1],
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("density_level 2 exceeds delta cell_level 1"),
            "expected I2 ceiling error, got: {err}",
        );

        validate_storage_levels_against_delta(
            section.grid_dimensions,
            &section.probes,
            &[Level::L0.to_u8(), Level::L2.to_u8()],
            &[0, 0, 1],
        )
        .unwrap();
    }

    #[test]
    fn octahedral_rejects_compact_payload_length_mismatched_to_format_tag() {
        let section = oct_section([2, 2, 1]);
        let mut bytes = section.to_bytes();
        // The payload remains a BC6H-sized blob, but the known RGBA16F tag has
        // a different exact length identity for the same valid-probe count.
        bytes[76..80].copy_from_slice(&IRRADIANCE_FORMAT_RGBA16F.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("compact_atlas_len") && msg.contains("metadata-derived stored tile"),
            "expected compact-payload-length error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_stored_geometry_mismatched_to_metadata() {
        let section = oct_section([2, 2, 1]);
        let mut bytes = section.to_bytes();
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stored atlas geometry") && msg.contains("metadata-derived tile"),
            "expected stored-geometry error, got: {msg}",
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
            msg.contains("stored atlas geometry"),
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

    // Regression: v9 accepted oversized probe strides and silently skipped
    // bytes that have no defined record semantics.
    #[test]
    fn octahedral_rejects_probe_stride_larger_than_v10_record() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[40..44].copy_from_slice(&(OCTAHEDRAL_PROBE_STRIDE + 4).to_le_bytes());

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("probe_stride") && msg.contains("expected exactly 8 for v10"),
            "expected exact v10 probe-stride error, got: {msg}",
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
            msg.contains("stored atlas geometry"),
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

    // Regression: omitting the trailing u32 slot-table count was interpreted
    // as an empty table instead of a truncated v9 section.
    #[test]
    fn octahedral_rejects_missing_empty_slot_table_prefix() {
        let section = OctahedralShVolumeSection::placeholder();
        let mut bytes = section.to_bytes();
        bytes.truncate(bytes.len() - 4);

        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("sh volume section truncated: map-light slot table"),
            "expected truncated slot-table error, got: {msg}",
        );
    }

    #[test]
    fn octahedral_rejects_previous_section_version() {
        let section = oct_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[0..4].copy_from_slice(&9u32.to_le_bytes());
        let err = OctahedralShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version 9")
                && msg.contains("expected 10")
                && msg.contains("recompile")
                && msg.contains("v10 stored-atlas format"),
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
