//! Payload tree assembly, completion tracking, and verification.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::resolve::{EntryExt, Resolved, is_prm_filename};

pub(super) const MARKER_NAME: &str = ".dist-incomplete";

/// Replace any prior payload with an empty root carrying the completion marker.
pub(super) fn replace_payload_root(
    output_root: &Path,
    payload_root: &Path,
    package_name: &str,
    outstanding: &[String],
) -> Result<(), String> {
    fs::create_dir_all(output_root).map_err(|error| {
        format!(
            "stage 5: create output root {}: {error}",
            output_root.display()
        )
    })?;
    clear_stale_stage_five_siblings(output_root, package_name)?;

    if payload_root.exists() {
        let aside = output_root.join(format!(".{package_name}.deleting-{}", std::process::id()));
        replace_existing_payload_with(
            payload_root,
            &aside,
            |path| fs::remove_dir_all(path),
            |path| write_marker(path, output_root, package_name, "stage 5", outstanding),
        )?;
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
        package_name,
        "stage 5",
        outstanding,
    )
}

fn replace_existing_payload_with<RemoveTree, MarkPartial>(
    payload_root: &Path,
    aside: &Path,
    remove_tree: RemoveTree,
    mark_partial: MarkPartial,
) -> Result<(), String>
where
    RemoveTree: FnOnce(&Path) -> io::Result<()>,
    MarkPartial: FnOnce(&Path) -> Result<(), String>,
{
    fs::rename(payload_root, aside).map_err(|error| {
        format!(
            "stage 5: rename payload root {} aside to {}: {error}",
            payload_root.display(),
            aside.display()
        )
    })?;

    let Err(remove_error) = remove_tree(aside) else {
        return Ok(());
    };
    if let Err(marker_error) = mark_partial(aside) {
        return Err(format!(
            "stage 5: remove old payload {} aside {} failed: {remove_error}; mark partial payload at {} failed: {marker_error}; partial payload was not restored",
            payload_root.display(),
            aside.display(),
            aside.display()
        ));
    }

    fs::rename(aside, payload_root).map_err(|restore_error| {
        format!(
            "stage 5: remove old payload {} aside {} failed: {remove_error}; restore failed: {restore_error}; marked partial payload remains at {}",
            payload_root.display(),
            aside.display(),
            aside.display()
        )
    })?;
    Err(format!(
        "stage 5: remove old payload aside {} failed: {remove_error}; restored marked partial payload at {}",
        aside.display(),
        payload_root.display()
    ))
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

pub(super) fn write_marker(
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

pub(super) fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stage 5: remove {}: {error}", path.display())),
    }
}

/// Return whether a working-tree path must not enter the payload tree.
pub(super) fn should_exclude(path: &Path, mod_root: &Path, emitted: EntryExt) -> bool {
    if has_component(path, ".build-caches")
        || has_component_pair(path, "maps", "autosave")
        || matches!(
            file_name(path),
            Some(".gitignore" | ".gitkeep" | ".DS_Store")
        )
        || matches!(
            extension(path),
            Some("map" | "ts" | "md" | "prl" | "js" | "bsp")
        )
    {
        return true;
    }

    if extension(path) != Some("luau") {
        return false;
    }

    let Ok(relative) = path.strip_prefix(mod_root) else {
        return false;
    };
    if relative.starts_with("scripts") {
        return true;
    }

    emitted == EntryExt::Js && relative.components().count() == 1
}

