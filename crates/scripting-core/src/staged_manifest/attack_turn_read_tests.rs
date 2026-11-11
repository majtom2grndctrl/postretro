// A brain guard's `attackTurnHeld(attack)` / `attackTurnFree(attack)` names one
// of its archetype's attacks, checked at load in both runtimes.
// See: context/lib/scripting.md §11 (Attack turn facts)

use std::fs;

use super::tests::temp_mod_root;
use super::*;

fn build(runtime: &str, entry: &str, source: &str) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("attack_turn_reads_{runtime}"));
    fs::write(dir.join(entry), source).expect("fixture should be written");
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn ts_mod(read: &str) -> String {
    format!(
        r#"globalThis.__postretroModManifest = defineMod({{ name: "M", id: "m", version: "1",
             entities: [{{ canonicalName: "grunt", components: {{ behavior: {{
               initial: "idle", moveSpeed: 3,
               attacks: {{ slam: {{ damage: 8, maxRange: 2, cooldownMs: 1200 }} }},
               activities: {{
                 idle: {{ animation: "idle", motion: "hold" }},
                 engage: {{ animation: "walk", motion: "chaseTarget", action: {{ attack: "slam" }} }}
               }},
               transitions: {{ idle: [{{ when: {{ op: "input", name: "{read}" }}, to: "engage" }}] }}
             }} }} }}] }});"#
    )
}

fn luau_mod(read: &str) -> String {
    format!(
        r#"return defineMod({{ name = "M", id = "m", version = "1",
             entities = {{ {{ canonicalName = "grunt", components = {{ behavior = {{
               initial = "idle", moveSpeed = 3,
               attacks = {{ slam = {{ damage = 8, maxRange = 2, cooldownMs = 1200 }} }},
               activities = {{
                 idle = {{ animation = "idle", motion = "hold" }},
                 engage = {{ animation = "walk", motion = "chaseTarget", action = {{ attack = "slam" }} }}
               }},
               transitions = {{ idle = {{ {{ when = {read}, to = "engage" }} }} }}
             }} }} }} }} }})"#
    )
}

#[test]
fn attack_turn_reads_of_an_archetype_attack_load_in_both_runtimes() {
    for read in ["@attackTurnHeld.slam", "@attackTurnFree.slam"] {
        let result = build("js_ok", "start-script.js", &ts_mod(read));
        assert!(
            matches!(result.status, StagedManifestBuildStatus::Built(_)),
            "{read}: {:?}",
            result.diagnostics
        );
    }
    // The Luau SDK helpers build the same reads.
    for read in [r#"attackTurnHeld("slam")"#, r#"attackTurnFree("slam")"#] {
        let result = build("luau_ok", "start-script.luau", &luau_mod(read));
        assert!(
            matches!(result.status, StagedManifestBuildStatus::Built(_)),
            "{read}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn attack_turn_reads_naming_no_archetype_attack_fail_load_in_both_runtimes() {
    let path = "entities[0].components.behavior.transitions.idle[0].when";
    let cases = [
        (
            build("js_held", "start-script.js", &ts_mod("@attackTurnHeld.jab")),
            "@attackTurnHeld.jab",
        ),
        (
            build("js_free", "start-script.js", &ts_mod("@attackTurnFree.jab")),
            "@attackTurnFree.jab",
        ),
        (
            build(
                "luau_held",
                "start-script.luau",
                &luau_mod(r#"attackTurnHeld("jab")"#),
            ),
            "@attackTurnHeld.jab",
        ),
        (
            build(
                "luau_free",
                "start-script.luau",
                &luau_mod(r#"attackTurnFree("jab")"#),
            ),
            "@attackTurnFree.jab",
        ),
    ];
    for (result, read) in cases {
        assert_eq!(result.status, StagedManifestBuildStatus::Failed, "{read}");
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains(read) && diagnostic.message.contains(path)
            }),
            "{read}: {:?}",
            result.diagnostics
        );
    }
}
