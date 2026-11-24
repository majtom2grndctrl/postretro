//! Stage 5 for the SDK bundle: the tree, the binaries, and the completion sweep.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::binaries::binary_name;
use crate::dist::payload::replace_payload_root;
use crate::dist::resolve::guard_payload_root;
use crate::project::MARKER_FILE;

use super::readme::{render_bundle_manifest, render_readme};
use super::{
    BIN_DIR, RELEASE_ENGINE_STEM, SDK_BUNDLE_NAME, STARTER_MOD, STARTER_TEMPLATE,
    TSCONFIG_TEMPLATE,
};

/// The binaries a bundle ships. Each is built elsewhere and named by a flag.
pub(super) struct BundleBinaries {
    /// Debug, `--features dev-tools`: the engine every authoring run launches.
    pub(super) authoring_engine: PathBuf,
    /// Optimized, default features: the engine `bin/postretro-tool dist` copies
    /// into every player payload built with this bundle.
    pub(super) release_engine: PathBuf,
    pub(super) prl_build: PathBuf,
    pub(super) scripts_build: PathBuf,
    pub(super) mint_identity: PathBuf,
    pub(super) tool: PathBuf,
}

pub(super) struct BundleTarget<'a> {
    /// The project whose `dist/` the bundle is written under. Assembly uses it
    /// only as the containment anchor; no project content enters a bundle.
    pub(super) project_root: &'a Path,
    pub(super) output_root: &'a Path,
    pub(super) bundle_root: &'a Path,
    /// Where the engine-owned trees and the starter come from — never the
    /// project.
    pub(super) install_root: &'a Path,
}

/// The starter mod's root inside the bundle: `content/<STARTER_MOD>`.
fn starter_mod_root(bundle_root: &Path) -> PathBuf {
    bundle_root.join("content").join(STARTER_MOD)
}

