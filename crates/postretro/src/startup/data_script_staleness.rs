// Stale level-script detection at level load. A `.prl` embeds a compiled copy
// of its data script at bake time and the engine runs only that copy, so a map
// baked before a script edit silently keeps the old behavior — or, after an SDK
// break, throws in `setupLevel` and installs no reactions at all.
// See: context/lib/scripting.md §2 (Data context lifecycle)

use std::path::Path;

use postretro_level_format::data_script::DataScriptSection;

/// Log an error when the level script the map was baked from is newer on disk
/// than the map itself. Silent when the source is absent — a shipped map on a
/// player's machine never has it — or when either timestamp is unreadable.
///
/// Compares the entry script only: an edit confined to an imported module or
/// the SDK does not trip it.
pub(crate) fn report_stale_data_script(map_path: &Path, section: Option<&DataScriptSection>) {
    let Some(section) = section else {
        return;
    };
    let source_path = Path::new(&section.source_path);
    if source_newer_than_map(map_path, source_path) {
        log::error!(
            "[Loader] `{}` was baked before the last edit to its level script `{}` and runs \
             the copy embedded at bake time; rebuild the map with prl-build to pick up the change",
            map_path.display(),
            source_path.display(),
        );
    }
}

fn source_newer_than_map(map_path: &Path, source_path: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(map_path), modified(source_path)) {
        (Some(map), Some(source)) => source > map,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    fn write_with_mtime(path: &Path, mtime: SystemTime) {
        let file = File::create(path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    #[test]
    fn source_edited_after_bake_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let (map, source) = (dir.path().join("level.prl"), dir.path().join("level.ts"));
        let baked = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&map, baked);
        write_with_mtime(&source, baked + Duration::from_secs(60));
        assert!(source_newer_than_map(&map, &source));
    }

    #[test]
    fn map_baked_after_source_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (map, source) = (dir.path().join("level.prl"), dir.path().join("level.ts"));
        let edited = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&source, edited);
        write_with_mtime(&map, edited + Duration::from_secs(60));
        assert!(!source_newer_than_map(&map, &source));
    }

    #[test]
    fn missing_source_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let map = dir.path().join("level.prl");
        write_with_mtime(&map, SystemTime::now());
        assert!(!source_newer_than_map(&map, &dir.path().join("absent.ts")));
    }
}
