//! Builders for the baked fluid lighting frame tests: in-memory SH levels and today's formula.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::{LevelGeometry, LevelGeometryShStorage, Renderer};
use glam::Vec3;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION, MAX_SH_ATLAS_DIMENSION,
    irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::f16_bits_to_f32;
use postretro_render_data::geometry::BvhTree;

/// Probes per affinity brick edge (`SH_AFFINITY_FACTOR`). A grid no wider
/// than this on any axis is one brick, whose L0 stored tiles run in the
/// grid's own x-fastest order.
const ONE_BRICK: u32 = 4;

/// `value` rounded through f16, as the RGBA16F atlas stores it.
pub(super) fn f16_round(value: Vec3) -> Vec3 {
    Vec3::from(
        value
            .to_array()
            .map(|channel| f16_bits_to_f32(f32_to_f16_bits(channel))),
    )
}

fn rgba16f_texel(rgb: Vec3) -> [u8; 8] {
    let mut bytes = [0; 8];
    for (channel, value) in [rgb.x, rgb.y, rgb.z, 1.0].into_iter().enumerate() {
        bytes[2 * channel..2 * channel + 2].copy_from_slice(&f32_to_f16_bits(value).to_le_bytes());
    }
    bytes
}

/// A legacy (id-34) SH volume as prl-build writes its uncompressed debug
/// form: every probe valid, L0, its depth moments far (Chebyshev visibility
/// 1 everywhere), and probe `(x, y, z)`'s whole tile the uniform irradiance
/// `colour([x, y, z])`. One affinity brick, so stored slot = x-fastest rank.
pub(super) fn sh_section(
    origin: [f32; 3],
    cell_size: [f32; 3],
    dimensions: [u32; 3],
    colour: impl Fn([u32; 3]) -> Vec3,
) -> OctahedralShVolumeSection {
    assert!(
        dimensions
            .iter()
            .all(|&axis| (1..=ONE_BRICK).contains(&axis)),
        "one affinity brick: {dimensions:?}"
    );
    let [gx, gy, gz] = dimensions;
    let count = (gx * gy * gz) as usize;
    let tile = DEFAULT_IRRADIANCE_TILE_DIMENSION;
    let layout =
        irradiance_atlas_array_layout([count as u32, 1, 1], tile, MAX_SH_ATLAS_DIMENSION).unwrap();
    let width = layout.atlas_width as usize;
    let layer_texels = width * layout.atlas_height as usize;
    let mut texels = vec![[0u8; 8]; layer_texels * layout.layer_count as usize];
    for z in 0..gz {
        for y in 0..gy {
            for x in 0..gx {
                let rank = (x + y * gx + z * gx * gy) as usize;
                let [layer, tx, ty] = irradiance_array_tile_location(
                    rank,
                    layout.tiles_per_layer,
                    layout.atlas_tiles_per_row,
                );
                let texel = rgba16f_texel(colour([x, y, z]));
                for row in 0..tile as usize {
                    for column in 0..tile as usize {
                        let at = layer as usize * layer_texels
                            + (ty as usize * tile as usize + row) * width
                            + tx as usize * tile as usize
                            + column;
                        texels[at] = texel;
                    }
                }
            }
        }
    }
    OctahedralShVolumeSection {
        grid_origin: origin,
        cell_size,
        grid_dimensions: dimensions,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: tile,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions: [layout.atlas_width, layout.atlas_height],
        layer_count: layout.layer_count,
        tiles_per_layer: layout.tiles_per_layer,
        atlas_tiles_per_row: layout.atlas_tiles_per_row,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: f32_to_f16_bits(100.0),
                mean_sq_distance: f32_to_f16_bits(10_000.0),
                ..Default::default()
            };
            count
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: texels.concat(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

/// A section the renderer must treat as no volume: a zero-dimension grid.
pub(super) fn zero_dimension_sh_section() -> OctahedralShVolumeSection {
    OctahedralShVolumeSection {
        grid_dimensions: [0, 0, 0],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        ..OctahedralShVolumeSection::placeholder()
    }
}

/// Baked direct SH (id 35) over `base`'s stored-tile geometry, every texel
/// the uniform irradiance `colour`.
pub(super) fn direct_section(
    base: &OctahedralShVolumeSection,
    colour: Vec3,
) -> DirectShVolumeSection {
    let texels = (base.atlas_dimensions[0] * base.atlas_dimensions[1] * base.layer_count) as usize;
    DirectShVolumeSection {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_dimensions: base.atlas_dimensions,
        layer_count: base.layer_count,
        tiles_per_layer: base.tiles_per_layer,
        atlas_tiles_per_row: base.atlas_tiles_per_row,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        atlas: rgba16f_texel(colour).repeat(texels),
    }
}

/// A dynamic-tier directional light of `colour` aimed along `aim`.
pub(super) fn dynamic_directional(colour: Vec3, aim: Vec3) -> MapLight {
    MapLight {
        origin: [0.0; 3],
        light_type: LightType::Directional,
        intensity: 1.0,
        color: colour.to_array(),
        falloff_model: FalloffModel::Linear,
        falloff_range: 1.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: aim.normalize().to_array(),
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: Vec::new(),
        cell_index: 0,
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// Install a level with no world geometry: its SH volume, baked direct SH
/// and lights only, as the level path does before fluid install.
pub(super) fn install_level(
    renderer: &mut Renderer,
    sh: Option<&OctahedralShVolumeSection>,
    direct: Option<&DirectShVolumeSection>,
    lights: &[MapLight],
) {
    let bvh = BvhTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    renderer.install_level_geometry(
        &LevelGeometry {
            vertices: &[],
            vertex_spec_owners: &[],
            indices: &[],
            bvh: &bvh,
            lights,
            light_influences: &[],
            sh_volume: sh,
            sh_storage: LevelGeometryShStorage::Legacy,
            lightmap: None,
            lightmap_streaming: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: direct,
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
            cells: &[],
            texture_materials: &[],
        },
        Default::default(),
    );
}

/// The frame scales the renderer applies to baked light.
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameScales {
    pub ambient_floor: f32,
    pub indirect_scale: f32,
    pub direct_scale: f32,
}

impl FrameScales {
    /// The renderer's defaults (`DEFAULT_AMBIENT_FLOOR`, `DEFAULT_INDIRECT_SCALE`,
    /// `DEFAULT_DYNAMIC_DIRECT_SCALE`).
    pub(super) const DEFAULT: Self = Self {
        ambient_floor: super::DEFAULT_AMBIENT_FLOOR,
        indirect_scale: super::DEFAULT_INDIRECT_SCALE,
        direct_scale: super::DEFAULT_DYNAMIC_DIRECT_SCALE,
    };

    pub(super) fn apply(self, renderer: &mut Renderer) {
        renderer.set_ambient_floor(self.ambient_floor);
        renderer.set_indirect_scale(self.indirect_scale);
        renderer.set_dynamic_direct_scale(self.direct_scale);
    }
}

/// A level's light at the water when every probe holds one irradiance: the
/// static SH indirect and baked direct SH fields, and the dynamic light the
/// face receives at full cosine.
#[derive(Debug, Clone, Copy)]
pub(super) struct UniformLight {
    pub indirect: Vec3,
    pub direct: Vec3,
    pub dynamic: Vec3,
}

/// Today's per-fragment fluid lighting for a uniform field: `ambient_floor ·
/// [ambient] + sh_indirect · indirect_scale · [any indirect] + sh_direct ·
/// direct_scale + dynamic · [dynamic direct]`. The compose passes leave
/// `sh_indirect` holding the static base only under its own bit (this level
/// has no animated deltas) and `sh_direct` the baked base only under the
/// static-direct bit (no promotions).
pub(super) fn todays_lighting(
    mask: LightTermMask,
    scales: FrameScales,
    light: UniformLight,
) -> Vec3 {
    let on = |term: LightTermMask| if mask.contains(term) { 1.0f32 } else { 0.0 };
    let any_indirect = on(LightTermMask::INDIRECT_STATIC).max(on(LightTermMask::INDIRECT_ANIMATED));
    Vec3::splat(scales.ambient_floor * on(LightTermMask::AMBIENT_FLOOR))
        + light.indirect * on(LightTermMask::INDIRECT_STATIC) * scales.indirect_scale * any_indirect
        + light.direct * on(LightTermMask::BAKED_DIRECT_STATIC) * scales.direct_scale
        + light.dynamic * on(LightTermMask::DYNAMIC_DIRECT)
}

/// `mask` with `term` cleared.
pub(super) fn without(mask: LightTermMask, term: LightTermMask) -> LightTermMask {
    let mut mask = mask;
    mask.set_enabled(term, false);
    mask
}
