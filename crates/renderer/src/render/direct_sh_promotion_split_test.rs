// GPU proof of the direct-SH promotion split: the real whole-load compose
// passes write a promotion-free direct atlas that no promotion weight moves,
// and a promotion handoff atlas that subtracts promoted lights. The shared
// group 3 binds the first at binding 15, the mesh superset the second.
// See: context/lib/rendering_pipeline.md §4 (Promoted static lights), §7.1
//
// The compose passes are WGSL, so proving their outputs means running them.
// The harness self-skips without an adapter and says so; a skipped run proves
// nothing. Set `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0`
// to make a missing adapter fail the test.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_render_cpu::frame_uniforms::{LightTermMask, UNIFORM_SIZE};
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, BIND_DYNAMIC_DIRECT_PARAMS, BIND_SH_DIRECT_ATLAS,
};
use wgpu::util::DeviceExt;

use super::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use super::direct_sh_compose::{
    DirectShComposeDebugOverrides, DirectShComposeFrameInputs, DirectShComposePipelines,
    DirectShComposeResources, DirectShComposeTimestampWrites, DirectShDebugOverride,
};
use super::direct_sh_resources::{DirectAtlasLayout, DirectShResources};
use super::gpu_test_harness::{GpuCtx, gpu_or_skip};
use super::pipeline_layout::uniform_bind_group_layout_entries;
use super::renderer_types::{PromotedBakedLightState, PromotedShadowPoolKind};
use super::sh_residency::ShAllocationLedger;
use super::sh_volume::AnimatedLightBuffers;
use super::uploads::UploadQueue;

