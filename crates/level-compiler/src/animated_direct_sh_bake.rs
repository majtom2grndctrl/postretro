// Sparse direct-SH delta baker for animated baked lights.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::affinity_grid::{
    AFFINITY_FACTOR, AffinityReachInputs, build_csr, csr_entry_cells, decompose_affinity_for_lights,
};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::delta_sh_cache::{DeltaShCacheInputs, DeltaShCacheTally, bake_or_load_delta_subblocks};
use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::MapLight;
use crate::portals::Portal;
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, ShBakeCtx, ShConfig, bake_probe_direct_rgb,
    pack_octahedral_irradiance_tile, probe_grid_layout, vec3_from,
};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR as FORMAT_AFFINITY_FACTOR,
    DEFAULT_DELTA_PROBE_F16_STRIDE as FORMAT_DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};

const TILE_DIMENSION: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
const TILE_BORDER: u32 = DEFAULT_IRRADIANCE_TILE_BORDER;

/// Cache stage id for raw animated-direct `(affinity cell, light)` sub-blocks.
/// It stays distinct from the other delta stages because their dense f16 payload
/// shapes match and could otherwise cross-serve cache entries.
pub(crate) const ANIMATED_DIRECT_DELTA_SH_STAGE_ID: &str = "animated_direct_delta_sh_subblock";

/// Bump when the animated-direct sub-block computation or its key inputs change.
pub(crate) const ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION: u32 = 1;

/// Inputs for the animated direct-SH delta bake. The probe grid comes from the
/// shared SH context, keeping section-45 sub-blocks coincident with base probes.
pub struct AnimatedDirectShBakeInputs<'a, 'b> {
    pub sh_ctx: &'a ShBakeCtx<'b>,
    pub portals: &'a [Portal],
    pub animated_lights: &'a AnimatedBakedLights<'b>,
}

/// Bake sparse, unit-radiance direct transport for every animated baked light.
///
/// Section 45 is independent of the static `DirectShVolume` base: its CSR and
/// descriptor table use `AnimatedBakedLights` order. Author intensity and color
/// are intentionally excluded so the runtime animation descriptor applies them
/// exactly once.
pub fn bake_animated_direct_sh_delta_volumes(
    inputs: &AnimatedDirectShBakeInputs<'_, '_>,
    config: &ShConfig,
) -> Option<AnimatedDirectShDeltaVolumesSection> {
    bake_animated_direct_sh_delta_volumes_controlled(
        inputs,
        config,
        None,
        &BakeControl::unrestricted(),
    )
}

