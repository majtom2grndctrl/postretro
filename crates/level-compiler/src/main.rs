// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bc5;
pub mod bvh_build;
pub mod cache;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_sh_bake;
pub mod fog_cell_masks;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod light_namespaces;
pub mod lightmap_bake;
pub mod map_data;
pub mod map_format;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod sdf_bake;
pub mod sh_bake;
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
    // --no-cache disables the cache entirely (no directory is created).
    // --cache-dir <path> overrides the default location. When both flags are
    // supplied, --no-cache wins.
    let stage_cache: Option<cache::StageCache> = if args.no_cache {
        log::info!("[prl-build] cache disabled via --no-cache");
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
    let bake_mode = if args.unshadowed_lightmap {
        lightmap_bake::BakeMode::Unshadowed
    } else {
        lightmap_bake::BakeMode::Shadowed
    };
    let lightmap_config = lightmap_bake::LightmapConfig {
        lightmap_density: args.lightmap_density,
        mode: bake_mode,
    };
    let final_lightmap_density;
    let lightmap_bake_output = {
        // Build serializable inputs for cache key derivation.
        // Clone geo_result BEFORE bake mutations (split_shared_vertices + UV writes).
        let lm_inputs = lightmap_bake::LightmapInputs {
            lights: static_baked_lights
                .entries()
                .iter()
                .map(|e| e.light.clone())
                .collect(),
            geometry: geo_result.clone(), // cloned before bake mutations alter vertex order
        };
        let lm_input_hash = {
            let mut buf =
                postcard::to_allocvec(&lm_inputs).expect("postcard serialize LightmapInputs");
            buf.extend_from_slice(
                &postcard::to_allocvec(&lightmap_config)
                    .expect("postcard serialize LightmapConfig"),
            );
            *blake3::hash(&buf).as_bytes()
        };
        let lm_key = cache::CacheKey::new("lightmap", lightmap_bake::STAGE_VERSION, &lm_input_hash);

        // Cache lookup
        let cached = stage_cache.as_ref().and_then(|c| c.get(&lm_key));

        let cached_section = cached.and_then(|bytes| {
            postretro_level_format::lightmap::LightmapSection::from_bytes(&bytes)
                .map_err(|e| log::warn!("[cache] corrupt lightmap entry, re-baking: {e}"))
                .ok()
        });

        if let Some(section) = cached_section {
            log::info!("[cache] lightmap hit");
            let density = section.texel_density;
            final_lightmap_density = density;
            let atlas =
                lightmap_bake::prepare_atlas(&mut geo_result, &static_baked_lights, density)
                    .map_err(|e| {
                        anyhow::anyhow!("lightmap atlas re-prepare failed on cache hit: {e}")
                    })?;
            lightmap_bake::LightmapBakeOutput {
                section,
                charts: atlas.charts,
                placements: atlas.placements,
                atlas_width: atlas.atlas_width,
                atlas_height: atlas.atlas_height,
            }
        } else {
            log::info!("[cache] lightmap miss");
            // Retry on atlas overflow: doubles texel size (halves resolution) up to
            // MAX_RETRIES times. Degrades quality instead of failing the build.
            // Per-face planar unwrap wastes atlas area, so large maps hit this often.
            const MAX_RETRIES: u32 = 3;
            let mut density = lightmap_config.lightmap_density;
            let mut attempt = 0;
            let output = loop {
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
                        mode: bake_mode,
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
            };
            // Only cache a successful result.
            if let Some(ref c) = stage_cache {
                c.put(&lm_key, &output.section.to_bytes());
            }
            output
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
    let sh_volume_section = {
        // Build serializable ShInputs for cache key derivation.
        let mut exterior_leaves_sorted: Vec<usize> = exterior_leaves.iter().copied().collect();
        exterior_leaves_sorted.sort(); // sort required: HashSet iteration order is non-deterministic
        let sh_inputs = sh_bake::ShInputs {
            static_lights: static_baked_lights
                .entries()
                .iter()
                .map(|e| e.light.clone())
                .collect(),
            animated_lights: animated_baked_lights
                .entries()
                .iter()
                .map(|e| e.light.clone())
                .collect(),
            geometry: geo_result.clone(), // cloned before bake mutations alter vertex order
            exterior_leaves: exterior_leaves_sorted,
        };
        let sh_input_hash = {
            let mut buf = postcard::to_allocvec(&sh_inputs).expect("postcard serialize ShInputs");
            buf.extend_from_slice(
                &postcard::to_allocvec(&sh_config).expect("postcard serialize ShConfig"),
            );
            *blake3::hash(&buf).as_bytes()
        };
        let sh_key = cache::CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &sh_input_hash);

        let cached = stage_cache.as_ref().and_then(|c| c.get(&sh_key));
        let cached_sh_section = cached.and_then(|bytes| {
            postretro_level_format::sh_volume::ShVolumeSection::from_bytes(&bytes)
                .map_err(|e| log::warn!("[cache] corrupt sh_volume entry, re-baking: {e}"))
                .ok()
        });

        if let Some(section) = cached_sh_section {
            log::info!("[cache] sh_volume hit");
            section
        } else {
            log::info!("[cache] sh_volume miss");
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
            let section = sh_bake::bake_sh_volume(&sh_ctx, &sh_config);
            if let Some(ref c) = stage_cache {
                c.put(&sh_key, &section.to_bytes());
            }
            section
        }
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
        };
        Some(animated_light_weight_maps::bake_animated_light_weight_maps(
            &wm_inputs,
        ))
    };
    timings.push(("AnimWeightMaps", stage_start.elapsed()));

    let animated_light_chunks_section = if animated_light_chunks_section.chunks.is_empty() {
        None
    } else {
        Some(animated_light_chunks_section)
    };

    let sdf_atlas_section = if args.bake_sdf {
        progress.start_stage("SDF atlas bake...");
        let stage_start = Instant::now();
        let sdf_config = sdf_bake::SdfConfig::default();
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
    /// Starting density in meters; baker retries at coarser densities on atlas overflow.
    lightmap_density: f32,
    /// When true, skip the per-texel visibility test during the lightmap bake so the
    /// atlas carries full static-light irradiance + bounce with no baked shadows.
    /// Default (false) reproduces `main`'s shadowed bake byte-for-byte.
    unshadowed_lightmap: bool,
    /// When true, run the SDF static-occluder atlas bake and emit
    /// `SectionId::SdfAtlas`. Default false — leaving the section absent
    /// preserves byte-identity with main for the same map.
    bake_sdf: bool,
    /// Override cache directory. None = use the workspace-root default.
    cache_dir: Option<PathBuf>,
    /// When true, bypass cache reads and writes entirely.
    no_cache: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    parse_args_from(std::env::args().skip(1))
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
    let mut lightmap_density = lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS;
    let mut unshadowed_lightmap = false;
    let mut bake_sdf = false;
    let mut cache_dir: Option<PathBuf> = None;
    let mut no_cache = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            "--probe-spacing" => {
                let spacing_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--probe-spacing requires a value"))?;
                let parsed: f32 = spacing_str.parse().map_err(|_| {
                    anyhow::anyhow!("--probe-spacing must be a positive number of meters")
                })?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    anyhow::bail!("--probe-spacing must be a positive number of meters");
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
                lightmap_density = parsed;
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
            "--unshadowed-lightmap" => {
                unshadowed_lightmap = true;
            }
            "--bake-sdf" => {
                bake_sdf = true;
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
             [--format <FORMAT>] [--probe-spacing <METERS>] [--lightmap-density <METERS>] \
             [--unshadowed-lightmap] [--bake-sdf] [--cache-dir <PATH>] [--no-cache]"
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
        unshadowed_lightmap,
        bake_sdf,
        cache_dir,
        no_cache,
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
            "--probe-spacing".to_string(),
            "0.5".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.probe_spacing, 0.5);
    }

    #[test]
    fn parse_args_probe_spacing_rejects_non_positive() {
        let args = vec![
            "input.map".to_string(),
            "--probe-spacing".to_string(),
            "0".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());

        let args = vec![
            "input.map".to_string(),
            "--probe-spacing".to_string(),
            "-1".to_string(),
        ];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_probe_spacing_requires_value() {
        let args = vec!["input.map".to_string(), "--probe-spacing".to_string()];
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

    // --- Build-stage cache integration tests (Task 7) ---
    //
    // These tests live alongside the parse_args tests because the level
    // compiler is a binary crate (no `[lib]` target), so a separate
    // `tests/` integration file cannot `use` the in-crate modules. They
    // exercise the end-to-end cache flow at the key-derivation +
    // StageCache layer, matching the exact computation used in `main`.

    use crate::cache::{CacheKey, StageCache};
    use crate::geometry::{FaceIndexRange, GeometryResult};
    use crate::lightmap_bake::{LightmapConfig, LightmapInputs};
    use crate::map_data::{FalloffModel, LightType, MapLight};
    use crate::partition::{Aabb, BspLeaf, BspTree};
    use crate::sh_bake::{ShConfig, ShInputs};
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique per-test temp directory under the OS temp dir. Mirrors the
    /// `fresh_temp_dir` helper inside `cache.rs` to avoid the extra
    /// `tempfile` dep for a handful of tests.
    fn fresh_cache_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_cache_int_{label}_{stamp}_{nonce}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Single-quad geometry — matches the shape used by determinism tests in
    /// `lightmap_bake.rs` / `sh_bake.rs`. Sufficient for hashing because the
    /// cache key only depends on the serialized bytes, not on the bake
    /// running to completion.
    fn minimal_geometry() -> GeometryResult {
        let v = |p: [f32; 3], uv: [f32; 2]| {
            Vertex::new(p, uv, [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], true, [0.0, 0.0])
        };
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    v([0.0, 0.0, 0.0], [0.0, 0.0]),
                    v([1.0, 0.0, 0.0], [1.0, 0.0]),
                    v([1.0, 0.0, 1.0], [1.0, 1.0]),
                    v([0.0, 0.0, 1.0], [0.0, 1.0]),
                ],
                indices: vec![0, 1, 2, 0, 2, 3],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            }],
        }
    }

    fn baseline_point_light() -> MapLight {
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
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
        }
    }

    /// Replicates the lightmap key derivation in `main`: postcard
    /// `LightmapInputs` and `LightmapConfig`, concatenate, blake3.
    fn lightmap_input_hash(inputs: &LightmapInputs, config: &LightmapConfig) -> [u8; 32] {
        let mut buf = postcard::to_allocvec(inputs).expect("postcard serialize LightmapInputs");
        buf.extend_from_slice(
            &postcard::to_allocvec(config).expect("postcard serialize LightmapConfig"),
        );
        *blake3::hash(&buf).as_bytes()
    }

    /// Replicates the SH key derivation in `main`: postcard `ShInputs`
    /// and `ShConfig`, concatenate, blake3.
    fn sh_input_hash(inputs: &ShInputs, config: &ShConfig) -> [u8; 32] {
        let mut buf = postcard::to_allocvec(inputs).expect("postcard serialize ShInputs");
        buf.extend_from_slice(&postcard::to_allocvec(config).expect("postcard serialize ShConfig"));
        *blake3::hash(&buf).as_bytes()
    }

    #[test]
    fn lightmap_cache_key_matches_on_identical_inputs() {
        let dir = fresh_cache_dir("lm_roundtrip");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let inputs = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: minimal_geometry(),
        };
        let config = LightmapConfig {
            lightmap_density: 0.25,
            mode: lightmap_bake::BakeMode::Shadowed,
        };
        let hash = lightmap_input_hash(&inputs, &config);
        let key = CacheKey::new("lightmap", lightmap_bake::STAGE_VERSION, &hash);

        // Run 1: first lookup must miss; then the stage would bake and `put`.
        assert!(
            cache.get(&key).is_none(),
            "fresh cache must miss before the first bake"
        );
        let payload = b"lightmap-section-bytes-stand-in".to_vec();
        cache.put(&key, &payload);

        // Run 2: identical inputs must hit the cache and return the same bytes.
        let inputs_again = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: minimal_geometry(),
        };
        let hash_again = lightmap_input_hash(&inputs_again, &config);
        let key_again = CacheKey::new("lightmap", lightmap_bake::STAGE_VERSION, &hash_again);
        let loaded = cache
            .get(&key_again)
            .expect("identical inputs must hit the cache");
        assert_eq!(loaded, payload);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sh_volume_cache_key_matches_on_identical_inputs() {
        let dir = fresh_cache_dir("sh_roundtrip");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let inputs = ShInputs {
            static_lights: vec![baseline_point_light()],
            animated_lights: Vec::new(),
            geometry: minimal_geometry(),
            exterior_leaves: Vec::new(),
        };
        let config = ShConfig { probe_spacing: 1.0 };
        let hash = sh_input_hash(&inputs, &config);
        let key = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &hash);

        assert!(
            cache.get(&key).is_none(),
            "fresh cache must miss before the first bake"
        );
        let payload = b"sh-volume-section-bytes-stand-in".to_vec();
        cache.put(&key, &payload);

        let inputs_again = ShInputs {
            static_lights: vec![baseline_point_light()],
            animated_lights: Vec::new(),
            geometry: minimal_geometry(),
            exterior_leaves: Vec::new(),
        };
        let hash_again = sh_input_hash(&inputs_again, &config);
        let key_again = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &hash_again);
        let loaded = cache
            .get(&key_again)
            .expect("identical inputs must hit the cache");
        assert_eq!(loaded, payload);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sh_volume_stage_version_bump_misses_then_hits() {
        // Anchors the contract that bumping `sh_bake::STAGE_VERSION` is what
        // invalidates the prior `sh_volume` cache entry (the depth-moment bake
        // rides this version bump). A stale entry written under the previous
        // version must not be served under the current one; the current version
        // then exhibits the normal miss → bake/put → hit sequence.
        let dir = fresh_cache_dir("sh_stage_bump");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let inputs = ShInputs {
            static_lights: vec![baseline_point_light()],
            animated_lights: Vec::new(),
            geometry: minimal_geometry(),
            exterior_leaves: Vec::new(),
        };
        let config = ShConfig { probe_spacing: 1.0 };
        let hash = sh_input_hash(&inputs, &config);

        // A pre-bump entry baked by the previous SH algorithm version.
        let stale_key = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION - 1, &hash);
        cache.put(&stale_key, b"sh-volume-baked-by-old-algorithm");

        // Same inputs, current version: the version is folded into the key, so
        // the stale entry must not be reachable — the first build is a miss.
        let current_key = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &hash);
        assert_ne!(
            stale_key.as_filename(),
            current_key.as_filename(),
            "a STAGE_VERSION bump must change the sh_volume cache key",
        );
        assert!(
            cache.get(&current_key).is_none(),
            "first build after a STAGE_VERSION bump must miss and rebake",
        );

        // The rebake stores the moment-bearing section; the second build hits.
        let rebaked = b"sh-volume-with-depth-moments".to_vec();
        cache.put(&current_key, &rebaked);
        let loaded = cache
            .get(&current_key)
            .expect("second build under the bumped version must hit the cache");
        assert_eq!(loaded, rebaked);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_misses_when_light_changes() {
        let dir = fresh_cache_dir("lm_light_change");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let baseline = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: minimal_geometry(),
        };
        let config = LightmapConfig {
            lightmap_density: 0.25,
            mode: lightmap_bake::BakeMode::Shadowed,
        };
        let key_a = CacheKey::new(
            "lightmap",
            lightmap_bake::STAGE_VERSION,
            &lightmap_input_hash(&baseline, &config),
        );
        cache.put(&key_a, b"baseline-section-stand-in");

        // Edit the light: intensity bump must produce a different cache key.
        let mut changed_light = baseline_point_light();
        changed_light.intensity = 2.0;
        let edited = LightmapInputs {
            lights: vec![changed_light],
            geometry: minimal_geometry(),
        };
        let key_b = CacheKey::new(
            "lightmap",
            lightmap_bake::STAGE_VERSION,
            &lightmap_input_hash(&edited, &config),
        );
        assert_ne!(
            key_a.as_filename(),
            key_b.as_filename(),
            "changing a light must change the cache key"
        );
        assert!(
            cache.get(&key_b).is_none(),
            "edited light must miss the cache"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_key_stable_across_non_bake_fields() {
        // `LightmapInputs` only carries lights + geometry — not fog_pixel_scale,
        // initial_gravity, or any other worldspawn metadata. So editing a
        // worldspawn property no baked stage reads must leave the cache key
        // unchanged. We model that by hashing two identical `LightmapInputs`
        // values (the unrelated property literally cannot leak into the key
        // because it has no field to land in) and asserting both keys match.
        let dir = fresh_cache_dir("lm_irrelevant_edit");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let inputs_first = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: minimal_geometry(),
        };
        let inputs_second = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: minimal_geometry(),
        };
        let config = LightmapConfig {
            lightmap_density: 0.25,
            mode: lightmap_bake::BakeMode::Shadowed,
        };

        let key_first = CacheKey::new(
            "lightmap",
            lightmap_bake::STAGE_VERSION,
            &lightmap_input_hash(&inputs_first, &config),
        );
        let key_second = CacheKey::new(
            "lightmap",
            lightmap_bake::STAGE_VERSION,
            &lightmap_input_hash(&inputs_second, &config),
        );
        assert_eq!(
            key_first.as_filename(),
            key_second.as_filename(),
            "identical lightmap inputs must produce identical keys",
        );

        let payload = b"placeholder-lightmap-bytes".to_vec();
        cache.put(&key_first, &payload);
        let loaded = cache
            .get(&key_second)
            .expect("non-bake-input edit must keep cache hit");
        assert_eq!(loaded, payload);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_version_bump_invalidates_cache() {
        let dir = fresh_cache_dir("stage_version_bump");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let input_hash = [0x42u8; 32];
        let key_v1 = CacheKey::new("lightmap", 1, &input_hash);
        let key_v2 = CacheKey::new("lightmap", 2, &input_hash);

        cache.put(&key_v1, b"baked-with-old-algorithm");

        assert_ne!(
            key_v1.as_filename(),
            key_v2.as_filename(),
            "stage version is folded into the filename digest, so bumping it must change the key",
        );
        assert!(
            cache.get(&key_v2).is_none(),
            "a stage version bump must invalidate prior entries",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lightmap_cache_hit_returns_byte_identical_section() {
        let dir = fresh_cache_dir("lm_real_bake");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let mut geo = minimal_geometry();
        let (bvh, prims, _) = bvh_build::build_bvh(&geo).expect("bvh build must succeed");
        let light = baseline_point_light();
        let static_lights =
            light_namespaces::StaticBakedLights::from_lights(std::slice::from_ref(&light));

        let inputs = LightmapInputs {
            lights: vec![baseline_point_light()],
            geometry: geo.clone(), // snapshot before bake mutations alter lightmap UVs
        };
        let config = LightmapConfig {
            lightmap_density: 0.25,
            mode: lightmap_bake::BakeMode::Shadowed,
        };
        let hash = lightmap_input_hash(&inputs, &config);
        let key = CacheKey::new("lightmap", lightmap_bake::STAGE_VERSION, &hash);

        let mut ctx = lightmap_bake::LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo, // same instance the BVH was built from
            lights: &static_lights,
        };
        let output = lightmap_bake::bake_lightmap(&mut ctx, &config).expect("bake must succeed");

        let baked_bytes = output.section.to_bytes();
        cache.put(&key, &baked_bytes);

        let loaded_bytes = cache.get(&key).expect("cache must hit after put");
        let section = postretro_level_format::lightmap::LightmapSection::from_bytes(&loaded_bytes)
            .expect("deserialization must succeed");
        let round_tripped = section.to_bytes();
        assert_eq!(
            baked_bytes, round_tripped,
            "cache round-trip must be byte-identical"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sh_volume_cache_hit_returns_byte_identical_section() {
        let dir = fresh_cache_dir("sh_real_bake");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let geo = minimal_geometry();
        let (bvh, prims, _) = bvh_build::build_bvh(&geo).expect("bvh build must succeed");
        let light = baseline_point_light();
        let static_lights =
            light_namespaces::StaticBakedLights::from_lights(std::slice::from_ref(&light));
        let animated_lights = light_namespaces::AnimatedBakedLights::from_lights(&[]);

        let tree = BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: Aabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            }],
        };

        let inputs = ShInputs {
            static_lights: vec![baseline_point_light()],
            animated_lights: Vec::new(),
            geometry: minimal_geometry(),
            exterior_leaves: Vec::new(),
        };
        let config = ShConfig { probe_spacing: 1.0 };
        let hash = sh_input_hash(&inputs, &config);
        let key = CacheKey::new("sh_volume", sh_bake::STAGE_VERSION, &hash);

        let exterior: HashSet<usize> = HashSet::new();
        let ctx = sh_bake::ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = sh_bake::bake_sh_volume(&ctx, &config);

        let baked_bytes = section.to_bytes();
        cache.put(&key, &baked_bytes);

        let loaded_bytes = cache.get(&key).expect("cache must hit after put");
        let section2 =
            postretro_level_format::sh_volume::ShVolumeSection::from_bytes(&loaded_bytes)
                .expect("deserialization must succeed");
        let round_tripped = section2.to_bytes();
        assert_eq!(
            baked_bytes, round_tripped,
            "sh volume cache round-trip must be byte-identical"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
