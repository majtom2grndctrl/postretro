// Animated SH and direct SH delta compose sizing and parameter packing.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DeltaShVolumesSection, delta_probe_f16_stride,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;

const COMPOSE_GRID_DIMS_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeGridParams {
    pub grid_dimensions: [u32; 3],
    pub atlas_dimensions: [u32; 2],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_tiles_per_row: u32,
    pub tiles_per_layer: u32,
    pub atlas_layer_count: u32,
    pub affinity_dims: [u32; 3],
    /// Compact id-34 base-atlas tile geometry. These reuse the two std140 tail
    /// words that used to be padding; dense atlas fields above remain the
    /// composed-output and sampler contract.
    pub compact_atlas_tiles_per_row: u32,
    pub compact_atlas_tiles_per_layer: u32,
}

/// Development-only description of the storage buffers bound by an SH compose
/// pass. Shipping builds do not compile this instrumentation.
#[cfg(feature = "dev-tools")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeStorageFootprint {
    pub delta_subblocks_bytes: usize,
    pub affinity_offsets_bytes: usize,
    pub affinity_lights_bytes: usize,
    pub animation_descriptor_indices_bytes: usize,
}

#[cfg(feature = "dev-tools")]
impl ComposeStorageFootprint {
    pub fn total_bytes(&self) -> usize {
        self.delta_subblocks_bytes
            + self.affinity_offsets_bytes
            + self.affinity_lights_bytes
            + self.animation_descriptor_indices_bytes
    }

