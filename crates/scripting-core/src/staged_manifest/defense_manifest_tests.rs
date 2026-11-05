// Declaring and registering a defense layer through the SDK in both runtimes,
// and the load failures a bad layer reference raises.
// See: context/lib/scripting.md §2 (Damage type and defense layer declarations)

use std::fs;

use postretro_foundation::defense::HandleRef;

use super::tests::temp_mod_root;
use super::*;

const TS_SHIELD_MOD: &str = r#"
    const shield = defineDefenseLayer("shield");
    globalThis.__postretroModManifest = defineMod({
        name: "Shields", id: "shields", version: "1",
        defenseLayers: [shield],
        entities: [{ canonicalName: "player", components: { health: {
            max: 100,
            layers: [{ layer: shield, capacity: 50, rechargeDelayMs: 2500, rechargeRate: 20 }],
        } } }],
    });
"#;

const LUAU_SHIELD_MOD: &str = r#"
    local shield = defineDefenseLayer("shield")
    return defineMod({
        name = "Shields", id = "shields", version = "1",
        defenseLayers = { shield },
        entities = {{ canonicalName = "player", components = { health = {
            max = 100,
            layers = {{ layer = shield, capacity = 50, rechargeDelayMs = 2500, rechargeRate = 20 }},
        } } }},
    })
"#;

fn build(runtime: &str, entry: &str, source: &str) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("defense_{runtime}"));
    fs::write(dir.join(entry), source).expect("fixture should be written");
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn built(result: StagedManifestBuildResult) -> Box<transfer::StagedManifest> {
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("manifest must build: {other:?} {:?}", result.diagnostics),
    }
}

#[test]
fn ts_and_luau_register_a_layer_and_produce_identical_descriptors() {
    let js = built(build("js", "start-script.js", TS_SHIELD_MOD));
    let luau = built(build("luau", "start-script.luau", LUAU_SHIELD_MOD));
    assert_eq!(js.defense.layers, ["shield"]);
    assert_eq!(js.defense, luau.defense);
    let js_health = js.entities[0].health.clone().unwrap();
    let luau_health = luau.entities[0].health.clone().unwrap();
    assert_eq!(
        js_health, luau_health,
        "both runtimes produce one descriptor"
    );
    // The handle crosses as exactly its kind and name.
    assert_eq!(
        js_health.layers[0].layer.0,
        serde_json::json!({ "kind": "defenseLayer", "name": "shield" })
    );
    assert_eq!(
        js_health.layers[0].layer.handle("layer").unwrap(),
        HandleRef::DefenseLayer {
            name: "shield".to_string()
        }
    );
}

