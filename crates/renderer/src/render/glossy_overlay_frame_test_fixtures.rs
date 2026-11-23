//! Fixtures for the glossy overlay's GPU gates: a world-less-of-fluid level of
//! three floor faces (a bound glossy face, an unbound glossy face, a
//! non-glossy face), one probe of uniform colour bound by id 54's world list,
//! test-only overlay variants, and the frame's HDR scene colour and D_static
//! read back.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::{UVec2, Vec3};
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    ReflectionProbeLightShare, ReflectionProbeRecord, ReflectionProbeWorldBinding,
    ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_level_loader::CellData;
use postretro_render_cpu::sh_compose::f16_bits_to_f32;
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};

use super::fluid_reflection_frame_test_fixtures::{
    FRAME, MAGENTA_UV, PROBE_TILE, SceneReadback, View, centre, on_plane, read_back, read_scene,
    view_from,
};
use super::fluid_reflection_sums_frame_test_fixtures::frame_at;
use super::{LevelGeometry, LevelGeometryShStorage, Renderer};

/// The probe's uniform capture colour, exact in f16.
pub(super) const PROBE_COLOUR: Vec3 = Vec3::new(0.25, 0.75, 0.125);

/// The floor faces, each `(x extent, z extent)` on the plane y = 0, in face
/// order: 0 a glossy face bound to probe 0, 1 a glossy face no probe binds
/// (a probe dropped at the cap), 2 a face of a non-glossy material.
pub(super) const FACES: [([f32; 2], [f32; 2]); 3] = [
    ([-3.0, 0.0], [1.0, 9.0]),
    ([0.0, 3.0], [1.0, 9.0]),
    ([-3.0, 3.0], [9.0, 16.0]),
];
/// Face `i`'s material bucket: the glossy faces share bucket 0.
const FACE_BUCKETS: [u32; 3] = [0, 0, 1];

/// The +Y unit normal, octahedral.
const UP_OCT: [u16; 2] = [32768, 65535];

/// The proofs' camera: 1.5 m above the floor, looking down and ahead over
/// all three faces.
pub(super) fn floor_view() -> View {
    view_from(Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.0, 0.0, 7.0))
}

/// A renderer at [`super::fluid_reflection_frame_test_fixtures::FRAME`]²
/// with bloom off, so scene colour is forward and the overlay alone.
pub(super) fn renderer() -> Option<Renderer> {
    let mut renderer = super::fluid_reflection_frame_test_fixtures::ssr_renderer()?;
    renderer.full_mut().bloom.set_enabled(false);
    Some(renderer)
}

/// One probe of [`PROBE_COLOUR`] over a box holding the floor, zero
/// distances (box projection, no capture shadow), no stored light; bound to
/// face 0 when `bind_world` holds, else to nothing.
pub(super) fn floor_probe(bind_world: bool) -> ReflectionProbesSection {
    let texel: Vec<u8> = PROBE_COLOUR
        .to_array()
        .map(f32_to_f16_bits)
        .into_iter()
        .chain([0x3C00])
        .flat_map(u16::to_le_bytes)
        .collect();
    ReflectionProbesSection {
        tile_dimension: PROBE_TILE,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        colour_mip_count: 1,
        world_bindings: if bind_world {
            vec![ReflectionProbeWorldBinding { face: 0, probe: 0 }]
        } else {
            Vec::new()
        },
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min: [-20.0, -1.0, -20.0],
            box_max: [20.0, 10.0, 20.0],
            capture_point: [-1.5, 0.002, 5.0],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        }],
        fluid_bindings: Vec::new(),
        tiles: texel.repeat((PROBE_TILE * PROBE_TILE) as usize),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: reflection_probe_zero_distances(1, PROBE_TILE),
    }
}

