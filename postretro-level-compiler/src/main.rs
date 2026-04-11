// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod geometry;
pub mod geometry_utils;
pub mod map_data;
pub mod map_format;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod portals;
pub mod visibility;

use std::path::PathBuf;
use std::time::Instant;

use map_format::{DEFAULT_MAP_FORMAT, MapFormat};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let started = Instant::now();
    let args = parse_args()?;

    log::info!("[Compiler] Input: {}", args.input.display());
    log::info!("[Compiler] Output: {}", args.output.display());
    log::info!("[Compiler] Map format: {:?}", args.format);

    if !args.format.is_supported() {
        anyhow::bail!("map format '{:?}' is not yet supported", args.format);
    }

    let map_data = parse::parse_map_file(&args.input, args.format)?;

    log::info!("[Compiler] Parsing complete.");

    // Partition space with brush-derived planes, then project each brush
    // side through the resulting tree to recover the world face list.
    // Solidity is assigned during construction, so face extraction never
    // emits a polygon into a solid leaf.
    let result = partition::partition(&map_data.brush_volumes)?;

    log::info!("[Compiler] BSP partitioning complete.");

    // Portals feed both the exterior flood-fill and the BSP/leaf encoder:
    // the encoder uses the exterior set to emit `face_count = 0` for
    // outside-the-map leaves in lockstep with the geometry section.
    let generated_portals = portals::generate_portals(&result.tree);
    let portal_count = generated_portals.len();
    if portal_count == 0 {
        log::warn!(
            "[Compiler] Portal generation produced 0 portals. Vis will treat all leaves as mutually visible."
        );
    }

    let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);

    let vis_result = visibility::encode_vis(&result.tree, &generated_portals, &exterior_leaves);
    visibility::log_stats(&vis_result, portal_count);

    log::info!("[Compiler] Visibility computation complete.");

    let geo_result = geometry::extract_geometry(&result.faces, &result.tree, &exterior_leaves);
    let empty_leaf_count = result
        .tree
        .leaves
        .iter()
        .enumerate()
        .filter(|(idx, l)| !l.is_solid && !exterior_leaves.contains(idx))
        .count();
    geometry::log_stats(&geo_result, empty_leaf_count);

    log::info!("[Compiler] Geometry extraction complete.");

    if args.pvs {
        log::info!("[Compiler] Writing precomputed PVS mode (--pvs).");
        pack::pack_and_write_pvs(
            &args.output,
            &geo_result,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &vis_result.leaf_pvs_section,
        )?;
    } else {
        log::info!("[Compiler] Writing portal graph mode (default).");
        let portals_section = pack::encode_portals(&generated_portals);
        pack::pack_and_write_portals(
            &args.output,
            &geo_result,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &portals_section,
        )?;
    }

    let elapsed = started.elapsed();
    log::info!("[Compiler] Done in {elapsed:.2?}.");

    Ok(())
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    /// When true, emit precomputed PVS (LeafPvs section) instead of portal graph.
    pvs: bool,
    /// Map dialect to parse (default: IdTech2).
    format: MapFormat,
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
    let mut format = DEFAULT_MAP_FORMAT;

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
            "--format" => {
                let fmt_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--format requires a value"))?;
                format = fmt_str
                    .parse::<MapFormat>()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
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
            "usage: prl-build <input.map> [-o <output.prl>] [--pvs] [--format <FORMAT>]"
        )
    })?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args {
        input,
        output,
        pvs,
        format,
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
        assert_eq!(parsed.format, MapFormat::IdTech2);
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
