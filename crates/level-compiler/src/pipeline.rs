//! Owns ordered level-compilation stage orchestration.
//! Governing contracts: `context/lib/build_pipeline.md`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::bake_control::BakeControl;
use crate::governor::Governor;
use crate::reporter::{Reporter, StageProgress};
use crate::{
    Args, bake_model_textures, bake_sprite_textures, compile_worldspawn_data_script,
    map_needs_sdf_atlas, resolve_content_root, resolve_lightmap_density,
    resolve_prm_root_via_cargo, resolve_texture_root,
};
use crate::{
    animated_direct_sh_bake, animated_light_chunks, animated_light_weight_maps,
    billboard_direct_scatter_bake, bvh_build, cache, cell_draw_index_bake, cell_visibility_bake,
    chunk_light_list_bake, delta_sections, delta_sh_bake, direct_sh_bake, entity_shadow_select,
    fog_cell_masks, geometry, kinematic_geometry, light_namespaces, lightmap_bake, lightmap_layer,
    map_data, navmesh_bake, pack, parse, partition, portals, sdf_bake, sh_analyze, sh_bake,
    sh_coarsen, sh_group, shadowmask_bake, texture_mips, texture_validation, trigger_volumes,
    visibility,
};

fn begin_stage(reporter: &dyn Reporter, id: StageId) -> Instant {
    reporter.begin_stage(id);
    Instant::now()
}

fn finish_stage(
    timings: &mut Vec<(&'static str, Duration)>,
    reporter: &dyn Reporter,
    id: StageId,
    started: Instant,
    produced_output: bool,
) {
    timings.push((id.label(), started.elapsed()));
    if produced_output {
        reporter.finish_stage(id);
    } else {
        reporter.skip_stage(id);
    }
}

/// Stable identity for one ordered compiler stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageId {
    Parsing,
    DataScript,
    TextureValidation,
    Partitioning,
    Visibility,
    Geometry,
    BvhBuild,
    CellVisibility,
    NavMesh,
    LightmapBake,
    ShBake,
    DeltaShBake,
    DirectShBake,
    AnimatedDirectShBake,
    EntityShadowLights,
    DirectShDeltaBake,
    BillboardDirectScatterBake,
    ShadowmaskAtlas,
    ChunkLightList,
    AnimatedLightChunks,
    AnimatedWeightMaps,
    SdfAtlasBake,
    TextureMips,
    Packing,
}

/// A stage's stable identity, Build Summary label, and predicted presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageDescriptor {
    pub id: StageId,
    pub label: &'static str,
    pub predicted_present: bool,
}

impl StageId {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Parsing => "Parsing",
            Self::DataScript => "DataScript",
            Self::TextureValidation => "TexValidation",
            Self::Partitioning => "Partitioning",
            Self::Visibility => "Visibility",
            Self::Geometry => "Geometry",
            Self::BvhBuild => "BVH Build",
            Self::CellVisibility => "Cell Visibility",
            Self::NavMesh => "NavMesh",
            Self::LightmapBake => "Lightmap Bake",
            Self::ShBake => "SH Bake",
            Self::DeltaShBake => "Delta SH Bake",
            Self::DirectShBake => "Direct SH Bake",
            Self::AnimatedDirectShBake => "Animated Direct SH Bake",
            Self::EntityShadowLights => "EntityShadowLights",
            Self::DirectShDeltaBake => "Direct SH Delta Bake",
            Self::BillboardDirectScatterBake => "Billboard Direct Scatter Bake",
            Self::ShadowmaskAtlas => "ShadowmaskAtlas",
            Self::ChunkLightList => "ChunkLightList",
            Self::AnimatedLightChunks => "AnimLightChunks",
            Self::AnimatedWeightMaps => "AnimWeightMaps",
            Self::SdfAtlasBake => "SDF Atlas Bake",
            Self::TextureMips => "TextureMips",
            Self::Packing => "Packing",
        }
    }

    pub const fn progress_label(self) -> &'static str {
        match self {
            Self::Parsing => "Parsing map...",
            Self::DataScript => "Data script compilation...",
            Self::TextureValidation => "Texture color-space validation...",
            Self::Partitioning => "BSP partitioning...",
            Self::Visibility => "Visibility computation...",
            Self::Geometry => "Geometry extraction...",
            Self::BvhBuild => "BVH build...",
            Self::CellVisibility => "Cell visibility bake...",
            Self::NavMesh => "NavMesh bake...",
            Self::LightmapBake => "Lightmap bake...",
            Self::ShBake => "SH volume bake...",
            Self::DeltaShBake => "Delta SH volume bake...",
            Self::DirectShBake => "Direct SH volume bake...",
            Self::AnimatedDirectShBake => "Animated direct SH delta bake...",
            Self::EntityShadowLights => "Entity shadow light selection...",
            Self::DirectShDeltaBake => "Direct SH delta volume bake...",
            Self::BillboardDirectScatterBake => "Billboard direct scatter bake...",
            Self::ShadowmaskAtlas => "Shadowmask atlas bake...",
            Self::ChunkLightList => "Chunk light list bake...",
            Self::AnimatedLightChunks => "Animated light chunks...",
            Self::AnimatedWeightMaps => "Animated light weight maps...",
            Self::SdfAtlasBake => "SDF atlas bake...",
            Self::TextureMips => "Texture mip bake...",
            Self::Packing => "Packing and writing...",
        }
    }
}

pub(crate) const ORDERED_STAGES: [StageId; 24] = [
    StageId::Parsing,
    StageId::DataScript,
    StageId::TextureValidation,
    StageId::Partitioning,
    StageId::Visibility,
    StageId::Geometry,
    StageId::BvhBuild,
    StageId::CellVisibility,
    StageId::NavMesh,
    StageId::LightmapBake,
    StageId::ShBake,
    StageId::DeltaShBake,
    StageId::DirectShBake,
    StageId::AnimatedDirectShBake,
    StageId::EntityShadowLights,
    StageId::DirectShDeltaBake,
    StageId::BillboardDirectScatterBake,
    StageId::ShadowmaskAtlas,
    StageId::ChunkLightList,
    StageId::AnimatedLightChunks,
    StageId::AnimatedWeightMaps,
    StageId::SdfAtlasBake,
    StageId::TextureMips,
    StageId::Packing,
];

/// Return the ordered stage descriptors predicted for parsed map content.
///
/// Prediction is side-effect free and can run before the bake worker starts.
/// SDF presence intentionally uses the same content predicate as execution.
pub fn planned_stages(lights: &[map_data::MapLight]) -> Vec<StageDescriptor> {
    planned_stages_for_sdf(map_needs_sdf_atlas(lights))
}

/// A map parsed once on the main thread so the TUI can derive its planned
/// content-dependent stage list before starting the bake worker.
pub(crate) struct PreparedMap {
    pub(crate) map_data: map_data::MapData,
    parsing_elapsed: Duration,
}

pub(crate) fn prepare(args: &Args) -> anyhow::Result<PreparedMap> {
    let started = Instant::now();
    let map_data = parse::parse_map_file(&args.input, args.format)?;
    Ok(PreparedMap {
        map_data,
        parsing_elapsed: started.elapsed(),
    })
}

fn planned_stages_for_sdf(needs_sdf: bool) -> Vec<StageDescriptor> {
    ORDERED_STAGES
        .iter()
        .copied()
        .map(|id| StageDescriptor {
            id,
            label: id.label(),
            predicted_present: id != StageId::SdfAtlasBake || needs_sdf,
        })
        .collect()
}

/// Execute the compiler stages in their stable order and print the Build Summary.
pub(crate) fn run(
    args: &Args,
    stage_cache: Option<cache::StageCache>,
    started: Instant,
    reporter: Arc<dyn Reporter>,
    governor: Arc<Governor>,
) -> anyhow::Result<()> {
    let stage_start = begin_stage(reporter.as_ref(), StageId::Parsing);
    let map_data = parse::parse_map_file(&args.input, args.format)?;
    let parsing_elapsed = stage_start.elapsed();
    run_after_parsing(
        args,
        stage_cache,
        started,
        reporter,
        governor,
        map_data,
        parsing_elapsed,
    )
}

pub(crate) fn run_prepared(
    args: &Args,
    stage_cache: Option<cache::StageCache>,
    started: Instant,
    reporter: Arc<dyn Reporter>,
    governor: Arc<Governor>,
    prepared: PreparedMap,
) -> anyhow::Result<()> {
    reporter.begin_stage(StageId::Parsing);
    run_after_parsing(
        args,
        stage_cache,
        started,
        reporter,
        governor,
        prepared.map_data,
        prepared.parsing_elapsed,
    )
}

