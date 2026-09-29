// Unit tests for the lightmap binding layout, usability filters and uploads.
// See: context/lib/testing_guide.md

use super::upload::{direction_texture_format, shadowmask_texture_descriptor};
use super::*;

use log::Level;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_BLOCK_CAP, ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK, ANIMATED_BLOCK_TABLE_HEADER_BYTES,
    ANIMATED_BLOCK_TABLE_UNIFORM_BYTES,
};
use postretro_level_format::lightmap::{
    DIRECTION_FORMAT_OCT_RG8, DIRECTION_FORMAT_OCT_RGBA8, LightmapMode,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

fn capture_logs(f: impl FnOnce()) -> Vec<(Level, String)> {
    let capture = postretro_test_log_capture::LogCapture::start();
    f();
    capture
        .records()
        .into_iter()
        .map(|record| (record.level, record.message))
        .collect()
}

fn fake_section(width: u32, height: u32) -> LightmapHeader {
    fake_section_layers(width, height, 1)
}

fn fake_section_layers(width: u32, height: u32, layer_count: u32) -> LightmapHeader {
    LightmapHeader {
        layer_count,
        irr_width: width,
        irr_height: height,
        irr_texel_density: 0.04,
        irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F,
        dir_width: width,
        dir_height: height,
        dir_texel_density: 0.04,
        direction_format: postretro_level_format::lightmap::DIRECTION_FORMAT_OCT_RG8,
        mode: LightmapMode::Shadowed,
    }
}

fn fake_shadowmask_section(width: u32, height: u32, layer_count: u32) -> ShadowmaskAtlasHeader {
    ShadowmaskAtlasHeader {
        format: postretro_level_format::shadowmask_atlas::SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
        width,
        height,
        layer_count,
        channels: vec![0],
    }
}

/// Atlas-fits-device guard: a section that exceeds the granted
/// `max_texture_dimension_2d` is dropped so the caller falls through to the
/// neutral placeholder, rather than panicking on texture creation. Pure
/// dimension comparison — no real oversize allocation needed.
#[test]
fn oversize_section_filtered_out() {
    let oversize = fake_section(16_384, 8192);
    assert!(
        filter_usable_section(Some(&oversize), 8192, 256).is_none(),
        "atlas wider than the granted limit must drop to placeholder",
    );

    let tall = fake_section(8192, 16_384);
    assert!(
        filter_usable_section(Some(&tall), 8192, 256).is_none(),
        "atlas taller than the granted limit must drop to placeholder",
    );
}

/// Regression: a byte-valid direction atlas wider than the device limit
/// reached `upload_direction_texture` and failed wgpu validation.
#[test]
fn oversize_direction_section_filtered_out() {
    let mut oversize_direction = fake_section(64, 64);
    oversize_direction.dir_width = 16_384;

    assert!(
        filter_usable_section(Some(&oversize_direction), 8192, 256).is_none(),
        "direction atlas wider than the granted limit must drop to placeholder",
    );
}

#[test]
fn oversize_section_logs_renderer_prefixed_error() {
    let oversize = fake_section(16_384, 8192);
    let captured = capture_logs(|| {
        assert!(filter_usable_section(Some(&oversize), 8192, 256).is_none());
    });

    assert!(
        captured.iter().any(|(level, message)| {
            *level == Level::Error
                && message.starts_with("[Renderer] Lightmap atlas 16384x8192")
                && message.contains("maxTextureDimension2D 8192")
        }),
        "oversize lightmap rejection should log one renderer-prefixed error, got {captured:?}",
    );
}

#[test]
fn at_or_under_limit_section_kept() {
    let at_limit = fake_section(8192, 8192);
    assert!(
        filter_usable_section(Some(&at_limit), 8192, 256).is_some(),
        "atlas exactly at the granted limit must be retained",
    );

    let under = fake_section(4096, 2048);
    assert!(
        filter_usable_section(Some(&under), 8192, 256).is_some(),
        "atlas under the granted limit must be retained",
    );
}

#[test]
fn zero_dimension_section_filtered_out() {
    let empty = fake_section(0, 0);
    assert!(
        filter_usable_section(Some(&empty), 8192, 256).is_none(),
        "zero-dimension section must drop to placeholder regardless of limit",
    );
}

/// Format contract (`postretro_level_format::lightmap`) requires
/// `layer_count >= 1`; `from_bytes` reads the field raw without gating. A
/// corrupt section with `layer_count == 0` must drop to the neutral
/// placeholder rather than reach `upload_irradiance_texture`, which would
/// build a zero-extent wgpu texture and panic in validation.
#[test]
fn zero_layer_count_section_filtered_out() {
    let no_layers = fake_section_layers(64, 64, 0);
    assert!(
        filter_usable_section(Some(&no_layers), 8192, 256).is_none(),
        "zero-layer section must drop to placeholder regardless of limit",
    );
}

#[test]
fn missing_section_filtered_out() {
    assert!(
        filter_usable_section(None, 8192, 256).is_none(),
        "absent section drops to placeholder",
    );
}

/// Array-layer-fits-device guard: a section whose `layer_count` exceeds the
/// granted `max_texture_array_layers` is dropped to the neutral placeholder.
/// No real adapter exposes a limit below 256, so this guards corrupt or
/// future multi-layer sections against an under-spec/clamped limit. Pure
/// comparison — no real array texture allocated.
#[test]
fn too_many_layers_section_filtered_out() {
    // 8 layers under a tiny 4-layer limit, with in-bounds dimensions so the
    // layer guard (not the dimension guard) is what rejects it.
    let many_layers = fake_section_layers(64, 64, 8);
    assert!(
        filter_usable_section(Some(&many_layers), 8192, 4).is_none(),
        "atlas with more layers than the granted limit must drop to placeholder",
    );

    // Exactly at the layer limit is retained.
    let at_limit = fake_section_layers(64, 64, 4);
    assert!(
        filter_usable_section(Some(&at_limit), 8192, 4).is_some(),
        "atlas exactly at the granted layer limit must be retained",
    );
}

#[test]
fn rejected_multilayer_shadowmask_uses_placeholder_path() {
    // Regression: a rejected two-layer atlas used to bind a one-layer
    // placeholder while the shader still sampled baked layer 1 directly.
    let oversize = fake_shadowmask_section(16_384, 64, 2);
    assert!(
        filter_usable_shadowmask_section(Some(&oversize), 8192, 256).is_none(),
        "oversize multi-layer shadowmask must use the all-visible placeholder path",
    );

    let too_many_layers = fake_shadowmask_section(64, 64, 8);
    assert!(
        filter_usable_shadowmask_section(Some(&too_many_layers), 8192, 4).is_none(),
        "over-layer-limit shadowmask must use the all-visible placeholder path",
    );
}

/// The device limit the renderer pins at acquisition.
const PINNED_TEXTURE_DIMENSION: u32 = 8192;

fn shadowmask_filter_errors(section: &ShadowmaskAtlasHeader) -> (bool, Vec<String>) {
    let mut kept = false;
    let captured = capture_logs(|| {
        kept = filter_usable_shadowmask_section(Some(section), PINNED_TEXTURE_DIMENSION, 256)
            .is_some();
    });
    let errors = captured
        .into_iter()
        .filter(|(level, message)| *level == Level::Error && message.starts_with("[Renderer]"))
        .map(|(_, message)| message)
        .collect();
    (kept, errors)
}

// Pin: width-boundary. `W` is 4-aligned, so the smallest texture over the
// limit is one block wider than it.
#[test]
fn shadowmask_texture_at_the_pinned_width_is_kept_and_one_block_wider_degrades() {
    let (kept, errors) = shadowmask_filter_errors(&fake_shadowmask_section(4096, 64, 2));
    assert!(
        kept && errors.is_empty(),
        "2W == 8192 must be kept: {errors:?}"
    );

    let (kept, errors) = shadowmask_filter_errors(&fake_shadowmask_section(4100, 64, 2));
    assert!(!kept, "2W == 8200 must degrade to the placeholder");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("texture 8200x64") && errors[0].contains("maxTextureDimension2D 8192"),
        "{errors:?}"
    );
}

