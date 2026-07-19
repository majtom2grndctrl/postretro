//! `scripts-build` CLI driver — argv parsing only; bundling lives in `lib.rs`.
//! Governed by `context/lib/scripting.md`.
//!
//! **This tool bundles and transpiles, it does not type-check.** Run
//! `tsc --noEmit` in your editor or CI for type safety. The sidecar's job is to:
//!
//! 1. Bundle the entry TypeScript file with all of its relative imports
//!    (`./foo`, `../bar/baz`) into a single self-contained module via
//!    `swc_bundler`.
//! 2. Strip TypeScript-only syntax (annotations, interfaces, enums, etc.) from
//!    every loaded file before the bundler stitches them together.
//! 3. Drop any remaining `import`/`export` declarations whose specifiers are
//!    bare (e.g. `"postretro"`). All engine APIs are injected as globals by
//!    the QuickJS host; bare specifiers are external by definition. QuickJS
//!    evaluates scripts in script mode (not ES module mode), so any surviving
//!    `import`/`export` would cause a syntax error.
//!
//! The sidecar exists so the `postretro` *runtime* binary never links `swc_*`
//! crates (which add meaningful binary size). Build-time prelude generation
//! uses this crate as a `[build-dependencies]` entry in `postretro/Cargo.toml`,
//! which does not affect the shipped engine binary. The `--dep-json` flag emits
//! a machine-readable dependency report (entry, output, dependencies) consumed
//! by the engine's staged manifest builder to track which source files belong to
//! the active mod-init dependency set.
//!
//! # CLI
//!
//! ```text
//! scripts-build --in <INPUT.ts> --out <OUTPUT.js>
//! scripts-build --in <INPUT.ts> --out <OUTPUT.js> --dep-json
//! scripts-build --in <INPUT.ts> --out <OUTPUT.js> --light-table <TABLE.json> --manifest-out <MANIFEST.json> [--mod-root <DIR>]
//! scripts-build --prelude --sdk-root <DIR> --out <OUTPUT.js>
//! ```
//!
//! In `--prelude` mode the bundler entry is `<DIR>/prelude.ts` and every named
//! export is rewritten as a `globalThis.<name> = <name>` assignment so the
//! resulting script, when evaluated in a QuickJS context, installs the SDK
//! library symbols as globals visible to subsequent user scripts. See
//! `context/lib/scripting.md §8` for the prelude design.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use postretro_level_format::light_membership::LightTable;
use postretro_script_compiler::{
    bundle_entry, bundle_entry_with_dependencies, light_membership::emit_light_membership_manifest,
    write_prelude,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("scripts-build: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mode = parse_args()?;

    match mode {
        CliMode::Bundle {
            input,
            output,
            dep_json,
            light_table,
            manifest_out,
            mod_root,
        } => {
            // Canonicalize so swc resolves relative imports against a stable
            // absolute path regardless of the cwd at invocation.
            let entry = std::fs::canonicalize(&input)
                .with_context(|| format!("failed to locate input `{}`", input.display()))?;
            let bundled = if is_luau_source(&entry) {
                postretro_script_compiler::BundleWithDependencies {
                    js: std::fs::read_to_string(&entry).with_context(|| {
                        format!("failed to read Luau input `{}`", entry.display())
                    })?,
                    dependencies: vec![entry.clone()],
                }
            } else if dep_json {
                bundle_entry_with_dependencies(&entry)?
            } else {
                postretro_script_compiler::BundleWithDependencies {
                    js: bundle_entry(&entry)?,
                    dependencies: Vec::new(),
                }
            };
            if let Some(parent) = output.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create output directory `{}`", parent.display())
                })?;
            }
            std::fs::write(&output, &bundled.js)
                .with_context(|| format!("failed to write output `{}`", output.display()))?;
            if let (Some(light_table), Some(manifest_out)) = (light_table, manifest_out) {
                let table_json = std::fs::read_to_string(&light_table).with_context(|| {
                    format!("failed to read light table `{}`", light_table.display())
                })?;
                let table: LightTable = serde_json::from_str(&table_json).with_context(|| {
                    format!("failed to parse light table `{}`", light_table.display())
                })?;
                let default_mod_root = entry.parent().unwrap_or_else(|| std::path::Path::new("."));
                let manifest = emit_light_membership_manifest(
                    &bundled.js,
                    &entry,
                    mod_root.as_deref().unwrap_or(default_mod_root),
                    &table,
                )
                .with_context(|| {
                    format!(
                        "failed to derive light-membership manifest for `{}`",
                        entry.display()
                    )
                })?;
                if let Some(parent) = manifest_out.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!(
                            "failed to create manifest output directory `{}`",
                            parent.display()
                        )
                    })?;
                }
                std::fs::write(&manifest_out, serde_json::to_vec(&manifest)?).with_context(
                    || {
                        format!(
                            "failed to write light-membership manifest `{}`",
                            manifest_out.display()
                        )
                    },
                )?;
            }
            if dep_json {
                let output = std::fs::canonicalize(&output).with_context(|| {
                    format!("failed to canonicalize output `{}`", output.display())
                })?;
                let report = serde_json::json!({
                    "entry": entry,
                    "output": output,
                    "dependencies": bundled.dependencies,
                });
                println!("{}", serde_json::to_string(&report)?);
            }
        }
        CliMode::Prelude { sdk_root, output } => {
            write_prelude(&sdk_root, &output)?;
        }
    }

    Ok(())
}

