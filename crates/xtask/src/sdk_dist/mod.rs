//! Host modder SDK bundle assembly.
//!
//! Unlike `dist` (which produces a runnable *player* payload — baked levels,
//! source-excluded content, a release engine), `sdk-dist` produces a *modder*
//! bundle that ships SOURCES and the tools to author with them: a DEBUG engine
//! built with `--features dev-tools` (TS auto-compile + hot reload require debug
//! assertions; the inspector requires the feature), the `prl-build` and
//! `scripts-build` compilers, the `sdk/`, `docs/`, and `tools/` trees, base
//! content, and the mod tree with its `.map`/`.ts` sources intact. It does not
//! bake levels and does not run the player payload's source-exclusion filter or
//! forbidden-source-artifact sweep.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dist::binary_name;
use crate::dist::cargo_target_dir;
use crate::dist::manifest::Manifest;
use crate::dist::payload::{MARKER_NAME, count_payload, replace_payload_root};
use crate::dist::resolve::{EntryExt, entry_script_choice, guard_payload_root, is_at_or_under};
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

    let binaries = stage_two_build_binaries(&cargo, &workspace, &target_dir)?;
    stage_three_assemble_bundle(
        &workspace,
        &cli.output_root,
        &bundle_root,
        &bundle_name,
        &manifest,
        &binaries,
    )?;
    sweep_sdk_bundle(&bundle_root, Path::new(&manifest.package.mod_root))?;

    fs::remove_file(bundle_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "sdk-dist stage 4: remove completion marker {}: {error}",
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

fn stage_three_assemble_bundle(
    workspace: &Path,
    output_root: &Path,
    bundle_root: &Path,
    bundle_name: &str,
    manifest: &Manifest,
    binaries: &BuiltBinaries,
) -> Result<(), String> {
    println!("sdk-dist stage 3: assemble SDK bundle tree");
    guard_payload_root(bundle_root, workspace)
        .map_err(|error| format!("sdk-dist stage 3: {error}"))?;
    // No baked levels ship in the SDK bundle, so nothing is ever outstanding.
    replace_payload_root(output_root, bundle_root, bundle_name, &[])?;

    fs::copy(
        &binaries.postretro,
        bundle_root.join(binary_name("postretro")),
    )
    .map_err(|error| {
        format!(
            "sdk-dist stage 3: copy debug postretro {}: {error}",
            binaries.postretro.display()
        )
    })?;

    let bin_dir = bundle_root.join("bin");
    fs::create_dir_all(&bin_dir).map_err(|error| {
        format!(
            "sdk-dist stage 3: create bin directory {}: {error}",
            bin_dir.display()
        )
    })?;
    fs::copy(&binaries.prl_build, bin_dir.join(binary_name("prl-build"))).map_err(|error| {
        format!(
            "sdk-dist stage 3: copy prl-build {}: {error}",
            binaries.prl_build.display()
        )
    })?;
    fs::copy(
        &binaries.scripts_build,
        bin_dir.join(binary_name("scripts-build")),
    )
    .map_err(|error| {
        format!(
            "sdk-dist stage 3: copy scripts-build {}: {error}",
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

    // The mod tree ships with its .map/.ts sources, but stale generated .prl/.js
    // are dropped: the modder regenerates those from source.
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

    let readme = render_readme(&manifest.package.name, &manifest.package.mod_root);
    fs::write(bundle_root.join("README.md"), readme).map_err(|error| {
        format!(
            "sdk-dist stage 3: write bundle README {}: {error}",
            bundle_root.join("README.md").display()
        )
    })?;

    Ok(())
}

/// Copy a directory tree into the bundle, skipping build caches, autosaves, VCS
/// metadata, and OS junk. When `skip_stale_outputs` is set (the mod tree), stale
/// generated `.prl`/`.js` build outputs are also dropped.
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
            "sdk-dist stage 3: create destination tree {}: {error}",
            destination.display()
        )
    })?;
    let entries = fs::read_dir(source).map_err(|error| {
        format!(
            "sdk-dist stage 3: read source tree {}: {error}",
            source.display()
        )
    })?;

    let mut copied = 0;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("sdk-dist stage 3: read source tree entry: {error}"))?;
        let source_path = entry.path();
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            return Err(format!(
                "sdk-dist stage 3: source tree entry {} is not valid UTF-8",
                source_path.display()
            ));
        };
        if should_skip_bundle_entry(&name, parent_name, skip_stale_outputs) {
            continue;
        }

        let destination_path = destination.join(&name);
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "sdk-dist stage 3: inspect source tree entry {}: {error}",
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
                    "sdk-dist stage 3: copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
            copied += 1;
        } else {
            return Err(format!(
                "sdk-dist stage 3: refuse non-regular source tree entry {}",
                source_path.display()
            ));
        }
    }
    Ok(copied)
}

