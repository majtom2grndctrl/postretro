//! The content stages both distribution outputs share.
//!
//! Stage 1 of `build_pipeline.md` §Distribution packaging builds release
//! binaries, which needs cargo; stages 2 through 7 are pure content work and do
//! not. That is the one real seam in the command, and it is where the tool
//! begins: everything here runs from binaries somebody else already built, named
//! by `binaries.rs`. The stage numbers keep their published values so the
//! documented order stays readable, marker text included.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::binaries::run_checked;
use crate::model_textures::bake_model_textures_for_gltf;
use crate::project::Project;

use super::payload::write_marker;
use super::resolve::{
    EntryExt, Resolved, bake_order, entry_script_choice, outstanding_outputs, resolve_map_set,
    scan_map_literals,
};

/// Stage 2: emit the one release entry script into scratch outside the payload.
pub(crate) fn emit_entry_script(
    project: &Project,
    scripts_build: &Path,
) -> Result<(EntryExt, PathBuf), String> {
    println!("Stage 2: emit mod entry script");
    let canonical_manifest = fs::canonicalize(project.manifest_path()).map_err(|error| {
        format!(
            "stage 2: canonicalize manifest {}: {error}",
            project.manifest_path().display()
        )
    })?;
    let scratch_key = blake3::hash(canonical_manifest.as_os_str().as_encoded_bytes())
        .to_hex()
        .to_string();
    let scratch = project.scratch_root().join(&scratch_key[..16]);
    if scratch.exists() {
        fs::remove_dir_all(&scratch)
            .map_err(|error| format!("stage 2: clear scratch {}: {error}", scratch.display()))?;
    }
    fs::create_dir_all(&scratch)
        .map_err(|error| format!("stage 2: create scratch {}: {error}", scratch.display()))?;

    let mod_root = project.mod_root();
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
                .current_dir(project.root())
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

/// Stage 3: read the shipped level set out of the script stage 2 emitted.
pub(crate) fn resolve_levels(
    entry_script: &Path,
    project: &Project,
) -> Result<Vec<Resolved>, String> {
    println!("Stage 3: resolve emitted map set");
    let script = fs::read(entry_script).map_err(|error| {
        format!(
            "stage 3: read entry script {}: {error}",
            entry_script.display()
        )
    })?;
    let scanned = scan_map_literals(&script);
    let resolved = resolve_map_set(&scanned, project.manifest(), project.root())
        .map_err(|error| format!("stage 3: {error}"))?;
    println!("  resolved {} map outputs", resolved.len());
    Ok(resolved)
}

/// Stage 4: bake every mod glTF's base-color sidecars into the project's
/// materials tree. `prl-build` bakes model textures only for `prop_mesh`
/// placements, so rigs declared in script would otherwise reach a payload with
/// no sidecar and render placeholders.
pub(crate) fn bake_model_textures(project: &Project) -> Result<(), String> {
    println!("Stage 4: bake model textures");
    let models = project.mod_root().join("models");
    let mut gltfs = Vec::new();
    collect_model_files(&models, &mut gltfs)?;
    gltfs.sort();

    let prm_root = project.materials_root();
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

/// Where stage 6 writes, and under which name the marker tracks it.
pub(crate) struct BakeTarget<'a> {
    pub(crate) output_root: &'a Path,
    pub(crate) payload_root: &'a Path,
    /// The completion marker's temp-file namespace — the payload or bundle root
    /// name, so two concurrent runs never collide on it.
    pub(crate) marker_name: &'a str,
    /// The payload-relative mod root the baked levels land under.
    pub(crate) payload_mod_root: &'a str,
}

/// Stage 6: one `prl-build --release` per resolved level, straight into the
/// payload, rewriting the completion marker after each.
///
/// `--release` is the only shippable bake, so the tool supplies it and a
/// manifest recipe may not. `--baked-root` and `--cache-dir` travel together:
/// the first keeps the compiler writing `.prm` sidecars where the engine reads
/// them, the second keeps the disposable stage cache out of the author's `.map`
/// directory. Bakes run one at a time in `bake_order`.
pub(crate) fn bake_levels(
    project: &Project,
    target: &BakeTarget<'_>,
    prl_build: &Path,
    resolved: &[Resolved],
) -> Result<(), String> {
    println!("Stage 6: bake release levels");
    let ordered = bake_order(resolved);
    write_marker(
        target.payload_root,
        target.output_root,
        target.marker_name,
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
        let output = target
            .payload_root
            .join(target.payload_mod_root)
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
            .current_dir(project.root())
            .arg(&resolved.source)
            .arg("--release")
            .arg("--no-tui")
            .arg("--baked-root")
            .arg(project.baked_root())
            .arg("--cache-dir")
            .arg(project.stage_cache_dir())
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
            target.payload_root,
            target.output_root,
            target.marker_name,
            stage,
            &outstanding,
        )?;
    }
    Ok(())
}

/// Stage 7: copy the materials tree in, after every writer of it has run.
pub(crate) fn copy_materials(project: &Project, payload_root: &Path) -> Result<(), String> {
    println!("Stage 7: copy baked materials");
    let destination = payload_root.join("baked").join("materials");
    let copied = super::payload::copy_prm_tree(&project.materials_root(), &destination)?;
    println!("  copied {copied} material mip files");
    Ok(())
}
