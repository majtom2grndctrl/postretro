//! Fixtures for the glossy floor layer's GPU proofs: a room of a bound
//! glossy floor and two walls behind it (a glossy wall, and a wall whose
//! material has `_s` and no `_p`), test-only forward and mover sources that
//! draw a bright highlight, and the highlight-free colour and scene-colour
//! chain read back.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{UVec2, Vec3};
use postretro_render_cpu::sh_compose::f16_bits_to_f32;

use super::fluid_reflection_frame_test_fixtures::{View, view_from};
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::Renderer;
use super::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, GlossyFrame, floor_probe, set_bucket_material, with_faces,
};
use super::glossy_overlay_gate_frame_fixtures::{ALL, frame_seeing};
use super::gpu_test_harness::wgsl_function;

/// The test highlight: forward's world static specular, and so its
/// highlight, is this times the texel's `_s` intensity.
pub(super) const HIGHLIGHT: f32 = 4.0;

/// The room: a floor at y = 0 over x ∈ [-3, 3], z ∈ [1, 9], and at z = 9,
/// facing −Z, the `_s`-only wall over x ∈ [-3, 0] and the glossy wall over
/// x ∈ [0, 3], each 3 m tall.
pub(super) const ROOM_X: [f32; 2] = [-3.0, 3.0];
pub(super) const ROOM_Z: [f32; 2] = [1.0, 9.0];
pub(super) const WALL_HEIGHT: f32 = 3.0;

/// The room's faces in face (and leaf) order, sorted by bucket: 0 the
/// glossy floor (bound to the probe), 1 the glossy wall (unbound), both
/// bucket 0; 2 the `_s`-only wall, bucket 1.
pub(super) fn room_faces() -> Vec<FixtureFace> {
    let up = postretro_level_format::octahedral::encode(0.0, 1.0, 0.0);
    let toward_eye = postretro_level_format::octahedral::encode(0.0, 0.0, -1.0);
    let (x, z, h) = (ROOM_X, ROOM_Z, WALL_HEIGHT);
    let wall = |x0: f32, x1: f32, bucket| FixtureFace {
        corners: [
            [x0, 0.0, z[1]],
            [x1, 0.0, z[1]],
            [x1, h, z[1]],
            [x0, h, z[1]],
        ],
        normal_oct: toward_eye,
        bucket,
    };
    vec![
        FixtureFace {
            corners: [
                [x[0], 0.0, z[0]],
                [x[1], 0.0, z[0]],
                [x[1], 0.0, z[1]],
                [x[0], 0.0, z[1]],
            ],
            normal_oct: up,
            bucket: 0,
        },
        wall(0.0, x[1], 0),
        wall(x[0], 0.0, 1),
    ]
}

/// The `_s`-only wall's material: a warm albedo, full `_s`, no `_p` (B 0),
/// no `_t` (A 255), so its highlight rides the albedo multiply untinted.
pub(super) const SPECULAR_WALL: FloorMaterial = FloorMaterial {
    albedo: [200, 120, 60, 255],
    slot: [255, 0, 0, 255],
    glow: None,
};

/// The glossy faces' material with a broad highlight (smoothness byte 100,
/// exponent about 12, so a runtime light's real Blinn-Phong highlight
/// covers many pixels) and `_t` 0: a tinted, white highlight.
pub(super) const GLOSSY_BROAD_TINTED: FloorMaterial = FloorMaterial {
    albedo: [180, 200, 255, 255],
    slot: [255, 0, 100, 0],
    glow: None,
};

/// [`GLOSSY_BROAD_TINTED`] with no `_t` (A 255): an untinted highlight,
/// so a mover drawn with it takes the pixel loop's untinted sum.
pub(super) const GLOSSY_BROAD_UNTINTED: FloorMaterial = FloorMaterial {
    albedo: [180, 200, 255, 255],
    slot: [255, 0, 100, 255],
    glow: None,
};

/// The proofs' camera: 1 m above the floor at the room's near end, looking
/// at the walls over the floor.
pub(super) fn room_view() -> View {
    view_from(Vec3::new(0.0, 1.0, 2.0), Vec3::new(0.0, 0.9, 9.0))
}

/// Install the room on the main thread with one probe bound to the floor,
/// bucket 0 `glossy` (glossy), bucket 1 [`SPECULAR_WALL`], an ambient floor
/// so every face shows its albedo.
pub(super) fn install_room(renderer: &mut Renderer, glossy: FloorMaterial) {
    install_faces(renderer, &room_faces(), &floor_probe(true));
    set_bucket_material(renderer, 0, glossy, true);
    set_bucket_material(renderer, 1, SPECULAR_WALL, false);
    renderer.set_ambient_floor(0.5);
}