// Pin: width-boundary. A `W`-only compare would keep this one.
#[test]
fn eight_k_lightmap_width_shadowmask_degrades_and_four_k_is_kept() {
    let (kept, errors) = shadowmask_filter_errors(&fake_shadowmask_section(8192, 8192, 1));
    assert!(!kept, "W == 8192 means a 16384-wide texture");
    assert!(
        errors.len() == 1 && errors[0].contains("texture 16384x8192"),
        "{errors:?}"
    );
    let (kept, _) = shadowmask_filter_errors(&fake_shadowmask_section(4096, 4096, 1));
    assert!(kept);
}

#[test]
fn hand_built_misaligned_shadowmask_degrades_with_a_renderer_error() {
    for (width, height) in [(6, 8), (8, 6)] {
        let (kept, errors) = shadowmask_filter_errors(&fake_shadowmask_section(width, height, 1));
        assert!(!kept, "{width}x{height} must reach the placeholder");
        assert!(
            errors.len() == 1 && errors[0].contains("is not BC5 block-aligned"),
            "{width}x{height}: {errors:?}"
        );
    }
}

#[test]
fn shadowmask_texture_description_is_bc5_double_width_at_half_the_raw_bytes() {
    let (width, height, layer_count) = (64, 32, 3);
    let section = fake_shadowmask_section(width, height, layer_count);
    let descriptor = shadowmask_texture_descriptor(&section);
    assert_eq!(descriptor.format, wgpu::TextureFormat::Bc5RgUnorm);
    assert_eq!(
        descriptor.size,
        wgpu::Extent3d {
            width: 2 * width,
            height,
            depth_or_array_layers: layer_count,
        }
    );
    assert_eq!(descriptor.mip_level_count, 1);
    assert_eq!(descriptor.dimension, wgpu::TextureDimension::D2);

    let texture_bytes = |format: wgpu::TextureFormat, size: wgpu::Extent3d| {
        let (block_width, block_height) = format.block_dimensions();
        let block_bytes = format.block_copy_size(None).expect("color format");
        u64::from(size.width.div_ceil(block_width))
            * u64::from(size.height.div_ceil(block_height))
            * u64::from(block_bytes)
            * u64::from(size.depth_or_array_layers)
    };
    let raw_bytes = texture_bytes(
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layer_count,
        },
    );
    let bc5_bytes = texture_bytes(descriptor.format, descriptor.size);
    assert_eq!(bc5_bytes * 2, raw_bytes);
    assert_eq!(
        bc5_bytes,
        postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection::payload_len(
            width,
            height,
            layer_count
        )
        .unwrap() as u64,
        "the upload's texture must hold exactly the section payload"
    );
}

