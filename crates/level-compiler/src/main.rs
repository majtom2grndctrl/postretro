// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod affinity_grid;
pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bc5;
pub mod bc6h;
pub mod bvh_build;
pub mod cache;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_sh_bake;
#[cfg(test)]
pub mod fixture_pipeline;
pub mod fog_cell_masks;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod light_namespaces;
pub mod lightmap_bake;
pub mod lightmap_layer;
pub mod map_data;
pub mod map_format;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod sdf_bake;
pub mod sh_bake;
pub mod sh_group;
pub mod texture_mips;
pub mod texture_validation;
pub mod visibility;

use std::path::{Path, PathBuf};
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use map_format::{DEFAULT_MAP_FORMAT, MapFormat};

struct BuildProgress {
    started: Instant,
    pb: Option<ProgressBar>,
    verbose: bool,
}

impl BuildProgress {
    fn new(started: Instant, verbose: bool) -> Self {
        Self {
            started,
            pb: None,
            verbose,
        }
    }

    fn start_stage(&mut self, msg: &str) {
        if let Some(pb) = self.pb.take() {
            pb.finish();
        }

        let elapsed = self.started.elapsed();

        if !self.verbose {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{elapsed:>4}  {spinner} {msg}")
                    .unwrap(),
            );
            pb.set_message(msg.to_string());
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            self.pb = Some(pb);
        } else {
            eprintln!("{:>6.2}s  {}", elapsed.as_secs_f32(), msg);
        }
    }

    fn finish(&mut self) {
        if let Some(pb) = self.pb.take() {
            pb.finish();
        }
    }
}

/// Resolve the textures directory from a map input path.
///
/// Mirrors the runtime resolver `content_root_from_map` in
/// `crates/postretro/src/main.rs`: `<content_root>/textures/`, where
/// `<content_root>` is the parent of the map's directory (typically
/// `content/<mod>/maps/`). For a map outside this layout the path is still
/// constructed; the validator is a no-op if the directory does not exist.
fn resolve_texture_root(map_path: &Path) -> PathBuf {
    let map_dir = map_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let content_root = map_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    content_root.join("textures")
}

/// Resolve the `.prm` mip-cache root from a map input path.
///
/// Lives next to the stage cache under `<workspace>/.build-caches/prm-cache/`.
/// Falls back to the map's parent directory when no `Cargo.toml` ancestor
/// is found — covers shipping or standalone layouts that omit the
/// workspace manifest.
fn resolve_prm_cache_root_via_cargo(map_path: &Path) -> PathBuf {
    cache::find_workspace_root(map_path)
        .unwrap_or_else(|| {
            map_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        })
        .join(".build-caches")
        .join("prm-cache")
}

/// Whether the SDF occluder atlas must bake — true iff any light carries the
/// `sdf` shadow type.
///
/// Content-driven, exactly like the lightmap bakes because lights exist: the
/// atlas follows from the map's content, not a CLI flag. So an `sdf`-typed
/// light can never ship without the atlas it needs (the no-atlas-silent-no-
/// shadow footgun is removed by construction). A map with zero `sdf` lights
/// emits no atlas section, which the runtime handles gracefully — `sdf_factor`
/// defaults to a no-op multiply.
fn map_needs_sdf_atlas(lights: &[map_data::MapLight]) -> bool {
    lights
        .iter()
        .any(|l| l.shadow_type == map_data::ShadowType::Sdf)
}

/// Resolve the effective lightmap density from the CLI flag and the
/// worldspawn `_lightmap_density` KVP.
///
/// Precedence (highest first):
///   1. `--lightmap-density` CLI flag (already validated by the CLI parser:
///      finite, > 0; non-conforming values hard-reject at arg parse).
///   2. `_lightmap_density` worldspawn KVP (validated in `parse_map_file`:
///      non-finite/≤0 values are warned-and-discarded so they arrive as `None`).
///   3. `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`.
fn resolve_lightmap_density(cli: Option<f32>, kvp: Option<f32>) -> f32 {
    cli.or(kvp)
        .unwrap_or(lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS)
}

