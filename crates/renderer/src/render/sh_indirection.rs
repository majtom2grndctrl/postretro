// Load-derived SH probe indirection shared by the compose and sampler paths.
//
// The word is deliberately not serialized. id-34 metadata is the only source
// of validity, brick density level, and the stored-atlas slot mapping.

use postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
use postretro_level_format::sh_reconstruct::{Level, stored_brick_prefix_sum};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_render_cpu::sh_compose::u32_slice_to_bytes;

/// Low two bits name the brick's storage level (L0/L1/L2).
pub(super) const SH_INDIRECTION_LEVEL_MASK: u32 = 0x0000_0003;
/// A nonzero word is valid only when this bit is set.
pub(super) const SH_INDIRECTION_VALID_BIT: u32 = 0x0000_0004;
/// Stored-atlas slots occupy the remaining high bits.
pub(super) const SH_INDIRECTION_SLOT_SHIFT: u32 = 3;
pub(super) const SH_INDIRECTION_SLOT_BITS: u32 = u32::BITS - SH_INDIRECTION_SLOT_SHIFT;
pub(super) const SH_INDIRECTION_MAX_SLOT: u32 = u32::MAX >> SH_INDIRECTION_SLOT_SHIFT;

/// The all-zero word is the sole invalid/sentinel representation.
pub(super) const INVALID_PROBE_INDIRECTION: u32 = 0;

/// Canonical decode helper appended to all WGSL consumers by the sampler task.
/// It declares no resources, so callers keep ownership of their bindings.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const WGSL_DECODE_HELPER: &str = include_str!("../shaders/sh_indirection.wgsl");

/// Decode a word for CPU assertions and diagnostics. Invalid words have no
/// level or slot; consumers must test `valid` before using either field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ProbeIndirectionWord {
    pub(super) valid: bool,
    pub(super) level: u32,
    pub(super) slot: u32,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn decode_probe_indirection_word(word: u32) -> ProbeIndirectionWord {
    ProbeIndirectionWord {
        valid: word & SH_INDIRECTION_VALID_BIT != 0,
        level: word & SH_INDIRECTION_LEVEL_MASK,
        slot: word >> SH_INDIRECTION_SLOT_SHIFT,
    }
}

fn encode_probe_indirection_word(level: Level, slot: u32) -> u32 {
    assert!(
        slot <= SH_INDIRECTION_MAX_SLOT,
        "SH stored-atlas slot {slot} exceeds the {}-bit probe-indirection field",
        SH_INDIRECTION_SLOT_BITS,
    );
    (slot << SH_INDIRECTION_SLOT_SHIFT)
        | SH_INDIRECTION_VALID_BIT
        | (u32::from(level.to_u8()) & SH_INDIRECTION_LEVEL_MASK)
}

/// Derive one word per dense grid probe from id-34 metadata and its brick-major
/// stored-tile prefix sum. This is the only builder: the indirect compose,
/// direct compose, animated-direct compose, and depth-moment B/A payload all
/// receive this exact returned array.
pub(super) fn build_probe_indirection_words(
    sh_section: Option<&OctahedralShVolumeSection>,
) -> Vec<u32> {
    let Some(section) = sh_section else {
        return vec![INVALID_PROBE_INDIRECTION];
    };

    let grid = section.grid_dimensions;
    let probe_count = section.total_probes();
    debug_assert_eq!(section.probes.len(), probe_count);
    if probe_count == 0 {
        return vec![INVALID_PROBE_INDIRECTION];
    }

    let affinity_dims = grid.map(|axis| axis.div_ceil(u32::from(AFFINITY_FACTOR)));
    let mut brick_levels = Vec::with_capacity(
        (affinity_dims[0] as usize) * (affinity_dims[1] as usize) * (affinity_dims[2] as usize),
    );
    for brick_z in 0..affinity_dims[2] {
        for brick_y in 0..affinity_dims[1] {
            for brick_x in 0..affinity_dims[0] {
                let probe_index = brick_x as usize * AFFINITY_FACTOR as usize
                    + brick_y as usize * AFFINITY_FACTOR as usize * grid[0] as usize
                    + brick_z as usize
                        * AFFINITY_FACTOR as usize
                        * grid[0] as usize
                        * grid[1] as usize;
                let level = Level::from_u8(section.probes[probe_index].density_level)
                    .expect("id-34 metadata was validated before renderer resource creation");
                brick_levels.push(level);
            }
        }
    }
    let validity: Vec<bool> = section
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let prefix = stored_brick_prefix_sum(grid, &brick_levels, &validity)
        .expect("id-34 metadata was validated before renderer resource creation");

    let mut words = vec![INVALID_PROBE_INDIRECTION; probe_count];
    let factor = AFFINITY_FACTOR as usize;
    for brick_z in 0..affinity_dims[2] as usize {
        for brick_y in 0..affinity_dims[1] as usize {
            for brick_x in 0..affinity_dims[0] as usize {
                let brick_index = brick_x
                    + brick_y * affinity_dims[0] as usize
                    + brick_z * affinity_dims[0] as usize * affinity_dims[1] as usize;
                let level = brick_levels[brick_index];
                let range = prefix.bricks[brick_index];
                let mut l0_slot_offset = 0u32;
                for local_z in 0..factor {
                    for local_y in 0..factor {
                        for local_x in 0..factor {
                            let x = brick_x * factor + local_x;
                            let y = brick_y * factor + local_y;
                            let z = brick_z * factor + local_z;
                            if x >= grid[0] as usize
                                || y >= grid[1] as usize
                                || z >= grid[2] as usize
                            {
                                continue;
                            }
                            let probe_index =
                                x + y * grid[0] as usize + z * grid[0] as usize * grid[1] as usize;
                            if !validity[probe_index] {
                                continue;
                            }
                            let slot = match level {
                                Level::L0 => {
                                    let slot = range.base_slot + l0_slot_offset;
                                    l0_slot_offset += 1;
                                    slot
                                }
                                Level::L1 | Level::L2 => range.base_slot,
                            };
                            words[probe_index] = encode_probe_indirection_word(level, slot);
                        }
                    }
                }
                debug_assert!(
                    level != Level::L0 || l0_slot_offset == range.stored_tile_count,
                    "the L0 stored-slot prefix must exactly cover valid probes",
                );
            }
        }
    }
    words
}

