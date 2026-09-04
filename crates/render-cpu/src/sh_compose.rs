// Animated SH and direct SH delta compose sizing and parameter packing.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DeltaShVolumesSection, PROBES_PER_CELL, delta_probe_f16_stride,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::sh_reconstruct::{
    Level, kept_mask, reconstruct_l1_tile, reconstruct_l2_tile, stored_delta_tiles,
};

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
    /// Legacy std140 tail words retained at their fixed offsets. They repeat
    /// the stored atlas geometry above exactly; the byte layout must not move.
    pub compact_atlas_tiles_per_row: u32,
    pub compact_atlas_tiles_per_layer: u32,
}

/// Development-only description of the storage buffers bound by an SH compose
/// pass. Shipping builds do not compile this instrumentation.
#[cfg(feature = "dev-tools")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeStorageFootprint {
    pub delta_subblocks_bytes: usize,
    pub delta_compaction_meta_bytes: usize,
    pub affinity_offsets_bytes: usize,
    pub affinity_lights_bytes: usize,
    pub animation_descriptor_indices_bytes: usize,
}

#[cfg(feature = "dev-tools")]
impl ComposeStorageFootprint {
    pub fn total_bytes(&self) -> usize {
        self.delta_subblocks_bytes
            + self.delta_compaction_meta_bytes
            + self.affinity_offsets_bytes
            + self.affinity_lights_bytes
            + self.animation_descriptor_indices_bytes
    }

