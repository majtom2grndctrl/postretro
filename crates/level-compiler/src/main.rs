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
            // We can't easily set the "start time" of an indicatif ProgressBar to the past
            // but we can use a custom formatter or just accept that it shows time-in-stage.
            // Actually, the user liked the "time since start" feel.

            // To show total elapsed time in the spinner line, we can use a template that
            // doesn't rely on the PB's internal clock if we want to be exact,
            // but indicatif's {elapsed} is fine for showing active progress.

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
/// Mirrors the runtime resolver in `postretro/src/main.rs`: `<asset_root>/textures/`,
/// where `<asset_root>` is the parent of the map's directory (typically `assets/maps/`).
/// For a map outside this layout the path is still constructed; the validator is a
/// no-op if the directory does not exist.
fn resolve_texture_root(map_path: &Path) -> PathBuf {
    let map_dir = map_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let asset_root = map_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    asset_root.join("textures")
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

    // Validate that every `_n.png` / `_s.png` surface-map sibling under the
    // textures root carries linear PNG color-space metadata. Diffuse textures
    // (no suffix) are not checked — the runtime samples them as Rgba8UnormSrgb.
    // See `context/lib/resource_management.md` §4 and `texture_validation.rs`.
    progress.start_stage("Texture color-space validation...");
    let stage_start = Instant::now();
    let texture_root = resolve_texture_root(&args.input);
    texture_validation::validate_sibling_color_spaces(&texture_root)?;
    timings.push(("TexValidation", stage_start.elapsed()));

    // Pre-compute light-namespace envelopes once so each bake stage takes the
    // typed slice it needs and applies no further filter. See
    // `light_namespaces.rs` for the index-space contracts.
    let static_baked_lights = light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
    let animated_baked_lights =
        light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
    let alpha_lights_ns = light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);

    progress.start_stage("BSP partitioning...");
    let stage_start = Instant::now();
    // Partition space with brush-derived planes, then project each brush
    // side through the resulting tree to recover the world face list.
    // Solidity is assigned during construction, so face extraction never
    // emits a polygon into a solid leaf.
    let result = partition::partition(&map_data.brush_volumes)?;
    timings.push(("Partitioning", stage_start.elapsed()));
    if args.verbose {
        partition::log_stats(&result.tree, &result.faces);
    }

    progress.start_stage("Visibility computation...");
    let stage_start = Instant::now();
    // Portals feed both the exterior flood-fill and the BSP/leaf encoder:
    // the encoder uses the exterior set to emit `face_count = 0` for
    // outside-the-map leaves in lockstep with the geometry section.
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
    // Build the global BVH over all static geometry. One acceleration
    // structure feeds both the runtime GPU traversal (via the flattened
    // BvhSection) and Milestone 5's CPU baker (via the live bvh::Bvh).
    let (bvh, bvh_primitives, bvh_section) =
        bvh_build::build_bvh(&geo_result).map_err(|e| anyhow::anyhow!("BVH build failed: {e}"))?;
    timings.push(("BVH Build", stage_start.elapsed()));
    if args.verbose {
        bvh_build::log_stats(&bvh_section);
    }

    progress.start_stage("Lightmap bake...");
    let stage_start = Instant::now();
    // Bake the directional lightmap. Writes per-vertex lightmap UVs back
    // into `geo_result.geometry` and returns the atlas section for packing.
    // Same BVH as the SH bake — one acceleration structure, two consumers.
    let static_light_count = map_data.lights.iter().filter(|l| !l.is_dynamic).count();
    let final_lightmap_density;
    let lightmap_bake_output = {
        // Retry on atlas overflow: double the texel size (halve resolution)
        // up to `MAX_RETRIES` times. Degrades quality instead of failing the
        // build on maps too large to fit at the default density. Per-face
        // planar unwrap wastes atlas area vs. a chart-merging unwrapper, so
        // large maps hit this regularly; a warning records the fallback so
        // it's not silent.
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
    // Pack-time validation: the baked lighting paths cannot absorb color
    // animation on a non-dynamic, non-bake_only light (Plan 2 Sub-plan 1).
    if let Err(msg) = sh_bake::validate_light_animations(&map_data.lights) {
        anyhow::bail!("light animation validation failed: {msg}");
    }
    // Bake the SH irradiance volume. Same BVH, different traversal: the
    // baker walks the tree on the CPU via the `bvh` crate, so the runtime
    // compute shader and the compile-time probe bake share one acceleration
    // structure.
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

    // Owned parallel `MapLight` slice in the AnimatedBakedLights namespace —
    // the animated weight-map baker holds its light list across multi-stage
    // pipelines and needs owned values.
    let (animated_chunk_lights, _) = animated_baked_lights.to_parallel_vecs();

    progress.start_stage("Animated light chunks...");
    let stage_start = Instant::now();
    // Builds the per-face, per-leaf animated-light chunk partition. Returns a
    // parallel `(chunk_range_start, chunk_range_count)` table indexed by BVH
    // leaf slot; pack stamps the table onto the on-disk `BvhLeaf` records at
    // serialization time (`pack::serialize_bvh_with_chunk_ranges`). The
    // explicit data flow replaces an earlier hidden mutation of `bvh_section`.
    // When the map has no animated lights the returned section is empty,
    // which is the signal to skip emission — no placeholder record needed.
    let (animated_light_chunks_section, bvh_chunk_ranges) =
        animated_light_chunks::build_animated_light_chunks(
            &bvh_section,
            &animated_baked_lights,
            &face_charts,
            &geo_result.face_index_ranges,
            final_lightmap_density,
        );
    timings.push(("AnimLightChunks", stage_start.elapsed()));

    // Weight-map bake: for every chunk, emit per-texel animated-light
    // contribution weights. Consumes the same `animated_chunk_lights` list the
    // chunk builder used, so chunk-list indices index directly into the
    // animated-light descriptor buffer at runtime with no remap.
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
    /// Enable detailed logging.
    verbose: bool,
    /// Map dialect to parse (default: IdTech2).
    format: MapFormat,
    /// SH probe grid spacing in meters.
    probe_spacing: f32,
    /// Starting lightmap texel density in meters. The baker retries at
    /// progressively coarser densities on atlas overflow.
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
}
