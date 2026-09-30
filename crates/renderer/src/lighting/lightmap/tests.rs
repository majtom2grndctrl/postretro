// Unit tests for the lightmap binding layouts, the all-resident pool plan,
// the pool texture shapes and the animated block table.
// See: context/lib/testing_guide.md

use super::bindings::{
    BIND_ANIMATED_ATLAS, BIND_ANIMATED_BLOCK_TABLE, BIND_ANIMATED_DIRECTION, BIND_DIRECTION,
    BIND_FILTERING_SAMPLER, BIND_IRRADIANCE, BIND_SAMPLER, BIND_SHADOWMASK_ATLAS,
};
use super::pool::{
    direction_pool_descriptor, irradiance_pool_descriptor, shadowmask_pool_descriptor,
};
use super::test_fixtures::{BlockFixture, block_fixture, zero_texels};
use super::*;

use log::Level;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection,
};
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_BLOCK_CAP, ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK, ANIMATED_BLOCK_TABLE_HEADER_BYTES,
    ANIMATED_BLOCK_TABLE_UNIFORM_BYTES,
};
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_render_cpu::lightmap_pool::{BLOCK_FLAG_NONE, BLOCK_FLAG_RESIDENT};

fn capture_logs(f: impl FnOnce()) -> Vec<(Level, String)> {
    let capture = postretro_test_log_capture::LogCapture::start();
    f();
    capture
        .records()
        .into_iter()
        .map(|record| (record.level, record.message))
        .collect()
}

fn fixture(extents: &[(u32, u32)]) -> BlockFixture {
    block_fixture(extents, 2, &zero_texels())
}

fn plan_for(fixture: &BlockFixture, max_dimension: u32, max_layers: u32) -> StaticPool {
    plan_static_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        &fixture.payloads,
        max_dimension,
        max_layers,
    )
}

fn blocks(pool: StaticPool) -> StaticPoolPlan {
    match pool {
        StaticPool::Blocks(plan) => plan,
        other => panic!("expected an installed pool, got {other:?}"),
    }
}

fn renderer_errors(logs: &[(Level, String)]) -> Vec<&str> {
    logs.iter()
        .filter(|(level, message)| *level == Level::Error && message.starts_with("[Renderer]"))
        .map(|(_, message)| message.as_str())
        .collect()
}

#[test]
fn absent_or_blockless_lightmap_plans_placeholder_mode() {
    assert_eq!(
        plan_static_pool(None, None, &[], 8192, 256),
        StaticPool::Absent
    );
    let empty = fixture(&[]);
    assert_eq!(plan_for(&empty, 8192, 256), StaticPool::Empty);
    assert_eq!(StaticPool::Empty.static_block_extents(), None);
    assert_eq!(StaticPool::Absent.static_block_extents(), None);
}

#[test]
fn every_block_places_in_block_id_order_with_its_shadowmask() {
    let extents = [(8, 8), (12, 4), (4, 16)];
    let fixture = fixture(&extents);
    let pool = plan_for(&fixture, 8192, 256);
    assert_eq!(pool.static_block_extents(), Some(&extents[..]));
    let plan = blocks(pool);
    assert_eq!(plan.pool.layer_count, 1);
    assert_eq!(plan.pool.placements.len(), 3);
    assert!(plan.with_shadowmask);
    for placement in &plan.pool.placements {
        assert_eq!(
            (placement.x % 4, placement.y % 4),
            (0, 0),
            "BC- and direction-aligned origins"
        );
    }
}

#[test]
fn payloads_that_disagree_with_the_index_degrade_with_a_renderer_error() {
    let fixture = fixture(&[(8, 8), (8, 4)]);
    let logs = capture_logs(|| {
        let short = plan_static_pool(
            Some(&fixture.index),
            None,
            &fixture.payloads[..1],
            8192,
            256,
        );
        assert_eq!(short, StaticPool::Rejected, "one payload for two blocks");
        let mut wrong = fixture.payloads.clone();
        wrong[1].irradiance.pop();
        assert_eq!(
            plan_static_pool(Some(&fixture.index), None, &wrong, 8192, 256),
            StaticPool::Rejected,
            "a short irradiance blob"
        );
    });
    let errors = renderer_errors(&logs);
    assert_eq!(errors.len(), 2, "{logs:?}");
    assert!(
        errors.iter().all(|e| e.contains("neutral placeholder")),
        "{errors:?}"
    );
}

fn streaming_plan_for(fixture: &BlockFixture, max_dimension: u32, max_layers: u32) -> StaticPool {
    plan_streaming_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        [3; 32],
        15,
        max_dimension,
        max_layers,
    )
}