/// Return whether a tree entry must not enter the SDK bundle.
///
/// Always drops build caches, `maps/autosave/`, any `.git*` entry, and
/// `.DS_Store`. When `skip_stale_outputs` is set, also drops generated
/// `.prl`/`.js` (the modder regenerates those from source).
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
/// The SDK bundle ships the authoring *source*, not the emitted runtime script:
/// a TypeScript mod ships `start-script.ts`, a Luau mod ships `start-script.luau`.
fn sdk_entry_source_name(choice: EntryExt) -> &'static str {
    match choice {
        EntryExt::Js => "start-script.ts",
        EntryExt::Luau => "start-script.luau",
    }
}

/// Light completion check: confirm the required top-level entries exist. Unlike
/// the player sweep, sources (`.map`/`.ts`/`.md`) are allowed and expected here.
fn sweep_sdk_bundle(bundle_root: &Path, mod_root: &Path) -> Result<(), String> {
    let mod_dir = bundle_root.join(mod_root);
    let choice = entry_script_choice(
        mod_dir.join("start-script.ts").is_file(),
        mod_dir.join("start-script.luau").is_file(),
    )
    .map_err(|error| format!("sdk-dist stage 4: {}: {error}", mod_dir.display()))?;
    let entry_script = mod_dir.join(sdk_entry_source_name(choice));

    let files = [
        bundle_root.join(binary_name("postretro")),
        bundle_root.join("bin").join(binary_name("prl-build")),
        bundle_root.join("bin").join(binary_name("scripts-build")),
        entry_script,
    ];
    for file in &files {
        if !file.is_file() {
            return Err(format!(
                "sdk-dist stage 4: required bundle file missing: {}",
                file.display()
            ));
        }
    }
    let sdk_dir = bundle_root.join("sdk");
    if !sdk_dir.is_dir() {
        return Err(format!(
            "sdk-dist stage 4: required bundle directory missing: {}",
            sdk_dir.display()
        ));
    }
    Ok(())
}

/// Render the modder quickstart README shipped at the bundle root.
fn render_readme(package_name: &str, mod_root: &str) -> String {
    let engine_binary = binary_name("postretro");
    let prl_build = binary_name("prl-build");
    let scripts_build = binary_name("scripts-build");
    format!(
        "# {package_name} — modding SDK\n\
\n\
This is a complete modding kit for `{package_name}`: an authoring engine, the\n\
level and script compilers, the SDK definitions, the human-facing docs, the\n\
asset tools, base content, and the mod tree with its `.map`/`.ts` sources. With\n\
it you can author levels and scripts and run them directly against the engine.\n\
\n\
## Host-native caveat\n\
\n\
This bundle is **host-native**: the binaries were compiled for the operating\n\
system this bundle was built on and link native C/C++ dependencies. Run it on\n\
that same OS. To build for another platform, produce the bundle there.\n\
\n\
The authoring engine is a **debug** build compiled with `--features dev-tools`:\n\
it auto-compiles `.ts` on load, hot-reloads script edits, and exposes the\n\
egui debug/inspector UI. It is not an optimized release build.\n\
\n\
## Quickstart\n\
\n\
1. **Author a level** in TrenchBroom using `sdk/TrenchBroom/postretro.fgd` and\n\
   `sdk/TrenchBroom/GameConfig.cfg`.\n\
2. **Compile the level**:\n\
   `bin/{prl_build} maps/<name>.map -o {mod_root}/maps/<name>.prl`.\n\
3. **Author scripts** in TypeScript against `sdk/types/` and `sdk/lib/`. The\n\
   debug engine auto-compiles `.ts` on load and hot-reloads edits, so you can\n\
   usually skip a manual compile. To compile by hand:\n\
   `bin/{scripts_build} --in {mod_root}/start-script.ts --out {mod_root}/start-script.js`.\n\
4. **Run**: launch `{engine_binary}` from the bundle root. The current working\n\
   directory must be the bundle root so content paths resolve. The\n\
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
- `docs/distribution.md` — producing a shippable **player** build with `dist`.\n"
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

    #[test]
    fn readme_covers_the_contract_points() {
        let readme = render_readme("postretro-dev", "content/dev");
        for needle in [
            "postretro-dev",
            "modding",
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
