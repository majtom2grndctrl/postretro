//! SDK bundle assembly — the engine install a game developer builds against.
//!
//! The bundle ships no game content. It carries the engine and the content
//! tooling: a DEBUG engine built with `--features dev-tools` (TS auto-compile
//! and hot reload require debug assertions; the inspector requires the
//! feature), the compilers, the tool itself, a release engine for the player
//! payloads the recipient builds, and the engine-owned `sdk/`, `docs/`,
//! `tools/` and `core/` trees. Game developers bring their own content: the
//! workspace's `content/dev` is an engine-developer fixture and never enters a
//! bundle.
//!
//! The bundle root is still a project, so the tool works on arrival: it
//! carries a generated `postretro.toml` and an empty starter mod at
//! `content/base`, copied from `sdk/templates/starter/` with its entry script
//! compiled beside it. The starter declares no levels, so the bundle bakes
//! nothing and carries no `baked/` tree.
//!
//! The bundle is named [`SDK_BUNDLE_NAME`] whatever project runs the command —
//! the project supplies only the output root its containment guard anchors on.
//! It reuses the player payload's containment and completion-gate machinery.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::binaries::{Helper, run_checked};
use crate::dist::parse_args;
use crate::dist::payload::{MARKER_NAME, count_payload};
use crate::dist::resolve::guard_payload_root;
use crate::project::Project;

mod assemble;
mod readme;

use assemble::{BundleBinaries, sweep_sdk_bundle};

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let cli = parse_args(args, "sdk-dist")?;
    let project = cli.project()?;
    let install_root = cli.install_root()?;
    let output_root = cli.output_root(&project);
    let bundle_root = output_root.join(SDK_BUNDLE_NAME);

    guard_payload_root(&bundle_root, project.root())
        .map_err(|error| format!("sdk-dist: {error}"))?;

    let binaries = BundleBinaries {
        authoring_engine: cli.binaries.resolve(Helper::AuthoringEngine)?,
        release_engine: cli.binaries.resolve(Helper::ReleaseEngine)?,
        prl_build: cli.binaries.resolve(Helper::PrlBuild)?,
        scripts_build: cli.binaries.resolve(Helper::ScriptsBuild)?,
        mint_identity: cli.binaries.resolve(Helper::MintIdentity)?,
        tool: cli.binaries.resolve(Helper::Tool)?,
    };

    // Compiled into scratch before stage 5 deletes the previous bundle, so a
    // starter that fails to compile costs nothing already built.
    let starter_script = emit_starter_script(&project, &install_root, &binaries.scripts_build)?;

    assemble::assemble_bundle(
        &assemble::BundleTarget {
            project_root: project.root(),
            output_root: &output_root,
            bundle_root: &bundle_root,
            install_root: &install_root,
        },
        &binaries,
        &starter_script,
    )?;

    sweep_sdk_bundle(&bundle_root)?;
    fs::remove_file(bundle_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "sdk-dist: remove completion marker {}: {error}",
            bundle_root.join(MARKER_NAME).display()
        )
    })?;

    let (files, bytes) = count_payload(&bundle_root)?;
    println!(
        "SDK distribution complete: {files} files, {bytes} bytes at {}",
        bundle_root.display()
    );
    Ok(0)
}

/// Compile the starter's `start-script.ts` into the project's scratch tree.
///
/// The debug authoring engine recompiles `start-script.ts` at every mod init,
/// so this is not what makes the starter boot there. It proves the starter
/// compiles before a bundle ships, and the emitted `.js` is what the release
/// engine in `bin/` loads, since a release engine compiles no TypeScript.
fn emit_starter_script(
    project: &Project,
    install_root: &Path,
    scripts_build: &Path,
) -> Result<PathBuf, String> {
    println!("Stage 2: compile the starter mod's entry script");
    let starter = crate::engine_trees::resolve(install_root, STARTER_TEMPLATE)?;
    let source = starter.join(STARTER_ENTRY_SOURCE);
    let scratch = project.scratch_root().join(SDK_BUNDLE_NAME);
    if scratch.exists() {
        fs::remove_dir_all(&scratch)
            .map_err(|error| format!("stage 2: clear scratch {}: {error}", scratch.display()))?;
    }
    fs::create_dir_all(&scratch)
        .map_err(|error| format!("stage 2: create scratch {}: {error}", scratch.display()))?;
    let output = scratch.join(STARTER_ENTRY_EMITTED);
    let mut command = Command::new(scripts_build);
    command.arg("--in").arg(&source).arg("--out").arg(&output);
    run_checked(&mut command, "stage 2 bundle the starter start-script.ts")?;
    Ok(output)
}

/// The SDK bundle's name: its root under `dist/` and the package its own
/// marker declares — one constant, so the two cannot drift. As the app name it
/// keeps authoring runs from the bundle apart from any installed game's
/// settings and saves, and from a bare `postretro` launch's.
pub(crate) const SDK_BUNDLE_NAME: &str = "postretro-sdk";

/// The mod the bundle's own marker declares, published at `content/base`.
pub(crate) const STARTER_MOD: &str = "base";

/// The starter mod's source tree, relative to the install root. It ships in
/// the bundle's `sdk/` too, so a developer starting a project in their own
/// repository copies it from there.
pub(crate) const STARTER_TEMPLATE: &str = "sdk/templates/starter";

/// The engine-owned editor config the starter mod root receives a copy of.
pub(crate) const TSCONFIG_TEMPLATE: &str = "sdk/templates/tsconfig.json";

const STARTER_ENTRY_SOURCE: &str = "start-script.ts";
const STARTER_ENTRY_EMITTED: &str = "start-script.js";

/// Where the SDK bundle's own binaries land, relative to the bundle root.
pub(crate) const BIN_DIR: &str = "bin";

/// The name the bundle's optimized engine ships under, beside the tool that
/// copies it into a payload. The bundle root's `postretro` is the debug
/// authoring engine, so the two cannot share a name.
pub(crate) const RELEASE_ENGINE_STEM: &str = "postretro-release";

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle's marker names the bundle, so a run from it keeps its player
    /// data under the bundle's own name — never an installed game's.
    #[test]
    fn bundle_marker_names_the_bundle_and_its_starter_mod() {
        let marker = readme::render_bundle_manifest().expect("the bundle marker renders");
        let parsed = crate::manifest::Manifest::parse(&marker).expect("the bundle marker parses");
        assert_eq!(parsed.package.name, SDK_BUNDLE_NAME);
        assert_eq!(parsed.package.mod_name, STARTER_MOD);
        assert!(parsed.recipes.is_empty(), "{marker}");
    }

    /// The tool's default helper search walks from its own directory, so the
    /// bundle's layout is what makes `./bin/postretro-tool dist` work with no
    /// flags at all: every binary it needs is either beside it or one level up.
    #[test]
    fn bundle_layout_puts_every_helper_where_the_default_search_looks() {
        let bundle = PathBuf::from("/bundle");
        assert_eq!(bundle.join(BIN_DIR), PathBuf::from("/bundle/bin"));
        assert_ne!(
            RELEASE_ENGINE_STEM, "postretro",
            "the debug authoring engine at the bundle root owns that name"
        );
    }
}
