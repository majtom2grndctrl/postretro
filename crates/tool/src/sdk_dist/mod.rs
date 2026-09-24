//! Modder SDK bundle assembly — a *content-complete* kit.
//!
//! Unlike `dist` (the lean *player* payload: baked levels, source-excluded
//! content, a release engine), `sdk-dist` produces a bundle that is playable on
//! arrival AND fully editable. It runs the same content bakes as the player
//! payload — model textures, per-level `prl-build --release`, materials copy —
//! so the baked `maps/<name>.prl` and `baked/materials/` ship ready to play, and
//! it *additionally* ships what authoring needs: a DEBUG engine built with
//! `--features dev-tools` (TS auto-compile + hot reload require debug
//! assertions; the inspector requires the feature), the compilers, the tool
//! itself, a release engine, the `sdk/`, `docs/`, and `tools/` trees, the
//! engine-owned `core/` tree, and the mod tree WHOLE — its `.map`/`.ts` sources
//! beside the freshly baked `.prl` and the emitted entry `.js`, never run
//! through the player payload's source-excluding filter.
//!
//! The bundle is a project in its own right: it carries a `postretro.toml`, so
//! its recipient can produce a player payload from it with no repository, no
//! Rust toolchain, and no cargo — which is the whole point of shipping the tool.
//!
//! It reuses the player payload's stages and its containment and
//! completion-gate machinery. The one invariant separating the two outputs is
//! subtraction, not baking: the player payload carries only released runtime
//! artifacts, while the SDK bundle is a superset that also carries the sources
//! and tools. The SDK sweep is therefore lighter — it confirms the required
//! entries exist rather than forbidding sources.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use crate::binaries::Helper;
use crate::dist::parse_args;
use crate::dist::payload::{MARKER_NAME, count_payload};
use crate::dist::resolve::guard_payload_root;
use crate::dist::stages::{self, BakeTarget};

mod assemble;
mod readme;

use assemble::{BundleBinaries, sweep_sdk_bundle};

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let cli = parse_args(args, "sdk-dist")?;
    let project = cli.project()?;
    let install_root = cli.install_root()?;
    let output_root = cli.output_root(&project);
    let bundle_name = sdk_bundle_root_name(&project.manifest().package.name);
    let bundle_root = output_root.join(&bundle_name);

    // Render the bundle's marker before anything is assembled. It refuses a
    // recipe source that lives outside the shipped mod tree — a bundle the
    // recipient's own `dist` could not build from — so that failure lands here,
    // on the author's machine, rather than after a broken tree has been written.
    let bundle_manifest = readme::render_bundle_manifest(project.manifest())?;

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

    let (entry_ext, entry_script) = stages::emit_entry_script(&project, &binaries.scripts_build)?;
    let resolved = stages::resolve_levels(&entry_script, &project)?;
    stages::bake_model_textures(&project)?;

    assemble::assemble_bundle(
        &project,
        &assemble::BundleTarget {
            output_root: &output_root,
            bundle_root: &bundle_root,
            bundle_name: &bundle_name,
            install_root: &install_root,
        },
        &binaries,
        entry_ext,
        &entry_script,
        &resolved,
        &bundle_manifest,
    )?;

    // The bundle's marker namespace is the `-sdk` bundle name, so its temp files
    // never collide with a concurrent player payload run.
    stages::bake_levels(
        &project,
        &BakeTarget {
            output_root: &output_root,
            payload_root: &bundle_root,
            marker_name: &bundle_name,
            payload_mod_root: project.mod_root_rel(),
        },
        &binaries.prl_build,
        &resolved,
    )?;
    stages::copy_materials(&project, &bundle_root)?;
    let locks = crate::dist::payload::remove_pack_locks(&bundle_root)?;
    if locks > 0 {
        println!("  removed {locks} publication locks left by the level bakes");
    }

    sweep_sdk_bundle(
        &bundle_root,
        Path::new(project.mod_root_rel()),
        entry_ext,
        &resolved,
    )?;
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

/// The SDK bundle root name: the player payload name plus a `-sdk` suffix so the
/// two never collide under the same output directory.
fn sdk_bundle_root_name(package_name: &str) -> String {
    format!("{package_name}-sdk")
}

/// Where the SDK bundle's own binaries land, relative to the bundle root.
pub(crate) const BIN_DIR: &str = "bin";

/// The name the bundle's optimized engine ships under, beside the tool that
/// copies it into a payload. The bundle root's `postretro` is the debug
/// authoring engine, so the two cannot share a name.
pub(crate) const RELEASE_ENGINE_STEM: &str = "postretro-release";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sdk_bundle_root_name_appends_sdk_suffix() {
        assert_eq!(sdk_bundle_root_name("postretro-dev"), "postretro-dev-sdk");
        assert_eq!(sdk_bundle_root_name("dev"), "dev-sdk");
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