    pub fn log(&self, log_label: &str) {
        let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
        log::info!(
            "[Renderer] {log_label} storage footprint: \
             delta_subblocks {:.2} MiB ({} B), delta_compaction_meta {:.2} MiB ({} B), affinity_offsets {:.2} MiB ({} B), \
             affinity_lights {:.2} MiB ({} B), animation_descriptor_indices {:.2} MiB ({} B) \
             - total {:.2} MiB ({} B)",
            mib(self.delta_subblocks_bytes),
            self.delta_subblocks_bytes,
            mib(self.delta_compaction_meta_bytes),
            self.delta_compaction_meta_bytes,
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
    /// One id-34-cross-checked valid-probe descriptor per affinity cell.
    pub valid_probe_masks: Vec<u64>,
    /// Per-affinity-cell delta coarsening level (0/1/2). Governs which valid
    /// probes are kept (stored) versus reconstructed intra-brick.
    pub cell_levels: Vec<u8>,
    /// Base f16-half offset for every post-drop CSR entry. Unlike
    /// `affinity_offsets`, this is indexed by entry rather than cell.
    pub entry_offsets: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

impl DeltaComposeBuffers {
    /// Pack the id-27 descriptor and post-drop entry offset table into one
    /// storage-buffer word stream: low/high words per cell validity mask, one
    /// widened coarsening level per cell, then one f16-half offset per CSR
    /// entry. The indirect and animated-direct compose shaders derive both
    /// splits from `grid.affinity_dims`, so this contains no separately mutable
    /// length field.
    pub fn compaction_meta_words(&self) -> Vec<u32> {
        let mut words =
            Vec::with_capacity(self.valid_probe_masks.len() * 3 + self.entry_offsets.len());
        for &mask in &self.valid_probe_masks {
            words.push(mask as u32);
            words.push((mask >> 32) as u32);
        }
        words.extend(self.cell_levels.iter().map(|&level| u32::from(level)));
        words.extend_from_slice(&self.entry_offsets);
        words
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectDeltaComposeBuffers {
    pub delta_subblocks: Vec<u16>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    /// One id-34-cross-checked valid-probe descriptor per affinity cell.
    pub valid_probe_masks: Vec<u64>,
    /// Per-affinity-cell delta coarsening level (0/1/2). Governs which valid
    /// probes are kept (stored) versus reconstructed intra-brick.
    pub cell_levels: Vec<u8>,
    /// Base f16-half offset for every post-drop CSR entry. Unlike
    /// `affinity_offsets`, this is indexed by entry rather than cell.
    pub entry_offsets: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

impl DirectDeltaComposeBuffers {
    /// Pack the id-41 descriptor and post-drop entry offset table into one
    /// storage-buffer word stream: low/high words per cell validity mask, one
    /// widened coarsening level per cell, then one f16-half offset per CSR
    /// entry. The direct compose shader derives both splits from
    /// `grid.affinity_dims`, so this contains no separately mutable length
    /// field.
    pub fn compaction_meta_words(&self) -> Vec<u32> {
        let mut words =
            Vec::with_capacity(self.valid_probe_masks.len() * 3 + self.entry_offsets.len());
        for &mask in &self.valid_probe_masks {
            words.push(mask as u32);
            words.push((mask >> 32) as u32);
        }
        words.extend(self.cell_levels.iter().map(|&level| u32::from(level)));
        words.extend_from_slice(&self.entry_offsets);
        words
    }
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
            valid_probe_masks: vec![0; affinity_cell_count(affinity_dims)],
            cell_levels: vec![0; affinity_cell_count(affinity_dims)],
            entry_offsets: Vec::new(),
            affinity_dims,
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        animation_descriptor_indices: delta.animation_descriptor_indices.clone(),
        valid_probe_masks: delta.valid_probe_masks.clone(),
        cell_levels: delta.cell_levels.clone(),
        entry_offsets: delta_entry_offsets(
            delta.affinity_cell_count(),
            &delta.affinity_offsets,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            delta.delta_probe_f16_stride(),
            delta.delta_subblocks.len(),
            "indirect SH delta",
        ),
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
            valid_probe_masks: vec![0; affinity_cell_count(affinity_dims)],
            cell_levels: vec![0; affinity_cell_count(affinity_dims)],
            entry_offsets: Vec::new(),
            affinity_dims,
        };
    };
    DirectDeltaComposeBuffers {
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        valid_probe_masks: delta.valid_probe_masks.clone(),
        cell_levels: delta.cell_levels.clone(),
        entry_offsets: delta_entry_offsets(
            delta.affinity_cell_count(),
            &delta.affinity_offsets,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            delta.delta_probe_f16_stride(),
            delta.delta_subblocks.len(),
            "direct SH delta",
        ),
        affinity_dims: delta.affinity_dims,
    }
}

fn delta_entry_offsets(
    affinity_cell_count: usize,
    affinity_offsets: &[u32],
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    delta_probe_f16_stride: usize,
    payload_len: usize,
    label: &str,
) -> Vec<u32> {
    let stride = u32::try_from(delta_probe_f16_stride)
        .unwrap_or_else(|_| panic!("{label} tile stride must fit shader u32 indexing"));
    let entry_count = affinity_offsets.last().copied().unwrap_or_default() as usize;
    let mut offsets = Vec::with_capacity(entry_count);
    let mut next_offset = 0u32;
    for cell in 0..affinity_cell_count {
        let entry_count = affinity_offsets[cell + 1] - affinity_offsets[cell];
        let level = Level::from_u8(cell_levels[cell])
            .unwrap_or_else(|| panic!("{label} cell {cell} coarsening level must be 0..=2"));
        let kept_tiles = stored_delta_tiles(level, valid_probe_masks[cell]) as u32;
        let cell_f16_len = kept_tiles
            .checked_mul(stride)
            .unwrap_or_else(|| panic!("{label} compact cell length must fit shader u32 indexing"));
        for _ in 0..entry_count {
            offsets.push(next_offset);
            next_offset = next_offset
                .checked_add(cell_f16_len)
                .unwrap_or_else(|| panic!("{label} payload must fit shader u32 indexing"));
        }
    }
    debug_assert_eq!(
        usize::try_from(next_offset).ok(),
        Some(payload_len),
        "entry offsets must cover the compact {label} payload exactly"
    );
    offsets
}

/// Resolve a direct-delta tile's f16-half offset from its post-drop CSR entry
/// and affinity-local probe. `None` is the required invalid-local result: the
/// compose shader must skip before reading any delta words.
pub fn resolve_direct_delta_f16_offset(
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    entry_offsets: &[u32],
    entry: usize,
    cell: usize,
    local_probe: u32,
    tile_f16_stride: u32,
) -> Option<u32> {
    resolve_delta_f16_offset(
        valid_probe_masks,
        cell_levels,
        entry_offsets,
        entry,
        cell,
        local_probe,
        tile_f16_stride,
    )
}

/// Resolve an animated-direct delta tile's f16-half offset from its post-drop
/// CSR entry and affinity-local probe. The animated compose pass must skip an
/// invalid local before calling its compact payload reader.
pub fn resolve_animated_direct_delta_f16_offset(
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    entry_offsets: &[u32],
    entry: usize,
    cell: usize,
    local_probe: u32,
    tile_f16_stride: u32,
) -> Option<u32> {
    resolve_delta_f16_offset(
        valid_probe_masks,
        cell_levels,
        entry_offsets,
        entry,
        cell,
        local_probe,
        tile_f16_stride,
    )
}

/// Resolve a compact delta tile's f16-half offset from its post-drop CSR entry
/// and affinity-local probe. Under coarsening the payload holds one tile per
/// KEPT probe, so this ranks the local within `kept_mask(level, mask)` rather
/// than raw validity. `None` is returned when the probe's KEPT bit is clear —
/// covering both invalid probes and dropped-valid ones (which are reconstructed
/// via [`reconstruct_delta_probe_tile`], not read directly). Behavior-preserving
/// at L0, where `kept_mask == mask`.
pub fn resolve_delta_f16_offset(
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    entry_offsets: &[u32],
    entry: usize,
    cell: usize,
    local_probe: u32,
    tile_f16_stride: u32,
) -> Option<u32> {
    if local_probe >= 64 {
        return None;
    }
    let mask = *valid_probe_masks.get(cell)?;
    let level = Level::from_u8(*cell_levels.get(cell)?)?;
    let kept = kept_mask(level, mask);
    if kept & (1u64 << local_probe) == 0 {
        return None;
    }
    let kept_rank = (kept & ((1u64 << local_probe) - 1)).count_ones();
    entry_offsets
        .get(entry)?
        .checked_add(kept_rank.checked_mul(tile_f16_stride)?)
}

/// Immutable section data used to reconstruct one or more delta probe tiles.
#[derive(Clone, Copy, Debug)]
pub struct DeltaProbeReconstructionContext<'a> {
    pub valid_probe_masks: &'a [u64],
    pub cell_levels: &'a [u8],
    /// Kept-rank base offsets produced by `delta_entry_offsets`.
    pub entry_offsets: &'a [u32],
    /// Packed f16 RGBA payload. Reconstruction reads RGB; alpha is unused.
    pub delta_subblocks: &'a [u16],
    /// Interior RGB texel count per tile.
    pub tile_texels: usize,
    /// f16 stride per probe tile (`tile_texels * 4` for RGBA16F).
    pub tile_f16_stride: u32,
}

/// Reconstruct the composed delta tile for one probe from a (possibly coarsened)
/// delta section, intra-brick. Three states:
///  - invalid       (validity bit clear)  -> None (skip)
///  - kept          (kept_mask bit set)    -> the stored tile, read by kept rank
///  - dropped-valid (valid but not kept)   -> reconstructed: L1 = intra-brick
///    trilinear over the brick's kept corners; L2 = the single kept brick-mean tile
///
/// `valid_probe_masks` is the section's full per-cell validity, with semantics
/// identical for id 27/41/45 — the delta-SH, direct-delta, and animated-direct
/// sections all validate probes through this mask (id 41/45 need no separate
/// `probe_indirection`). This CPU reference is the AC-3 golden the WGSL compose
/// port must match.
pub fn reconstruct_delta_probe_tile(
    context: &DeltaProbeReconstructionContext<'_>,
    entry: usize,
    cell: usize,
    local_probe: u32,
) -> Option<Vec<glam::Vec3>> {
    if local_probe >= PROBES_PER_CELL as u32 {
        return None;
    }
    let level = Level::from_u8(*context.cell_levels.get(cell)?)?;
    let mask = *context.valid_probe_masks.get(cell)?;
    let local_bit = 1u64 << local_probe;
    if mask & local_bit == 0 {
        return None; // invalid probe
    }
    let kept = kept_mask(level, mask);
    let entry_base = *context.entry_offsets.get(entry)?;

    // Read `tile_texels` RGB texels from the f16 payload at the given kept rank,
    // taking the R,G,B of each 4-f16 (RGBA) texel.
    let decode = |kept_rank: u32| -> Vec<glam::Vec3> {
        let base = (entry_base + kept_rank * context.tile_f16_stride) as usize;
        (0..context.tile_texels)
            .map(|t| {
                let i = base + t * 4;
                glam::Vec3::new(
                    f16_bits_to_f32(context.delta_subblocks[i]),
                    f16_bits_to_f32(context.delta_subblocks[i + 1]),
                    f16_bits_to_f32(context.delta_subblocks[i + 2]),
                )
            })
            .collect()
    };

    if kept & local_bit != 0 {
        // Kept (also the whole of L0): read the stored tile by kept rank.
        let kept_rank = (kept & (local_bit - 1)).count_ones();
        return Some(decode(kept_rank));
    }

    // Dropped-valid: gather the brick's kept tiles into a local lattice and
    // reconstruct intra-brick.
    let mut kept_tiles: [Option<Vec<glam::Vec3>>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
    let mut remaining = kept;
    while remaining != 0 {
        let k = remaining.trailing_zeros();
        let kept_rank = (kept & ((1u64 << k) - 1)).count_ones();
        kept_tiles[k as usize] = Some(decode(kept_rank));
        remaining &= remaining - 1;
    }

    match level {
        Level::L1 => reconstruct_l1_tile(&kept_tiles, local_probe as usize, context.tile_texels),
        Level::L2 => reconstruct_l2_tile(&kept_tiles, context.tile_texels),
        // L0 has no dropped-valid probes (kept == valid), so this is unreachable;
        // treat defensively as the stored tile.
        Level::L0 => {
            let kept_rank = (kept & (local_bit - 1)).count_ones();
            Some(decode(kept_rank))
        }
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
            valid_probe_masks: vec![0; affinity_cell_count(affinity_dims)],
            cell_levels: vec![0; affinity_cell_count(affinity_dims)],
            entry_offsets: Vec::new(),
            affinity_dims,
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        animation_descriptor_indices: delta.animation_descriptor_indices.clone(),
        valid_probe_masks: delta.valid_probe_masks.clone(),
        cell_levels: delta.cell_levels.clone(),
        entry_offsets: delta_entry_offsets(
            delta.affinity_cell_count(),
            &delta.affinity_offsets,
            &delta.valid_probe_masks,
            &delta.cell_levels,
            delta.delta_probe_f16_stride(),
            delta.delta_subblocks.len(),
            "animated direct SH delta",
        ),
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
        assert_eq!(b.valid_probe_masks, vec![0, 0]);
        assert!(b.entry_offsets.is_empty());
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
            valid_probe_masks: vec![u64::MAX; 3],
            cell_levels: vec![0u8; 3],
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
        assert_eq!(b.valid_probe_masks, vec![u64::MAX; 3]);
        assert_eq!(
            b.entry_offsets,
            vec![0, (PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE) as u32]
        );
        assert_eq!(
            b.compaction_meta_words(),
            vec![
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                0,
                0,
                0,
                0,
                (PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE) as u32,
            ]
        );
    }

    #[test]
    fn delta_compaction_meta_places_cell_levels_before_entry_offsets() {
        let buffers = DeltaComposeBuffers {
            animated_light_count: 0,
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0, 1, 2],
            affinity_lights: vec![0, 1],
            animation_descriptor_indices: Vec::new(),
            valid_probe_masks: vec![0x0000_0000_0000_9009, 0x9009_0000_0000_0000],
            cell_levels: vec![1, 2],
            entry_offsets: vec![144, 180],
            affinity_dims: [2, 1, 1],
        };

        assert_eq!(
            buffers.compaction_meta_words(),
            vec![0x0000_9009, 0, 0, 0x9009_0000, 1, 2, 144, 180,],
            "id-27/id-45 shaders derive their entry-offset base after two mask words and one level word per cell",
        );
    }

    #[test]
    fn indirect_delta_resolver_uses_within_cell_rank_and_retains_zero_length_entries() {
        let stride = DEFAULT_DELTA_PROBE_F16_STRIDE as u32;
        let mixed = (1u64 << 1) | (1u64 << 5) | (1u64 << 31);
        let tail = (1u64 << 2) | (1u64 << 4);
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0, 1, 2],
            valid_probe_masks: vec![mixed, 0, tail],
            cell_levels: vec![0u8; 3],
            affinity_offsets: vec![0, 1, 2, 3],
            affinity_lights: vec![0, 1, 2],
            delta_subblocks: vec![0; 5 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        };
        let buffers = build_delta_buffers(Some(&section), [12, 1, 1]);

        assert_eq!(buffers.entry_offsets, vec![0, 3 * stride, 3 * stride]);
        assert_eq!(
            resolve_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                0,
                0,
                5,
                stride,
            ),
            Some(stride),
            "local 5 is rank one in the first cell, not global probe rank five"
        );
        assert_eq!(
            resolve_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                1,
                1,
                0,
                stride,
            ),
            None,
            "an all-invalid retained entry reads no delta payload"
        );
        assert_eq!(
            resolve_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                2,
                2,
                4,
                stride,
            ),
            Some(4 * stride),
            "the entry after a zero-length cell shares its predecessor's base offset"
        );
    }

    #[test]
    fn build_direct_delta_buffers_no_section_returns_empty_payload_with_full_empty_offsets() {
        let b = build_direct_delta_buffers(None, [5, 2, 1]);
        assert!(b.delta_subblocks.is_empty());
        assert_eq!(b.affinity_dims, [2, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 0, 0]);
        assert!(b.affinity_lights.is_empty());
        assert_eq!(b.valid_probe_masks, vec![0, 0]);
        assert!(b.entry_offsets.is_empty());
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
            valid_probe_masks: vec![u64::MAX; 3],
            cell_levels: vec![0u8; 3],
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_direct_delta_buffers(Some(&section), [12, 1, 1]);
        assert_eq!(b.affinity_dims, [3, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        assert_eq!(b.delta_subblocks, subblocks);
        assert_eq!(b.valid_probe_masks, vec![u64::MAX; 3]);
        assert_eq!(
            b.entry_offsets,
            vec![0, (PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE) as u32]
        );
        assert_eq!(
            b.compaction_meta_words(),
            vec![
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                0,
                0,
                0,
                0,
                (PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE) as u32,
            ]
        );
    }

    #[test]
    fn direct_delta_compaction_meta_places_cell_levels_before_entry_offsets() {
        let buffers = DirectDeltaComposeBuffers {
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0, 1, 2],
            affinity_lights: vec![0, 1],
            valid_probe_masks: vec![0x0000_0000_0000_9009, 0x9009_0000_0000_0000],
            cell_levels: vec![1, 2],
            entry_offsets: vec![144, 180],
            affinity_dims: [2, 1, 1],
        };

        assert_eq!(
            buffers.compaction_meta_words(),
            vec![0x0000_9009, 0, 0, 0x9009_0000, 1, 2, 144, 180],
            "id-41 derives its entry-offset base after two mask words and one level word per cell",
        );
    }

    #[test]
    fn direct_delta_resolver_uses_within_cell_rank_and_retains_zero_length_entries() {
        let stride = DEFAULT_DELTA_PROBE_F16_STRIDE as u32;
        let mixed = (1u64 << 1) | (1u64 << 5) | (1u64 << 31);
        let tail = (1u64 << 2) | (1u64 << 4);
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![mixed, 0, tail],
            cell_levels: vec![0u8; 3],
            affinity_offsets: vec![0, 1, 2, 3],
            affinity_lights: vec![0, 1, 2],
            delta_subblocks: vec![0; 5 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        };
        let buffers = build_direct_delta_buffers(Some(&section), [12, 1, 1]);

        assert_eq!(buffers.entry_offsets, vec![0, 3 * stride, 3 * stride]);
        assert_eq!(
            resolve_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                0,
                0,
                5,
                stride,
            ),
            Some(stride),
            "local 5 is rank one in the first cell, not global probe rank five"
        );
        assert_eq!(
            resolve_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                1,
                1,
                0,
                stride,
            ),
            None,
            "an all-invalid retained entry reads no delta payload"
        );
        assert_eq!(
            resolve_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                2,
                2,
                4,
                stride,
            ),
            Some(4 * stride),
            "the entry after a zero-length cell shares its predecessor's base offset"
        );
        assert_eq!(
            resolve_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                0,
                0,
                3,
                stride,
            ),
            None,
            "invalid locals never alias the adjacent compact tile"
        );
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
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: subblocks.clone(),
        };

        let buffers = build_animated_direct_delta_buffers(Some(&section), [1, 1, 1]);
        assert_eq!(buffers.animation_descriptor_indices, vec![7]);
        assert_eq!(buffers.affinity_lights, vec![0]);
        assert_eq!(buffers.delta_subblocks, subblocks);
        assert_eq!(buffers.valid_probe_masks, vec![u64::MAX]);
        assert_eq!(buffers.entry_offsets, vec![0]);
        assert_eq!(
            buffers.compaction_meta_words(),
            vec![u32::MAX, u32::MAX, 0, 0],
            "the animated pass receives mask words and a cell level before post-drop f16 offsets"
        );
    }

    #[test]
    fn animated_direct_delta_resolver_uses_rank_and_preserves_zero_length_entries() {
        let stride = DEFAULT_DELTA_PROBE_F16_STRIDE as u32;
        let mixed = (1u64 << 1) | (1u64 << 5) | (1u64 << 31);
        let tail = (1u64 << 2) | (1u64 << 4);
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0, 1, 2],
            valid_probe_masks: vec![mixed, 0, tail],
            cell_levels: vec![0u8; 3],
            affinity_offsets: vec![0, 1, 2, 3],
            affinity_lights: vec![0, 1, 2],
            delta_subblocks: vec![0; 5 * DEFAULT_DELTA_PROBE_F16_STRIDE],
        };
        let buffers = build_animated_direct_delta_buffers(Some(&section), [12, 1, 1]);

        assert_eq!(buffers.entry_offsets, vec![0, 3 * stride, 3 * stride]);
        assert_eq!(
            resolve_animated_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                0,
                0,
                5,
                stride,
            ),
            Some(stride),
            "local 5 is rank one in the first compact cell"
        );
        assert_eq!(
            resolve_animated_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                1,
                1,
                0,
                stride,
            ),
            None,
            "an all-invalid retained animated entry reads no payload"
        );
        assert_eq!(
            resolve_animated_direct_delta_f16_offset(
                &buffers.valid_probe_masks,
                &buffers.cell_levels,
                &buffers.entry_offsets,
                2,
                2,
                4,
                stride,
            ),
            Some(4 * stride),
            "the entry after a zero-length cell shares its predecessor's base offset"
        );
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

    // The reconstruction reference below takes the section's raw arrays
    // (`valid_probe_masks` + `cell_levels` + offsets + payload), so a single test
    // set covers id 27/41/45: the direct (41) and animated-direct (45) sections
    // prove probe validity through `valid_probe_masks` exactly as the indirect
    // (27) section does — no `probe_indirection` is consulted (AC 4).

    #[test]
    fn reconstruct_l1_dropped_valid_probe_is_intra_brick_trilinear() {
        let corners = [0usize, 3, 12, 15, 48, 51, 60, 63];
        let interior = 21usize; // (1,1,1), a valid non-corner probe
        let mut mask = 1u64 << interior;
        for &c in &corners {
            mask |= 1u64 << c;
        }
        let valid_probe_masks = vec![mask];
        let cell_levels = vec![Level::L1.to_u8()];
        let tile_texels = 1usize;
        let stride = (tile_texels * 4) as u32;

        // Kept = the 8 corners. Store one RGBA texel per kept tile in kept-rank
        // (ascending local) order, value = 10 + lx*2 splatted across RGB.
        let kept = kept_mask(Level::L1, mask);
        let mut delta_subblocks = Vec::new();
        let mut remaining = kept;
        while remaining != 0 {
            let k = remaining.trailing_zeros() as usize;
            let lx = k % 4;
            let bits = f32_to_f16_bits(10.0 + lx as f32 * 2.0);
            delta_subblocks.extend_from_slice(&[bits, bits, bits, 0]);
            remaining &= remaining - 1;
        }
        let entry_offsets = vec![0u32];
        let context = DeltaProbeReconstructionContext {
            valid_probe_masks: &valid_probe_masks,
            cell_levels: &cell_levels,
            entry_offsets: &entry_offsets,
            delta_subblocks: &delta_subblocks,
            tile_texels,
            tile_f16_stride: stride,
        };

        // Dropped interior local (1,1,1): trilinear over the linear-x ramp -> 12.
        let recon = reconstruct_delta_probe_tile(&context, 0, 0, interior as u32)
            .expect("dropped interior local reconstructs");
        assert!(
            (recon[0].x - 12.0).abs() < 1e-2,
            "trilinear ramp at (1,1,1) must be 12, got {}",
            recon[0].x
        );

        // A kept corner reads its stored value (local 3 -> lx 3 -> 16).
        let corner =
            reconstruct_delta_probe_tile(&context, 0, 0, 3).expect("kept corner reads stored tile");
        assert!(
            (corner[0].x - 16.0).abs() < 1e-2,
            "kept corner local 3 stores 16, got {}",
            corner[0].x
        );

        // An invalid local (validity clear) returns None.
        assert!(
            reconstruct_delta_probe_tile(&context, 0, 0, 1).is_none(),
            "an invalid local reconstructs to None"
        );
    }

    #[test]
    fn reconstruct_l2_dropped_valid_probe_returns_brick_mean() {
        let valid_probe_masks = vec![0b1111u64]; // locals 0..3 valid
        let cell_levels = vec![Level::L2.to_u8()];
        let tile_texels = 1usize;
        let stride = (tile_texels * 4) as u32;
        let mean = 42.0f32;
        let bits = f32_to_f16_bits(mean);
        // Kept = the single lowest bit (local 0) holding the brick-mean tile.
        let delta_subblocks = vec![bits, bits, bits, 0];
        let entry_offsets = vec![0u32];
        let context = DeltaProbeReconstructionContext {
            valid_probe_masks: &valid_probe_masks,
            cell_levels: &cell_levels,
            entry_offsets: &entry_offsets,
            delta_subblocks: &delta_subblocks,
            tile_texels,
            tile_f16_stride: stride,
        };

        // A dropped-valid local returns the brick mean.
        let dropped = reconstruct_delta_probe_tile(&context, 0, 0, 1)
            .expect("dropped-valid returns the brick mean");
        assert!((dropped[0].x - mean).abs() < 1e-2);

        // The kept representative (local 0) reads the stored mean directly.
        let rep = reconstruct_delta_probe_tile(&context, 0, 0, 0)
            .expect("kept representative reads the stored mean tile");
        assert!((rep[0].x - mean).abs() < 1e-2);

        // An invalid local returns None.
        assert!(
            reconstruct_delta_probe_tile(&context, 0, 0, 4).is_none(),
            "an invalid local reconstructs to None"
        );
    }

    #[test]
    fn kept_rank_offsets_mix_l0_and_l1_cells() {
        let stride = 4u32;
        // Cell 0 (L0): locals 0 and 2 valid -> 2 kept tiles.
        let mask0 = 0b101u64;
        // Cell 1 (L1): all 8 corners valid plus non-corner local 1 -> 8 kept.
        let corners = [0u64, 3, 12, 15, 48, 51, 60, 63];
        let mut mask1 = 1u64 << 1;
        for c in corners {
            mask1 |= 1u64 << c;
        }
        let valid_probe_masks = vec![mask0, mask1];
        let cell_levels = vec![Level::L0.to_u8(), Level::L1.to_u8()];
        let affinity_offsets = vec![0u32, 1, 2];
        // payload = kept(cell0)=2 + kept(cell1)=8 -> 10 tiles.
        let payload_len = 10 * stride as usize;

        let offsets = delta_entry_offsets(
            2,
            &affinity_offsets,
            &valid_probe_masks,
            &cell_levels,
            stride as usize,
            payload_len,
            "mixed level test",
        );
        // The L1 cell's entry base advances by cell 0's KEPT-tile count (2), not
        // its full validity popcount.
        assert_eq!(offsets, vec![0, 2 * stride]);

        // L0 rank in cell 0: local 2 is kept rank 1.
        assert_eq!(
            resolve_delta_f16_offset(&valid_probe_masks, &cell_levels, &offsets, 0, 0, 2, stride),
            Some(stride),
        );
        // L1 kept corner local 3 is kept rank 1 within its own cell's base.
        assert_eq!(
            resolve_delta_f16_offset(&valid_probe_masks, &cell_levels, &offsets, 1, 1, 3, stride),
            Some(2 * stride + stride),
        );
        // A dropped-valid non-corner (local 1) has its kept bit clear -> None.
        assert_eq!(
            resolve_delta_f16_offset(&valid_probe_masks, &cell_levels, &offsets, 1, 1, 1, stride),
            None,
        );
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
            compact_atlas_tiles_per_row: 20,
            compact_atlas_tiles_per_layer: 400,
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
        assert_eq!(word(56), 20);
        assert_eq!(word(60), 400);
    }
}
