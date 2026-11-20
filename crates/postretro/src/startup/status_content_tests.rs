// The statuses Scripting surface example run as mod manifests through the
// production script runtime: the `content/dev/scripts/statuses-demo.{ts,luau}`
// twins must register the same statuses, damage types, fluid, impact policy
// and reaction, and content/dev's own TypeScript manifest must carry them
// with its dev hooks: the incendiary rifle, the burn trap, the status HUD
// and lava in the playtest map. Behavior through the fixed tick is
// `sim::status_dev_fixture_tests`.
// See: context/lib/scripting.md §2 (Status declarations)

use postretro_foundation::defense::{DefenseVocabulary, HandleRef};
use postretro_foundation::status::StatusDeclaration;
use postretro_scripting_core::runtime::ModManifestResult;

use super::fluids_swim_surface_tests::{dev_content, dev_manifest, fluids_named_by, run_manifest};

fn ts_twin() -> ModManifestResult {
    let bundle =
        postretro_script_compiler::bundle_entry(&dev_content().join("scripts/statuses-demo.ts"))
            .expect("statuses-demo.ts bundles");
    run_manifest("start-script.js", &bundle).expect("TS twin initializes")
}

fn luau_twin() -> ModManifestResult {
    let source = std::fs::read_to_string(dev_content().join("scripts/statuses-demo.luau"))
        .expect("statuses-demo.luau reads");
    run_manifest("start-script.luau", &source).expect("Luau twin initializes")
}

/// A status as authored, its damage type by name: comparable across
/// vocabularies whose type indices differ.
fn authored(
    vocabulary: &DefenseVocabulary,
    status: &StatusDeclaration,
) -> (StatusDeclaration, Option<String>) {
    let type_name = status
        .damage_type
        .map(|index| vocabulary.damage_types[index].name.clone());
    let mut status = status.clone();
    status.damage_type = None;
    (status, type_name)
}

fn statuses(manifest: &ModManifestResult) -> Vec<(StatusDeclaration, Option<String>)> {
    manifest
        .defense
        .statuses
        .iter()
        .map(|status| authored(&manifest.defense, status))
        .collect()
}

fn policy(manifest: &ModManifestResult, id: &str) -> Vec<serde_json::Value> {
    manifest
        .events
        .iter()
        .find(|event| event.id.ends_with(id))
        .map(|event| event.policy.clone())
        .unwrap_or_else(|| panic!("no impact event `{id}`"))
}

fn reaction(manifest: &ModManifestResult, name: &str) -> postretro_entities::ReactionDescriptor {
    manifest
        .reactions
        .iter()
        .find(|scoped| scoped.reaction.name == name)
        .map(|scoped| scoped.reaction.descriptor.clone())
        .unwrap_or_else(|| panic!("no reaction `{name}`"))
}

// The example runs as a content/dev fixture in TypeScript and Luau, and both
// declare the same statuses, types, lava, policy and cure.
#[test]
fn statuses_twins_run_as_mod_manifests_and_declare_the_same_surface() {
    let ts = ts_twin();
    let luau = luau_twin();

    assert_eq!(ts.defense, luau.defense, "one registered vocabulary");
    let types: Vec<&str> = ts
        .defense
        .damage_types
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(types, ["fire", "acid"]);
    let names: Vec<&str> = ts
        .defense
        .statuses
        .iter()
        .map(|status| status.name.as_str())
        .collect();
    assert_eq!(names, ["burning", "scalding", "corroding", "wet"]);
    let burning = &ts.defense.statuses[0];
    assert_eq!(
        (
            burning.damage,
            burning.tick_ms,
            burning.duration_ms,
            burning.max_stacks
        ),
        (4.0, 500.0, Some(3000.0), 3)
    );
    assert!(burning.presentation.emitter.is_some(), "embers");
    assert_eq!(
        burning.presentation.tick_sound.as_deref(),
        Some("fixtures/pellet_hit")
    );
    assert_eq!(ts.defense.statuses[2].duration_ms, None, "until cleared");
    assert!(ts.defense.statuses[3].is_marker());

    assert_eq!(ts.fluids, luau.fluids);
    let lava = &ts.fluids.descriptors()[0];
    assert_eq!(lava.name, "lava");
    assert_eq!(
        lava.status.as_ref().unwrap().handle("status").unwrap(),
        HandleRef::Status {
            name: "scalding".to_string()
        }
    );
    assert_eq!(
        policy(&ts, "statuses.ignite"),
        policy(&luau, "statuses.ignite")
    );
    assert_eq!(
        reaction(&ts, "statuses.cure"),
        reaction(&luau, "statuses.cure")
    );
}

// content/dev's TypeScript manifest carries the example's statuses, lava,
// policy and cure under its own mod id, with its hooks: the burn trap's
// trigger event, the incendiary rifle in the player's loadout, a HUD
// indicator per status, and a lava pool in the playtest map.
#[test]
fn dev_manifest_carries_the_statuses_example_and_its_dev_hooks() {
    let dev = dev_manifest();
    let twin = ts_twin();

    assert_eq!(dev.id, "postretro.dev");
    assert_eq!(statuses(&dev), statuses(&twin));
    let lava = dev
        .fluids
        .descriptors()
        .iter()
        .find(|fluid| fluid.name == "lava")
        .expect("content/dev declares lava");
    assert_eq!(lava, &twin.fluids.descriptors()[0]);
    assert_eq!(
        policy(&dev, "statuses.ignite"),
        policy(&twin, "statuses.ignite")
    );
    assert_eq!(
        reaction(&dev, "statuses.cure"),
        reaction(&twin, "statuses.cure")
    );

    let trap = dev
        .trigger_events
        .iter()
        .find(|event| event.tag == "burn_trap")
        .expect("the burn trap installs");
    assert_eq!(trap.fire, ["statuses.burnTrap"]);
    reaction(&dev, "statuses.burnTrap");

    let player = dev
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some("player"))
        .expect("the dev player");
    assert!(
        player
            .inventory
            .as_ref()
            .unwrap()
            .loadout
            .iter()
            .any(|weapon| weapon == "incendiary_rifle"),
        "the incendiary rifle is in the dev loadout"
    );

    let hud = dev
        .ui_trees
        .iter()
        .find(|tree| tree.name == "hud.statuses")
        .expect("the status HUD registers");
    let tree = format!("{:?}", hud.tree);
    for status in ["burning", "scalding", "corroding", "wet"] {
        assert!(
            tree.contains(&format!("player.status.{status}")),
            "an indicator for `{status}`"
        );
    }

    let named = fluids_named_by("e24-environment-test.map");
    assert!(named.contains("lava"), "the playtest map has a lava pool");
    assert!(dev.fluids.index_of("lava").is_some());
}