pub(super) fn assemble_bundle(
    target: &BundleTarget<'_>,
    binaries: &BundleBinaries,
    starter_script: &Path,
) -> Result<(), String> {
    println!("Stage 5: assemble the SDK bundle tree");
    guard_payload_root(target.bundle_root, target.project_root)
        .map_err(|error| format!("sdk-dist stage 5: {error}"))?;
    // Resolved before the delete below. Each is one filesystem check, and a
    // wrong or missing install root would otherwise cost the previous good
    // bundle before anything diagnosed it.
    let trees: Vec<(&str, PathBuf)> = crate::engine_trees::BUNDLE_TREES
        .iter()
        .map(|name| {
            crate::engine_trees::resolve(target.install_root, name).map(|tree| (*name, tree))
        })
        .collect::<Result<_, _>>()?;
    let starter = crate::engine_trees::resolve(target.install_root, STARTER_TEMPLATE)?;
    let tsconfig = target.install_root.join(TSCONFIG_TEMPLATE);
    if !tsconfig.is_file() {
        return Err(format!(
            "sdk-dist stage 5: engine-owned template {} not found",
            tsconfig.display()
        ));
    }

    // The bundle bakes no levels, so the completion marker lists none: its
    // status line alone records that assembly has not finished.
    replace_payload_root(target.output_root, target.bundle_root, SDK_BUNDLE_NAME, &[])?;

    install_binaries(target.bundle_root, binaries)?;

    // These trees ship verbatim: they are not mod source, so no stale-output drop.
    // All four are engine-owned, so all four come from the install and never from
    // the project — a developer's content repository carries none of them, and a
    // project that happens to have a directory of the same name does not get to
    // stand in for the engine's.
    for (name, tree) in &trees {
        let copied = copy_bundle_tree(tree, &target.bundle_root.join(name), false)?;
        println!("  copied {copied} files from {name}/ ({})", tree.display());
    }

    // The starter mod: the template's sources, the engine's editor config, the
    // freshly emitted entry script, and an empty `maps/` for the first level.
    // A stale `.js` committed beside the template is dropped in favour of the
    // one stage 2 just compiled.
    let mod_root = starter_mod_root(target.bundle_root);
    let copied = copy_bundle_tree(&starter, &mod_root, true)?;
    println!(
        "  copied {copied} starter files into content/{STARTER_MOD} ({})",
        starter.display()
    );
    for (source, name) in [
        (tsconfig.as_path(), "tsconfig.json"),
        (starter_script, "start-script.js"),
    ] {
        let destination = mod_root.join(name);
        fs::copy(source, &destination).map_err(|error| {
            format!(
                "sdk-dist stage 5: install {} as {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
    }
    let maps = mod_root.join("maps");
    fs::create_dir_all(&maps).map_err(|error| {
        format!(
            "sdk-dist stage 5: create starter maps directory {}: {error}",
            maps.display()
        )
    })?;

    write_bundle_file(target.bundle_root, MARKER_FILE, &render_bundle_manifest()?)?;
    write_bundle_file(target.bundle_root, "README.md", &render_readme())?;
    Ok(())
}

/// Copy the bundle's binaries: the authoring engine at the root where a launch
/// finds it, everything else under `bin/` where the tool's own sibling search
/// looks.
fn install_binaries(bundle_root: &Path, binaries: &BundleBinaries) -> Result<(), String> {
    copy_binary(
        &binaries.authoring_engine,
        &bundle_root.join(binary_name("postretro")),
    )?;
    // The debug engine's TS auto-compile and hot reload shell out to
    // `scripts-build`, which it looks for beside its own executable or on PATH
    // (`crates/scripting-core/src/watcher.rs`) — not under `bin/`. The README's
    // authoring steps promise that loop, so the compiler ships at the root too. `bin/`
    // keeps its copy: that is the path the docs give for a manual compile, and
    // the one the tool's own sibling search finds.
    copy_binary(
        &binaries.scripts_build,
        &bundle_root.join(binary_name("scripts-build")),
    )?;

    let bin_dir = bundle_root.join(BIN_DIR);
    fs::create_dir_all(&bin_dir).map_err(|error| {
        format!(
            "sdk-dist stage 5: create bin directory {}: {error}",
            bin_dir.display()
        )
    })?;
    for (source, stem) in [
        (&binaries.release_engine, RELEASE_ENGINE_STEM),
        (&binaries.prl_build, "prl-build"),
        (&binaries.scripts_build, "scripts-build"),
        (&binaries.mint_identity, "mint-identity"),
        (&binaries.tool, "postretro-tool"),
    ] {
        copy_binary(source, &bin_dir.join(binary_name(stem)))?;
    }
    Ok(())
}

fn copy_binary(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination).map_err(|error| {
        format!(
            "sdk-dist stage 5: copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn write_bundle_file(bundle_root: &Path, name: &str, contents: &str) -> Result<(), String> {
    let path = bundle_root.join(name);
    fs::write(&path, contents)
        .map_err(|error| format!("sdk-dist stage 5: write {}: {error}", path.display()))
}

/// Copy a directory tree into the bundle, skipping build caches, autosaves, VCS
/// metadata, and OS junk. When `skip_stale_outputs` is set (the starter tree), stale
/// committed generated `.prl`/`.js` build outputs are also dropped.
fn copy_bundle_tree(
    source: &Path,
    destination: &Path,
    skip_stale_outputs: bool,
) -> Result<usize, String> {
    let source_parent = source.file_name().and_then(OsStr::to_str);
    copy_bundle_tree_inner(source, destination, source_parent, skip_stale_outputs)
}

fn copy_bundle_tree_inner(
    source: &Path,
    destination: &Path,
    parent_name: Option<&str>,
    skip_stale_outputs: bool,
) -> Result<usize, String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "sdk-dist stage 5: create destination tree {}: {error}",
            destination.display()
        )
    })?;
    let entries = fs::read_dir(source).map_err(|error| {
        format!(
            "sdk-dist stage 5: read source tree {}: {error}",
            source.display()
        )
    })?;

    let mut copied = 0;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("sdk-dist stage 5: read source tree entry: {error}"))?;
        let source_path = entry.path();
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            return Err(format!(
                "sdk-dist stage 5: source tree entry {} is not valid UTF-8",
                source_path.display()
            ));
        };
        if should_skip_bundle_entry(&name, parent_name, skip_stale_outputs) {
            continue;
        }

        let destination_path = destination.join(&name);
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "sdk-dist stage 5: inspect source tree entry {}: {error}",
                source_path.display()
            )
        })?;
        if file_type.is_dir() {
            copied += copy_bundle_tree_inner(
                &source_path,
                &destination_path,
                Some(name.as_str()),
                skip_stale_outputs,
            )?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "sdk-dist stage 5: copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
            copied += 1;
        } else {
            return Err(format!(
                "sdk-dist stage 5: refuse non-regular source tree entry {}",
                source_path.display()
            ));
        }
    }
    Ok(copied)
}