/// Copy a content tree while excluding source-only and stale generated files.
pub(super) fn copy_filtered_tree(
    source: &Path,
    destination: &Path,
    mod_root: &Path,
    emitted: EntryExt,
) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "stage 5: create destination tree {}: {error}",
            destination.display()
        )
    })?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("stage 5: read source tree {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("stage 5: read source tree entry: {error}"))?;
        let source_path = entry.path();
        if should_exclude(&source_path, mod_root, emitted) {
            continue;
        }

        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "stage 5: inspect source tree entry {}: {error}",
                source_path.display()
            )
        })?;
        if file_type.is_dir() {
            copy_filtered_tree(&source_path, &destination_path, mod_root, emitted)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "stage 5: copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            return Err(format!(
                "stage 5: refuse non-regular source tree entry {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

/// Copy completed material-mip files. A missing source directory is an empty success.
pub(super) fn copy_prm_tree(source: &Path, destination: &Path) -> Result<usize, String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "stage 7: create materials directory {}: {error}",
            destination.display()
        )
    })?;

    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "stage 7: read materials {}: {error}",
                source.display()
            ));
        }
    };

    let mut copied = 0;
    for entry in entries {
        let entry = entry.map_err(|error| format!("stage 7: read material entry: {error}"))?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "stage 7: inspect material entry {}: {error}",
                entry.path().display()
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if file_type.is_file() && is_prm_filename(name) {
            fs::copy(entry.path(), destination.join(name)).map_err(|error| {
                format!("stage 7: copy material {}: {error}", entry.path().display())
            })?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Verify that the finished payload contains only released runtime artifacts.
pub(super) fn sweep_payload(
    payload_root: &Path,
    mod_root: &Path,
    emitted: EntryExt,
    resolved: &[Resolved],
) -> Result<(), String> {
    let payload_mod_root = payload_root.join(mod_root);
    verify_entry_script_set(&payload_mod_root, emitted)?;

    let mut prls = BTreeSet::new();
    let materials_root = payload_root.join("baked").join("materials");
    sweep_tree(payload_root, payload_root, &materials_root, &mut prls)?;

    let expected_prls = resolved
        .iter()
        .map(|resolved| mod_root.join(&resolved.output))
        .collect();
    if prls != expected_prls {
        return Err(format!(
            "payload sweep: .prl set differs from resolved outputs; found {prls:?}, expected {expected_prls:?}"
        ));
    }
    Ok(())
}

/// Count regular files and their total byte size under a payload root.
pub(super) fn count_payload(root: &Path) -> Result<(u64, u64), String> {
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

fn sweep_tree(
    payload_root: &Path,
    directory: &Path,
    materials_root: &Path,
    prls: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("payload sweep: read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("payload sweep: read directory entry: {error}"))?;
        let path = entry.path();
        let relative = path.strip_prefix(payload_root).map_err(|error| {
            format!(
                "payload sweep: derive payload-relative path for {}: {error}",
                path.display()
            )
        })?;
        if is_sweep_forbidden(relative) {
            return Err(format!(
                "payload sweep: forbidden source artifact {}",
                relative.display()
            ));
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("payload sweep: inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            sweep_tree(payload_root, &path, materials_root, prls)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(format!(
                "payload sweep: refuse non-regular payload entry {}",
                path.display()
            ));
        }
        if extension(relative) == Some("prl") {
            prls.insert(relative.to_path_buf());
        }
        if path.starts_with(materials_root)
            && !is_prm_filename(file_name(&path).unwrap_or_default())
        {
            return Err(format!(
                "payload sweep: invalid material mip filename {}",
                relative.display()
            ));
        }
    }
    Ok(())
}

fn verify_entry_script_set(mod_root: &Path, emitted: EntryExt) -> Result<(), String> {
    let js = mod_root.join(EntryExt::Js.file_name());
    let luau = mod_root.join(EntryExt::Luau.file_name());
    let exact_set = match emitted {
        EntryExt::Js => js.is_file() && !luau.exists(),
        EntryExt::Luau => luau.is_file() && !js.exists(),
    };
    if exact_set {
        Ok(())
    } else {
        Err(format!(
            "payload sweep: mod root {} must contain only {}",
            mod_root.display(),
            emitted.file_name()
        ))
    }
}

fn is_sweep_forbidden(path: &Path) -> bool {
    has_component_pair(path, "maps", "autosave")
        || matches!(file_name(path), Some(".DS_Store"))
        || matches!(extension(path), Some("map" | "ts" | "md" | "bsp"))
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(OsStr::to_str)
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(OsStr::to_str)
}

fn has_component(path: &Path, expected: &str) -> bool {
    path.components().any(|component| match component {
        Component::Normal(value) => value == OsStr::new(expected),
        _ => false,
    })
}

fn has_component_pair(path: &Path, first: &str, second: &str) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    components
        .windows(2)
        .any(|pair| pair[0] == first && pair[1] == second)
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{
        EntryExt, MARKER_NAME, copy_prm_tree, replace_existing_payload_with, should_exclude,
    };

    fn mod_root() -> PathBuf {
        PathBuf::from("content/dev")
    }

    #[test]
    fn should_exclude_rejects_source_and_stale_output_extensions() {
        let root = mod_root();
        for extension in ["map", "ts", "md", "prl", "js", "bsp"] {
            let path = root.join("maps").join(format!("example.{extension}"));
            assert!(should_exclude(&path, &root, EntryExt::Js), "{path:?}");
        }
    }

    #[test]
    fn should_exclude_rejects_autosave_and_metadata_entries() {
        let root = mod_root();
        assert!(should_exclude(
            &root.join("nested/maps/autosave/preview.png"),
            &root,
            EntryExt::Js
        ));
        for name in [".build-caches", ".gitignore", ".gitkeep", ".DS_Store"] {
            assert!(
                should_exclude(&root.join(name), &root, EntryExt::Js),
                "{name}"
            );
        }
    }

    #[test]
    fn should_exclude_scopes_luau_to_the_mod_entry_branch_and_scripts_directory() {
        let root = mod_root();
        assert!(should_exclude(
            &root.join("start-script.luau"),
            &root,
            EntryExt::Js
        ));
        assert!(!should_exclude(
            &root.join("start-script.luau"),
            &root,
            EntryExt::Luau
        ));
        assert!(should_exclude(
            &root.join("scripts/level.luau"),
            &root,
            EntryExt::Js
        ));
        assert!(should_exclude(
            &root.join("scripts/level.luau"),
            &root,
            EntryExt::Luau
        ));
    }

    #[test]
    fn should_exclude_preserves_runtime_asset_extensions() {
        let root = mod_root();
        for extension in [
            "png", "gltf", "glb", "bin", "wav", "json", "jpg", "txt", "ttf",
        ] {
            let path = root.join("assets").join(format!("example.{extension}"));
            assert!(!should_exclude(&path, &root, EntryExt::Js), "{path:?}");
        }
    }

    #[test]
    fn copy_prm_tree_creates_empty_destination_for_absent_source() {
        let root = unique_temp_dir();
        let source = root.join("missing-materials");
        let destination = root.join("payload/baked/materials");

        let copied = copy_prm_tree(&source, &destination).expect("absent source succeeds");

        assert_eq!(copied, 0);
        assert!(destination.is_dir());
        assert_eq!(
            fs::read_dir(&destination)
                .expect("destination readable")
                .count(),
            0
        );
        remove_temp_dir(&root);
    }

    #[test]
    fn marker_failure_leaves_partial_payload_aside() {
        let root = unique_temp_dir();
        let payload_root = root.join("postretro-dev");
        let aside = root.join(".postretro-dev.deleting-test");
        fs::create_dir_all(&payload_root).expect("payload root created");
        fs::write(payload_root.join("retained.txt"), "retained")
            .expect("retained payload file written");
        fs::write(payload_root.join("removed.txt"), "removed")
            .expect("removed payload file written");

        let result = replace_existing_payload_with(
            &payload_root,
            &aside,
            |partial| {
                fs::remove_file(partial.join("removed.txt"))?;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated partial removal",
                ))
            },
            |_| Err("simulated marker failure".to_string()),
        );

        let error = result.expect_err("marker failure rejects replacement");
        assert!(
            !payload_root.exists(),
            "partial payload must not be restored"
        );
        assert_eq!(
            fs::read_to_string(aside.join("retained.txt")).expect("retained payload readable"),
            "retained"
        );
        assert!(!aside.join("removed.txt").exists());
        assert!(!aside.join(MARKER_NAME).exists());
        assert!(error.contains(&aside.display().to_string()));
        assert!(error.contains("simulated marker failure"));
        remove_temp_dir(&root);
    }

    fn remove_temp_dir(root: &Path) {
        for attempt in 0..3 {
            match fs::remove_dir_all(root) {
                Ok(()) => return,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty && attempt < 2 => {}
                Err(error) => panic!("temporary tree {} removed: {error}", root.display()),
            }
        }
        unreachable!("last cleanup attempt returns or panics");
    }

    fn unique_temp_dir() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_dist_payload_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("temporary tree created");
        path
    }

    #[test]
    fn should_exclude_does_not_treat_base_content_luau_as_mod_source() {
        let root = mod_root();
        assert!(!should_exclude(
            Path::new("content/base/scripts/splash.luau"),
            &root,
            EntryExt::Js
        ));
    }
}