fn run_after_parsing(
    args: &Args,
    stage_cache: Option<cache::StageCache>,
    started: Instant,
    reporter: Arc<dyn Reporter>,
    governor: Arc<Governor>,
    mut map_data: map_data::MapData,
    parsing_elapsed: Duration,
) -> anyhow::Result<()> {
    let mut timings = Vec::new();
    timings.push((StageId::Parsing.label(), parsing_elapsed));
    reporter.finish_stage(StageId::Parsing);
    let sh_coarsening_enabled = !map_data.uniform_grid_optout;
    if !sh_coarsening_enabled {
        log::info!(
            "[sh-coarsen] disabled by worldspawn `_sh_coarsen \"0\"`; id 41 remains uniform L0"
        );
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::DataScript);
    let compiled_data_script = compile_worldspawn_data_script(
        &args.input,
        map_data.data_script.as_deref(),
        &map_data.lights,
    )?;
    let (data_script_section, membership_manifest) = match compiled_data_script {
        Some(script) => (Some(script.section), Some(script.membership_manifest)),
        None => (None, None),
    };
    if let Some(membership_manifest) = membership_manifest.as_ref() {
        let inventory = crate::script_light_membership::apply_manifest(
            &mut map_data.lights,
            &map_data.light_start_active_defaults,
            membership_manifest,
        )?;
        crate::script_light_membership::log_inventory(&inventory, &map_data.lights);
    }
    // Every cached bake stage keys from the post-injection light namespaces or
    // their `MapLight` records, so a manifest membership change produces a
    // different cache key without a cache-epoch bump. The compiled script bytes
    // are packed uncached into DataScript and do not affect bake output unless
    // their evaluated manifest changes membership.
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::DataScript,
        stage_start,
        data_script_section.is_some(),
    );

    let stage_start = begin_stage(reporter.as_ref(), StageId::TextureValidation);
    let texture_root = resolve_texture_root(&args.input);
    texture_validation::validate_sibling_color_spaces(&texture_root)?;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::TextureValidation,
        stage_start,
        true,
    );

    let static_baked_lights = light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
    let animated_baked_lights =
        light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
    let alpha_lights_ns = light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);

    if static_baked_lights.is_empty() {
        log::warn!(
            "[Compiler] No non-animated baked lights: static world geometry will use a white \
             lightmap placeholder. Add at least one static baked light so world lightmaps render \
             correctly. The build will continue."
        );
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::Partitioning);
    let result = partition::partition(&map_data.brush_volumes)?;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::Partitioning,
        stage_start,
        true,
    );
    if args.verbose {
        partition::log_stats(&result.tree, &result.faces);
    }

    // Watertightness diagnostic. Surfaces holes in the static world geometry
    // (faces the clipper dropped, or T-junction cracks) so they can be fixed.
    // A warning, never a build failure: a compiler bug must not block a level
    // designer, only become visible. Runs on the pre-exterior-cull face set.
    let watertight = partition::check_watertight(&result.faces);
    if watertight.open_edge_count > 0 {
        log::warn!(
            "[Compiler] Watertightness: {} open edge(s) in world geometry — possible holes \
             you can see through. Diagnostic only; the build still succeeds. Sample locations \
             (world-space meters, showing up to {}):",
            watertight.open_edge_count,
            watertight.samples.len(),
        );
        for edge in &watertight.samples {
            log::warn!(
                "[Compiler]   open edge near ({:.3}, {:.3}, {:.3}) — near brush {}",
                edge.midpoint.x,
                edge.midpoint.y,
                edge.midpoint.z,
                edge.brush_index,
            );
        }
    } else if args.verbose {
        log::info!("[Compiler] Watertightness: world geometry is closed (0 open edges)");
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::Visibility);
    // The exterior set is used by the BSP/leaf encoder to emit `face_count = 0`
    // for outside-the-map leaves in lockstep with the geometry section.
    let generated_portals = portals::generate_portals(&result.tree);
    let portal_count = generated_portals.len();
    if portal_count == 0 {
        log::warn!(
            "Portal generation produced 0 portals. Vis will treat all leaves as mutually visible."
        );
    }

    let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);

    let vis_result = visibility::encode_vis(&result.tree, &exterior_leaves);
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::Visibility,
        stage_start,
        true,
    );
    if args.verbose {
        visibility::log_stats(&vis_result, portal_count);
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::Geometry);
    let mut geo_result = geometry::extract_geometry(&result.faces, &result.tree, &exterior_leaves);
    let carried_lights_by_mover = kinematic_geometry::member_lights_by_mover(
        &map_data.kinematic_movers,
        &map_data.carried_light_links,
        &alpha_lights_ns,
    );
    let kinematic_geometry_section = kinematic_geometry::encode_kinematic_geometry_section(
        &map_data.kinematic_movers,
        &map_data.kinematic_waypoints,
        &carried_lights_by_mover,
        &generated_portals,
        &mut geo_result.texture_names,
    );
    let trigger_volumes_section =
        trigger_volumes::encode_trigger_volumes_section(&map_data.trigger_volumes);
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::Geometry,
        stage_start,
        true,
    );
    if args.verbose {
        let empty_leaf_count = result
            .tree
            .leaves
            .iter()
            .enumerate()
            .filter(|(idx, l)| !l.is_solid && !exterior_leaves.contains(idx))
            .count();
        geometry::log_stats(&geo_result, empty_leaf_count);
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::BvhBuild);
    let (bvh, bvh_primitives, bvh_section) =
        bvh_build::build_bvh(&geo_result).map_err(|e| anyhow::anyhow!("BVH build failed: {e}"))?;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::BvhBuild,
        stage_start,
        true,
    );
    if args.verbose {
        bvh_build::log_stats(&bvh_section);
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::CellVisibility);
    // This is deliberately an all-cells pass: solid and zero-portal leaves
    // remain valid CellIds as singleton components instead of being elided.
    let cell_visibility_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::CellVisibility, cell_visibility_progress.clone());
    let cell_visibility_control =
        BakeControl::new(Arc::clone(&governor), &cell_visibility_progress);
    let cell_visibility_bytes = cell_visibility_bake::cell_visibility_bake(
        &result.tree,
        &generated_portals,
        &cell_visibility_control,
    )?
    .to_bytes()?;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::CellVisibility,
        stage_start,
        true,
    );

    // Cell draw index (id 37): per-cell BVH-leaf spans for the runtime visible-cell
    // candidate cull. Derived from the already-sorted flat BVH leaves joined into
    // the encoded BSP leaf records (cell_id == BSP leaf index). Uncached — it is a
    // cheap CSR pass over data the (uncached) BVH stage just produced. Omitted for
    // zero-leaf maps; emission is independent of portal presence.
    let cell_draw_index_bytes = cell_draw_index_bake::bake_cell_draw_index(
        &bvh_section.leaves,
        &vis_result.leaves_section.leaves,
    )
    .map(|section| section.to_bytes());

    let stage_start = begin_stage(reporter.as_ref(), StageId::NavMesh);
    // Walkable navigation graph baked from the extracted geometry's triangles
    // (already filtered to empty, non-exterior leaf faces). `None` when no
    // walkable region survives — the section is then omitted and the build still
    // succeeds (SDF-atlas precedent). Cached on blake3(postcard(geo_result) ||
    // postcard(nav params)), mirroring the SDF stage's `SdfInputs` hash.
    let navmesh_section = {
        let nav_input_hash = {
            let mut buf =
                postcard::to_allocvec(&geo_result).expect("postcard serialize geo_result");
            buf.extend_from_slice(
                &postcard::to_allocvec(&map_data.nav_params)
                    .expect("postcard serialize nav params"),
            );
            *blake3::hash(&buf).as_bytes()
        };
        let nav_key = cache::CacheKey::new(
            "navmesh",
            navmesh_bake::NAVMESH_STAGE_VERSION,
            &nav_input_hash,
        );

        // Cache stores the section's `to_bytes()`; an empty payload is the
        // sentinel for a cached "no walkable region" result (no section).
        let cached = stage_cache.as_ref().and_then(|c| c.get(&nav_key));
        match cached {
            Some(bytes) if bytes.is_empty() => {
                log::info!("[cache] navmesh hit (no walkable region)");
                None
            }
            Some(bytes) => {
                match postretro_level_format::navmesh::NavMeshSection::from_bytes(&bytes) {
                    Ok(section) => {
                        log::info!("[cache] navmesh hit");
                        Some(section)
                    }
                    Err(e) => {
                        log::warn!("[cache] corrupt navmesh entry, re-baking: {e}");
                        let section = navmesh_bake::bake_navmesh(&geo_result, &map_data.nav_params);
                        if let Some(ref c) = stage_cache {
                            c.put(
                                &nav_key,
                                &section.as_ref().map(|s| s.to_bytes()).unwrap_or_default(),
                            );
                        }
                        section
                    }
                }
            }
            None => {
                log::info!("[cache] navmesh miss");
                let section = navmesh_bake::bake_navmesh(&geo_result, &map_data.nav_params);
                if let Some(ref c) = stage_cache {
                    c.put(
                        &nav_key,
                        &section.as_ref().map(|s| s.to_bytes()).unwrap_or_default(),
                    );
                }
                section
            }
        }
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::NavMesh,
        stage_start,
        navmesh_section.is_some(),
    );

    let stage_start = begin_stage(reporter.as_ref(), StageId::LightmapBake);
    let lightmap_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::LightmapBake, lightmap_progress.clone());
    let lightmap_control = BakeControl::new(Arc::clone(&governor), &lightmap_progress);
    let static_light_count = map_data.lights.iter().filter(|l| !l.is_dynamic).count();
    let effective_lightmap_density =
        resolve_lightmap_density(args.lightmap_density, map_data.lightmap_density);
    let lightmap_config = lightmap_bake::LightmapConfig {
        lightmap_density: effective_lightmap_density,
        area_sample_count: args.soft_shadow_samples,
        uncompressed_irradiance: args.uncompressed_irradiance,
    };
    let final_lightmap_density;
    let lightmap_bake_output = if let Some(ref cache) = stage_cache {
        // Warm path: two-level lightmap cache. First checks a memoized composited
        // `LightmapSection`; on a hit (no-edit rebuild) it skips the layer reads,
        // composite, dilate, and BC6H encode entirely. On a section-cache miss it
        // falls through to the per-light layer cache — each unchanged light's layer
        // hits, only edited lights re-bake — then composites/dilates/encodes. Either
        // way the composite equals the monolithic `bake_face_chart` output bit-for-bit,
        // so the only difference from the cold path is cache reuse, not different output.
        // The multi-bin packer opens new array layers instead of failing on
        // atlas area, so there is no density-coarsening retry — prepare once at
        // the fixed density.
        let density = lightmap_config.lightmap_density;
        let prepared = lightmap_bake::prepare_atlas(&mut geo_result, &static_baked_lights, density)
            .map_err(|e| anyhow::anyhow!("Lightmap atlas prepare failed: {e}"))?;
        final_lightmap_density = density;

        // Mirror `bake_lightmap`'s placeholder branch: with no static lights or no
        // packed placements there is nothing to composite, so emit a placeholder
        // section while still returning the planned charts/placements for the
        // downstream animated-light passes.
        if static_baked_lights.is_empty() || prepared.placements.is_empty() {
            lightmap_bake::LightmapBakeOutput {
                section: postretro_level_format::lightmap::LightmapSection::placeholder(),
                charts: prepared.charts,
                placements: prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
                layer_count: prepared.layer_count,
            }
        } else {
            let shared = lightmap_layer::SharedAtlas {
                charts: &prepared.charts,
                placements: &prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
            };
            // Direct-lightmap light set: global `static_lights` order with `Sdf`
            // shadow-type lights dropped, exactly as the monolithic `bake_lightmap`
            // does — so the composited layer sum reproduces the cold bake.
            let layer_lights: Vec<&map_data::MapLight> = static_baked_lights
                .entries()
                .iter()
                .map(|e| e.light)
                .filter(|l| l.shadow_type != map_data::ShadowType::Sdf)
                .collect();
            let warm_lightmap_total = prepared.placements.len().saturating_mul(layer_lights.len());
            lightmap_control.publish_total(warm_lightmap_total);

            // Compute every light's layer input hash up front (cheap — no blob
            // reads). These both fold into the second-level section key and feed
            // the per-light layer keys on a section-cache miss.
            let layer_input_hashes: Vec<[u8; 32]> = layer_lights
                .iter()
                .map(|light| {
                    lightmap_layer::layer_input_hash(
                        light,
                        &shared,
                        &bvh_primitives,
                        &geo_result,
                        density,
                        args.soft_shadow_samples,
                    )
                })
                .collect();

            // Second-level cache: memoize the composited `LightmapSection` so a
            // no-edit rebuild does one section decode and skips the layer reads,
            // composite, dilate, and BC6H encode entirely. The section bytes are
            // a pure function of the folded inputs (proven byte-identical by the
            // existing determinism gate), so caching them cannot perturb output.
            let section_input_hash = lightmap_layer::section_input_hash(
                &layer_input_hashes,
                density,
                lightmap_config.uncompressed_irradiance,
            );
            let section_key = cache::CacheKey::new(
                "lightmap_section",
                lightmap_layer::LIGHTMAP_SECTION_VERSION,
                &section_input_hash,
            );

            // A `from_bytes` failure on a present entry is treated as a miss
            // (warn + recompose), mirroring the layer codec's corruption handling.
            let cached_section = cache.get(&section_key).and_then(|bytes| {
                match postretro_level_format::lightmap::LightmapSection::from_bytes(&bytes) {
                    Ok(section) => Some(section),
                    Err(err) => {
                        log::warn!("[Compiler] corrupt lightmap section, recomposing: {err}");
                        None
                    }
                }
            });

            let section = match cached_section {
                Some(section) => {
                    log::info!("[cache] lightmap_section hit");
                    // Serial lightmap work ignores the core throttle, but every
                    // cache unit still honors pause before reporting completion.
                    lightmap_control.governor().checkpoint();
                    lightmap_control.advance(warm_lightmap_total);
                    section
                }
                None => {
                    log::info!("[cache] lightmap_section miss");
                    let mut layers: Vec<lightmap_layer::LightmapLayer> =
                        Vec::with_capacity(layer_lights.len());
                    for (light, input_hash) in layer_lights.iter().zip(&layer_input_hashes) {
                        let layer_key = cache::CacheKey::new(
                            "lightmap_layer",
                            lightmap_layer::LAYER_FORMAT_VERSION,
                            input_hash,
                        );
                        let layer = match cache
                            .get(&layer_key)
                            .and_then(|bytes| lightmap_layer::LightmapLayer::from_bytes(&bytes))
                        {
                            Some(layer) => {
                                log::info!("[cache] lightmap_layer hit");
                                lightmap_control.governor().checkpoint();
                                lightmap_control.advance(prepared.placements.len());
                                layer
                            }
                            None => {
                                log::info!("[cache] lightmap_layer miss");
                                let layer = lightmap_layer::bake_light_layer_controlled(
                                    light,
                                    &shared,
                                    &bvh,
                                    &bvh_primitives,
                                    &geo_result,
                                    args.soft_shadow_samples,
                                    &lightmap_control,
                                );
                                cache.put(&layer_key, &layer.to_bytes());
                                layer
                            }
                        };
                        layers.push(layer);
                    }

                    let mut composite = lightmap_layer::composite_layers(
                        &layers,
                        prepared.atlas_width,
                        prepared.atlas_height,
                    );
                    composite.dilate();
                    let section =
                        composite.encode_section(density, lightmap_config.uncompressed_irradiance);
                    cache.put(&section_key, &section.to_bytes());
                    section
                }
            };

            lightmap_bake::LightmapBakeOutput {
                section,
                charts: prepared.charts,
                placements: prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
                layer_count: prepared.layer_count,
            }
        }
    } else {
        // Cold / exact path (`--no-cache`): the monolithic whole-atlas bake, the
        // shippable source of truth. No layer reads/writes. The multi-bin packer
        // opens new array layers instead of failing on atlas area, so there is no
        // density-coarsening retry — bake once at the fixed density.
        let density = lightmap_config.lightmap_density;
        final_lightmap_density = density;
        let mut lm_ctx = lightmap_bake::LightmapBakeCtx {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &mut geo_result,
            lights: &static_baked_lights,
        };
        lightmap_bake::bake_lightmap_controlled(
            &mut lm_ctx,
            &lightmap_bake::LightmapConfig {
                lightmap_density: density,
                area_sample_count: args.soft_shadow_samples,
                uncompressed_irradiance: args.uncompressed_irradiance,
            },
            &lightmap_control,
        )
        .map_err(|e| anyhow::anyhow!("Lightmap bake failed: {e}"))?
    };
    let lightmap_bake::LightmapBakeOutput {
        section: lightmap_section,
        charts: face_charts,
        placements: face_placements,
        atlas_width,
        atlas_height,
        layer_count: static_atlas_layer_count,
    } = lightmap_bake_output;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::LightmapBake,
        stage_start,
        !static_baked_lights.is_empty() && !face_placements.is_empty(),
    );
    if args.verbose {
        lightmap_bake::log_stats(&lightmap_section, static_light_count);
    }
    let stage_start = begin_stage(reporter.as_ref(), StageId::ShBake);
    let sh_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::ShBake, sh_progress.clone());
    let sh_control = BakeControl::new(Arc::clone(&governor), &sh_progress);
    if let Err(msg) = sh_bake::validate_light_animations(&map_data.lights) {
        anyhow::bail!("light animation validation failed: {msg}");
    }
    let sh_config = sh_bake::ShConfig {
        probe_spacing: args.probe_spacing,
    };
    let sh_ctx = sh_bake::ShBakeCtx {
        bvh: &bvh,
        primitives: &bvh_primitives,
        geometry: &geo_result,
        tree: &result.tree,
        exterior_leaves: &exterior_leaves,
        static_lights: &static_baked_lights,
        animated_lights: &animated_baked_lights,
        total_light_count: map_data.lights.len(),
    };
    let mut sh_volume_section = if let Some(ref cache) = stage_cache {
        // Warm path: per-probe-group SH. Each group bakes/loads a cached
        // entry over its probe subset with a bounded reaching-light set, then the
        // groups assemble into the volume. This is a deliberate approximation —
        // lights past the reach cutoff drop, so far-bounce regions run slightly
        // dim. Not byte-identical to the cold whole-volume bake; the cold
        // `--no-cache` build is the exact ship source of truth.
        log::warn!("{}", sh_group::WARM_SH_APPROX_WARNING);
        sh_group::bake_sh_volume_grouped_controlled(&sh_ctx, &sh_config, Some(cache), &sh_control)
    } else {
        // Cold / exact path (`--no-cache`): the monolithic whole-volume bake, the
        // shippable source of truth. No per-group reads/writes, no warning.
        sh_bake::bake_sh_volume_controlled(&sh_ctx, &sh_config, &sh_control)
    };
    // Keep the raw MapData-indexed descriptor lookup for compiler-only delta
    // policy. Runtime map lights come from compact AlphaLights (`_bake_only`
    // omitted), so the emitted table is remapped exactly once below.
    let raw_slot_for_map_light = sh_volume_section.slot_for_map_light.clone();
    // SH bake stages use raw MapData source indices so bake-only animated
    // lights can own descriptors. Runtime map lights come from compact
    // AlphaLights (`_bake_only` omitted), so remap the lookup table exactly
    // once at the PRL boundary.
    sh_volume_section.slot_for_map_light =
        alpha_lights_ns.compact_source_table(&sh_volume_section.slot_for_map_light);
    // Both warm grouped and cold monolithic bakes reach this packaging seam as
    // the same compact RGBA16F v9 section. Keep group-cache records lossless and
    // format-independent; the emitted base atlas defaults to BC6H here.
    let compact_atlas_bytes = sh_volume_section.compact_atlas.len();
    // Output-preserving SH analysis and the coarsening classifier both need the
    // base indirect tiles at full RGBA16F precision — capture the compact
    // section BEFORE the lossy BC6H re-encode below. Cloned only under
    // `--sh-analyze` or the default-on id-41 classifier; changes no emitted
    // bytes (the clone is read, never packed).
    let sh_analyze_base_indirect: Option<
        postretro_level_format::sh_volume::OctahedralShVolumeSection,
    > = if args.sh_analyze || sh_coarsening_enabled {
        Some(sh_volume_section.clone())
    } else {
        None
    };
    sh_volume_section =
        sh_bake::encode_sh_volume_section_bc6h(&sh_volume_section, args.uncompressed_irradiance);
    let total_probes = sh_volume_section.total_probes();
    let valid_probes = sh_volume_section
        .probes
        .iter()
        .filter(|probe| probe.validity != 0)
        .count();
    log::info!(
        "[Compiler] OctahedralShVolume compact atlas: {valid_probes}/{total_probes} valid probes, \
         compact {compact_atlas_bytes} bytes, encoded {} bytes, format tag {}",
        sh_volume_section.compact_atlas.len(),
        sh_volume_section.irradiance_format,
    );
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::ShBake,
        stage_start,
        sh_volume_section.grid_dimensions != [0, 0, 0],
    );
    if args.verbose {
        sh_bake::log_stats(&sh_volume_section);
    }
    let stage_start = begin_stage(reporter.as_ref(), StageId::DeltaShBake);
    let delta_sh_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::DeltaShBake, delta_sh_progress.clone());
    let delta_sh_control = BakeControl::new(Arc::clone(&governor), &delta_sh_progress);
    let delta_sh_volumes_section = {
        let inputs = delta_sh_bake::DeltaBakeInputs {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &geo_result,
            tree: &result.tree,
            exterior_leaves: &exterior_leaves,
            portals: &generated_portals,
            animated_lights: &animated_baked_lights,
        };
        delta_sh_bake::bake_delta_sh_volumes_controlled(&inputs, &sh_config, &delta_sh_control)
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::DeltaShBake,
        stage_start,
        delta_sh_volumes_section.is_some(),
    );
    if args.verbose {
        if let Some(ref section) = delta_sh_volumes_section {
            delta_sh_bake::log_stats(section);
        } else {
            log::info!("DeltaShVolumes: skipped (no animated lights)");
        }
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::DirectShBake);
    let direct_sh_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::DirectShBake, direct_sh_progress.clone());
    let direct_sh_control = BakeControl::new(Arc::clone(&governor), &direct_sh_progress);
    // Baked static-direct octahedral SH for dynamic objects. Emitted IFF there are
    // static (baked) lights — a map whose only lights are animated produces NO
    // direct section (absence; the loader treats it as direct = 0), so animated
    // direct never double-counts against animated-light weight maps.
    // Display-only: the reporter renders DirectShBake as skipped for a degenerate
    // (empty) probe grid, but the section is still produced unconditionally when
    // static lights exist — preserving byte-identity with the baseline and keeping
    // the downstream EntityShadowLights selection running exactly as before.
    let mut direct_sh_present = false;
    // Pre-BC6H base-direct capture for the SH analysis (RGBA16F precision).
    // Populated only under `--sh-analyze`; read-only, never packed.
    let mut sh_analyze_base_direct: Option<
        postretro_level_format::direct_sh_volume::DirectShVolumeSection,
    > = None;
    let direct_sh_volume_section = if static_baked_lights.is_empty() {
        if args.verbose {
            log::info!("DirectShVolume: skipped (no static lights)");
        }
        None
    } else {
        let inputs = direct_sh_bake::DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &generated_portals,
        };
        // Warm path passes the shared `StageCache`; cold (`--no-cache`/`--release`)
        // passes `None`. Unlike the indirect bake's warm bounded-light approximation,
        // the direct bake uses a strict whole-section cache key with the provably-zero
        // light-reach cull in both modes — output is byte-identical regardless of path.
        let raw = direct_sh_bake::bake_direct_sh_volume_cached_controlled(
            &inputs,
            &sh_config,
            stage_cache.as_ref(),
            &direct_sh_control,
        );
        if args.verbose {
            direct_sh_bake::log_cull_savings(&inputs, &sh_config);
        }
        direct_sh_present = raw.grid_dimensions != [0, 0, 0];
        if args.sh_analyze || sh_coarsening_enabled {
            sh_analyze_base_direct = Some(raw.clone());
        }
        // Re-encode the uncompressed RGBA16F bake output into the production
        // BC6H section, honoring the lightmap path's debug bypass. Emitted
        // unconditionally (matching baseline) even for a degenerate grid, where the
        // encoder passes the empty section through unchanged.
        let section = direct_sh_bake::encode_direct_section_bc6h(
            &raw,
            lightmap_config.uncompressed_irradiance,
        );
        if args.verbose {
            // Report both footprints alongside indirect SH for comparison.
            let post_compression = section.atlas.len();
            let pre_compression = direct_sh_bake::direct_dense_atlas_byte_size(&raw);
            let indirect_atlas = sh_volume_section
                .try_to_bytes()
                .map_err(|error| {
                    anyhow::anyhow!("OctahedralShVolume violates its v9 wire contract: {error}")
                })?
                .len();
            log::info!(
                "[Compiler] DirectShVolume atlas footprint: {post_compression} bytes BC6H \
                 (pre-compression dense {pre_compression} bytes); indirect OctahedralShVolume \
                 section {indirect_atlas} bytes",
            );
        }
        Some(section)
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::DirectShBake,
        stage_start,
        direct_sh_present,
    );

    let stage_start = begin_stage(reporter.as_ref(), StageId::AnimatedDirectShBake);
    let animated_direct_sh_progress = StageProgress::indeterminate();
    reporter.declare_progress(
        StageId::AnimatedDirectShBake,
        animated_direct_sh_progress.clone(),
    );
    let animated_direct_sh_control =
        BakeControl::new(Arc::clone(&governor), &animated_direct_sh_progress);
    let animated_direct_sh_delta_volumes_section = if animated_baked_lights.is_empty() {
        None
    } else {
        let inputs = animated_direct_sh_bake::AnimatedDirectShBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &generated_portals,
            animated_lights: &animated_baked_lights,
        };
        animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes_controlled(
            &inputs,
            &sh_config,
            &animated_direct_sh_control,
        )
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::AnimatedDirectShBake,
        stage_start,
        animated_direct_sh_delta_volumes_section.is_some(),
    );
    if args.verbose {
        if let Some(ref section) = animated_direct_sh_delta_volumes_section {
            animated_direct_sh_bake::log_stats(section);
        } else {
            log::info!("AnimatedDirectShDeltaVolumes: skipped (no animated lights or probe grid)");
        }
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::EntityShadowLights);
    let raw_entity_shadow_lights_section = direct_sh_volume_section.as_ref().and_then(|_| {
        let inputs = entity_shadow_select::EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &geo_result,
            static_lights: &static_baked_lights,
            alpha_lights: &alpha_lights_ns,
            params: map_data.entity_shadow_params,
        };
        let section = entity_shadow_select::select_entity_shadow_lights(&inputs);
        if section.light_indices.is_empty() {
            None
        } else {
            Some(section)
        }
    });
    let entity_shadow_lights_elapsed = stage_start.elapsed();
    if args.verbose {
        if let Some(ref section) = raw_entity_shadow_lights_section {
            log::info!(
                "EntityShadowLights: selected {} static light candidate(s)",
                section.light_indices.len()
            );
        } else {
            log::info!("EntityShadowLights: skipped (no DirectShVolume selection)");
        }
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::DirectShDeltaBake);
    let direct_sh_delta_progress = StageProgress::indeterminate();
    reporter.declare_progress(StageId::DirectShDeltaBake, direct_sh_delta_progress.clone());
    let direct_sh_delta_control =
        BakeControl::new(Arc::clone(&governor), &direct_sh_delta_progress);
    let raw_direct_sh_delta_volumes_section =
        raw_entity_shadow_lights_section
            .as_ref()
            .and_then(|section| {
                let inputs = direct_sh_bake::DirectBakeInputs {
                    sh_ctx: &sh_ctx,
                    portals: &generated_portals,
                };
                direct_sh_bake::bake_direct_sh_delta_volumes_controlled(
                    &inputs,
                    &sh_config,
                    &alpha_lights_ns,
                    section,
                    &direct_sh_delta_control,
                )
            });
    let direct_sh_delta_elapsed = stage_start.elapsed();

    let (entity_shadow_lights_section, direct_sh_delta_volumes_section, direct_sh_delta_stats) =
        match raw_entity_shadow_lights_section {
            Some(selection) => match raw_direct_sh_delta_volumes_section {
                Some((deltas, stats))
                    if direct_sh_volume_section.as_ref().is_some_and(|direct| {
                        pack::direct_sh_delta_is_usable_for_selection(
                            &deltas,
                            direct,
                            selection.light_indices.len(),
                        )
                    }) =>
                {
                    (Some(selection), Some(deltas), Some(stats))
                }
                Some(_) => {
                    log::warn!(
                        "EntityShadowLights: clearing {} selected static light candidate(s) because DirectShDeltaVolumes is unusable for the DirectShVolume base",
                        selection.light_indices.len()
                    );
                    (None, None, None)
                }
                None => {
                    log::warn!(
                        "EntityShadowLights: clearing {} selected static light candidate(s) because DirectShDeltaVolumes is absent",
                        selection.light_indices.len()
                    );
                    (None, None, None)
                }
            },
            None => (None, None, None),
        };
    timings.push((
        StageId::EntityShadowLights.label(),
        entity_shadow_lights_elapsed,
    ));
    if entity_shadow_lights_section.is_some() {
        reporter.finish_stage(StageId::EntityShadowLights);
    } else {
        reporter.skip_stage(StageId::EntityShadowLights);
    }
    timings.push((StageId::DirectShDeltaBake.label(), direct_sh_delta_elapsed));
    if direct_sh_delta_volumes_section.is_some() {
        reporter.finish_stage(StageId::DirectShDeltaBake);
    } else {
        reporter.skip_stage(StageId::DirectShDeltaBake);
    }
    log_direct_sh_delta_stats(direct_sh_delta_stats.as_ref(), args.verbose);

    // The three delta bakes meet at one owned compiler-only seam. Exact-zero
    // drop observes the dense bake output; valid-probe compaction then rewrites
    // id 41 to the id-34-valid local tiles before the payload cap is applied.
    let script_mutable_descriptor_slots = crate::delta_drop_policy::script_mutable_descriptor_slots(
        &map_data.lights,
        membership_manifest.as_ref(),
        &raw_slot_for_map_light,
        animated_baked_lights.len(),
    );
    let mut delta_sections = delta_sections::PostBakeDeltaSections::new(
        args.delta_section_config,
        delta_sh_volumes_section,
        entity_shadow_lights_section,
        direct_sh_delta_volumes_section,
        animated_direct_sh_delta_volumes_section,
    );
    debug_assert_eq!(
        delta_sections.config, args.delta_section_config,
        "the post-bake delta handoff must retain the resolved compiler configuration"
    );
    delta_sections.apply_exact_zero_drop_policy(&script_mutable_descriptor_slots)?;
    // The emitted-reconstruction diagnostic compares final compact bytes with
    // their post-drop dense source. Retain this measurement-only snapshot only
    // for an explicit coarsened analysis; normal and uniform-L0 bakes do not
    // pay the additional memory cost.
    let sh_analyze_dense_deltas = (args.sh_analyze && sh_coarsening_enabled).then(|| {
        (
            delta_sections.indirect.clone(),
            delta_sections.direct.clone(),
            delta_sections.animated_direct.clone(),
        )
    });
    // Production id-41 coarsening classifies each 4×4×4 brick while payloads
    // are still dense, before the single valid-probe compaction. The map-level
    // opt-out is checked here, before classification, so its untouched all-L0
    // path remains byte-identical to the pre-activation uniform bake.
    run_sh_coarsening_if_enabled(sh_coarsening_enabled, || {
        apply_coarsen_classification(
            args,
            &map_data.sh_protect_aabbs,
            sh_analyze_base_indirect.as_ref(),
            sh_analyze_base_direct.as_ref(),
            &mut delta_sections,
            &script_mutable_descriptor_slots,
        )
    })?;
    delta_sections.apply_valid_probe_compaction(&sh_volume_section)?;
    delta_sections.enforce_payload_cap()?;
    if let (Some(selection), Some(deltas)) = (
        delta_sections.entity_shadow_lights.as_ref(),
        delta_sections.direct.as_ref(),
    ) {
        anyhow::ensure!(
            pack::direct_sh_delta_covers_selection(deltas, selection.light_indices.len()),
            "DirectShDeltaVolumes lost coverage for an EntityShadowLights selection after delta dropping"
        );
    }

    // Output-preserving SH coarsenability analysis (`--sh-analyze`). Runs AFTER
    // the delta sections are finalized — post EntityShadowLights static-light
    // selection AND post exact-zero-drop policy (and post payload-cap enforcement,
    // which either passed above or failed the build) — so both the delta byte
    // accounting and the composed-receiver error reflect the EMITTED delta set
    // rather than a pre-filter superset. The dense pre-BC6H base tiles were
    // captured earlier (before base compaction / BC6H re-encode) and are threaded
    // in unchanged. Measurement only — it reads the finalized owned sections,
    // never mutates, so the emitted `.prl` stays byte-identical to a run without
    // the flag.
    if args.sh_analyze {
        run_sh_analysis(
            args,
            sh_analyze_base_indirect.as_ref(),
            sh_analyze_base_direct.as_ref(),
            sh_analyze_dense_deltas
                .as_ref()
                .map(|dense| sh_analyze::DenseDeltaSections {
                    indirect: dense.0.as_ref(),
                    direct: dense.1.as_ref(),
                    animated_direct: dense.2.as_ref(),
                }),
            delta_sections.indirect.as_ref(),
            delta_sections.direct.as_ref(),
            delta_sections.animated_direct.as_ref(),
        );
    }

    // Billboard scatter is deliberately baked after `PostBakeDeltaSections`
    // finalizes id 45. The animated scatter section must duplicate id 45's
    // descriptor and CSR layout exactly, including any exact-zero entry drops.
    let stage_start = begin_stage(reporter.as_ref(), StageId::BillboardDirectScatterBake);
    let scatter_inputs = billboard_direct_scatter_bake::BillboardDirectScatterBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &generated_portals,
        animated_lights: &animated_baked_lights,
    };
    let animated_scatter_layout_fits_pack_cap =
        delta_sections
            .animated_direct
            .as_ref()
            .is_none_or(|direct_deltas| {
                billboard_direct_scatter_bake::animated_scatter_layout_fits_pack_cap(direct_deltas)
            });
    if !animated_scatter_layout_fits_pack_cap {
        log::warn!(
            "[Compiler] Billboard direct scatter sections 47/48 withheld: finalized section-45 layout would exceed section 48's encoded pack cap"
        );
    }
    let scatter_control = BakeControl::new(Arc::clone(&governor), &StageProgress::indeterminate());
    let mut billboard_direct_scatter_volume_section = animated_scatter_layout_fits_pack_cap
        .then(|| {
            billboard_direct_scatter_bake::bake_billboard_direct_scatter_volume_cached_controlled(
                &scatter_inputs,
                &sh_config,
                delta_sections
                    .animated_direct
                    .as_ref()
                    .is_some_and(|section| {
                        billboard_direct_scatter_bake::has_animated_static_light_map_entries(
                            section,
                            &animated_baked_lights,
                        )
                    }),
                stage_cache.as_ref(),
                &scatter_control,
            )
        })
        .flatten();
    let animated_scatter_control =
        BakeControl::new(Arc::clone(&governor), &StageProgress::indeterminate());
    let animated_billboard_direct_scatter_delta_volumes_section =
        billboard_direct_scatter_volume_section.as_ref().and_then(|_| {
            delta_sections.animated_direct.as_ref().and_then(|direct_deltas| {
                billboard_direct_scatter_bake::bake_animated_billboard_direct_scatter_delta_volumes_cached_controlled(
                    &scatter_inputs,
                    &sh_config,
                    direct_deltas,
                    stage_cache.as_ref(),
                    &animated_scatter_control,
                )
            })
        });
    if billboard_direct_scatter_volume_section.is_some()
        && delta_sections.animated_direct.is_some()
        && animated_billboard_direct_scatter_delta_volumes_section.is_none()
    {
        log::warn!(
            "[Compiler] BillboardDirectScatterVolume withheld because its required animated scatter companion could not be baked"
        );
        billboard_direct_scatter_volume_section = None;
    }
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::BillboardDirectScatterBake,
        stage_start,
        billboard_direct_scatter_volume_section.is_some(),
    );
    if args.verbose {
        match (
            billboard_direct_scatter_volume_section.as_ref(),
            animated_billboard_direct_scatter_delta_volumes_section.as_ref(),
        ) {
            (Some(base), Some(animated)) => log::info!(
                "BillboardDirectScatter: {} base probes, {} animated CSR entries",
                base.total_probes().unwrap_or_default(),
                animated.affinity_lights.len(),
            ),
            (Some(base), None) => log::info!(
                "BillboardDirectScatter: {} base probes, no animated deltas",
                base.total_probes().unwrap_or_default(),
            ),
            (None, _) => log::info!(
                "BillboardDirectScatter: skipped (no static_light_map direct source or animated scatter entries)"
            ),
        }
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::ShadowmaskAtlas);
    // Shadowmask layers now bake charts in parallel. They must share the live
    // governor, but not the completed LightmapBake progress stage: that stage
    // has already finished and its published total covers different work.
    let shadowmask_progress = StageProgress::indeterminate();
    let shadowmask_control = BakeControl::new(Arc::clone(&governor), &shadowmask_progress);
    let shadowmask_atlas_section = if delta_sections.entity_shadow_lights.is_some() {
        let shared = lightmap_layer::SharedAtlas {
            charts: &face_charts,
            placements: &face_placements,
            atlas_width,
            atlas_height,
        };
        shadowmask_bake::bake_shadowmask_atlas_cached(
            delta_sections.entity_shadow_lights.as_ref(),
            &alpha_lights_ns,
            &shared,
            &bvh,
            &bvh_primitives,
            &geo_result,
            final_lightmap_density,
            args.soft_shadow_samples,
            stage_cache.as_ref(),
            &shadowmask_control,
        )
    } else {
        None
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::ShadowmaskAtlas,
        stage_start,
        shadowmask_atlas_section.is_some(),
    );
    if args.verbose {
        if let Some(ref section) = shadowmask_atlas_section {
            log::info!(
                "ShadowmaskAtlas: {}x{}x{}, {} selected channel entr(y/ies), {} bytes",
                section.width,
                section.height,
                section.layer_count,
                section.channels.len(),
                section.data.len(),
            );
        } else {
            log::info!("ShadowmaskAtlas: skipped (no selected static lights)");
        }
    }

    let stage_start = begin_stage(reporter.as_ref(), StageId::ChunkLightList);
    let chunk_light_list_section = {
        let inputs = chunk_light_list_bake::ChunkLightListInputs {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &geo_result,
            lights: &alpha_lights_ns,
            tree: &result.tree,
            portals: &generated_portals,
            exterior_leaves: &exterior_leaves,
        };
        chunk_light_list_bake::bake_chunk_light_list(
            &inputs,
            chunk_light_list_bake::DEFAULT_CELL_SIZE_METERS,
            chunk_light_list_bake::DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .map_err(|e| anyhow::anyhow!("Chunk light list bake failed: {e}"))?
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::ChunkLightList,
        stage_start,
        true,
    );

    let alpha_lights_section = pack::encode_alpha_lights(&alpha_lights_ns, &result.tree);
    let light_influence_section = pack::encode_light_influence(&alpha_lights_ns);
    let light_tags_section = pack::encode_light_tags(&alpha_lights_ns);
    let map_entities_section = pack::encode_map_entities(&map_data.map_entities);
    let fog_volumes_section = pack::encode_fog_volumes(
        &map_data.fog_volumes,
        map_data.fog_pixel_scale,
        map_data.initial_gravity,
    );
    let fog_cell_masks_section =
        fog_cell_masks::bake_fog_cell_masks(&result.tree, &map_data.fog_volumes);

    let (animated_chunk_lights, _) = animated_baked_lights.to_parallel_vecs();

    let stage_start = begin_stage(reporter.as_ref(), StageId::AnimatedLightChunks);
    // Returns a parallel chunk-range table indexed by BVH leaf slot; pack stamps
    // it onto the on-disk `BvhLeaf` records at serialization time. Empty section
    // signals no animated lights — no placeholder record is emitted.
    let (animated_light_chunks_section, bvh_chunk_ranges) =
        animated_light_chunks::build_animated_light_chunks(
            &bvh_section,
            &animated_baked_lights,
            &face_charts,
            &geo_result.face_index_ranges,
            final_lightmap_density,
        );
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::AnimatedLightChunks,
        stage_start,
        !animated_light_chunks_section.chunks.is_empty(),
    );

    let stage_start = begin_stage(reporter.as_ref(), StageId::AnimatedWeightMaps);
    // Keep genuine no-op stages indeterminate: publishing a zero total would
    // briefly present 100% before the stage is marked skipped.
    let animated_weight_progress = StageProgress::indeterminate();
    reporter.declare_progress(
        StageId::AnimatedWeightMaps,
        animated_weight_progress.clone(),
    );
    let animated_weight_control =
        BakeControl::new(Arc::clone(&governor), &animated_weight_progress);
    let animated_light_weight_maps_section = if animated_light_chunks_section.chunks.is_empty() {
        None
    } else {
        let wm_inputs = animated_light_weight_maps::WeightMapInputs {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &geo_result,
            chunk_section: &animated_light_chunks_section,
            lights: &animated_chunk_lights,
            face_charts: &face_charts,
            face_placements: &face_placements,
            atlas_width,
            atlas_height,
            static_atlas_layer_count,
            area_sample_count: args.soft_shadow_samples,
        };

        // Build the input hash from owned/serializable data. Charts, placements,
        // and the chunk section don't derive `Serialize`, so the hash folds
        // `animated_light_chunks_section.to_bytes()` as a proxy. That proxy is a
        // valid fingerprint for charts AND placements because
        // `build_animated_light_chunks` (and the upstream chart/placement
        // construction) are deterministic given geometry + lights + density — the
        // section bytes faithfully capture those derived inputs.
        //
        // Deliberate divergence from the lightmap/sh stages: those hash a
        // pre-bake geometry clone, but this hashes the post-mutation `geo_result`.
        // That's correct here — the weight-map bake consumes the mutated geometry,
        // and the mutations (`split_shared_vertices`, UV assignment) are
        // idempotent and deterministic, so post-mutation geometry is a stable
        // function of the inputs. Do not "fix" this to a pre-bake clone; it would
        // hash geometry the bake doesn't actually consume.
        let wm_input_hash = {
            let mut buf = postcard::to_allocvec(&animated_chunk_lights)
                .expect("postcard serialize animated_chunk_lights");
            buf.extend_from_slice(
                &postcard::to_allocvec(&geo_result).expect("postcard serialize geo_result"),
            );
            buf.extend_from_slice(&final_lightmap_density.to_le_bytes());
            buf.extend_from_slice(&atlas_width.to_le_bytes());
            buf.extend_from_slice(&atlas_height.to_le_bytes());
            buf.extend_from_slice(&static_atlas_layer_count.to_le_bytes());
            buf.extend_from_slice(&animated_light_chunks_section.to_bytes());
            buf.extend_from_slice(&args.soft_shadow_samples.to_le_bytes());
            *blake3::hash(&buf).as_bytes()
        };
        let wm_key = cache::CacheKey::new(
            "animated_lm_weight_maps",
            animated_light_weight_maps::STAGE_VERSION,
            &wm_input_hash,
        );

        let cached = stage_cache.as_ref().and_then(|c| c.get(&wm_key));
        let cached_wm_section = cached.and_then(|bytes| {
            postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection::from_bytes(&bytes)
                .map_err(|e| {
                    log::warn!("[cache] corrupt animated_lm_weight_maps entry, re-baking: {e}")
                })
                .ok()
        });

        if let Some(section) = cached_wm_section {
            log::info!("[cache] animated_lm_weight_maps hit");
            animated_light_weight_maps::validate_animated_atlas_budget(
                atlas_width,
                atlas_height,
                section.slot_to_static_layer.len() as u32,
            )
            .map_err(|e| anyhow::anyhow!("Animated weight-map bake failed: {e}"))?;
            animated_weight_control.publish_total(animated_light_chunks_section.chunks.len());
            // Cache-hit fast-advance on the orchestrator thread: honor pause only,
            // no permit (the parallel bake path is what needs a permit).
            animated_weight_control.governor().checkpoint();
            animated_weight_control.advance(animated_light_chunks_section.chunks.len());
            Some(section)
        } else {
            log::info!("[cache] animated_lm_weight_maps miss");
            let section = animated_light_weight_maps::bake_animated_light_weight_maps_controlled(
                &wm_inputs,
                &animated_weight_control,
            )
            .map_err(|e| anyhow::anyhow!("Animated weight-map bake failed: {e}"))?;
            if let Some(ref c) = stage_cache {
                c.put(&wm_key, &section.to_bytes());
            }
            Some(section)
        }
    };
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::AnimatedWeightMaps,
        stage_start,
        animated_light_weight_maps_section.is_some(),
    );

    let animated_light_chunks_section = if animated_light_chunks_section.chunks.is_empty() {
        None
    } else {
        Some(animated_light_chunks_section)
    };

    let sdf_atlas_section = if map_needs_sdf_atlas(&map_data.lights) {
        let stage_start = begin_stage(reporter.as_ref(), StageId::SdfAtlasBake);
        let sdf_config = sdf_bake::SdfConfig {
            voxel_size_m: args.voxel_size,
            ..sdf_bake::SdfConfig::default()
        };
        let section = {
            // Build serialisable inputs for the cache key. Geometry hash also
            // captures triangle order, so a deterministic geometry result
            // means a deterministic cache key.
            let sdf_inputs = sdf_bake::SdfInputs {
                geometry: geo_result.clone(),
            };
            let sdf_input_hash = {
                let mut buf =
                    postcard::to_allocvec(&sdf_inputs).expect("postcard serialize SdfInputs");
                buf.extend_from_slice(
                    &postcard::to_allocvec(&sdf_config).expect("postcard serialize SdfConfig"),
                );
                *blake3::hash(&buf).as_bytes()
            };
            let sdf_key =
                cache::CacheKey::new("sdf_atlas", sdf_bake::STAGE_VERSION, &sdf_input_hash);

            let cached = stage_cache.as_ref().and_then(|c| c.get(&sdf_key));
            let cached_section = cached.and_then(|bytes| {
                postretro_level_format::sdf_atlas::SdfAtlasSection::from_bytes(&bytes)
                    .map_err(|e| log::warn!("[cache] corrupt sdf_atlas entry, re-baking: {e}"))
                    .ok()
            });

            if let Some(section) = cached_section {
                log::info!("[cache] sdf_atlas hit");
                section
            } else {
                log::info!("[cache] sdf_atlas miss");
                let ctx = sdf_bake::SdfBakeCtx {
                    geometry: &geo_result,
                    tree: &result.tree,
                };
                let section = sdf_bake::bake_sdf_atlas(&ctx, &sdf_config);
                if let Some(ref c) = stage_cache {
                    c.put(&sdf_key, &section.to_bytes());
                }
                section
            }
        };
        finish_stage(
            &mut timings,
            reporter.as_ref(),
            StageId::SdfAtlasBake,
            stage_start,
            true,
        );
        if args.verbose {
            sdf_bake::log_stats(&section);
        }
        Some(section)
    } else {
        None
    };

    let stage_start = begin_stage(reporter.as_ref(), StageId::TextureMips);
    let prm_root = resolve_prm_root_via_cargo(&args.input);
    let name_to_key =
        texture_mips::bake_texture_mips(&geo_result.texture_names.names, &texture_root, &prm_root)?;
    let content_root = resolve_content_root(&args.input);
    bake_model_textures(&map_data.map_entities, &content_root, &prm_root);
    bake_sprite_textures(&map_data.map_entities, &texture_root, &prm_root);
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::TextureMips,
        stage_start,
        true,
    );

    let stage_start = begin_stage(reporter.as_ref(), StageId::Packing);

    let portals_section = pack::encode_portals(&generated_portals);
    pack::pack_and_write_portals_with_billboard_scatter(
        &args.output,
        &geo_result,
        &name_to_key,
        &vis_result.leaves_section,
        &result.tree,
        &portals_section,
        &exterior_leaves,
        &bvh_section,
        &bvh_chunk_ranges,
        &alpha_lights_section,
        &light_influence_section,
        &sh_volume_section,
        direct_sh_volume_section.as_ref(),
        delta_sections.entity_shadow_lights.as_ref(),
        delta_sections.direct.as_ref(),
        shadowmask_atlas_section.as_ref(),
        &lightmap_section,
        &chunk_light_list_section,
        animated_light_chunks_section.as_ref(),
        animated_light_weight_maps_section.as_ref(),
        light_tags_section.as_ref(),
        delta_sections.indirect.as_ref(),
        data_script_section.as_ref(),
        map_entities_section.as_ref(),
        &fog_volumes_section,
        fog_cell_masks_section.as_ref(),
        sdf_atlas_section.as_ref(),
        navmesh_section.as_ref(),
        kinematic_geometry_section.as_ref(),
        trigger_volumes_section.as_ref(),
        cell_draw_index_bytes,
        Some(cell_visibility_bytes),
        delta_sections.animated_direct.as_ref(),
        billboard_direct_scatter_volume_section.as_ref(),
        animated_billboard_direct_scatter_delta_volumes_section.as_ref(),
    )?;
    finish_stage(
        &mut timings,
        reporter.as_ref(),
        StageId::Packing,
        stage_start,
        true,
    );

    reporter.finalize(&timings, started.elapsed());

    Ok(())
}