/// Install the floor level on the main thread with `probes` as its id 54,
/// then its probes (no fluid volume), and mark bucket 0 glossy as a
/// material whose specular slot passes the opt-in would be.
pub(super) fn install_floor(renderer: &mut Renderer, probes: &ReflectionProbesSection) {
    with_floor(Some(probes), |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
    renderer.install_fluid_surfaces(&[], Some(probes));
    let level = renderer.level_mut();
    assert_eq!(level.loaded_textures.len(), 1, "one placeholder material");
    level.loaded_textures[0].reflects = true;
}

/// The floor level, lent to `use_geometry`.
pub(super) fn with_floor<R>(
    probes: Option<&ReflectionProbesSection>,
    use_geometry: impl FnOnce(&LevelGeometry<'_>) -> R,
) -> R {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut leaves = Vec::new();
    for (face, (x, z)) in FACES.into_iter().enumerate() {
        let base = vertices.len() as u32;
        let corners = [
            [x[0], 0.0, z[0]],
            [x[1], 0.0, z[0]],
            [x[1], 0.0, z[1]],
            [x[0], 0.0, z[1]],
        ];
        for position in corners {
            vertices.push(WorldVertex {
                position,
                base_uv: MAGENTA_UV,
                normal_oct: UP_OCT,
                tangent_packed: [0; 2],
                lightmap_uv: [0; 2],
                lightmap_block: 0,
                animated_block: 0,
            });
        }
        let first = indices.len() as u32;
        // Both windings: back-face culling keeps the one facing up.
        indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|index| base + index));
        leaves.push(BvhLeaf {
            aabb_min: [x[0] - 0.1, -0.1, z[0] - 0.1],
            aabb_max: [x[1] + 0.1, 0.1, z[1] + 0.1],
            material_bucket_id: FACE_BUCKETS[face],
            index_offset: first,
            index_count: 12,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        });
    }
    let (aabb_min, aabb_max) = ([-3.1, -0.1, 0.9], [3.1, 0.1, 16.1]);
    let mut nodes = vec![BvhNode {
        aabb_min,
        aabb_max,
        skip_index: leaves.len() as u32 + 1,
        left_child_or_leaf_index: 1,
        flags: 0,
    }];
    nodes.extend(leaves.iter().enumerate().map(|(slot, leaf)| BvhNode {
        aabb_min: leaf.aabb_min,
        aabb_max: leaf.aabb_max,
        skip_index: slot as u32 + 2,
        left_child_or_leaf_index: slot as u32,
        flags: BVH_NODE_FLAG_LEAF,
    }));
    let bvh = BvhTree {
        nodes,
        leaves,
        root_node_index: 0,
    };
    let cells = [CellData {
        bounds_min: Vec3::from(aabb_min),
        bounds_max: Vec3::from(aabb_max),
        face_start: 0,
        face_count: FACES.len() as u32,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    }];
    use_geometry(&LevelGeometry {
        vertices: &vertices,
        vertex_spec_owners: &[],
        vertex_emissive_volumes: &[],
        reflection_probes: probes,
        emissive_volumes: &[],
        area_glow_lights: &[],
        indices: &indices,
        bvh: &bvh,
        lights: &[],
        light_influences: &[],
        sh_volume: None,
        sh_storage: LevelGeometryShStorage::Legacy,
        probe_depth_tiles: None,
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
        mover_probe_channel: None,
        entity_shadow_lights: &[],
        shadowmask_atlas: None,
        sdf_atlas: None,
        lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
        cell_draw_index: None,
        kinematic_geometry: None,
        cells: &cells,
        texture_materials: &[],
    })
}

/// Which face, if any, the camera sees at `pixel`'s centre, by at least
/// `margin` metres inside it, so the GPU's coverage agrees.
pub(super) fn face_at(view: View, pixel: UVec2, margin: f32) -> Option<usize> {
    let point = on_plane(view, centre(pixel), 0.0)?;
    FACES.iter().position(|(x, z)| {
        point.x > x[0] + margin
            && point.x < x[1] - margin
            && point.z > z[0] + margin
            && point.z < z[1] - margin
    })
}

/// Whether `pixel` lies within `margin` metres of any face, or misses the
/// plane: a pixel whose coverage the CPU cannot call either way.
pub(super) fn near_any_face(view: View, pixel: UVec2, margin: f32) -> bool {
    let Some(point) = on_plane(view, centre(pixel), 0.0) else {
        return false;
    };
    FACES.iter().any(|(x, z)| {
        point.x > x[0] - margin
            && point.x < x[1] + margin
            && point.z > z[0] - margin
            && point.z < z[1] + margin
    })
}

/// One frame and what it left: the HDR scene colour and, while allocated,
/// forward's D_static.
pub(super) struct GlossyFrame {
    pub scene: SceneReadback,
    pub static_diffuse: Option<Vec<Vec3>>,
}

impl GlossyFrame {
    pub fn colour(&self, pixel: UVec2) -> Vec3 {
        self.scene.colour_at(pixel)
    }

    pub fn static_diffuse(&self, pixel: UVec2) -> Vec3 {
        self.static_diffuse.as_ref().expect("D_static is allocated")
            [(pixel.y * FRAME + pixel.x) as usize]
    }
}

