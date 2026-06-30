// Scripting subsystem: Rust-owned entity/component APIs, engine-global typed state, and persistence.
// See: context/lib/scripting.md for governing scripting contracts and ownership.

// Renderer, audio, and input own their own data structures and are unaffected
// by anything in this module.

#![deny(unsafe_code)]
// Some component types not yet wired to their bridges — silence dead-code at
// the subsystem level rather than scattering item-level annotations.
#![allow(dead_code)]

pub(crate) mod builtins;
pub(crate) mod entity_world_primitives;
pub(crate) mod map_entity;
pub(crate) mod primitives;
pub(crate) mod reactions;
pub(crate) mod state_persistence;
pub(crate) mod state_store;
pub(crate) mod typedef;

#[cfg(test)]
mod extraction_path_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    const MOVED_PATH_SENTINELS: &[&str] = &[
        "conv.rs",
        "data_descriptors",
        "game_state_refs.rs",
        "ir/e2e_tests.rs",
        "ir/parity_tests.rs",
        "ir/scopes.rs",
        "ir/test_scope.rs",
        "luau.rs",
        "luau_prelude.rs",
        "luau_require.rs",
        "luau_virtual_modules.rs",
        "primitive_adapters.rs",
        "primitives/entity.rs",
        "primitives/light.rs",
        "primitives/mod.rs",
        "primitives/store.rs",
        "primitives/world.rs",
        "primitives_registry.rs",
        "quickjs.rs",
        "reaction_dispatch.rs",
        "reaction_registry.rs",
        "reactions/mod.rs",
        "reactions/registry.rs",
        "reactions/system_commands.rs",
        "refresh_plan.rs",
        "runtime",
        "sequence.rs",
        "staged_manifest.rs",
        "state_crossings.rs",
        "store_bridge.rs",
        "typedef/common.rs",
        "typedef/luau.rs",
        "typedef/mod.rs",
        "typedef/tests",
        "typedef/ts.rs",
        "ui",
        "watcher.rs",
    ];

    const INTENTIONAL_COMPATIBILITY_OR_FIXTURE_PATHS: &[&str] = &[
        "primitives/entity.rs",
        "primitives/light.rs",
        "primitives/mod.rs",
        "primitives/store.rs",
        "primitives/world.rs",
        "reactions/mod.rs",
        "reactions/registry.rs",
        "reactions/system_commands.rs",
        "typedef/mod.rs",
        "typedef/tests",
    ];

    #[test]
    fn scripting_core_extraction_deleted_paths_do_not_reappear() {
        let scripting_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scripting");
        let unexpected = MOVED_PATH_SENTINELS
            .iter()
            .copied()
            .filter(|path| {
                !INTENTIONAL_COMPATIBILITY_OR_FIXTURE_PATHS.contains(path)
                    && implementation_path_exists(&scripting_dir.join(path))
            })
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        assert!(
            unexpected.is_empty(),
            "scripting-core implementation paths reappeared under crates/postretro/src/scripting: {unexpected:?}. Keep VM substrate in crates/scripting-core; only the allowlisted compatibility barrels and typedef fixtures may remain here."
        );
    }

    fn implementation_path_exists(path: &Path) -> bool {
        if path.is_file() {
            return true;
        }
        path.is_dir() && contains_file(path)
    }

    fn contains_file(dir: &Path) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };

        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            path.is_file() || (path.is_dir() && contains_file(&path))
        })
    }
}

