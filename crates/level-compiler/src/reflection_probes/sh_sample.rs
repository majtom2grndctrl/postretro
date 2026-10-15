// Reflection-probe SH reads: CPU mirror of the forward pass's indirect SH sample.
// See: context/lib/rendering_pipeline.md §4 (Indirect) · shaders sh_sample.wgsl, sh_indirection.wgsl

use glam::{UVec3, Vec3, Vec4};
use postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{encode_unquantized_uv, irradiance_array_tile_location};
use postretro_level_format::sh_reconstruct::{Level, stored_node_prefix_sum};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::bc6h::f16_bits_to_f32;

/// The renderer's production probe-occlusion default
/// (`render_cpu::sh_volume::DEFAULT_PROBE_OCCLUSION`): Chebyshev depth
/// visibility weights each probe. `POSTRETRO_SH_FAST=1` turns it off at
/// runtime, a diagnostic the capture does not follow.
pub(crate) const PRODUCTION_PROBE_OCCLUSION: bool = true;

// `sh_sample.wgsl` / `forward.wgsl` constants, mirrored.
const SH_NORMAL_OFFSET_M: f32 = 0.1;
const SH_DEPTH_MIN_VARIANCE_M2: f32 = 1.0e-4;
const SH_DEPTH_BIAS_CELL_FRACTION: f32 = 0.05;
const SH_DEPTH_MIN_VISIBILITY: f32 = 0.03;
const SH_WEIGHT_EPSILON: f32 = 1.0e-5;

/// A probe's load-derived indirection: storage level, node scale and the
/// stored-atlas slot, or `None` for an invalid probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProbeIndirection {
    level: Level,
    scale: u8,
    slot: u32,
}

/// Indirect SH as the forward pass samples it (`sample_sh_indirect` with
/// backface rejection), over the packed pre-BC6H base volume: the same stored
/// set, levels and slots the runtime samples, minus block compression. The
/// whole-cell fast paths in the shader are shortcuts of this per-corner sum
/// and agree with it to rounding.
pub(crate) struct ShVolumeSampler<'a> {
    section: &'a OctahedralShVolumeSection,
    indirection: Vec<Option<ProbeIndirection>>,
    probe_occlusion: bool,
}

impl<'a> ShVolumeSampler<'a> {
    /// Fails when a volume with probes is not the uncompressed packed volume
    /// this sampler reads, or its stored-set metadata does not add up.
    pub(crate) fn new(
        section: &'a OctahedralShVolumeSection,
        probe_occlusion: bool,
    ) -> anyhow::Result<Self> {
        let indirection = if section.total_probes() == 0 {
            Vec::new()
        } else {
            anyhow::ensure!(
                section.irradiance_format == IRRADIANCE_FORMAT_RGBA16F,
                "reflection probes sample the packed RGBA16F SH volume, found format {}",
                section.irradiance_format
            );
            probe_indirection(section)?
        };
        Ok(Self {
            section,
            indirection,
            probe_occlusion,
        })
    }

