//! Host modder SDK bundle assembly — a *content-complete* kit.
//!
//! Unlike `dist` (the lean *player* payload: baked levels, source-excluded
//! content, a release engine), `sdk-dist` produces a bundle that is playable on
//! arrival AND fully editable. It runs the same content bakes as the player
//! payload — model textures, per-level `prl-build --release`, materials copy —
//! so the baked `maps/<name>.prl` and `baked/materials/` ship ready to play, and
//! it *additionally* ships what authoring needs: a DEBUG engine built with
//! `--features dev-tools` (TS auto-compile + hot reload require debug assertions;
//! the inspector requires the feature), the `prl-build`/`scripts-build`
//! compilers, the `sdk/`, `docs/`, and `tools/` trees, base content, and the mod
//! tree WHOLE — its `.map`/`.ts` sources beside the freshly baked `.prl` and the
//! emitted entry `.js`, never run through the player payload's source-excluding
//! filter.
//!
//! It reuses the player payload's bake stages and its containment and
//! completion-gate machinery. The one invariant separating the two outputs is
//! subtraction, not baking: the player payload carries only released runtime
//! artifacts, while the SDK bundle is a superset that also carries the sources
//! and tools. The SDK sweep is therefore lighter — it confirms the required
//! entries exist rather than forbidding sources.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dist::manifest::Manifest;
use crate::dist::payload::{MARKER_NAME, count_payload, replace_payload_root};
use crate::dist::resolve::{
    EntryExt, Resolved, bake_order, guard_payload_root, is_at_or_under, outstanding_outputs,
};
use crate::dist::{
    binary_name, cargo_target_dir, stage_four_bake_model_textures, stage_seven_copy_materials,
    stage_six_bake_levels, stage_three_resolve_levels, stage_two_emit_entry_script,
};
use crate::{run_checked, workspace_root};

struct SdkDistArgs {
    manifest_path: PathBuf,
    output_root: PathBuf,
}

struct BuiltBinaries {
    postretro: PathBuf,
    prl_build: PathBuf,
    scripts_build: PathBuf,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let workspace = workspace_root()?;
    let cli = parse_args(args, &workspace)?;
    let manifest = Manifest::read(&cli.manifest_path)?;
    let bundle_name = sdk_bundle_root_name(&manifest.package.name);
    let bundle_root = cli.output_root.join(&bundle_name);

    // Stage 1: prove the bundle root is a removable tree under dist/, and that
    // the cargo target directory does not itself sit under dist/ (its release/
    // dir holds the engine binary that provenance would misread as a payload).
    guard_payload_root(&bundle_root, &workspace)
        .map_err(|error| format!("sdk-dist stage 1: {error}"))?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let target_dir = cargo_target_dir(&cargo, &workspace)?;
    let workspace_dist = workspace.join("dist");
    let target_inside_dist = is_at_or_under(&target_dir, &workspace_dist).map_err(|error| {
        format!(
            "sdk-dist stage 1: compare cargo target directory {} with {}: {error}",
            target_dir.display(),
            workspace_dist.display()
        )
    })?;
    if target_inside_dist {
        return Err(format!(
            "sdk-dist stage 1: refuse cargo target directory {} at or under workspace dist/",
            target_dir.display()
        ));
    }

    // Stage 2: build the debug dev-tools engine and the release compilers.
    let binaries = stage_two_build_binaries(&cargo, &workspace, &target_dir)?;

    // Stage 3: emit the mod entry script and resolve the shipped level set,
    // reusing the player payload's stages verbatim.
    let (entry_ext, entry_script) = stage_two_emit_entry_script(
        &binaries.scripts_build,
        &workspace,
        &target_dir,
        &cli.manifest_path,
        &manifest,
    )?;
    let resolved = stage_three_resolve_levels(&entry_script, &manifest, &workspace)?;

    // Stage 4: bake model textures into the workspace materials tree.
    stage_four_bake_model_textures(&workspace, &manifest)?;

