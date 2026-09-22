//! Stage 5 for the SDK bundle: the tree, the binaries, and the completion sweep.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::binaries::binary_name;
use crate::dist::launcher::emit_launcher;
use crate::dist::payload::replace_payload_root;
use crate::dist::resolve::{
    EntryExt, Resolved, bake_order, guard_payload_root, outstanding_outputs,
};
use crate::project::{MARKER_FILE, Project};

use super::readme::render_readme;
use super::{BIN_DIR, RELEASE_ENGINE_STEM};

/// The binaries a bundle ships. Each is built elsewhere and named by a flag.
pub(super) struct BundleBinaries {
    /// Debug, `--features dev-tools`: the one engine that both plays the baked
    /// maps and serves the edit-and-reload loop.
    pub(super) authoring_engine: PathBuf,
    /// Optimized, default features: what a player payload built from this
    /// bundle must carry, and what the bundle's own `dist` copies.
    pub(super) release_engine: PathBuf,
    pub(super) prl_build: PathBuf,
    pub(super) scripts_build: PathBuf,
    pub(super) mint_identity: PathBuf,
    pub(super) tool: PathBuf,
}

pub(super) struct BundleTarget<'a> {
    pub(super) output_root: &'a Path,
    pub(super) bundle_root: &'a Path,
    pub(super) bundle_name: &'a str,
    /// Where the engine-owned trees come from — never the project.
    pub(super) install_root: &'a Path,
}