/// Return whether a tree entry must not enter the SDK bundle.
///
/// Always drops build caches, `maps/autosave/`, any `.git*` entry, `.DS_Store`,
/// and compiler scratch a killed bake left behind. When `skip_stale_outputs` is
/// set (the starter tree), also drops committed generated `.prl`/`.js`: stage 2
/// supplies the fresh `.js`, and the bundle ships no `.prl`.
fn should_skip_bundle_entry(
    name: &str,
    parent_name: Option<&str>,
    skip_stale_outputs: bool,
) -> bool {
    if name == ".build-caches"
        || name == ".DS_Store"
        || name.starts_with(".git")
        || crate::dist::payload::is_build_scratch(name)
    {
        return true;
    }
    if name == "autosave" && parent_name == Some("maps") {
        return true;
    }
    if skip_stale_outputs
        && matches!(
            Path::new(name).extension().and_then(OsStr::to_str),
            Some("prl" | "js")
        )
    {
        return true;
    }
    false
}

/// Completion check for the bundle. It confirms the required entries exist —
/// the engines, the compilers, the tool, the project marker, every engine-owned
/// tree (`sdk/`, `docs/`, `tools/`, and each `core/` subtree in its own right),
/// and the starter mod — and refuses the two ways game content could reach a
/// bundle: a mod under `content/` other than the starter, and a `baked/` tree.
pub(super) fn sweep_sdk_bundle(bundle_root: &Path) -> Result<(), String> {
    let mod_dir = starter_mod_root(bundle_root);
    let bin_dir = bundle_root.join(BIN_DIR);

    let required_files = [
        bundle_root.join(binary_name("postretro")),
        // Beside the engine, where its own hot-reload discovery looks.
        bundle_root.join(binary_name("scripts-build")),
        bundle_root.join(MARKER_FILE),
        bin_dir.join(binary_name(RELEASE_ENGINE_STEM)),
        bin_dir.join(binary_name("prl-build")),
        bin_dir.join(binary_name("scripts-build")),
        bin_dir.join(binary_name("mint-identity")),
        bin_dir.join(binary_name("postretro-tool")),
        // The author edits the `.ts`; the `.js` is the output of the compile
        // check that ran before the bundle shipped.
        mod_dir.join("start-script.ts"),
        mod_dir.join("start-script.js"),
        mod_dir.join("tsconfig.json"),
    ];
    for file in &required_files {
        if !file.is_file() {
            return Err(format!(
                "sdk-dist sweep: required bundle file missing: {}",
                file.display()
            ));
        }
    }

    let core = bundle_root.join(crate::engine_trees::CORE_TREE);
    for dir in [
        bundle_root.join("sdk"),
        bundle_root.join("docs"),
        bundle_root.join("tools"),
        // Each `core/` subtree separately, not the tree as a whole: every one of
        // them degrades to a warning at runtime rather than a failure, so a
        // half-copied `core/` is exactly what a sweep over the parent would let
        // through. `licenses/` is a shipping obligation rather than a runtime
        // one — the two default typefaces are compiled into the engine, and SIL
        // OFL 1.1 requires their licence to travel with the binary that embeds
        // them, which this tree is the only copy of.
        core.join("ui"),
        core.join("textures"),
        core.join("licenses"),
        mod_dir.join("maps"),
    ] {
        if !dir.is_dir() {
            return Err(format!(
                "sdk-dist sweep: required bundle directory missing: {}",
                dir.display()
            ));
        }
    }

    refuse_game_content(bundle_root)?;
    // Publication locks and compiler scratch are build scaffolding rather than
    // content; refusing them keeps a later change from quietly shipping them.
    refuse_build_scaffolding(bundle_root)
}

/// The bundle ships no game content: `content/` holds the starter alone, and
/// nothing baked travels with it.
fn refuse_game_content(bundle_root: &Path) -> Result<(), String> {
    let content = bundle_root.join("content");
    for entry in fs::read_dir(&content)
        .map_err(|error| format!("sdk-dist sweep: read {}: {error}", content.display()))?
    {
        let entry = entry.map_err(|error| format!("sdk-dist sweep: read entry: {error}"))?;
        if entry.file_name() != OsStr::new(STARTER_MOD) {
            return Err(format!(
                "sdk-dist sweep: the bundle ships no game content, but {} is beside the \
                 starter mod",
                entry.path().display()
            ));
        }
    }
    let baked = bundle_root.join("baked");
    if baked.exists() {
        return Err(format!(
            "sdk-dist sweep: the bundle ships no game content, but it carries {}",
            baked.display()
        ));
    }
    Ok(())
}

