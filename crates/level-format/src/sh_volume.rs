// OctahedralShVolume section (id 34): the live baked-irradiance section.
//
// See: context/lib/build_pipeline.md

use crate::FormatError;
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
/// (current) — atlas metadata became layer-aware for 2D array texture uploads,
/// storing shared per-layer dimensions plus `layer_count` and
/// `tiles_per_layer`.
pub const SH_VOLUME_VERSION: u32 = 8;

/// Sentinel for "this map light has no animated-light section slot" in
/// `OctahedralShVolumeSection.slot_for_map_light`. Non-animated lights and any
/// light the bake excluded from the animated-baked namespace use this value.
pub const ANIMATED_SLOT_NONE: u32 = u32::MAX;

/// Byte stride of one serialized octahedral probe metadata record:
/// `u8 validity` + two f16 depth moments + 3 bytes of padding.
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
}

/// One `Rgba16Float` atlas texel, stored as raw f16 channel bits.
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
///   Header (76 bytes):
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
///   Probe metadata records (probe_stride bytes each, x-fastest order):
///     u8       validity
///     f16      mean_distance          (E[d])
///     f16      mean_sq_distance       (E[d²])
///     u8 × 3   padding
///
///   Atlas texels:
///     layer-major, then row-major atlas_width × atlas_height texels per layer
///     f16 × 4 RGBA per texel
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
    pub atlas_texels: Vec<OctahedralAtlasTexel>,
    pub animation_descriptors: Vec<AnimationDescriptor>,
    pub slot_for_map_light: Vec<u32>,
}

impl OctahedralShVolumeSection {
    pub const HEADER_SIZE: usize = 76;

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
            atlas_texels: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn total_atlas_texels(&self) -> usize {
        self.layer_count as usize
            * self.atlas_dimensions[0] as usize
            * self.atlas_dimensions[1] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let total_probes = self.total_probes();
        debug_assert_eq!(self.probes.len(), total_probes);
        debug_assert_eq!(self.atlas_texels.len(), self.total_atlas_texels());
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

        let mut buf = Vec::with_capacity(
            Self::HEADER_SIZE
                + total_probes * OCTAHEDRAL_PROBE_STRIDE as usize
                + self.atlas_texels.len() * OCTAHEDRAL_ATLAS_TEXEL_STRIDE as usize,
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

        for probe in &self.probes {
            buf.push(probe.validity);
            buf.extend_from_slice(&probe.mean_distance.to_le_bytes());
            buf.extend_from_slice(&probe.mean_sq_distance.to_le_bytes());
            buf.extend_from_slice(&[0u8; 3]);
        }

        for texel in &self.atlas_texels {
            for channel in &texel.rgba {
                buf.extend_from_slice(&channel.to_le_bytes());
            }
        }

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
                     recompile the .prl with the current `prl-build`"
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
        debug_assert_eq!(o, Self::HEADER_SIZE);

        if probe_stride < OCTAHEDRAL_PROBE_STRIDE {
            return Err(invalid_data(format!(
                "octahedral sh volume probe_stride {probe_stride} is smaller than the minimum {OCTAHEDRAL_PROBE_STRIDE}"
            )));
        }

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
            });
            o += probe_stride as usize;
        }

        let total_atlas_texels = (layer_count as usize)
            .checked_mul(atlas_dimensions[0] as usize)
            .and_then(|n| n.checked_mul(atlas_dimensions[1] as usize))
            .ok_or_else(|| {
                invalid_data(format!(
                    "octahedral sh volume layer_count {layer_count} and atlas_dimensions {atlas_dimensions:?} overflow",
                ))
            })?;
        let atlas_bytes = total_atlas_texels
            .checked_mul(OCTAHEDRAL_ATLAS_TEXEL_STRIDE as usize)
            .ok_or_else(|| {
                invalid_data("octahedral sh volume atlas byte count overflow".to_string())
            })?;
        if data.len() < o + atlas_bytes {
            return Err(truncated("octahedral atlas texels"));
        }

        let mut atlas_texels = Vec::with_capacity(total_atlas_texels);
        for _ in 0..total_atlas_texels {
            atlas_texels.push(OctahedralAtlasTexel {
                rgba: [
                    read_u16(data, o),
                    read_u16(data, o + 2),
                    read_u16(data, o + 4),
                    read_u16(data, o + 6),
                ],
            });
            o += OCTAHEDRAL_ATLAS_TEXEL_STRIDE as usize;
        }

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
            atlas_texels,
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
        let total = (grid[0] * grid[1] * grid[2]) as usize;
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let tile_border = DEFAULT_IRRADIANCE_TILE_BORDER;
        let layout = irradiance_atlas_array_layout(grid, tile_dimension, max_dim).unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        let atlas_total = (layout.layer_count * layout.atlas_width * layout.atlas_height) as usize;
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
            probes: (0..total)
                .map(|i| OctahedralShProbe {
                    validity: (i % 2) as u8,
                    mean_distance: f32_to_f16_bits(i as f32 + 0.5),
                    mean_sq_distance: f32_to_f16_bits(i as f32 + 1.0),
                })
                .collect(),
            atlas_texels: (0..atlas_total)
                .map(|i| OctahedralAtlasTexel {
                    rgba: [i as u16, i as u16 + 1, i as u16 + 2, 0x3c00],
                })
                .collect(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn octahedral_round_trip_preserves_single_layer_metadata_and_atlas() {
        let section = oct_section([2, 2, 1]);
        assert_eq!(section.layer_count, 1);
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
        let expected_len = OctahedralShVolumeSection::HEADER_SIZE
            + 4 * OCTAHEDRAL_PROBE_STRIDE as usize
            + (12 * 12) * OCTAHEDRAL_ATLAS_TEXEL_STRIDE as usize
            + 4;
        assert_eq!(bytes.len(), expected_len);

        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn octahedral_round_trip_preserves_multi_layer_metadata_and_atlas() {
        let section = oct_section_with_max_dim([20, 1, 1], 20);
        assert_eq!(section.layer_count, 3);
        assert_eq!(section.tiles_per_layer, 9);
        assert_eq!(section.atlas_tiles_per_row, 3);
        assert_eq!(section.atlas_dimensions, [18, 18]);

        let bytes = section.to_bytes();
        let restored = OctahedralShVolumeSection::from_bytes(&bytes).unwrap();
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
            msg.contains("version 7") && msg.contains("expected 8"),
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
