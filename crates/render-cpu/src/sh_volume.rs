// SH volume bind slots, uniform packing, and animation payload sizing.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};

pub const BIND_SH_TOTAL_ATLAS: u32 = 1;
pub const BIND_SH_ATLAS_SAMPLER: u32 = 2;
pub const BIND_SH_GRID_INFO: u32 = 10;
pub const BIND_ANIM_DESCRIPTORS: u32 = 11;
pub const BIND_ANIM_SAMPLES: u32 = 12;
pub const BIND_SCRIPTED_LIGHT_DESCRIPTORS: u32 = 13;
pub const BIND_SH_DEPTH_MOMENTS: u32 = BIND_SCRIPTED_LIGHT_DESCRIPTORS + 1;
pub const BIND_SH_DIRECT_ATLAS: u32 = BIND_SH_DEPTH_MOMENTS + 1;
pub const BIND_DYNAMIC_DIRECT_PARAMS: u32 = BIND_SH_DIRECT_ATLAS + 1;
/// Billboard-only normal-free direct-scatter volume. This remains outside the
/// mesh-only dynamic-direct extension at binding 16, so the shared group-3
/// layout can expose it to the billboard vertex stage without changing mesh
/// bindings.
pub const BIND_BILLBOARD_DIRECT_SCATTER: u32 = BIND_DYNAMIC_DIRECT_PARAMS + 1;
pub const DYNAMIC_DIRECT_PARAMS_SIZE: usize = 16;

/// Byte size of `ShGridInfo` — six `vec4` slots to satisfy std140 alignment
/// rules (vec3 fields align to 16, followed by a same-slot scalar).
///
/// Layout (must match the WGSL `ShGridInfo` structs in shader consumers):
///   0..12   grid_origin       (vec3<f32>)
///   12..16  has_sh_volume     (u32, 0 or 1)
///   16..28  cell_size         (vec3<f32>)
///   28..32  _pad0             (u32)
///   32..44  grid_dimensions   (vec3<u32>)
///   44..48  _pad1             (u32)
///   48..56  atlas_dimensions  (vec2<u32>)
///   56..60  tile_dimension    (u32)
///   60..64  tile_border       (u32)
///   64..68  atlas_tiles_per_row (u32)
///   68..72  atlas_tile_rows   (u32)
///   72..76  tile_interior     (u32)
///   76..80  _pad2             (u32)
///   80..84  probe_occlusion   (u32, 0 or 1)
///   84..88  tiles_per_layer   (u32)
///   88..92  atlas_layer_count (u32)
///   92..96  _pad3             (u32)
pub const SH_GRID_INFO_SIZE: usize = 96;
pub const DEFAULT_PROBE_OCCLUSION: bool = true;
pub const ANIMATION_DESCRIPTOR_SIZE: usize = 48;
pub const ANIMATION_DESCRIPTOR_ACTIVE_OFFSET: usize = 36;
pub const SCRIPTED_BRIGHTNESS_SLOT: usize = 128;
pub const SCRIPTED_COLOR_SLOT_F32: usize = 128;
pub const SCRIPTED_FLOATS_PER_LIGHT: usize = SCRIPTED_BRIGHTNESS_SLOT + SCRIPTED_COLOR_SLOT_F32;

pub fn build_dynamic_direct_params_bytes(
    scale: f32,
    has_direct: bool,
) -> [u8; DYNAMIC_DIRECT_PARAMS_SIZE] {
    let mut bytes = [0u8; DYNAMIC_DIRECT_PARAMS_SIZE];
    bytes[0..4].copy_from_slice(&scale.to_ne_bytes());
    bytes[8..12].copy_from_slice(&(has_direct as u32).to_ne_bytes());
    bytes
}

#[derive(Clone, Copy)]
pub struct ShGridInfoParams {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub atlas_dimensions: [u32; 2],
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub atlas_tiles_per_row: u32,
    pub tiles_per_layer: u32,
    pub atlas_layer_count: u32,
    pub present: bool,
    pub probe_occlusion_enabled: bool,
}

