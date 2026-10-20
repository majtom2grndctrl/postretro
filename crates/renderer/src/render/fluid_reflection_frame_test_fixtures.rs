//! Fixtures for the adapter-backed screen-space reflection proofs: a pool
//! with opaque quads standing in it, the frame's depth, scene colour and
//! reflection target read back, and each fragment's expectation from the
//! GPU-free CPU references (the march on the frame's own depth, then the
//! layered composite).
//! See: context/lib/rendering_pipeline.md §7.3a

use super::Renderer;
use super::fluid_surface_frame_test::{capture_encode, capture_through};
use glam::{IVec2, Mat4, UVec2, Vec2, Vec3, Vec4};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    ReflectionProbeBinding, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_render_cpu::fluid_surface::FluidSurfaceLook;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_REFLECTION_ALL_TERMS, FluidReflectionFragment, FluidReflectionLights,
    REFLECTION_SCREEN_MISS, reflection_above_horizon, reflection_fragment, reflection_mirror,
};
use postretro_render_cpu::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, FLUID_REFLECTION_STYLE_SMOOTH, FluidTexelAxes, reflection_point,
};
use postretro_render_cpu::fluid_surface::screen_march::{
    ScreenMarch, ScreenMarchDepthBuffer, screen_march,
};
use postretro_render_cpu::sh_compose::f16_bits_to_f32;

/// The proofs' capture extent: large enough that a quarter-extent target
/// still holds a 16 × 16 grid.
pub(super) const FRAME: u32 = 64;

/// A renderer capturing at [`FRAME`]², or `None` without an adapter.
pub(super) fn ssr_renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(FRAME, FRAME) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[FluidSurfaceProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// A camera: its eye and the view-projection the capture draws through.
#[derive(Debug, Clone, Copy)]
pub(super) struct View {
    pub eye: Vec3,
    pub view_proj: Mat4,
}

/// The proofs' camera at `eye` looking at `target`: 60° vertical field of
/// view, square, standard Z from 0.1 m to 100 m.
pub(super) fn view_from(eye: Vec3, target: Vec3) -> View {
    view_through(eye, target, 60.0)
}

/// [`view_from`] with a vertical field of view of `degrees`.
pub(super) fn view_through(eye: Vec3, target: Vec3, degrees: f32) -> View {
    let projection =
        glam::camera::rh::proj::directx::perspective(degrees.to_radians(), 1.0, 0.1, 100.0);
    View {
        eye,
        view_proj: projection * glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y),
    }
}

/// The base view: 2 m above the water, looking down and ahead over the
/// pool, the standing quad and the back wall.
pub(super) fn base_view() -> View {
    view_from(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.5, 5.0))
}

/// The pool's extent, engine metres: its right edge sits on screen, so
/// water ends there in view.
pub(super) const POOL_X: [f32; 2] = [-6.0, 2.3];
pub(super) const POOL_Z: [f32; 2] = [0.5, 8.0];
/// Texels per metre on the pool: pixel style's 1 m cells span many pixels
/// near the camera.
pub(super) const POOL_TEXELS_PER_METRE: f32 = 1.0;

/// A fluid volume of horizontal faces at height `y` over [`POOL_Z`], split
/// along x at `x_cuts` (ascending, ends included), outward +Y, with UVs of
/// [`POOL_TEXELS_PER_METRE`] texels per metre of world x and z.
pub(super) fn pool(source_entity: u32, y: f32, x_cuts: &[f32]) -> EnvironmentVolumeRecord {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, y, z],
        uv: [x * POOL_TEXELS_PER_METRE, z * POOL_TEXELS_PER_METRE],
    };
    let [z0, z1] = POOL_Z;
    EnvironmentVolumeRecord {
        source_entity,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: x_cuts
            .windows(2)
            .map(|span| EnvironmentSurfaceFace {
                texture_index: 0,
                vertices: vec![
                    corner(span[0], z0),
                    corner(span[0], z1),
                    corner(span[1], z1),
                    corner(span[1], z0),
                ],
            })
            .collect(),
    }
}

/// The base pool: one face at height 0 over [`POOL_X`].
pub(super) fn base_pool() -> EnvironmentVolumeRecord {
    pool(0, 0.0, &POOL_X)
}