    pub fn log(&self, log_label: &str) {
        let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
        log::info!(
            "[Renderer] {log_label} storage footprint: \
             delta_subblocks {:.2} MiB ({} B), affinity_offsets {:.2} MiB ({} B), \
             affinity_lights {:.2} MiB ({} B), animation_descriptor_indices {:.2} MiB ({} B) \
             - total {:.2} MiB ({} B)",
            mib(self.delta_subblocks_bytes),
            self.delta_subblocks_bytes,
            mib(self.affinity_offsets_bytes),
            self.affinity_offsets_bytes,
            mib(self.affinity_lights_bytes),
            self.affinity_lights_bytes,
            mib(self.animation_descriptor_indices_bytes),
            self.animation_descriptor_indices_bytes,
            mib(self.total_bytes()),
            self.total_bytes(),
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeltaComposeBuffers {
    pub animated_light_count: u32,
    pub delta_subblocks: Vec<u16>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    pub animation_descriptor_indices: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectDeltaComposeBuffers {
    pub delta_subblocks: Vec<u16>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

/// CPU-side source of truth for the animated-direct compose scale. The Pass-B
/// WGSL helper mirrors this exactly: unit-radiance delta transport receives the
/// authored intensity/color once through `base_color`, then brightness.
#[derive(Clone, Copy, Debug)]
pub struct AnimatedLightScaleDescriptor<'a> {
    /// Positive for a closed loop. Negative selects endpoint-clamped sampling;
    /// the magnitude is the packed finite-period time scale.
    pub period: f32,
    pub phase: f32,
    pub base_color: [f32; 3],
    pub brightness: &'a [f32],
    pub color: &'a [[f32; 3]],
    pub is_active: bool,
}

/// Match `animated_light_scale` in `animated_direct_sh_compose.wgsl` without a
/// GPU context. `base_color` is either authored `intensity × color`, or an
/// intensity splat when a color curve supplies RGB.
pub fn animated_light_scale(
    descriptor: Option<AnimatedLightScaleDescriptor<'_>>,
    time: f32,
) -> [f32; 3] {
    let Some(descriptor) = descriptor.filter(|descriptor| descriptor.is_active) else {
        return [0.0; 3];
    };

    let cycle_t = animation_curve_t(descriptor.period, descriptor.phase, time);
    let brightness = sample_curve_catmull_rom(descriptor.brightness, cycle_t).max(0.0);
    let color = if descriptor.color.is_empty() {
        descriptor.base_color
    } else {
        let sampled = sample_color_catmull_rom(descriptor.color, cycle_t);
        [
            sampled[0].max(0.0) * descriptor.base_color[0],
            sampled[1].max(0.0) * descriptor.base_color[1],
            sampled[2].max(0.0) * descriptor.base_color[2],
        ]
    };

    [
        color[0] * brightness,
        color[1] * brightness,
        color[2] * brightness,
    ]
}

fn animation_curve_t(period: f32, phase: f32, time: f32) -> f32 {
    if period < 0.0 {
        return -1.0 - (time / (-period).max(1.0e-6) + phase).clamp(0.0, 1.0);
    }
    (time / period.max(1.0e-6) + phase).rem_euclid(1.0)
}

fn sample_curve_catmull_rom(samples: &[f32], cycle_t: f32) -> f32 {
    match samples {
        [] => 1.0,
        [value] => *value,
        _ => {
            let count = samples.len();
            let is_open = cycle_t <= -1.0;
            let t = if is_open {
                (-cycle_t - 1.0).clamp(0.0, 1.0)
            } else {
                cycle_t
            };
            let scaled = if is_open {
                t * (count - 1) as f32
            } else {
                t * count as f32
            };
            let i1 = if is_open {
                (scaled.floor() as usize).min(count - 1)
            } else {
                scaled.floor() as usize % count
            };
            let (i0, i2, i3) = if is_open {
                (
                    i1.saturating_sub(1),
                    (i1 + 1).min(count - 1),
                    (i1 + 2).min(count - 1),
                )
            } else {
                ((i1 + count - 1) % count, (i1 + 1) % count, (i1 + 2) % count)
            };
            let fraction = scaled.fract();
            let (p0, p1, p2, p3) = (samples[i0], samples[i1], samples[i2], samples[i3]);
            let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
            let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
            let c = -0.5 * p0 + 0.5 * p2;
            ((a * fraction + b) * fraction + c) * fraction + p1
        }
    }
}

fn sample_color_catmull_rom(samples: &[[f32; 3]], cycle_t: f32) -> [f32; 3] {
    match samples {
        [] => [1.0; 3],
        [value] => *value,
        _ => {
            let count = samples.len();
            let is_open = cycle_t <= -1.0;
            let t = if is_open {
                (-cycle_t - 1.0).clamp(0.0, 1.0)
            } else {
                cycle_t
            };
            let scaled = if is_open {
                t * (count - 1) as f32
            } else {
                t * count as f32
            };
            let i1 = if is_open {
                (scaled.floor() as usize).min(count - 1)
            } else {
                scaled.floor() as usize % count
            };
            let (i0, i2, i3) = if is_open {
                (
                    i1.saturating_sub(1),
                    (i1 + 1).min(count - 1),
                    (i1 + 2).min(count - 1),
                )
            } else {
                ((i1 + count - 1) % count, (i1 + 1) % count, (i1 + 2) % count)
            };
            let fraction = scaled.fract();
            std::array::from_fn(|channel| {
                let (p0, p1, p2, p3) = (
                    samples[i0][channel],
                    samples[i1][channel],
                    samples[i2][channel],
                    samples[i3][channel],
                );
                let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
                let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
                let c = -0.5 * p0 + 0.5 * p2;
                ((a * fraction + b) * fraction + c) * fraction + p1
            })
        }
    }
}

pub fn build_delta_buffers(
    delta: Option<&DeltaShVolumesSection>,
    grid_dimensions: [u32; 3],
) -> DeltaComposeBuffers {
    let Some(delta) = delta else {
        let affinity_dims = affinity_dims_for_grid(grid_dimensions);
        return DeltaComposeBuffers {
            animated_light_count: 0,
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0; affinity_cell_count(affinity_dims) + 1],
            affinity_lights: Vec::new(),
            animation_descriptor_indices: Vec::new(),
            affinity_dims,
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        animation_descriptor_indices: delta.animation_descriptor_indices.clone(),
        affinity_dims: delta.affinity_dims,
    }
}

pub fn build_direct_delta_buffers(
    delta: Option<&DirectShDeltaVolumesSection>,
    grid_dimensions: [u32; 3],
) -> DirectDeltaComposeBuffers {
    let Some(delta) = delta else {
        let affinity_dims = affinity_dims_for_grid(grid_dimensions);
        return DirectDeltaComposeBuffers {
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0; affinity_cell_count(affinity_dims) + 1],
            affinity_lights: Vec::new(),
            affinity_dims,
        };
    };
    DirectDeltaComposeBuffers {
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        affinity_dims: delta.affinity_dims,
    }
}

pub fn build_animated_direct_delta_buffers(
    delta: Option<&AnimatedDirectShDeltaVolumesSection>,
    grid_dimensions: [u32; 3],
) -> DeltaComposeBuffers {
    let Some(delta) = delta else {
        let affinity_dims = affinity_dims_for_grid(grid_dimensions);
        return DeltaComposeBuffers {
            animated_light_count: 0,
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0; affinity_cell_count(affinity_dims) + 1],
            affinity_lights: Vec::new(),
            animation_descriptor_indices: Vec::new(),
            affinity_dims,
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        animation_descriptor_indices: delta.animation_descriptor_indices.clone(),
        affinity_dims: delta.affinity_dims,
    }
}

fn affinity_dims_for_grid(grid_dimensions: [u32; 3]) -> [u32; 3] {
    let factor = AFFINITY_FACTOR as u32;
    [
        grid_dimensions[0].div_ceil(factor).max(1),
        grid_dimensions[1].div_ceil(factor).max(1),
        grid_dimensions[2].div_ceil(factor).max(1),
    ]
}

fn affinity_cell_count(dims: [u32; 3]) -> usize {
    dims[0] as usize * dims[1] as usize * dims[2] as usize
}

pub fn build_compose_grid_bytes(params: ComposeGridParams) -> [u8; COMPOSE_GRID_DIMS_SIZE] {
    let mut bytes = [0u8; COMPOSE_GRID_DIMS_SIZE];
    bytes[0..4].copy_from_slice(&params.grid_dimensions[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&params.grid_dimensions[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&params.grid_dimensions[2].to_ne_bytes());
    bytes[12..16].copy_from_slice(&params.tile_dimension.to_ne_bytes());
    bytes[16..20].copy_from_slice(&params.atlas_dimensions[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&params.atlas_dimensions[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&params.tile_border.to_ne_bytes());
    bytes[28..32]
        .copy_from_slice(&(delta_probe_f16_stride(params.tile_dimension) as u32).to_ne_bytes());
    bytes[32..36].copy_from_slice(&params.affinity_dims[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&params.affinity_dims[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&params.affinity_dims[2].to_ne_bytes());
    bytes[44..48].copy_from_slice(&params.atlas_tiles_per_row.to_ne_bytes());
    bytes[48..52].copy_from_slice(&params.tiles_per_layer.to_ne_bytes());
    bytes[52..56].copy_from_slice(&params.atlas_layer_count.to_ne_bytes());
    bytes[56..60].copy_from_slice(&params.compact_atlas_tiles_per_row.to_ne_bytes());
    bytes[60..64].copy_from_slice(&params.compact_atlas_tiles_per_layer.to_ne_bytes());
    bytes
}

pub fn u16_slice_to_bytes(data: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn u32_slice_to_bytes(data: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn pad_storage_bytes(mut bytes: Vec<u8>, min_bytes: usize) -> Vec<u8> {
    if bytes.is_empty() {
        bytes.resize(min_bytes, 0);
    }
    bytes
}

pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x3ff) as u32;

    let f32_bits: u32 = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            let mut m = mant;
            let mut e: i32 = -14;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            let m = m & 0x3ff;
            let e_f32 = (e + 127) as u32;
            (sign << 31) | (e_f32 << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (mant << 13)
    } else {
        let e_f32 = exp + (127 - 15);
        (sign << 31) | (e_f32 << 23) | (mant << 13)
    };

    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh_volume::f32_to_f16_bits;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::delta_sh_volumes::{
        DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };

    fn sample_subblock(seed: u16) -> Vec<u16> {
        (0..PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE)
            .map(|i| seed.wrapping_add(i as u16))
            .collect()
    }

    #[test]
    fn f16_bits_round_trip_for_simple_values() {
        for v in [0.0f32, 1.0, -1.0, 0.5, 2.0, -0.25, 100.0] {
            let bits = f32_to_f16_bits(v);
            let back = f16_bits_to_f32(bits);
            assert!((back - v).abs() < 1e-3);
        }
    }

    #[test]
    fn build_delta_buffers_no_section_returns_empty_payload_with_full_empty_offsets() {
        let b = build_delta_buffers(None, [5, 2, 1]);
        assert_eq!(b.animated_light_count, 0);
        assert!(b.delta_subblocks.is_empty());
        assert_eq!(b.affinity_dims, [2, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 0, 0]);
    }

    #[test]
    fn build_delta_buffers_maps_section_fields_keeping_f16() {
        let mut subblocks = sample_subblock(10);
        subblocks.extend(sample_subblock(200));
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![4, u32::MAX],
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_delta_buffers(Some(&section), [12, 1, 1]);
        assert_eq!(b.animated_light_count, 2);
        assert_eq!(b.affinity_dims, [3, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        assert_eq!(b.animation_descriptor_indices, vec![4, u32::MAX]);
        assert_eq!(b.delta_subblocks, subblocks);
    }

    #[test]
    fn build_direct_delta_buffers_no_section_returns_empty_payload_with_full_empty_offsets() {
        let b = build_direct_delta_buffers(None, [5, 2, 1]);
        assert!(b.delta_subblocks.is_empty());
        assert_eq!(b.affinity_dims, [2, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 0, 0]);
        assert!(b.affinity_lights.is_empty());
    }

    #[test]
    fn build_direct_delta_buffers_maps_section_fields_keeping_f16() {
        let mut subblocks = sample_subblock(10);
        subblocks.extend(sample_subblock(200));
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_direct_delta_buffers(Some(&section), [12, 1, 1]);
        assert_eq!(b.affinity_dims, [3, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        assert_eq!(b.delta_subblocks, subblocks);
    }

    #[test]
    fn build_animated_direct_delta_buffers_keeps_its_own_descriptor_index_space() {
        let subblocks = sample_subblock(10);
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![7],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: subblocks.clone(),
        };

        let buffers = build_animated_direct_delta_buffers(Some(&section), [1, 1, 1]);
        assert_eq!(buffers.animation_descriptor_indices, vec![7]);
        assert_eq!(buffers.affinity_lights, vec![0]);
        assert_eq!(buffers.delta_subblocks, subblocks);
    }

    #[test]
    fn animated_light_scale_follows_compose_lifecycle_without_gpu() {
        let assert_scale = |actual: [f32; 3], expected: [f32; 3]| {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1.0e-5,
                    "scale {actual} did not match {expected}"
                );
            }
        };

        let authored = AnimatedLightScaleDescriptor {
            period: 1.0,
            phase: 0.0,
            base_color: [2.0, 1.0, 0.5],
            brightness: &[],
            color: &[],
            is_active: true,
        };
        // Initial-active carries authored radiance; initial-inactive is dark.
        assert_scale(animated_light_scale(Some(authored), 0.0), [2.0, 1.0, 0.5]);
        assert_scale(
            animated_light_scale(
                Some(AnimatedLightScaleDescriptor {
                    is_active: false,
                    ..authored
                }),
                0.0,
            ),
            [0.0; 3],
        );

        let brightness = [0.5, 1.0, 0.5, 0.0];
        let color = [[1.0, 0.5, 0.25], [1.0, 0.5, 0.25]];
        let installed = AnimatedLightScaleDescriptor {
            base_color: [2.0; 3],
            brightness: &brightness,
            color: &color,
            ..authored
        };
        // Trigger-installed color/brightness applies intensity once. At the
        // second knot, looping evaluation reaches the exact authored sample.
        assert_scale(animated_light_scale(Some(installed), 0.25), [2.0, 1.0, 0.5]);
        assert_scale(
            animated_light_scale(Some(installed), 1.25),
            animated_light_scale(Some(installed), 0.25),
        );

        // A finite descriptor uses the negative-period endpoint-clamped mode.
        // It reaches the final sample at the completion boundary instead of
        // wrapping to the closed curve's first sample.
        let one_shot = [0.25, 0.75];
        let playing = AnimatedLightScaleDescriptor {
            period: -1.0,
            brightness: &one_shot,
            color: &[],
            ..authored
        };
        let settled = AnimatedLightScaleDescriptor {
            base_color: [1.5, 0.75, 0.375],
            brightness: &[],
            color: &[],
            ..authored
        };
        assert_scale(
            animated_light_scale(Some(playing), 1.0),
            animated_light_scale(Some(settled), 1.0),
        );
        // Explicitly clearing holds authored/settled radiance. Despawn removes
        // the descriptor contribution; a reload reinstates the initial state.
        assert_scale(animated_light_scale(Some(settled), 2.0), [1.5, 0.75, 0.375]);
        assert_scale(animated_light_scale(None, 2.0), [0.0; 3]);
        assert_scale(animated_light_scale(Some(authored), 0.0), [2.0, 1.0, 0.5]);
    }

    #[test]
    fn build_compose_grid_bytes_packs_layer_fields_at_std140_tail() {
        let bytes = build_compose_grid_bytes(ComposeGridParams {
            grid_dimensions: [2, 3, 4],
            atlas_dimensions: [120, 60],
            tile_dimension: 6,
            tile_border: 1,
            atlas_tiles_per_row: 20,
            tiles_per_layer: 400,
            atlas_layer_count: 3,
            affinity_dims: [1, 2, 3],
            compact_atlas_tiles_per_row: 7,
            compact_atlas_tiles_per_layer: 49,
        });

        let word = |offset: usize| {
            u32::from_ne_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };

        assert_eq!(bytes.len(), COMPOSE_GRID_DIMS_SIZE);
        assert_eq!(word(44), 20);
        assert_eq!(word(48), 400);
        assert_eq!(word(52), 3);
        assert_eq!(word(56), 7);
        assert_eq!(word(60), 49);
    }
}