pub fn build_grid_info_bytes(params: ShGridInfoParams) -> [u8; SH_GRID_INFO_SIZE] {
    let mut bytes = [0u8; SH_GRID_INFO_SIZE];
    // grid_origin vec3 at 0..12, has_sh_volume u32 at 12..16.
    bytes[0..4].copy_from_slice(&params.grid_origin[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&params.grid_origin[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&params.grid_origin[2].to_ne_bytes());
    bytes[12..16].copy_from_slice(&(params.present as u32).to_ne_bytes());
    // cell_size vec3 at 16..28, _pad0 at 28..32.
    bytes[16..20].copy_from_slice(&params.cell_size[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&params.cell_size[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&params.cell_size[2].to_ne_bytes());
    // grid_dimensions vec3<u32> at 32..44, _pad1 at 44..48.
    bytes[32..36].copy_from_slice(&params.grid_dimensions[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&params.grid_dimensions[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&params.grid_dimensions[2].to_ne_bytes());
    // bytes[44..48] is _pad1, already zero.
    bytes[48..52].copy_from_slice(&params.atlas_dimensions[0].to_ne_bytes());
    bytes[52..56].copy_from_slice(&params.atlas_dimensions[1].to_ne_bytes());
    bytes[56..60].copy_from_slice(&params.tile_dimension.to_ne_bytes());
    bytes[60..64].copy_from_slice(&params.tile_border.to_ne_bytes());
    bytes[64..68].copy_from_slice(&params.atlas_tiles_per_row.to_ne_bytes());
    // atlas_tile_rows (68..72) is vestigial: no shader reads it — tile placement
    // derives from `atlas_tiles_per_row` + `tile_dimension`. It occupies the slot
    // the old `tile_grid_dimensions.y` used, so the word is retained to preserve
    // the uniform's byte offsets. Written as 0 so a reader can't mistake it for a
    // load-bearing value (the offset/size stay unchanged either way).
    bytes[68..72].copy_from_slice(&0u32.to_ne_bytes());
    let interior = params
        .tile_dimension
        .saturating_sub(params.tile_border.saturating_mul(2));
    bytes[72..76].copy_from_slice(&interior.to_ne_bytes());
    bytes[80..84].copy_from_slice(&(params.probe_occlusion_enabled as u32).to_ne_bytes());
    bytes[84..88].copy_from_slice(&params.tiles_per_layer.to_ne_bytes());
    bytes[88..92].copy_from_slice(&params.atlas_layer_count.to_ne_bytes());
    bytes
}

pub fn build_animation_buffers(
    section: Option<&OctahedralShVolumeSection>,
) -> (Vec<u8>, Vec<u8>, u32) {
    let Some(sec) = section else {
        return (dummy_descriptor_buffer(), dummy_storage_buffer(), 0);
    };
    let animated_light_count = sec.animation_descriptors.len();
    if animated_light_count == 0 {
        return (dummy_descriptor_buffer(), dummy_storage_buffer(), 0);
    }

    let mut samples: Vec<f32> = Vec::new();
    let mut descriptors = Vec::with_capacity(animated_light_count * ANIMATION_DESCRIPTOR_SIZE);

    for desc in &sec.animation_descriptors {
        let brightness_offset = samples.len() as u32;
        let brightness_count = desc.brightness.len() as u32;
        samples.extend_from_slice(&desc.brightness);

        let color_offset = samples.len() as u32;
        let color_count = desc.color.len() as u32;
        for rgb in &desc.color {
            samples.extend_from_slice(rgb);
        }

        let direction_offset = samples.len() as u32;
        let direction_count = desc.direction.len() as u32;
        for dir in &desc.direction {
            debug_assert!(
                (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2] - 1.0).abs() < 1.0e-4,
            );
            samples.extend_from_slice(dir);
        }

        write_descriptor_bytes(
            &mut descriptors,
            desc,
            brightness_offset,
            brightness_count,
            color_offset,
            color_count,
            direction_offset,
            direction_count,
        );
    }

    let sample_bytes = if samples.is_empty() {
        // Regression: a script-reserved animated light can have a descriptor
        // without any authored FGD curve samples. wgpu rejects a zero-sized
        // storage-buffer binding before the runtime script can upload its curve.
        dummy_storage_buffer()
    } else {
        f32_slice_to_bytes(&samples)
    };

    (descriptors, sample_bytes, animated_light_count as u32)
}

#[allow(clippy::too_many_arguments)]
fn write_descriptor_bytes(
    out: &mut Vec<u8>,
    desc: &AnimationDescriptor,
    brightness_offset: u32,
    brightness_count: u32,
    color_offset: u32,
    color_count: u32,
    direction_offset: u32,
    direction_count: u32,
) {
    let start = out.len();
    out.resize(start + ANIMATION_DESCRIPTOR_SIZE, 0);
    let s = &mut out[start..start + ANIMATION_DESCRIPTOR_SIZE];
    s[0..4].copy_from_slice(&desc.period.to_ne_bytes());
    s[4..8].copy_from_slice(&desc.phase.to_ne_bytes());
    s[8..12].copy_from_slice(&brightness_offset.to_ne_bytes());
    s[12..16].copy_from_slice(&brightness_count.to_ne_bytes());
    s[16..20].copy_from_slice(&desc.base_color[0].to_ne_bytes());
    s[20..24].copy_from_slice(&desc.base_color[1].to_ne_bytes());
    s[24..28].copy_from_slice(&desc.base_color[2].to_ne_bytes());
    s[28..32].copy_from_slice(&color_offset.to_ne_bytes());
    s[32..36].copy_from_slice(&color_count.to_ne_bytes());
    s[36..40].copy_from_slice(&desc.start_active.to_ne_bytes());
    s[40..44].copy_from_slice(&direction_offset.to_ne_bytes());
    s[44..48].copy_from_slice(&direction_count.to_ne_bytes());
}

fn f32_slice_to_bytes(data: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for &v in data {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    bytes
}

fn dummy_storage_buffer() -> Vec<u8> {
    vec![0u8; ANIMATION_DESCRIPTOR_SIZE]
}

fn dummy_descriptor_buffer() -> Vec<u8> {
    vec![0u8; ANIMATION_DESCRIPTOR_SIZE]
}

pub fn probe_occlusion_seed_from_fast_env(value: Option<&str>) -> bool {
    value.map_or(DEFAULT_PROBE_OCCLUSION, |v| v != "1")
}

pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp32 = ((bits >> 23) & 0xff) as i32;
    let mant32 = bits & 0x7fffff;

    if exp32 == 0xff {
        let mant16 = if mant32 != 0 { 0x200 } else { 0 };
        return (sign << 15) | (0x1f << 10) | mant16;
    }

    let exp16 = exp32 - 127 + 15;
    if exp16 >= 0x1f {
        return (sign << 15) | (0x1f << 10);
    }
    if exp16 <= 0 {
        if exp16 < -10 {
            return sign << 15;
        }
        let mant = mant32 | 0x800000;
        let shift = 14 - exp16;
        let mut rounded = (mant >> shift) as u16;
        let round_bit = 1u32 << (shift - 1);
        let remainder = mant & (round_bit - 1);
        let halfway = (mant & round_bit) != 0;
        if halfway && (remainder != 0 || (rounded & 1) != 0) {
            rounded = rounded.wrapping_add(1);
        }
        return (sign << 15) | rounded;
    }

    let mut mant16 = (mant32 >> 13) as u16;
    let round_bits = mant32 & 0x1fff;
    if round_bits > 0x1000 || (round_bits == 0x1000 && (mant16 & 1) != 0) {
        mant16 += 1;
        if mant16 == 0x400 {
            mant16 = 0;
            return (sign << 15) | (((exp16 + 1) as u16) << 10) | mant16;
        }
    }
    (sign << 15) | ((exp16 as u16) << 10) | mant16
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::sh_volume::OctahedralShProbe;

    fn test_octahedral_section(
        grid: [u32; 3],
        animation_descriptors: Vec<AnimationDescriptor>,
    ) -> OctahedralShVolumeSection {
        let probe_count = grid[0] as usize * grid[1] as usize * grid[2] as usize;
        let atlas_dimensions =
            postretro_level_format::octahedral::irradiance_atlas_dimensions(grid, 6);
        let atlas_tiles_per_row =
            postretro_level_format::octahedral::irradiance_atlas_tiles_per_row(grid).unwrap();
        let tiles_per_layer = (atlas_dimensions[0] / 6).saturating_mul(atlas_dimensions[1] / 6);
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: grid,
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: 6,
            tile_border: 1,
            atlas_dimensions,
            layer_count: 1,
            tiles_per_layer,
            atlas_tiles_per_row,
            probes: vec![OctahedralShProbe::default(); probe_count],
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
            compact_atlas: Vec::new(),
            animation_descriptors,
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn f32_to_f16_bits_known_values() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(-1.0), 0xbc00);
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(f32_to_f16_bits(2.0), 0x4000);
        assert_eq!(f32_to_f16_bits(-0.5), 0xb800);
    }

    #[test]
    fn grid_info_bytes_encode_origin_and_present_flag() {
        let bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: [1.5, 2.5, 3.5],
            cell_size: [0.25, 0.5, 1.0],
            grid_dimensions: [4, 5, 6],
            atlas_dimensions: [66, 66],
            tile_dimension: 6,
            tile_border: 1,
            atlas_tiles_per_row: 11,
            tiles_per_layer: 121,
            atlas_layer_count: 3,
            present: true,
            probe_occlusion_enabled: true,
        });
        assert_eq!(bytes.len(), SH_GRID_INFO_SIZE);

        assert_eq!(f32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 1.5);
        assert_eq!(f32::from_ne_bytes(bytes[4..8].try_into().unwrap()), 2.5);
        assert_eq!(f32::from_ne_bytes(bytes[8..12].try_into().unwrap()), 3.5);
        assert_eq!(u32::from_ne_bytes(bytes[12..16].try_into().unwrap()), 1);
        assert_eq!(f32::from_ne_bytes(bytes[16..20].try_into().unwrap()), 0.25);
        assert_eq!(u32::from_ne_bytes(bytes[36..40].try_into().unwrap()), 5);
        assert_eq!(u32::from_ne_bytes(bytes[48..52].try_into().unwrap()), 66);
        assert_eq!(u32::from_ne_bytes(bytes[56..60].try_into().unwrap()), 6);
        assert_eq!(u32::from_ne_bytes(bytes[64..68].try_into().unwrap()), 11);
        assert_eq!(u32::from_ne_bytes(bytes[68..72].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(bytes[72..76].try_into().unwrap()), 4);
        assert_eq!(u32::from_ne_bytes(bytes[80..84].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(bytes[84..88].try_into().unwrap()), 121);
        assert_eq!(u32::from_ne_bytes(bytes[88..92].try_into().unwrap()), 3);
    }

    #[test]
    fn grid_info_flag_zero_when_absent() {
        let bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [1, 1, 1],
            atlas_dimensions: [1, 1],
            tile_dimension: 1,
            tile_border: 0,
            atlas_tiles_per_row: 1,
            tiles_per_layer: 1,
            atlas_layer_count: 1,
            present: false,
            probe_occlusion_enabled: true,
        });
        assert_eq!(u32::from_ne_bytes(bytes[12..16].try_into().unwrap()), 0);
    }

    #[test]
    fn probe_occlusion_seed_defaults_on_and_fast_env_disables() {
        assert!(probe_occlusion_seed_from_fast_env(None));
        assert!(!probe_occlusion_seed_from_fast_env(Some("1")));
        assert!(probe_occlusion_seed_from_fast_env(Some("0")));
        assert!(probe_occlusion_seed_from_fast_env(Some("true")));
    }

    #[test]
    fn grid_info_bytes_encode_probe_occlusion_flag() {
        for (enabled, expected) in [(true, 1u32), (false, 0u32)] {
            let bytes = build_grid_info_bytes(ShGridInfoParams {
                grid_origin: [0.0; 3],
                cell_size: [1.0; 3],
                grid_dimensions: [1, 1, 1],
                atlas_dimensions: [1, 1],
                tile_dimension: 1,
                tile_border: 0,
                atlas_tiles_per_row: 1,
                tiles_per_layer: 1,
                atlas_layer_count: 1,
                present: true,
                probe_occlusion_enabled: enabled,
            });
            assert_eq!(
                u32::from_ne_bytes(bytes[80..84].try_into().unwrap()),
                expected,
            );
            assert_eq!(u32::from_ne_bytes(bytes[84..88].try_into().unwrap()), 1);
            assert_eq!(u32::from_ne_bytes(bytes[88..92].try_into().unwrap()), 1);
            assert!(bytes[92..96].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn dynamic_direct_params_pack_layout() {
        let bytes = build_dynamic_direct_params_bytes(0.5, true);
        assert_eq!(bytes.len(), DYNAMIC_DIRECT_PARAMS_SIZE);
        assert_eq!(f32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 0.5);
        assert!(bytes[4..8].iter().all(|&b| b == 0));
        assert_eq!(u32::from_ne_bytes(bytes[8..12].try_into().unwrap()), 1);
        assert!(bytes[12..16].iter().all(|&b| b == 0));

        let absent = build_dynamic_direct_params_bytes(1.0, false);
        assert_eq!(u32::from_ne_bytes(absent[8..12].try_into().unwrap()), 0);
    }

    #[test]
    fn scripted_descriptor_buffer_sizing_matches_bridge_payload_size() {
        for map_light_count in [0usize, 1, 4, 17, 256] {
            let alloc_slots = map_light_count.max(1);
            let alloc_bytes = alloc_slots * ANIMATION_DESCRIPTOR_SIZE;
            let expected_upload_bytes = map_light_count * ANIMATION_DESCRIPTOR_SIZE;
            if map_light_count == 0 {
                assert_eq!(alloc_bytes, ANIMATION_DESCRIPTOR_SIZE);
                assert_eq!(expected_upload_bytes, 0);
            } else {
                assert_eq!(alloc_bytes, expected_upload_bytes);
                assert_eq!(alloc_bytes % ANIMATION_DESCRIPTOR_SIZE, 0);
            }
        }
    }

    #[test]
    fn build_animation_buffers_no_section_produces_dummies() {
        let (descriptors, samples, count) = build_animation_buffers(None);
        assert_eq!(count, 0);
        assert!(!descriptors.is_empty());
        assert!(!samples.is_empty());
    }

    // Regression: script-only animated membership produced an empty GPU sample
    // buffer and wgpu rejected the SH bind group before levelLoad could run.
    #[test]
    fn build_animation_buffers_script_only_descriptor_produces_sample_dummy() {
        let section = test_octahedral_section(
            [1, 1, 1],
            vec![AnimationDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: [1.0; 3],
                brightness: Vec::new(),
                color: Vec::new(),
                direction: Vec::new(),
                start_active: 1,
            }],
        );

        let (descriptors, samples, count) = build_animation_buffers(Some(&section));

        assert_eq!(count, 1);
        assert_eq!(descriptors.len(), ANIMATION_DESCRIPTOR_SIZE);
        assert_eq!(samples.len(), ANIMATION_DESCRIPTOR_SIZE);
        assert!(samples.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn build_animation_buffers_packs_descriptors_and_samples() {
        let section = test_octahedral_section(
            [2, 1, 1],
            vec![
                AnimationDescriptor {
                    period: 2.0,
                    phase: 0.25,
                    base_color: [1.0, 0.5, 0.25],
                    brightness: vec![0.0, 1.0, 0.5, 1.0],
                    color: vec![],
                    direction: vec![],
                    start_active: 1,
                },
                AnimationDescriptor {
                    period: 1.0,
                    phase: 0.0,
                    base_color: [0.1, 0.2, 0.3],
                    brightness: vec![],
                    color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    direction: vec![],
                    start_active: 0,
                },
            ],
        );

        let (descriptors, samples, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 2);
        assert_eq!(descriptors.len(), 2 * ANIMATION_DESCRIPTOR_SIZE);

        assert_eq!(
            f32::from_ne_bytes(descriptors[0..4].try_into().unwrap()),
            2.0
        );
        assert_eq!(
            f32::from_ne_bytes(descriptors[4..8].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[8..12].try_into().unwrap()),
            0
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[12..16].try_into().unwrap()),
            4
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[32..36].try_into().unwrap()),
            0
        );

        let brightness_offset_1 =
            u32::from_ne_bytes(descriptors[48 + 8..48 + 12].try_into().unwrap());
        let brightness_count_1 =
            u32::from_ne_bytes(descriptors[48 + 12..48 + 16].try_into().unwrap());
        let color_offset_1 = u32::from_ne_bytes(descriptors[48 + 28..48 + 32].try_into().unwrap());
        let color_count_1 = u32::from_ne_bytes(descriptors[48 + 32..48 + 36].try_into().unwrap());
        assert_eq!(brightness_offset_1, 4);
        assert_eq!(brightness_count_1, 0);
        assert_eq!(color_offset_1, 4);
        assert_eq!(color_count_1, 2);
        assert_eq!(samples.len(), (4 + 6) * 4);
    }

    #[test]
    fn descriptor_round_trip_pack_unpack_symmetric() {
        let desc = AnimationDescriptor {
            period: 3.75,
            phase: 0.625,
            base_color: [0.9, 0.5, 0.125],
            brightness: vec![0.25, 0.5, 1.0],
            color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.5]],
            direction: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            start_active: 0,
        };
        let section = test_octahedral_section([1, 1, 1], vec![desc.clone()]);

        let (descriptors, _samples, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 1);
        assert_eq!(descriptors.len(), ANIMATION_DESCRIPTOR_SIZE);

        assert_eq!(
            f32::from_ne_bytes(descriptors[0..4].try_into().unwrap()),
            desc.period
        );
        assert_eq!(
            f32::from_ne_bytes(descriptors[4..8].try_into().unwrap()),
            desc.phase
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[8..12].try_into().unwrap()),
            0
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[12..16].try_into().unwrap()),
            desc.brightness.len() as u32,
        );
        assert_eq!(
            f32::from_ne_bytes(descriptors[16..20].try_into().unwrap()),
            desc.base_color[0],
        );
        assert_eq!(
            f32::from_ne_bytes(descriptors[20..24].try_into().unwrap()),
            desc.base_color[1],
        );
        assert_eq!(
            f32::from_ne_bytes(descriptors[24..28].try_into().unwrap()),
            desc.base_color[2],
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[28..32].try_into().unwrap()),
            desc.brightness.len() as u32,
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[32..36].try_into().unwrap()),
            desc.color.len() as u32,
        );
        assert_eq!(
            u32::from_ne_bytes(
                descriptors
                    [ANIMATION_DESCRIPTOR_ACTIVE_OFFSET..ANIMATION_DESCRIPTOR_ACTIVE_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
            desc.start_active,
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[40..44].try_into().unwrap()),
            (desc.brightness.len() + desc.color.len() * 3) as u32,
        );
        assert_eq!(
            u32::from_ne_bytes(descriptors[44..48].try_into().unwrap()),
            desc.direction.len() as u32,
        );
    }

    #[test]
    fn direction_channel_packs_after_brightness_and_color_samples() {
        let dir0 = [0.0f32, 1.0, 0.0];
        let dir1 = [0.0f32, 0.0, 1.0];
        let dir2 = [0.0f32, -1.0, 0.0];
        let dir3 = [0.0f32, 0.0, -1.0];

        let section = test_octahedral_section(
            [1, 1, 1],
            vec![AnimationDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: [1.0, 1.0, 1.0],
                brightness: vec![1.0, 0.5],
                color: vec![[1.0, 0.0, 0.0]],
                direction: vec![dir0, dir1, dir2, dir3],
                start_active: 1,
            }],
        );

        let (descriptors, samples_bytes, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 1);

        let direction_offset = u32::from_ne_bytes(descriptors[40..44].try_into().unwrap()) as usize;
        let direction_count = u32::from_ne_bytes(descriptors[44..48].try_into().unwrap()) as usize;
        assert_eq!(direction_offset, 5);
        assert_eq!(direction_count, 4);

        let samples: Vec<f32> = samples_bytes
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        for (i, sample) in [dir0, dir1, dir2, dir3].iter().enumerate() {
            let base = direction_offset + i * 3;
            assert_eq!(samples[base], sample[0]);
            assert_eq!(samples[base + 1], sample[1]);
            assert_eq!(samples[base + 2], sample[2]);
        }
    }
}
