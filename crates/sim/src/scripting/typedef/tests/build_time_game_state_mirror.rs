// Drift guard: script-compiler's build-time `getGameState()` mirror vs. the catalog and runtime bridge.
// See: context/lib/scripting.md §5 · context/lib/testing_guide.md (Drift guards derive from the source)

use std::collections::{BTreeMap, BTreeSet};

use postretro_entities::engine_state_catalog::engine_state_catalog;
use postretro_foundation::owner_tokens::BY_PLAYER_OWNER_TOKENS;
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
        *JS_INSTALL_BY_PLAYER, *RUNTIME_QUICKJS_INSTALL_BY_PLAYER,
        "light_membership.rs JS_INSTALL_BY_PLAYER drifted from game_state_refs.rs \
         QUICKJS_INSTALL_BY_PLAYER"
    );
    assert_eq!(
        *LUAU_INSTALL_BY_PLAYER, *RUNTIME_LUAU_INSTALL_BY_PLAYER,
        "light_membership.rs LUAU_INSTALL_BY_PLAYER drifted from game_state_refs.rs \
         LUAU_INSTALL_BY_PLAYER"
    );
}

/// The quoted `@`-tokens a source line or block names, `@invalid` (the
/// rejection spelling) excluded.
fn owner_tokens_in(source: &str) -> BTreeSet<String> {
    source
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|quoted| quoted.starts_with('@') && *quoted != "@invalid")
        .map(str::to_string)
        .collect()
}

/// The one line of an SDK file declaring its `byPlayer` token list.
fn sdk_token_line(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/lib")
        .join(file);
    let source = std::fs::read_to_string(&path).expect("SDK source reads");
    let mut lines = source.lines().filter(|line| {
        line.contains("BY_PLAYER_OWNER_TOKENS =") || line.contains("BY_PLAYER_OWNER_TOKENS:")
    });
    let line = lines
        .next()
        .unwrap_or_else(|| panic!("{file} must declare BY_PLAYER_OWNER_TOKENS on one line"))
        .to_string();
    assert!(
        lines.next().is_none(),
        "{file} declares BY_PLAYER_OWNER_TOKENS once"
    );
    line
}

/// Every `byPlayer` recognizing site accepts exactly `BY_PLAYER_OWNER_TOKENS`:
/// both runtime installers (generated from it) and both hand-written SDK
/// `storeRef` sites, which read the handle's `__wire` against their list.
#[test]
fn by_player_sites_accept_exactly_the_owner_token_constant() {
    let expected: BTreeSet<String> = BY_PLAYER_OWNER_TOKENS
        .iter()
        .map(|token| token.to_string())
        .collect();
    for (site, source) in [
        (
            "QUICKJS_INSTALL_BY_PLAYER",
            RUNTIME_QUICKJS_INSTALL_BY_PLAYER.as_str(),
        ),
        (
            "LUAU_INSTALL_BY_PLAYER",
            RUNTIME_LUAU_INSTALL_BY_PLAYER.as_str(),
        ),
    ] {
        assert_eq!(owner_tokens_in(source), expected, "{site}");
    }
    for (file, lookup) in [
        ("data_script.ts", "BY_PLAYER_OWNER_TOKENS.includes(wire)"),
        ("data_script.luau", "BY_PLAYER_OWNER_TOKENS[wire]"),
    ] {
        assert_eq!(
            owner_tokens_in(&sdk_token_line(file)),
            expected,
            "sdk/lib/{file} storeRef drifted from BY_PLAYER_OWNER_TOKENS"
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk/lib")
            .join(file);
        let source = std::fs::read_to_string(path).unwrap();
        assert!(
            source.contains(lookup),
            "sdk/lib/{file} storeRef must check `__wire` against its token list"
        );
    }
}
