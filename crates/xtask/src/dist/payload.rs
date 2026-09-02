//! Payload copy filters and the final distribution completion sweep.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::resolve::{EntryExt, Resolved, is_prm_filename};

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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{EntryExt, copy_prm_tree, should_exclude};

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
        fs::remove_dir_all(root).expect("temporary tree removed");
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