/// Prepare the shared lightmap atlas for the warm per-light layer path, applying
/// the same atlas-overflow density-halving retry the cold monolithic bake uses.
///
/// `prepare_atlas` plans charts, shelf-packs, and assigns lightmap UVs into the
/// geometry exactly as the cold bake's internal `prepare_atlas` call does; on
/// `AtlasOverflow` it doubles the texel density and re-prepares (up to
/// `MAX_RETRIES`), mirroring the cold path's loop so both modes converge on the
/// same density and chart layout. Returns the prepared atlas plus the density it
/// was prepared at (the value all downstream stages must use as authoritative).
fn prepare_lightmap_atlas_with_retry(
    geometry: &mut geometry::GeometryResult,
    static_lights: &light_namespaces::StaticBakedLights<'_>,
    start_density: f32,
) -> anyhow::Result<(lightmap_bake::PreparedAtlas, f32)> {
    const MAX_RETRIES: u32 = 3;
    let mut density = start_density;
    let mut attempt = 0;
    loop {
        match lightmap_bake::prepare_atlas(geometry, static_lights, density) {
            Ok(prepared) => return Ok((prepared, density)),
            Err(lightmap_bake::LightmapBakeError::AtlasOverflow {
                max,
                needed_w,
                needed_h,
                ..
            }) if attempt < MAX_RETRIES => {
                let next = density * 2.0;
                let retries_left = MAX_RETRIES - attempt - 1;
                log::warn!(
                    "Lightmap atlas overflow at {density} m/texel \
                     (computed {needed_w}x{needed_h} px, limit {max}x{max} px); \
                     retrying at {next} m/texel ({retries_left} retr{} remaining)",
                    if retries_left == 1 { "y" } else { "ies" }
                );
                density = next;
                attempt += 1;
            }
            Err(e) => return Err(anyhow::anyhow!("Lightmap atlas prepare failed: {e}")),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let started = Instant::now();
    let args = parse_args()?;

    let log_level = if args.verbose { "info" } else { "warn" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    if args.verbose {
        log::info!("Input: {}", args.input.display());
        log::info!("Output: {}", args.output.display());
        log::info!("Map format: {:?}", args.format);
    }

    if !args.format.is_supported() {
        anyhow::bail!("map format '{:?}' is not yet supported", args.format);
    }

    // Construct stage cache. Default dir = <workspace-root>/.build-caches/prl-cache/.
    // --no-cache and --release both disable the cache entirely (no directory is
    // created), selecting the exact ship path (exact monolithic lightmap + exact
    // whole-volume SH). --release is the intent-named equivalent of the mechanical
    // --no-cache; routing both to `None` means the warm/cold branches below need no
    // change. --cache-dir <path> overrides the default location for warm builds;
    // when --no-cache or --release is also supplied, the cache stays disabled.
    let stage_cache: Option<cache::StageCache> = if args.release || args.no_cache {
        if args.release {
            log::info!("[prl-build] release bake: exact lighting, cache bypassed");
        } else {
            log::info!("[prl-build] cache disabled via --no-cache");
        }
        None
    } else {
        let dir = args.cache_dir.clone().unwrap_or_else(|| {
            cache::find_workspace_root(args.input.as_ref())
                .unwrap_or_else(|| {
                    args.input
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .to_path_buf()
                })
                .join(".build-caches")
                .join("prl-cache")
        });
        match cache::StageCache::new(&dir) {
            Ok(c) => {
                log::info!("[prl-build] cache directory: {}", dir.display());
                Some(c)
            }
            Err(e) => {
                log::warn!(
                    "[prl-build] cache disabled: failed to create {}: {e}",
                    dir.display()
                );
                None
            }
        }
    };

    let mut timings = Vec::new();
    let mut progress = BuildProgress::new(started, args.verbose);

    progress.start_stage("Parsing map...");
    let stage_start = Instant::now();
    let map_data = parse::parse_map_file(&args.input, args.format)?;
    timings.push(("Parsing", stage_start.elapsed()));

    progress.start_stage("Data script compilation...");
    let stage_start = Instant::now();
    let data_script_section =
        compile_worldspawn_data_script(&args.input, map_data.data_script.as_deref())?;
    timings.push(("DataScript", stage_start.elapsed()));

    progress.start_stage("Texture color-space validation...");
    let stage_start = Instant::now();
    let texture_root = resolve_texture_root(&args.input);
    texture_validation::validate_sibling_color_spaces(&texture_root)?;
    timings.push(("TexValidation", stage_start.elapsed()));

    let static_baked_lights = light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
    let animated_baked_lights =
        light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
    let alpha_lights_ns = light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);

    progress.start_stage("BSP partitioning...");
    let stage_start = Instant::now();
    let result = partition::partition(&map_data.brush_volumes)?;
    timings.push(("Partitioning", stage_start.elapsed()));
    if args.verbose {
        partition::log_stats(&result.tree, &result.faces);
    }

    progress.start_stage("Visibility computation...");
    let stage_start = Instant::now();
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
    timings.push(("Visibility", stage_start.elapsed()));
    if args.verbose {
        visibility::log_stats(&vis_result, portal_count);
    }

    progress.start_stage("Geometry extraction...");
    let stage_start = Instant::now();
    let mut geo_result = geometry::extract_geometry(&result.faces, &result.tree, &exterior_leaves);
    timings.push(("Geometry", stage_start.elapsed()));
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

    progress.start_stage("BVH build...");
    let stage_start = Instant::now();
    let (bvh, bvh_primitives, bvh_section) =
        bvh_build::build_bvh(&geo_result).map_err(|e| anyhow::anyhow!("BVH build failed: {e}"))?;
    timings.push(("BVH Build", stage_start.elapsed()));
    if args.verbose {
        bvh_build::log_stats(&bvh_section);
    }

    progress.start_stage("Lightmap bake...");
    let stage_start = Instant::now();
    let static_light_count = map_data.lights.iter().filter(|l| !l.is_dynamic).count();
    let effective_lightmap_density =
        resolve_lightmap_density(args.lightmap_density, map_data.lightmap_density);
    let lightmap_config = lightmap_bake::LightmapConfig {
        lightmap_density: effective_lightmap_density,
        area_sample_count: args.soft_shadow_samples,
        uncompressed_irradiance: false,
    };
    let final_lightmap_density;
    let lightmap_bake_output = if let Some(ref cache) = stage_cache {
        // Warm path: per-light lightmap layers (Task 7). Prepare the shared atlas
        // ONCE over the full static set, then bake/load one cached layer per static
        // light and composite into the byte-identical pre-BC6H atlas. The composite
        // equals the monolithic `bake_face_chart` bit-for-bit, so the only
        // difference from the cold path is that an unchanged light's layer is
        // served from cache instead of re-baked.
        let (prepared, density) = prepare_lightmap_atlas_with_retry(
            &mut geo_result,
            &static_baked_lights,
            lightmap_config.lightmap_density,
        )?;
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
                                layer
                            }
                            None => {
                                log::info!("[cache] lightmap_layer miss");
                                let layer = lightmap_layer::bake_light_layer(
                                    light,
                                    &shared,
                                    &bvh,
                                    &bvh_primitives,
                                    &geo_result,
                                    args.soft_shadow_samples,
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
            }
        }
    } else {
        // Cold / exact path (`--no-cache`): the monolithic whole-atlas bake, the
        // shippable source of truth. No layer reads/writes.
        const MAX_RETRIES: u32 = 3;
        let mut density = lightmap_config.lightmap_density;
        let mut attempt = 0;
        loop {
            let mut lm_ctx = lightmap_bake::LightmapBakeCtx {
                bvh: &bvh,
                primitives: &bvh_primitives,
                geometry: &mut geo_result,
                lights: &static_baked_lights,
            };
            match lightmap_bake::bake_lightmap(
                &mut lm_ctx,
                &lightmap_bake::LightmapConfig {
                    lightmap_density: density,
                    area_sample_count: args.soft_shadow_samples,
                    uncompressed_irradiance: false,
                },
            ) {
                Ok(result) => {
                    final_lightmap_density = density;
                    break result;
                }
                Err(lightmap_bake::LightmapBakeError::AtlasOverflow {
                    max,
                    needed_w,
                    needed_h,
                    ..
                }) if attempt < MAX_RETRIES => {
                    let next = density * 2.0;
                    let retries_left = MAX_RETRIES - attempt - 1;
                    log::warn!(
                        "Lightmap atlas overflow at {density} m/texel \
                         (computed {needed_w}x{needed_h} px, limit {max}x{max} px); \
                         retrying at {next} m/texel ({retries_left} retr{} remaining)",
                        if retries_left == 1 { "y" } else { "ies" }
                    );
                    density = next;
                    attempt += 1;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Lightmap bake failed: {e}"));
                }
            }
        }
    };
    timings.push(("Lightmap Bake", stage_start.elapsed()));
    let lightmap_bake::LightmapBakeOutput {
        section: lightmap_section,
        charts: face_charts,
        placements: face_placements,
        atlas_width,
        atlas_height,
    } = lightmap_bake_output;
    if args.verbose {
        lightmap_bake::log_stats(&lightmap_section, static_light_count);
    }

    progress.start_stage("SH volume bake...");
    let stage_start = Instant::now();
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
    let sh_volume_section = if let Some(ref cache) = stage_cache {
        // Warm path: per-probe-group SH (Task 7). Each group bakes/loads a cached
        // entry over its probe subset with a bounded reaching-light set, then the
        // groups assemble into the volume. This is a deliberate approximation —
        // lights past the reach cutoff drop, so far-bounce regions run slightly
        // dim. Not byte-identical to the cold whole-volume bake; the cold
        // `--no-cache` build is the exact ship source of truth.
        log::warn!("{}", sh_group::WARM_SH_APPROX_WARNING);
        sh_group::bake_sh_volume_grouped(&sh_ctx, &sh_config, Some(cache))
    } else {
        // Cold / exact path (`--no-cache`): the monolithic whole-volume bake, the
        // shippable source of truth. No per-group reads/writes, no warning.
        sh_bake::bake_sh_volume(&sh_ctx, &sh_config)
    };
    timings.push(("SH Bake", stage_start.elapsed()));
    if args.verbose {
        sh_bake::log_stats(&sh_volume_section);
    }

    progress.start_stage("Delta SH volume bake...");
    let stage_start = Instant::now();
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
        delta_sh_bake::bake_delta_sh_volumes(&inputs, &sh_config)
    };
    timings.push(("Delta SH Bake", stage_start.elapsed()));
    if args.verbose {
        if let Some(ref section) = delta_sh_volumes_section {
            delta_sh_bake::log_stats(section);
        } else {
            log::info!("DeltaShVolumes: skipped (no animated lights)");
        }
    }

    progress.start_stage("Chunk light list bake...");
    let stage_start = Instant::now();
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
    timings.push(("ChunkLightList", stage_start.elapsed()));

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

    progress.start_stage("Animated light chunks...");
    let stage_start = Instant::now();
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
    timings.push(("AnimLightChunks", stage_start.elapsed()));

    progress.start_stage("Animated light weight maps...");
    let stage_start = Instant::now();
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
            Some(section)
        } else {
            log::info!("[cache] animated_lm_weight_maps miss");
            let section = animated_light_weight_maps::bake_animated_light_weight_maps(&wm_inputs);
            if let Some(ref c) = stage_cache {
                c.put(&wm_key, &section.to_bytes());
            }
            Some(section)
        }
    };
    timings.push(("AnimWeightMaps", stage_start.elapsed()));

    let animated_light_chunks_section = if animated_light_chunks_section.chunks.is_empty() {
        None
    } else {
        Some(animated_light_chunks_section)
    };

    let sdf_atlas_section = if map_needs_sdf_atlas(&map_data.lights) {
        progress.start_stage("SDF atlas bake...");
        let stage_start = Instant::now();
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
        timings.push(("SDF Atlas Bake", stage_start.elapsed()));
        if args.verbose {
            sdf_bake::log_stats(&section);
        }
        Some(section)
    } else {
        None
    };

    progress.start_stage("Texture mip bake...");
    let stage_start = Instant::now();
    let prm_cache_root = resolve_prm_cache_root_via_cargo(&args.input);
    let name_to_key = texture_mips::bake_texture_mips(
        &geo_result.texture_names.names,
        &texture_root,
        &prm_cache_root,
    )?;
    timings.push(("TextureMips", stage_start.elapsed()));

    progress.start_stage("Packing and writing...");
    let stage_start = Instant::now();

    let portals_section = pack::encode_portals(&generated_portals);
    pack::pack_and_write_portals(
        &args.output,
        &geo_result,
        &name_to_key,
        &vis_result.nodes_section,
        &vis_result.leaves_section,
        &portals_section,
        &bvh_section,
        &bvh_chunk_ranges,
        &alpha_lights_section,
        &light_influence_section,
        &sh_volume_section,
        &lightmap_section,
        &chunk_light_list_section,
        animated_light_chunks_section.as_ref(),
        animated_light_weight_maps_section.as_ref(),
        light_tags_section.as_ref(),
        delta_sh_volumes_section.as_ref(),
        data_script_section.as_ref(),
        map_entities_section.as_ref(),
        &fog_volumes_section,
        fog_cell_masks_section.as_ref(),
        sdf_atlas_section.as_ref(),
    )?;
    timings.push(("Packing", stage_start.elapsed()));

    progress.finish();

    println!("\nBuild Summary:");
    for (name, duration) in &timings {
        println!("  {: <15} {:>6.2}s", name, duration.as_secs_f32());
    }
    println!(
        "  {: <15} {:>6.2}s",
        "Total",
        started.elapsed().as_secs_f32()
    );

    Ok(())
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    verbose: bool,
    format: MapFormat,
    probe_spacing: f32,
    /// Starting density in meters; baker retries at coarser densities on atlas
    /// overflow. `None` means the flag was not passed — the effective bake
    /// density falls through to the worldspawn `_lightmap_density` KVP, then
    /// to `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`. Passing the flag
    /// overrides any KVP (`--lightmap-density` keeps its hard-reject posture
    /// on non-finite/≤0 values in the CLI parser).
    lightmap_density: Option<f32>,
    /// Soft-shadow area-sample count (penumbra escalation target). Raising it
    /// invalidates both the cached lightmap stage and the cached animated
    /// weight-map stage, triggering a re-bake of each. Default
    /// `lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT`.
    soft_shadow_samples: u32,
    /// SDF occluder-atlas voxel edge length in meters. Overrides
    /// `sdf_bake::DEFAULT_VOXEL_SIZE_METERS` for this run.
    voxel_size: f32,
    /// Override cache directory. None = use the workspace-root default.
    cache_dir: Option<PathBuf>,
    /// When true, bypass cache reads and writes entirely.
    no_cache: bool,
    /// When true, produce a shippable map: the exact ship path (exact monolithic
    /// lightmap + exact whole-volume SH). Named for intent ("I am producing a
    /// shippable map") rather than cache mechanics; it implies `--no-cache`, so
    /// the warm/cold branches need no change — both flags route the stage cache
    /// to `None`. Passing both is fine (identical effect, no conflict).
    release: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args().skip(1))
}