/// Capture one frame through `view` at time 0 and read it back.
pub(super) fn glossy_frame(renderer: &mut Renderer, view: View) -> GlossyFrame {
    frame_at(renderer, view, 0.0);
    GlossyFrame {
        scene: read_scene(renderer),
        static_diffuse: read_static_diffuse(renderer),
    }
}

/// The active level's D_static target, decoded, or `None` when it is not
/// allocated.
pub(super) fn read_static_diffuse(renderer: &Renderer) -> Option<Vec<Vec3>> {
    let texture = renderer
        .level()
        .glossy
        .as_ref()?
        .static_diffuse_texture()?
        .clone();
    Some(match texture.format() {
        wgpu::TextureFormat::Rg11b10Ufloat => {
            read_back(renderer, &texture, 4, wgpu::TextureAspect::All)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| rg11b10_to_vec3(u32::from_le_bytes(*bytes)))
                .collect()
        }
        wgpu::TextureFormat::Rgba16Float => {
            read_back(renderer, &texture, 8, wgpu::TextureAspect::All)
                .as_chunks::<8>()
                .0
                .iter()
                .map(|texel| {
                    Vec3::from([0, 1, 2].map(|c| {
                        f16_bits_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]))
                    }))
                })
                .collect()
        }
        format => panic!("unexpected D_static format {format:?}"),
    })
}

/// One packed `Rg11b10Ufloat` texel: R and G 6-bit mantissas, B 5-bit, each
/// with a 5-bit exponent of bias 15.
fn rg11b10_to_vec3(packed: u32) -> Vec3 {
    fn small_float(bits: u32, mantissa_bits: u32) -> f32 {
        let mantissa = bits & ((1 << mantissa_bits) - 1);
        let exponent = bits >> mantissa_bits;
        let scale = (1u32 << mantissa_bits) as f32;
        match exponent {
            0 => mantissa as f32 / scale * 2f32.powi(-14),
            31 => f32::INFINITY,
            e => (1.0 + mantissa as f32 / scale) * 2f32.powi(e as i32 - 15),
        }
    }
    Vec3::new(
        small_float(packed & 0x7FF, 6),
        small_float((packed >> 11) & 0x7FF, 6),
        small_float(packed >> 22, 5),
    )
}

/// One f16 step at `value`'s magnitude: what one rounding of a sum to the
/// HDR scene colour can move it.
pub(super) fn half_step(value: f32) -> f32 {
    let magnitude = value.abs().max(2f32.powi(-14));
    2f32.powi(magnitude.log2().floor() as i32 - 10)
}

/// The overlay variants the gates swap in for the production fragment.
pub(super) const VARIANTS: &str = r#"
// Adds nothing: forward's colour, with forward's D_static written beside it.
@fragment
fn fs_test_zero(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

// A constant per fragment, whatever the gate: counts overlay fragments.
@fragment
fn fs_test_constant(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(0.125, 0.125, 0.125, 0.0);
}

// R forced to forward's own static diffuse read back this frame.
@fragment
fn fs_test_reflection_is_static_diffuse(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let term = glossy_overlay_term(in);
    let d_static = glossy_overlay_static_diffuse(in.clip_position.xy);
    let reflection = glossy_overlay_static_diffuse(in.clip_position.xy);
    return vec4<f32>(term.weight * (reflection - d_static), 0.0);
}

// As above, with the subtracted static diffuse deliberately perturbed.
@fragment
fn fs_test_perturbed_static_diffuse(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let term = glossy_overlay_term(in);
    let d_static = glossy_overlay_static_diffuse(in.clip_position.xy) * 0.5;
    let reflection = glossy_overlay_static_diffuse(in.clip_position.xy);
    return vec4<f32>(term.weight * (reflection - d_static), 0.0);
}
"#;

/// Run `frame` with the overlay's fragment `entry` from [`VARIANTS`] in the
/// production pipeline's place, restored after.
pub(super) fn with_variant<R>(
    renderer: &mut Renderer,
    entry: &str,
    frame: impl FnOnce(&mut Renderer) -> R,
) -> R {
    let variant =
        renderer
            .full()
            .glossy_overlay
            .variant_pipeline(&renderer.device, VARIANTS, entry);
    let production = renderer.full_mut().glossy_overlay.swap_pipeline(variant);
    let result = frame(renderer);
    renderer.full_mut().glossy_overlay.swap_pipeline(production);
    result
}

/// Overlay passes recorded so far.
pub(super) fn overlays(renderer: &Renderer) -> u32 {
    renderer.full().glossy_overlay.recorded()
}
