// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bvh_build;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_sh_bake;
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
pub mod sh_bake;
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

    let mut timings = Vec::new();
    let mut progress = BuildProgress::new(started, args.verbose);

    progress.start_stage("Parsing map...");
    let stage_start = Instant::now();
    let map_data = parse::parse_map_file(&args.input, args.format)?;
    timings.push(("Parsing", stage_start.elapsed()));

    // Must run before any geometry bake: failures block the compile so the
    // engine never loads a `.prl` whose paired `.js` is stale or missing.
    progress.start_stage("Script compilation...");
    let stage_start = Instant::now();
    compile_worldspawn_script(&args.input, map_data.script.as_deref())?;
    timings.push(("ScriptCompile", stage_start.elapsed()));

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
    let final_lightmap_density;
    let lightmap_bake_output = {
        // Retry on atlas overflow: doubles texel size (halves resolution) up to
        // MAX_RETRIES times. Degrades quality instead of failing the build.
        // Per-face planar unwrap wastes atlas area, so large maps hit this often.
        const MAX_RETRIES: u32 = 3;
        let mut density = args.lightmap_density;
        let mut attempt = 0;
        loop {
            let mut lm_inputs = lightmap_bake::LightmapInputs {
                bvh: &bvh,
                primitives: &bvh_primitives,
                geometry: &mut geo_result,
                lights: &static_baked_lights,
            };
            match lightmap_bake::bake_lightmap(&mut lm_inputs, density) {
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
    let sh_inputs = sh_bake::BakeInputs {
        bvh: &bvh,
        primitives: &bvh_primitives,
        geometry: &geo_result,
        tree: &result.tree,
        exterior_leaves: &exterior_leaves,
        static_lights: &static_baked_lights,
        animated_lights: &animated_baked_lights,
    };
    let sh_volume_section = sh_bake::bake_sh_volume(&sh_inputs, args.probe_spacing);
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
        delta_sh_bake::bake_delta_sh_volumes(&inputs, args.probe_spacing)
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
    let fog_volumes_section =
        pack::encode_fog_volumes(&map_data.fog_volumes, map_data.fog_pixel_scale);

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

    progress.start_stage("Packing and writing...");
    let stage_start = Instant::now();

    let portals_section = pack::encode_portals(&generated_portals);
    pack::pack_and_write_portals(
        &args.output,
        &geo_result,
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
             [--format <FORMAT>] [--probe-spacing <METERS>] [--lightmap-density <METERS>]"
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
    })
}

/// Locate the `scripts-build` sidecar for compiling worldspawn `.ts` scripts.
///
/// Duplicated from `crates/postretro/src/scripting/watcher.rs`
/// `TsCompilerPath::detect`; `watcher.rs` is `cfg(debug_assertions)` so cannot
/// be imported. If the cascade grows, promote to a shared crate.
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

/// Compile the worldspawn `script` if one is set, producing a sibling `.js`
/// artifact next to the source `.ts`.
///
/// Behavior matrix:
/// - `script_path == None` → no-op.
/// - `.js` newer than `.ts` → skip (already up to date).
/// - `scripts-build` found → invoke it; failure aborts the build.
/// - `scripts-build` missing but stale-fresh `.js` exists → warn and continue
///   (lets the engine ship without the sidecar in environments where the
///   author has pre-compiled).
/// - `scripts-build` missing and no `.js` → hard error.
fn compile_worldspawn_script(
    map_path: &std::path::Path,
    script_path: Option<&str>,
) -> anyhow::Result<()> {
    let Some(script_rel) = script_path else {
        return Ok(());
    };

    let map_dir = map_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let ts_path = map_dir.join(script_rel);
    let js_path = ts_path.with_extension("js");

    if !ts_path.is_file() {
        anyhow::bail!(
            "[prl-build] script = {script_rel} resolves to {} which does not exist",
            ts_path.display()
        );
    }

    if let Some(true) = js_is_fresh(&ts_path, &js_path) {
        log::info!(
            "[prl-build] script up to date: {} (skipping compile)",
            js_path.display()
        );
        return Ok(());
    }

    match find_scripts_build() {
        Some(compiler) => {
            log::info!(
                "[prl-build] compiling script {} -> {} via {}",
                ts_path.display(),
                js_path.display(),
                compiler.display()
            );
            let out = std::process::Command::new(&compiler)
                .arg("--in")
                .arg(&ts_path)
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
                    "[prl-build] scripts-build failed for {}: exit status {}",
                    ts_path.display(),
                    out.status
                );
            }
            Ok(())
        }
        None => {
            if js_path.is_file() {
                log::warn!(
                    "[prl-build] scripts-build not found; using existing compiled artifact {} \
                     (mtime > source). Install scripts-build or ship it next to prl-build to recompile.",
                    js_path.display()
                );
                Ok(())
            } else {
                anyhow::bail!(
                    "script = {script_rel} is set but scripts-build was not found and no compiled .js artifact exists beside the .ts file. Run scripts-build first or ship it next to prl-build."
                );
            }
        }
    }
}

/// Compile the worldspawn `data_script`, if present, and return the
/// `DataScriptSection` to embed in the PRL.
///
/// Behavior matrix:
/// - `path == None` → returns `Ok(None)`; no section is emitted.
/// - source file missing → hard error (no `.js` fallback).
/// - `.luau` source → read raw bytes, no compilation.
/// - `.ts`/`.js` source → compile via `scripts-build` (or fall back to a
///   freshly-modified sibling `.js` when the compiler is absent — same policy
///   as the behavior-script stage), then read the resulting `.js` bytes.
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
}
