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
        "luau_prelude.rs",
        "primitives/entity.rs",
        "primitives/light.rs",
        "primitives/mod.rs",
        "primitives/store.rs",
        "primitives/world.rs",
        "reactions/mod.rs",
        "reactions/registry.rs",
        "reactions/system_commands.rs",
        "state_crossings.rs",
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
