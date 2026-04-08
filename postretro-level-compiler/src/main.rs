// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod geometry;
pub mod geometry_utils;
pub mod map_data;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod visibility;

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let started = Instant::now();
    let args = parse_args()?;

    log::info!("[Compiler] Input: {}", args.input.display());
    log::info!("[Compiler] Output: {}", args.output.display());

    let map_data = parse::parse_map_file(&args.input)?;

    log::info!("[Compiler] Parsing complete.");

    let result = partition::partition(map_data.world_faces, &map_data.brush_volumes)?;

    log::info!("[Compiler] BSP partitioning complete.");

    let geometry_section = geometry::extract_geometry(&result.faces, &result.tree);
    let empty_leaf_count = result.tree.leaves.iter().filter(|l| !l.is_solid).count();
    geometry::log_stats(&geometry_section, empty_leaf_count);

    log::info!("[Compiler] Geometry extraction complete.");

    let (vis_result, generated_portals) = visibility::build_portal_pvs(&result.tree);
    let portal_count = generated_portals.len();
    if portal_count == 0 {
        log::warn!(
            "[Compiler] Portal generation produced 0 portals. Vis will treat all leaves as mutually visible."
        );
    }
    visibility::log_stats(&vis_result, portal_count);

    log::info!("[Compiler] Visibility computation complete.");

    if args.pvs {
        log::info!("[Compiler] Writing precomputed PVS mode (--pvs).");
        pack::pack_and_write_pvs(
            &args.output,
            &geometry_section,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &vis_result.leaf_pvs_section,
        )?;
    } else {
        log::info!("[Compiler] Writing portal graph mode (default).");
        let portals_section = pack::encode_portals(&generated_portals);
        pack::pack_and_write_portals(
            &args.output,
            &geometry_section,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &portals_section,
        )?;
    }

    let elapsed = started.elapsed();
    log::info!("[Compiler] Done in {elapsed:.2?}.");

    Ok(())
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    /// When true, emit precomputed PVS (LeafPvs section) instead of portal graph.
    pvs: bool,
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
            _ if input.is_none() => {
                input = Some(PathBuf::from(arg));
            }
            _ => {
                anyhow::bail!("unexpected argument: {arg}");
            }
        }
    }

    let input = input
        .ok_or_else(|| anyhow::anyhow!("usage: prl-build <input.map> [-o <output.prl>] [--pvs]"))?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args { input, output, pvs })
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
}