/// Usage text for `-h`/`--help`. Built from the live default constants so the
/// printed defaults never drift from the values the parser actually applies.
fn help_text() -> String {
    format!(
        "prl-build — Postretro level compiler (.map -> .prl)\n\
         \n\
         USAGE:\n    \
         prl-build <input.map> [-o <output.prl>] [OPTIONS]\n\
         \n\
         ARGS:\n    \
         <input.map>    Input TrenchBroom/Quake-style .map file to compile (required)\n\
         \n\
         OPTIONS:\n    \
         -o <output.prl>            Output PRL path (default: input path with a .prl extension)\n    \
         -v, --verbose              Verbose stage logging to stderr (default: off)\n    \
         --format <FORMAT>          Map source format: idtech2 | idtech3 | idtech4 (default: idtech2)\n    \
         --sh-probe-spacing <METERS> SH irradiance probe spacing in meters, > 0 (default: {probe})\n    \
         --lightmap-density <METERS> Starting lightmap texel size in meters, > 0 (default: {density})\n    \
         --soft-shadow-samples <N>  Soft-shadow penumbra area-sample count, >= {probe_floor} (default: {samples})\n    \
         --sdf-voxel-size <METERS>  SDF occluder-atlas voxel edge length in meters, > 0 (default: {voxel})\n    \
         --cache-dir <PATH>         Override the stage-cache directory (default: <workspace>/.build-caches/prl-cache)\n    \
         --no-cache                 Disable the stage cache entirely; wins over --cache-dir (default: off)\n    \
         --release                  Produce a shippable map: exact lighting, cache bypassed (implies --no-cache). The interactive default is a fast warm build with approximate indirect lighting; ship only --release artifacts (default: off)\n    \
         -h, --help                 Print this help and exit\n",
        probe = sh_bake::DEFAULT_PROBE_SPACING,
        density = lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS,
        samples = lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
        probe_floor = lightmap_bake::SOFT_PROBE_SAMPLES,
        voxel = sdf_bake::DEFAULT_VOXEL_SIZE_METERS,
    )
}

