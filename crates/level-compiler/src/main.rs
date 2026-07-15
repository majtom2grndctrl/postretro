// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL Compilation

pub mod affinity_grid;
pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bake_control;
pub mod bc5;
pub mod bc6h;
pub mod bvh_build;
pub mod cache;
pub mod cell_draw_index_bake;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_sh_bake;
pub mod direct_sh_bake;
pub mod entity_shadow_select;
#[cfg(test)]
pub mod fixture_pipeline;
pub mod fog_cell_masks;
pub mod format;
pub mod geometry;
pub mod geometry_utils;
pub mod governor;
pub mod kinematic_geometry;
pub mod light_namespaces;
pub mod lightmap_bake;
pub mod lightmap_layer;
pub mod logger;
pub mod map_data;
pub mod map_format;
pub mod navmesh_bake;
pub mod pack;
pub mod parse;
pub mod partition;
pub mod pipeline;
pub mod portals;
pub mod reporter;
pub mod sdf_bake;
pub mod sh_bake;
pub mod sh_group;
pub mod shadowmask_bake;
pub mod texture_mips;
pub mod texture_validation;
pub mod trigger_volumes;
pub mod visibility;

use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::time::Instant;

use map_format::{DEFAULT_MAP_FORMAT, MapFormat};

/// Resolve the root used by content-relative model handles.
///
/// Mirrors the runtime derivation for `content/<mod>/maps/<map>`:
/// model handles resolve from `content/<mod>`.
fn resolve_content_root(map_path: &Path) -> PathBuf {
    let map_dir = map_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    map_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the textures directory from a map input path.
///
/// Mirrors the runtime resolver `content_root_from_map` in
/// `crates/postretro/src/main.rs`: `<content_root>/textures/`, where
/// `<content_root>` is the parent of the map's directory (typically
/// `content/<mod>/maps/`). For a map outside this layout the path is still
/// constructed; the validator is a no-op if the directory does not exist.
fn resolve_texture_root(map_path: &Path) -> PathBuf {
    resolve_content_root(map_path).join("textures")
}

/// Resolve the compiled-material output root from a map input path.
///
/// `.prm` mip sidecars are runtime-*required* compiled output, not a disposable
/// cache, so they live in `<workspace>/baked/materials/` — a top-level
/// compiled-output tree, sibling to the disposable `.build-caches/` stage cache.
/// The runtime reader (`derive_prm_root_dev_layout` in
/// `crates/postretro/src/startup/worker.rs`) must resolve to this same directory
/// in the dev layout; if they diverge, every texture silently degrades to a
/// placeholder.
///
/// Falls back to `<map parent>/baked/materials/` when no `Cargo.toml` ancestor
/// is found — covers shipping or standalone layouts that omit the workspace
/// manifest. The runtime's analogous fallback uses `content_root`, so the two
/// fallbacks need not coincide outside the dev layout (shipping is out of scope).
fn resolve_prm_root_via_cargo(map_path: &Path) -> PathBuf {
    cache::find_workspace_root(map_path)
        .unwrap_or_else(|| {
            map_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        })
        .join("baked")
        .join("materials")
}

fn prop_mesh_model_handles(entities: &[map_data::MapEntityRecord]) -> Vec<&str> {
    let mut seen = HashSet::new();
    let mut handles = Vec::new();

    for entity in entities {
        if entity.classname != "prop_mesh" {
            continue;
        }
        // Runtime KVP conversion is last-value-wins, so bake the same model.
        let Some(model) = entity
            .key_values
            .iter()
            .rev()
            .find_map(|(key, value)| (key == "model").then_some(value.as_str()))
        else {
            continue;
        };
        if !model.trim().is_empty() && seen.insert(model) {
            handles.push(model);
        }
    }

    handles
}

fn bake_model_textures_with<Resolve, ResolveError, Bake, BakeError>(
    entities: &[map_data::MapEntityRecord],
    content_root: &Path,
    prm_root: &Path,
    mut resolve_base_color_paths: Resolve,
    mut bake_diffuse: Bake,
) where
    Resolve: FnMut(&Path) -> Result<Vec<PathBuf>, ResolveError>,
    ResolveError: Display,
    Bake: FnMut(&Path, &Path) -> Result<[u8; 32], BakeError>,
    BakeError: Display,
{
    let mut seen_textures = HashSet::new();

    for model_handle in prop_mesh_model_handles(entities) {
        let model_path = content_root.join(model_handle);
        let texture_paths = match resolve_base_color_paths(&model_path) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!(
                    "[prl-build] failed to resolve model textures for {}: {error}",
                    model_path.display()
                );
                continue;
            }
        };

        for texture_path in texture_paths {
            if !seen_textures.insert(texture_path.clone()) {
                continue;
            }
            if let Err(error) = bake_diffuse(&texture_path, prm_root) {
                log::warn!(
                    "[prl-build] failed to bake model texture {}: {error}",
                    texture_path.display()
                );
            }
        }
    }
}