    // Stage 5: assemble the bundle tree (sources + tools + verbatim trees), with
    // the completion marker tracking the outstanding level bakes.
    stage_five_assemble_bundle(
        &workspace,
        &cli.output_root,
        &bundle_root,
        &bundle_name,
        &manifest,
        &binaries,
        entry_ext,
        &entry_script,
        &resolved,
    )?;

    // Stage 6: bake the levels into the bundle mod tree, rewriting the marker
    // after each bake (reused dist stage; package name is the -sdk bundle name so
    // its marker temp files never collide with a concurrent player payload run).
    stage_six_bake_levels(
        &workspace,
        &cli.output_root,
        &bundle_root,
        &bundle_name,
        &manifest.package.mod_root,
        &binaries.prl_build,
        &resolved,
    )?;

    // Stage 7: copy the baked materials into the bundle.
    stage_seven_copy_materials(&workspace, &bundle_root)?;

    // Stage 8: light SDK sweep, then drop the completion marker.
    sweep_sdk_bundle(
        &bundle_root,
        Path::new(&manifest.package.mod_root),
        entry_ext,
        &resolved,
    )?;

    fs::remove_file(bundle_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "sdk-dist stage 8: remove completion marker {}: {error}",
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

fn parse_args(args: Vec<OsString>, workspace: &Path) -> Result<SdkDistArgs, String> {
    let invocation_dir =
        std::env::current_dir().map_err(|error| format!("read invoking directory: {error}"))?;
    let mut manifest_path = workspace.join("dist.toml");
    let mut output_root = workspace.join("dist");
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].to_str().ok_or_else(|| {
            format!(
                "sdk-dist argument {} is not valid UTF-8",
                args[index].to_string_lossy()
            )
        })?;
        let value = match flag {
            "--manifest" | "--out" => args
                .get(index + 1)
                .ok_or_else(|| format!("sdk-dist {flag} requires a path\n\n{}", usage()))?,
            _ => return Err(format!("unknown sdk-dist argument `{flag}`\n\n{}", usage())),
        };
        let path = PathBuf::from(value);
        match flag {
            "--manifest" => {
                manifest_path = if path.is_absolute() {
                    path
                } else {
                    invocation_dir.join(path)
                };
            }
            "--out" => {
                output_root = if path.is_absolute() {
                    path
                } else {
                    workspace.join(path)
                };
            }
            _ => unreachable!("only recognized sdk-dist flags reach this branch"),
        }
        index += 2;
    }

    Ok(SdkDistArgs {
        manifest_path,
        output_root,
    })
}

fn usage() -> &'static str {
    "sdk-dist usage:\n  cargo run -p xtask -- sdk-dist [--manifest <path>] [--out <dir>]"
}

/// The SDK bundle root name: the player payload name plus a `-sdk` suffix so the
/// two never collide under `dist/`.
fn sdk_bundle_root_name(package_name: &str) -> String {
    format!("{package_name}-sdk")
}

fn stage_two_build_binaries(
    cargo: &OsStr,
    workspace: &Path,
    target_dir: &Path,
) -> Result<BuiltBinaries, String> {
    println!(
        "sdk-dist stage 2: build debug dev-tools postretro, release prl-build and scripts-build"
    );
    build_engine_debug(cargo, workspace)?;
    build_release_binary(cargo, workspace, "postretro-level-compiler", "prl-build")?;
    build_release_binary(
        cargo,
        workspace,
        "postretro-script-compiler",
        "scripts-build",
    )?;

    let debug_dir = target_dir.join("debug");
    let release_dir = target_dir.join("release");
    let binaries = BuiltBinaries {
        postretro: debug_dir.join(binary_name("postretro")),
        prl_build: release_dir.join(binary_name("prl-build")),
        scripts_build: release_dir.join(binary_name("scripts-build")),
    };
    for (label, path) in [
        ("debug postretro", &binaries.postretro),
        ("release prl-build", &binaries.prl_build),
        ("release scripts-build", &binaries.scripts_build),
    ] {
        if !path.is_file() {
            return Err(format!(
                "sdk-dist stage 2: {label} not found at {}",
                path.display()
            ));
        }
    }
    Ok(binaries)
}