fn parse_args_from<I>(mut args: I) -> anyhow::Result<Args>
where
    I: Iterator<Item = String>,
{
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut verbose = false;
    let mut format = DEFAULT_MAP_FORMAT;
    let mut probe_spacing = sh_bake::DEFAULT_PROBE_SPACING;
    let mut lightmap_density: Option<f32> = None;
    let mut soft_shadow_samples = lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
    let mut voxel_size = sdf_bake::DEFAULT_VOXEL_SIZE_METERS;
    let mut cache_dir: Option<PathBuf> = None;
    let mut no_cache = false;
    let mut release = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", help_text());
                std::process::exit(0);
            }
            "-o" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("-o requires an output path"))?;
                output = Some(PathBuf::from(path));
            }
            "-v" | "--verbose" => {
                verbose = true;
            }
            "--format" => {
                let fmt_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--format requires a value"))?;
                format = fmt_str
                    .parse::<MapFormat>()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            "--sh-probe-spacing" => {
                let spacing_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sh-probe-spacing requires a value"))?;
                let parsed: f32 = spacing_str.parse().map_err(|_| {
                    anyhow::anyhow!("--sh-probe-spacing must be a positive number of meters")
                })?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    anyhow::bail!("--sh-probe-spacing must be a positive number of meters");
                }
                probe_spacing = parsed;
            }
            "--lightmap-density" => {
                let density_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--lightmap-density requires a value"))?;
                let parsed: f32 = density_str.parse().map_err(|_| {
                    anyhow::anyhow!("--lightmap-density must be a positive number of meters")
                })?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    anyhow::bail!("--lightmap-density must be a positive number of meters");
                }
                lightmap_density = Some(parsed);
            }
            "--soft-shadow-samples" => {
                let samples_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--soft-shadow-samples requires a value"))?;
                let parsed: u32 = samples_str.parse().map_err(|_| {
                    anyhow::anyhow!("--soft-shadow-samples must be a positive integer")
                })?;
                if parsed < lightmap_bake::SOFT_PROBE_SAMPLES {
                    anyhow::bail!(
                        "--soft-shadow-samples must be >= {} (the probe-set floor)",
                        lightmap_bake::SOFT_PROBE_SAMPLES
                    );
                }
                soft_shadow_samples = parsed;
            }
            "--sdf-voxel-size" => {
                let voxel_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sdf-voxel-size requires a value"))?;
                let parsed: f32 = voxel_str.parse().map_err(|_| {
                    anyhow::anyhow!("--sdf-voxel-size must be a positive number of meters")
                })?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    anyhow::bail!("--sdf-voxel-size must be a positive number of meters");
                }
                voxel_size = parsed;
            }
            "--cache-dir" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--cache-dir requires a path"))?;
                cache_dir = Some(PathBuf::from(path));
            }
            "--no-cache" => {
                no_cache = true;
            }
            "--release" => {
                release = true;
            }
            _ if input.is_none() => {
                input = Some(PathBuf::from(arg));
            }
            _ => {
                anyhow::bail!("unexpected argument: {arg}");
            }
        }
    }

    let input = input.ok_or_else(|| {
        anyhow::anyhow!(
            "usage: prl-build <input.map> [-o <output.prl>] [-v|--verbose] \
             [--format <FORMAT>] [--sh-probe-spacing <METERS>] [--lightmap-density <METERS>] \
             [--soft-shadow-samples <N>] [--sdf-voxel-size <METERS>] [--cache-dir <PATH>] [--no-cache] [--release]\n\
             (run `prl-build --help` for the full flag list)"
        )
    })?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args {
        input,
        output,
        verbose,
        format,
        probe_spacing,
        lightmap_density,
        soft_shadow_samples,
        voxel_size,
        cache_dir,
        no_cache,
        release,
    })
}