fn bake_model_textures(
    entities: &[map_data::MapEntityRecord],
    content_root: &Path,
    prm_root: &Path,
) {
    bake_model_textures_with(
        entities,
        content_root,
        prm_root,
        postretro_level_format::gltf_resolve::resolve_document_base_color_paths,
        texture_mips::bake_diffuse_texture,
    );
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

/// What the output-directory precheck decided to do for a parsed answer.
///
/// `Create` carries the directory to create; `Abort` ends the run before any
/// bake; `Reprompt` means the answer was unrecognized and the caller should
/// ask again. The `Proceed`/`Create`/`Abort` split is kept pure so the
/// terminal-I/O wrapper (`precheck_output_dir`) stays a thin shell.
#[derive(Debug, PartialEq, Eq)]
enum DirAnswer {
    Create,
    Abort,
    Reprompt,
}

/// Decide whether the output's parent directory must be created.
///
/// Pure: callers pass the result of `parent.exists()` so this needs no
/// filesystem. Returns `Some(parent)` when the parent is a non-empty path that
/// does not exist (so it must be created); `None` when the parent is empty /
/// the current directory, or already exists — the common case, zero prompts.
fn output_dir_to_create(output: &Path, parent_exists: bool) -> Option<PathBuf> {
    match output.parent() {
        // No parent or an empty parent (e.g. a bare filename) means the
        // current directory, which always exists. Nothing to create.
        Some(parent) if !parent.as_os_str().is_empty() && !parent_exists => {
            Some(parent.to_path_buf())
        }
        _ => None,
    }
}

/// Parse a user's terminal answer to the create-directory prompt.
///
/// Pure. `answer` is `None` on EOF (no answer available — non-interactive
/// stdin, closed pipe), which aborts. A bare Enter (empty trimmed line) is the
/// `[Y/n]` default → `Create`. Recognized yes/no map to `Create`/`Abort`;
/// anything else is `Reprompt` so the caller loops rather than guessing.
fn parse_dir_answer(answer: Option<&str>) -> DirAnswer {
    match answer {
        None => DirAnswer::Abort,
        Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => DirAnswer::Create,
            "n" | "no" => DirAnswer::Abort,
            _ => DirAnswer::Reprompt,
        },
    }
}