pub(super) fn assemble_bundle(
    project: &Project,
    target: &BundleTarget<'_>,
    binaries: &BundleBinaries,
    entry_ext: EntryExt,
    entry_script: &Path,
    resolved: &[Resolved],
    bundle_manifest: &str,
) -> Result<(), String> {
    println!("Stage 5: assemble content-complete bundle tree");
    guard_payload_root(target.bundle_root, project.root())
        .map_err(|error| format!("sdk-dist stage 5: {error}"))?;
    // Resolved before the delete below. Each is one `is_dir` check, and a wrong
    // or missing install root would otherwise cost the previous good bundle
    // before anything diagnosed it.
    let trees: Vec<(&str, PathBuf)> = crate::engine_trees::BUNDLE_TREES
        .iter()
        .map(|name| {
            crate::engine_trees::resolve(target.install_root, name).map(|tree| (*name, tree))
        })
        .collect::<Result<_, _>>()?;

    // The completion gate tracks the outstanding level bakes exactly as the
    // player payload does: the marker carries the full resolved set at assembly,
    // and stage 6 rewrites it after each bake.
    let ordered = bake_order(resolved);
    replace_payload_root(
        target.output_root,
        target.bundle_root,
        target.bundle_name,
        &outstanding_outputs(&ordered, 0),
    )?;

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

    // The mod tree ships WITH its .map/.ts sources, but committed stale generated
    // .prl/.js are dropped: fresh .prl come from stage 6's level bake and the
    // fresh entry .js is installed just below. It publishes under the project's
    // own declared mod root — the same path the bundle's own `dist` re-publishes,
    // and the one the `postretro.toml` written below names.
    let mod_root_rel = project.mod_root_rel();
    let bundle_mod_root = target.bundle_root.join(mod_root_rel);
    let mod_copied = copy_bundle_tree(&project.mod_root(), &bundle_mod_root, true)?;
    println!("  copied {mod_copied} files from the mod tree (with sources) into {mod_root_rel}");

    // Install the emitted entry .js beside its .ts source (a TS mod). A Luau mod
    // already shipped its `start-script.luau` source via the tree copy and needs
    // no emitted sibling.
    if entry_ext == EntryExt::Js {
        let installed = bundle_mod_root.join(EntryExt::Js.file_name());
        fs::copy(entry_script, &installed).map_err(|error| {
            format!(
                "sdk-dist stage 5: install emitted entry script {}: {error}",
                installed.display()
            )
        })?;
    }

    // Already rendered and validated up front (`sdk_dist::run`), so a recipe
    // source outside the shipped mod tree failed before this tree was written.
    write_bundle_file(target.bundle_root, MARKER_FILE, bundle_manifest)?;
    emit_launcher(target.bundle_root, target.bundle_name, mod_root_rel)?;
    write_bundle_file(
        target.bundle_root,
        "README.md",
        &render_readme(
            &project.manifest().package.name,
            target.bundle_name,
            mod_root_rel,
        ),
    )?;
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
    // (`crates/scripting-core/src/watcher.rs`) — not under `bin/`. Flow B of the
    // README promises that loop, so the compiler ships at the root too. `bin/`
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
/// metadata, and OS junk. When `skip_stale_outputs` is set (the mod tree), stale
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
/// Always drops build caches, `maps/autosave/`, any `.git*` entry, and
/// `.DS_Store`. When `skip_stale_outputs` is set, also drops committed generated
/// `.prl`/`.js` (fresh ones are produced during the run).
fn should_skip_bundle_entry(
    name: &str,
    parent_name: Option<&str>,
    skip_stale_outputs: bool,
) -> bool {
    if name == ".build-caches" || name == ".DS_Store" || name.starts_with(".git") {
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

/// The mod-root-relative source entry-script filename for a resolved choice.
///
/// The SDK bundle ships the authoring *source*: a TypeScript mod's source is
/// `start-script.ts` (the emitted `.js` ships beside it), a Luau mod's source is
/// `start-script.luau`.
fn sdk_entry_source_name(choice: EntryExt) -> &'static str {
    match choice {
        EntryExt::Js => "start-script.ts",
        EntryExt::Luau => "start-script.luau",
    }
}

/// Light completion check for the content-complete bundle. Unlike the player
/// sweep, sources (`.map`/`.ts`/`.md`) are allowed and expected; this instead
/// confirms the required entries are present: the engines, the compilers, the
/// tool, the project marker, every engine-owned tree (`sdk/`, `docs/`, `tools/`,
/// and each `core/` subtree in its own right), the mod entry SOURCE, every
/// resolved baked `maps/<name>.prl`, and the baked `baked/materials/` tree.
pub(super) fn sweep_sdk_bundle(
    bundle_root: &Path,
    mod_root: &Path,
    entry_ext: EntryExt,
    resolved: &[Resolved],
) -> Result<(), String> {
    let mod_dir = bundle_root.join(mod_root);
    let bin_dir = bundle_root.join(BIN_DIR);

    let mut required_files = vec![
        bundle_root.join(binary_name("postretro")),
        // Beside the engine, where its own hot-reload discovery looks.
        bundle_root.join(binary_name("scripts-build")),
        bundle_root.join(MARKER_FILE),
        bin_dir.join(binary_name(RELEASE_ENGINE_STEM)),
        bin_dir.join(binary_name("prl-build")),
        bin_dir.join(binary_name("scripts-build")),
        bin_dir.join(binary_name("mint-identity")),
        bin_dir.join(binary_name("postretro-tool")),
        mod_dir.join(sdk_entry_source_name(entry_ext)),
    ];
    // Every resolved level must have baked into the bundle's maps/ tree.
    for resolved in resolved {
        required_files.push(mod_dir.join(&resolved.output));
    }
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
        bundle_root.join("baked").join("materials"),
    ] {
        if !dir.is_dir() {
            return Err(format!(
                "sdk-dist sweep: required bundle directory missing: {}",
                dir.display()
            ));
        }
    }

    // The bundle is a superset, so its sweep forbids almost nothing — but a
    // publication lock is build scaffolding rather than content, and refusing it
    // here is what keeps a later change from quietly shipping them again.
    refuse_pack_locks(bundle_root)
}

fn refuse_pack_locks(directory: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("sdk-dist sweep: read {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("sdk-dist sweep: read entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            refuse_pack_locks(&path)?;
            continue;
        }
        if crate::dist::payload::is_pack_lock(entry.file_name().to_str().unwrap_or_default()) {
            return Err(format!(
                "sdk-dist sweep: publication lock left in the bundle: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published mod root these bundle-sweep tests build under. The bundle
    /// now honors the project's declared mod root; the sweep only requires a
    /// two-component path, so this stands in for whatever a project names.
    const PAYLOAD_MOD_ROOT: &str = "content/base";

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
                "campaign-test.map",
                "start-script.ts",
                "start-script.luau",
                "anim-demo.README.md",
                "texture.png",
                "scene.gltf",
                "audio.wav",
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
    fn skip_predicate_drops_stale_build_outputs_only_for_the_mod_tree() {
        for name in ["campaign-test.prl", "start-script.js"] {
            assert!(
                should_skip_bundle_entry(name, Some("maps"), true),
                "{name} dropped for mod tree"
            );
            assert!(
                !should_skip_bundle_entry(name, Some("maps"), false),
                "{name} kept for verbatim tree"
            );
        }
    }

    #[test]
    fn entry_source_name_ships_the_authoring_source_not_the_emitted_script() {
        assert_eq!(sdk_entry_source_name(EntryExt::Js), "start-script.ts");
        assert_eq!(sdk_entry_source_name(EntryExt::Luau), "start-script.luau");
    }

    fn resolved(output: &str) -> Resolved {
        Resolved {
            output: output.to_string(),
            source: PathBuf::from("unused.map"),
            args: Vec::new(),
            lightmap_density: 0.04,
        }
    }

    /// Build a minimal complete bundle tree the sweep should accept, then let a
    /// caller knock one required entry out to prove the sweep rejects it.
    fn assemble_swept_bundle(root: &Path, mod_root: &Path, resolved: &[Resolved]) {
        let mod_dir = root.join(mod_root);
        fs::create_dir_all(mod_dir.join("maps")).unwrap();
        fs::create_dir_all(root.join(BIN_DIR)).unwrap();
        for tree in crate::engine_trees::BUNDLE_TREES {
            fs::create_dir_all(root.join(tree)).unwrap();
        }
        let core = root.join(crate::engine_trees::CORE_TREE);
        for subtree in ["ui", "textures", "licenses"] {
            fs::create_dir_all(core.join(subtree)).unwrap();
        }
        fs::create_dir_all(root.join("baked").join("materials")).unwrap();
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
        fs::write(mod_dir.join("start-script.ts"), "source").unwrap();
        for level in resolved {
            fs::write(mod_dir.join(&level.output), "baked").unwrap();
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_sdk_dist_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("temporary tree created");
        path
    }

    #[test]
    fn sweep_accepts_a_content_complete_bundle_with_baked_levels_and_sources() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [
            resolved("maps/campaign-test.prl"),
            resolved("maps/arena.prl"),
        ];
        assemble_swept_bundle(&root, mod_root, &levels);

        assert!(sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels).is_ok());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_requires_each_resolved_baked_prl() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        fs::remove_file(root.join(mod_root).join("maps/campaign-test.prl")).unwrap();

        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("missing baked .prl is rejected");
        assert!(error.contains("campaign-test.prl"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    /// The bundle's sweep forbids almost nothing, which is exactly why the one
    /// thing it does forbid has to be pinned.
    #[test]
    fn sweep_refuses_a_bundle_that_still_carries_a_publication_lock() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        assert!(
            sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels).is_ok(),
            "the clean bundle is the control"
        );

        fs::write(
            root.join(mod_root)
                .join("maps")
                .join(".campaign-test.prl.pack.lock"),
            "lock",
        )
        .unwrap();
        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("a lock in the bundle is refused");
        assert!(error.contains("campaign-test.prl.pack.lock"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    /// `core/ui` holds the pause menu, frontend menu and keyboard. A bundle
    /// without it boots with those screens gone and only warnings to say so, so
    /// the sweep treats its absence as a failure rather than a gap.
    #[test]
    fn sweep_requires_every_engine_owned_tree_and_core_subtree() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);

        // Each engine-owned subtree separately. A sweep over bare `core/` would
        // pass a half-copied tree, and every one of those absences is a runtime
        // warning rather than a failure: the built-in screens, the boot splash,
        // and the OFL text the embedded typefaces oblige the bundle to carry.
        let core = root.join(crate::engine_trees::CORE_TREE);
        for subtree in ["ui", "textures", "licenses"] {
            let path = core.join(subtree);
            fs::remove_dir_all(&path).unwrap();
            let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
                .expect_err("a bundle missing an engine-owned subtree is rejected");
            assert!(error.contains(subtree), "{error}");
            fs::create_dir_all(&path).unwrap();
        }

        for tree in ["sdk", "docs", "tools"] {
            fs::remove_dir_all(root.join(tree)).unwrap();
            let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
                .expect_err("a bundle missing an engine-owned tree is rejected");
            assert!(error.contains(tree), "{error}");
            fs::create_dir_all(root.join(tree)).unwrap();
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_requires_the_baked_materials_directory() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        fs::remove_dir_all(root.join("baked").join("materials")).unwrap();

        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("missing baked materials is rejected");
        assert!(error.contains("materials"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_requires_the_mod_entry_source() {
        let root = unique_temp_dir();
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        fs::remove_file(root.join(mod_root).join("start-script.ts")).unwrap();

        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("missing entry source is rejected");
        assert!(error.contains("start-script.ts"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    /// The bundle's reason for existing is that its recipient can run `dist`
    /// from it. That needs the tool, the binaries the tool drives, and the
    /// project marker that tells the tool where it is.
    #[test]
    fn sweep_requires_everything_the_bundles_own_dist_run_needs() {
        let mod_root = Path::new(PAYLOAD_MOD_ROOT);
        let levels = [resolved("maps/campaign-test.prl")];
        for required in [
            PathBuf::from(MARKER_FILE),
            PathBuf::from(BIN_DIR).join(binary_name("postretro-tool")),
            PathBuf::from(BIN_DIR).join(binary_name(RELEASE_ENGINE_STEM)),
            PathBuf::from(BIN_DIR).join(binary_name("prl-build")),
            PathBuf::from(BIN_DIR).join(binary_name("scripts-build")),
            PathBuf::from(BIN_DIR).join(binary_name("mint-identity")),
        ] {
            let root = unique_temp_dir();
            assemble_swept_bundle(&root, mod_root, &levels);
            fs::remove_file(root.join(&required)).unwrap();

            let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels).expect_err(
                &format!("a bundle missing {} cannot run dist", required.display()),
            );
            assert!(
                error.contains(&required.file_name().unwrap().to_string_lossy().to_string()),
                "{error}"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }
}
