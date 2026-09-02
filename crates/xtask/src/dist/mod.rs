//! Host distribution assembly for a runnable Postretro payload.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{bake_model_textures_for_gltf, run_checked, workspace_root};

mod launcher;
pub(crate) mod manifest;
mod payload;
mod resolve;

use manifest::Manifest;
use payload::{copy_filtered_tree, copy_prm_tree, sweep_payload};
use resolve::{
    EntryExt, Resolved, bake_order, entry_script_choice, guard_payload_root, is_at_or_under,
    outstanding_outputs, resolve_map_set, scan_map_literals,
};

const MARKER_NAME: &str = ".dist-incomplete";

struct DistArgs {
    manifest_path: PathBuf,
    output_root: PathBuf,
}

struct BuiltBinaries {
    postretro: PathBuf,
    prl_build: PathBuf,
    scripts_build: PathBuf,
}

/// Information stage 2 and stage 3 establish for all following stages.
struct RunState {
    entry_ext: EntryExt,
    entry_script: PathBuf,
    resolved: Vec<Resolved>,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let workspace = workspace_root()?;
    let cli = parse_args(args, &workspace)?;
    let manifest = Manifest::read(&cli.manifest_path)?;
    let payload_root = cli.output_root.join(&manifest.package.name);

    guard_payload_root(&payload_root, &workspace).map_err(|error| error.to_string())?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let target_dir = cargo_target_dir(&cargo, &workspace)?;
    let workspace_dist = workspace.join("dist");
    let target_inside_dist = is_at_or_under(&target_dir, &workspace_dist).map_err(|error| {
        format!(
            "stage 1: compare cargo target directory {} with {}: {error}",
            target_dir.display(),
            workspace_dist.display()
        )
    })?;
    if target_inside_dist {
        return Err(format!(
            "stage 1: refuse cargo target directory {} at or under workspace dist/",
            target_dir.display()
        ));
    }

    let binaries = stage_one_build_binaries(&cargo, &workspace, &target_dir)?;
    let (entry_ext, entry_script) = stage_two_emit_entry_script(
        &binaries.scripts_build,
        &workspace,
        &target_dir,
        &cli.manifest_path,
        &manifest,
    )?;
    let resolved = stage_three_resolve_levels(&entry_script, &manifest, &workspace)?;
    let state = RunState {
        entry_ext,
        entry_script,
        resolved,
    };

    stage_four_bake_model_textures(&workspace, &manifest)?;
    stage_five_assemble_payload(
        &workspace,
        &cli.output_root,
        &payload_root,
        &manifest,
        &binaries.postretro,
        &state,
    )?;
    stage_six_bake_levels(
        &workspace,
        &cli.output_root,
        &payload_root,
        &manifest,
        &binaries.prl_build,
        &state,
    )?;
    stage_seven_copy_materials(&workspace, &payload_root)?;
    sweep_payload(
        &payload_root,
        Path::new(&manifest.package.mod_root),
        state.entry_ext,
        &state.resolved,
    )?;
    fs::remove_file(payload_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "payload sweep: remove completion marker {}: {error}",
            payload_root.join(MARKER_NAME).display()
        )
    })?;

    let (files, bytes) = count_payload(&payload_root)?;
    println!(
        "Distribution complete: {files} files, {bytes} bytes at {}",
        payload_root.display()
    );
    Ok(0)
}

fn parse_args(args: Vec<OsString>, workspace: &Path) -> Result<DistArgs, String> {
    let invocation_dir =
        std::env::current_dir().map_err(|error| format!("read invoking directory: {error}"))?;
    let mut manifest_path = workspace.join("dist.toml");
    let mut output_root = workspace.join("dist");
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].to_str().ok_or_else(|| {
            format!(
                "dist argument {} is not valid UTF-8",
                args[index].to_string_lossy()
            )
        })?;
        let value = match flag {
            "--manifest" | "--out" => args
                .get(index + 1)
                .ok_or_else(|| format!("dist {flag} requires a path\n\n{}", usage()))?,
            _ => return Err(format!("unknown dist argument `{flag}`\n\n{}", usage())),
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
            _ => unreachable!("only recognized dist flags reach this branch"),
        }
        index += 2;
    }

    Ok(DistArgs {
        manifest_path,
        output_root,
    })
}

pub(crate) fn usage() -> &'static str {
    "dist usage:\n  cargo run -p xtask -- dist [--manifest <path>] [--out <dir>]"
}

