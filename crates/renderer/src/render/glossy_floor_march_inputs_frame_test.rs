// GPU proof that the floor march reads its material as the overlay does: the
// march rasterises at the floor tier's fraction of the scene, yet the texel's
// exponent and its normal-map normal, read through the shared helpers on a
// material whose every mip differs, match the overlay's full-extent reads at
// the same floor, at High (½) and Medium (¼).
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{UVec2, Vec3};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_data::material::{Material, TextureMaterial};

use super::GlossyFloorMarchPipelines;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::FRAME;
use crate::render::glossy_floor_frame_test_fixtures::read_floor_layer;
use crate::render::glossy_overlay::GlossyOverlayPipelines;
use crate::render::glossy_overlay_frame_test_fixtures::{renderer, with_variant};
use crate::render::glossy_overlay_room_frame_test_fixtures::{
    Room, RoomMaterial, room_floor_view, room_frame,
};
use crate::render::material_plan::{build_material_bind_group, create_mip_aniso_sampler};

/// The material's mip-0 size: over the room's 6 m × 8 m floor its texels are
/// 2–3 cm, a scene pixel of the floor view spans about two of them, so the
/// overlay reads mip 1 or so and each coarser fraction one mip more.
const SIZE: u32 = 256;
/// The chain's levels: 256² down to 1².
const LEVELS: u32 = 9;

/// The specular slot's smoothness byte at each mip: below the exponent cap
/// at every mip, and from mip 0 to mip 6 each mip's decoded log₂ n is more
/// than one below the last's, so a read a mip coarse moves it by more than
/// one.
const SMOOTHNESS: [u8; LEVELS as usize] = [210, 185, 160, 135, 110, 85, 60, 35, 10];

/// The normal map's tangent-space x at each mip, as a byte: a read a mip
/// coarse moves the decoded x by about 0.16.
fn normal_x(level: u32) -> u8 {
    (128 + 20 * level).min(255) as u8
}

/// A uniform grey probe capture: the proof reads no probe colour.
fn grey(_direction: Vec3, _mip: u32) -> Vec3 {
    Vec3::splat(0.5)
}

/// The march's inputs where it writes them: log₂ of the exponent it reads, and
/// its normal map's tangent-space x and y, through the derivatives and helpers
/// its production entry uses; c = 1 so every covered texel reads back.
const MARCH_INPUTS: &str = r#"
@fragment
fn fs_test_march_inputs(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let d = glossy_floor_derivatives(in);
    let depth = glossy_overlay_depth(in, d, false);
    let exponent = glossy_floor_exponent(depth.uv, d.ddx, d.ddy);
    let n = glossy_overlay_tangent_normal(depth.uv, d, true);
    return vec4<f32>(log2(exponent), n.x, n.y, 1.0);
}
"#;

/// The overlay's reads of the same inputs at its own fragment, added over
/// forward on the floor alone: log₂ of its texel's exponent plus one (so a
/// floor pixel never adds 0), and its normal map's x and y plus one.
const OVERLAY_INPUTS: &str = r#"
@fragment
fn fs_test_overlay_inputs(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let d = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_depth(in, d, false);
    let texel = glossy_overlay_texel(depth.uv, d.ddx, d.ddy);
    let n = glossy_overlay_tangent_normal(depth.uv, d, true);
    let inputs = vec3<f32>(log2(texel.exponent), n.x, n.y) + vec3<f32>(1.0);
    return vec4<f32>(select(vec3<f32>(0.0), inputs, in.world_normal.y > 0.9), 0.0);
}
"#;

/// Replace bucket 0's material with a white, fully reflective, normal-mapped
/// one whose specular slot and normal map hold a different value at every
/// mip, bound through a sampler that reads the whole chain.
fn install_mip_coded_material(renderer: &mut Renderer) {
    use crate::render::loaded_texture::{placeholder_loaded_texture, upload_texture_data};
    let device = renderer.device.clone();
    let queue = renderer.queue.raw().clone();
    let chain = |texel: &dyn Fn(u32) -> [u8; 4]| -> Vec<(u32, Vec<u8>)> {
        (0..LEVELS)
            .map(|level| {
                let side = SIZE >> level;
                (side, texel(level).repeat((side * side) as usize))
            })
            .collect()
    };
    let upload = |format, levels: &[(u32, Vec<u8>)], label| {
        let refs: Vec<_> = levels
            .iter()
            .map(|(side, bytes)| (*side, *side, &bytes[..]))
            .collect();
        upload_texture_data(&device, &queue, format, &refs, label)
    };
    let mut texture = placeholder_loaded_texture(&device, &queue);
    (texture.diffuse_texture, texture.diffuse_view) = upload(
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &chain(&|_| [255; 4]),
        "march inputs albedo",
    );
    let slot = chain(&|level| [255, 127, SMOOTHNESS[level as usize], 254]);
    (texture.specular_texture, texture.specular_view) =
        upload(wgpu::TextureFormat::Rgba8Unorm, &slot, "march inputs slot");
    (texture.normal_texture, texture.normal_view) = upload(
        wgpu::TextureFormat::Rgba8Unorm,
        &chain(&|level| [normal_x(level), 100, 255, 255]),
        "march inputs normal",
    );
    texture.mip_count = LEVELS;
    texture.reflects = true;
    texture.normal_mapped = true;
    let sampler = create_mip_aniso_sampler(&device, LEVELS);
    let binding = build_material_bind_group(
        &device,
        &renderer.full().texture_bind_group_layout,
        &texture,
        &sampler,
        TextureMaterial {
            material: Material::Default,
            emissive_strength: 0.0,
        },
        renderer.surface_depth_quality,
        "Glossy March Inputs Material",
    );
    let level = renderer.level_mut();
    level.loaded_textures[0] = texture;
    level.gpu_textures[0] = binding.into();
}