/// A probe-bound mirror at zero opacity: each water pixel is its reflection
/// alone (F = 1 at reflectivity 50), over nothing.
pub(super) fn mirror() -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_reflectivity: 50.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.0)
    }
}

/// [`mirror`] at `reflectivity`.
pub(super) fn mirror_at(reflectivity: f32) -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_reflectivity: reflectivity,
        ..mirror()
    }
}

/// The probe capture's uniform colour, f16 bits: linear (0, 0.5, 0).
const PROBE_GREEN: [u16; 3] = [0, 0x3800, 0];
const PROBE_TILE: u32 = 8;

/// One probe over a box holding the whole scene, its capture a uniform
/// green with zero distances (box projection, no capture shadow), bound to
/// each (volume, face) of `bindings`.
pub(super) fn green_probe(bindings: &[(u32, u32)]) -> ReflectionProbesSection {
    let green = PROBE_GREEN
        .into_iter()
        .chain([0x3C00])
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<u8>>();
    ReflectionProbesSection {
        tile_dimension: PROBE_TILE,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min: [-10.0; 3],
            box_max: [10.0; 3],
            capture_point: [0.0, 0.002, 4.0],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        }],
        bindings: bindings
            .iter()
            .map(|&(volume, face)| ReflectionProbeBinding {
                volume,
                face,
                probe: 0,
            })
            .collect(),
        tiles: green.repeat((PROBE_TILE * PROBE_TILE) as usize),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: reflection_probe_zero_distances(1, PROBE_TILE),
    }
}

/// An opaque world quad: four corners in order, every vertex at one
/// placeholder-checkerboard UV, so the quad is one flat colour.
#[derive(Debug, Clone, Copy)]
pub(super) struct WorldQuad {
    pub corners: [Vec3; 4],
    pub base_uv: [f32; 2],
}

/// The placeholder checkerboard's first (magenta) square.
pub(super) const MAGENTA_UV: [f32; 2] = [0.06, 0.06];
/// Its second (black) square.
pub(super) const BLACK_UV: [f32; 2] = [0.19, 0.06];

/// An upright quad facing ±z at `z`.
pub(super) fn upright(x: [f32; 2], y: [f32; 2], z: f32, base_uv: [f32; 2]) -> WorldQuad {
    WorldQuad {
        corners: [
            Vec3::new(x[0], y[0], z),
            Vec3::new(x[1], y[0], z),
            Vec3::new(x[1], y[1], z),
            Vec3::new(x[0], y[1], z),
        ],
        base_uv,
    }
}

/// A horizontal quad at height `y`.
pub(super) fn level(x: [f32; 2], z: [f32; 2], y: f32, base_uv: [f32; 2]) -> WorldQuad {
    WorldQuad {
        corners: [
            Vec3::new(x[0], y, z[0]),
            Vec3::new(x[1], y, z[0]),
            Vec3::new(x[1], y, z[1]),
            Vec3::new(x[0], y, z[1]),
        ],
        base_uv,
    }
}

/// The opaque quad standing in the pool: magenta, 1 m tall, 3 m wide at
/// z = 4.
pub(super) fn standing_quad() -> WorldQuad {
    upright([-1.5, 1.5], [0.0, 1.0], 4.0, MAGENTA_UV)
}

/// The black back wall the pool meets at z = 8, tall enough that water
/// behind the standing quad reflects it above the quad's top.
pub(super) fn back_wall() -> WorldQuad {
    upright([-6.0, 6.0], [0.0, 4.0], POOL_Z[1], BLACK_UV)
}

