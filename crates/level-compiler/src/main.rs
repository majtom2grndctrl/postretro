// postretro-level-compiler: level compiler entry point.
// See: context/lib/build_pipeline.md §PRL Compilation

pub mod affinity_grid;
pub mod animated_direct_sh_bake;
pub mod animated_light_chunks;
pub mod animated_light_weight_maps;
pub mod bake_control;
pub mod bc5;
pub mod bc6h;
pub mod bvh_build;
pub mod cache;
pub mod cell_draw_index_bake;
pub mod cell_visibility_bake;
pub mod chart_raster;
pub mod chunk_light_list_bake;
pub mod delta_drop_policy;
pub mod delta_sections;
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
pub mod script_light_membership;
pub mod sdf_bake;
pub mod sh_analyze;
pub mod sh_bake;
pub mod sh_coarsen;
pub mod sh_group;
pub mod sh_runtime_envelope;
pub mod shadowmask_bake;
pub mod size_options;
pub mod texture_mips;
pub mod texture_validation;
pub mod trigger_volumes;
pub mod tui;
pub mod visibility;

#[cfg(test)]
mod binary_tests;

use std::collections::HashSet;
use std::fmt::Display;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::Context as _;
use map_format::{DEFAULT_MAP_FORMAT, MapFormat};
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::light_membership::LightMembershipManifest;

