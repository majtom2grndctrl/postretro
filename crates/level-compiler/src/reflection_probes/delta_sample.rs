// Reflection-probe animated indirect: each animated light's delta SH as the composed atlas carries it.
// See: context/lib/rendering_pipeline.md §4 (Animated SH delta volumes), §7.1 step 5 · shaders sh_compose.wgsl

use glam::Vec3;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::sh_reconstruct::{Level, kept_mask, local_xyz, trilinear_weight};

use super::sh_sample::ShVolumeSampler;
use super::shading::AnimatedTerms;
use crate::bc6h::f16_bits_to_f32;

/// Each light's unit-radiance indirect at a capture hit, from id 27 as the
/// level ships it (after exact-zero drop, coarsening and valid-probe
/// compaction). `sh_compose.wgsl` adds every light's delta, reconstructed for
/// the stored slot's writer probe and scaled by its descriptor, into the
/// stored slots the forward pass samples; the descriptor scale is the only
/// non-linear step, so a light's unit indirect is the base sample's weighted
/// sum over that light's delta texels.
pub(crate) struct DeltaShSampler<'a> {
    section: &'a DeltaShVolumesSection,
    /// f16-half payload offset of each CSR entry's kept tiles.
    entry_offsets: Vec<u32>,
    /// Writer `(affinity cell, local probe)` of each base stored slot.
    slot_writers: Vec<Option<(u32, u8)>>,
    tile_dimension: usize,
}

impl<'a> DeltaShSampler<'a> {
    /// Fails when id 27 does not share the base volume's tile and affinity
    /// geometry, which the loader would also refuse.
    pub(crate) fn new(
        section: &'a DeltaShVolumesSection,
        base: &ShVolumeSampler<'_>,
        base_tile_dimension: u32,
        base_grid: [u32; 3],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            section.tile_dimension == base_tile_dimension,
            "delta SH tile dimension {} differs from the base volume's {base_tile_dimension}",
            section.tile_dimension
        );
        let factor = u32::from(AFFINITY_FACTOR);
        let affinity = base_grid.map(|axis| axis.div_ceil(factor));
        anyhow::ensure!(
            section.affinity_dims == affinity,
            "delta SH affinity grid {:?} differs from the base volume's {affinity:?}",
            section.affinity_dims
        );
        let cells = section.affinity_cell_count();
        anyhow::ensure!(
            section.affinity_offsets.len() == cells + 1
                && section.cell_levels.len() == cells
                && section.valid_probe_masks.len() == cells,
            "delta SH metadata does not cover its affinity grid"
        );
        let stride = section.delta_probe_f16_stride() as u32;
        let mut entry_offsets = Vec::with_capacity(section.affinity_lights.len());
        let mut offset = 0u32;
        for cell in 0..cells {
            let level = cell_level(section, cell)?;
            let tiles = kept_mask(level, section.valid_probe_masks[cell]).count_ones();
            let range = section.affinity_offsets[cell]..section.affinity_offsets[cell + 1];
            for _ in range {
                entry_offsets.push(offset);
                offset += tiles * stride;
            }
        }
        anyhow::ensure!(
            entry_offsets.len() == section.affinity_lights.len()
                && offset as usize == section.delta_subblocks.len(),
            "delta SH payload length does not match its CSR"
        );
        Ok(Self {
            section,
            entry_offsets,
            slot_writers: base.slot_writers(),
            tile_dimension: section.tile_dimension as usize,
        })
    }

    /// Add each light's unit-radiance indirect, times `scale`, to `out`:
    /// the base sample's `slots` and the tile `taps` read over the light's
    /// composed delta texels.
    pub(crate) fn accumulate(
        &self,
        slots: &[(u32, f32)],
        taps: &[(usize, usize, f32); 4],
        scale: Vec3,
        out: &mut AnimatedTerms,
    ) {
        for &(slot, slot_weight) in slots {
            let Some(Some((cell, local))) = self.slot_writers.get(slot as usize).copied() else {
                continue;
            };
            let cell = cell as usize;
            let range = self.section.affinity_offsets[cell] as usize
                ..self.section.affinity_offsets[cell + 1] as usize;
            for entry in range {
                let mut value = Vec3::ZERO;
                for &(x, y, tap_weight) in taps {
                    if tap_weight > 0.0 {
                        value +=
                            self.composed_texel(entry, cell, local, y * self.tile_dimension + x)
                                * tap_weight;
                    }
                }
                if value != Vec3::ZERO {
                    out.add(
                        self.section.affinity_lights[entry],
                        value * slot_weight * scale,
                    );
                }
            }
        }
    }

    /// One texel of entry `entry`'s delta as the compose pass adds it into
    /// the slot written by `local` of `cell`: an L0 kept probe's stored tile;
    /// an L1 brick's present kept corners, trilinearly blended for `local`; an
    /// L2 brick's one representative tile.
    fn composed_texel(&self, entry: usize, cell: usize, local: u8, texel: usize) -> Vec3 {
        let section = self.section;
        let Ok(level) = cell_level(section, cell) else {
            return Vec3::ZERO;
        };
        let kept = kept_mask(level, section.valid_probe_masks[cell]);
        let rank_of = |probe: usize| (kept & ((1u64 << probe) - 1)).count_ones();
        let read = |probe: usize| self.read_texel(entry, rank_of(probe), texel);
        let local = usize::from(local);
        match level {
            Level::L0 => {
                if kept & (1u64 << local) != 0 {
                    read(local)
                } else {
                    Vec3::ZERO
                }
            }
            Level::L1 => {
                let target = local_xyz(local);
                let (mut sum, mut weight_sum) = (Vec3::ZERO, 0.0);
                for corner in postretro_level_format::sh_reconstruct::corner_locals() {
                    if kept & (1u64 << corner) == 0 {
                        continue;
                    }
                    let weight = trilinear_weight(target, local_xyz(corner));
                    if weight > 0.0 {
                        sum += read(corner) * weight;
                        weight_sum += weight;
                    }
                }
                if weight_sum > 0.0 {
                    sum / weight_sum
                } else {
                    Vec3::ZERO
                }
            }
            Level::L2 => {
                if kept == 0 {
                    Vec3::ZERO
                } else {
                    read(kept.trailing_zeros() as usize)
                }
            }
        }
    }

    fn read_texel(&self, entry: usize, rank: u32, texel: usize) -> Vec3 {
        let stride = self.section.delta_probe_f16_stride();
        let base = self.entry_offsets[entry] as usize + rank as usize * stride + texel * 3;
        let half = |i: usize| f16_bits_to_f32(self.section.delta_subblocks[base + i]);
        Vec3::new(half(0), half(1), half(2))
    }
}