/// Install `quads` as the level's world geometry, each wound both ways so
/// it draws whichever side the forward pass culls, under one BVH leaf in
/// one cell, with the ambient floor lit so the checkerboard reads in
/// colour. Call before the fluid install, which takes the level's lattice.
pub(super) fn install_world_quads(renderer: &mut Renderer, quads: &[WorldQuad]) {
    use postretro_level_loader::CellData;
    use postretro_render_data::geometry::{
        BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex,
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for quad in quads {
        let base = vertices.len() as u32;
        for corner in quad.corners {
            low = low.min(corner);
            high = high.max(corner);
            vertices.push(WorldVertex {
                position: corner.to_array(),
                base_uv: quad.base_uv,
                normal_oct: [32768; 2],
                tangent_packed: [0; 2],
                lightmap_uv: [0; 2],
                lightmap_block: 0,
                animated_block: 0,
            });
        }
        indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|index| base + index));
    }
    let (aabb_min, aabb_max) = ((low - 0.1).to_array(), (high + 0.1).to_array());
    let bvh = BvhTree {
        nodes: vec![
            BvhNode {
                aabb_min,
                skip_index: 2,
                aabb_max,
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            BvhNode {
                aabb_min,
                skip_index: 2,
                aabb_max,
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
            },
        ],
        leaves: vec![BvhLeaf {
            aabb_min,
            aabb_max,
            material_bucket_id: 0,
            index_offset: 0,
            index_count: indices.len() as u32,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }],
        root_node_index: 0,
    };
    let cells = [CellData {
        bounds_min: Vec3::from(aabb_min),
        bounds_max: Vec3::from(aabb_max),
        face_start: 0,
        face_count: quads.len() as u32,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    }];
    renderer.install_level_geometry(
        &super::LevelGeometry {
            vertices: &vertices,
            vertex_spec_owners: &[],
            indices: &indices,
            bvh: &bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: None,
            sh_storage: super::LevelGeometryShStorage::Legacy,
            lightmap: None,
            lightmap_streaming: None,
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
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            cell_draw_index: None,
            kinematic_geometry: None,
            cells: &cells,
            texture_materials: &[],
        },
        Default::default(),
    );
}

/// The base level: the standing quad and the back wall, then the base
/// pool bound to the green probe.
pub(super) fn install_base_level(renderer: &mut Renderer) {
    install_world_quads(renderer, &[standing_quad(), back_wall()]);
    renderer.install_fluid_surfaces(&[base_pool()], Some(&green_probe(&[(0, 0)])));
}

/// One captured frame through `view`, inside a validation error scope: the
/// frame records the reflection and fluid passes (when it has a reflecting
/// draw) without a validation error, which the scope would capture rather
/// than wgpu's default handler panicking.
pub(super) fn capture_checked(renderer: &mut Renderer, view: View) -> Vec<u8> {
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let pixels = capture_through(renderer, view.view_proj, view.eye);
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "the frame raised a validation error: {error:?}"
    );
    pixels
}

/// `texture`'s level 0, `texel_bytes` a texel, read back tight.
fn read_back(
    renderer: &Renderer,
    texture: &wgpu::Texture,
    texel_bytes: u32,
    aspect: wgpu::TextureAspect,
) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let row = width * texel_bytes;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Fluid reflection proof readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Fluid reflection proof readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "fluid reflection proof readback");
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
    let tight = (0..height)
        .flat_map(|y| {
            let start = (y * padded) as usize;
            data[start..start + row as usize].iter().copied()
        })
        .collect();
    drop(data);
    buffer.unmap();
    tight
}

/// An RGBA16F texel grid decoded to linear floats.
fn rgba16f(bytes: &[u8]) -> Vec<Vec4> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|texel| {
            Vec4::from(
                [0, 1, 2, 3]
                    .map(|c| f16_bits_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]))),
            )
        })
        .collect()
}

/// The frame's opaque scene as the reflection pass read it: the scene
/// depth and the HDR scene colour, at [`FRAME`]².
pub(super) struct SceneReadback {
    pub depth: Vec<f32>,
    pub colour: Vec<Vec3>,
}

impl SceneReadback {
    pub fn depth_at(&self, pixel: UVec2) -> f32 {
        self.depth[(pixel.y * FRAME + pixel.x) as usize]
    }

    pub fn colour_at(&self, pixel: UVec2) -> Vec3 {
        self.colour[(pixel.y * FRAME + pixel.x) as usize]
    }
}

