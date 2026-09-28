// GPU coverage of the lightmap-family byte meter: the rows describe the
// textures actually bound — a real compact atlas, the placeholder that
// replaced a rejected one, and the placeholders an unload installs.
// See: context/lib/rendering_pipeline.md §7.8
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// meter reads wgpu textures. The self-skipping tests set
// `POSTRETRO_REQUIRE_GPU` like `animated_atlas_parity_test`; the whole-renderer
// test is on-demand, like the other offscreen-renderer coverage.

use postretro_level_format::animated_light_chunks::{
    AnimatedLightChunk, AnimatedLightChunksSection,
};
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE;

use super::animated_lightmap::{
    AnimatedLightmapResources, AnimatedLmDebugConfig, with_dummy_fallback,
};
use super::pipeline_layout::uniform_bind_group_layout_entries;
use super::residency::ResidencyAllocationState;
use super::sh_volume::AnimatedLightBuffers;
use super::{LIGHTMAP_ANIMATED_DIRECTION, LIGHTMAP_ANIMATED_IRRADIANCE};

fn device_or_skip(test: &str) -> Option<wgpu::Device> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let device = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()
    .and_then(|adapter| {
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lightmap_residency_test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .ok()
    })
    .map(|(device, _queue)| device);
    if device.is_none() {
        let required = std::env::var("POSTRETRO_REQUIRE_GPU")
            .is_ok_and(|value| !value.is_empty() && value != "0");
        assert!(
            !required,
            "{test}: POSTRETRO_REQUIRE_GPU is set but no GPU adapter is available"
        );
        eprintln!("[lightmap_residency_test] skipping {test}: no GPU adapter available");
    }
    device
}

/// `pages` 8×8 blocks on as many 64² pages, each with one lit 1×1 chunk.
fn one_block_per_page(pages: u32) -> (AnimatedLightWeightMapsSection, AnimatedLightChunksSection) {
    let section = AnimatedLightWeightMapsSection {
        page_size: 64,
        compact_layers: pages,
        blocks: (0..pages)
            .map(|page| AnimatedBlock {
                static_layer: 0,
                static_x: 0,
                static_y: 0,
                compact_x: 0,
                compact_y: 0,
                compact_layer: page,
                width: 8,
                height: 8,
            })
            .collect(),
        chunk_rects: (0..pages)
            .map(|page| ChunkAtlasRect {
                compact_x: 2,
                compact_y: 2,
                width: 1,
                height: 1,
                texel_offset: page,
                block: page,
            })
            .collect(),
        offset_counts: (0..pages)
            .map(|_| TexelLightEntry {
                offset: 0,
                count: 1,
            })
            .collect(),
        texel_lights: vec![TexelLight {
            light_index: 0,
            weight: 1.0,
            direction_oct: [0, 0],
        }],
    };
    let chunks = AnimatedLightChunksSection {
        chunks: (0..pages)
            .map(|_| AnimatedLightChunk {
                aabb_min: [0.0; 3],
                face_index: 0,
                aabb_max: [1.0; 3],
                index_offset: 0,
                uv_min: [0.0; 2],
                uv_max: [1.0; 2],
                index_count: 0,
                _padding: 0,
            })
            .collect(),
        light_indices: Vec::new(),
    };
    (section, chunks)
}

/// Build the animated pair the way level install does: the constructor, and
/// on failure the dummy marked as a fallback.
fn install_animated(
    device: &wgpu::Device,
    section: &AnimatedLightWeightMapsSection,
    chunks: &AnimatedLightChunksSection,
    static_layers: (u32, u32),
) -> (Result<(), String>, AnimatedLightmapResources) {
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("meter uniforms layout"),
        entries: &uniform_bind_group_layout_entries(),
    });
    let animation = AnimatedLightBuffers::for_test(device, vec![0u8; ANIMATION_DESCRIPTOR_SIZE]);
    let built = AnimatedLightmapResources::new(
        device,
        Some(section),
        Some(chunks),
        &[],
        &animation,
        &uniform_layout,
        Some(static_layers),
        AnimatedLmDebugConfig::disabled(),
    );
    let outcome = built.as_ref().map(|_| ()).map_err(Clone::clone);
    let resources = with_dummy_fallback(
        built,
        || {
            AnimatedLightmapResources::dummy(
                device,
                &animation,
                &uniform_layout,
                AnimatedLmDebugConfig::disabled(),
            )
            .into_fallback()
        },
        "animated lightmap meter test",
    );
    (outcome, resources)
}

#[test]
fn a_real_compact_atlas_meters_page_count_times_page_bytes() {
    let Some(device) = device_or_skip("a_real_compact_atlas_meters_page_count_times_page_bytes")
    else {
        return;
    };
    let (section, chunks) = one_block_per_page(3);
    let (outcome, resources) = install_animated(&device, &section, &chunks, (64, 1));
    assert_eq!(outcome, Ok(()));
    let [irradiance, direction] = resources.residency_rows();
    assert_eq!(irradiance.name, LIGHTMAP_ANIMATED_IRRADIANCE);
    assert_eq!(direction.name, LIGHTMAP_ANIMATED_DIRECTION);
    assert_eq!(irradiance.bytes, 3 * 64 * 64 * 8, "Rgba16Float pages");
    assert_eq!(direction.bytes, 3 * 64 * 64 * 4, "Rgba8Unorm pages");
    assert_eq!(irradiance.state, ResidencyAllocationState::Data);
}

