// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod bvh_build;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod map_data;
pub mod map_format;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod sdf_bake;
pub mod sh_bake;
pub mod visibility;

use std::path::PathBuf;
use std::time::Instant;

use glam::Vec3;
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
    let geo_result = geometry::extract_geometry(&result.faces, &result.tree, &exterior_leaves);
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
        lights: &map_data.lights,
    };
    let sh_volume_section = sh_bake::bake_sh_volume(&sh_inputs, args.probe_spacing);
    timings.push(("SH Bake", stage_start.elapsed()));
    if args.verbose {
        sh_bake::log_stats(&sh_volume_section);
    }

    progress.start_stage("SDF bake...");
    let stage_start = Instant::now();
    // Find the player-start origin for the SDF flood-fill seed. The flood
    // marks bricks reachable from this point as EMPTY (open air); unreachable
    // bricks become INTERIOR (inside solid or exterior margin band).
    let player_start_world: Vec3 = map_data
        .entities
        .iter()
        .find(|e| e.classname == "info_player_start")
        .and_then(|e| e.origin)
        .map(|o| Vec3::new(o.x as f32, o.y as f32, o.z as f32))
        .unwrap_or_else(|| {
            let (raw_min, raw_max) = sdf_bake::world_aabb_pub(&geo_result.geometry);
            let cx = (raw_min[0] + raw_max[0]) * 0.5;
            let cy = (raw_min[1] + raw_max[1]) * 0.5;
            let cz = (raw_min[2] + raw_max[2]) * 0.5;
            log::warn!(
                "[SdfBake] No info_player_start entity found — using geometry centroid \
                 ({cx:.1}, {cy:.1}, {cz:.1}) as flood-fill seed. Add an \
                 info_player_start entity for reliable SDF classification."
            );
            Vec3::new(cx, cy, cz)
        });
    let sdf_inputs = sdf_bake::SdfBakeInputs {
        bvh: &bvh,
        primitives: &bvh_primitives,
        geometry: &geo_result,
        player_start_world,
    };
    let sdf_section = sdf_bake::bake_sdf_atlas(
        &sdf_inputs,
        args.sdf_voxel_size,
        args.sdf_brick_size,
        sdf_bake::DEFAULT_MARGIN_M,
    );
    timings.push(("SDF Bake", stage_start.elapsed()));
    if args.verbose {
        sdf_bake::log_stats(&sdf_section);
    }

    progress.start_stage("Packing and writing...");
    let stage_start = Instant::now();
    let alpha_lights_section = pack::encode_alpha_lights(&map_data.lights);
    let light_influence_section = pack::encode_light_influence(&map_data.lights);

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
            &sdf_section,
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
            &sdf_section,
        )?;
    }
    timings.push(("Packing", stage_start.elapsed()));

    progress.finish();

    println!("\nBuild Summary:");
    for (name, duration) in &timings {
        println!("  {: <15} {:>6.2}s", name, duration.as_secs_f32());
    }
    println!("  {: <15} {:>6.2}s", "Total", started.elapsed().as_secs_f32());

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
    /// SDF voxel edge length in meters (overrides default).
    sdf_voxel_size: f32,
    /// SDF brick size in voxels per edge (overrides default).
    sdf_brick_size: u32,
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
    let mut sdf_voxel_size = sdf_bake::DEFAULT_VOXEL_SIZE_M;
    let mut sdf_brick_size = sdf_bake::DEFAULT_BRICK_SIZE_VOXELS;

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
            "--sdf-voxel-size" => {
                let val_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sdf-voxel-size requires a value"))?;
                let parsed: f32 = val_str.parse().map_err(|_| {
                    anyhow::anyhow!("--sdf-voxel-size must be a positive number of meters")
                })?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    anyhow::bail!("--sdf-voxel-size must be a positive number of meters");
                }
                sdf_voxel_size = parsed;
            }
            "--sdf-brick-size" => {
                let val_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sdf-brick-size requires a value"))?;
                let parsed: u32 = val_str
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--sdf-brick-size must be a positive integer"))?;
                if parsed == 0 {
                    anyhow::bail!("--sdf-brick-size must be a positive integer");
                }
                sdf_brick_size = parsed;
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
             [--format <FORMAT>] [--probe-spacing <METERS>] \
             [--sdf-voxel-size <METERS>] [--sdf-brick-size <VOXELS>]"
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
        sdf_voxel_size,
        sdf_brick_size,
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