/// Read the last frame's scene depth and colour. Read after a frame whose
/// fluid draws added nothing (Off at zero opacity), the colour is the
/// opaque scene the reflection pass samples.
pub(super) fn read_scene(renderer: &Renderer) -> SceneReadback {
    let full = renderer.full();
    let depth_texture = full.depth_view.texture();
    let colour_texture = full.screen_effects.scene_color_view().texture();
    assert_eq!(
        (depth_texture.width(), depth_texture.height()),
        (FRAME, FRAME)
    );
    let depth = read_back(renderer, depth_texture, 4, wgpu::TextureAspect::DepthOnly)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    let colour = rgba16f(&read_back(
        renderer,
        colour_texture,
        8,
        wgpu::TextureAspect::All,
    ))
    .into_iter()
    .map(Vec4::truncate)
    .collect();
    SceneReadback { depth, colour }
}

/// The reflection target as the last frame left it: premultiplied
/// `(c·SSR, c)` per texel.
pub(super) struct TargetReadback {
    pub extent: UVec2,
    pub texels: Vec<Vec4>,
}

impl TargetReadback {
    /// The texel at `texel`, clamped to the edge as the sampler clamps.
    pub fn at(&self, texel: IVec2) -> Vec4 {
        let clamped = texel.clamp(IVec2::ZERO, self.extent.as_ivec2() - 1);
        self.texels[(clamped.y as u32 * self.extent.x + clamped.x as u32) as usize]
    }
}

pub(super) fn read_reflection_target(renderer: &Renderer) -> TargetReadback {
    let texture = renderer
        .full()
        .fluid_surface
        .reflection_bound_view()
        .texture()
        .clone();
    TargetReadback {
        extent: UVec2::new(texture.width(), texture.height()),
        texels: rgba16f(&read_back(renderer, &texture, 8, wgpu::TextureAspect::All)),
    }
}

