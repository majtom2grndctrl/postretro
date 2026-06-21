// Hot-reload dependency-set classification and path normalization.

use super::*;

#[cfg(debug_assertions)]
use super::super::types::{ActiveModInitDependencies, normalize_changed_path};

#[test]
#[cfg(debug_assertions)]
fn active_dependencies_trigger_only_member_paths() {
    let dir = temp_mod_root("dependency_membership");
    let entry = dir.join("start-script.ts");
    let active = dir.join("actors/player.ts");
    let output = dir.join("start-script.js");
    let unrelated_ts = dir.join("actors/unrelated.ts");
    let unrelated_js = dir.join("actors/unrelated.js");
    let unrelated_luau = dir.join("actors/unrelated.luau");
    fs::create_dir_all(dir.join("actors")).unwrap();
    for path in [
        &entry,
        &active,
        &output,
        &unrelated_ts,
        &unrelated_js,
        &unrelated_luau,
    ] {
        fs::write(path, "").unwrap();
    }

    let deps =
        ActiveModInitDependencies::from_dependencies(&dir, [entry.clone(), active.clone()].iter())
            .unwrap();

    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(deps.changed_paths_affect_mod_init(&[active]));
    assert!(
        !deps.changed_paths_affect_mod_init(&[output]),
        "generated start-script.js is compiler output when TS is active"
    );
    assert!(!deps.changed_paths_affect_mod_init(&[unrelated_ts]));
    assert!(!deps.changed_paths_affect_mod_init(&[unrelated_js]));
    assert!(!deps.changed_paths_affect_mod_init(&[unrelated_luau]));
}

#[test]
#[cfg(debug_assertions)]
fn javascript_only_dependency_set_is_entry_only() {
    let dir = temp_mod_root("js_entry_only");
    let entry = dir.join("start-script.js");
    let helper = dir.join("helper.js");
    fs::write(&entry, "").unwrap();
    fs::write(&helper, "").unwrap();

    let deps = ActiveModInitDependencies::from_dependencies(&dir, [entry.clone()].iter()).unwrap();

    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(
        !deps.changed_paths_affect_mod_init(&[helper]),
        "JS dependency discovery is intentionally entry-only"
    );
}

#[test]
#[cfg(debug_assertions)]
fn luau_dependency_set_matches_entry_and_required_files() {
    let dir = temp_mod_root("luau_deps");
    let entry = dir.join("start-script.luau");
    let required = dir.join("actors/player.luau");
    let unrelated = dir.join("actors/unrelated.luau");
    fs::create_dir_all(dir.join("actors")).unwrap();
    fs::write(&entry, "").unwrap();
    fs::write(&required, "").unwrap();
    fs::write(&unrelated, "").unwrap();

    let deps = ActiveModInitDependencies::from_dependencies(
        &dir,
        [entry.clone(), required.clone()].iter(),
    )
    .unwrap();

    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(deps.changed_paths_affect_mod_init(&[required]));
    assert!(!deps.changed_paths_affect_mod_init(&[unrelated]));
}

// Regression: a start script importing a file above the mod root (e.g.
// `../../sdk/behaviors/...`) made from_dependencies fail the whole set, leaving
// the active dependency set empty so no in-root edit ever triggered hot reload.
#[test]
#[cfg(debug_assertions)]
fn out_of_root_import_does_not_disable_in_root_hot_reload() {
    // Mod root `<tmp>/mod/content/dev` with a sibling `sdk/behaviors` above it,
    // mirroring `content/dev/start-script.ts` importing `../../sdk/behaviors/...`.
    let base = temp_mod_root("partition_e2e");
    let mod_root = base.join("content/dev");
    let in_root_import = mod_root.join("scripts/reference-pistol.ts");
    let entry = mod_root.join("start-script.ts");
    let unrelated = mod_root.join("scripts/unrelated.ts");
    let out_of_root = base.join("sdk/behaviors/reference/entities.ts");
    for path in [&entry, &in_root_import, &unrelated, &out_of_root] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    let deps = ActiveModInitDependencies::from_dependencies(
        &mod_root,
        [entry.clone(), in_root_import.clone(), out_of_root.clone()].iter(),
    )
    .unwrap();

    assert_eq!(
        deps.len(),
        2,
        "in-root entry and import are committed; out-of-root dep is dropped"
    );
    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(
        deps.changed_paths_affect_mod_init(&[in_root_import]),
        "editing the in-root imported script must trigger hot reload"
    );
    assert!(
        !deps.changed_paths_affect_mod_init(&[out_of_root]),
        "the dropped out-of-root dependency must not trigger hot reload"
    );
    assert!(
        !deps.changed_paths_affect_mod_init(&[unrelated]),
        "an in-root file that is not a dependency must not trigger hot reload"
    );
}

#[test]
#[cfg(debug_assertions)]
fn rename_classification_checks_old_and_new_paths() {
    let dir = temp_mod_root("rename_membership");
    let active = dir.join("actors/player.luau");
    let renamed = dir.join("actors/player-renamed.luau");
    fs::create_dir_all(dir.join("actors")).unwrap();
    fs::write(&active, "").unwrap();
    let deps = ActiveModInitDependencies::from_dependencies(&dir, [active.clone()].iter()).unwrap();

    fs::rename(&active, &renamed).unwrap();

    assert!(
        deps.changed_paths_affect_mod_init(&[active, renamed]),
        "rename should trigger when either the old or new path is active"
    );
}

