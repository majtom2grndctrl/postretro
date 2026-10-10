// Load-time error for a map baked before the last edit to its level script.
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
    use log::Level;
    use postretro_test_log_capture::LogCapture;
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

    /// Equal timestamps read as fresh: only a strictly newer source is stale.
    #[test]
    fn equal_mtimes_are_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (map, source) = (dir.path().join("level.prl"), dir.path().join("level.ts"));
        let stamp = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&source, stamp);
        write_with_mtime(&map, stamp);
        assert!(!source_newer_than_map(&map, &source));
    }

    /// The section carries the path as `prl-build` records it: canonicalized,
    /// which on Windows is a `\\?\` verbatim path.
    fn section_for(source: &Path) -> DataScriptSection {
        DataScriptSection {
            compiled_bytes: Vec::new(),
            source_path: std::fs::canonicalize(source)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        }
    }

    #[test]
    fn stale_section_logs_one_error_naming_map_and_source() {
        let dir = tempfile::tempdir().unwrap();
        let (map, source) = (dir.path().join("level.prl"), dir.path().join("level.ts"));
        let baked = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&map, baked);
        write_with_mtime(&source, baked + Duration::from_secs(60));
        let section = section_for(&source);

        let capture = LogCapture::start();
        report_stale_data_script(&map, Some(&section));
        capture.assert_logged_once(Level::Error, "level.prl");
        capture.assert_logged_once(Level::Error, "level.ts");
    }

    #[test]
    fn fresh_or_absent_section_logs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (map, source) = (dir.path().join("level.prl"), dir.path().join("level.ts"));
        let edited = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&source, edited);
        write_with_mtime(&map, edited + Duration::from_secs(60));
        let section = section_for(&source);

        let capture = LogCapture::start();
        report_stale_data_script(&map, Some(&section));
        report_stale_data_script(&map, None);
        capture.assert_not_logged(Level::Error, "level.prl");
    }
}