#[test]
fn bad_layer_references_fail_load_naming_the_key_path() {
    let cases = [
        (
            "unregistered",
            r#"const shield = defineDefenseLayer("shield");
               const armor = defineDefenseLayer("armor");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, layers: [{ layer: armor, capacity: 5 }] } } }] });"#,
            "entities[0].components.health.layers[0].layer",
        ),
        (
            "bare-string",
            r#"const shield = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, layers: [{ layer: "shield", capacity: 5 }] } } }] });"#,
            "components.health.layers[0].layer",
        ),
        (
            "type-as-layer",
            r#"const shock = defineDamageType("shock", {});
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 damageTypes: [shock],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, layers: [{ layer: shock, capacity: 5 }] } } }] });"#,
            "expected a defense layer handle",
        ),
        (
            "listed-twice",
            r#"const shield = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, layers: [
                   { layer: shield, capacity: 5 }, { layer: shield, capacity: 5 }] } } }] });"#,
            "listed twice",
        ),
        (
            "out-of-range",
            r#"const shield = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, layers: [{ layer: shield, capacity: 2e6 }] } } }] });"#,
            "components.health.layers[0].capacity",
        ),
        (
            "unknown-health-key",
            r#"globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 entities: [{ canonicalName: "e", components: { health: { max: 10, shields: 5 } } }] });"#,
            "unknown field `shields`",
        ),
        (
            "layer-named-health",
            r#"const health = defineDefenseLayer("health");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1", defenseLayers: [health] });"#,
            "defenseLayers[0]",
        ),
        (
            "duplicate-registration",
            r#"const a = defineDefenseLayer("shield");
               const b = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1", defenseLayers: [a, b] });"#,
            "defenseLayers[1]",
        ),
        (
            "string-in-list",
            r#"globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1", defenseLayers: ["shield"] });"#,
            "bare string",
        ),
        (
            "unknown-option",
            r#"const shield = defineDefenseLayer("shield", { recharge: 1 });
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1", defenseLayers: [shield] });"#,
            "defenseLayers[0].options.recharge",
        ),
        (
            "weapon-type-unregistered",
            r#"const shock = defineDamageType("shock", {});
               const plasma = defineDamageType("plasma", {});
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 damageTypes: [shock],
                 entities: [{ canonicalName: "w", components: { weapon: {
                   damage: 1, range: 10, resolution: "hitscan", damageType: plasma,
                   primary: { trigger: "press", recoveryMs: 100, steps: [{ kind: "shot" }] } } } }] });"#,
            "entities[0].components.weapon.damageType",
        ),
        (
            "weapon-type-is-layer",
            r#"const shield = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "w", components: { weapon: {
                   damage: 1, range: 10, resolution: "hitscan", damageType: shield,
                   primary: { trigger: "press", recoveryMs: 100, steps: [{ kind: "shot" }] } } } }] });"#,
            "expected a damage type handle",
        ),
        (
            "resist-key-unregistered",
            r#"const shield = defineDefenseLayer("shield");
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 defenseLayers: [shield],
                 entities: [{ canonicalName: "e", components: { health: { max: 10,
                   layers: [{ layer: shield, capacity: 5, resist: { shock: 0.5 } }] } } }] });"#,
            "components.health.layers[0].resist.shock",
        ),
        (
            "health-resist-out-of-range",
            r#"const shock = defineDamageType("shock", {});
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 damageTypes: [shock],
                 entities: [{ canonicalName: "e", components: { health: { max: 10, resist: { shock: 65 } } } }] });"#,
            "components.health.resist.shock",
        ),
        (
            "matrix-key-unregistered",
            r#"const shock = defineDamageType("shock", { shield: 2 });
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1", damageTypes: [shock] });"#,
            "damageTypes[0].matrix.shield",
        ),
    ];
    for (label, source, needle) in cases {
        let result = build(label, "start-script.js", source);
        assert_eq!(
            result.status,
            StagedManifestBuildStatus::Failed,
            "{label} must fail load"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(needle)),
            "{label}: no diagnostic mentions `{needle}`: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn luau_layer_names_outside_the_grammar_fail_load() {
    for (label, name) in [
        ("empty", ""),
        ("dot", "a.b"),
        ("colon", "a:b"),
        ("at", "a@b"),
    ] {
        let source = format!(
            r#"local layer = defineDefenseLayer("{name}")
               return defineMod({{ name = "M", id = "m", version = "1", defenseLayers = {{ layer }} }})"#
        );
        let result = build(&format!("luau_{label}"), "start-script.luau", &source);
        assert_eq!(result.status, StagedManifestBuildStatus::Failed, "{label}");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("defenseLayers[0].name")),
            "{label}: {:?}",
            result.diagnostics
        );
    }
    let long = "x".repeat(65);
    let source = format!(
        r#"return defineMod({{ name = "M", id = "m", version = "1", defenseLayers = {{ defineDefenseLayer("{long}") }} }})"#
    );
    assert_eq!(
        build("luau_long", "start-script.luau", &source).status,
        StagedManifestBuildStatus::Failed
    );
}

#[test]
fn a_valid_vocabulary_with_both_resists_and_a_mixed_matrix_loads() {
    let source = r#"
        const shield = defineDefenseLayer("shield");
        const armor = defineDefenseLayer("armor");
        const shock = defineDamageType("shock", { shield: 2, armor: 0, health: 0.75 });
        const flechette = defineDamageType("flechette", { shield: "bypass", armor: 0 });
        globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
          defenseLayers: [shield, armor], damageTypes: [shock, flechette], defaultDamageType: shock,
          entities: [{ canonicalName: "e", components: { health: { max: 70, resist: { shock: 0.5 },
            layers: [{ layer: shield, capacity: 30, resist: { flechette: 0 } }, { layer: armor, capacity: 120 }] } } }] });
    "#;
    let manifest = built(build("valid-mixed", "start-script.js", source));
    assert_eq!(manifest.defense.layers, ["shield", "armor"]);
    assert_eq!(manifest.defense.default_damage_type, Some(0));
    assert_eq!(
        manifest.entities[0].health.as_ref().unwrap().layers.len(),
        2
    );
}