/// Concatenate the CLI (`--sh-protect-aabb`) and mapper-authored
/// (`sh_protect_volume`) protection AABBs into the single list the coarsening
/// classifier reads. Both sources are world-space `[minx,miny,minz,maxx,maxy,maxz]`
/// boxes in engine meters, so the union is a plain concatenation — a brick is
/// protected if it intersects any box from either source.
fn combined_protect_aabbs(cli: &[[f32; 6]], map: &[[f32; 6]]) -> Vec<[f32; 6]> {
    let mut combined = Vec::with_capacity(cli.len() + map.len());
    combined.extend_from_slice(cli);
    combined.extend_from_slice(map);
    combined
}

/// Invoke the production classifier only when the map has not selected the
/// uniform-grid fallback. Keeping the predicate outside the closure makes the
/// opt-out a true pre-classification short-circuit.
fn run_sh_coarsening_if_enabled(
    enabled: bool,
    classify: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if enabled {
        classify()?;
    }
    Ok(())
}

/// Classify id 41 coarsening levels and stamp them in place before valid-probe
/// compaction consumes the dense payload. Ids 27 and 45 stay uniform L0 because
/// their runtime animation/script amplitudes have no finite bake-known bound.
/// Reads the pre-BC6H base indirect (RGBA16F) for composed magnitude + the sole
/// probe-validity authority. No-op when the base grid is absent/degenerate.
fn apply_coarsen_classification(
    args: &Args,
    map_protect_aabbs: &[[f32; 6]],
    base_indirect: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    base_direct: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    delta_sections: &mut delta_sections::PostBakeDeltaSections,
    mutable_descriptors: &crate::delta_drop_policy::ScriptMutableDescriptorSlots,
) -> anyhow::Result<()> {
    // Enforce the owner-approved section scope even on degenerate/absent-base
    // exits. The bake producers currently initialize these arrays to L0, but
    // this boundary owns the policy rather than relying on that detail.
    delta_sections.enforce_id41_only_coarsening_policy();

    let Some(base) = base_indirect else {
        log::warn!("[sh-coarsen] no base SH volume produced; coarsening skipped (uniform L0)");
        return Ok(());
    };
    if base.grid_dimensions == [0, 0, 0] {
        log::warn!("[sh-coarsen] degenerate SH grid; coarsening skipped (uniform L0)");
        return Ok(());
    }
    let validity: Vec<u8> = base.probes.iter().map(|p| p.validity).collect();
    let grid = sh_coarsen::SectionGrid {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dims: base.grid_dimensions,
        validity: &validity,
    };
    let params = sh_coarsen::CoarsenParams::default();

    // Both protection sources reach the classifier: the `--sh-protect-aabb` CLI
    // stand-in and the mapper-authored `sh_protect_volume` brush AABBs. Their
    // union forces every intersecting brick to L0 (dense).
    let protect_aabbs = combined_protect_aabbs(&args.sh_protect_aabbs, map_protect_aabbs);

    // The direct classifier still reads all three dense sections for the
    // composed reference magnitude, but produces levels only for id 41.
    let direct_levels = {
        let all = sh_coarsen::DeltaSectionsRef {
            indirect: delta_sections.indirect.as_ref(),
            direct: delta_sections.direct.as_ref(),
            anim_direct: delta_sections.animated_direct.as_ref(),
        };
        delta_sections.direct.as_ref().map(|_| {
            sh_coarsen::classify_direct_levels(
                base,
                base_direct,
                all,
                grid,
                &protect_aabbs,
                &params,
            )
        })
    };

    // Guard the defensive grid-mismatch path so id 41 is never handed a
    // wrong-length level array that would corrupt compaction / the wire
    // contract. All sections remain uniform L0 in that case.
    let mut direct_classified = false;
    if let (Some(section), Some(levels)) = (delta_sections.direct.as_mut(), direct_levels) {
        let cells = section.affinity_cell_count();
        if levels.len() == cells {
            section.cell_levels = levels;
            direct_classified = true;
        } else {
            section
                .cell_levels
                .fill(postretro_level_format::sh_reconstruct::Level::L0.to_u8());
            log::warn!(
                "[sh-coarsen] direct affinity mismatch (levels {} vs {cells} cells); leaving uniform L0",
                levels.len()
            );
        }
    }
    log::info!(
        "[sh-coarsen] id 41 classification {}; ids 27/45 remain uniform L0",
        if direct_classified {
            "applied"
        } else {
            "not applicable"
        }
    );
    crate::sh_runtime_envelope::apply_runtime_safe_envelope(
        base,
        base_direct,
        delta_sections,
        mutable_descriptors,
        &params,
    )?;
    Ok(())
}