fn cargo_target_dir(cargo: &OsStr, workspace: &Path) -> Result<PathBuf, String> {
    let output = Command::new(cargo)
        .current_dir(workspace)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .output()
        .map_err(|error| format!("stage 1: run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "stage 1: cargo metadata exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("stage 1: parse cargo metadata: {error}"))?;
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("stage 1: cargo metadata omitted target_directory")?;
    Ok(PathBuf::from(target_dir))
}

fn stage_one_build_binaries(
    cargo: &OsStr,
    workspace: &Path,
    target_dir: &Path,
) -> Result<BuiltBinaries, String> {
    println!("Stage 1: build release postretro, prl-build, and scripts-build");
    cargo_build(cargo, workspace, "postretro", "postretro")?;
    cargo_build(cargo, workspace, "postretro-level-compiler", "prl-build")?;
    cargo_build(
        cargo,
        workspace,
        "postretro-script-compiler",
        "scripts-build",
    )?;

    let release_dir = target_dir.join("release");
    let binaries = BuiltBinaries {
        postretro: release_dir.join(binary_name("postretro")),
        prl_build: release_dir.join(binary_name("prl-build")),
        scripts_build: release_dir.join(binary_name("scripts-build")),
    };
    for (label, path) in [
        ("postretro", &binaries.postretro),
        ("prl-build", &binaries.prl_build),
        ("scripts-build", &binaries.scripts_build),
    ] {
        if !path.is_file() {
            return Err(format!(
                "stage 1: release {label} not found at {}",
                path.display()
            ));
        }
    }
    Ok(binaries)
}

fn cargo_build(cargo: &OsStr, workspace: &Path, package: &str, binary: &str) -> Result<(), String> {
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace)
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg(package)
        .arg("--bin")
        .arg(binary);
    run_checked(&mut command, &format!("stage 1 build {binary}"))
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn stage_two_emit_entry_script(
    scripts_build: &Path,
    workspace: &Path,
    target_dir: &Path,
    manifest_path: &Path,
    manifest: &Manifest,
) -> Result<(EntryExt, PathBuf), String> {
    println!("Stage 2: emit mod entry script");
    let canonical_manifest = fs::canonicalize(manifest_path).map_err(|error| {
        format!(
            "stage 2: canonicalize manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let scratch_key = blake3::hash(canonical_manifest.as_os_str().as_encoded_bytes())
        .to_hex()
        .to_string();
    let scratch = target_dir.join("dist-work").join(&scratch_key[..16]);
    if scratch.exists() {
        fs::remove_dir_all(&scratch)
            .map_err(|error| format!("stage 2: clear scratch {}: {error}", scratch.display()))?;
    }
    fs::create_dir_all(&scratch)
        .map_err(|error| format!("stage 2: create scratch {}: {error}", scratch.display()))?;

    let mod_root = workspace.join(&manifest.package.mod_root);
    let choice = entry_script_choice(
        mod_root.join("start-script.ts").is_file(),
        mod_root.join("start-script.luau").is_file(),
    )
    .map_err(|error| format!("stage 2: {}: {error}", mod_root.display()))?;

    let output = scratch.join(choice.file_name());
    match choice {
        EntryExt::Js => {
            let mut command = Command::new(scripts_build);
            command
                .current_dir(workspace)
                .arg("--in")
                .arg(mod_root.join("start-script.ts"))
                .arg("--out")
                .arg(&output);
            run_checked(&mut command, "stage 2 bundle start-script.ts")?;
        }
        EntryExt::Luau => {
            fs::copy(mod_root.join("start-script.luau"), &output).map_err(|error| {
                format!(
                    "stage 2: copy Luau entry script to {}: {error}",
                    output.display()
                )
            })?;
        }
    }
    Ok((choice, output))
}

fn stage_three_resolve_levels(
    entry_script: &Path,
    manifest: &Manifest,
    workspace: &Path,
) -> Result<Vec<Resolved>, String> {
    println!("Stage 3: resolve emitted map set");
    let script = fs::read(entry_script).map_err(|error| {
        format!(
            "stage 3: read entry script {}: {error}",
            entry_script.display()
        )
    })?;
    let scanned = scan_map_literals(&script);
    let resolved = resolve_map_set(&scanned, manifest, workspace)
        .map_err(|error| format!("stage 3: {error}"))?;
    println!("  resolved {} map outputs", resolved.len());
    Ok(resolved)
}

fn stage_four_bake_model_textures(workspace: &Path, manifest: &Manifest) -> Result<(), String> {
    println!("Stage 4: bake model textures");
    let models = workspace.join(&manifest.package.mod_root).join("models");
    let mut gltfs = Vec::new();
    collect_model_files(&models, &mut gltfs)?;
    gltfs.sort();

    let prm_root = workspace.join("baked").join("materials");
    for gltf in &gltfs {
        let baked = bake_model_textures_for_gltf(gltf, &prm_root)
            .map_err(|error| format!("stage 4: {error}"))?;
        if baked.is_empty() {
            println!("  {}: no filesystem base-color textures", gltf.display());
        }
    }
    println!("  baked model textures for {} model files", gltfs.len());
    Ok(())
}

fn collect_model_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "stage 4: read models directory {}: {error}",
                directory.display()
            ));
        }
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("stage 4: read model directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_model_files(&path, files)?;
        } else if matches!(
            path.extension().and_then(OsStr::to_str),
            Some("gltf" | "glb")
        ) {
            files.push(path);
        }
    }
    Ok(())
}