static DATA_SCRIPT_TEMP_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    // Install capture immediately after argument parsing so every subsequent
    // early exit can emit the same exactly-once warning summary as a bake.
    let log_sink = logger::install(args.verbose)?;
    let reporter_mode = select_reporter_mode(
        args.tui,
        TerminalStreams {
            stdin: std::io::stdin().is_terminal(),
            stdout: std::io::stdout().is_terminal(),
            stderr: std::io::stderr().is_terminal(),
        },
    )
    .inspect_err(|_| {
        log_sink.print_warning_summary();
    })?;

    // Fail fast: if the output directory is missing, prompt to create it now —
    // before parsing the map or running any bake — so a missing folder never
    // wastes a multi-minute bake that only fails at the final write.
    precheck_output_dir(&args.output).inspect_err(|_| {
        log_sink.print_warning_summary();
    })?;

    if args.verbose {
        log::info!("Input: {}", args.input.display());
        log::info!("Output: {}", args.output.display());
        log::info!("Map format: {:?}", args.format);
    }

    if !args.format.is_supported() {
        log_sink.print_warning_summary();
        anyhow::bail!("map format '{:?}' is not yet supported", args.format);
    }

    // Construct stage cache. Default dir = <workspace-root>/.build-caches/prl-cache/.
    // --no-cache and --release both disable the cache entirely (no directory is
    // created), selecting the exact ship path (exact monolithic lightmap + exact
    // whole-volume SH). --release is the intent-named equivalent of the mechanical
    // --no-cache; routing both to `None` means the warm/cold branches in the
    // pipeline need no change. --cache-dir <path> overrides the default location
    // for warm builds; when --no-cache or --release is also supplied, the cache
    // stays disabled.
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

    let governor = std::sync::Arc::new(governor::Governor::new(args.jobs, false));
    match reporter_mode {
        ReporterMode::Plain => {
            let reporter = std::sync::Arc::new(reporter::PlainReporter::new(started, log_sink));
            let pipeline_reporter: std::sync::Arc<dyn reporter::Reporter> = reporter.clone();
            let result = pipeline::run(&args, stage_cache, started, pipeline_reporter, governor);
            if result.is_err() {
                reporter::Reporter::finalize_failure(reporter.as_ref());
            }
            result
        }
        ReporterMode::Tui => {
            // The planned list is content-dependent. Parse once before entering
            // the alternate screen, then pass that parsed map to the worker.
            // Its measured duration remains the Parsing Build Summary row.
            let prepared = match pipeline::prepare(&args) {
                Ok(prepared) => prepared,
                Err(error) => {
                    log_sink.print_warning_summary();
                    return Err(error);
                }
            };
            let planned = pipeline::planned_stages(&prepared.map_data.lights);
            tui::run_tui(planned, log_sink, governor, move |reporter, governor| {
                pipeline::run_prepared(&args, stage_cache, started, reporter, governor, prepared)
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TuiPreference {
    Auto,
    Force,
    Disable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReporterMode {
    Plain,
    Tui,
}

#[derive(Clone, Copy, Debug)]
struct TerminalStreams {
    stdin: bool,
    stdout: bool,
    stderr: bool,
}

fn select_reporter_mode(
    preference: TuiPreference,
    streams: TerminalStreams,
) -> anyhow::Result<ReporterMode> {
    let all_terminals = streams.stdin && streams.stdout && streams.stderr;
    match (preference, all_terminals) {
        (TuiPreference::Disable, _) => Ok(ReporterMode::Plain),
        (TuiPreference::Force, true) | (TuiPreference::Auto, true) => Ok(ReporterMode::Tui),
        (TuiPreference::Auto, false) => Ok(ReporterMode::Plain),
        (TuiPreference::Force, false) => anyhow::bail!(
            "--tui requires stdin, stdout, and stderr to all be attached to terminals"
        ),
    }
}

fn default_jobs_for(logical_cores: usize) -> usize {
    match logical_cores {
        0 | 1 => 1,
        2..=8 => logical_cores - 1,
        _ => logical_cores - 2,
    }
}

fn default_jobs() -> usize {
    default_jobs_for(
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1),
    )
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    verbose: bool,
    format: MapFormat,
    probe_spacing: f32,
    /// Fixed lightmap texel size in meters. The multi-bin packer opens new
    /// array layers instead of failing on atlas area, so there is no
    /// density-coarsening retry. `None` means the flag was not passed — the
    /// effective bake density falls through to the worldspawn
    /// `_lightmap_density` KVP, then to
    /// `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`. Passing the flag
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
    /// Maximum aggregate raw payload size for the three baked delta sections.
    /// This compiler-only setting is enforced by the post-bake delta policy;
    /// it has no PRL, FGD, loader, or runtime representation.
    delta_section_config: delta_sections::DeltaSectionConfig,
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
    /// Maximum concurrent governed bake work items.
    jobs: usize,
    /// Interactive reporter selection policy.
    tui: TuiPreference,
    /// When true, run the output-preserving SH coarsenability analysis pass
    /// after all base + delta SH bakes are available. Measurement only — it
    /// emits log/JSON diagnostics and changes no emitted `.prl` bytes.
    sh_analyze: bool,
    /// Destination path for the machine-readable per-brick + aggregate SH
    /// analysis JSON. `None` with `sh_analyze` set defaults to
    /// `<output>.sh-analysis.json`. Ignored when `sh_analyze` is false.
    sh_analyze_out: Option<PathBuf>,
    /// Protection-volume stand-in for SH coarsening and analysis. Each entry is a
    /// world-space AABB `[minx, miny, minz, maxx, maxy, maxz]`; any 4×4×4 brick
    /// intersecting any AABB is forced to keep full L0 density in the
    /// id-41 classifier and the analysis's protected projection. Repeatable.
    /// Compiler-only measurement input; never stored.
    sh_protect_aabbs: Vec<[f32; 6]>,
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
         -j, --jobs <N>             Initial compiler worker permits, >= 1 (default: {jobs})\n    \
         --tui                      Force the interactive UI (requires terminal stdin/stdout/stderr)\n    \
         --no-tui                   Disable the interactive UI and print line-oriented progress\n    \
         -v, --verbose              Verbose stage logging to stderr (default: off)\n    \
         --format <FORMAT>          Map source format: idtech2 | idtech3 | idtech4 (default: idtech2)\n    \
         --sh-probe-spacing <METERS> SH irradiance probe spacing in meters, > 0 (default: {probe})\n    \
         --lightmap-density <METERS> Fixed lightmap texel size in meters, > 0 (default: {density})\n    \
         --soft-shadow-samples <N>  Soft-shadow penumbra area-sample count, >= {probe_floor} (default: {samples})\n    \
         --sdf-voxel-size <METERS>  SDF occluder-atlas voxel edge length in meters, > 0 (default: {voxel})\n    \
         --cache-dir <PATH>         Override the stage-cache directory (default: <workspace>/.build-caches/prl-cache)\n    \
         --cache-max-size <SIZE>    LRU budget for the stage cache, pruned at build start; accepts e.g. 2GiB, 512MiB, or a byte count (default: {cache_max})\n    \
         --sh-delta-max-size <SIZE> Aggregate raw payload cap for ids 27, 41, and 45 after the compiler delta policy; accepts e.g. 256MiB or a byte count (default: {delta_max})\n    \
         --no-cache                 Disable the stage cache entirely; wins over --cache-dir (default: off)\n    \
         --release                  Produce a shippable map: exact lighting, cache bypassed (implies --no-cache). The interactive default is a fast warm build with approximate indirect lighting; ship only --release artifacts (default: off)\n    \
         --uncompressed-irradiance  Store the lightmap irradiance atlas uncompressed as Rgba16Float instead of BC6H — larger; for debugging/quality comparison (default: off, BC6H)\n    \
         --sh-analyze               Run the output-preserving SH coarsenability analysis pass (measurement only; emits summary + JSON, changes no emitted bytes) (default: off)\n    \
         --sh-analyze-out <PATH>    Destination for the SH analysis JSON (default: <output>.sh-analysis.json when --sh-analyze is set)\n    \
         --sh-protect-aabb <AABB>   Force L0 for id-41 bricks intersecting a world-space AABB minx,miny,minz,maxx,maxy,maxz; repeatable (default: none)\n    \
         -h, --help                 Print this help and exit\n",
        probe = sh_bake::DEFAULT_PROBE_SPACING,
        density = lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS,
        samples = lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
        probe_floor = lightmap_bake::SOFT_PROBE_SAMPLES,
        voxel = sdf_bake::DEFAULT_VOXEL_SIZE_METERS,
        cache_max = size_options::format_size_for_help(cache::DEFAULT_MAX_BYTES),
        delta_max = size_options::format_size_for_help(delta_sections::DEFAULT_MAX_PAYLOAD_BYTES),
        jobs = default_jobs(),
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
    let mut cache_max_bytes = cache::DEFAULT_MAX_BYTES;
    let mut delta_section_config = delta_sections::DeltaSectionConfig::default();
    let mut no_cache = false;
    let mut release = false;
    let mut uncompressed_irradiance = false;
    let mut jobs = default_jobs();
    let mut tui = TuiPreference::Auto;
    let mut sh_analyze = false;
    let mut sh_analyze_out: Option<PathBuf> = None;
    let mut sh_protect_aabbs: Vec<[f32; 6]> = Vec::new();

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
            "-j" | "--jobs" => {
                let jobs_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
                jobs = jobs_str
                    .parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("{arg} must be an integer >= 1"))?;
                if jobs == 0 {
                    anyhow::bail!("{arg} must be an integer >= 1");
                }
            }
            "--tui" => {
                if tui == TuiPreference::Disable {
                    anyhow::bail!("--tui and --no-tui are mutually exclusive");
                }
                tui = TuiPreference::Force;
            }
            "--no-tui" => {
                if tui == TuiPreference::Force {
                    anyhow::bail!("--tui and --no-tui are mutually exclusive");
                }
                tui = TuiPreference::Disable;
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
                cache_max_bytes = size_options::parse_size("--cache-max-size", &size_str)?;
            }
            "--sh-delta-max-size" => {
                let size_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sh-delta-max-size requires a value"))?;
                delta_section_config.max_payload_bytes =
                    size_options::parse_size("--sh-delta-max-size", &size_str)?;
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
            "--sh-analyze" => {
                sh_analyze = true;
            }
            "--sh-analyze-out" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sh-analyze-out requires a path"))?;
                sh_analyze_out = Some(PathBuf::from(path));
            }
            "--sh-protect-aabb" => {
                let spec = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--sh-protect-aabb requires a value"))?;
                sh_protect_aabbs.push(parse_protect_aabb(&spec)?);
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
             [--soft-shadow-samples <N>] [--sdf-voxel-size <METERS>] [--cache-dir <PATH>] [--cache-max-size <SIZE>] [--sh-delta-max-size <SIZE>] [--no-cache] [--release]\n\
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
        delta_section_config,
        no_cache,
        release,
        uncompressed_irradiance,
        jobs,
        tui,
        sh_analyze,
        sh_analyze_out,
        sh_protect_aabbs,
    })
}

/// Parse a `--sh-protect-aabb minx,miny,minz,maxx,maxy,maxz` value into a
/// world-space AABB. Each of the six comma-separated fields must be a finite
/// number, and each max must be >= its matching min.
fn parse_protect_aabb(spec: &str) -> anyhow::Result<[f32; 6]> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 6 {
        anyhow::bail!(
            "--sh-protect-aabb expects 6 comma-separated numbers \
             (minx,miny,minz,maxx,maxy,maxz), got {}",
            parts.len()
        );
    }
    let mut v = [0.0f32; 6];
    for (i, part) in parts.iter().enumerate() {
        let parsed: f32 = part.trim().parse().map_err(|_| {
            anyhow::anyhow!(
                "--sh-protect-aabb field {} is not a number: {part:?}",
                i + 1
            )
        })?;
        if !parsed.is_finite() {
            anyhow::bail!("--sh-protect-aabb field {} must be finite", i + 1);
        }
        v[i] = parsed;
    }
    for axis in 0..3 {
        if v[axis + 3] < v[axis] {
            anyhow::bail!(
                "--sh-protect-aabb max[{axis}] ({}) must be >= min[{axis}] ({})",
                v[axis + 3],
                v[axis]
            );
        }
    }
    Ok(v)
}

/// Locate the `scripts-build` sidecar for compiling and evaluating worldspawn
/// data scripts.
///
// TODO(scripting-tools-dedup): duplicates `TsCompilerPath::detect` in
// `crates/scripting-core/src/watcher.rs`, reached by the engine through a
// debug-only compatibility re-export. The level-compiler still cannot import
// the engine-side wrapper. The matching subprocess invocation lives in
// `run_ts_compiler` in the watcher module. Consolidate into a
// shared `postretro-scripts-tools` crate when the level-compiler gains more
// scripting integration. See:
// context/plans/drafts/scripting-tools-dedup/index.md
fn is_compiler_stale(binary_path: &Path) -> bool {
    let source_dirs = compiler_freshness_roots();
    if source_dirs.iter().all(|source_dir| !source_dir.is_dir()) {
        return false;
    }
    let sidecar_mtime = match std::fs::metadata(binary_path).and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Find newest source mtime recursively
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = source_dirs;
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

fn compiler_freshness_roots() -> Vec<PathBuf> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("level compiler lives two directories below the workspace")
        .to_path_buf();
    vec![
        workspace_root.join("crates/script-compiler/src"),
        // scripts-build embeds and evaluates this SDK source during manifest
        // derivation. An SDK edit must invalidate the production sidecar even
        // when the Rust crate itself is unchanged.
        workspace_root.join("sdk/lib"),
    ]
}

/// Return the non-empty streams captured from a child process.
fn captured_command_output(stdout: &[u8], stderr: &[u8]) -> Vec<(&'static str, String)> {
    [("stdout", stdout), ("stderr", stderr)]
        .into_iter()
        .filter_map(|(stream, bytes)| {
            let output = String::from_utf8_lossy(bytes);
            (!output.trim().is_empty()).then(|| (stream, output.into_owned()))
        })
        .collect()
}

/// Send captured child-process streams through the compiler logger.
///
/// A data-script sidecar rebuild may run while the TUI owns the alternate
/// screen. Child processes must therefore never inherit the terminal: their
/// output is captured, then the active reporter renders it in its log pane.
fn log_captured_command_output(label: &str, stdout: &[u8], stderr: &[u8], level: log::Level) {
    for (stream, output) in captured_command_output(stdout, stderr) {
        log::log!(level, "[prl-build] {label} {stream}:\n{output}");
    }
}

fn scripts_build_beside(exe_dir: &Path, name: &str) -> Option<PathBuf> {
    let profile_binary = exe_dir.parent().map(|parent| parent.join(name));

    // A Cargo unit-test executable lives in `target/<profile>/deps/`. Cargo
    // builds the standalone sidecar at `target/<profile>/`, while a stale
    // un-hashed compatibility binary can remain in `deps/`. Prefer the
    // profile-level sidecar for that test-only layout so a child `cargo build`
    // is also the binary we subsequently invoke.
    if exe_dir
        .file_name()
        .is_some_and(|component| component == "deps")
        && let Some(profile_binary) = profile_binary.as_ref()
        && profile_binary.is_file()
    {
        return Some(profile_binary.clone());
    }

    let adjacent = exe_dir.join(name);
    if adjacent.is_file() {
        return Some(adjacent);
    }

    // A test target without a profile-level sidecar still falls through here,
    // preserving the normal adjacent-then-parent discovery cascade.
    profile_binary.filter(|binary| binary.is_file())
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

    let path = exe_dir
        .as_deref()
        .and_then(|dir| scripts_build_beside(dir, name));

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

    // Cargo does not wire `scripts-build` into this binary test target as a
    // `CARGO_BIN_EXE_*` dependency. A previous `cargo build` can therefore
    // leave an older sibling binary in `target/<profile>/` even though the
    // tests exercise newly compiled `prl-build` code. Rebuild in test builds
    // so the subprocess has the CLI contract the test target expects. Normal
    // development and release builds retain the source-mtime freshness check.
    let needs_build = cfg!(test)
        || match &path {
            None => true,
            Some(p) => is_compiler_stale(p),
        };

    if needs_build {
        log::info!("[prl-build] scripts-build is missing or stale. Rebuilding via cargo...");
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("build")
            .arg("-p")
            .arg("postretro-script-compiler")
            .arg("--bin")
            .arg("scripts-build");
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        match cmd.output() {
            Ok(output) if output.status.success() => {
                log_captured_command_output(
                    "cargo build",
                    &output.stdout,
                    &output.stderr,
                    log::Level::Info,
                );
                log::info!("[prl-build] scripts-build compiled successfully.");
                if let Some(ref dir) = exe_dir
                    && let Some(candidate) = scripts_build_beside(dir, name)
                {
                    return Some(candidate);
                }
            }
            Ok(output) => {
                log::error!(
                    "[prl-build] Failed to compile scripts-build: exit code {}",
                    output.status
                );
                log_captured_command_output(
                    "cargo build",
                    &output.stdout,
                    &output.stderr,
                    log::Level::Error,
                );
            }
            Err(err) => {
                log::error!("[prl-build] Failed to spawn cargo build: {}", err);
            }
        }
    }

    path
}

/// The compiled PRL payload paired with the sidecar that determined its
/// compiler-only light membership. The manifest is never serialized into PRL;
/// it has already been applied before the bake namespaces are formed.
#[derive(Debug)]
pub(crate) struct CompiledDataScript {
    pub(crate) section: DataScriptSection,
    pub(crate) membership_manifest: LightMembershipManifest,
}

/// Private staging directory for the JSON sidecars and compiled/pass-through
/// script output. A directory, rather than predictable filenames in the map
/// folder, prevents stale sidecars from one build being mistaken for another.
struct DataScriptTempDir {
    path: PathBuf,
    light_table_path: PathBuf,
    manifest_path: PathBuf,
    script_output_path: PathBuf,
    cleaned: bool,
}

impl DataScriptTempDir {
    fn create() -> anyhow::Result<Self> {
        let base = std::env::temp_dir();
        let process_id = std::process::id();
        for _ in 0..64 {
            let sequence = DATA_SCRIPT_TEMP_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("postretro-prl-build-{process_id}-{sequence}"));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        light_table_path: path.join("light-table.json"),
                        manifest_path: path.join("light-membership.json"),
                        script_output_path: path.join("data-script.out"),
                        path,
                        cleaned: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "[prl-build] failed to create data-script temporary directory {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        anyhow::bail!(
            "[prl-build] could not allocate a unique data-script temporary directory under {}",
            base.display()
        );
    }

    fn write_light_table(
        &self,
        table: &postretro_level_format::light_membership::LightTable,
    ) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(table)
            .context("[prl-build] failed to serialize light table for scripts-build")?;
        std::fs::write(&self.light_table_path, bytes).map_err(|error| {
            anyhow::anyhow!(
                "[prl-build] failed to write light table {}: {error}",
                self.light_table_path.display()
            )
        })
    }

    fn read_manifest(&self) -> anyhow::Result<LightMembershipManifest> {
        let bytes = std::fs::read(&self.manifest_path).map_err(|error| {
            anyhow::anyhow!(
                "[prl-build] scripts-build did not produce light-membership manifest {}: {error}",
                self.manifest_path.display()
            )
        })?;
        let manifest =
            serde_json::from_slice::<LightMembershipManifest>(&bytes).map_err(|error| {
                anyhow::anyhow!(
                    "[prl-build] malformed light-membership manifest {}: {error}",
                    self.manifest_path.display()
                )
            })?;
        manifest.validate_version().map_err(|error| {
            anyhow::anyhow!(
                "[prl-build] stale or unsupported light-membership manifest {}: {error}",
                self.manifest_path.display()
            )
        })?;
        Ok(manifest)
    }

    fn cleanup(mut self) -> anyhow::Result<()> {
        self.cleaned = true;
        std::fs::remove_dir_all(&self.path).map_err(|error| {
            anyhow::anyhow!(
                "[prl-build] failed to remove data-script temporary directory {}: {error}",
                self.path.display()
            )
        })
    }
}