/// Locate the `scripts-build` sidecar for compiling worldspawn `.ts` scripts.
///
// TODO(scripting-tools-dedup): duplicates `TsCompilerPath::detect` in
// `crates/postretro/src/scripting/watcher.rs`. That module is
// `#[cfg(debug_assertions)]`-gated and lives inside the engine crate, which
// pulls in wgpu — the level-compiler can't import it. The matching mtime
// check lives in `js_is_fresh` below; the matching subprocess invocation
// lives in `run_ts_compiler` in the watcher module. Consolidate into a
// shared `postretro-scripts-tools` crate when the level-compiler gains more
// scripting integration. See:
// context/plans/drafts/scripting-tools-dedup/index.md
fn find_scripts_build() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = &exe_dir {
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path_var = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        "scripts-build.exe"
    } else {
        "scripts-build"
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Compile the worldspawn `data_script`, if present, and return the
/// `DataScriptSection` to embed in the PRL.
///
/// Behavior matrix:
/// - `path == None` → returns `Ok(None)`; no section is emitted.
/// - source file missing → hard error (no `.js` fallback).
/// - `.luau` source → read raw bytes, no compilation.
/// - `.ts`/`.js` source → compile via `scripts-build` (or fall back to a
///   freshly-modified sibling `.js` when the compiler is absent), then read
///   the resulting `.js` bytes.
///
/// The stored `source_path` is the resolved absolute path captured at compile
/// time, reserved for the future hot-reload watcher.
fn compile_worldspawn_data_script(
    map_path: &std::path::Path,
    data_script_path: Option<&str>,
) -> anyhow::Result<Option<postretro_level_format::data_script::DataScriptSection>> {
    let Some(rel) = data_script_path else {
        return Ok(None);
    };

    let map_dir = map_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let source_path = map_dir.join(rel);

    if !source_path.is_file() {
        anyhow::bail!(
            "[prl-build] data_script = {rel} resolves to {} which does not exist",
            source_path.display()
        );
    }

    let extension = source_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());

    let compiled_bytes = match extension.as_deref() {
        Some("luau") => {
            log::info!(
                "[prl-build] embedding Luau data script {} (no compilation)",
                source_path.display()
            );
            std::fs::read(&source_path).map_err(|e| {
                anyhow::anyhow!(
                    "[prl-build] failed to read data_script {}: {e}",
                    source_path.display()
                )
            })?
        }
        Some("ts") | Some("js") => {
            let js_path = source_path.with_extension("js");
            // For `.js` source `js_path == source_path`; the mtime check passes
            // trivially and we just read bytes back — no compile needed.
            let needs_compile = extension.as_deref() == Some("ts")
                && !matches!(js_is_fresh(&source_path, &js_path), Some(true));

            if needs_compile {
                match find_scripts_build() {
                    Some(compiler) => {
                        log::info!(
                            "[prl-build] compiling data_script {} -> {} via {}",
                            source_path.display(),
                            js_path.display(),
                            compiler.display()
                        );
                        let out = std::process::Command::new(&compiler)
                            .arg("--in")
                            .arg(&source_path)
                            .arg("--out")
                            .arg(&js_path)
                            .output()
                            .map_err(|e| {
                                anyhow::anyhow!(
                                    "[prl-build] failed to spawn scripts-build at {}: {e}",
                                    compiler.display()
                                )
                            })?;
                        if !out.status.success() {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            if !stderr.trim().is_empty() {
                                eprintln!("[prl-build] scripts-build stderr:\n{stderr}");
                            }
                            if !stdout.trim().is_empty() {
                                eprintln!("[prl-build] scripts-build stdout:\n{stdout}");
                            }
                            anyhow::bail!(
                                "[prl-build] scripts-build failed for data_script {}: exit status {}",
                                source_path.display(),
                                out.status
                            );
                        }
                    }
                    None => {
                        if !js_path.is_file() {
                            anyhow::bail!(
                                "[prl-build] data_script = {rel} but scripts-build was not found and no compiled .js artifact exists beside the .ts file. Install scripts-build or ship it next to prl-build."
                            );
                        }
                        log::warn!(
                            "[prl-build] scripts-build not found; embedding existing compiled data_script artifact {}",
                            js_path.display()
                        );
                    }
                }
            }

            std::fs::read(&js_path).map_err(|e| {
                anyhow::anyhow!(
                    "[prl-build] failed to read compiled data_script {}: {e}",
                    js_path.display()
                )
            })?
        }
        Some(other) => {
            anyhow::bail!(
                "[prl-build] data_script = {rel} has unsupported extension '.{other}' (expected .ts, .js, or .luau)"
            );
        }
        None => {
            anyhow::bail!(
                "[prl-build] data_script = {rel} has no file extension (expected .ts, .js, or .luau)"
            );
        }
    };

    let absolute_source_path = std::fs::canonicalize(&source_path)
        .unwrap_or(source_path.clone())
        .to_string_lossy()
        .into_owned();

    log::info!(
        "[prl-build] data_script embedded: {} bytes from {}",
        compiled_bytes.len(),
        absolute_source_path
    );

    Ok(Some(pack::encode_data_script(
        compiled_bytes,
        absolute_source_path,
    )))
}