fn stage_five_assemble_payload(
    workspace: &Path,
    output_root: &Path,
    payload_root: &Path,
    manifest: &Manifest,
    postretro: &Path,
    state: &RunState,
) -> Result<(), String> {
    println!("Stage 5: assemble payload tree");
    guard_payload_root(payload_root, workspace).map_err(|error| format!("stage 5: {error}"))?;
    fs::create_dir_all(output_root).map_err(|error| {
        format!(
            "stage 5: create output root {}: {error}",
            output_root.display()
        )
    })?;
    clear_stale_stage_five_siblings(output_root, &manifest.package.name)?;

    let ordered = bake_order(&state.resolved);
    let all_outstanding = outstanding_outputs(&ordered, 0);
    if payload_root.exists() {
        let aside = output_root.join(format!(
            ".{}.deleting-{}",
            manifest.package.name,
            std::process::id()
        ));
        fs::rename(payload_root, &aside).map_err(|error| {
            format!(
                "stage 5: rename payload root {} aside to {}: {error}",
                payload_root.display(),
                aside.display()
            )
        })?;
        if let Err(remove_error) = fs::remove_dir_all(&aside) {
            let marker_result = write_marker(
                &aside,
                output_root,
                &manifest.package.name,
                "stage 5",
                &all_outstanding,
            );
            let restore_result = fs::rename(&aside, payload_root);
            return match (marker_result, restore_result) {
                (Ok(()), Ok(())) => Err(format!(
                    "stage 5: remove old payload aside {} failed: {remove_error}; restored partial payload at {}",
                    aside.display(),
                    payload_root.display()
                )),
                (marker_error, restore_error) => Err(format!(
                    "stage 5: remove old payload {} aside {} failed: {remove_error}; marker result: {}; restore result: {}",
                    payload_root.display(),
                    aside.display(),
                    marker_error
                        .err()
                        .map_or_else(|| "ok".to_string(), |error| error.to_string()),
                    restore_error
                        .err()
                        .map_or_else(|| "ok".to_string(), |error| error.to_string())
                )),
            };
        }
    }

    fs::create_dir_all(payload_root).map_err(|error| {
        format!(
            "stage 5: create payload root {}: {error}",
            payload_root.display()
        )
    })?;
    write_marker(
        payload_root,
        output_root,
        &manifest.package.name,
        "stage 5",
        &all_outstanding,
    )?;

    fs::copy(postretro, payload_root.join(binary_name("postretro"))).map_err(|error| {
        format!(
            "stage 5: copy release postretro {}: {error}",
            postretro.display()
        )
    })?;
    launcher::emit_launcher(
        payload_root,
        &manifest.package.name,
        &manifest.package.mod_root,
    )?;
    let workspace_mod_root = workspace.join(&manifest.package.mod_root);
    copy_filtered_tree(
        &workspace.join("content").join("base"),
        &payload_root.join("content").join("base"),
        &workspace_mod_root,
        state.entry_ext,
    )?;
    copy_filtered_tree(
        &workspace_mod_root,
        &payload_root.join(&manifest.package.mod_root),
        &workspace_mod_root,
        state.entry_ext,
    )?;

    let payload_mod_root = payload_root.join(&manifest.package.mod_root);
    remove_if_exists(&payload_mod_root.join("start-script.js"))?;
    remove_if_exists(&payload_mod_root.join("start-script.luau"))?;
    fs::copy(
        &state.entry_script,
        payload_mod_root.join(state.entry_ext.file_name()),
    )
    .map_err(|error| {
        format!(
            "stage 5: install entry script {}: {error}",
            state.entry_script.display()
        )
    })?;
    Ok(())
}

