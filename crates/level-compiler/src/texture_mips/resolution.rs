use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn normalize_map_texture_name(name: &str) -> String {
    let lowered = name.to_lowercase().replace('\\', "/");
    lowered
        .strip_prefix("textures/")
        .map(str::to_owned)
        .unwrap_or(lowered)
}

pub(super) fn bare_segment(normalized: &str) -> &str {
    normalized
        .rsplit_once('/')
        .map(|(_, stem)| stem)
        .unwrap_or(normalized)
}

pub(super) fn build_name_to_path_map(texture_root: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let mut stem_owner: HashMap<String, PathBuf> = HashMap::new();
    let mut ambiguous_stems = HashSet::new();
    let collections = match std::fs::read_dir(texture_root) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!(
                "[prl-build] cannot read texture root {}: {err}",
                texture_root.display()
            );
            return map;
        }
    };
    let mut collection_dirs: Vec<PathBuf> = collections
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    collection_dirs.sort();
    for collection_path in collection_dirs {
        let Some(collection_name) = collection_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_lowercase)
        else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(&collection_path) else {
            continue;
        };
        let mut file_paths: Vec<PathBuf> = files.flatten().map(|e| e.path()).collect();
        file_paths.sort();
        for file_path in file_paths {
            if !file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .eq_ignore_ascii_case("png")
            {
                continue;
            }
            let Some(stem) = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_lowercase)
            else {
                continue;
            };
            let rel_key = format!("{collection_name}/{stem}");
            if let Some(existing) = map.get(&rel_key) {
                log::warn!(
                    "[prl-build] duplicate texture path '{rel_key}': found in {} and {}, using first found",
                    existing.display(),
                    file_path.display()
                );
            } else {
                map.insert(rel_key, file_path.clone());
            }
            match stem_owner.get(&stem) {
                Some(first) if !ambiguous_stems.contains(&stem) => {
                    log::warn!(
                        "[prl-build] bare texture name '{stem}' exists in multiple collections ({} and {}); the bare-stem alias is disabled — qualify it as 'collection/{stem}' to resolve",
                        first.display(),
                        file_path.display()
                    );
                    ambiguous_stems.insert(stem.clone());
                }
                Some(_) => {}
                None => {
                    stem_owner.insert(stem.clone(), file_path.clone());
                }
            }
        }
    }
    for (stem, path) in stem_owner {
        if !ambiguous_stems.contains(&stem) {
            map.entry(stem).or_insert(path);
        }
    }
    map
}