/// Pin P7: an atlas rejected after its size is known (here, more pages than
/// the device's array-layer limit) meters the placeholder that replaced it.
#[test]
fn a_rejected_atlas_meters_its_placeholder_not_the_rejected_size() {
    let Some(device) =
        device_or_skip("a_rejected_atlas_meters_its_placeholder_not_the_rejected_size")
    else {
        return;
    };
    let pages = device.limits().max_texture_array_layers + 1;
    let (section, chunks) = one_block_per_page(pages);
    let (outcome, resources) = install_animated(&device, &section, &chunks, (64, 1));
    let error = outcome.expect_err("more pages than the device allows must be rejected");
    assert!(error.contains("maxTextureArrayLayers"), "{error}");
    let [irradiance, direction] = resources.residency_rows();
    assert_eq!(irradiance.bytes, 8, "1×1 Rgba16Float placeholder");
    assert_eq!(direction.bytes, 4, "1×1 Rgba8Unorm placeholder");
    assert_eq!(irradiance.state, ResidencyAllocationState::Fallback);
    assert_eq!(direction.state, ResidencyAllocationState::Fallback);
}

/// Pins P5 and P12 through the renderer's own install and unload wiring: a
/// level without animated lights meters the animated pair at placeholder
/// size, and after an unload every row is the placeholder again; the next
/// level's rows are its own.
#[test]
#[ignore = "on-demand GPU coverage"]
fn offscreen_renderer_meter_returns_to_placeholders_after_unload() {
    use crate::render::Renderer;
    use postretro_level_format::lightmap::{LightmapPayloads, LightmapSection};

    let mut renderer = match Renderer::new_offscreen(8, 8) {
        Ok(renderer) => renderer,
        Err(error) if error.to_string().contains("requires a GPU adapter") => return,
        Err(error) => panic!("offscreen renderer must initialize: {error:#}"),
    };
    let boot = renderer
        .lightmap_residency_report()
        .cloned()
        .expect("full renderer");
    assert_eq!(boot.allocations.len(), 5);

    // A 64² two-layer static lightmap and no animated sections.
    let static_size = 64_u32;
    let section = LightmapSection {
        layer_count: 2,
        irr_width: static_size,
        irr_height: static_size,
        irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F,
        irradiance: vec![0u8; (static_size * static_size * 2 * 8) as usize],
        dir_width: static_size,
        dir_height: static_size,
        direction: vec![0u8; (static_size * static_size * 2 * 2) as usize],
        ..LightmapSection::placeholder()
    };
    let (header, payloads) = section.into_parts();
    let install = |renderer: &mut Renderer, header: Option<&_>, payloads| {
        let empty_bvh = crate::render::BvhTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let geometry = crate::render::LevelGeometry {
            vertices: &[],
            indices: &[],
            bvh: &empty_bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: None,
            sh_storage: crate::render::LevelGeometryShStorage::Legacy,
            lightmap: header,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::default(),
            cell_draw_index: None,
            kinematic_geometry: None,
            cells: &[],
            texture_materials: &[],
        };
        renderer.install_level_geometry(&geometry, payloads);
    };

    install(
        &mut renderer,
        Some(&header),
        postretro_level_loader::GpuLightingPayloads {
            lightmap: Some(payloads),
            shadowmask: None,
        },
    );
    let level_a = renderer.lightmap_residency_report().cloned().unwrap();
    assert_eq!(
        level_a.bytes(super::LIGHTMAP_STATIC_IRRADIANCE),
        Some(u64::from(static_size * static_size * 2 * 8))
    );
    for name in [LIGHTMAP_ANIMATED_IRRADIANCE, LIGHTMAP_ANIMATED_DIRECTION] {
        assert_eq!(
            level_a.bytes(name),
            boot.bytes(name),
            "no animated lights: {name} stays at placeholder size"
        );
    }

    renderer.release_level_resources();
    let unloaded = renderer.lightmap_residency_report().cloned().unwrap();
    for (row, boot_row) in unloaded.allocations.iter().zip(&boot.allocations) {
        assert_eq!(row.name, boot_row.name);
        assert_eq!(
            row.bytes, boot_row.bytes,
            "{} returns to its placeholder",
            row.name
        );
    }

    // Level B: a one-layer atlas; its rows are its own, not A's.
    let section_b = LightmapSection {
        layer_count: 1,
        irr_width: static_size,
        irr_height: static_size,
        irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F,
        irradiance: vec![0u8; (static_size * static_size * 8) as usize],
        dir_width: static_size,
        dir_height: static_size,
        direction: vec![0u8; (static_size * static_size * 2) as usize],
        ..LightmapSection::placeholder()
    };
    let (header_b, payloads_b): (_, LightmapPayloads) = section_b.into_parts();
    install(
        &mut renderer,
        Some(&header_b),
        postretro_level_loader::GpuLightingPayloads {
            lightmap: Some(payloads_b),
            shadowmask: None,
        },
    );
    let level_b = renderer.lightmap_residency_report().cloned().unwrap();
    assert_eq!(
        level_b.bytes(super::LIGHTMAP_STATIC_IRRADIANCE),
        Some(u64::from(static_size * static_size * 8))
    );
}