fn cell_level(section: &DeltaShVolumesSection, cell: usize) -> anyhow::Result<Level> {
    Level::from_u8(section.cell_levels[cell])
        .ok_or_else(|| anyhow::anyhow!("delta SH cell {cell} has an unknown level"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection_probes::reflection_probes_test_fixtures::dense_sh_volume;
    use crate::sh_density::pack_indirect_section;
    use postretro_level_format::lightmap::f32_to_f16_bits;

    /// A one-cell 4³ delta section whose single light's tile at local probe
    /// `l` is uniform `value(l)` over every texel.
    fn delta_section(
        level: Level,
        light: u32,
        value: impl Fn(usize) -> f32,
    ) -> DeltaShVolumesSection {
        let mask = u64::MAX;
        let kept = kept_mask(level, mask);
        let tile = 6usize;
        let mut payload = Vec::new();
        for probe in 0..64 {
            if kept & (1u64 << probe) == 0 {
                continue;
            }
            for _ in 0..tile * tile {
                payload.extend_from_slice(&[f32_to_f16_bits(value(probe)), 0, 0]);
            }
        }
        DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: tile as u32,
            tile_border: 1,
            animation_descriptor_indices: (0..=light).collect(),
            valid_probe_masks: vec![mask],
            cell_levels: vec![level.to_u8()],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![light],
            delta_subblocks: payload,
        }
    }

    fn base() -> postretro_level_format::sh_volume::OctahedralShVolumeSection {
        let dense = dense_sh_volume([4, 4, 4], |_, _, _| [1.0, 1.0, 1.0]);
        pack_indirect_section(dense, Some(Level::L0)).unwrap().0
    }

    /// A linear delta field: trilinear blending reproduces it, so the unit
    /// indirect at a point is the field there, at every coarsening level that
    /// keeps it.
    fn field(probe: usize) -> f32 {
        let (x, y, z) = local_xyz(probe);
        1.0 + x as f32 + 2.0 * y as f32 + 4.0 * z as f32
    }

    #[test]
    fn delta_indirect_follows_the_base_sample_weights_at_every_level() {
        let base = base();
        let sampler = ShVolumeSampler::new(&base, false).unwrap();
        let point = Vec3::new(1.25, 1.4, 2.5);
        let mut slots = Vec::new();
        let base_value = sampler.indirect_and_slots(point, Vec3::Y, Some(&mut slots));
        assert!((base_value - Vec3::ONE).length() < 1e-3, "{base_value}");
        let weight_total: f32 = slots.iter().map(|s| s.1).sum();
        assert!((weight_total - 1.0).abs() < 1e-5, "{slots:?}");
        let taps = sampler.tile_taps(Vec3::Y);

        // Up normal at y = 1.4 reads the y = 2 layer at x frac 0.25, z frac 0.5.
        let expected = 1.0 + 1.25 + 2.0 * 2.0 + 4.0 * 2.5;
        for level in [Level::L0, Level::L1] {
            let delta = delta_section(level, 3, field);
            let deltas = DeltaShSampler::new(&delta, &sampler, 6, [4, 4, 4]).unwrap();
            let mut out = AnimatedTerms::default();
            deltas.accumulate(&slots, &taps, Vec3::ONE, &mut out);
            assert_eq!(out.lights().len(), 1);
            let (light, value) = out.lights()[0];
            assert_eq!(light, 3);
            assert!((value.x - expected).abs() < 2e-2, "{level:?}: {value}");
            assert_eq!(value.y, 0.0);
        }
        // L2 keeps one representative tile for the whole brick.
        let delta = delta_section(Level::L2, 0, |_| 2.5);
        let deltas = DeltaShSampler::new(&delta, &sampler, 6, [4, 4, 4]).unwrap();
        let mut out = AnimatedTerms::default();
        deltas.accumulate(&slots, &taps, Vec3::splat(2.0), &mut out);
        assert!(
            (out.lights()[0].1.x - 5.0).abs() < 1e-2,
            "{:?}",
            out.lights()
        );
    }

    #[test]
    fn a_delta_of_the_wrong_geometry_is_refused() {
        let base = base();
        let sampler = ShVolumeSampler::new(&base, false).unwrap();
        let mut delta = delta_section(Level::L0, 0, field);
        delta.affinity_dims = [2, 1, 1];
        assert!(DeltaShSampler::new(&delta, &sampler, 6, [4, 4, 4]).is_err());
    }
}