impl Drop for DataScriptTempDir {
    fn drop(&mut self) {
        if !self.cleaned
            && let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "[prl-build] failed to remove data-script temporary directory {} after an error: {error}",
                self.path.display()
            );
        }
    }
}

/// Compile and evaluate the worldspawn `data_script`, if present.
///
/// The same `scripts-build --in/--out` invocation produces the PRL's script
/// bytes and a mandatory, versioned membership sidecar. An absent KVP remains
/// the normal no-script path; once a script is present, a missing or malformed
/// sidecar is a build error rather than a silently unanimated static light.
fn compile_worldspawn_data_script(
    map_path: &Path,
    data_script_path: Option<&str>,
    lights: &[map_data::MapLight],
) -> anyhow::Result<Option<CompiledDataScript>> {
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

    match extension.as_deref() {
        Some("ts") | Some("js") | Some("luau") => {}
        Some(other) => anyhow::bail!(
            "[prl-build] data_script = {rel} has unsupported extension '.{other}' (expected .ts, .js, or .luau)"
        ),
        None => anyhow::bail!(
            "[prl-build] data_script = {rel} has no file extension (expected .ts, .js, or .luau)"
        ),
    }

    let temporary = DataScriptTempDir::create()?;
    let light_table = crate::script_light_membership::light_table_from_lights(lights)?;
    temporary.write_light_table(&light_table)?;

    // Always stage emitted bytes away from authored content. In particular, a
    // `.js` input must never select itself as `--out` and be truncated while
    // scripts-build is still deriving the manifest.
    let output_path = temporary.script_output_path.clone();
    let compiler = find_scripts_build().ok_or_else(|| {
        anyhow::anyhow!(
            "[prl-build] data_script {} requires scripts-build to derive light membership, but scripts-build was not found or could not be built",
            source_path.display()
        )
    })?;
    log::info!(
        "[prl-build] compiling data_script {} -> {} with light-membership sidecar via {}",
        source_path.display(),
        output_path.display(),
        compiler.display()
    );
    let output = std::process::Command::new(&compiler)
        .arg("--in")
        .arg(&source_path)
        .arg("--out")
        .arg(&output_path)
        .arg("--light-table")
        .arg(&temporary.light_table_path)
        .arg("--manifest-out")
        .arg(&temporary.manifest_path)
        .arg("--mod-root")
        .arg(resolve_content_root(map_path))
        .output()
        .map_err(|error| {
            anyhow::anyhow!(
                "[prl-build] failed to spawn scripts-build at {} for data_script {}: {error}",
                compiler.display(),
                source_path.display()
            )
        })?;
    if !output.status.success() {
        log_captured_command_output(
            "scripts-build",
            &output.stdout,
            &output.stderr,
            log::Level::Error,
        );
        let diagnostic = captured_command_output(&output.stdout, &output.stderr)
            .into_iter()
            .map(|(stream, text)| format!("{stream}: {text}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "[prl-build] scripts-build failed for data_script {}: exit status {}; {}",
            source_path.display(),
            output.status,
            diagnostic
        );
    }

    let membership_manifest = temporary.read_manifest()?;
    let compiled_bytes = std::fs::read(&output_path).map_err(|error| {
        anyhow::anyhow!(
            "[prl-build] failed to read compiled data_script {}: {error}",
            output_path.display()
        )
    })?;
    temporary.cleanup()?;

    let absolute_source_path = std::fs::canonicalize(&source_path)
        .unwrap_or(source_path.clone())
        .to_string_lossy()
        .into_owned();

    log::info!(
        "[prl-build] data_script embedded: {} bytes from {}",
        compiled_bytes.len(),
        absolute_source_path
    );

    Ok(Some(CompiledDataScript {
        section: pack::encode_data_script(compiled_bytes, absolute_source_path),
        membership_manifest,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_command_output_preserves_both_streams_without_terminal_writes() {
        // Regression: Cargo inherited stdout/stderr while the TUI owned the
        // alternate screen, corrupting the rendered interface.
        let output = captured_command_output(b"build note\n", b"build warning\n");
        assert_eq!(
            output,
            vec![
                ("stdout", "build note\n".to_owned()),
                ("stderr", "build warning\n".to_owned()),
            ]
        );
    }

    #[test]
    fn captured_command_output_omits_blank_streams() {
        assert!(captured_command_output(b"  \n", b"").is_empty());
    }

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

    #[test]
    fn scripts_build_beside_test_binary_prefers_profile_sidecar() {
        let root = unique_temp_dir("scripts-build-resolution");
        let deps = root.join("debug/deps");
        std::fs::create_dir_all(&deps).expect("create test executable directory");
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        let stale_deps_binary = deps.join(name);
        let current_profile_binary = root.join("debug").join(name);
        std::fs::write(&stale_deps_binary, "stale").expect("write stale deps sidecar");
        std::fs::write(&current_profile_binary, "current").expect("write current profile sidecar");

        assert_eq!(
            scripts_build_beside(&deps, name),
            Some(current_profile_binary),
            "a Cargo test binary must ignore a stale deps/scripts-build in favor of the profile sidecar"
        );
        std::fs::remove_dir_all(root).expect("remove sidecar-resolution fixture");
    }

    #[test]
    fn scripts_build_freshness_tracks_embedded_sdk_sources() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let roots = compiler_freshness_roots();
        assert!(roots.contains(&workspace.join("crates/script-compiler/src")));
        assert!(
            roots.contains(&workspace.join("sdk/lib")),
            "SDK sources embedded by scripts-build must participate in production freshness"
        );
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
        assert_eq!(
            parsed.delta_section_config.max_payload_bytes,
            delta_sections::DEFAULT_MAX_PAYLOAD_BYTES
        );
        assert_eq!(parsed.jobs, default_jobs());
        assert_eq!(parsed.tui, TuiPreference::Auto);
    }

    #[test]
    fn default_jobs_leaves_headroom() {
        assert_eq!(default_jobs_for(0), 1);
        assert_eq!(default_jobs_for(1), 1);
        assert_eq!(default_jobs_for(2), 1);
        assert_eq!(default_jobs_for(8), 7);
        assert_eq!(default_jobs_for(9), 7);
        assert_eq!(default_jobs_for(16), 14);
    }

    #[test]
    fn parse_args_jobs_accepts_both_spellings_and_rejects_invalid_values() {
        let parsed =
            parse_args_from(["input.map", "-j", "3"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(parsed.jobs, 3);

        let parsed =
            parse_args_from(["input.map", "--jobs", "5"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(parsed.jobs, 5);

        for value in ["0", "-1", "many"] {
            assert!(
                parse_args_from(
                    ["input.map", "--jobs", value]
                        .into_iter()
                        .map(str::to_owned)
                )
                .is_err()
            );
        }
        assert!(parse_args_from(["input.map", "--jobs"].into_iter().map(str::to_owned)).is_err());
    }

    #[test]
    fn parse_args_tui_preferences_are_mutually_exclusive() {
        let forced =
            parse_args_from(["input.map", "--tui"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(forced.tui, TuiPreference::Force);

        let disabled =
            parse_args_from(["input.map", "--no-tui"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(disabled.tui, TuiPreference::Disable);

        for flags in [["--tui", "--no-tui"], ["--no-tui", "--tui"]] {
            assert!(
                parse_args_from(
                    ["input.map", flags[0], flags[1]]
                        .into_iter()
                        .map(str::to_owned)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn reporter_selection_uses_all_three_terminal_streams() {
        let terminals = TerminalStreams {
            stdin: true,
            stdout: true,
            stderr: true,
        };
        assert_eq!(
            select_reporter_mode(TuiPreference::Auto, terminals).unwrap(),
            ReporterMode::Tui
        );
        assert_eq!(
            select_reporter_mode(TuiPreference::Disable, terminals).unwrap(),
            ReporterMode::Plain
        );

        for streams in [
            TerminalStreams {
                stdin: false,
                ..terminals
            },
            TerminalStreams {
                stdout: false,
                ..terminals
            },
            TerminalStreams {
                stderr: false,
                ..terminals
            },
        ] {
            assert_eq!(
                select_reporter_mode(TuiPreference::Auto, streams).unwrap(),
                ReporterMode::Plain
            );
            assert_eq!(
                select_reporter_mode(TuiPreference::Disable, streams).unwrap(),
                ReporterMode::Plain
            );
            assert!(select_reporter_mode(TuiPreference::Force, streams).is_err());
        }
    }

    #[test]
    fn help_lists_jobs_and_tui_flags() {
        let help = help_text();
        assert!(help.contains("-j, --jobs <N>"));
        assert!(help.contains("--tui"));
        assert!(help.contains("--no-tui"));
        assert!(help.contains("--sh-delta-max-size <SIZE>"));
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
    fn parse_args_sh_analyze_defaults_off() {
        let parsed = parse_args_from(vec!["input.map".to_string()].into_iter()).unwrap();
        assert!(!parsed.sh_analyze);
        assert!(parsed.sh_analyze_out.is_none());
        assert!(parsed.sh_protect_aabbs.is_empty());
    }

    #[test]
    fn parse_args_rejects_retired_sh_coarsen_flag() {
        let error =
            parse_args_from(vec!["input.map".to_string(), "--sh-coarsen".to_string()].into_iter())
                .expect_err("the pre-release flag has no compatibility shim");
        assert!(
            error
                .to_string()
                .contains("unexpected argument: --sh-coarsen")
        );
    }

    #[test]
    fn parse_args_sh_analyze_flags() {
        let args = vec![
            "input.map".to_string(),
            "--sh-analyze".to_string(),
            "--sh-analyze-out".to_string(),
            "/tmp/out.json".to_string(),
            "--sh-protect-aabb".to_string(),
            "-1,-2,-3,4,5,6".to_string(),
            "--sh-protect-aabb".to_string(),
            "10,10,10,20,20,20".to_string(),
        ];
        let parsed = parse_args_from(args.into_iter()).unwrap();
        assert!(parsed.sh_analyze);
        assert_eq!(parsed.sh_analyze_out, Some(PathBuf::from("/tmp/out.json")));
        assert_eq!(parsed.sh_protect_aabbs.len(), 2);
        assert_eq!(
            parsed.sh_protect_aabbs[0],
            [-1.0, -2.0, -3.0, 4.0, 5.0, 6.0]
        );
        assert_eq!(
            parsed.sh_protect_aabbs[1],
            [10.0, 10.0, 10.0, 20.0, 20.0, 20.0]
        );
    }

    #[test]
    fn parse_protect_aabb_rejects_bad_field_count() {
        assert!(parse_protect_aabb("1,2,3").is_err());
        assert!(parse_protect_aabb("1,2,3,4,5,6,7").is_err());
    }

    #[test]
    fn parse_protect_aabb_rejects_inverted_bounds() {
        // max.x < min.x.
        let err = parse_protect_aabb("5,0,0,1,10,10").unwrap_err();
        assert!(err.to_string().contains("must be >="), "got: {err}");
    }

    #[test]
    fn parse_protect_aabb_rejects_non_number() {
        assert!(parse_protect_aabb("1,2,x,4,5,6").is_err());
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
    fn parse_args_sh_delta_max_size_defaults_to_unconditional_256_mib_cap() {
        let parsed = parse_args_from(["input.map"].into_iter().map(str::to_owned)).unwrap();

        assert_eq!(
            parsed.delta_section_config.max_payload_bytes,
            256 * 1024 * 1024
        );
    }

    #[test]
    fn parse_args_sh_delta_max_size_accepts_existing_size_syntax() {
        let parsed = parse_args_from(
            ["input.map", "--sh-delta-max-size", "1.5GiB"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(
            parsed.delta_section_config.max_payload_bytes,
            1536 * 1024 * 1024
        );
    }

    #[test]
    fn parse_args_sh_delta_max_size_requires_a_valid_value() {
        assert!(
            parse_args_from(
                ["input.map", "--sh-delta-max-size"]
                    .into_iter()
                    .map(str::to_owned),
            )
            .is_err()
        );
        assert!(
            parse_args_from(
                ["input.map", "--sh-delta-max-size", "12XB"]
                    .into_iter()
                    .map(str::to_owned),
            )
            .is_err()
        );
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
        let result = compile_worldspawn_data_script(Path::new("/dev/null/fake.map"), None, &[])
            .expect("None KVP must succeed");
        assert!(
            result.is_none(),
            "absent data_script KVP must not emit a DataScript section"
        );
    }

    #[test]
    fn data_script_sidecar_supports_light_membership_flags() {
        // Regression: a stale sibling `scripts-build` previously accepted the
        // old CLI but rejected the paired manifest arguments that prl-build
        // now always supplies for a data script.
        let compiler =
            find_scripts_build().expect("scripts-build must resolve for data-script tests");
        let output = std::process::Command::new(&compiler)
            .arg("--help")
            .output()
            .expect("run resolved scripts-build");
        assert!(
            output.status.success(),
            "resolved scripts-build should print help: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8_lossy(&output.stderr);
        assert!(
            help.contains("--light-table"),
            "resolved scripts-build must support --light-table: {help}"
        );
        assert!(
            help.contains("--manifest-out"),
            "resolved scripts-build must support --manifest-out: {help}"
        );
        assert!(
            help.contains("--mod-root"),
            "resolved scripts-build must support runtime-parity Luau module resolution: {help}"
        );
    }

    #[test]
    fn data_script_missing_file_is_hard_error() {
        let tmp_dir = std::env::temp_dir().join("postretro_data_script_missing");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let map_path = tmp_dir.join("test.map");
        let _ = std::fs::write(&map_path, "");
        let result = compile_worldspawn_data_script(&map_path, Some("does-not-exist.ts"), &[]);
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
        let tmp_dir = unique_temp_dir("data-script-luau");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let map_path = tmp_dir.join("test.map");
        std::fs::write(&map_path, "").unwrap();
        let luau_path = tmp_dir.join("level-data.luau");
        let luau_source = "function setupLevel(_)\n  return { reactions = { defineReaction(\"noop\", { primitive = \"noop\" }) } }\nend\n";
        std::fs::write(&luau_path, luau_source).unwrap();

        let compiled = compile_worldspawn_data_script(&map_path, Some("level-data.luau"), &[])
            .expect("luau data_script should compile")
            .expect("section must be emitted");

        assert_eq!(compiled.section.compiled_bytes, luau_source.as_bytes());
        assert!(
            compiled.section.source_path.ends_with("level-data.luau"),
            "source_path should reference the .luau file, got: {}",
            compiled.section.source_path
        );
        assert!(compiled.membership_manifest.lights.is_empty());
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn data_script_javascript_source_is_never_used_as_staged_output() {
        let tmp_dir = unique_temp_dir("data-script-js-source");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let map_path = tmp_dir.join("test.map");
        std::fs::write(&map_path, "").unwrap();
        let source_path = tmp_dir.join("level-data.js");
        let source = "globalThis.setupLevel = function() { return { reactions: [] }; };\n";
        std::fs::write(&source_path, source).unwrap();

        let compiled = compile_worldspawn_data_script(&map_path, Some("level-data.js"), &[])
            .expect("JavaScript data script should compile")
            .expect("section must be emitted");

        assert_eq!(
            std::fs::read_to_string(&source_path).expect("read authored source after compile"),
            source,
            "scripts-build output must never overwrite the authored .js input"
        );
        assert!(!compiled.section.compiled_bytes.is_empty());
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn fixture_keeps_script_and_kvp_animated_lights_distinct() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("level compiler lives two directories below the workspace")
            .to_path_buf();
        let map_path = workspace.join("content/dev/maps/script_light_membership_fixture.map");
        let mut map_data = parse::parse_map_file(&map_path, MapFormat::IdTech2)
            .expect("parse script-membership fixture");
        let compiled = compile_worldspawn_data_script(
            &map_path,
            map_data.data_script.as_deref(),
            &map_data.lights,
        )
        .expect("compile fixture data script")
        .expect("fixture has data_script KVP");

        let inventory = script_light_membership::apply_manifest(
            &mut map_data.lights,
            &map_data.light_start_active_defaults,
            &compiled.membership_manifest,
        )
        .expect("apply fixture membership manifest");

        assert_eq!(map_data.light_start_active_defaults, [false, true, true]);
        assert_eq!(inventory.derived_static_indices, vec![0]);
        assert!(
            map_data.lights[0]
                .animation
                .as_ref()
                .expect("script target receives an animation placeholder")
                .start_active,
            "a levelLoad animation defaults to active, overriding the authored _start_inactive fallback"
        );
        assert_eq!(
            light_namespaces::StaticBakedLights::from_lights(&map_data.lights).len(),
            1,
            "the steady-light control must remain in the ordinary static namespace"
        );
        assert_eq!(
            light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights).len(),
            2,
            "the KVP curve and script target must each reserve animated bake output"
        );
        assert!(!compiled.section.compiled_bytes.is_empty());
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
            carrier: String::new(),
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