// Pin: partial-lighting-install, plus the header-without-payload fallback.
// The upload pairs a usable header with the payload install moved in;
// nothing else uploads, and a header with no payload degrades loudly.
#[test]
fn upload_pairs_usable_headers_with_their_moved_payloads_only() {
    let header = fake_shadowmask_section(64, 64, 1);
    assert_eq!(
        paired_with_payload(Some(&header), Some(vec![7u8]), "ShadowmaskAtlas"),
        Some((&header, vec![7u8]))
    );
    assert_eq!(
        paired_with_payload::<ShadowmaskAtlasHeader, Vec<u8>>(
            None,
            Some(vec![7u8]),
            "ShadowmaskAtlas"
        ),
        None,
        "a filtered-out header's payload is dropped at the upload"
    );
    assert_eq!(
        paired_with_payload::<ShadowmaskAtlasHeader, Vec<u8>>(None, None, "ShadowmaskAtlas"),
        None,
        "a level with neither section installs placeholders"
    );
    let captured = capture_logs(|| {
        assert_eq!(
            paired_with_payload::<_, Vec<u8>>(Some(&header), None, "ShadowmaskAtlas"),
            None
        );
    });
    assert!(
        captured
            .iter()
            .any(|(level, message)| *level == Level::Error
                && message
                    .starts_with("[Renderer] ShadowmaskAtlas header arrived without its payload")),
        "a header whose payload is gone degrades loudly: {captured:?}"
    );
}

#[test]
fn hand_built_unknown_shadowmask_format_degrades_with_a_renderer_error() {
    let mut section = fake_shadowmask_section(64, 64, 1);
    section.format = 0;
    let (kept, errors) = shadowmask_filter_errors(&section);
    assert!(!kept, "an unknown tag must reach the placeholder");
    assert!(
        errors.len() == 1 && errors[0].contains("is not BC5 side-by-side"),
        "{errors:?}"
    );
}