/// Fail-fast precheck for the output directory, run before any bake work.
///
/// If the resolved output `.prl` path's parent directory is missing, prompt on
/// the terminal to create it. This is a CLI tool, so interactive prompting plus
/// a non-zero exit on user-abort is appropriate (unlike subsystem library
/// code). EOF on stdin (CI, `</dev/null`, closed pipe) returns `Ok(0)` from
/// `read_line` and is treated as "no answer" → abort, so this never hangs.
fn precheck_output_dir(output: &Path) -> anyhow::Result<()> {
    let parent = match output_dir_to_create(output, output.parent().is_some_and(Path::exists)) {
        Some(parent) => parent,
        None => return Ok(()),
    };

    use std::io::Write as _;
    let stdin = std::io::stdin();
    let mut stderr = std::io::stderr();
    let mut line = String::new();
    loop {
        write!(
            stderr,
            "Output directory '{}' does not exist. Create it? [Y/n] ",
            parent.display()
        )?;
        stderr.flush()?;

        line.clear();
        let answer = if stdin.read_line(&mut line)? == 0 {
            // EOF: no answer available. Don't loop on repeated EOF — abort.
            None
        } else {
            Some(line.as_str())
        };

        match parse_dir_answer(answer) {
            DirAnswer::Create => {
                std::fs::create_dir_all(&parent).map_err(|e| {
                    anyhow::anyhow!(
                        "[Compiler] failed to create output directory '{}': {e}",
                        parent.display()
                    )
                })?;
                return Ok(());
            }
            DirAnswer::Abort => {
                anyhow::bail!(
                    "[Compiler] aborted: output directory '{}' does not exist. \
                     Re-run with an existing folder in -o (or create '{}' first).",
                    parent.display(),
                    parent.display()
                );
            }
            DirAnswer::Reprompt => continue,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let started = Instant::now();
    let args = parse_args()?;

    // Fail fast: if the output directory is missing, prompt to create it now —
    // before parsing the map or running any bake — so a missing folder never
    // wastes a multi-minute bake that only fails at the final write.
    precheck_output_dir(&args.output)?;

    let log_sink = logger::install(args.verbose)?;

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
                // Bound the cache before this build writes a fresh generation:
                // content addressing never reclaims orphaned generations, so an
                // LRU sweep at build start is what keeps the directory from
                // growing without limit. Off the bake path (one readdir + a few
                // unlinks); best-effort, never fails the build.
                c.prune_to_budget(args.cache_max_bytes);
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

    let reporter: std::sync::Arc<dyn reporter::Reporter> =
        std::sync::Arc::new(reporter::PlainReporter::new(started, log_sink));
    let permits = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let governor = std::sync::Arc::new(governor::Governor::new(permits, false));
    pipeline::run(&args, stage_cache, started, reporter, governor)
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
    /// invalidates cached lightmap layers, any shadowmask memo keyed through
    /// selected layer hashes, and the cached animated weight-map stage,
    /// triggering a re-bake/rebuild of each affected stage. Default
    /// `lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT`.
    soft_shadow_samples: u32,
    /// SDF occluder-atlas voxel edge length in meters. Overrides
    /// `sdf_bake::DEFAULT_VOXEL_SIZE_METERS` for this run.
    voxel_size: f32,
    /// Override cache directory. None = use the workspace-root default.
    cache_dir: Option<PathBuf>,
    /// LRU size budget for the stage cache, in bytes. The cache is pruned to
    /// this at build start (oldest-used entries first). Defaults to
    /// `cache::DEFAULT_MAX_BYTES`; ignored when the cache is disabled.
    cache_max_bytes: u64,
    /// When true, bypass cache reads and writes entirely.
    no_cache: bool,
    /// When true, produce a shippable map: the exact ship path (exact monolithic
    /// lightmap + exact whole-volume SH). Named for intent ("I am producing a
    /// shippable map") rather than cache mechanics; it implies `--no-cache`, so
    /// the warm/cold branches need no change — both flags route the stage cache
    /// to `None`. Passing both is fine (identical effect, no conflict).
    release: bool,
    /// When true, store the baked lightmap irradiance atlas uncompressed as
    /// `Rgba16Float` instead of the default BC6H. BC6H is the default (smaller
    /// on disk and in VRAM); the uncompressed form is larger and exists for
    /// debugging and quality comparison against the compressed path.
    uncompressed_irradiance: bool,
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
         --cache-max-size <SIZE>    LRU budget for the stage cache, pruned at build start; accepts e.g. 2GiB, 512MiB, or a byte count (default: {cache_max})\n    \
         --no-cache                 Disable the stage cache entirely; wins over --cache-dir (default: off)\n    \
         --release                  Produce a shippable map: exact lighting, cache bypassed (implies --no-cache). The interactive default is a fast warm build with approximate indirect lighting; ship only --release artifacts (default: off)\n    \
         --uncompressed-irradiance  Store the lightmap irradiance atlas uncompressed as Rgba16Float instead of BC6H — larger; for debugging/quality comparison (default: off, BC6H)\n    \
         -h, --help                 Print this help and exit\n",
        probe = sh_bake::DEFAULT_PROBE_SPACING,
        density = lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS,
        samples = lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
        probe_floor = lightmap_bake::SOFT_PROBE_SAMPLES,
        voxel = sdf_bake::DEFAULT_VOXEL_SIZE_METERS,
        cache_max = format_size_gib(cache::DEFAULT_MAX_BYTES),
    )
}

/// Render a byte budget as a `GiB` string for help text (e.g. `2 GiB`).
fn format_size_gib(bytes: u64) -> String {
    format!("{} GiB", bytes / (1024 * 1024 * 1024))
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
    let mut cache_max_bytes = cache::DEFAULT_MAX_BYTES;
    let mut no_cache = false;
    let mut release = false;
    let mut uncompressed_irradiance = false;

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
            "--cache-max-size" => {
                let size_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--cache-max-size requires a value"))?;
                cache_max_bytes = parse_size(&size_str)?;
            }
            "--no-cache" => {
                no_cache = true;
            }
            "--release" => {
                release = true;
            }
            "--uncompressed-irradiance" => {
                uncompressed_irradiance = true;
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
             [--soft-shadow-samples <N>] [--sdf-voxel-size <METERS>] [--cache-dir <PATH>] [--cache-max-size <SIZE>] [--no-cache] [--release]\n\
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
        cache_max_bytes,
        no_cache,
        release,
        uncompressed_irradiance,
    })
}

