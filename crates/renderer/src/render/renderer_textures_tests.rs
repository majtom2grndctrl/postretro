// Off-main world-texture upload against the main-thread install oracle.
// See: context/lib/testing_guide.md §3 (GPU tests) · context/lib/boot_sequence.md §2

use super::*;
use crate::compute_cull::IndirectDrawCommand;
use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
use crate::render::upload_thread::tests::wait_for;
use crate::render::{ClearColor, LevelGeometry, LevelGeometryShStorage, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
    expected_level_count,
};
use postretro_level_loader::{CellData, ShDrainBatch};
use postretro_render_cpu::texture_prep::prepare_world_textures;
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

/// A full mip chain of `format` with distinct bytes per level and texel.
pub(in crate::render) fn chain(format: PrmFormat, width: u16, height: u16, seed: u8) -> PrmSlot {
    let level_count = expected_level_count(width, height);
    let bpp = match format {
        PrmFormat::R8Unorm => 1,
        _ => 4,
    };
    let mut payload = Vec::new();
    for level in 0..level_count {
        let w = (u32::from(width) >> level).max(1);
        let h = (u32::from(height) >> level).max(1);
        payload.extend((0..bpp * w * h).map(|i| (i as u8).wrapping_mul(7) ^ seed ^ level));
    }
    PrmSlot {
        format,
        width,
        height,
        level_count,
        payload,
    }
}

pub(in crate::render) fn write_prm(root: &Path, key: [u8; 32], slots: [Option<PrmSlot>; 4]) {
    let mut slot_mask = PrmSlots::empty();
    for (present, bit) in slots.iter().zip([
        PrmSlots::DIFFUSE,
        PrmSlots::SPECULAR,
        PrmSlots::NORMAL,
        PrmSlots::EMISSIVE,
    ]) {
        if present.is_some() {
            slot_mask |= bit;
        }
    }
    let bytes = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask,
            bundle_hash: [0u8; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots,
    }
    .to_bytes()
    .expect("fixture .prm serializes");
    std::fs::write(
        root.join(format!("{}.prm", cache_filename_for_key(&key))),
        bytes,
    )
    .unwrap();
}

/// Every resource-shaping property of one installed material.
fn summary(renderer: &Renderer) -> Vec<String> {
    let full = renderer.full();
    let describe = |texture: &wgpu::Texture| {
        format!(
            "{}x{} {:?} mips={}",
            texture.width(),
            texture.height(),
            texture.format(),
            texture.mip_level_count()
        )
    };
    let mut rows: Vec<String> = full
        .loaded_textures
        .iter()
        .map(|t| {
            format!(
                "mip_count={} diffuse=[{}] specular=[{}] normal=[{}] emissive=[{}] relief={:?}",
                t.mip_count,
                describe(&t.diffuse_texture),
                describe(&t.specular_texture),
                describe(&t.normal_texture),
                describe(&t.emissive_texture),
                t.surface_relief,
            )
        })
        .collect();
    rows.push(format!("bind groups={}", full.gpu_textures.len()));
    rows
}