#[test]
fn hand_built_shadowmask_payload_of_the_wrong_length_degrades_with_a_renderer_error() {
    let (width, height, layer_count) = (8, 8, 2);
    let section = fake_shadowmask_section(width, height, layer_count);
    let expected = ShadowmaskAtlasSection::payload_len(width, height, layer_count)
        .expect("fixture dimensions have a payload length");

    let captured = capture_logs(|| {
        assert!(shadowmask_payload_matches_header(
            &section,
            &vec![0u8; expected]
        ));
    });
    assert!(
        !captured.iter().any(|(level, _)| *level == Level::Error),
        "{captured:?}"
    );

    for len in [0, expected - 16, expected + 16, expected / 2] {
        let captured = capture_logs(|| {
            assert!(
                !shadowmask_payload_matches_header(&section, &vec![0u8; len]),
                "a {len}-byte payload must reach the placeholder"
            );
        });
        assert!(
            captured
                .iter()
                .any(|(level, message)| *level == Level::Error
                    && message.starts_with("[Renderer] ShadowmaskAtlas payload is")
                    && message.contains(&format!("expected Some({expected})"))),
            "{len} bytes: {captured:?}"
        );
    }
}

#[test]
fn usable_multilayer_shadowmask_keeps_real_resource() {
    let section = fake_shadowmask_section(64, 64, 2);
    assert!(
        filter_usable_shadowmask_section(Some(&section), 8192, 256).is_some(),
        "in-limit multi-layer shadowmask must keep its authored atlas",
    );
    assert!(
        filter_usable_shadowmask_section(None, 8192, 256).is_none(),
        "absent shadowmask must use the all-visible placeholder path",
    );
}

// The group-4 BGL is a fixed contract with `forward.wgsl`'s `@binding`
// decorators. This pins which textures are filterable, and the two sampler
// bindings (nearest + linear).
#[test]
fn bgl_entries_pin_sampler_split() {
    let entries = bind_group_layout_entries();
    assert_eq!(entries.len(), 8, "group-4 BGL must expose eight bindings");

    let tex_sample = |b: u32| {
        entries
            .iter()
            .find(|e| e.binding == b)
            .and_then(|e| match e.ty {
                wgpu::BindingType::Texture { sample_type, .. } => Some(sample_type),
                _ => None,
            })
    };
    let sampler_ty = |b: u32| {
        entries
            .iter()
            .find(|e| e.binding == b)
            .and_then(|e| match e.ty {
                wgpu::BindingType::Sampler(t) => Some(t),
                _ => None,
            })
    };
    let texture_view_dimension = |b: u32| {
        entries
            .iter()
            .find(|e| e.binding == b)
            .and_then(|e| match e.ty {
                wgpu::BindingType::Texture { view_dimension, .. } => Some(view_dimension),
                _ => None,
            })
    };

    // Irradiance + animated atlas filter linear (Rgba16Float is filterable).
    assert_eq!(
        tex_sample(BIND_IRRADIANCE),
        Some(wgpu::TextureSampleType::Float { filterable: true })
    );
    assert_eq!(
        tex_sample(BIND_ANIMATED_ATLAS),
        Some(wgpu::TextureSampleType::Float { filterable: true })
    );
    assert_eq!(
        tex_sample(BIND_SHADOWMASK_ATLAS),
        Some(wgpu::TextureSampleType::Float { filterable: true })
    );
    // Both direction atlases stay nearest (direction lerp ≠ slerp): both are
    // octahedral-encoded (static atlas 1, animated atlas 5), and oct vectors
    // must not be linearly interpolated.
    assert_eq!(
        tex_sample(BIND_DIRECTION),
        Some(wgpu::TextureSampleType::Float { filterable: false })
    );
    assert_eq!(
        tex_sample(BIND_ANIMATED_DIRECTION),
        Some(wgpu::TextureSampleType::Float { filterable: false })
    );
    assert_eq!(
        texture_view_dimension(BIND_ANIMATED_ATLAS),
        Some(wgpu::TextureViewDimension::D2Array),
        "animated irradiance must bind as texture_2d_array",
    );
    assert_eq!(
        texture_view_dimension(BIND_ANIMATED_DIRECTION),
        Some(wgpu::TextureViewDimension::D2Array),
        "animated direction must bind as texture_2d_array",
    );
    let block_table_entry = entries
        .iter()
        .find(|entry| entry.binding == BIND_ANIMATED_BLOCK_TABLE)
        .expect("animated block table binding must exist");
    assert_eq!(
        block_table_entry.visibility,
        wgpu::ShaderStages::FRAGMENT,
        "the block table is read in the fragment stage only",
    );
    assert!(matches!(
        block_table_entry.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            ..
        }
    ));
    // Two samplers: nearest at binding 2, linear at binding 4.
    assert_eq!(
        sampler_ty(BIND_SAMPLER),
        Some(wgpu::SamplerBindingType::NonFiltering)
    );
    assert_eq!(
        sampler_ty(BIND_FILTERING_SAMPLER),
        Some(wgpu::SamplerBindingType::Filtering)
    );
}