/// Parse a `--cache-max-size` value into a byte count. Accepts a plain integer
/// (bytes) or a decimal value with a binary unit suffix: `B`, `KiB`, `MiB`,
/// `GiB`, `TiB` (case-insensitive; a bare `K`/`M`/`G`/`T` is treated as the
/// binary unit). Examples: `2GiB`, `1536MiB`, `2147483648`.
fn parse_size(raw: &str) -> anyhow::Result<u64> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("--cache-max-size requires a value");
    }
    // Split the numeric prefix from an optional unit suffix.
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num_str, unit_str) = s.split_at(split);
    let value: f64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "--cache-max-size: '{raw}' is not a valid size (e.g. 2GiB, 512MiB, or a byte count)"
        )
    })?;
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("--cache-max-size must be a non-negative size");
    }
    let multiplier: u64 = match unit_str.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kib" => 1024,
        "m" | "mib" => 1024 * 1024,
        "g" | "gib" => 1024 * 1024 * 1024,
        "t" | "tib" => 1024u64 * 1024 * 1024 * 1024,
        other => {
            anyhow::bail!("--cache-max-size: unknown unit '{other}' (use B, KiB, MiB, GiB, or TiB)")
        }
    };
    Ok((value * multiplier as f64) as u64)
}

/// Locate the `scripts-build` sidecar for compiling worldspawn `.ts` scripts.
///
// TODO(scripting-tools-dedup): duplicates `TsCompilerPath::detect` in
// `crates/scripting-core/src/watcher.rs`, reached by the engine through a
// debug-only compatibility re-export. The level-compiler still cannot import
// the engine-side wrapper. The matching mtime check lives in `js_is_fresh`
// below; the matching subprocess invocation lives in `run_ts_compiler` in the
// watcher module. Consolidate into a
// shared `postretro-scripts-tools` crate when the level-compiler gains more
// scripting integration. See:
// context/plans/drafts/scripting-tools-dedup/index.md
fn is_compiler_stale(binary_path: &Path) -> bool {
    let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("script-compiler")
        .join("src");
    if !source_dir.is_dir() {
        return false;
    }
    let sidecar_mtime = match std::fs::metadata(binary_path).and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Find newest source mtime recursively
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![source_dir];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(metadata) = std::fs::metadata(&path) {
                if let Ok(mtime) = metadata.modified() {
                    newest = Some(newest.map_or(mtime, |cur| cur.max(mtime)));
                }
            }
        }
    }

    match newest {
        Some(newest_mtime) => newest_mtime > sidecar_mtime,
        None => false,
    }
}