/// Reads mip 0 of an sRGB `view` through a compute probe on the main thread's
/// queue, re-encoded to bytes. World textures carry no `COPY_SRC`, so a
/// texture-to-buffer copy is not available; a `textureLoad` reads exactly what
/// a draw would sample.
fn read_srgb_mip0(
    renderer: &Renderer,
    view: &wgpu::TextureView,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let device = &renderer.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture mip readback probe"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../shaders/texture_mip_readback.wgsl").into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("texture mip readback probe"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let size = u64::from(width * height) * 16;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture mip probe output"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture mip probe readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture mip probe"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("texture mip probe"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(width, height, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    renderer.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map ok");
    let texels: Vec<f32> =
        bytemuck::cast_slice(&slice.get_mapped_range().expect("mapped")).to_vec();
    readback.unmap();
    // The load decodes sRGB colour to linear; alpha is stored linear.
    let encode = |linear: f32| {
        if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        }
    };
    texels
        .chunks(4)
        .flat_map(|texel| {
            [
                encode(texel[0]),
                encode(texel[1]),
                encode(texel[2]),
                texel[3],
            ]
            .map(|value| (value * 255.0).round() as u8)
        })
        .collect()
}

/// One triangle in one cell whose leaf draws material bucket 0.
pub(in crate::render) fn install_one_triangle(renderer: &mut Renderer, materials: &[Material]) {
    install_one_triangle_with_sh(renderer, materials, None, None);
}

/// [`install_one_triangle`] with an SH volume and baked direct SH, so the SH
/// and direct compose families bind real data.
pub(in crate::render) fn install_one_triangle_with_sh(
    renderer: &mut Renderer,
    materials: &[Material],
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    direct_sh_volume: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
) {
    let vertices: Vec<WorldVertex> = [[-0.4, -0.4, 0.0], [0.4, -0.4, 0.0], [0.0, 0.4, 0.0]]
        .map(|position| WorldVertex {
            position,
            base_uv: [0.0; 2],
            normal_oct: [32768; 2],
            tangent_packed: [0; 2],
            lightmap_uv: [0; 2],
            lightmap_block: 0,
            animated_block: 0,
        })
        .to_vec();
    let indices = [0u32, 1, 2];
    let leaf = BvhLeaf {
        aabb_min: [-0.5; 3],
        material_bucket_id: 0,
        aabb_max: [0.5; 3],
        index_offset: 0,
        index_count: 3,
        cell_id: 0,
        chunk_range_start: 0,
        chunk_range_count: 0,
    };
    let bvh = BvhTree {
        nodes: vec![
            BvhNode {
                aabb_min: leaf.aabb_min,
                skip_index: 2,
                aabb_max: leaf.aabb_max,
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            BvhNode {
                aabb_min: leaf.aabb_min,
                skip_index: 2,
                aabb_max: leaf.aabb_max,
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
            },
        ],
        leaves: vec![leaf],
        root_node_index: 0,
    };
    let index = CellDrawIndexSection::from_bytes(
        &CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        }
        .to_bytes(),
    )
    .unwrap();
    let cells = [CellData {
        bounds_min: Vec3::splat(-0.5),
        bounds_max: Vec3::splat(0.5),
        face_start: 0,
        face_count: 1,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    }];
    renderer.install_level_geometry(
        &LevelGeometry {
            vertices: &vertices,
            vertex_spec_owners: &[],
            indices: &indices,
            bvh: &bvh,
            lights: &[],
            light_influences: &[],
            sh_volume,
            sh_storage: LevelGeometryShStorage::Legacy,
            lightmap: None,
            lightmap_streaming: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            cell_draw_index: Some(&index),
            kinematic_geometry: None,
            cells: &cells,
            texture_materials: materials,
        },
        Default::default(),
    );
}

#[test]
fn off_main_world_texture_upload_matches_main_thread_install() {
    let test = "off_main_world_texture_upload_matches_main_thread_install";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let small = [0x11u8; 32];
    let wide = [0x22u8; 32];
    let missing = [0x33u8; 32];
    let small_diffuse = chain(PrmFormat::Rgba8UnormSrgb, 4, 4, 0x5A);
    let small_mip0 = small_diffuse.payload[..4 * 4 * 4].to_vec();
    write_prm(dir.path(), small, [Some(small_diffuse), None, None, None]);
    write_prm(
        dir.path(),
        wide,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 8, 4, 0x3C)),
            Some(chain(PrmFormat::R8Unorm, 8, 4, 0x77)),
            None,
            None,
        ],
    );
    let names: Vec<String> = ["small", "zero", "missing", "wide"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    let keys = TextureCacheKeysSection {
        keys: vec![small, [0u8; 32], missing, wide],
    };
    let materials = vec![Material::Default; names.len()];

    // Oracle: today's main-thread install.
    renderer.install_textures(&names, &keys, dir.path(), &materials);
    let oracle = summary(&renderer);
    assert_eq!(oracle.len(), names.len() + 1);

    let prepared = prepare_world_textures(&names, &keys, dir.path());
    let uploaded = wait_for(renderer.begin_world_texture_upload(prepared))
        .expect("the upload thread reports no error");

    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    renderer.install_uploaded_textures(uploaded, &materials);
    assert_eq!(
        summary(&renderer),
        oracle,
        "off-main install matches the oracle"
    );

    // The upload thread submitted its writes before handing the result back,
    // so this main-thread submit samples the uploaded bytes.
    let diffuse = &renderer.full().loaded_textures[0].diffuse_view;
    assert_eq!(
        read_srgb_mip0(&renderer, diffuse, 4, 4),
        small_mip0,
        "mip 0 written off-main is visible to the main thread"
    );

    install_one_triangle(&mut renderer, &materials);
    let cells = VisibleCells::Culled(vec![0]);
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            Vec3::ZERO,
            &[],
            &[],
            1.0,
            ClearColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            true,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records");
    let forward_binds: Vec<u32> = renderer
        .full()
        .compute_cull
        .as_ref()
        .expect("a level with leaves builds the cull")
        .draw_trace
        .borrow()
        .last()
        .expect("the frame traced its forward draws")
        .iter()
        .filter_map(|command| match command {
            IndirectDrawCommand::BindMaterial(bucket) => Some(*bucket),
            _ => None,
        })
        .collect();
    assert_eq!(
        forward_binds,
        vec![0],
        "the forward pass binds the off-main material"
    );
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "install, readback or frame raised a validation error: {error:?}"
    );
}