/// Build the authoring engine: DEBUG (no `--release`) with `--features dev-tools`.
fn build_engine_debug(cargo: &OsStr, workspace: &Path) -> Result<(), String> {
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace)
        .arg("build")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--features")
        .arg("dev-tools");
    run_checked(
        &mut command,
        "sdk-dist stage 2 build postretro (debug, dev-tools)",
    )
}

fn build_release_binary(
    cargo: &OsStr,
    workspace: &Path,
    package: &str,
    binary: &str,
) -> Result<(), String> {
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace)
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg(package)
        .arg("--bin")
        .arg(binary);
    run_checked(&mut command, &format!("sdk-dist stage 2 build {binary}"))
}

#[allow(clippy::too_many_arguments)]
fn stage_five_assemble_bundle(
    workspace: &Path,
    output_root: &Path,
    bundle_root: &Path,
    bundle_name: &str,
    manifest: &Manifest,
    binaries: &BuiltBinaries,
    entry_ext: EntryExt,
    entry_script: &Path,
    resolved: &[Resolved],
) -> Result<(), String> {
    println!("sdk-dist stage 5: assemble content-complete bundle tree");
    guard_payload_root(bundle_root, workspace)
        .map_err(|error| format!("sdk-dist stage 5: {error}"))?;
    // The completion gate tracks the outstanding level bakes exactly as the
    // player payload does: the marker carries the full resolved set at assembly,
    // and stage 6 rewrites it after each bake.
    let ordered = bake_order(resolved);
    let all_outstanding = outstanding_outputs(&ordered, 0);
    replace_payload_root(output_root, bundle_root, bundle_name, &all_outstanding)?;

    fs::copy(
        &binaries.postretro,
        bundle_root.join(binary_name("postretro")),
    )
    .map_err(|error| {
        format!(
            "sdk-dist stage 5: copy debug postretro {}: {error}",
            binaries.postretro.display()
        )
    })?;

    let bin_dir = bundle_root.join("bin");
    fs::create_dir_all(&bin_dir).map_err(|error| {
        format!(
            "sdk-dist stage 5: create bin directory {}: {error}",
            bin_dir.display()
        )
    })?;
    fs::copy(&binaries.prl_build, bin_dir.join(binary_name("prl-build"))).map_err(|error| {
        format!(
            "sdk-dist stage 5: copy prl-build {}: {error}",
            binaries.prl_build.display()
        )
    })?;
    fs::copy(
        &binaries.scripts_build,
        bin_dir.join(binary_name("scripts-build")),
    )
    .map_err(|error| {
        format!(
            "sdk-dist stage 5: copy scripts-build {}: {error}",
            binaries.scripts_build.display()
        )
    })?;

    // These trees ship verbatim: they are not mod source, so no stale-output drop.
    for (name, source, destination) in [
        ("sdk", workspace.join("sdk"), bundle_root.join("sdk")),
        ("docs", workspace.join("docs"), bundle_root.join("docs")),
        ("tools", workspace.join("tools"), bundle_root.join("tools")),
        (
            "content/base",
            workspace.join("content").join("base"),
            bundle_root.join("content").join("base"),
        ),
    ] {
        let copied = copy_bundle_tree(&source, &destination, false)?;
        println!("  copied {copied} files from {name}/");
    }

    // The mod tree ships WITH its .map/.ts sources, but committed stale generated
    // .prl/.js are dropped: fresh .prl come from stage 6's level bake and the
    // fresh entry .js is installed just below.
    let mod_root_rel = Path::new(&manifest.package.mod_root);
    let mod_copied = copy_bundle_tree(
        &workspace.join(mod_root_rel),
        &bundle_root.join(mod_root_rel),
        true,
    )?;
    println!(
        "  copied {mod_copied} files from mod tree {} (with sources)",
        manifest.package.mod_root
    );

    // Install the emitted entry .js beside its .ts source (a TS mod). A Luau mod
    // already shipped its `start-script.luau` source via the tree copy and needs
    // no emitted sibling.
    if entry_ext == EntryExt::Js {
        let installed = bundle_root
            .join(mod_root_rel)
            .join(EntryExt::Js.file_name());
        fs::copy(entry_script, &installed).map_err(|error| {
            format!(
                "sdk-dist stage 5: install emitted entry script {}: {error}",
                installed.display()
            )
        })?;
    }

    let readme = render_readme(&manifest.package.name, &manifest.package.mod_root);
    fs::write(bundle_root.join("README.md"), readme).map_err(|error| {
        format!(
            "sdk-dist stage 5: write bundle README {}: {error}",
            bundle_root.join("README.md").display()
        )
    })?;

    Ok(())
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
/// confirms the required entries are present: the engine and compilers, `sdk/`,
/// the mod entry SOURCE, every resolved baked `maps/<name>.prl`, and the baked
/// `baked/materials/` tree.
fn sweep_sdk_bundle(
    bundle_root: &Path,
    mod_root: &Path,
    entry_ext: EntryExt,
    resolved: &[Resolved],
) -> Result<(), String> {
    let mod_dir = bundle_root.join(mod_root);

    let mut required_files = vec![
        bundle_root.join(binary_name("postretro")),
        bundle_root.join("bin").join(binary_name("prl-build")),
        bundle_root.join("bin").join(binary_name("scripts-build")),
        mod_dir.join(sdk_entry_source_name(entry_ext)),
    ];
    // Every resolved level must have baked into the bundle's maps/ tree.
    for resolved in resolved {
        required_files.push(mod_dir.join(&resolved.output));
    }
    for file in &required_files {
        if !file.is_file() {
            return Err(format!(
                "sdk-dist stage 8: required bundle file missing: {}",
                file.display()
            ));
        }
    }

    for dir in [
        bundle_root.join("sdk"),
        bundle_root.join("baked").join("materials"),
    ] {
        if !dir.is_dir() {
            return Err(format!(
                "sdk-dist stage 8: required bundle directory missing: {}",
                dir.display()
            ));
        }
    }
    Ok(())
}

/// Render the modder quickstart README shipped at the bundle root.
fn render_readme(package_name: &str, mod_root: &str) -> String {
    let engine_binary = binary_name("postretro");
    let prl_build = binary_name("prl-build");
    let scripts_build = binary_name("scripts-build");
    format!(
        "# {package_name} — content-complete modding SDK\n\
\n\
This is a complete, content-complete kit for `{package_name}`. It is\n\
**playable out of the box** — the levels and material mips are already baked —\n\
and **fully editable** — it ships the mod's `.map`/`.ts` sources, the level and\n\
script compilers, the SDK definitions, the human-facing docs, the asset tools,\n\
and a debug/hot-reload authoring engine. Play it as-is, or edit and reload.\n\
\n\
## Host-native caveat\n\
\n\
This bundle is **host-native**: the binaries were compiled for the operating\n\
system this bundle was built on and link native C/C++ dependencies. Run it on\n\
that same OS. To build for another platform, produce the bundle there.\n\
\n\
The engine is a **debug** build compiled with `--features dev-tools`: it\n\
auto-compiles `.ts` on load, hot-reloads script edits, and exposes the egui\n\
debug/inspector UI. It is not an optimized release build. To produce a lean,\n\
optimized **player** build (baked content, no sources or tools), see\n\
`docs/distribution.md`.\n\
\n\
## Flow A — play immediately\n\
\n\
Launch `{engine_binary}` from the bundle root. The baked `maps/*.prl` levels and\n\
`baked/materials/` mips are already present, so the game runs with no build\n\
step. The current working directory must be the bundle root so content paths\n\
resolve.\n\
\n\
## Flow B — author (edit and reload)\n\
\n\
1. **Edit a level** in TrenchBroom using `sdk/TrenchBroom/postretro.fgd` and\n\
   `sdk/TrenchBroom/GameConfig.cfg`.\n\
2. **Recompile the level**:\n\
   `bin/{prl_build} maps/<name>.map -o {mod_root}/maps/<name>.prl`.\n\
3. **Edit scripts** in TypeScript against `sdk/types/` and `sdk/lib/`. This debug\n\
   engine auto-compiles `.ts` on load and hot-reloads edits, so you can usually\n\
   skip a manual compile. To compile by hand:\n\
   `bin/{scripts_build} --in {mod_root}/start-script.ts --out {mod_root}/start-script.js`.\n\
4. **Re-run**: launch `{engine_binary}` from the bundle root again. The\n\
   `--features dev-tools` inspector is available in this build.\n\
\n\
## Further reading\n\
\n\
- `docs/level_design.md` — level authoring and the compiler pipeline.\n\
- `docs/scripting-reference.md` — the scripting API surface.\n\
- `docs/weapon-mounts.md` — weapon mount and viewmodel authoring.\n\
- `docs/diagnostics.md` — diagnosing and profiling a running session.\n\
- `tools/` — Python asset helpers. See `tools/README.md` for the Python and\n\
  library requirements they need.\n\
- `docs/distribution.md` — producing a lean, shippable **player** build with\n\
  `dist`.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_bundle_root_name_appends_sdk_suffix() {
        assert_eq!(sdk_bundle_root_name("postretro-dev"), "postretro-dev-sdk");
        assert_eq!(sdk_bundle_root_name("dev"), "dev-sdk");
    }

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
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("sdk")).unwrap();
        fs::create_dir_all(root.join("baked").join("materials")).unwrap();
        fs::write(root.join(binary_name("postretro")), "engine").unwrap();
        fs::write(root.join("bin").join(binary_name("prl-build")), "prl").unwrap();
        fs::write(
            root.join("bin").join(binary_name("scripts-build")),
            "scripts",
        )
        .unwrap();
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
        let mod_root = Path::new("content/dev");
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
        let mod_root = Path::new("content/dev");
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        fs::remove_file(root.join(mod_root).join("maps/campaign-test.prl")).unwrap();

        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("missing baked .prl is rejected");
        assert!(error.contains("campaign-test.prl"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sweep_requires_the_baked_materials_directory() {
        let root = unique_temp_dir();
        let mod_root = Path::new("content/dev");
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
        let mod_root = Path::new("content/dev");
        let levels = [resolved("maps/campaign-test.prl")];
        assemble_swept_bundle(&root, mod_root, &levels);
        fs::remove_file(root.join(mod_root).join("start-script.ts")).unwrap();

        let error = sweep_sdk_bundle(&root, mod_root, EntryExt::Js, &levels)
            .expect_err("missing entry source is rejected");
        assert!(error.contains("start-script.ts"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn readme_covers_both_flows_and_the_contract_points() {
        let readme = render_readme("postretro-dev", "content/dev");
        for needle in [
            "postretro-dev",
            "content-complete",
            "playable out of the box",
            "fully editable",
            "Flow A",
            "play immediately",
            "Flow B",
            "edit and reload",
            "host-native",
            "dev-tools",
            "hot-reload",
            "sdk/TrenchBroom/postretro.fgd",
            "GameConfig.cfg",
            "prl-build",
            "scripts-build",
            "content/dev/maps/<name>.prl",
            "docs/level_design.md",
            "docs/scripting-reference.md",
            "docs/weapon-mounts.md",
            "docs/diagnostics.md",
            "docs/distribution.md",
            "tools/",
        ] {
            assert!(readme.contains(needle), "README missing {needle:?}");
        }
    }
}