fn find_scripts_build() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let name = if cfg!(windows) {
        "scripts-build.exe"
    } else {
        "scripts-build"
    };

    let path = if let Some(ref dir) = exe_dir {
        let candidate = dir.join(name);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    } else {
        None
    };

    let path = path.or_else(|| {
        std::env::var_os("PATH").and_then(|path_var| {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            None
        })
    });

    let needs_build = match &path {
        None => true,
        Some(p) => is_compiler_stale(p),
    };

    if needs_build {
        log::info!("[prl-build] scripts-build is missing or stale. Rebuilding via cargo...");
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("build").arg("-p").arg("postretro-script-compiler");
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        match cmd.status() {
            Ok(status) if status.success() => {
                log::info!("[prl-build] scripts-build compiled successfully.");
                if let Some(ref dir) = exe_dir {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
            Ok(status) => {
                log::error!(
                    "[prl-build] Failed to compile scripts-build: exit code {}",
                    status
                );
            }
            Err(err) => {
                log::error!("[prl-build] Failed to spawn cargo build: {}", err);
            }
        }
    }

    path
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
// freshness check in `crates/scripting-core/src/runtime/compile.rs`. See the
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

    fn unique_temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "prl-build-main-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 37 + y * 11) as u8,
                (x * 13 + y * 29) as u8,
                (x * 7 + y * 43) as u8,
                255,
            ])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn map_entity(classname: &str, key_values: &[(&str, &str)]) -> map_data::MapEntityRecord {
        map_data::MapEntityRecord {
            classname: classname.to_string(),
            origin: glam::DVec3::ZERO,
            angles: [0.0; 3],
            key_values: key_values
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn resolve_content_root_uses_map_directory_grandparent() {
        assert_eq!(
            resolve_content_root(Path::new("content/base/maps/test.map")),
            PathBuf::from("content/base")
        );
        assert_eq!(
            resolve_texture_root(Path::new("content/base/maps/test.map")),
            PathBuf::from("content/base/textures")
        );
    }

    #[test]
    fn prop_mesh_model_handles_use_final_value_and_deduplicate_in_map_order() {
        let entities = vec![
            map_entity("light", &[("model", "models/ignored.gltf")]),
            map_entity("prop_mesh", &[("other", "value")]),
            map_entity("prop_mesh", &[("model", "")]),
            map_entity("prop_mesh", &[("model", "models/first.gltf")]),
            map_entity("prop_mesh", &[("model", "models/first.gltf")]),
            map_entity(
                "prop_mesh",
                &[
                    ("model", "models/ignored-overwritten.gltf"),
                    ("model", "models/second.gltf"),
                ],
            ),
            map_entity(
                "prop_mesh",
                &[
                    ("model", "models/ignored-overwritten.gltf"),
                    ("model", "models/first.gltf"),
                ],
            ),
            map_entity(
                "prop_mesh",
                &[("model", "models/ignored-overwritten.gltf"), ("model", "")],
            ),
        ];

        assert_eq!(
            prop_mesh_model_handles(&entities),
            vec!["models/first.gltf", "models/second.gltf"]
        );
    }

    #[test]
    fn model_texture_bake_deduplicates_paths_and_continues_after_errors() {
        let entities = vec![
            map_entity("prop_mesh", &[("model", "models/first.gltf")]),
            map_entity("prop_mesh", &[("model", "models/first.gltf")]),
            map_entity("prop_mesh", &[("model", "models/malformed.gltf")]),
            map_entity("prop_mesh", &[("model", "models/second.gltf")]),
        ];
        let content_root = Path::new("content/base");
        let cache_root = Path::new("baked/materials");
        let shared = PathBuf::from("content/base/models/shared.png");
        let first_only = PathBuf::from("content/base/models/first.png");
        let unreadable = PathBuf::from("content/base/models/unreadable.png");
        let mut resolved_models = Vec::new();
        let mut baked_textures = Vec::new();

        bake_model_textures_with(
            &entities,
            content_root,
            cache_root,
            |model_path| -> Result<Vec<PathBuf>, &'static str> {
                resolved_models.push(model_path.to_path_buf());
                match model_path.file_name().and_then(|name| name.to_str()) {
                    Some("first.gltf") => {
                        Ok(vec![shared.clone(), first_only.clone(), shared.clone()])
                    }
                    Some("malformed.gltf") => Err("malformed glTF"),
                    Some("second.gltf") => Ok(vec![shared.clone(), unreadable.clone()]),
                    _ => Ok(Vec::new()),
                }
            },
            |texture_path, observed_cache_root| -> Result<[u8; 32], &'static str> {
                assert_eq!(observed_cache_root, cache_root);
                baked_textures.push(texture_path.to_path_buf());
                if texture_path == unreadable {
                    Err("unreadable PNG")
                } else {
                    Ok([1; 32])
                }
            },
        );

        assert_eq!(
            resolved_models,
            vec![
                content_root.join("models/first.gltf"),
                content_root.join("models/malformed.gltf"),
                content_root.join("models/second.gltf"),
            ]
        );
        assert_eq!(baked_textures, vec![shared, first_only, unreadable]);
    }

    #[test]
    fn model_texture_bake_creates_and_regenerates_blake3_named_sidecar() {
        let root = unique_temp_dir("model-texture-boundary");
        let content_root = root.join("content/base");
        let models_root = content_root.join("models");
        let cache_root = root.join("prm-cache");
        std::fs::create_dir_all(&models_root).unwrap();

        let texture_bytes = png_bytes(2, 2);
        std::fs::write(models_root.join("base-color.png"), &texture_bytes).unwrap();
        std::fs::write(
            models_root.join("fixture.gltf"),
            r#"{
                "asset": {"version": "2.0"},
                "images": [{"uri": "base-color.png"}],
                "textures": [{"source": 0}],
                "materials": [{
                    "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}
                }]
            }"#,
        )
        .unwrap();

        let entities = vec![map_entity("prop_mesh", &[("model", "models/fixture.gltf")])];
        let expected_sidecar =
            cache_root.join(format!("{}.prm", blake3::hash(&texture_bytes).to_hex()));

        bake_model_textures(&entities, &content_root, &cache_root);
        assert!(expected_sidecar.is_file());

        std::fs::remove_file(&expected_sidecar).unwrap();
        bake_model_textures(&entities, &content_root, &cache_root);
        assert!(expected_sidecar.is_file());

        let no_prop_cache_root = root.join("no-prop-prm-cache");
        let no_prop_entities = vec![map_entity("light", &[("model", "models/fixture.gltf")])];
        bake_model_textures(&no_prop_entities, &content_root, &no_prop_cache_root);
        assert!(!no_prop_cache_root.exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The compiler (writer) and runtime (reader) derive the `.prm` root by two
    /// different walks — the compiler finds the workspace via a `Cargo.toml`
    /// ancestor, the runtime goes two parents up from `content_root`. In the dev
    /// layout both MUST land on `<workspace>/baked/materials`; if they diverge,
    /// every world texture silently degrades to a placeholder. This guards that
    /// invariant by reproducing the runtime's two-parent walk inline (its
    /// function lives in the `postretro` crate and is not importable here).
    #[test]
    fn prm_root_writer_and_reader_agree_in_dev_layout() {
        let workspace = unique_temp_dir("prm-root-agreement");
        let content_root = workspace.join("content").join("base");
        let maps_dir = content_root.join("maps");
        std::fs::create_dir_all(&maps_dir).unwrap();
        // The compiler resolver walks up to the nearest `Cargo.toml`.
        std::fs::write(workspace.join("Cargo.toml"), "[workspace]\n").unwrap();

        let map_path = maps_dir.join("level.map");
        let expected = workspace.join("baked").join("materials");

        // Writer side: prl-build.
        let writer_root = resolve_prm_root_via_cargo(&map_path);
        assert_eq!(writer_root, expected);

        // Reader side: mirror of `derive_prm_root_dev_layout`
        // (crates/postretro/src/startup/worker.rs) — two parents up from
        // `content_root`, then `baked/materials`.
        let reader_root = content_root
            .parent()
            .and_then(|c| c.parent())
            .unwrap_or(content_root.as_path())
            .join("baked")
            .join("materials");
        assert_eq!(reader_root, expected);
        assert_eq!(writer_root, reader_root);

        std::fs::remove_dir_all(&workspace).unwrap();
    }

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
    fn parse_args_uncompressed_irradiance_flag() {
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(
            !parsed.uncompressed_irradiance,
            "irradiance should default to compressed (BC6H)"
        );

        let args = vec![
            "input.map".to_string(),
            "--uncompressed-irradiance".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(
            parsed.uncompressed_irradiance,
            "--uncompressed-irradiance should set the flag"
        );
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
    fn parse_args_cache_max_size_default_is_two_gib() {
        let args = vec!["input.map".to_string()];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.cache_max_bytes, cache::DEFAULT_MAX_BYTES);
        assert_eq!(parsed.cache_max_bytes, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_args_cache_max_size_accepts_units() {
        let args = vec![
            "input.map".to_string(),
            "--cache-max-size".to_string(),
            "512MiB".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert_eq!(parsed.cache_max_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn parse_args_cache_max_size_requires_value() {
        let args = vec!["input.map".to_string(), "--cache-max-size".to_string()];
        assert!(parse_args_from(args.into_iter()).is_err());
    }

    #[test]
    fn parse_size_handles_units_and_bytes() {
        assert_eq!(parse_size("2147483648").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("2GiB").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("2gib").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1536MiB").unwrap(), 1536 * 1024 * 1024);
        assert_eq!(parse_size("1.5GiB").unwrap(), 1536 * 1024 * 1024);
        assert_eq!(parse_size("4G").unwrap(), 4u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("0").unwrap(), 0);
    }

    #[test]
    fn parse_size_rejects_garbage_and_unknown_units() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("12XB").is_err());
        assert!(parse_size("-5GiB").is_err());
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

    #[test]
    fn output_dir_existing_parent_proceeds() {
        // Parent exists → nothing to create (the common case, zero prompts).
        let out = PathBuf::from("/some/existing/out.prl");
        assert_eq!(output_dir_to_create(&out, true), None);
    }

    #[test]
    fn output_dir_bare_filename_proceeds() {
        // No parent component → current directory, which always exists.
        let out = PathBuf::from("out.prl");
        assert_eq!(output_dir_to_create(&out, false), None);
    }

    #[test]
    fn output_dir_missing_parent_returns_dir() {
        // Missing parent → returns the directory that must be created.
        let out = PathBuf::from("/some/missing/out.prl");
        assert_eq!(
            output_dir_to_create(&out, false),
            Some(PathBuf::from("/some/missing"))
        );
    }

    #[test]
    fn dir_answer_yes_variants_create() {
        for yes in ["y", "yes", "Y", "YES", "  yes  ", "Yes\n"] {
            assert_eq!(
                parse_dir_answer(Some(yes)),
                DirAnswer::Create,
                "answer {yes:?} should map to Create"
            );
        }
    }

    #[test]
    fn dir_answer_bare_enter_is_default_create() {
        // `[Y/n]` means Enter == Yes. A bare newline (empty trimmed) creates.
        assert_eq!(parse_dir_answer(Some("\n")), DirAnswer::Create);
        assert_eq!(parse_dir_answer(Some("")), DirAnswer::Create);
    }

    #[test]
    fn dir_answer_no_variants_abort() {
        for no in ["n", "no", "N", "NO", "  no  ", "No\n"] {
            assert_eq!(
                parse_dir_answer(Some(no)),
                DirAnswer::Abort,
                "answer {no:?} should map to Abort"
            );
        }
    }

    #[test]
    fn dir_answer_eof_aborts() {
        // None models EOF (read_line returned 0 bytes) → abort, never hang/loop.
        assert_eq!(parse_dir_answer(None), DirAnswer::Abort);
    }

    #[test]
    fn dir_answer_garbage_reprompts() {
        for junk in ["maybe", "q", "1", "y e s"] {
            assert_eq!(
                parse_dir_answer(Some(junk)),
                DirAnswer::Reprompt,
                "answer {junk:?} should re-prompt rather than guess"
            );
        }
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