/// Drive the output-preserving SH coarsenability analysis and emit its summary
/// and JSON. Reads captured pre-BC6H base tiles and the three FINALIZED delta
/// sections (post static-light selection + exact-zero-drop, i.e. the emitted
/// set); mutates nothing that reaches the packer.
fn run_sh_analysis(
    args: &Args,
    base_indirect: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    base_direct: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    dense_deltas: Option<sh_analyze::DenseDeltaSections<'_>>,
    delta_indirect: Option<&postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    delta_direct: Option<
        &postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection,
    >,
    delta_anim_direct: Option<
        &postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
) {
    let Some(base) = base_indirect else {
        log::warn!("[sh-analyze] no base SH volume produced (empty grid?); analysis skipped");
        return;
    };
    if base.grid_dimensions == [0, 0, 0] {
        log::warn!("[sh-analyze] degenerate SH grid; analysis skipped");
        return;
    }
    let validity: Vec<u8> = base.probes.iter().map(|p| p.validity).collect();
    let protect: Vec<sh_analyze::ProtectAabb> = args
        .sh_protect_aabbs
        .iter()
        .map(|a| sh_analyze::ProtectAabb {
            min: [a[0], a[1], a[2]],
            max: [a[3], a[4], a[5]],
        })
        .collect();
    let inputs = sh_analyze::AnalyzeInputs {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dims: base.grid_dimensions,
        validity: &validity,
        base_indirect: base,
        base_direct,
        delta_indirect,
        delta_direct,
        delta_anim_direct,
        protect_aabbs: &protect,
        thresholds: &sh_analyze::DEFAULT_THRESHOLDS,
    };
    let mut report = sh_analyze::run_analysis(&inputs);
    if let Some(dense) = dense_deltas {
        match sh_analyze::run_emitted_reconstruction_analysis(
            &inputs,
            dense.indirect,
            dense.direct,
            dense.animated_direct,
            delta_indirect,
            delta_direct,
            delta_anim_direct,
        ) {
            Ok(emitted) => report.emitted_reconstruction = Some(emitted),
            Err(error) => {
                log::warn!("[sh-analyze] emitted reconstruction validation skipped: {error}")
            }
        }
    }
    sh_analyze::log_summary(&report);

    let out = args.sh_analyze_out.clone().unwrap_or_else(|| {
        let mut p = args.output.clone().into_os_string();
        p.push(".sh-analysis.json");
        std::path::PathBuf::from(p)
    });
    match sh_analyze::write_json(&report, &out) {
        Ok(()) => log::info!("[sh-analyze] wrote analysis JSON to {}", out.display()),
        Err(e) => log::warn!("[sh-analyze] failed to write {}: {e}", out.display()),
    }
}

