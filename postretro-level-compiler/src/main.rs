// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod animated_light_chunks;
pub mod bvh_build;
pub mod chunk_light_list_bake;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod lightmap_bake;
pub mod map_data;
pub mod map_format;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod sh_bake;
pub mod visibility;

use std::path::PathBuf;
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

    let vis_result = visibility::encode_vis(&result.tree, &generated_portals, &exterior_leaves);
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
    let (bvh, bvh_primitives, mut bvh_section) =
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
    let (lightmap_section, face_charts) = {
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
                lights: &map_data.lights,
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
                    log::warn!(
                        "Lightmap atlas overflow at {density} m/texel (needed {needed_w}x{needed_h}, \
                         max {max}x{max}); retrying at {next} m/texel"
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
    if args.verbose {
        lightmap_bake::log_stats(&lightmap_section, static_light_count);
    }

    progress.start_stage("SH volume bake...");
    let stage_start = Instant::now();
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
        lights: &map_data.lights,
    };
    let sh_volume_section = sh_bake::bake_sh_volume(&sh_inputs, args.probe_spacing);
    timings.push(("SH Bake", stage_start.elapsed()));
    if args.verbose {
        sh_bake::log_stats(&sh_volume_section);
    }

    progress.start_stage("Chunk light list bake...");
    let stage_start = Instant::now();
    let chunk_light_list_section = {
        let inputs = chunk_light_list_bake::ChunkLightListInputs {
            bvh: &bvh,
            primitives: &bvh_primitives,
            geometry: &geo_result,
            lights: &map_data.lights,
        };
        chunk_light_list_bake::bake_chunk_light_list(
            &inputs,
            chunk_light_list_bake::DEFAULT_CELL_SIZE_METERS,
            chunk_light_list_bake::DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .map_err(|e| anyhow::anyhow!("Chunk light list bake failed: {e}"))?
    };
    timings.push(("ChunkLightList", stage_start.elapsed()));

    // Build the `!bake_only`-filtered light list once; the alpha-lights and
    // light-influence sections are both indexed against it, and the
    // animated-light-chunks builder emits indices into the same namespace.
    let alpha_lights_section = pack::encode_alpha_lights(&map_data.lights);
    let light_influence_section = pack::encode_light_influence(&map_data.lights);
    let filtered_lights: Vec<map_data::MapLight> = map_data
        .lights
        .iter()
        .filter(|l| !l.bake_only)
        .cloned()
        .collect();
    debug_assert_eq!(filtered_lights.len(), light_influence_section.records.len());

    progress.start_stage("Animated light chunks...");
    let stage_start = Instant::now();
    // Builds the per-face, per-leaf animated-light chunk partition and
    // stamps `chunk_range_start` / `chunk_range_count` on every `BvhLeaf`.
    // Must run before `bvh_section` is serialized into the output file.
    // When the map has no animated lights the returned section is empty,
    // which is the signal to skip emission — no placeholder record needed.
    let animated_light_chunks_section = animated_light_chunks::build_animated_light_chunks(
        &mut bvh_section,
        &filtered_lights,
        &light_influence_section.records,
        &face_charts,
        &geo_result.face_index_ranges,
        final_lightmap_density,
    );
    let animated_light_chunks_section = if animated_light_chunks_section.chunks.is_empty() {
        None
    } else {
        Some(animated_light_chunks_section)
    };
    timings.push(("AnimLightChunks", stage_start.elapsed()));

    progress.start_stage("Packing and writing...");
    let stage_start = Instant::now();

    if args.pvs {
        if args.verbose {
            log::info!("Writing precomputed PVS mode (--pvs).");
        }
        pack::pack_and_write_pvs(
            &args.output,
            &geo_result,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &vis_result.leaf_pvs_section,
            &bvh_section,
            &alpha_lights_section,
            &light_influence_section,
            &sh_volume_section,
            &lightmap_section,
            &chunk_light_list_section,
            animated_light_chunks_section.as_ref(),
        )?;
    } else {
        if args.verbose {
            log::info!("Writing portal graph mode (default).");
        }
        let portals_section = pack::encode_portals(&generated_portals);
        pack::pack_and_write_portals(
            &args.output,
            &geo_result,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &portals_section,
            &bvh_section,
            &alpha_lights_section,
            &light_influence_section,
            &sh_volume_section,
            &lightmap_section,
            &chunk_light_list_section,
            animated_light_chunks_section.as_ref(),
        )?;
    }
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
    /// When true, emit precomputed PVS (LeafPvs section) instead of portal graph.
    pvs: bool,
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
    let mut pvs = false;
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
            "--pvs" => {
                pvs = true;
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
            "usage: prl-build <input.map> [-o <output.prl>] [--pvs] [-v|--verbose] \
             [--format <FORMAT>] [--probe-spacing <METERS>] [--lightmap-density <METERS>]"
        )
    })?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args {
        input,
        output,
        pvs,
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
        assert!(!parsed.pvs);
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
    fn parse_args_pvs_flag() {
        let args = vec!["input.map".to_string(), "--pvs".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.pvs);
    }

    #[test]
    fn parse_args_pvs_flag_with_output() {
        let args = vec![
            "input.map".to_string(),
            "--pvs".to_string(),
            "-o".to_string(),
            "out.prl".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.pvs);
        assert_eq!(parsed.output, PathBuf::from("out.prl"));
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