/// Install `faces` with `probes` as id 54 and no fluid volume.
pub(super) fn install_faces(
    renderer: &mut Renderer,
    faces: &[FixtureFace],
    probes: &ReflectionProbesSection,
) {
    with_faces(faces, Some(probes), None, |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    renderer.install_fluid_surfaces(&[], Some(probes));
}

/// The floor targets as the last frame left them, read back: the
/// highlight-free colour and every chain level, each with its extent.
#[derive(Debug, PartialEq)]
pub(super) struct FloorTargets {
    pub highlight_free: (UVec2, Vec<Vec3>),
    /// Level 0 first.
    pub chain: Vec<(UVec2, Vec<Vec3>)>,
}

impl FloorTargets {
    pub fn highlight_free(&self, pixel: UVec2) -> Vec3 {
        let (extent, texels) = &self.highlight_free;
        texels[(pixel.y * extent.x + pixel.x) as usize]
    }
}

/// The active level's floor targets, read back, or `None` when they are
/// not allocated.
pub(super) fn read_floor_targets(renderer: &Renderer) -> Option<FloorTargets> {
    let targets = renderer.level().glossy.as_ref()?.floor()?;
    let chain = targets.chain_texture();
    Some(FloorTargets {
        highlight_free: read_level(renderer, targets.highlight_free_texture(), 0),
        chain: (0..chain.mip_level_count())
            .map(|level| read_level(renderer, chain, level))
            .collect(),
    })
}

/// The scene colour the last frame left, at any extent, read back.
pub(super) fn read_scene_colour(renderer: &Renderer) -> (UVec2, Vec<Vec3>) {
    let texture = renderer
        .full()
        .screen_effects
        .scene_color_view()
        .texture()
        .clone();
    read_level(renderer, &texture, 0)
}

/// One proof-extent frame and what it left: the scene colour, D_static,
/// and the floor targets while they exist.
pub(super) struct FloorFrame {
    pub glossy: GlossyFrame,
    pub floor: Option<FloorTargets>,
}

impl FloorFrame {
    pub fn colour(&self, pixel: UVec2) -> Vec3 {
        self.glossy.colour(pixel)
    }

    pub fn highlight_free(&self, pixel: UVec2) -> Vec3 {
        self.floor
            .as_ref()
            .expect("the floor targets are allocated")
            .highlight_free(pixel)
    }
}

/// One draw-all frame through `view` at time 0, read back.
pub(super) fn floor_frame(renderer: &mut Renderer, view: View) -> FloorFrame {
    let glossy = frame_seeing(renderer, view, &ALL);
    FloorFrame {
        glossy,
        floor: read_floor_targets(renderer),
    }
}

/// Mip `level` of `texture`, an `Rg11b10Ufloat` or `Rgba16Float` colour,
/// decoded: its extent and its texels row by row.
pub(super) fn read_level(
    renderer: &Renderer,
    texture: &wgpu::Texture,
    level: u32,
) -> (UVec2, Vec<Vec3>) {
    let size = texture.size().mip_level_size(level, texture.dimension());
    let texel_bytes = texture
        .format()
        .block_copy_size(None)
        .expect("a colour format");
    let row = size.width * texel_bytes;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Glossy floor proof readback"),
        size: u64::from(padded * size.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Glossy floor proof readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.height),
            },
        },
        wgpu::Extent3d {
            depth_or_array_layers: 1,
            ..size
        },
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "glossy floor proof readback");
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map callback").expect("map");
    let data = slice.get_mapped_range().expect("mapped");
    let format = texture.format();
    let texels = (0..size.height)
        .flat_map(|y| {
            let start = (y * padded) as usize;
            data[start..start + row as usize]
                .chunks_exact(texel_bytes as usize)
                .map(|texel| decode_colour(format, texel))
                .collect::<Vec<_>>()
        })
        .collect();
    drop(data);
    buffer.unmap();
    (UVec2::new(size.width, size.height), texels)
}

/// One colour texel's RGB, linear.
fn decode_colour(format: wgpu::TextureFormat, texel: &[u8]) -> Vec3 {
    match format {
        wgpu::TextureFormat::Rgba16Float => Vec3::from(
            [0, 1, 2]
                .map(|c| f16_bits_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]))),
        ),
        wgpu::TextureFormat::Rg11b10Ufloat => {
            let bits = u32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]]);
            Vec3::new(
                unsigned_float(bits & 0x7FF, 6),
                unsigned_float((bits >> 11) & 0x7FF, 6),
                unsigned_float(bits >> 22, 5),
            )
        }
        format => panic!("unexpected floor-layer format {format:?}"),
    }
}