/// The capture's RGB at (`column`, `row`).
pub(super) fn rgb(pixels: &[u8], pixel: UVec2) -> [u8; 3] {
    let at = ((pixel.y * FRAME + pixel.x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

/// The largest per-channel gap between two captured colours.
pub(super) fn gap(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)
}

/// Every pixel of the capture, row-major.
pub(super) fn pixels() -> impl Iterator<Item = UVec2> {
    (0..FRAME).flat_map(|row| (0..FRAME).map(move |column| UVec2::new(column, row)))
}

/// The world point on the plane y = `height` under screen position `at`,
/// in [`FRAME`] pixels (a pixel's centre is its index plus one half), or
/// `None` where the view ray never meets the plane in front of the eye.
pub(super) fn on_plane(view: View, at: Vec2, height: f32) -> Option<Vec3> {
    let ndc = Vec2::new(
        at.x / FRAME as f32 * 2.0 - 1.0,
        1.0 - at.y / FRAME as f32 * 2.0,
    );
    let inverse = view.view_proj.inverse();
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    let dir = far - near;
    let t = (height - near.y) / dir.y;
    (t > 0.0 && t.is_finite()).then(|| near + dir * t)
}

/// The centre of capture pixel `pixel`.
pub(super) fn centre(pixel: UVec2) -> Vec2 {
    pixel.as_vec2() + 0.5
}

/// `point`'s NDC depth and screen position in [`FRAME`] pixels.
pub(super) fn project(view: View, point: Vec3) -> (Vec2, f32) {
    let clip = view.view_proj * point.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        Vec2::new((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * FRAME as f32,
        ndc.z,
    )
}

/// Whether `point`, on the pool's x and z extent with `margin` metres
/// to spare, lies on the pool.
pub(super) fn on_pool(point: Vec3, margin: f32) -> bool {
    point.x > POOL_X[0] + margin
        && point.x < POOL_X[1] - margin
        && point.z > POOL_Z[0] + margin
        && point.z < POOL_Z[1] - margin
}

/// The pool's water at `pixel`'s centre on the plane y = `height` when the
/// camera sees it there: on the pool by a margin and in front of the
/// opaque scene by a margin, so the GPU's coverage agrees.
pub(super) fn visible_water(
    view: View,
    scene: &SceneReadback,
    pixel: UVec2,
    height: f32,
) -> Option<Vec3> {
    let point = on_plane(view, centre(pixel), height)?;
    if !on_pool(point, 0.05) {
        return None;
    }
    let (_, depth) = project(view, point);
    (depth < scene.depth_at(pixel) - 1.0e-4).then_some(point)
}

/// The march's tuning as the renderer will upload it.
#[derive(Debug, Clone, Copy)]
pub(super) struct MarchTuning {
    pub stride: f32,
    pub budget: u32,
    pub thickness: f32,
}

pub(super) fn march_tuning(renderer: &Renderer) -> MarchTuning {
    let uniform = renderer.full().fluid_surface.reflection_pending_uniform();
    MarchTuning {
        stride: uniform.stride,
        budget: uniform.step_budget,
        thickness: uniform.thickness,
    }
}

/// The CPU references' view of one frame: its camera, opaque scene and
/// march tuning, with the green probe.
pub(super) struct Reference<'a> {
    pub view: View,
    pub scene: &'a SceneReadback,
    pub tuning: MarchTuning,
    pub probes: ReflectionProbesSection,
}

/// A fluid fragment's reflection as the CPU references give it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Expected {
    /// The march from the style's point, on the frame's own depth.
    pub march: ScreenMarch,
    /// The target value that march writes: `(c·SSR, c)`.
    pub screen: Vec4,
    /// The capture's encoding of the fragment over the white clear.
    pub encoded: [u8; 3],
}

impl Reference<'_> {
    pub fn new<'a>(view: View, scene: &'a SceneReadback, tuning: MarchTuning) -> Reference<'a> {
        Reference {
            view,
            scene,
            tuning,
            probes: green_probe(&[]),
        }
    }

    /// The CPU march of `ray` from `origin` across the frame's own depth.
    pub fn march(&self, origin: Vec3, ray: Vec3) -> ScreenMarch {
        screen_march(
            origin,
            ray,
            self.view.view_proj,
            &ScreenMarchDepthBuffer {
                texels: &self.scene.depth,
                dimensions: UVec2::splat(FRAME),
            },
            self.tuning.stride,
            self.tuning.budget,
            self.tuning.thickness,
        )
    }

    /// What the reflection pass writes for `march`: the scene colour at its
    /// stop, clamped to the f16 maximum and premultiplied by its confidence,
    /// or a literal 0 wherever the confidence is not above 0, as
    /// `fs_reflection` writes it.
    pub fn screen(&self, march: ScreenMarch) -> Vec4 {
        let colour = self
            .scene
            .colour_at(march.pixel.as_uvec2().min(UVec2::splat(FRAME - 1)))
            .min(Vec3::splat(65504.0));
        if march.confidence > 0.0 {
            (colour * march.confidence).extend(march.confidence)
        } else {
            Vec4::ZERO
        }
    }

    /// The fragment of the mirror at `world_position` (style `style`, texel
    /// UV `texel_uv`) composited over the white clear with `screen` as its
    /// target sample, as `fs_main_reflecting`'s CPU reference draws it, and
    /// captured.
    pub fn composite(
        &self,
        style: u32,
        world_position: Vec3,
        texel_uv: Vec2,
        screen: Vec4,
    ) -> [u8; 3] {
        let fragment = self.fragment(style, world_position, texel_uv);
        let out = reflection_fragment(
            Vec4::ZERO,
            screen,
            &fragment,
            &self.probes.probes,
            [self.probes.tile_dimension, self.probes.tile_border],
            &FluidReflectionLights::NONE,
            |_, _| probe_green(),
            |_, _| 0.0,
        );
        capture_encode(out.truncate() * out.w + Vec3::ONE * (1.0 - out.w))
    }

    /// The probe term alone at `world_position`: c = 0.
    pub fn probe_only(&self, style: u32, world_position: Vec3, texel_uv: Vec2) -> [u8; 3] {
        self.composite(style, world_position, texel_uv, REFLECTION_SCREEN_MISS)
    }

    fn fragment(
        &self,
        style: u32,
        world_position: Vec3,
        texel_uv: Vec2,
    ) -> FluidReflectionFragment {
        FluidReflectionFragment {
            probe: 0,
            camera: self.view.eye,
            world_position,
            outward: Vec3::Y,
            light_term_mask: FLUID_REFLECTION_ALL_TERMS,
            reflectivity: mirror().surface_reflectivity,
            style,
            texel_uv,
            axes: pool_axes(),
            time: CAPTURE_TIME,
            // Every proof cell spans pixels, below the fade threshold.
            texels_per_pixel: 0.0,
        }
    }

    /// A smooth-style fragment at `world_position`: its march, target value
    /// and captured composite.
    pub fn smooth(&self, world_position: Vec3) -> Expected {
        let view = (self.view.eye - world_position).normalize();
        let ray = reflection_above_horizon(reflection_mirror(view, Vec3::Y));
        let march = self.march(world_position, ray);
        let screen = self.screen(march);
        Expected {
            march,
            screen,
            encoded: self.composite(
                FLUID_REFLECTION_STYLE_SMOOTH,
                world_position,
                Vec2::ZERO,
                screen,
            ),
        }
    }

    /// A pixel-style cell of the base pool at height `height`: the march
    /// from its centre about its tilted normal, its target value and the
    /// captured composite every fragment of the cell draws.
    pub fn pixel_cell(&self, cell: IVec2, height: f32) -> Expected {
        let texel_uv = cell.as_vec2() + 0.5;
        let world = cell_centre(cell, height);
        let probe = &self.probes.probes[0];
        let point = reflection_point(
            FLUID_REFLECTION_STYLE_PIXEL,
            texel_uv,
            world,
            Vec3::Y,
            pool_axes(),
            CAPTURE_TIME,
            0.0,
            Vec3::from(probe.box_min),
            Vec3::from(probe.box_max),
            Vec3::from(probe.capture_point),
        );
        let view = (self.view.eye - point.position).normalize();
        let ray = reflection_above_horizon(reflection_mirror(view, point.normal));
        let march = self.march(point.position, ray);
        let screen = self.screen(march);
        Expected {
            march,
            screen,
            encoded: self.composite(FLUID_REFLECTION_STYLE_PIXEL, world, texel_uv, screen),
        }
    }
}