#[cfg(test)]
mod scripting_boundary_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    const REMOVED_OR_COLLAPSED_BARRELS: &[&str] = &[
        "data_registry",
        "foundation_pods",
        "game_state_refs",
        "ir",
        "ir_scopes",
        "luau_require",
        "luau_virtual_modules",
        "refresh_plan",
        "luau",
        "quickjs",
        "watcher",
        "value_types",
        "registry",
        "components",
        "ctx",
        "slot_table",
        "provenance",
        "error",
        "engine_state_catalog",
        "conv",
        "reaction_dispatch",
        "runtime",
        "sequence",
        "staged_manifest",
        "state_crossings",
        "luau_prelude",
        "primitives_registry",
        "data_descriptors",
        "typedef",
    ];

    #[test]
    fn scripting_boundary_rejects_removed_barrel_imports() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();

        collect_boundary_violations(&src_dir, &src_dir, &mut violations);

        assert!(
            violations.is_empty(),
            "removed/collapsed scripting barrels were referenced through crate::scripting::<name> or crate::scripting::{{<name>...}}: {violations:#?}. Import floor/core APIs from postretro_foundation, postretro_entities, or postretro_scripting_core directly."
        );
    }

    fn collect_boundary_violations(src_dir: &Path, path: &Path, violations: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect_boundary_violations(src_dir, &path, violations);
            } else if is_rust_source(&path) && !is_allowlisted(src_dir, &path) {
                scan_rust_source(src_dir, &path, violations);
            }
        }
    }

    fn scan_rust_source(src_dir: &Path, path: &Path, violations: &mut Vec<String>) {
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };

        for (line_index, line) in source.lines().enumerate() {
            for barrel in REMOVED_OR_COLLAPSED_BARRELS {
                let pattern = format!("crate::scripting::{barrel}");
                if line.contains(&pattern) {
                    violations.push(format!(
                        "{}:{} contains {pattern}",
                        display_relative_path(src_dir, path).display(),
                        line_index + 1
                    ));
                }
            }
        }

        let grouped_pattern = "crate::scripting::{";
        for (match_index, _) in source.match_indices(grouped_pattern) {
            let group_start = match_index + grouped_pattern.len();
            let Some(group_end) = grouped_use_tree_end(&source[group_start..]) else {
                continue;
            };
            let group = &source[group_start..group_start + group_end];

            for barrel in REMOVED_OR_COLLAPSED_BARRELS {
                if grouped_use_tree_contains_barrel(group, barrel) {
                    violations.push(format!(
                        "{}:{} contains crate::scripting::{{...{barrel}...}}",
                        display_relative_path(src_dir, path).display(),
                        line_number_at(&source, match_index)
                    ));
                }
            }
        }
    }

    fn grouped_use_tree_end(source_after_open_brace: &str) -> Option<usize> {
        let mut depth = 0usize;

        for (index, byte) in source_after_open_brace.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' if depth == 0 => return Some(index),
                b'}' => depth -= 1,
                _ => {}
            }
        }

        None
    }

    fn grouped_use_tree_contains_barrel(group: &str, barrel: &str) -> bool {
        let mut depth = 0usize;
        let mut item_start = 0usize;

        for (index, byte) in group.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    if grouped_use_item_starts_with_barrel(&group[item_start..index], barrel) {
                        return true;
                    }
                    item_start = index + 1;
                }
                _ => {}
            }
        }

        grouped_use_item_starts_with_barrel(&group[item_start..], barrel)
    }

    fn grouped_use_item_starts_with_barrel(item: &str, barrel: &str) -> bool {
        let trimmed = item.trim_start();
        let Some(rest) = trimmed.strip_prefix(barrel) else {
            return false;
        };

        rest.is_empty()
            || rest.starts_with("::")
            || rest.chars().next().is_some_and(char::is_whitespace)
    }

    fn line_number_at(source: &str, byte_index: usize) -> usize {
        source[..byte_index]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1
    }

    fn is_rust_source(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "rs")
    }

    fn is_allowlisted(src_dir: &Path, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(src_dir) else {
            return false;
        };

        // This source scan catches direct `crate::scripting::<name>` paths and
        // top-level grouped `crate::scripting::{<name>...}` imports. It still
        // does not parse aliases to `scripting` or `super::` relative forms.
        // After barrel deletion those forms are compile errors, so this is a
        // reintroduction lock rather than a full parser.
        relative == Path::new("scripting/mod.rs")
            || relative.starts_with(Path::new("scripting/typedef"))
    }

    fn display_relative_path(src_dir: &Path, path: &Path) -> PathBuf {
        path.strip_prefix(src_dir)
            .map(PathBuf::from)
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