fn log_direct_sh_delta_stats(stats: Option<&direct_sh_bake::DirectDeltaBakeStats>, verbose: bool) {
    if let Some(stats) = stats {
        let top = stats
            .rows
            .first()
            .expect("a usable direct SH delta section has at least one CSR entry");
        log::info!(
            "DirectShDeltaVolumes: {} delta bytes; top static_index {} (selection slot {}, {} CSR entries, {} bytes)",
            stats.total_bytes,
            top.static_index,
            top.selection_slot,
            top.csr_entry_count,
            top.byte_total,
        );
        if verbose {
            for row in &stats.rows {
                log::info!(
                    "DirectShDeltaVolumes histogram: selection slot {}, static_index {}, {} CSR entries, {} bytes",
                    row.selection_slot,
                    row.static_index,
                    row.csr_entry_count,
                    row.byte_total,
                );
            }
        }
    } else if verbose {
        log::info!("DirectShDeltaVolumes: skipped (no usable selected light deltas)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use log::Level;
    use postretro_test_log_capture::LogCapture;

    #[test]
    fn combined_protect_aabbs_concatenates_cli_then_map_sources() {
        let cli = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let map = [
            [2.0, 2.0, 2.0, 3.0, 3.0, 3.0],
            [4.0, 4.0, 4.0, 5.0, 5.0, 5.0],
        ];
        let combined = combined_protect_aabbs(&cli, &map);
        // Both sources reach the classifier: neither is dropped, and CLI boxes
        // lead so the order is stable for the coarsening sweep.
        assert_eq!(combined.len(), 3);
        assert_eq!(combined[0], cli[0]);
        assert_eq!(combined[1], map[0]);
        assert_eq!(combined[2], map[1]);
    }

    #[test]
    fn combined_protect_aabbs_handles_empty_sources() {
        let empty: [[f32; 6]; 0] = [];
        let one = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        assert!(combined_protect_aabbs(&empty, &empty).is_empty());
        assert_eq!(combined_protect_aabbs(&one, &empty), one);
        assert_eq!(combined_protect_aabbs(&empty, &one), one);
    }

    #[test]
    fn uniform_grid_optout_short_circuits_before_classification() {
        let calls = std::cell::Cell::new(0);
        run_sh_coarsening_if_enabled(false, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .expect("disabled coarsening remains a valid uniform bake");
        assert_eq!(calls.get(), 0, "classification must not run under opt-out");
    }

    #[test]
    fn default_path_invokes_classification_once() {
        let calls = std::cell::Cell::new(0);
        run_sh_coarsening_if_enabled(true, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .expect("default-on classification succeeds");
        assert_eq!(calls.get(), 1);
    }

    fn direct_delta_stats_fixture() -> direct_sh_bake::DirectDeltaBakeStats {
        direct_sh_bake::DirectDeltaBakeStats {
            rows: vec![
                direct_sh_bake::DirectDeltaBakeStatsRow {
                    selection_slot: 3,
                    static_index: 17,
                    csr_entry_count: 4,
                    byte_total: 73_728,
                },
                direct_sh_bake::DirectDeltaBakeStatsRow {
                    selection_slot: 1,
                    static_index: 5,
                    csr_entry_count: 1,
                    byte_total: 18_432,
                },
            ],
            total_bytes: 92_160,
        }
    }

    #[test]
    fn planned_stage_contract_pins_order_labels_and_sdf_prediction() {
        let without_sdf = planned_stages_for_sdf(false);
        let with_sdf = planned_stages_for_sdf(true);

        assert_eq!(without_sdf.len(), 24);
        assert_eq!(with_sdf.len(), 24);
        assert_eq!(
            without_sdf
                .iter()
                .map(|stage| (stage.id, stage.label))
                .collect::<Vec<_>>(),
            vec![
                (StageId::Parsing, "Parsing"),
                (StageId::DataScript, "DataScript"),
                (StageId::TextureValidation, "TexValidation"),
                (StageId::Partitioning, "Partitioning"),
                (StageId::Visibility, "Visibility"),
                (StageId::Geometry, "Geometry"),
                (StageId::BvhBuild, "BVH Build"),
                (StageId::CellVisibility, "Cell Visibility"),
                (StageId::NavMesh, "NavMesh"),
                (StageId::LightmapBake, "Lightmap Bake"),
                (StageId::ShBake, "SH Bake"),
                (StageId::DeltaShBake, "Delta SH Bake"),
                (StageId::DirectShBake, "Direct SH Bake"),
                (StageId::AnimatedDirectShBake, "Animated Direct SH Bake"),
                (StageId::EntityShadowLights, "EntityShadowLights"),
                (StageId::DirectShDeltaBake, "Direct SH Delta Bake"),
                (
                    StageId::BillboardDirectScatterBake,
                    "Billboard Direct Scatter Bake",
                ),
                (StageId::ShadowmaskAtlas, "ShadowmaskAtlas"),
                (StageId::ChunkLightList, "ChunkLightList"),
                (StageId::AnimatedLightChunks, "AnimLightChunks"),
                (StageId::AnimatedWeightMaps, "AnimWeightMaps"),
                (StageId::SdfAtlasBake, "SDF Atlas Bake"),
                (StageId::TextureMips, "TextureMips"),
                (StageId::Packing, "Packing"),
            ]
        );
        assert!(
            without_sdf
                .iter()
                .filter(|stage| stage.id != StageId::SdfAtlasBake)
                .all(|stage| stage.predicted_present)
        );
        let sdf_index = without_sdf
            .iter()
            .position(|stage| stage.id == StageId::SdfAtlasBake)
            .expect("planned stages include SDF atlas bake");
        assert!(!without_sdf[sdf_index].predicted_present);
        assert!(with_sdf[sdf_index].predicted_present);
    }

    #[test]
    fn direct_delta_info_logging_emits_one_summary_without_histogram() {
        let capture = LogCapture::start();

        log_direct_sh_delta_stats(Some(&direct_delta_stats_fixture()), false);

        let records = capture.records();
        assert_eq!(
            records.len(),
            1,
            "info mode adds exactly one delta log line"
        );
        assert_eq!(records[0].level, Level::Info);
        assert_eq!(
            records[0].message,
            "DirectShDeltaVolumes: 92160 delta bytes; top static_index 17 (selection slot 3, 4 CSR entries, 73728 bytes)"
        );
        capture.assert_not_logged(Level::Info, "DirectShDeltaVolumes histogram:");
    }

    #[test]
    fn direct_delta_verbose_logging_emits_sorted_per_slot_histogram() {
        let capture = LogCapture::start();

        log_direct_sh_delta_stats(Some(&direct_delta_stats_fixture()), true);

        let messages: Vec<_> = capture
            .records()
            .into_iter()
            .map(|record| record.message)
            .collect();
        assert_eq!(
            messages,
            vec![
                "DirectShDeltaVolumes: 92160 delta bytes; top static_index 17 (selection slot 3, 4 CSR entries, 73728 bytes)",
                "DirectShDeltaVolumes histogram: selection slot 3, static_index 17, 4 CSR entries, 73728 bytes",
                "DirectShDeltaVolumes histogram: selection slot 1, static_index 5, 1 CSR entries, 18432 bytes",
            ]
        );
    }

    #[test]
    fn direct_delta_info_logging_is_silent_after_usability_rejection() {
        let capture = LogCapture::start();

        log_direct_sh_delta_stats(None, false);

        assert!(
            capture.records().is_empty(),
            "a discarded delta section must not publish a footprint summary"
        );
    }
}
