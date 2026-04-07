// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL

pub mod geometry;
pub mod map_data;
pub mod pack;
pub mod parse;
pub mod partition;
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

    let vis_result = visibility::build_passthrough_pvs(&result.tree);
    visibility::log_stats(&vis_result);

    log::info!("[Compiler] Visibility computation complete.");

    pack::pack_and_write(&args.output, &geometry_section, &vis_result.section)?;

    let elapsed = started.elapsed();
    log::info!("[Compiler] Done in {elapsed:.2?}.");

    Ok(())
}

struct Args {
    input: PathBuf,
    output: PathBuf,
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

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("-o requires an output path"))?;
                output = Some(PathBuf::from(path));
            }
            _ if input.is_none() => {
                input = Some(PathBuf::from(arg));
            }
            _ => {
                anyhow::bail!("unexpected argument: {arg}");
            }
        }
    }

    let input =
        input.ok_or_else(|| anyhow::anyhow!("usage: prl-build <input.map> [-o <output.prl>]"))?;

    let output = output.unwrap_or_else(|| input.with_extension("prl"));

    Ok(Args { input, output })
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
    fn parse_args_rejects_unknown_flags() {
        let args = vec!["input.map".to_string(), "--bsp".to_string()];
        let result = parse_args_from(args.into_iter());
        assert!(result.is_err());
    }
}