fn clear_stale_stage_five_siblings(output_root: &Path, package_name: &str) -> Result<(), String> {
    let deleting_prefix = format!(".{package_name}.deleting-");
    let marker_prefix = format!(".{package_name}.marker-");
    for entry in fs::read_dir(output_root).map_err(|error| {
        format!(
            "stage 5: scan output root {}: {error}",
            output_root.display()
        )
    })? {
        let entry = entry.map_err(|error| format!("stage 5: read output root entry: {error}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&deleting_prefix)
            || (name.starts_with(&marker_prefix) && name.ends_with(".tmp"))
        {
            remove_path(&entry.path()).map_err(|error| {
                format!(
                    "stage 5: remove stale sibling {}: {error}",
                    entry.path().display()
                )
            })?;
        }
    }
    Ok(())
}

fn write_marker(
    destination_root: &Path,
    output_root: &Path,
    package_name: &str,
    stage: &str,
    outstanding: &[String],
) -> Result<(), String> {
    let mut contents = String::from(stage);
    contents.push('\n');
    for output in outstanding {
        contents.push_str(output);
        contents.push('\n');
    }

    let temporary = output_root.join(format!(".{package_name}.marker-{}.tmp", std::process::id()));
    fs::write(&temporary, contents).map_err(|error| {
        format!(
            "{stage}: write marker temporary {}: {error}",
            temporary.display()
        )
    })?;
    fs::rename(&temporary, destination_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "{stage}: install marker into {}: {error}",
            destination_root.join(MARKER_NAME).display()
        )
    })
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stage 5: remove {}: {error}", path.display())),
    }
}

fn stage_six_bake_levels(
    workspace: &Path,
    output_root: &Path,
    payload_root: &Path,
    manifest: &Manifest,
    prl_build: &Path,
    state: &RunState,
) -> Result<(), String> {
    println!("Stage 6: bake release levels");
    let ordered = bake_order(&state.resolved);
    write_marker(
        payload_root,
        output_root,
        &manifest.package.name,
        "stage 6",
        &outstanding_outputs(&ordered, 0),
    )?;
    println!("  bake order:");
    for resolved in &ordered {
        println!(
            "    {} (density {})",
            resolved.output, resolved.lightmap_density
        );
    }

    for (index, resolved) in ordered.iter().enumerate() {
        let output = payload_root
            .join(&manifest.package.mod_root)
            .join(&resolved.output);
        let parent = output.parent().expect("resolved output has a maps/ parent");
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "stage 6: create output parent {}: {error}",
                parent.display()
            )
        })?;

        let mut command = Command::new(prl_build);
        command
            .current_dir(workspace)
            .arg(&resolved.source)
            .arg("--release")
            .arg("--no-tui")
            .args(&resolved.args)
            .arg("-o")
            .arg(&output);
        run_checked(
            &mut command,
            &format!(
                "stage 6 bake {} from {}",
                resolved.output,
                resolved.source.display()
            ),
        )?;

        let outstanding = outstanding_outputs(&ordered, index + 1);
        let stage = if outstanding.is_empty() {
            "stage 7"
        } else {
            "stage 6"
        };
        write_marker(
            payload_root,
            output_root,
            &manifest.package.name,
            stage,
            &outstanding,
        )?;
    }
    Ok(())
}

fn stage_seven_copy_materials(workspace: &Path, payload_root: &Path) -> Result<(), String> {
    println!("Stage 7: copy baked materials");
    let source = workspace.join("baked").join("materials");
    let destination = payload_root.join("baked").join("materials");
    let copied = copy_prm_tree(&source, &destination)?;
    println!("  copied {copied} material mip files");
    Ok(())
}

fn count_payload(root: &Path) -> Result<(u64, u64), String> {
    let mut files = 0;
    let mut bytes = 0;
    count_payload_tree(root, &mut files, &mut bytes)?;
    Ok((files, bytes))
}

fn count_payload_tree(directory: &Path, files: &mut u64, bytes: &mut u64) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("count payload directory {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("read payload directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            count_payload_tree(&path, files, bytes)?;
        } else {
            let metadata = fs::metadata(&path)
                .map_err(|error| format!("read payload file {}: {error}", path.display()))?;
            *files += 1;
            *bytes += metadata.len();
        }
    }
    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
