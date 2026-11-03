// The discrete `assistReceived()` / `assistGiven()` player-event sources in
// both SDK runtimes: each lowers to a condition-free `playerEvents` entry.
// See: context/lib/scripting.md §12 (Player events, Assist)
use super::*;

/// `players().on(assistReceived(), …)` and `players().on(assistGiven(), …)`
/// lower byte-identically in TypeScript and Luau: the source word as `edge`,
/// the fire names and `levels`, and no `condition` key.
#[test]
fn assist_sources_lower_identically_across_runtimes_without_a_condition() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { assistGiven, assistReceived, players } from "postretro";
        JSON.stringify([
          players().on(assistReceived(), ["revive", "cue"]),
          players().on(assistGiven(), ["thanks"], { levels: ["arena"] }),
        ]);
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        return {
          Postretro.players():on(Postretro.assistReceived(), { "revive", "cue" }),
          Postretro.players():on(Postretro.assistGiven(), { "thanks" }, { levels = { "arena" } }),
        }
    "#;
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        serde_json::to_string(&typescript).unwrap(),
        serde_json::to_string(&luau).unwrap(),
        "both sources must lower byte-identically"
    );
    assert_eq!(
        typescript,
        serde_json::json!([
            { "edge": "assistReceived", "fire": ["revive", "cue"] },
            { "edge": "assistGiven", "fire": ["thanks"], "levels": ["arena"] },
        ])
    );
}

/// The Luau globals lift the sources too, so a script without `require`
/// builds the same entry.
#[test]
fn assist_sources_are_lifted_luau_globals() {
    let luau = luau_fixture_value(r#"return players():on(assistGiven(), { "thanks" })"#);
    assert_eq!(
        luau,
        serde_json::json!({ "edge": "assistGiven", "fire": ["thanks"] })
    );
}