/// `>` not `>=`: equal mtimes (same-second write) must trigger recompilation.
/// mtime is unreliable after `git checkout` and on network filesystems — this
/// is best-effort, not a correctness gate.
// TODO(scripting-tools-dedup): mirrors `compile_start_script_if_stale`'s
// freshness check in `crates/postretro/src/scripting/runtime.rs`. See the
// TODO above `find_scripts_build` for the consolidation plan.
fn js_is_fresh(ts_path: &std::path::Path, js_path: &std::path::Path) -> Option<bool> {
    if !js_path.is_file() {
        return Some(false);
    }
    let ts_mtime = std::fs::metadata(ts_path).ok()?.modified().ok()?;
    let js_mtime = std::fs::metadata(js_path).ok()?.modified().ok()?;
    Some(js_mtime > ts_mtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_basic() {
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.input, PathBuf::from("input.map"));
        assert_eq!(parsed.output, PathBuf::from("input.prl"));
        assert!(!parsed.verbose);
        assert_eq!(parsed.format, MapFormat::IdTech2);
        assert_eq!(parsed.probe_spacing, sh_bake::DEFAULT_PROBE_SPACING);
        assert_eq!(parsed.voxel_size, sdf_bake::DEFAULT_VOXEL_SIZE_METERS);
    }

    #[test]
    fn parse_args_voxel_size() {
        let args = vec![
            "input.map".to_string(),
            "--sdf-voxel-size".to_string(),
            "0.25".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.voxel_size, 0.25);
    }

    #[test]
    fn parse_args_voxel_size_rejects_non_positive() {
        let args = vec![
            "input.map".to_string(),
            "--sdf-voxel-size".to_string(),
            "0".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());

        let args = vec![
            "input.map".to_string(),
            "--sdf-voxel-size".to_string(),
            "-1".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_voxel_size_rejects_non_finite() {
        let args = vec![
            "input.map".to_string(),
            "--sdf-voxel-size".to_string(),
            "nan".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_voxel_size_requires_value() {
        let args = vec!["input.map".to_string(), "--sdf-voxel-size".to_string()];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_verbose_flag() {
        let args = vec!["input.map".to_string(), "-v".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.verbose);

        let args = vec!["input.map".to_string(), "--verbose".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.verbose);
    }

    #[test]
    fn parse_args_probe_spacing() {
        let args = vec![
            "input.map".to_string(),
            "--sh-probe-spacing".to_string(),
            "0.5".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.probe_spacing, 0.5);
    }

    #[test]
    fn parse_args_probe_spacing_rejects_non_positive() {
        let args = vec![
            "input.map".to_string(),
            "--sh-probe-spacing".to_string(),
            "0".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());

        let args = vec![
            "input.map".to_string(),
            "--sh-probe-spacing".to_string(),
            "-1".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_probe_spacing_requires_value() {
        let args = vec!["input.map".to_string(), "--sh-probe-spacing".to_string()];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_with_output() {
        let args = vec![
            "input.map".to_string(),
            "-o".to_string(),
            "out.prl".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.output, PathBuf::from("out.prl"));
    }

    // resolve_lightmap_density precedence: CLI > KVP > default. The CLI flag's
    // own validation lives in `parse_args_from`; the KVP's lives in
    // `parse_map_file`. This resolver only composes the two precedences.

    #[test]
    fn resolve_lightmap_density_uses_default_when_neither_set() {
        let d = resolve_lightmap_density(None, None);
        assert_eq!(d, lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS);
    }

    #[test]
    fn resolve_lightmap_density_uses_kvp_when_cli_absent() {
        let d = resolve_lightmap_density(None, Some(0.02));
        assert_eq!(d, 0.02);
    }

    #[test]
    fn resolve_lightmap_density_cli_overrides_kvp() {
        let d = resolve_lightmap_density(Some(0.01), Some(0.02));
        assert_eq!(
            d, 0.01,
            "CLI --lightmap-density must override the worldspawn `_lightmap_density` KVP"
        );
    }

    #[test]
    fn resolve_lightmap_density_cli_overrides_default() {
        let d = resolve_lightmap_density(Some(0.08), None);
        assert_eq!(d, 0.08);
    }

    #[test]
    fn parse_args_lightmap_density_unset_is_none() {
        // Without --lightmap-density on the command line, Args carries None so
        // the resolver can fall through to the KVP / default.
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.lightmap_density, None);
    }

    #[test]
    fn parse_args_lightmap_density_set_is_some() {
        let args = vec![
            "input.map".to_string(),
            "--lightmap-density".to_string(),
            "0.03".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.lightmap_density, Some(0.03));
    }

    #[test]
    fn parse_args_pvs_flag_rejected() {
        let args = vec!["input.map".to_string(), "--pvs".to_string()];
        assert!(
            parse_args_from(args.into_iter()).is_err(),
            "--pvs is retired and must be rejected"
        );
    }

    #[test]
    fn parse_args_rejects_unknown_flags() {
        let args = vec!["input.map".to_string(), "--bsp".to_string()];
        let result = parse_args_from(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn parse_args_format_idtech2() {
        let args = vec![
            "input.map".to_string(),
            "--format".to_string(),
            "idtech2".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.format, MapFormat::IdTech2);
    }

    #[test]
    fn parse_args_format_idtech3() {
        let args = vec![
            "input.map".to_string(),
            "--format".to_string(),
            "idtech3".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.format, MapFormat::IdTech3);
    }

    #[test]
    fn parse_args_format_rejects_unknown() {
        let args = vec![
            "input.map".to_string(),
            "--format".to_string(),
            "bogus".to_string(),
        ];
        let result = parse_args_from(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown map format"), "got: {msg}");
    }

    #[test]
    fn parse_args_format_requires_value() {
        let args = vec!["input.map".to_string(), "--format".to_string()];
        let result = parse_args_from(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn parse_args_no_cache_flag() {
        let args = vec!["input.map".to_string(), "--no-cache".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.no_cache);
    }

    #[test]
    fn parse_args_cache_dir_flag() {
        let args = vec![
            "input.map".to_string(),
            "--cache-dir".to_string(),
            "/tmp/my-cache".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.cache_dir, Some(PathBuf::from("/tmp/my-cache")));
    }

    #[test]
    fn parse_args_cache_dir_requires_value() {
        let args = vec!["input.map".to_string(), "--cache-dir".to_string()];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_no_cache_defaults() {
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(!parsed.no_cache);
        assert!(parsed.cache_dir.is_none());
    }

    #[test]
    fn parse_args_release_flag() {
        let args = vec!["input.map".to_string(), "--release".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.release);
    }

    #[test]
    fn parse_args_release_defaults_unset() {
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(!parsed.release);
    }

    /// `--release` routes the stage cache to `None` exactly like `--no-cache`,
    /// selecting the exact ship path. The cache-selection predicate is
    /// `args.release || args.no_cache`; assert release alone satisfies it.
    #[test]
    fn parse_args_release_implies_no_cache_selection() {
        let args = vec!["input.map".to_string(), "--release".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        // `--release` need not set `no_cache` itself; the cache predicate keys on
        // either flag, so the observable (cache bypassed) holds.
        assert!(
            parsed.release || parsed.no_cache,
            "release must bypass the stage cache like no-cache"
        );
    }

    /// `--release` and `--no-cache` together parse without error (identical
    /// effect, no conflict).
    #[test]
    fn parse_args_release_and_no_cache_coexist() {
        let args = vec![
            "input.map".to_string(),
            "--release".to_string(),
            "--no-cache".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.release);
        assert!(parsed.no_cache);
    }

    #[test]
    fn parse_args_soft_shadow_samples() {
        // At-floor value is accepted.
        let floor = lightmap_bake::SOFT_PROBE_SAMPLES;
        let args = vec![
            "input.map".to_string(),
            "--soft-shadow-samples".to_string(),
            floor.to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.soft_shadow_samples, floor);

        // Above-floor value is accepted.
        let args = vec![
            "input.map".to_string(),
            "--soft-shadow-samples".to_string(),
            "16".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.soft_shadow_samples, 16);

        // Below-floor values (1–3) are rejected.
        for below in 1..floor {
            let args = vec![
                "input.map".to_string(),
                "--soft-shadow-samples".to_string(),
                below.to_string(),
            ];
            assert!(
                parse_args_from(args.into_iter()).is_err(),
                "--soft-shadow-samples {below} should be rejected (below probe floor {floor})"
            );
        }
    }

    #[test]
    fn data_script_absent_kvp_emits_no_section() {
        let result = compile_worldspawn_data_script(Path::new("/dev/null/fake.map"), None)
            .expect("None KVP must succeed");
        assert!(
            result.is_none(),
            "absent data_script KVP must not emit a DataScript section"
        );
    }

    #[test]
    fn data_script_missing_file_is_hard_error() {
        let tmp_dir = std::env::temp_dir().join("postretro_data_script_missing");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let map_path = tmp_dir.join("test.map");
        let _ = std::fs::write(&map_path, "");
        let result = compile_worldspawn_data_script(&map_path, Some("does-not-exist.ts"));
        assert!(
            result.is_err(),
            "missing data_script file must be a compile error"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("does not exist"),
            "error should mention the missing file, got: {msg}"
        );
    }

    #[test]
    fn data_script_luau_passes_through() {
        let tmp_dir = std::env::temp_dir().join("postretro_data_script_luau");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let map_path = tmp_dir.join("test.map");
        let _ = std::fs::write(&map_path, "");
        let luau_path = tmp_dir.join("level-data.luau");
        let luau_source = "return { foo = 1 }";
        std::fs::write(&luau_path, luau_source).unwrap();

        let section = compile_worldspawn_data_script(&map_path, Some("level-data.luau"))
            .expect("luau data_script should compile")
            .expect("section must be emitted");

        assert_eq!(section.compiled_bytes, luau_source.as_bytes());
        assert!(
            section.source_path.ends_with("level-data.luau"),
            "source_path should reference the .luau file, got: {}",
            section.source_path
        );
    }

    // The per-light lightmap-layer and per-group SH cache wiring is exercised by
    // the unit tests in `lightmap_layer.rs` and `sh_group.rs` (round-trip skip,
    // light-edit locality, corruption recovery). These remaining tests cover the
    // CLI surface and the content-driven SDF gating predicate.

    use crate::map_data::{FalloffModel, LightType, MapLight};
    use glam::DVec3;

    fn baseline_point_light() -> MapLight {
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    /// Content-driven SDF gating: a map with any `sdf`-typed light bakes the
    /// occluder atlas; a map with none does not. Pins the predicate that
    /// replaced the retired `--bake-sdf` flag.
    #[test]
    fn sdf_atlas_gated_on_sdf_typed_light_presence() {
        // No lights → no atlas.
        assert!(!map_needs_sdf_atlas(&[]));

        // Only `static_light_map` lights → no atlas.
        let static_only = vec![baseline_point_light(), baseline_point_light()];
        assert!(
            !map_needs_sdf_atlas(&static_only),
            "a map with no sdf-typed light must not bake the SDF atlas",
        );

        // At least one `sdf` light → atlas bakes.
        let mut sdf_light = baseline_point_light();
        sdf_light.shadow_type = crate::map_data::ShadowType::Sdf;
        let mixed = vec![baseline_point_light(), sdf_light];
        assert!(
            map_needs_sdf_atlas(&mixed),
            "a map with any sdf-typed light must bake the SDF atlas",
        );
    }
}