#[test]
#[cfg(debug_assertions)]
fn atomic_rename_classification_covers_entries_dependencies_and_unrelated_files() {
    fn write_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "").unwrap();
    }

    fn rename_affects(deps: &ActiveModInitDependencies, old_path: &Path, new_path: &Path) -> bool {
        fs::rename(old_path, new_path).unwrap();
        deps.changed_paths_affect_mod_init(&[old_path.to_path_buf(), new_path.to_path_buf()])
    }

    let ts_root = temp_mod_root("rename_ts_matrix");
    let ts_entry = ts_root.join("start-script.ts");
    let ts_dep = ts_root.join("actors/player.ts");
    let ts_unrelated = ts_root.join("actors/unrelated.ts");
    for path in [&ts_entry, &ts_dep, &ts_unrelated] {
        write_file(path);
    }
    let ts_deps = ActiveModInitDependencies::from_dependencies(
        &ts_root,
        [ts_entry.clone(), ts_dep.clone()].iter(),
    )
    .unwrap();
    assert!(rename_affects(
        &ts_deps,
        &ts_dep,
        &ts_root.join("actors/player.ts.tmp")
    ));
    assert!(rename_affects(
        &ts_deps,
        &ts_entry,
        &ts_root.join("start-script.ts.tmp")
    ));
    assert!(!rename_affects(
        &ts_deps,
        &ts_unrelated,
        &ts_root.join("actors/unrelated.ts.tmp")
    ));

    let luau_root = temp_mod_root("rename_luau_matrix");
    let luau_entry = luau_root.join("start-script.luau");
    let luau_dep = luau_root.join("actors/player.luau");
    let luau_unrelated = luau_root.join("actors/unrelated.luau");
    for path in [&luau_entry, &luau_dep, &luau_unrelated] {
        write_file(path);
    }
    let luau_deps = ActiveModInitDependencies::from_dependencies(
        &luau_root,
        [luau_entry.clone(), luau_dep.clone()].iter(),
    )
    .unwrap();
    assert!(rename_affects(
        &luau_deps,
        &luau_dep,
        &luau_root.join("actors/player.luau.tmp")
    ));
    assert!(rename_affects(
        &luau_deps,
        &luau_entry,
        &luau_root.join("start-script.luau.tmp")
    ));
    assert!(!rename_affects(
        &luau_deps,
        &luau_unrelated,
        &luau_root.join("actors/unrelated.luau.tmp")
    ));

    let js_root = temp_mod_root("rename_js_matrix");
    let js_entry = js_root.join("start-script.js");
    let js_helper = js_root.join("helper.js");
    for path in [&js_entry, &js_helper] {
        write_file(path);
    }
    let js_deps =
        ActiveModInitDependencies::from_dependencies(&js_root, [js_entry.clone()].iter()).unwrap();
    assert!(rename_affects(
        &js_deps,
        &js_entry,
        &js_root.join("start-script.js.tmp")
    ));
    assert!(!rename_affects(
        &js_deps,
        &js_helper,
        &js_root.join("helper.js.tmp")
    ));
}

#[test]
#[cfg(debug_assertions)]
fn absent_start_script_state_triggers_only_entry_candidates() {
    let dir = temp_mod_root("absent_entry");
    let deps = ActiveModInitDependencies::no_start_script(&dir).unwrap();
    let entry = dir.join("start-script.ts");
    let inactive = dir.join("actors/player.ts");
    fs::create_dir_all(dir.join("actors")).unwrap();
    fs::write(&entry, "").unwrap();
    fs::write(&inactive, "").unwrap();

    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(!deps.changed_paths_affect_mod_init(&[inactive]));
}

#[test]
#[cfg(debug_assertions)]
fn normalize_changed_path_appends_missing_segments_to_canonical_parent() {
    let dir = temp_mod_root("missing_normalization");
    let existing_parent = dir.join("scripts");
    fs::create_dir_all(&existing_parent).unwrap();
    let missing = existing_parent.join("nested").join("start-script.ts");

    let normalized = normalize_changed_path(&missing).unwrap();
    let expected = dir
        .canonicalize()
        .unwrap()
        .join("scripts")
        .join("nested")
        .join("start-script.ts");

    assert_eq!(normalized, expected);
}

#[cfg(unix)]
#[test]
#[cfg(debug_assertions)]
fn out_of_root_dependencies_are_dropped_not_fatal() {
    use std::os::unix::fs::symlink;

    let dir = temp_mod_root("dep_under_root");
    let entry = dir.join("start-script.ts");
    fs::write(&entry, "").unwrap();
    let outside = temp_mod_root("dep_outside_root");
    let outside_file = outside.join("outside.ts");
    fs::write(&outside_file, "").unwrap();
    let linked = dir.join("linked.ts");
    symlink(&outside_file, &linked).unwrap();

    // The symlink resolves above the mod root: it must be dropped, while the
    // in-root entry is still committed and the call succeeds.
    let deps =
        ActiveModInitDependencies::from_dependencies(&dir, [entry.clone(), linked.clone()].iter())
            .unwrap();

    assert!(deps.changed_paths_affect_mod_init(&[entry]));
    assert!(
        !deps.changed_paths_affect_mod_init(&[linked]),
        "out-of-root dependency should not trigger hot reload"
    );
}