/// Parsed command-line invocation: either bundle a user entry script or build
/// the SDK-library prelude. The two modes share output handling but differ in
/// how the entry is located and how exports survive into the output JS.
enum CliMode {
    Bundle {
        input: PathBuf,
        output: PathBuf,
        dep_json: bool,
        light_table: Option<PathBuf>,
        manifest_out: Option<PathBuf>,
        mod_root: Option<PathBuf>,
    },
    Prelude {
        sdk_root: PathBuf,
        output: PathBuf,
    },
}

fn parse_args() -> Result<CliMode> {
    // Tiny hand-rolled parser. Two modes:
    //   * `--in <path> --out <path>` — bundle a user entry script.
    //   * `--prelude --sdk-root <dir> --out <path>` — bundle `<dir>/prelude.ts`
    //     with named exports rewritten as `globalThis.<name> = <name>`.
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut sdk_root: Option<PathBuf> = None;
    let mut prelude = false;
    let mut dep_json = false;
    let mut light_table: Option<PathBuf> = None;
    let mut manifest_out: Option<PathBuf> = None;
    let mut mod_root: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--in" => {
                input = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--in` requires a path argument"))?
                        .into(),
                );
            }
            "--out" => {
                output = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--out` requires a path argument"))?
                        .into(),
                );
            }
            "--prelude" => {
                prelude = true;
            }
            "--dep-json" => {
                dep_json = true;
            }
            "--light-table" => {
                light_table = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--light-table` requires a path argument"))?
                        .into(),
                );
            }
            "--manifest-out" => {
                manifest_out = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--manifest-out` requires a path argument"))?
                        .into(),
                );
            }
            "--mod-root" => {
                mod_root = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--mod-root` requires a path argument"))?
                        .into(),
                );
            }
            "--sdk-root" => {
                sdk_root = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--sdk-root` requires a path argument"))?
                        .into(),
                );
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!(
                "unknown argument `{other}` (expected `--in <path> --out <path>` \
                 or `--prelude --sdk-root <dir> --out <path>`)"
            ),
        }
    }
    let output = output.ok_or_else(|| anyhow!("missing `--out <path>`"))?;
    if light_table.is_some() != manifest_out.is_some() {
        bail!("`--light-table` and `--manifest-out` must be supplied together");
    }
    if prelude {
        if input.is_some() {
            bail!("`--in` is incompatible with `--prelude`");
        }
        if dep_json {
            bail!("`--dep-json` is incompatible with `--prelude`");
        }
        if light_table.is_some() {
            bail!("`--light-table` and `--manifest-out` are incompatible with `--prelude`");
        }
        if mod_root.is_some() {
            bail!("`--mod-root` is incompatible with `--prelude`");
        }
        let sdk_root =
            sdk_root.ok_or_else(|| anyhow!("`--prelude` requires `--sdk-root <dir>`"))?;
        Ok(CliMode::Prelude { sdk_root, output })
    } else {
        if sdk_root.is_some() {
            bail!("`--sdk-root` is only valid with `--prelude`");
        }
        let input = input.ok_or_else(|| anyhow!("missing `--in <path>`"))?;
        if mod_root.is_some() && light_table.is_none() {
            bail!("`--mod-root` is only valid with `--light-table` and `--manifest-out`");
        }
        Ok(CliMode::Bundle {
            input,
            output,
            dep_json,
            light_table,
            manifest_out,
            mod_root,
        })
    }
}

fn print_usage() {
    eprintln!(
        "scripts-build — bundle and transpile TypeScript to JavaScript (no type checking).\n\
         \n\
         USAGE:\n    scripts-build --in <INPUT.ts> --out <OUTPUT.js> [--dep-json] [--light-table <TABLE.json> --manifest-out <MANIFEST.json> [--mod-root <DIR>]]\n\
         \n    scripts-build --prelude --sdk-root <DIR> --out <OUTPUT.js>\n\
         \n\
         Run `tsc --noEmit` in your editor or CI for type safety."
    );
}

fn is_luau_source(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("luau"))
}