/// One L0 affinity brick: 64 probes, one stored 6×6 tile each, 8 tiles a row.
const GRID: [u32; 3] = [4, 4, 4];
const TILE: u32 = 6;
const TILES_PER_ROW: u32 = 8;
const ATLAS: [u32; 2] = [TILE * TILES_PER_ROW, TILE * TILES_PER_ROW];
const TEXEL_BYTES: u32 = 8;
const F16_QUARTER: u16 = 0x3400;
const F16_EIGHTH: u16 = 0x3000;
const F16_ONE: u16 = 0x3C00;
/// Static and animated promotion weights each run takes, both at once.
const WEIGHTS: [f32; 3] = [0.0, 0.5, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fixture {
    /// Ids 35 and 41: Pass A writes both final atlases.
    StaticOnly,
    /// Ids 35, 41 and 45: Pass A writes the intermediate, Pass B both finals.
    WithAnimated,
}

/// Per-texel base radiance from four exact f16 values, so every stored slot
/// carries data and a misplaced write would show.
fn base_section() -> DirectShVolumeSection {
    let quarters = [0x3400u16, 0x3800, 0x3A00, F16_ONE];
    let mut atlas = Vec::with_capacity((ATLAS[0] * ATLAS[1] * TEXEL_BYTES) as usize);
    for y in 0..ATLAS[1] {
        for x in 0..ATLAS[0] {
            let value = quarters[((x + 2 * y) % 4) as usize];
            for half in [value, value, value, F16_ONE] {
                atlas.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    DirectShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: GRID,
        tile_dimension: TILE,
        tile_border: 1,
        atlas_dimensions: ATLAS,
        layer_count: 1,
        tiles_per_layer: TILES_PER_ROW * TILES_PER_ROW,
        atlas_tiles_per_row: TILES_PER_ROW,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        atlas,
    }
}

/// One selected static light (selection index 0) over every probe.
fn static_delta() -> DirectShDeltaVolumesSection {
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 1],
        tile_dimension: TILE,
        tile_border: 1,
        valid_probe_masks: vec![u64::MAX],
        cell_levels: vec![0],
        affinity_offsets: vec![0, 1],
        affinity_lights: vec![0],
        delta_subblocks: vec![F16_QUARTER; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

/// One animated-baked light (raw index 0, descriptor 0) over every probe.
fn animated_delta() -> AnimatedDirectShDeltaVolumesSection {
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 1],
        tile_dimension: TILE,
        tile_border: 1,
        animation_descriptor_indices: vec![0],
        valid_probe_masks: vec![u64::MAX],
        cell_levels: vec![0],
        affinity_offsets: vec![0, 1],
        affinity_lights: vec![0],
        delta_subblocks: vec![F16_EIGHTH; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

/// An active descriptor with a constant curve (count 0 samples as 1.0).
fn animation_descriptor() -> Vec<u8> {
    let mut record = vec![0u8; ANIMATION_DESCRIPTOR_SIZE];
    let mut put = |at: usize, word: u32| record[at..at + 4].copy_from_slice(&word.to_ne_bytes());
    put(0, 1.0_f32.to_bits()); // period
    put(16, 2.0_f32.to_bits()); // base colour
    put(20, 1.0_f32.to_bits());
    put(24, 0.5_f32.to_bits());
    put(36, 1); // is_active
    record
}

/// Level-0 indirection: probe `p` stores at slot `p`.
fn probe_indirection_words() -> Vec<u32> {
    (0..GRID.iter().product::<u32>())
        .map(|probe| (probe << 5) | 0x4)
        .collect()
}

struct Scene {
    direct: DirectShResources,
    compose: DirectShComposeResources,
    weights: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

fn build_scene(ctx: &GpuCtx, fixture: Fixture) -> Scene {
    let device = &ctx.device;
    let base = base_section();
    let delta = static_delta();
    let animated = (fixture == Fixture::WithAnimated).then(animated_delta);
    let mut ledger = ShAllocationLedger::new();
    let direct = DirectShResources::new(
        device,
        &ctx.queue,
        Some(&base),
        Some(&delta),
        animated.as_ref(),
        Some(DirectAtlasLayout::from_direct_section(&base)),
        true,
        &mut ledger,
    );
    assert!(direct.has_direct, "the fixture's base uploads");
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("promotion split uniforms layout"),
        entries: &uniform_bind_group_layout_entries(),
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("promotion split uniforms"),
        contents: &[0u8; UNIFORM_SIZE],
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("promotion split uniforms"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("promotion split selection weights"),
        contents: &0.0_f32.to_ne_bytes(),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let animation = AnimatedLightBuffers::for_test(device, animation_descriptor());
    let compose = DirectShComposeResources::new(
        device,
        &direct,
        &animation,
        &probe_indirection_words(),
        Some(&delta),
        animated.as_ref(),
        &weights,
        &DirectShComposePipelines::new(device, &uniform_layout),
        true,
        &mut ledger,
    );
    assert!(
        compose.promotion_pipeline().is_some(),
        "the fixture composes"
    );
    Scene {
        direct,
        compose,
        weights,
        uniform_bind_group,
        uniform_buffer,
    }
}

/// Both final atlases' bytes after one compose at promotion weight `weight`
/// for the static and the animated light: `(promotion-free, handoff)`.
fn compose_at(
    ctx: &GpuCtx,
    scene: &mut Scene,
    weight: f32,
    mask: LightTermMask,
) -> (Vec<u8>, Vec<u8>) {
    let mut uniforms = [0u8; UNIFORM_SIZE];
    uniforms[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
    ctx.queue.write_buffer(&scene.uniform_buffer, 0, &uniforms);
    ctx.queue
        .write_buffer(&scene.weights, 0, &weight.to_ne_bytes());
    let animated_states = [PromotedBakedLightState {
        weight,
        pool_kind: Some(PromotedShadowPoolKind::Spot),
        ..PromotedBakedLightState::default()
    }];
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    scene.compose.dispatch_if_needed(
        &uploads,
        &mut encoder,
        DirectShComposeFrameInputs {
            uniform_bind_group: &scene.uniform_bind_group,
            active: true,
            light_term_mask: mask,
            debug_overrides: DirectShComposeDebugOverrides {
                promotion: DirectShDebugOverride::default(),
                animated: AnimatedDirectShDebugOverride::default(),
            },
            animated_promotion_states: &animated_states,
            timestamp_writes: DirectShComposeTimestampWrites {
                promotion: None,
                animated: None,
            },
        },
    );
    let promotion_free = copy_out(&ctx.device, &mut encoder, scene.direct.atlas_view.texture());
    let handoff = copy_out(
        &ctx.device,
        &mut encoder,
        scene.direct.handoff_atlas_view.texture(),
    );
    uploads.submit([encoder.finish()]);
    (
        read(&ctx.device, &promotion_free),
        read(&ctx.device, &handoff),
    )
}

/// 256-byte-aligned row stride for one Rgba16Float atlas row.
fn padded_row_bytes() -> u32 {
    (ATLAS[0] * TEXEL_BYTES).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn copy_out(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("promotion split readback"),
        size: u64::from(padded_row_bytes() * ATLAS[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes()),
                rows_per_image: Some(ATLAS[1]),
            },
        },
        wgpu::Extent3d {
            width: ATLAS[0],
            height: ATLAS[1],
            depth_or_array_layers: 1,
        },
    );
    buffer
}

/// De-padded atlas bytes.
fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    receiver
        .recv()
        .expect("map callback")
        .expect("readback maps");
    let mapped = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let row = (ATLAS[0] * TEXEL_BYTES) as usize;
    let bytes = mapped
        .chunks(padded_row_bytes() as usize)
        .flat_map(|padded| padded[..row].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    bytes
}

/// The red channel of texel `(x, y)`, decoded from f16.
fn red(bytes: &[u8], x: u32, y: u32) -> f32 {
    let at = ((y * ATLAS[0] + x) * TEXEL_BYTES) as usize;
    f16_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f32::from(bits & 0x3FF);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

fn assert_split(ctx: &GpuCtx, fixture: Fixture) {
    let mut scene = build_scene(ctx, fixture);
    let runs: Vec<(f32, Vec<u8>, Vec<u8>)> = WEIGHTS
        .iter()
        .map(|&weight| {
            let (free, handoff) = compose_at(ctx, &mut scene, weight, LightTermMask::ALL);
            (weight, free, handoff)
        })
        .collect();
    let (_, free_at_zero, handoff_at_zero) = &runs[0];

    assert_eq!(
        free_at_zero, handoff_at_zero,
        "{fixture:?}: with every promotion weight zero the two atlases agree byte for byte",
    );
    for (weight, free, handoff) in &runs[1..] {
        assert_eq!(
            free, free_at_zero,
            "{fixture:?}: the promotion-free atlas must not move at weight {weight}",
        );
        assert_ne!(
            handoff, handoff_at_zero,
            "{fixture:?}: the handoff atlas must subtract promoted lights at weight {weight}",
        );
    }

    // Exact values at one texel whose base red is 0.25: the static delta is
    // 0.25 a unit weight, the animated one 0.125 × base colour red 2.0.
    let (x, y) = (0, 0);
    let animated_red = if fixture == Fixture::WithAnimated {
        0.25
    } else {
        0.0
    };
    assert_eq!(red(free_at_zero, x, y), 0.25 + animated_red);
    let (_, _, handoff_at_one) = &runs[2];
    assert_eq!(
        red(handoff_at_one, x, y),
        0.0,
        "{fixture:?}: at full weight the handoff atlas drops both promoted lights",
    );

    // Promotion-free means independent of the dynamic-direct bit too: the
    // handoff atlas only subtracts while dynamic direct can add lights back.
    let mut dynamic_off = LightTermMask::ALL;
    dynamic_off.set_enabled(LightTermMask::DYNAMIC_DIRECT, false);
    let (free, _) = compose_at(ctx, &mut scene, 1.0, dynamic_off);
    assert_eq!(
        &free, free_at_zero,
        "{fixture:?}: the promotion-free atlas ignores the dynamic-direct bit",
    );

    // The baked-direct mask bits still select terms in the promotion-free atlas.
    let mut static_off = LightTermMask::ALL;
    static_off.set_enabled(LightTermMask::BAKED_DIRECT_STATIC, false);
    let (free, _) = compose_at(ctx, &mut scene, 0.0, static_off);
    assert_eq!(
        red(&free, x, y),
        animated_red,
        "{fixture:?}: clearing baked-direct-static removes the base from the promotion-free atlas",
    );
}

#[test]
fn promotion_free_direct_atlas_ignores_promotion_weights_without_section_45() {
    let Some(ctx) =
        gpu_or_skip("promotion_free_direct_atlas_ignores_promotion_weights_without_section_45")
    else {
        return;
    };
    assert_split(&ctx, Fixture::StaticOnly);
}

#[test]
fn promotion_free_direct_atlas_ignores_promotion_weights_with_section_45() {
    let Some(ctx) =
        gpu_or_skip("promotion_free_direct_atlas_ignores_promotion_weights_with_section_45")
    else {
        return;
    };
    assert_split(&ctx, Fixture::WithAnimated);
}

/// The view a binding-15 entry names.
fn binding_15_view<'a>(entries: &'a [wgpu::BindGroupEntry<'a>]) -> &'a wgpu::TextureView {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.binding == BIND_SH_DIRECT_ATLAS);
    let entry = matches.next().expect("binding 15 is bound");
    assert!(matches.next().is_none(), "binding 15 is bound once");
    match &entry.resource {
        wgpu::BindingResource::TextureView(view) => view,
        other => panic!("binding 15 must be a texture view, got {other:?}"),
    }
}

#[test]
fn whole_load_group_3_binds_the_promotion_free_atlas_and_the_mesh_superset_the_handoff() {
    let Some(ctx) = gpu_or_skip(
        "whole_load_group_3_binds_the_promotion_free_atlas_and_the_mesh_superset_the_handoff",
    ) else {
        return;
    };
    for fixture in [Fixture::StaticOnly, Fixture::WithAnimated] {
        let scene = build_scene(&ctx, fixture);
        let direct = &scene.direct;
        let shared = vec![direct.shared_atlas_entry()];
        let mesh = direct.mesh_superset_entries(&shared);

        let shared_view = binding_15_view(&shared);
        let mesh_view = binding_15_view(&mesh);
        assert_eq!(shared_view, &direct.atlas_view, "{fixture:?}");
        assert_eq!(mesh_view, &direct.handoff_atlas_view, "{fixture:?}");
        assert_ne!(
            shared_view.texture(),
            mesh_view.texture(),
            "{fixture:?}: the two groups bind distinct atlases",
        );
        // Each sampled view names the texture its compose pass writes.
        assert_eq!(
            shared_view.texture(),
            direct.atlas_storage_view.as_ref().unwrap().texture(),
            "{fixture:?}",
        );
        assert_eq!(
            mesh_view.texture(),
            direct.handoff_storage_view.as_ref().unwrap().texture(),
            "{fixture:?}",
        );
        assert!(
            mesh.iter()
                .any(|entry| entry.binding == BIND_DYNAMIC_DIRECT_PARAMS),
            "{fixture:?}: the mesh superset keeps its dynamic-direct params",
        );
        assert_eq!(mesh.len(), shared.len() + 1, "{fixture:?}");
    }
}