#[test]
fn static_direction_texture_format_follows_section_tag() {
    assert_eq!(
        direction_texture_format(DIRECTION_FORMAT_OCT_RG8),
        wgpu::TextureFormat::Rg8Unorm,
        "current static direction sections must upload exactly their RG bytes",
    );
    assert_eq!(
        direction_texture_format(DIRECTION_FORMAT_OCT_RGBA8),
        wgpu::TextureFormat::Rgba8Unorm,
        "legacy static direction sections must retain their padded upload format",
    );
}

/// CPU mirror of lightmap_sample.wgsl's `animated_block_uv` entry decode.
fn decode_block(bytes: &[u8], block: usize) -> (i32, i32, u32) {
    let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
    let at = ANIMATED_BLOCK_TABLE_HEADER_BYTES as usize
        + block * ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK as usize;
    let packed = word(at);
    (
        i32::from(packed as u16 as i16),
        i32::from((packed >> 16) as u16 as i16),
        word(at + 4),
    )
}

#[test]
fn block_table_packs_header_and_signed_offsets_per_block() {
    use postretro_level_format::animated_light_weight_maps::AnimatedBlock;
    let block = |static_x, static_y, compact_x, compact_y, compact_layer| AnimatedBlock {
        static_layer: 0,
        static_x,
        static_y,
        compact_x,
        compact_y,
        compact_layer,
        width: 4,
        height: 4,
    };
    let section = AnimatedLightWeightMapsSection {
        page_size: 1024,
        compact_layers: 2,
        blocks: vec![block(2000, 10, 0, 1000, 0), block(5, 7000, 900, 20, 1)],
        ..AnimatedLightWeightMapsSection::empty()
    };
    let bytes = animated_block_table_bytes(Some(&section), 8192);
    assert_eq!(bytes.len(), ANIMATED_BLOCK_TABLE_BYTES);
    let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!((word(0), word(4), word(8)), (8192, 1024, 2));
    assert_eq!(decode_block(&bytes, 0), (-2000, 990, 0));
    assert_eq!(decode_block(&bytes, 1), (895, -6980, 1));
}

#[test]
fn empty_block_table_names_no_block() {
    let bytes = animated_block_table_bytes(None, 2048);
    assert_eq!(bytes.len(), ANIMATED_BLOCK_TABLE_BYTES);
    assert!(
        bytes.iter().all(|&b| b == 0),
        "block_count 0 resolves every id to none"
    );
}

/// The table the renderer requests, the forward shader's declared array,
/// and the compiler's cap are one number (pin P10).
#[test]
fn block_table_capacity_matches_forward_wgsl_and_the_compiler_cap() {
    let forward = include_str!("../../shaders/forward.wgsl");
    let declared = forward
        .split("blocks: array<vec4<u32>, ")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .and_then(|len| len.trim().parse::<u32>().ok())
        .expect("forward.wgsl declares the block table array");
    let blocks_per_vec4 = 16 / ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK;
    assert_eq!(declared * blocks_per_vec4, ANIMATED_BLOCK_CAP);
    let shader_table_bytes = ANIMATED_BLOCK_TABLE_HEADER_BYTES + declared * 16;
    assert_eq!(shader_table_bytes, ANIMATED_BLOCK_TABLE_UNIFORM_BYTES);
    assert!(
        u64::from(ANIMATED_BLOCK_TABLE_UNIFORM_BYTES)
            <= wgpu::Limits::default().max_uniform_buffer_binding_size,
        "the renderer requests default limits; the table must fit them",
    );
    assert!(ANIMATED_BLOCK_CAP < u32::from(u16::MAX));
}

#[test]
fn placeholder_static_lightmap_offers_no_static_layer_to_the_animated_atlas() {
    let mut header = fake_section_layers(2048, 2048, 3);
    assert_eq!(
        usable_static_layers(Some(&header), 8192, 256),
        Some((2048, 3))
    );
    header.irr_width = 1;
    header.irr_height = 1;
    header.layer_count = 1;
    header.dir_width = 1;
    header.dir_height = 1;
    assert!(header.is_placeholder());
    assert_eq!(usable_static_layers(Some(&header), 8192, 256), None);
    assert_eq!(usable_static_layers(None, 8192, 256), None);
}