/// An unsigned float of a 5-bit exponent (bias 15) and `mantissa_bits`.
fn unsigned_float(bits: u32, mantissa_bits: u32) -> f32 {
    let exponent = (bits >> mantissa_bits) as i32;
    let mantissa = (bits & ((1 << mantissa_bits) - 1)) as f32 / (1 << mantissa_bits) as f32;
    match exponent {
        0 => mantissa * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + mantissa) * 2f32.powi(e - 15),
    }
}

/// The largest error one store of a value of `magnitude` in the floor
/// layer's format can carry, and one store of it in the scene colour's,
/// plus a small absolute floor: the slack comparing a highlight-free texel
/// with a scene colour texel computed from the same f32 value.
pub(super) fn store_slack(renderer: &Renderer, magnitude: f32) -> f32 {
    let format = renderer
        .full()
        .glossy_overlay
        .forward_forms()
        .highlight_free_format;
    let relative = match format {
        // 6 mantissa bits, truncated or rounded, then the scene colour's.
        wgpu::TextureFormat::Rg11b10Ufloat => 2f32.powi(-5),
        _ => 2f32.powi(-10),
    };
    magnitude.abs() * relative + 1.0e-3
}

/// The forward source as the renderer composes it, with world static
/// specular replaced by a constant [`HIGHLIGHT`] times the texel's `_s`
/// intensity, whatever light the face owns: a bright highlight on every
/// face whose material has `_s`, through forward's own tint rule and
/// composition.
fn forward_highlight_source(renderer: &Renderer) -> String {
    let source = if renderer.cube_array_supported {
        super::SHADER_SOURCE.to_owned()
    } else {
        super::strip_point_shadow_cube(super::SHADER_SOURCE)
    };
    let production = wgsl_function(&source, "world_static_specular").to_owned();
    let test = format!(
        "fn world_static_specular(world_pos: vec3<f32>, N: vec3<f32>, V: vec3<f32>, \
         spec_exp: f32, spec_int: f32, owners: vec4<u32>, mask: vec4<f32>, \
         owners_readable: bool, sdf_sel: SdfLightSelection, sdf_factor: vec4<f32>, \
         sdf_force_lit: bool) -> vec3<f32> {{\n    return vec3<f32>({HIGHLIGHT:?}) * spec_int;\n}}"
    );
    assert_eq!(source.matches(&production).count(), 1);
    source.replace(&production, &test)
}

/// Run `frame` with forward's static-diffuse forms (every form a glossy
/// level binds above off) built from [`forward_highlight_source`].
pub(super) fn with_forward_highlight<R>(
    renderer: &mut Renderer,
    frame: impl FnOnce(&mut Renderer) -> R,
) -> R {
    let source = forward_highlight_source(renderer);
    let variant = renderer
        .full()
        .glossy_overlay
        .forward_forms()
        .variant(&renderer.device, &source);
    let production = renderer
        .full_mut()
        .glossy_overlay
        .swap_forward_forms(variant);
    let result = frame(renderer);
    renderer
        .full_mut()
        .glossy_overlay
        .swap_forward_forms(production);
    result
}

/// Run `frame` with the mover's beauty pipelines built from its composed
/// source with the selected-static gate opened, so every runtime light in
/// reach adds the mover's own specular, pixel and smooth alike.
pub(super) fn with_mover_highlight<R>(
    renderer: &mut Renderer,
    frame: impl FnOnce(&mut Renderer) -> R,
) -> R {
    let gate = "i >= kinematic_light_params.scripted_light_count";
    let source = renderer.full().kinematic_brush.composed_source();
    assert_eq!(source.matches(gate).count(), 2, "both loops gate on it");
    let variant = renderer
        .full()
        .kinematic_brush
        .beauty_variant(&renderer.device, &source.replace(gate, "true"));
    let production = renderer
        .full_mut()
        .kinematic_brush
        .swap_beauty_pipelines(variant);
    let result = frame(renderer);
    renderer
        .full_mut()
        .kinematic_brush
        .swap_beauty_pipelines(production);
    result
}

/// The mean of `texels` (an `extent` grid) over the `block`² block at
/// `cell`: what an isotropic box filter makes of it.
pub(super) fn box_mean(texels: &[Vec3], extent: UVec2, cell: UVec2, block: u32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for y in cell.y * block..(cell.y + 1) * block {
        for x in cell.x * block..(cell.x + 1) * block {
            sum += texels[(y.min(extent.y - 1) * extent.x + x.min(extent.x - 1)) as usize];
        }
    }
    sum / (block * block) as f32
}
