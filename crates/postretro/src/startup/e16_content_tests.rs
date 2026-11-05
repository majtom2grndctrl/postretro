// The shipped defense-layer content in both runtimes: the dev mod (TypeScript) and the
// committed `e16-luau-mod` fixture (its Luau mirror) register the same
// vocabulary and author the same defense descriptors, the dev mod loads
// clean, its weapons name their types, and an unnamed definer fails with the
// binding-name diagnostic in each runtime.
// See: context/lib/scripting.md §11 (Defense layers)

use std::path::{Path, PathBuf};

use postretro_entities::ScriptCtx;
use postretro_foundation::defense::HandleRef;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ModManifestResult, ScriptRuntime, ScriptRuntimeConfig};
use postretro_test_log_capture::LogCapture;

use super::fluids_swim_surface_tests::{dev_manifest, run_manifest};
use crate::scripting::primitives::register_all;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e16-luau-mod")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// The Luau fixture copied whole into a temporary mod root and initialized
/// as at boot.
fn luau_manifest() -> ModManifestResult {
    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    let mut runtime = ScriptRuntime::new(
        &primitives,
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        &ctx,
    )
    .expect("script runtime constructs");
    let mod_root = tempfile::tempdir().expect("temp mod root");
    copy_tree(&fixture(), mod_root.path());
    runtime
        .run_mod_init(mod_root.path())
        .expect("the e16 Luau fixture initializes");
    runtime
        .mod_manifest()
        .expect("a committed manifest")
        .clone()
}

fn entity<'a>(
    manifest: &'a ModManifestResult,
    name: &str,
) -> &'a postretro_entities::EntityTypeDescriptor {
    manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no `{name}` descriptor"))
}

/// The defense half of a descriptor: its layers and resistances, and a
/// weapon's damage and type.
fn defense_shape(manifest: &ModManifestResult, name: &str) -> serde_json::Value {
    let descriptor = entity(manifest, name);
    let health = descriptor.health.as_ref().map(|health| {
        serde_json::json!({ "max": health.max, "layers": health.layers, "resist": health.resist })
    });
    let weapon = descriptor.weapon.as_ref().map(
        |weapon| serde_json::json!({ "damage": weapon.damage, "damageType": weapon.damage_type }),
    );
    serde_json::json!({ "health": health, "weapon": weapon })
}

// The TS dev mod and its Luau mirror register one vocabulary and author
// identical defense descriptors, reactions and policies; handles cross as
// `{ kind, name }`.
#[test]
fn e16_ts_and_luau_mods_produce_identical_vocabulary_manifests() {
    let ts = dev_manifest();
    let luau = luau_manifest();
    assert_eq!(ts.defense, luau.defense, "one registered vocabulary");
    assert_eq!(ts.defense.layers, ["shield", "armor"]);
    let types: Vec<_> = ts
        .defense
        .damage_types
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(
        types,
        [
            "ballistic",
            "plasma",
            "explosive",
            "shock",
            "corrosive",
            "flechette"
        ]
    );
    assert_eq!(ts.defense.default_damage_type, Some(0), "ballistic");

    for name in [
        "player",
        "reference_enemy",
        "reference_enemy_shielded",
        "arc_rifle",
        "acid_rifle",
    ] {
        assert_eq!(
            defense_shape(&ts, name),
            defense_shape(&luau, name),
            "`{name}` differs between runtimes"
        );
    }
    let layer = &entity(&luau, "player").health.as_ref().unwrap().layers[0].layer;
    assert_eq!(
        layer.0,
        serde_json::json!({ "kind": "defenseLayer", "name": "shield" }),
        "a handle crosses as its kind and name only"
    );

    let named = |manifest: &ModManifestResult, name: &str| {
        manifest
            .reactions
            .iter()
            .find(|scoped| scoped.reaction.name == name)
            .map(|scoped| scoped.reaction.descriptor.clone())
            .unwrap_or_else(|| panic!("no reaction `{name}`"))
    };
    for name in ["combat.shieldBooster", "turret.overload"] {
        assert_eq!(named(&ts, name), named(&luau, name), "`{name}`");
    }
    let event = |manifest: &ModManifestResult, id: &str| {
        manifest
            .events
            .iter()
            .find(|event| event.id.ends_with(id))
            .map(|event| event.policy.clone())
            .unwrap_or_else(|| panic!("no impact event `{id}`"))
    };
    assert_eq!(event(&ts, "shield-break"), event(&luau, "shield-break"));
    let pads = |manifest: &ModManifestResult| {
        manifest
            .trigger_events
            .iter()
            .find(|event| event.tag == "shield_booster")
            .cloned()
            .expect("the booster pads install")
    };
    assert_eq!(pads(&ts), pads(&luau));
}