    /// Irradiance for a surface at `position` with unit `normal` (shading,
    /// offset and geometric normal alike: the capture reads no normal map).
    pub(crate) fn indirect(&self, position: Vec3, normal: Vec3) -> Vec3 {
        if self.indirection.is_empty() {
            return Vec3::ZERO;
        }
        let s = self.section;
        let cell = Vec3::from(s.cell_size);
        let origin = Vec3::from(s.grid_origin);
        let dims = UVec3::from(s.grid_dimensions);
        let offset_world = position + normal * SH_NORMAL_OFFSET_M * cell;
        let gdims_f = (dims.as_vec3() - Vec3::ONE).max(Vec3::ZERO);
        let cell_coord = (offset_world - origin) / cell.max(Vec3::splat(1.0e-6));
        let gf = cell_coord.clamp(Vec3::ZERO, gdims_f);
        let gi = gf.floor().as_uvec3();
        let gfrac = gf - gf.floor();

        let mut sum = Vec3::ZERO;
        let mut weight_sum = 0.0;
        for corner in 0..8u32 {
            let offset = UVec3::new(corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let idx = (gi + offset).min(dims - UVec3::ONE);
            let probe = (idx.x + idx.y * dims.x + idx.z * dims.x * dims.y) as usize;
            let indirection = self.indirection[probe];
            let sample = match indirection {
                Some(ind) => self.sample_resolved(idx, ind, normal),
                None => Vec4::ZERO,
            };
            let weight = self.probe_weight(
                probe,
                idx,
                offset,
                gfrac,
                offset_world,
                normal,
                indirection.is_some(),
            );
            sum += weight * sample.truncate().max(Vec3::ZERO);
            weight_sum += weight;
        }
        if weight_sum < SH_WEIGHT_EPSILON {
            return Vec3::ZERO;
        }
        sum / weight_sum
    }

    #[allow(clippy::too_many_arguments)]
    fn probe_weight(
        &self,
        probe: usize,
        idx: UVec3,
        offset: UVec3,
        gfrac: Vec3,
        sample_world: Vec3,
        geo_normal: Vec3,
        valid: bool,
    ) -> f32 {
        if !valid {
            return 0.0;
        }
        let high = offset.cmpgt(UVec3::ZERO);
        let axis = Vec3::select(high, gfrac, Vec3::ONE - gfrac);
        let trilinear = axis.x * axis.y * axis.z;
        let cell = Vec3::from(self.section.cell_size);
        let dir_to_probe = (offset.as_vec3() - gfrac) * cell;
        let backface = dir_to_probe.dot(geo_normal).max(0.0);
        let visibility = if self.probe_occlusion {
            self.depth_visibility(probe, idx, sample_world)
        } else {
            1.0
        };
        trilinear * backface * visibility
    }

    /// Chebyshev visibility from the probe's baked depth moments.
    fn depth_visibility(&self, probe: usize, idx: UVec3, sample_world: Vec3) -> f32 {
        let s = self.section;
        let record = &s.probes[probe];
        let mean = f16_bits_to_f32(record.mean_distance);
        let mean2 = f16_bits_to_f32(record.mean_sq_distance);
        let variance = (mean2 - mean * mean).max(SH_DEPTH_MIN_VARIANCE_M2);
        let cell = Vec3::from(s.cell_size);
        let probe_world = Vec3::from(s.grid_origin) + idx.as_vec3() * cell;
        let distance = (sample_world - probe_world).length();
        let bias = cell.min_element().max(0.0) * SH_DEPTH_BIAS_CELL_FRACTION;
        let delta = (distance - mean - bias).max(0.0);
        let visibility = if delta > 0.0 {
            variance / (variance + delta * delta)
        } else {
            1.0
        };
        visibility.clamp(SH_DEPTH_MIN_VISIBILITY, 1.0)
    }

    fn sample_resolved(&self, idx: UVec3, ind: ProbeIndirection, dir: Vec3) -> Vec4 {
        match ind.level {
            Level::L1 => self.sample_l1(idx, ind, dir),
            Level::L0 | Level::L2 => self.sample_slot(ind.slot, dir),
        }
    }

    /// `sample_l1_probe_atlas`: the probe's value reconstructed from its
    /// node's eight stored corners, present corners only.
    fn sample_l1(&self, idx: UVec3, ind: ProbeIndirection, dir: Vec3) -> Vec4 {
        let edge = u32::from(AFFINITY_FACTOR) << ind.scale;
        let local = idx - (idx / edge) * edge;
        let fraction = local.as_vec3() / (edge - 1) as f32;
        let mut sum = Vec3::ZERO;
        let mut weight_sum = 0.0;
        for corner in 0..8u32 {
            let high =
                UVec3::new(corner & 1, (corner >> 1) & 1, (corner >> 2) & 1).cmpne(UVec3::ZERO);
            let axis = Vec3::select(high, fraction, Vec3::ONE - fraction);
            let weight = axis.x * axis.y * axis.z;
            if weight <= 0.0 {
                continue;
            }
            let sample = self.sample_slot(ind.slot + corner, dir);
            let present = weight * sample.w;
            sum += present * sample.truncate().max(Vec3::ZERO);
            weight_sum += present;
        }
        if weight_sum < SH_WEIGHT_EPSILON {
            return Vec4::ZERO;
        }
        (sum / weight_sum).extend(1.0)
    }

    /// `sample_probe_atlas_slot`: one bilinear tap at the direction's
    /// octahedral coordinate inside the slot's tile.
    fn sample_slot(&self, slot: u32, dir: Vec3) -> Vec4 {
        let s = self.section;
        let tile = s.tile_dimension as usize;
        let interior = s.tile_dimension.saturating_sub(2 * s.tile_border).max(1) as f32;
        let dir = dir.normalize_or_zero();
        let oct = encode_unquantized_uv(dir.x, dir.y, dir.z);
        let border = s.tile_border as f32;
        let texel = [border + oct[0] * interior, border + oct[1] * interior];
        let [layer, tile_x, tile_y] =
            irradiance_array_tile_location(slot as usize, s.tiles_per_layer, s.atlas_tiles_per_row);
        let mut taps = [[0usize; 2]; 2];
        let mut fraction = [0.0f32; 2];
        for axis in 0..2 {
            let p = texel[axis] - 0.5;
            let lo = p.floor();
            let lo_i = (lo.max(0.0) as usize).min(tile - 1);
            taps[axis] = [lo_i, (lo_i + 1).min(tile - 1)];
            fraction[axis] = p - lo;
        }
        let width = s.atlas_dimensions[0] as usize;
        let layer_texels = width * s.atlas_dimensions[1] as usize;
        let fetch = |x: usize, y: usize| {
            let texel = layer as usize * layer_texels
                + (tile_y as usize * tile + y) * width
                + tile_x as usize * tile
                + x;
            let at = texel * 8;
            let channel = |c: usize| {
                f16_bits_to_f32(u16::from_le_bytes([
                    s.compact_atlas[at + c * 2],
                    s.compact_atlas[at + c * 2 + 1],
                ]))
            };
            Vec4::new(channel(0), channel(1), channel(2), channel(3))
        };
        let [fx, fy] = fraction;
        let top = fetch(taps[0][0], taps[1][0]).lerp(fetch(taps[0][1], taps[1][0]), fx);
        let bottom = fetch(taps[0][0], taps[1][1]).lerp(fetch(taps[0][1], taps[1][1]), fx);
        top.lerp(bottom, fy)
    }
}

/// Per-probe indirection from id-34 metadata and its brick-major stored-tile
/// prefix sum, as the renderer's `build_probe_indirection_words` derives it at
/// load: an L0 brick's valid probes take consecutive slots, an L1 or L2 node's
/// probes share the node's base slot.
fn probe_indirection(
    section: &OctahedralShVolumeSection,
) -> anyhow::Result<Vec<Option<ProbeIndirection>>> {
    let grid = section.grid_dimensions;
    let factor = usize::from(AFFINITY_FACTOR);
    let affinity = grid.map(|axis| axis.div_ceil(u32::from(AFFINITY_FACTOR)) as usize);
    let (gx, gy) = (grid[0] as usize, grid[1] as usize);
    let mut brick_levels = Vec::with_capacity(affinity[0] * affinity[1] * affinity[2]);
    let mut brick_scales = Vec::with_capacity(brick_levels.capacity());
    for bz in 0..affinity[2] {
        for by in 0..affinity[1] {
            for bx in 0..affinity[0] {
                let probe = &section.probes[bx * factor + by * factor * gx + bz * factor * gx * gy];
                brick_levels
                    .push(Level::from_u8(probe.density_level).ok_or_else(|| {
                        anyhow::anyhow!("SH probe density level is out of range")
                    })?);
                brick_scales.push(probe.node_scale);
            }
        }
    }
    let validity: Vec<bool> = section.probes.iter().map(|p| p.validity != 0).collect();
    let prefix = stored_node_prefix_sum(grid, &brick_levels, &brick_scales, &validity)
        .ok_or_else(|| anyhow::anyhow!("SH stored-tile prefix sum rejects the packed volume"))?;

    let mut out = vec![None; section.probes.len()];
    for bz in 0..affinity[2] {
        for by in 0..affinity[1] {
            for bx in 0..affinity[0] {
                let brick = bx + by * affinity[0] + bz * affinity[0] * affinity[1];
                let level = brick_levels[brick];
                let scale = brick_scales[brick];
                let edge = 1usize << scale;
                let node = (bx / edge * edge)
                    + (by / edge * edge) * affinity[0]
                    + (bz / edge * edge) * affinity[0] * affinity[1];
                let base_slot = prefix.bricks[node].base_slot;
                let mut l0_offset = 0u32;
                for lz in 0..factor {
                    for ly in 0..factor {
                        for lx in 0..factor {
                            let (x, y, z) = (bx * factor + lx, by * factor + ly, bz * factor + lz);
                            if x >= gx || y >= gy || z >= grid[2] as usize {
                                continue;
                            }
                            let probe = x + y * gx + z * gx * gy;
                            if !validity[probe] {
                                continue;
                            }
                            let slot = match level {
                                Level::L0 => {
                                    l0_offset += 1;
                                    base_slot + l0_offset - 1
                                }
                                Level::L1 | Level::L2 => base_slot,
                            };
                            out[probe] = Some(ProbeIndirection { level, scale, slot });
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection_probes::reflection_probes_test_fixtures::dense_sh_volume;
    use crate::sh_density::pack_indirect_section;

    /// Uniform-tile probes carrying a linear field, so trilinear blending and
    /// an L1 node's corner reconstruction both reproduce it exactly.
    fn field(x: u32, y: u32, z: u32) -> f32 {
        x as f32 + 10.0 * y as f32 + 100.0 * z as f32
    }

    #[test]
    fn sh_sampler_blends_the_packed_volume_as_the_forward_pass_does() {
        let dense = dense_sh_volume([4, 4, 4], |x, y, z| [field(x, y, z), 1.0, 0.0]);
        let (packed, _) = pack_indirect_section(dense, Some(Level::L0)).unwrap();
        let sampler = ShVolumeSampler::new(&packed, false).unwrap();

        // On the probe plane y = 1.4 the 0.1-cell normal offset lifts the read
        // to y = 1.5; an up normal rejects the four probes below, leaving the
        // bilinear blend of the upper layer (y = 2) at x frac 0.25, z frac 0.5.
        let value = sampler.indirect(Vec3::new(1.25, 1.4, 2.5), Vec3::Y);
        let expected = 1.25 + 20.0 + 250.0;
        assert!((value.x - expected).abs() < 0.01, "{value}");
        assert!((value.y - 1.0).abs() < 1e-3, "{value}");

        // A down normal keeps the lower layer instead (offset read at y 1.3).
        let below = sampler.indirect(Vec3::new(1.25, 1.4, 2.5), Vec3::NEG_Y);
        assert!((below.x - (1.25 + 10.0 + 250.0)).abs() < 0.01, "{below}");
    }

    #[test]
    fn sh_sampler_reads_l1_and_l2_nodes_through_their_shared_slots() {
        let dense = || dense_sh_volume([4, 4, 4], |x, y, z| [field(x, y, z), 1.0, 0.0]);
        let (l0, _) = pack_indirect_section(dense(), Some(Level::L0)).unwrap();
        let (l1, _) = pack_indirect_section(dense(), Some(Level::L1)).unwrap();
        let l0 = ShVolumeSampler::new(&l0, false).unwrap();
        let l1 = ShVolumeSampler::new(&l1, false).unwrap();
        for point in [
            Vec3::new(1.25, 1.4, 2.5),
            Vec3::new(0.3, 2.2, 0.9),
            Vec3::new(2.9, 0.1, 1.6),
        ] {
            let (a, b) = (l0.indirect(point, Vec3::Y), l1.indirect(point, Vec3::Y));
            assert!((a - b).length() < 0.05, "{point}: L0 {a} vs L1 {b}");
        }

        // L2 stores one tile for the brick: every read returns it.
        let (l2, _) = pack_indirect_section(dense(), Some(Level::L2)).unwrap();
        let l2 = ShVolumeSampler::new(&l2, false).unwrap();
        let a = l2.indirect(Vec3::new(0.3, 2.2, 0.9), Vec3::Y);
        let b = l2.indirect(Vec3::new(2.9, 0.1, 1.6), Vec3::Y);
        assert!((a - b).length() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn sh_sampler_drops_invalid_probes_and_reads_zero_with_none_left() {
        let dense = dense_sh_volume([4, 4, 4], |_, y, _| {
            if y >= 2 {
                [f32::NAN; 3]
            } else {
                [2.0, 2.0, 2.0]
            }
        });
        let (packed, _) = pack_indirect_section(dense, Some(Level::L0)).unwrap();
        let sampler = ShVolumeSampler::new(&packed, false).unwrap();
        // Only the lower layer is valid; an up normal rejects it entirely.
        assert_eq!(
            sampler.indirect(Vec3::new(1.5, 1.4, 1.5), Vec3::Y),
            Vec3::ZERO
        );
        let down = sampler.indirect(Vec3::new(1.5, 1.4, 1.5), Vec3::NEG_Y);
        assert!((down - Vec3::splat(2.0)).length() < 1e-3, "{down}");
    }

    #[test]
    fn probe_occlusion_mirrors_the_renderer_default() {
        assert_eq!(
            PRODUCTION_PROBE_OCCLUSION,
            postretro_render_cpu::sh_volume::DEFAULT_PROBE_OCCLUSION
        );
    }
}