fn refuse_build_scaffolding(directory: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("sdk-dist sweep: read {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("sdk-dist sweep: read entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            refuse_build_scaffolding(&path)?;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if crate::dist::payload::is_pack_lock(name) {
            return Err(format!(
                "sdk-dist sweep: publication lock left in the bundle: {}",
                path.display()
            ));
        }
        if crate::dist::payload::is_build_scratch(name) {
            return Err(format!(
                "sdk-dist sweep: compiler scratch file left in the bundle: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_predicate_drops_caches_vcs_and_os_junk_in_both_modes() {
        for skip_stale in [false, true] {
            for name in [
                ".build-caches",
                ".DS_Store",
                ".git",
                ".gitignore",
                ".gitkeep",
                ".gitattributes",
            ] {
                assert!(
                    should_skip_bundle_entry(name, None, skip_stale),
                    "{name} (skip_stale={skip_stale})"
                );
            }
        }
    }

    #[test]
    fn skip_predicate_scopes_autosave_to_the_maps_parent() {
        assert!(should_skip_bundle_entry("autosave", Some("maps"), false));
        assert!(!should_skip_bundle_entry(
            "autosave",
            Some("scripts"),
            false
        ));
        assert!(!should_skip_bundle_entry("autosave", None, false));
    }

    #[test]
    fn skip_predicate_keeps_sources_and_assets_in_both_modes() {
        for skip_stale in [false, true] {
            for name in [
                "start-script.ts",
                "start-script.luau",
                "README.md",
                "texture.png",
                "scene.gltf",
                "data.json",
            ] {
                assert!(
                    !should_skip_bundle_entry(name, Some("maps"), skip_stale),
                    "{name} (skip_stale={skip_stale})"
                );
            }
        }
    }

    #[test]
    fn skip_predicate_drops_stale_build_outputs_only_for_the_starter_tree() {
        for name in ["arena.prl", "start-script.js"] {
            assert!(
                should_skip_bundle_entry(name, Some("maps"), true),
                "{name} dropped for the starter tree"
            );
            assert!(
                !should_skip_bundle_entry(name, Some("maps"), false),
                "{name} kept for a verbatim tree"
            );
        }
    }

    /// Compiler scratch a killed bake left behind is not a source, in either
    /// copy mode.
    #[test]
    fn skip_predicate_drops_build_scratch_in_both_modes() {
        for skip_stale in [false, true] {
            for name in [
                ".arena.prl.pack-a1B2c3.tmp",
                ".arena.prl.cluster-sh-XyZ09.spool",
                "wall_n.prm.tmp.4812",
            ] {
                assert!(
                    should_skip_bundle_entry(name, Some("maps"), skip_stale),
                    "{name} (skip_stale={skip_stale})"
                );
            }
        }
    }

    /// Build a minimal complete bundle tree the sweep should accept, so a test
    /// can knock one entry out, or add a forbidden one, and prove the sweep
    /// notices.
    fn assemble_swept_bundle(root: &Path) {
        let mod_dir = starter_mod_root(root);
        fs::create_dir_all(mod_dir.join("maps")).unwrap();
        fs::create_dir_all(root.join(BIN_DIR)).unwrap();
        for tree in crate::engine_trees::BUNDLE_TREES {
            fs::create_dir_all(root.join(tree)).unwrap();
        }
        let core = root.join(crate::engine_trees::CORE_TREE);
        for subtree in ["ui", "textures", "licenses"] {
            fs::create_dir_all(core.join(subtree)).unwrap();
        }
        fs::write(root.join(binary_name("postretro")), "engine").unwrap();
        fs::write(root.join(binary_name("scripts-build")), "scripts").unwrap();
        fs::write(root.join(MARKER_FILE), "[package]\n").unwrap();
        for stem in [
            RELEASE_ENGINE_STEM,
            "prl-build",
            "scripts-build",
            "mint-identity",
            "postretro-tool",
        ] {
            fs::write(root.join(BIN_DIR).join(binary_name(stem)), stem).unwrap();
        }
        for name in ["start-script.ts", "start-script.js", "tsconfig.json"] {
            fs::write(mod_dir.join(name), name).unwrap();
        }
    }

    fn unique_temp_dir() -> PathBuf {
        // Tests run in parallel and macOS clocks tick in microseconds, so the
        // timestamp alone collides; the counter keeps names unique in-process.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_sdk_dist_{}_{}_{}",
            std::process::id(),
            unique,
            sequence
        ));
        fs::create_dir_all(&path).expect("temporary tree created");
        path
    }

    #[test]
    fn sweep_accepts_an_install_with_only_the_starter_mod() {
        let root = unique_temp_dir();
        assemble_swept_bundle(&root);
        assert!(sweep_sdk_bundle(&root).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    /// The bundle ships no game content, so a second mod or a baked tree means
    /// content reached it.
    #[test]
    fn sweep_refuses_game_content_beside_the_starter() {
        let root = unique_temp_dir();
        assemble_swept_bundle(&root);
        fs::create_dir_all(root.join("content").join("dev")).unwrap();
        let error = sweep_sdk_bundle(&root).expect_err("a second mod is refused");
        assert!(error.contains("no game content"), "{error}");
        fs::remove_dir_all(root.join("content").join("dev")).unwrap();

        fs::create_dir_all(root.join("baked").join("materials")).unwrap();
        let error = sweep_sdk_bundle(&root).expect_err("a baked tree is refused");
        assert!(error.contains("baked"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_refuses_a_bundle_that_still_carries_a_publication_lock() {
        let root = unique_temp_dir();
        assemble_swept_bundle(&root);
        fs::write(
            starter_mod_root(&root)
                .join("maps")
                .join(".arena.prl.pack.lock"),
            "lock",
        )
        .unwrap();
        let error = sweep_sdk_bundle(&root).expect_err("a lock in the bundle is refused");
        assert!(error.contains("arena.prl.pack.lock"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_refuses_a_bundle_that_still_carries_build_scratch() {
        let root = unique_temp_dir();
        assemble_swept_bundle(&root);
        fs::write(
            starter_mod_root(&root)
                .join("maps")
                .join(".arena.prl.pack-a1B2c3.tmp"),
            "scratch",
        )
        .unwrap();
        let error = sweep_sdk_bundle(&root).expect_err("scratch in the bundle is refused");
        assert!(error.contains("compiler scratch"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    /// `core/ui` holds the pause menu, frontend menu and keyboard. A bundle
    /// without it boots with those screens gone and only warnings to say so, so
    /// the sweep treats its absence as a failure rather than a gap.
    #[test]
    fn sweep_requires_every_engine_owned_tree_and_core_subtree() {
        let root = unique_temp_dir();
        assemble_swept_bundle(&root);

        let core = root.join(crate::engine_trees::CORE_TREE);
        for subtree in ["ui", "textures", "licenses"] {
            let path = core.join(subtree);
            fs::remove_dir_all(&path).unwrap();
            let error = sweep_sdk_bundle(&root)
                .expect_err("a bundle missing an engine-owned subtree is rejected");
            assert!(error.contains(subtree), "{error}");
            fs::create_dir_all(&path).unwrap();
        }

        for tree in ["sdk", "docs", "tools"] {
            fs::remove_dir_all(root.join(tree)).unwrap();
            let error = sweep_sdk_bundle(&root)
                .expect_err("a bundle missing an engine-owned tree is rejected");
            assert!(error.contains(tree), "{error}");
            fs::create_dir_all(root.join(tree)).unwrap();
        }

        let _ = fs::remove_dir_all(&root);
    }

    /// The starter is what lets `bin/postretro-tool run` boot on arrival. The
    /// author edits the `.ts` against the `tsconfig.json`; the `.js` is the
    /// output of the compile check that ran before the bundle shipped.
    #[test]
    fn sweep_requires_the_whole_starter_mod() {
        for name in ["start-script.ts", "start-script.js", "tsconfig.json"] {
            let root = unique_temp_dir();
            assemble_swept_bundle(&root);
            fs::remove_file(starter_mod_root(&root).join(name)).unwrap();
            let error = sweep_sdk_bundle(&root).expect_err("a partial starter is rejected");
            assert!(error.contains(name), "{error}");
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// The bundle's recipient runs the tool against their own project. That
    /// needs the tool, the binaries the tool drives, and — for the starter —
    /// the project marker that tells the tool where it is.
    #[test]
    fn sweep_requires_everything_the_bundles_own_tool_runs_need() {
        for required in [
            PathBuf::from(MARKER_FILE),
            PathBuf::from(BIN_DIR).join(binary_name("postretro-tool")),
            PathBuf::from(BIN_DIR).join(binary_name(RELEASE_ENGINE_STEM)),
            PathBuf::from(BIN_DIR).join(binary_name("prl-build")),
            PathBuf::from(BIN_DIR).join(binary_name("scripts-build")),
            PathBuf::from(BIN_DIR).join(binary_name("mint-identity")),
        ] {
            let root = unique_temp_dir();
            assemble_swept_bundle(&root);
            fs::remove_file(root.join(&required)).unwrap();

            let error = sweep_sdk_bundle(&root).expect_err(&format!(
                "a bundle missing {} cannot run the tool",
                required.display()
            ));
            assert!(
                error.contains(&required.file_name().unwrap().to_string_lossy().to_string()),
                "{error}"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }
}