/// One frame with the march's inputs in its target, read back as
/// (log₂ n, x, y) per covered target texel.
fn march_inputs(renderer: &mut Renderer) -> (UVec2, Vec<Option<Vec3>>) {
    let variant = renderer.full().glossy_overlay.floor().march_variant(
        &renderer.device,
        MARCH_INPUTS,
        "fs_test_march_inputs",
    );
    let production = renderer
        .full_mut()
        .glossy_overlay
        .floor_mut()
        .swap_march(GlossyFloorMarchPipelines::every(variant));
    room_frame(renderer, room_floor_view(), 0.0);
    renderer
        .full_mut()
        .glossy_overlay
        .floor_mut()
        .swap_march(production);
    let layer = read_floor_layer(renderer).expect("the floor tier runs the floor layer");
    let texels = layer
        .texels
        .iter()
        .map(|texel| (texel.w == 1.0).then(|| texel.truncate()))
        .collect();
    (layer.extent, texels)
}

/// One frame with the overlay's reads of the same inputs, less forward's
/// colour: (log₂ n, x, y) per scene pixel of the floor.
fn overlay_inputs(renderer: &mut Renderer) -> Vec<Option<Vec3>> {
    let variant = renderer.full().glossy_overlay.variant_pipeline(
        &renderer.device,
        OVERLAY_INPUTS,
        "fs_test_overlay_inputs",
    );
    let production = renderer
        .full_mut()
        .glossy_overlay
        .swap_pipelines(GlossyOverlayPipelines::both(variant));
    let drawn = room_frame(renderer, room_floor_view(), 0.0);
    renderer
        .full_mut()
        .glossy_overlay
        .swap_pipelines(production);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        room_frame(renderer, room_floor_view(), 0.0)
    });
    (0..FRAME * FRAME)
        .map(|index| {
            let pixel = UVec2::new(index % FRAME, index / FRAME);
            let added = drawn.colour(pixel) - forward.colour(pixel);
            (added.x > 0.5).then(|| added - Vec3::ONE)
        })
        .collect()
}

// The march reads the exponent and the normal map at the overlay's mip on a
// floor whose every mip differs: each target texel whose scene pixels all lie
// on the floor matches the mean of the overlay's reads over them, at High and
// Medium. Derivatives taken at the target's own extent read one (High) or two
// (Medium) mips coarse and fail it.
#[test]
fn the_floor_march_reads_its_material_at_the_overlays_mip() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    Room::new([grey; 3]).install(&mut renderer, &RoomMaterial::WHITE_MIRROR);
    install_mip_coded_material(&mut renderer);
    for quality in [FluidReflectionQuality::High, FluidReflectionQuality::Medium] {
        renderer.set_reflection_quality(quality);
        let (extent, march) = march_inputs(&mut renderer);
        let overlay = overlay_inputs(&mut renderer);
        let block = FRAME / extent.x;
        assert_eq!(
            extent,
            UVec2::splat(FRAME / block),
            "{quality:?}: square target"
        );
        let mut compared = 0;
        let mut worst = Vec3::ZERO;
        for y in 0..extent.y {
            for x in 0..extent.x {
                let Some(marched) = march[(y * extent.x + x) as usize] else {
                    continue;
                };
                let reads: Option<Vec<Vec3>> = (0..block * block)
                    .map(|i| {
                        let pixel = UVec2::new(x * block + i % block, y * block + i / block);
                        overlay[(pixel.y * FRAME + pixel.x) as usize]
                    })
                    .collect();
                let Some(reads) = reads else {
                    continue;
                };
                let mean = reads.iter().sum::<Vec3>() / reads.len() as f32;
                worst = worst.max((marched - mean).abs());
                compared += 1;
            }
        }
        assert!(
            compared >= 16,
            "{quality:?}: {compared} floor texels compared"
        );
        // A mip coarser moves log₂ n by more than 1 and x by about 0.16;
        // matching reads differ by the level's spread over a block (up to
        // 4 × 4 scene pixels at Medium) and the stored formats' rounding.
        // On Metal, reads at the target's own derivatives miss by more than
        // 2.5 and 0.17; at the scene's, by under 0.6 and 0.04.
        assert!(
            worst.x < 1.0 && worst.y < 0.08,
            "{quality:?}: the march's reads differ from the overlay's by {worst} over {compared} texels"
        );
    }
}