// A streamed level needs no payloads at install: its plan keeps the extents
// the animated atlas keys on, the level's identity, and a cap bounded so the
// pool plus its spare layer fit the device.
#[test]
fn a_streamed_level_plans_its_pool_shape_without_payloads() {
    assert_eq!(
        plan_streaming_pool(None, None, [0; 32], 15, 8192, 256),
        StaticPool::Absent
    );
    assert_eq!(
        streaming_plan_for(&fixture(&[]), 8192, 256),
        StaticPool::Empty
    );

    let extents = [(8, 8), (12, 4)];
    let fixture = fixture(&extents);
    let pool = streaming_plan_for(&fixture, 8192, 256);
    assert_eq!(pool.static_block_extents(), Some(&extents[..]));
    let StaticPool::Streaming(plan) = pool else {
        panic!("expected a streamed pool, got {pool:?}");
    };
    assert_eq!(plan.content_tag, [3; 32]);
    assert!(plan.with_shadowmask);
    assert_eq!((plan.pool_cap_layers, plan.max_array_layers), (15, 256));

    let StaticPool::Streaming(tight) = streaming_plan_for(&fixture, 8192, 8) else {
        panic!("eight array layers still hold a pool");
    };
    assert_eq!(
        tight.pool_cap_layers, 7,
        "the cap leaves room for the spare layer"
    );
}

#[test]
fn a_device_that_cannot_hold_a_streamed_pool_or_its_shadowmask_degrades_with_an_error() {
    let fixture = fixture(&[(8, 8)]);
    let logs = capture_logs(|| {
        assert_eq!(
            streaming_plan_for(&fixture, 1024, 256),
            StaticPool::Rejected
        );
        assert_eq!(streaming_plan_for(&fixture, 8192, 1), StaticPool::Rejected);
        let StaticPool::Streaming(plan) = streaming_plan_for(&fixture, 2048, 256) else {
            panic!("a 2048² layer fits; only the two-group shadowmask does not");
        };
        assert!(!plan.with_shadowmask);
    });
    let errors = renderer_errors(&logs);
    assert_eq!(errors.len(), 3, "{logs:?}");
    assert!(
        errors[0].contains("maxTextureDimension2D 1024"),
        "{errors:?}"
    );
    assert!(errors[1].contains("maxTextureArrayLayers 1"), "{errors:?}");
    assert!(errors[2].contains("ShadowmaskAtlas rejected"), "{errors:?}");
}

// The streamed pool holds the all-resident ceiling the same way, spare layer
// included.
#[test]
fn a_streamed_level_too_big_to_hold_at_once_still_streams_and_one_layer_short_degrades() {
    // Five 1024² blocks need two 2048² layers, three with the spare. A device
    // with two layers can still stream a subset; the pool model defers growth
    // past it. A device with one layer cannot hold a layer plus the spare.
    let fixture = fixture(&[(1024, 1024); 5]);
    assert!(matches!(
        streaming_plan_for(&fixture, 8192, 3),
        StaticPool::Streaming(_)
    ));
    assert!(matches!(
        streaming_plan_for(&fixture, 8192, 2),
        StaticPool::Streaming(_)
    ));
    let logs = capture_logs(|| {
        assert_eq!(streaming_plan_for(&fixture, 8192, 1), StaticPool::Rejected);
    });
    let errors = renderer_errors(&logs);
    assert_eq!(errors.len(), 1, "{logs:?}");
    assert!(errors[0].contains("needs 2 array layer(s)"), "{errors:?}");
}

#[test]
fn a_device_below_the_pool_layer_or_its_layer_count_degrades_to_placeholders() {
    // Five 1024² blocks need two 2048² layers.
    let fixture = fixture(&[(1024, 1024); 5]);
    assert_eq!(blocks(plan_for(&fixture, 8192, 256)).pool.layer_count, 2);
    let logs = capture_logs(|| {
        assert_eq!(plan_for(&fixture, 8192, 1), StaticPool::Rejected);
        assert_eq!(plan_for(&fixture, 1024, 256), StaticPool::Rejected);
    });
    let errors = renderer_errors(&logs);
    assert_eq!(errors.len(), 2, "{logs:?}");
    assert!(errors[0].contains("maxTextureArrayLayers 1"), "{errors:?}");
    assert!(
        errors[1].contains("maxTextureDimension2D 1024"),
        "{errors:?}"
    );
}

#[test]
fn unusable_shadowmask_groups_keep_the_lightmap_and_drop_only_the_shadowmask() {
    let mut fixture = fixture(&[(8, 8), (8, 8)]);
    let logs = capture_logs(|| {
        // Two 2048-wide groups need a 4096-wide texture.
        let narrow = blocks(plan_for(&fixture, 2048, 256));
        assert!(!narrow.with_shadowmask);
        fixture.payloads[1].shadowmask = None;
        assert!(!blocks(plan_for(&fixture, 8192, 256)).with_shadowmask);
    });
    let errors = renderer_errors(&logs);
    assert_eq!(errors.len(), 2, "{logs:?}");
    assert!(
        errors
            .iter()
            .all(|e| e.contains("ShadowmaskAtlas rejected"))
    );
    assert!(
        errors[1].contains("block 1 arrived without its groups"),
        "{errors:?}"
    );

    // No id 42: no shadowmask, and nothing to log.
    let without = block_fixture(
        &[(8, 8)],
        2,
        &super::test_fixtures::FixtureTexels {
            shadowmask: None,
            ..zero_texels()
        },
    );
    let logs = capture_logs(|| assert!(!blocks(plan_for(&without, 8192, 256)).with_shadowmask));
    assert!(renderer_errors(&logs).is_empty(), "{logs:?}");
}