/// The capture's animation clock (`capture_through`'s time).
const CAPTURE_TIME: f32 = 1.0;

/// The probe capture's linear colour.
pub(super) fn probe_green() -> Vec3 {
    Vec3::from(PROBE_GREEN.map(f16_bits_to_f32))
}

/// The pool's texel axes: metres per texel along world x and z.
pub(super) fn pool_axes() -> FluidTexelAxes {
    FluidTexelAxes {
        u: Vec3::X / POOL_TEXELS_PER_METRE,
        v: Vec3::Z / POOL_TEXELS_PER_METRE,
    }
}

/// The water texel cell a pool point lies in.
pub(super) fn cell_of(point: Vec3) -> IVec2 {
    (Vec2::new(point.x, point.z) * POOL_TEXELS_PER_METRE)
        .floor()
        .as_ivec2()
}

/// How far, in texels, a pool point lies from its cell's nearest edge.
pub(super) fn cell_margin(point: Vec3) -> f32 {
    let uv = Vec2::new(point.x, point.z) * POOL_TEXELS_PER_METRE;
    let inside = uv - uv.floor();
    inside.min(Vec2::ONE - inside).min_element()
}

/// A cell's unpulled world centre on the plane y = `height`.
pub(super) fn cell_centre(cell: IVec2, height: f32) -> Vec3 {
    let uv = (cell.as_vec2() + 0.5) / POOL_TEXELS_PER_METRE;
    Vec3::new(uv.x, height, uv.y)
}

/// The texel of a target `extent` that screen position `at` (in [`FRAME`]
/// pixels) lies in.
pub(super) fn target_texel(at: Vec2, extent: UVec2) -> IVec2 {
    (at * extent.as_vec2() / FRAME as f32).floor().as_ivec2()
}

/// The bilinear footprint a smooth fragment at `pixel` reads from a target
/// of `extent` through the linear-clamp sampler: the four texels and their
/// weights.
pub(super) fn bilinear_footprint(pixel: UVec2, extent: UVec2) -> [(IVec2, f32); 4] {
    let at = centre(pixel) / FRAME as f32 * extent.as_vec2() - 0.5;
    let base = at.floor();
    let f = at - base;
    let base = base.as_ivec2();
    [
        (base, (1.0 - f.x) * (1.0 - f.y)),
        (base + IVec2::X, f.x * (1.0 - f.y)),
        (base + IVec2::Y, (1.0 - f.x) * f.y),
        (base + IVec2::ONE, f.x * f.y),
    ]
}
