// The dev mod's characters, runs and checkpoints scripting-surface fixture,
// run as a mod manifest in both runtimes with identity enforcement on.
// See: docs/scripting-reference.md (Characters, runs and checkpoints)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use postretro_entities::ReactionDescriptor;
use postretro_entities::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{
    ModManifestResult, SavesManifest, ScriptRuntime, ScriptRuntimeConfig,
};
use serde_json::json;

use crate::scripting::primitives::register_all;

fn dev_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

/// What one runtime's mod init committed: the manifest, the run-scoped slots
/// and the durable keys it validated.
struct Initialized {
    manifest: ModManifestResult,
    run_scoped: BTreeSet<String>,
    durable_keys: BTreeSet<String>,
}

/// Run `source` as a mod root's start script beside content/dev's identity
/// ledger, through a runtime that enforces durable identity, as boot does.
fn initialize(file_name: &str, source: &[u8]) -> Initialized {
    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    let config = ScriptRuntimeConfig::default();
    assert!(
        !config.skip_identity_enforcement,
        "fixture: the runtime enforces durable identity"
    );
    let mut runtime =
        ScriptRuntime::new(&primitives, &config, &ctx).expect("script runtime constructs");
    let mod_root = tempfile::tempdir().expect("temp mod root");
    std::fs::copy(
        dev_content().join("identity.json"),
        mod_root.path().join("identity.json"),
    )
    .expect("content/dev's identity ledger copies");
    std::fs::write(mod_root.path().join(file_name), source).expect("start script writes");
    runtime
        .run_mod_init(mod_root.path())
        .unwrap_or_else(|error| panic!("{file_name} initializes: {error}"));
    let durable_keys = runtime
        .store_identity()
        .expect("an enforcing runtime keeps the validated ledger")
        .slots
        .keys()
        .filter(|name| name.starts_with("savesFixture."))
        .cloned()
        .collect();
    Initialized {
        manifest: runtime.mod_manifest().expect("mod init commits").clone(),
        run_scoped: runtime.committed_run_scoped_slots().clone(),
        durable_keys,
    }
}

fn reaction<'a>(manifest: &'a ModManifestResult, name: &str) -> (&'a str, &'a serde_json::Value) {
    let found = manifest
        .reactions
        .iter()
        .find(|scoped| scoped.reaction.name == name)
        .unwrap_or_else(|| panic!("the fixture declares `{name}`"));
    let ReactionDescriptor::Primitive(primitive) = &found.reaction.descriptor else {
        panic!("`{name}` is one system reaction");
    };
    (primitive.primitive.as_str(), &primitive.args)
}

// The Scripting surface example runs as a content/dev fixture in TypeScript
// and in Luau, its persisted slots minted in identity.json (D3). Both declare
// the same saves block, run-scoped slots, stores, rows and reactions; Luau's
// 1-based row indices arrive 0-based.
#[test]
fn saves_surface_fixture_ts_and_luau_load() {
    let scripts = dev_content().join("scripts");
    let ts = initialize(
        "start-script.js",
        postretro_script_compiler::bundle_entry(&scripts.join("saves-surface-fixture.ts"))
            .expect("the TS fixture bundles through scripts-build")
            .as_bytes(),
    );
    let luau = initialize(
        "start-script.luau",
        &std::fs::read(scripts.join("saves-surface-fixture.luau")).expect("the Luau fixture reads"),
    );

    let saves = Some(SavesManifest {
        character_slots: 4,
        run_slots: 3,
    });
    assert_eq!(ts.manifest.saves, saves);
    assert_eq!(luau.manifest.saves, saves);

    let run_scoped = BTreeSet::from(["savesFixture.run.generatorOn".to_string()]);
    assert_eq!(ts.run_scoped, run_scoped);
    assert_eq!(luau.run_scoped, run_scoped);

    // Each persisted slot, world, run and character alike, has its minted key.
    let minted = BTreeSet::from([
        "savesFixture.hero.xp".to_string(),
        "savesFixture.meta.episode2".to_string(),
        "savesFixture.run.generatorOn".to_string(),
    ]);
    assert_eq!(ts.durable_keys, minted);
    assert_eq!(luau.durable_keys, minted);

    assert_eq!(
        ts.manifest.store_declarations,
        luau.manifest.store_declarations
    );
    assert_eq!(ts.manifest.reactions, luau.manifest.reactions);
    assert_eq!(ts.manifest.ui_trees, luau.manifest.ui_trees);
    assert_eq!(ts.manifest.maps, luau.manifest.maps);

    // One reaction per verb per row, 0-based on the wire from both runtimes.
    assert_eq!(ts.manifest.reactions.len(), 3 * 3 + 4 * 4 + 1);
    for (name, primitive, args) in [
        ("runs.continue.0", "continueRun", json!({ "index": 0 })),
        (
            "runs.new.2",
            "newRun",
            json!({ "index": 2, "map": "campaign-test" }),
        ),
        ("runs.delete.1", "deleteRun", json!({ "index": 1 })),
        (
            "characters.select.0",
            "selectCharacter",
            json!({ "index": 0 }),
        ),
        (
            "characters.create.3",
            "createCharacter",
            json!({ "index": 3 }),
        ),
        (
            "characters.delete.2",
            "deleteCharacter",
            json!({ "index": 2 }),
        ),
        ("level.bridge", "checkpoint", json!({ "name": "bridge" })),
    ] {
        for (runtime, manifest) in [("TS", &ts.manifest), ("Luau", &luau.manifest)] {
            assert_eq!(
                reaction(manifest, name),
                (primitive, &args),
                "{runtime}: `{name}`"
            );
        }
    }
    let (primitive, args) = reaction(&luau.manifest, "characters.name.1");
    assert_eq!(primitive, "showDialog");
    assert_eq!(args["onCommit"], "characters.create.1");

    // Each row's controls press their own row's reactions, ids suffixed from 0.
    let [tree] = ts.manifest.ui_trees.as_slice() else {
        panic!("the fixture registers one tree");
    };
    let wire = serde_json::to_string(&tree.tree).unwrap();
    for (id, on_press) in [
        ("continue.0", "runs.continue.0"),
        ("delete.2", "runs.delete.2"),
        ("create.3", "characters.name.3"),
        ("select.1", "characters.select.1"),
    ] {
        assert!(
            wire.contains(&format!(r#""id":"{id}""#)) && wire.contains(on_press),
            "row control `{id}` presses `{on_press}`: {wire}"
        );
    }
}