#[test]
fn pool_textures_are_edge_square_layers_in_stored_formats_and_copyable() {
    let plan = blocks(plan_for(&fixture(&[(1024, 1024); 5]), 8192, 256));
    let edge = LIGHTMAP_POOL_LAYER_EDGE;
    let irradiance = irradiance_pool_descriptor(&plan);
    let direction = direction_pool_descriptor(&plan);
    let shadowmask = shadowmask_pool_descriptor(&plan);
    assert_eq!(
        (
            irradiance.size.width,
            irradiance.size.height,
            irradiance.size.depth_or_array_layers
        ),
        (edge, edge, 2)
    );
    assert_eq!(irradiance.format, wgpu::TextureFormat::Rgba16Float);
    assert_eq!(
        (
            direction.size.width,
            direction.size.height,
            direction.size.depth_or_array_layers
        ),
        (edge / 2, edge / 2, 2),
        "direction sits at edge / scale"
    );
    assert_eq!(direction.format, wgpu::TextureFormat::Rg8Unorm);
    assert_eq!(
        (
            shadowmask.size.width,
            shadowmask.size.height,
            shadowmask.size.depth_or_array_layers
        ),
        (2 * edge, edge, 2),
        "two mask groups side by side"
    );
    assert_eq!(shadowmask.format, wgpu::TextureFormat::Bc5RgUnorm);
    for descriptor in [irradiance, direction, shadowmask] {
        assert!(descriptor.usage.contains(
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
        ));
        assert_eq!(descriptor.mip_level_count, 1, "the shader samples level 0");
    }
}

#[test]
fn block_table_layout_is_one_vertex_only_read_only_storage_buffer() {
    let [entry] = block_table_bind_group_layout_entries();
    assert_eq!(entry.binding, 0);
    assert_eq!(entry.visibility, wgpu::ShaderStages::VERTEX);
    assert!(matches!(
        entry.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            ..
        }
    ));
}

/// The WGSL helpers' pool edge and flag bits are the Rust contract's values.
#[test]
fn lightmap_sample_wgsl_constants_match_the_pool_contract() {
    let wgsl = include_str!("../../shaders/lightmap_sample.wgsl");
    let constant = |name: &str| -> String {
        wgsl.lines()
            .find_map(|line| line.strip_prefix(&format!("const {name}: ")))
            .and_then(|rest| rest.split_once('=').map(|(_, v)| v.trim().to_owned()))
            .unwrap_or_else(|| panic!("lightmap_sample.wgsl must declare {name}"))
    };
    assert_eq!(
        constant("LIGHTMAP_POOL_LAYER_EDGE"),
        format!("{}.0;", LIGHTMAP_POOL_LAYER_EDGE)
    );
    assert_eq!(
        constant("LIGHTMAP_BLOCK_RESIDENT"),
        format!("{BLOCK_FLAG_RESIDENT}u;")
    );
    assert_eq!(
        constant("LIGHTMAP_BLOCK_NONE"),
        format!("{BLOCK_FLAG_NONE}u;")
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
fn animated_block_table_packs_compact_minus_block_local_offsets() {
    let block = |block_x, block_y, compact_x, compact_y, compact_layer| AnimatedBlock {
        lightmap_block: 7,
        block_x,
        block_y,
        compact_x,
        compact_y,
        compact_layer,
        width: 4,
        height: 4,
    };
    let section = AnimatedLightWeightMapsSection {
        page_size: 1024,
        compact_layers: 2,
        blocks: vec![block(2000, 10, 0, 1000, 0), block(5, 1700, 900, 20, 1)],
        ..AnimatedLightWeightMapsSection::empty()
    };
    let bytes = animated_block_table_bytes(Some(&section));
    assert_eq!(bytes.len(), ANIMATED_BLOCK_TABLE_BYTES);
    let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(
        (word(0), word(4), word(8)),
        (0, 1024, 2),
        "the retired static-layer-size word is reserved zero"
    );
    assert_eq!(decode_block(&bytes, 0), (-2000, 990, 0));
    assert_eq!(decode_block(&bytes, 1), (895, -1680, 1));
}

#[test]
fn empty_block_table_names_no_block() {
    let bytes = animated_block_table_bytes(None);
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