pub fn bake_animated_direct_sh_delta_volumes_controlled(
    inputs: &AnimatedDirectShBakeInputs<'_, '_>,
    config: &ShConfig,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> Option<AnimatedDirectShDeltaVolumesSection> {
    bake_animated_direct_sh_delta_volumes_controlled_with_tally(inputs, config, cache, control).0
}

/// Test-facing cache accounting for animated direct delta entries. This keeps
/// the production stage API section-only while exposing the CSR locality
/// contract to cross-bake tests.
pub(crate) fn bake_animated_direct_sh_delta_volumes_controlled_with_tally(
    inputs: &AnimatedDirectShBakeInputs<'_, '_>,
    config: &ShConfig,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> (
    Option<AnimatedDirectShDeltaVolumesSection>,
    DeltaShCacheTally,
) {
    if inputs.animated_lights.is_empty() || inputs.sh_ctx.geometry.geometry.vertices.is_empty() {
        return (None, DeltaShCacheTally::default());
    }

    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        return (None, DeltaShCacheTally::default());
    }

    let animated_light_count = inputs.animated_lights.len();
    let lights: Vec<&MapLight> = inputs
        .animated_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .collect();
    let geometry_vertices: Vec<[f32; 3]> = inputs
        .sh_ctx
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect();
    let reach = AffinityReachInputs {
        geometry_vertices: &geometry_vertices,
        tree: inputs.sh_ctx.tree,
        exterior_leaves: inputs.sh_ctx.exterior_leaves,
        portals: inputs.portals,
        probe_spacing: config.probe_spacing,
    };
    // Direct reach clips to each light's falloff-sphere AABB, then portal-floods
    // from its source leaf; spotlight cones intentionally do not clip this cull.
    let decomposition = decompose_affinity_for_lights(&reach, &lights);
    let affinity_dims = decomposition.affinity_dims;
    let (affinity_offsets, affinity_lights) = build_csr(
        &decomposition.per_light_cells,
        decomposition.affinity_cell_count(),
    );
    control.publish_total(affinity_lights.len());

    debug_assert_eq!(
        affinity_offsets.len(),
        decomposition.affinity_cell_count() + 1,
        "animated direct CSR needs one trailing offset"
    );
    debug_assert_eq!(
        affinity_offsets.last().copied().unwrap_or_default() as usize,
        affinity_lights.len(),
        "animated direct CSR trailing offset must match entries"
    );
    debug_assert!(
        affinity_lights
            .iter()
            .all(|&index| (index as usize) < animated_light_count),
        "animated direct CSR must stay in AnimatedBakedLights index space"
    );

    let entries = inputs.animated_lights.entries();
    let csr_cells = csr_entry_cells(&affinity_offsets);
    let keyed_lights: Vec<MapLight> = entries
        .iter()
        .map(|entry| unit_radiance_light(entry.light))
        .collect();
    let light_seed_axes: Vec<u64> = entries
        .iter()
        .map(|entry| entry.source_index as u64)
        .collect();
    let valid_probe_masks = cache_valid_probe_masks(&layout, affinity_dims);
    let cached_subblocks = bake_or_load_delta_subblocks(
        &DeltaShCacheInputs {
            stage_id: ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            stage_version: ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
            geometry_hash: crate::sh_group::geometry_content_hash(inputs.sh_ctx.geometry),
            affinity_dims,
            affinity_lights: &affinity_lights,
            csr_entry_cells: &csr_cells,
            valid_probe_masks: &valid_probe_masks,
            probe_spacing: config.probe_spacing,
            light_seed_axes: &light_seed_axes,
            expected_subblock_f16_len: PROBES_PER_CELL * FORMAT_DEFAULT_DELTA_PROBE_F16_STRIDE,
        },
        &keyed_lights,
        cache,
        control,
        |animated_index, cell| {
            let entry = &entries[animated_index as usize];
            bake_direct_subblock(
                inputs,
                &layout,
                entry.light,
                entry.source_index as u64,
                cell,
                affinity_dims,
            )
        },
    );

    debug_assert_eq!(
        cached_subblocks.subblocks.len(),
        affinity_lights.len() * PROBES_PER_CELL * FORMAT_DEFAULT_DELTA_PROBE_F16_STRIDE
    );

    (
        Some(AnimatedDirectShDeltaVolumesSection {
            affinity_factor: FORMAT_AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: TILE_DIMENSION,
            tile_border: TILE_BORDER,
            animation_descriptor_indices: (0..animated_light_count as u32).collect(),
            valid_probe_masks: vec![u64::MAX; decomposition.affinity_cell_count()],
            cell_levels: vec![0u8; decomposition.affinity_cell_count()],
            affinity_offsets,
            affinity_lights,
            delta_subblocks: cached_subblocks.subblocks,
        }),
        cached_subblocks.tally,
    )
}

/// Normalize a delta's single light to unit radiance before folding it into a
/// cache key. Runtime animation descriptors apply authored color and intensity.
fn unit_radiance_light(light: &MapLight) -> MapLight {
    let mut unit_light = light.clone();
    unit_light.intensity = 1.0;
    unit_light.color = [1.0; 3];
    unit_light
}

/// Cache-only per-cell validity masks. This mirrors the valid-probe gate in
/// `bake_direct_subblock`, including probes outside a partial edge cell.
fn cache_valid_probe_masks(layout: &ProbeGridLayout, affinity_dims: [u32; 3]) -> Vec<u64> {
    let nx = layout.dims[0] as usize;
    let ny = layout.dims[1] as usize;
    let cell_count = affinity_dims[0] * affinity_dims[1] * affinity_dims[2];
    (0..cell_count)
        .map(|cell| {
            let cell_x = cell % affinity_dims[0];
            let cell_y = (cell / affinity_dims[0]) % affinity_dims[1];
            let cell_z = cell / (affinity_dims[0] * affinity_dims[1]);
            let mut mask = 0u64;
            for local_z in 0..AFFINITY_FACTOR {
                for local_y in 0..AFFINITY_FACTOR {
                    for local_x in 0..AFFINITY_FACTOR {
                        let probe_x = cell_x * AFFINITY_FACTOR + local_x;
                        let probe_y = cell_y * AFFINITY_FACTOR + local_y;
                        let probe_z = cell_z * AFFINITY_FACTOR + local_z;
                        if probe_x >= layout.dims[0]
                            || probe_y >= layout.dims[1]
                            || probe_z >= layout.dims[2]
                        {
                            continue;
                        }
                        let probe_index =
                            probe_z as usize * nx * ny + probe_y as usize * nx + probe_x as usize;
                        if layout.validity[probe_index] != 0 {
                            let local = (local_z * AFFINITY_FACTOR * AFFINITY_FACTOR
                                + local_y * AFFINITY_FACTOR
                                + local_x) as u64;
                            mask |= 1u64 << local;
                        }
                    }
                }
            }
            mask
        })
        .collect()
}

fn bake_direct_subblock(
    inputs: &AnimatedDirectShBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    light: &MapLight,
    light_global_index: u64,
    affinity_cell: u32,
    affinity_dims: [u32; 3],
) -> Vec<u16> {
    let cell_x = affinity_cell % affinity_dims[0];
    let cell_y = (affinity_cell / affinity_dims[0]) % affinity_dims[1];
    let cell_z = affinity_cell / (affinity_dims[0] * affinity_dims[1]);
    let nx = layout.dims[0] as usize;
    let ny = layout.dims[1] as usize;
    let ray_ctx = RaytracingCtx {
        bvh: inputs.sh_ctx.bvh,
        primitives: inputs.sh_ctx.primitives,
        geometry: inputs.sh_ctx.geometry,
    };

    let mut unit_light = light.clone();
    unit_light.intensity = 1.0;
    unit_light.color = [1.0; 3];

    let mut out = Vec::with_capacity(PROBES_PER_CELL * FORMAT_DEFAULT_DELTA_PROBE_F16_STRIDE);
    for local_z in 0..AFFINITY_FACTOR {
        for local_y in 0..AFFINITY_FACTOR {
            for local_x in 0..AFFINITY_FACTOR {
                let probe_x = cell_x * AFFINITY_FACTOR + local_x;
                let probe_y = cell_y * AFFINITY_FACTOR + local_y;
                let probe_z = cell_z * AFFINITY_FACTOR + local_z;
                let tile = if probe_x < layout.dims[0]
                    && probe_y < layout.dims[1]
                    && probe_z < layout.dims[2]
                {
                    let probe_index =
                        probe_z as usize * nx * ny + probe_y as usize * nx + probe_x as usize;
                    if layout.validity[probe_index] != 0 {
                        let coefficients = bake_probe_direct_rgb(
                            &ray_ctx,
                            vec3_from(layout.probe_positions[probe_index]),
                            &[&unit_light],
                            &[light_global_index],
                            probe_index as u64,
                        );
                        pack_octahedral_irradiance_tile(
                            &coefficients,
                            true,
                            TILE_DIMENSION,
                            TILE_BORDER,
                        )
                    } else {
                        pack_octahedral_irradiance_tile(
                            &[0.0; 27],
                            false,
                            TILE_DIMENSION,
                            TILE_BORDER,
                        )
                    }
                } else {
                    pack_octahedral_irradiance_tile(&[0.0; 27], false, TILE_DIMENSION, TILE_BORDER)
                };
                for texel in tile {
                    out.extend_from_slice(&texel.rgba);
                }
            }
        }
    }
    out
}

pub fn log_stats(section: &AnimatedDirectShDeltaVolumesSection) {
    let emitted_probe_count = section
        .expected_delta_subblock_f16_count()
        .map(|halves| halves / section.delta_probe_f16_stride())
        .expect("compiler-owned animated-direct section must have a representable payload size");
    log::info!(
        "[Compiler] AnimatedDirectShDeltaVolumes: {} animated light(s), {} CSR entries, {} emitted probes, affinity_dims {}x{}x{}",
        section.animation_descriptor_indices.len(),
        section.affinity_lights.len(),
        emitted_probe_count,
        section.affinity_dims[0],
        section.affinity_dims[1],
        section.affinity_dims[2],
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::light_membership::{
        LightMembershipManifest, LightMembershipRecord,
    };
    use postretro_level_format::texture_names::TextureNamesSection;

    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::entity_shadow_select::{EntityShadowSelectionInputs, select_entity_shadow_lights};
    use crate::geometry::{FaceIndexRange, GeometryResult};
    use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
    use crate::map_data::{
        EntityShadowParams, FalloffModel, LightAnimation, LightType, ShadowType,
    };
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};
    use crate::script_light_membership;

    fn vertex(position: [f32; 3]) -> Vertex {
        Vertex::new(
            position,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn cube_geometry() -> GeometryResult {
        let s = 2.0;
        let positions = [
            [-s, -s, -s],
            [s, -s, -s],
            [s, s, -s],
            [-s, s, -s],
            [-s, -s, s],
            [s, -s, s],
            [s, s, s],
            [-s, s, s],
        ];
        GeometryResult {
            geometry: GeometrySection {
                vertices: positions.into_iter().map(vertex).collect(),
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn empty_tree() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-100.0),
                    max: DVec3::splat(100.0),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            }],
        }
    }

    fn animated_light(origin: DVec3) -> MapLight {
        MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 4.0,
            color: [0.2, 0.6, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 8.0,
            light_size: 0.5,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: Vec::new(),
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn bake(lights: &[MapLight]) -> AnimatedDirectShDeltaVolumesSection {
        let geometry = cube_geometry();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("test geometry must build a BVH");
        let tree = empty_tree();
        let exterior = HashSet::new();
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = AnimatedDirectShBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            animated_lights: &animated_lights,
        };
        bake_animated_direct_sh_delta_volumes(&inputs, &ShConfig { probe_spacing: 1.0 })
            .expect("animated light and geometry must emit section 45")
    }

    fn subblock_for(
        section: &AnimatedDirectShDeltaVolumesSection,
        affinity_cell: usize,
        light_index: u32,
    ) -> Option<&[u16]> {
        let entry = (section.affinity_offsets[affinity_cell] as usize
            ..section.affinity_offsets[affinity_cell + 1] as usize)
            .find(|&index| section.affinity_lights[index] == light_index)?;
        let stride = PROBES_PER_CELL * section.delta_probe_f16_stride();
        Some(&section.delta_subblocks[entry * stride..(entry + 1) * stride])
    }

    fn section_bytes(section: &AnimatedDirectShDeltaVolumesSection) -> Vec<u8> {
        let bytes = section
            .try_to_bytes()
            .expect("animated-direct bake must satisfy its wire contract");
        AnimatedDirectShDeltaVolumesSection::from_bytes(&bytes)
            .expect("animated-direct bake bytes must decode")
            .try_to_bytes()
            .expect("decoded animated-direct section must stay encodable")
    }

    #[test]
    fn animated_direct_bake_is_deterministic_with_seeded_soft_visibility() {
        let lights = vec![animated_light(DVec3::new(0.0, 1.0, 0.0))];

        let first = section_bytes(&bake(&lights));
        let second = section_bytes(&bake(&lights));

        assert_eq!(
            first, second,
            "the per-light global seed must keep soft-visibility direct tiles deterministic"
        );
    }

    #[test]
    fn animated_direct_cache_key_uses_unit_radiance_light_and_stage_version() {
        let mut authored = animated_light(DVec3::ZERO);
        authored.color = [0.25, 0.5, 0.75];
        authored.intensity = 7.0;
        let key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            42,
            &unit_radiance_light(&authored),
        );

        let mut recolored = authored.clone();
        recolored.color = [0.1, 0.2, 0.3];
        recolored.intensity = 0.1;
        let recolored_key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            42,
            &unit_radiance_light(&recolored),
        );
        let bumped_version_key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION + 1,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            42,
            &unit_radiance_light(&authored),
        );
        let different_source_key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            43,
            &unit_radiance_light(&authored),
        );

        assert_ne!(
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            crate::delta_sh_bake::INDIRECT_DELTA_SH_STAGE_ID,
            "animated-direct entries must not share the indirect-delta namespace"
        );
        assert_eq!(key.as_filename(), recolored_key.as_filename());
        assert_ne!(key.as_filename(), bumped_version_key.as_filename());
        assert_ne!(key.as_filename(), different_source_key.as_filename());
    }

    #[test]
    fn animated_direct_subblocks_are_separable_per_light() {
        let first_light = animated_light(DVec3::new(-0.5, 1.0, 0.0));
        let both = bake(&[
            first_light.clone(),
            animated_light(DVec3::new(0.75, 1.0, 0.0)),
        ]);
        let alone = bake(&[first_light]);

        assert_eq!(both.animation_descriptor_indices, vec![0, 1]);
        let common_cell = (0..alone.affinity_cell_count())
            .find(|&cell| {
                subblock_for(&alone, cell, 0).is_some() && subblock_for(&both, cell, 0).is_some()
            })
            .expect("the first light must retain at least one directly reaching cell");
        assert_eq!(
            subblock_for(&both, common_cell, 0),
            subblock_for(&alone, common_cell, 0),
            "a light's unit-radiance sub-block cannot depend on a sibling animated light"
        );
    }

    #[test]
    fn script_animated_light_routes_only_to_animated_direct_delta() {
        let script_animated_light = {
            let mut light = animated_light(DVec3::new(0.0, 1.0, 0.0));
            light.animation = None;
            light
        };
        let mut static_light = animated_light(DVec3::new(1.0, 1.0, 0.0));
        static_light.animation = None;
        let mut lights = vec![script_animated_light, static_light];
        let manifest = LightMembershipManifest::new(
            vec![LightMembershipRecord {
                index: 0,
                is_dynamic: false,
                start_active: Some(true),
                start_active_conflict: false,
            }],
            Vec::new(),
        );
        script_light_membership::apply_manifest(&mut lights, &[true, true], &manifest)
            .expect("script membership must reserve the baked animation slot");

        let direct_delta = bake(&lights);
        assert!(
            !direct_delta.affinity_lights.is_empty(),
            "script-animated baked light must produce section-45 transport"
        );

        let geometry = cube_geometry();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("test geometry must build a BVH");
        let static_lights = StaticBakedLights::from_lights(&lights);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        assert_eq!(
            static_lights
                .entries()
                .iter()
                .map(|entry| entry.source_index)
                .collect::<Vec<_>>(),
            vec![1],
            "script-animated light must be absent from DirectShVolume's static base namespace"
        );
        let selected = select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            static_lights: &static_lights,
            alpha_lights: &alpha_lights,
            params: EntityShadowParams::default(),
        });
        assert_eq!(
            selected.light_indices,
            vec![1],
            "the promotable static light must select in AlphaLights slot 1"
        );

        // StaticBakedLights normally filters the script placeholder first. Keep
        // `is_animated` while removing that placeholder so this assertion drives
        // the selector's independent no-double-count exclusion.
        let mut selector_lights = lights.clone();
        selector_lights[0].animation = None;
        selector_lights[0].is_animated = true;
        let selector_static_lights = StaticBakedLights::from_lights(&selector_lights);
        let selector_alpha_lights = AlphaLightsNs::from_lights(&selector_lights);
        let selected = select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            static_lights: &selector_static_lights,
            alpha_lights: &selector_alpha_lights,
            params: EntityShadowParams::default(),
        });
        assert_eq!(
            selected.light_indices,
            vec![1],
            "the animated light must not enter EntityShadowLights promotion"
        );
    }

    #[test]
    fn direction_animation_bakes_finite_rest_direction_delta() {
        let mut light = animated_light(DVec3::new(0.0, 1.0, 0.0));
        light.light_type = LightType::Spot;
        light.cone_angle_inner = Some(0.35);
        light.cone_angle_outer = Some(0.6);
        light.cone_direction = Some([0.0, -1.0, 0.0]);
        light
            .animation
            .as_mut()
            .expect("fixture is animated")
            .direction = Some(vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);

        let section = bake(&[light]);
        let decoded = section
            .delta_subblocks
            .iter()
            .map(|&bits| crate::sh_bake::f16_bits_to_f32(bits))
            .collect::<Vec<_>>();
        assert!(decoded.iter().all(|value| value.is_finite()));
        assert!(
            decoded
                .chunks_exact(4)
                .any(|rgba| rgba[..3].iter().any(|&value| value > 0.0)),
            "the authored rest direction must produce direct transport even when a direction curve is present"
        );
    }
}
