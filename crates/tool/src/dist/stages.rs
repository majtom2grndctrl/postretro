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

use postretro_model::gltf_loader::skinned_clip_library_path;

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
///
/// The same walk refuses a skinned model under `models/` whose clip library would
/// not reach the payload: the reference loads in dev and fails to load from the
/// shipped tree.
pub(crate) fn bake_model_textures(project: &Project) -> Result<(), String> {
    println!("Stage 4: bake model textures");
    let models = project.mod_root().join("models");
    let mut gltfs = Vec::new();
    collect_model_files(&models, &mut gltfs)?;
    gltfs.sort();

    let prm_root = project.materials_root();
    for gltf in &gltfs {
        check_clip_library(gltf, &project.mod_root())?;
        let baked = bake_model_textures_for_gltf(gltf, &prm_root)
            .map_err(|error| format!("stage 4: {error}"))?;
        if baked.is_empty() {
            println!("  {}: no filesystem base-color textures", gltf.display());
        }
    }
    println!("  baked model textures for {} model files", gltfs.len());
    Ok(())
}

/// Refuse `gltf` when it is a skinned model naming a library that is missing or
/// lies outside `mod_root`, the only tree a payload copies. Canonical paths, so a
/// reference's `..` is judged by where it lands and not by how it reads.
fn check_clip_library(gltf: &Path, mod_root: &Path) -> Result<(), String> {
    let Some(library) = skinned_clip_library_path(gltf)
        .map_err(|error| format!("stage 4: read clip library reference: {error}"))?
    else {
        return Ok(());
    };
    let missing = |reason: String| {
        format!(
            "stage 4: {} names clip library {}, which is missing: {reason}",
            gltf.display(),
            library.display()
        )
    };
    let resolved = fs::canonicalize(&library).map_err(|error| missing(error.to_string()))?;
    if !resolved.is_file() {
        return Err(missing("not a file".to_string()));
    }
    let mod_root = fs::canonicalize(mod_root)
        .map_err(|error| format!("stage 4: canonicalize {}: {error}", mod_root.display()))?;
    if !resolved.starts_with(&mod_root) {
        return Err(format!(
            "stage 4: {} names clip library {}, which resolves outside the mod root {}",
            gltf.display(),
            resolved.display(),
            mod_root.display()
        ));
    }
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

#[cfg(test)]
mod tests {
    use postretro_model::gltf_loader::load_model;
    use tempfile::TempDir;

    use super::super::payload::copy_filtered_tree;
    use super::*;
    use crate::project::MARKER_FILE;

    /// The reference the fixture models carry.
    const FIXTURE_REFERENCE: &str = "\"../library/library.gltf\"";

    /// A fixture folder of the model crate's clip library set: `mesh` is a bound
    /// skinned model, `static` a static model naming a library, `library` a
    /// mesh-less library with an external buffer.
    fn fixture(folder: &str) -> PathBuf {
        PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR")
                .expect("cargo sets CARGO_MANIFEST_DIR for its own test runs"),
        )
        .join("../model/tests/fixtures/clip_library")
        .join(folder)
    }

    fn project() -> (TempDir, Project) {
        let dir = tempfile::tempdir().expect("temporary project created");
        let manifest = dir.path().join(MARKER_FILE);
        fs::write(&manifest, "[package]\nname = \"game\"\nmod = \"game\"\n")
            .expect("manifest written");
        let project = Project::open(&manifest).expect("manifest opens");
        fs::create_dir_all(project.mod_root().join("models")).expect("models directory created");
        (dir, project)
    }

    /// Write fixture `folder`'s model at `path`, its `clipLibrary` value set to
    /// the JSON `reference`.
    fn write_model(path: &Path, folder: &str, reference: &str) {
        let text = fs::read_to_string(fixture(folder).join("model.gltf")).expect("fixture reads");
        assert!(
            text.contains(FIXTURE_REFERENCE),
            "the fixture names a library"
        );
        fs::create_dir_all(path.parent().unwrap()).expect("model directory created");
        fs::write(path, text.replace(FIXTURE_REFERENCE, reference)).expect("model written");
    }

    fn write_library(directory: &Path) {
        fs::create_dir_all(directory).expect("library directory created");
        for file in ["library.gltf", "library.bin"] {
            fs::copy(fixture("library").join(file), directory.join(file)).expect("library copied");
        }
    }

    fn hero(project: &Project) -> PathBuf {
        project.mod_root().join("models/hero/model.gltf")
    }

    #[test]
    fn stage_four_refuses_a_skinned_model_whose_library_is_missing() {
        let (_dir, project) = project();
        write_model(&hero(&project), "mesh", "\"../libraries/absent.gltf\"");
        let error = bake_model_textures(&project).expect_err("a missing library is refused");
        assert!(error.contains("is missing"), "{error}");
        assert!(error.contains("hero"), "the error names the model: {error}");

        // A directory canonicalizes, and a load cannot read it.
        fs::create_dir_all(project.mod_root().join("models/libraries/absent.gltf")).unwrap();
        let error = bake_model_textures(&project).expect_err("a directory is no library");
        assert!(error.contains("is missing"), "{error}");
    }

    #[test]
    fn stage_four_refuses_an_existing_library_outside_the_mod_root() {
        let (dir, project) = project();
        // The second shares the mod root's name as a string prefix.
        for folder in ["shared", "game-extras"] {
            write_library(&dir.path().join("content").join(folder));
            let reference = format!("\"../../../{folder}/library.gltf\"");
            write_model(&hero(&project), "mesh", &reference);
            let error =
                bake_model_textures(&project).expect_err("an out-of-mod library is refused");
            assert!(error.contains("outside the mod root"), "{folder}: {error}");
        }
    }

    #[test]
    fn stage_four_accepts_a_nested_model_whose_reference_leaves_models_but_not_the_mod() {
        let (_dir, project) = project();
        write_library(&project.mod_root().join("clip_libraries/rifle"));
        write_model(
            &project.mod_root().join("models/squad/hero/model.gltf"),
            "mesh",
            "\"../../../clip_libraries/rifle/library.gltf\"",
        );
        bake_model_textures(&project).expect("the library stays in the mod");
    }

    #[test]
    fn stage_four_skips_static_models_and_malformed_references() {
        let (dir, project) = project();
        write_library(&dir.path().join("content/shared"));
        let models = project.mod_root().join("models");
        write_model(&models.join("crate/a.gltf"), "static", "\"absent.gltf\"");
        write_model(
            &models.join("crate/b.gltf"),
            "static",
            "\"../../../shared/library.gltf\"",
        );
        for (index, malformed) in ["\"file:lib.gltf\"", "\"/rooted.gltf\"", "\"\"", "7"]
            .iter()
            .enumerate()
        {
            write_model(&models.join(format!("odd/{index}.gltf")), "mesh", malformed);
        }
        bake_model_textures(&project).expect("each of these loads, so each passes");
    }

    #[test]
    fn stage_four_leaves_a_librarys_buffers_unchecked() {
        let (_dir, project) = project();
        let library = project.mod_root().join("models/libraries");
        write_library(&library);
        fs::remove_file(library.join("library.bin")).unwrap();
        write_model(&hero(&project), "mesh", "\"../libraries/library.gltf\"");
        bake_model_textures(&project).expect("a library's buffers are not stage 4's check");
    }

    // `dist` end to end needs release binaries; stage 4 and the payload's mod-tree
    // copy are the stages a library passes through. The source mod is gone before
    // the load, so the clips can only come from the payload.
    #[test]
    fn library_under_models_ships_in_the_payload_and_binds_from_it() {
        let (dir, project) = project();
        let library = "models/clip_libraries/rifle";
        write_library(&project.mod_root().join(library));
        write_model(
            &hero(&project),
            "mesh",
            "\"../clip_libraries/rifle/library.gltf\"",
        );

        bake_model_textures(&project).expect("stage 4 passes a mesh-less library");

        let source = project.mod_root();
        let payload = dir.path().join("payload").join(project.mod_root_rel());
        copy_filtered_tree(&source, &payload, &source, EntryExt::Js).expect("the mod tree copies");
        fs::remove_dir_all(&source).unwrap();

        for file in ["library.gltf", "library.bin"] {
            let shipped = payload.join(library).join(file);
            assert!(shipped.is_file(), "{} ships", shipped.display());
        }
        let model = load_model(&payload.join("models/hero/model.gltf"))
            .expect("the bound model loads from the payload");
        assert!(
            model.clips.iter().any(|clip| clip.name == "leap"),
            "the library's clip binds: {:?}",
            model
                .clips
                .iter()
                .map(|clip| &clip.name)
                .collect::<Vec<_>>()
        );
    }
}