/// Exact byte carrier for every compose storage buffer. Keeping conversion
/// here prevents one pass from accidentally using a different endianness or
/// padded representation than the other two.
pub(super) fn probe_indirection_storage_bytes(words: &[u32]) -> Vec<u8> {
    u32_slice_to_bytes(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
        irradiance_atlas_array_layout,
    };
    use postretro_level_format::sh_volume::{OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe};

    fn fixture() -> OctahedralShVolumeSection {
        let grid = [16, 4, 4];
        let layout =
            irradiance_atlas_array_layout([73, 1, 1], DEFAULT_IRRADIANCE_TILE_DIMENSION, 8192)
                .unwrap();
        let mut probes = vec![
            OctahedralShProbe {
                validity: 1,
                ..Default::default()
            };
            16 * 4 * 4
        ];
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..16 {
                    let index = x + y * 16 + z * 16 * 4;
                    probes[index].density_level = (x / 4) as u8;
                    if x >= 12 {
                        probes[index].validity = 0;
                        probes[index].density_level = 2;
                    }
                }
            }
        }
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: grid,
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [layout.atlas_width, layout.atlas_height],
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            compact_atlas: vec![
                0;
                (layout.layer_count
                    * layout.atlas_width.div_ceil(4)
                    * layout.atlas_height.div_ceil(4)
                    * 16) as usize
            ],
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn builder_maps_levels_slots_and_empty_bricks_from_metadata() {
        let words = build_probe_indirection_words(Some(&fixture()));
        assert_eq!(words.len(), 16 * 4 * 4);

        let l0_first = decode_probe_indirection_word(words[0]);
        let l0_last = decode_probe_indirection_word(words[3 + 3 * 16 + 3 * 16 * 4]);
        assert_eq!(
            l0_first,
            ProbeIndirectionWord {
                valid: true,
                level: 0,
                slot: 0
            }
        );
        assert_eq!(
            l0_last,
            ProbeIndirectionWord {
                valid: true,
                level: 0,
                slot: 63
            }
        );

        let l1_a = decode_probe_indirection_word(words[4]);
        let l1_b = decode_probe_indirection_word(words[7 + 2 * 16 + 1 * 16 * 4]);
        assert_eq!(
            l1_a,
            ProbeIndirectionWord {
                valid: true,
                level: 1,
                slot: 64
            }
        );
        assert_eq!(l1_b, l1_a);

        let l2_a = decode_probe_indirection_word(words[8]);
        let l2_b = decode_probe_indirection_word(words[11 + 1 * 16 + 2 * 16 * 4]);
        assert_eq!(
            l2_a,
            ProbeIndirectionWord {
                valid: true,
                level: 2,
                slot: 72
            }
        );
        assert_eq!(l2_b, l2_a);

        assert_eq!(words[12], INVALID_PROBE_INDIRECTION);
        assert_eq!(decode_probe_indirection_word(words[12]).valid, false);
    }

    #[test]
    fn invalid_and_missing_sections_use_the_all_zero_sentinel() {
        assert_eq!(
            build_probe_indirection_words(None),
            vec![INVALID_PROBE_INDIRECTION]
        );
        assert_eq!(
            decode_probe_indirection_word(INVALID_PROBE_INDIRECTION),
            ProbeIndirectionWord {
                valid: false,
                level: 0,
                slot: 0,
            }
        );
    }

    #[test]
    fn wgsl_decode_constants_match_the_rust_contract() {
        assert!(SH_INDIRECTION_SLOT_BITS >= 28);
        for (name, value) in [
            ("SH_INDIRECTION_LEVEL_MASK", SH_INDIRECTION_LEVEL_MASK),
            ("SH_INDIRECTION_VALID_BIT", SH_INDIRECTION_VALID_BIT),
            ("SH_INDIRECTION_SLOT_SHIFT", SH_INDIRECTION_SLOT_SHIFT),
        ] {
            let literal = format!("const {name}: u32 = 0x{value:08x}u");
            assert!(
                WGSL_DECODE_HELPER.contains(&literal),
                "missing WGSL constant {literal}"
            );
        }
        naga::front::wgsl::parse_str(WGSL_DECODE_HELPER)
            .expect("the canonical SH indirection decode helper must remain valid WGSL");
    }

    #[test]
    fn compose_carriers_share_one_word_array_and_moments_pack_the_same_words() {
        let section = fixture();
        let words = build_probe_indirection_words(Some(&section));
        let indirect_carrier = probe_indirection_storage_bytes(&words);
        let direct_carrier = probe_indirection_storage_bytes(&words);
        let animated_direct_carrier = probe_indirection_storage_bytes(&words);
        assert_eq!(indirect_carrier, direct_carrier);
        assert_eq!(direct_carrier, animated_direct_carrier);

        let packed = crate::render::sh_volume::pack_probe_depth_moments(
            &section.probes,
            section.grid_dimensions,
            &words,
        );
        for (probe, &word) in words.iter().enumerate() {
            let packed_word =
                u32::from(packed[probe * 4 + 2]) | (u32::from(packed[probe * 4 + 3]) << 16);
            assert_eq!(packed_word, word, "probe {probe} moment B/A word diverged");
        }
    }
}
