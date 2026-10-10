// Drift guard: script-compiler's build-time `getGameState()` mirror vs. the catalog and runtime bridge.
// See: context/lib/scripting.md §5 · context/lib/testing_guide.md (Drift guards derive from the source)

use std::collections::BTreeMap;

use postretro_entities::engine_state_catalog::engine_state_catalog;
use postretro_script_compiler::light_membership::{
    JS_INSTALL_BY_PLAYER, LUAU_INSTALL_BY_PLAYER, MIRRORED_GAME_STATE_LEAVES,
};
use postretro_scripting_core::game_state_refs::{
    LUAU_INSTALL_BY_PLAYER as RUNTIME_LUAU_INSTALL_BY_PLAYER,
    QUICKJS_INSTALL_BY_PLAYER as RUNTIME_QUICKJS_INSTALL_BY_PLAYER, state_ref_kind,
};

/// SDK path → (slot, kind, per-player), the shape both trees serve per leaf.
type LeafMap = BTreeMap<String, (String, &'static str, bool)>;

#[test]
fn build_time_game_state_mirror_matches_engine_state_catalog() {
    // Expectation comes from the catalog through the runtime bridge's own
    // kind mapping, so a new catalog entry or value type fails here.
    let catalog = engine_state_catalog().expect("engine-state catalog is valid");
    let expected: LeafMap = catalog
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.sdk_path.join("."),
                (
                    entry.wire_name.to_string(),
                    state_ref_kind(entry.value_type),
                    entry.is_per_player(),
                ),
            )
        })
        .collect();

    // The build-time bridge derives each leaf's SDK path by splitting its slot.
    let mirrored: LeafMap = MIRRORED_GAME_STATE_LEAVES
        .iter()
        .map(|&(slot, kind, per_player)| (slot.to_string(), (slot.to_string(), kind, per_player)))
        .collect();
    assert_eq!(
        mirrored.len(),
        MIRRORED_GAME_STATE_LEAVES.len(),
        "MIRRORED_GAME_STATE_LEAVES lists a slot more than once"
    );

    assert_eq!(
        mirrored, expected,
        "script-compiler's MIRRORED_GAME_STATE_LEAVES (light_membership.rs) drifted from \
         engine_state_catalog(); update the mirror to match the catalog"
    );
}

#[test]
fn build_time_by_player_installers_match_runtime_bridge() {
    assert_eq!(
        JS_INSTALL_BY_PLAYER, RUNTIME_QUICKJS_INSTALL_BY_PLAYER,
        "light_membership.rs JS_INSTALL_BY_PLAYER drifted from game_state_refs.rs \
         QUICKJS_INSTALL_BY_PLAYER"
    );
    assert_eq!(
        LUAU_INSTALL_BY_PLAYER, RUNTIME_LUAU_INSTALL_BY_PLAYER,
        "light_membership.rs LUAU_INSTALL_BY_PLAYER drifted from game_state_refs.rs \
         LUAU_INSTALL_BY_PLAYER"
    );
}