// A Luau descriptor module that requires the vocabulary module again
// gets handles naming the layers `defineMod` registered, and loads.
#[test]
fn luau_descriptor_requiring_defenses_again_resolves_registered_layers() {
    let luau = luau_manifest();
    for (name, layer) in [
        ("player", "shield"),
        ("reference_enemy", "armor"),
        ("reference_enemy_shielded", "shield"),
    ] {
        let handle = entity(&luau, name).health.as_ref().unwrap().layers[0]
            .layer
            .handle("layer")
            .unwrap();
        assert_eq!(
            handle,
            HandleRef::DefenseLayer {
                name: layer.to_string()
            }
        );
        assert!(
            luau.defense.layer_index(layer).is_some(),
            "`{layer}` is registered"
        );
    }
}

// The dev mod, with its defense-layer content, initializes without a warning.
#[test]
fn dev_mod_loads_clean_with_e16_content() {
    let capture = LogCapture::start();
    let manifest = dev_manifest();
    let loud: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Warn)
        // Predates the defense-layer content; `ghost_links_surface_tests` allows the same lint.
        .filter(|record| {
            !record
                .message
                .contains("components.behavior: activities [startle] are unreachable")
        })
        .collect();
    assert!(loud.is_empty(), "{loud:#?}");
    let player = entity(&manifest, "player").health.as_ref().unwrap();
    assert_eq!(player.layers.len(), 1, "the player carries its shield");
    let enemy = entity(&manifest, "reference_enemy");
    assert_eq!(enemy.health.as_ref().unwrap().layers.len(), 1, "armor");
    let claw = &enemy.behavior.as_ref().unwrap().attacks["claw"];
    assert_eq!(
        claw.damage_type.as_ref().unwrap().0,
        serde_json::json!({ "kind": "damageType", "name": "corrosive" })
    );
}

// Every dev-mod weapon names its type: the plasma bolt plasma, the
// rocket explosive, the triangle guns shock and corrosive, the rest ballistic.
#[test]
fn dev_weapons_name_their_damage_types() {
    let manifest = dev_manifest();
    let mut weapons = 0;
    for descriptor in &manifest.entities {
        let Some(weapon) = descriptor.weapon.as_ref() else {
            continue;
        };
        weapons += 1;
        let name = descriptor.canonical_name.as_deref().unwrap_or("?");
        let expected = match name {
            "reference_plasma_bolt" => "plasma",
            "reference_rocket" => "explosive",
            "arc_rifle" => "shock",
            "acid_rifle" => "corrosive",
            _ => "ballistic",
        };
        let handle = weapon
            .damage_type
            .as_ref()
            .unwrap_or_else(|| panic!("`{name}` names no damage type"))
            .handle(name)
            .unwrap();
        assert_eq!(
            handle,
            HandleRef::DamageType {
                name: expected.to_string()
            },
            "`{name}`"
        );
    }
    assert!(
        weapons >= 10,
        "the dev mod's weapons were all checked ({weapons})"
    );
}

// An unnamed definer outside a direct top-level binding
// fails with the binding-name diagnostic, in both runtimes.
#[test]
fn defense_definers_without_name_raise_binding_name_diagnostic() {
    let bundle = |source: &str| {
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("start-script.ts");
        std::fs::write(&entry, source).unwrap();
        postretro_script_compiler::bundle_entry(&entry).expect("the start script bundles")
    };
    for (file, source) in [
        (
            "start-script.js",
            bundle(
                "import { defineDefenseLayer, defineMod } from \"postretro\";\nconst layers = [defineDefenseLayer()];\nexport default defineMod({ name: \"x\", id: \"x.x\", version: \"1\" });",
            ),
        ),
        (
            "start-script.js",
            bundle(
                "import { defineDamageType, defineMod } from \"postretro\";\nconst types = [defineDamageType({ shield: 2 })];\nexport default defineMod({ name: \"x\", id: \"x.x\", version: \"1\" });",
            ),
        ),
        (
            "start-script.luau",
            "local P = require(\"postretro\")\nlocal layer = P.defineDefenseLayer()\nreturn P.defineMod({ name = \"x\", id = \"x.x\", version = \"1\" })".to_string(),
        ),
        (
            "start-script.luau",
            "local P = require(\"postretro\")\nlocal ty = P.defineDamageType({ shield = 2 })\nreturn P.defineMod({ name = \"x\", id = \"x.x\", version = \"1\" })".to_string(),
        ),
    ] {
        let error = run_manifest(file, &source).expect_err("an unnamed definer fails load");
        assert!(error.contains("binding-name sugar"), "{file}: {error}");
    }
}
