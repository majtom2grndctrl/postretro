// Character weapon mounts through the SDK and the staged manifest load in both
// runtimes: the Scripting surface example (row S1), one table shared by
// several definitions (row L5), a defined character reused in another
// definition, the builders' by-value weapon check, and a misspelled mesh key
// failing load (row L1).
// See: context/lib/entity_model.md §2 (MeshComponent) · context/lib/scripting.md §1

use std::fs;

use postretro_entities::data_descriptors::MountDescriptor;

use super::tests::temp_mod_root;
use super::*;

const WEAPON_TS: &str = r#"{ damage: 1, range: 10, resolution: "hitscan", primary: { trigger: "press", recoveryMs: 100, steps: [{ kind: "shot" }] } }"#;
const WEAPON_LUAU: &str = r#"{ damage = 1, range = 10, resolution = "hitscan", primary = { trigger = "press", recoveryMs = 100, steps = { { kind = "shot" } } } }"#;

const RIFLE_BAKE: &str = "models/cyberpunk_weapons/rifle/exo_red.gltf";
const PISTOL_BAKE: &str = "models/cyberpunk_weapons/pistol/exo_red.gltf";

fn bundle_typescript(label: &str, modules: &[(&str, String)]) -> String {
    let dir = temp_mod_root(&format!("weapon_mounts_ts_{label}"));
    for (file, source) in modules {
        fs::write(dir.join(file), source).expect("TypeScript module writes");
    }
    let entry = fs::canonicalize(dir.join(modules[0].0)).expect("entry exists");
    postretro_script_compiler::bundle_entry(&entry).expect("the fixture bundles")
}

fn build(label: &str, files: &[(&str, &str)]) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("weapon_mounts_{label}"));
    for (file, source) in files {
        fs::write(dir.join(file), source).expect("fixture should be written");
    }
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn built(label: &str, files: &[(&str, &str)]) -> Box<transfer::StagedManifest> {
    let result = build(label, files);
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("{label} must build: {other:?} {:?}", result.diagnostics),
    }
}

fn assert_fails_mentioning(label: &str, result: &StagedManifestBuildResult, needle: &str) {
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

fn mount(weapon: &str, model: &str) -> MountDescriptor {
    MountDescriptor {
        item: weapon.to_string(),
        attachments: [("hand_r".to_string(), model.to_string())]
            .into_iter()
            .collect(),
    }
}

fn mounts_of(manifest: &transfer::StagedManifest, name: &str) -> Option<Vec<MountDescriptor>> {
    manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("`{name}` registers"))
        .mesh
        .as_ref()
        .and_then(|mesh| mesh.mounts.clone())
}

// The brief's Scripting surface example, verbatim apart from the weapon
// modules it imports and the manifest that registers it.
const PLAYER_TS: &str = r#"
import { defineEntity } from "postretro";
import { referencePistolEntity as pistol } from "./reference-pistol";
import { referenceRifleEntity as rifle } from "./reference-rifle";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    mesh: {
      model: "models/exo_red/model.gltf",
      shadowOnly: true,
      // Each bake fitted to exo_red's hand with `postretro-tool solve-weapon-mount`,
      // kept beside its weapon and named for the rig.
      mounts: [
        { item: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
        { item: pistol, attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf" } },
      ],
      // animations… as today
    },
    inventory: { loadout: [rifle, pistol] },
    // movement, health… as today
  },
});
// Weapons no longer carry `thirdPersonModel`; `viewmodel` stays.
"#;

const START_TS: &str = r#"
import { defineMod } from "postretro";
import { playerEntity } from "./player";
import { referencePistolEntity } from "./reference-pistol";
import { referenceRifleEntity } from "./reference-rifle";

export default defineMod({
  name: "Mounts", id: "e21.mounts", version: "1",
  entities: [referenceRifleEntity, referencePistolEntity, playerEntity],
});
"#;

const PLAYER_LUAU: &str = r#"
local pistol = require("./reference-pistol.luau").referencePistolEntity
local rifle = require("./reference-rifle.luau").referenceRifleEntity

return {
  playerEntity = defineEntity({
    canonicalName = "player",
    components = {
      mesh = {
        model = "models/exo_red/model.gltf",
        shadowOnly = true,
        mounts = {
          { item = rifle, attachments = { hand_r = "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
          { item = pistol, attachments = { hand_r = "models/cyberpunk_weapons/pistol/exo_red.gltf" } },
        },
      },
      inventory = { loadout = { rifle, pistol } },
    },
  }),
}
"#;

const START_LUAU: &str = r#"
local playerEntity = require("./player.luau").playerEntity
local referencePistolEntity = require("./reference-pistol.luau").referencePistolEntity
local referenceRifleEntity = require("./reference-rifle.luau").referenceRifleEntity

return defineMod({
  name = "Mounts", id = "e21.mounts", version = "1",
  entities = { referenceRifleEntity, referencePistolEntity, playerEntity },
})
"#;

fn weapon_module_ts(binding: &str, name: &str) -> String {
    format!(
        "import {{ defineEntity }} from \"postretro\";\nexport const {binding} = defineEntity({{ canonicalName: \"{name}\", components: {{ weapon: {WEAPON_TS} }} }});\n"
    )
}

fn weapon_module_luau(binding: &str, name: &str) -> String {
    format!(
        "return {{ {binding} = defineEntity({{ canonicalName = \"{name}\", components = {{ weapon = {WEAPON_LUAU} }} }}) }}\n"
    )
}

// S1: the Scripting surface example loads in both runtimes and lowers each
// weapon handle to its canonical name, giving one descriptor.
#[test]
fn the_weapon_mounts_surface_example_lowers_identically_in_both_runtimes() {
    let bundled = bundle_typescript(
        "surface",
        &[
            ("start-script.ts", START_TS.to_string()),
            ("player.ts", PLAYER_TS.to_string()),
            (
                "reference-pistol.ts",
                weapon_module_ts("referencePistolEntity", "reference_pistol"),
            ),
            (
                "reference-rifle.ts",
                weapon_module_ts("referenceRifleEntity", "reference_rifle"),
            ),
        ],
    );
    let ts = built("surface_ts", &[("start-script.js", &bundled)]);
    let pistol_luau = weapon_module_luau("referencePistolEntity", "reference_pistol");
    let rifle_luau = weapon_module_luau("referenceRifleEntity", "reference_rifle");
    let luau = built(
        "surface_luau",
        &[
            ("start-script.luau", START_LUAU),
            ("player.luau", PLAYER_LUAU),
            ("reference-pistol.luau", &pistol_luau),
            ("reference-rifle.luau", &rifle_luau),
        ],
    );
    assert_eq!(ts.entities, luau.entities, "both runtimes produce one set");

    assert_eq!(
        mounts_of(&ts, "player"),
        Some(vec![
            mount("reference_rifle", RIFLE_BAKE),
            mount("reference_pistol", PISTOL_BAKE),
        ])
    );
    let player = ts
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some("player"))
        .unwrap();
    assert!(player.mesh.as_ref().unwrap().shadow_only);
    assert_eq!(
        player.inventory.as_ref().unwrap().loadout,
        ["reference_rifle", "reference_pistol"]
    );
}

const SHARED_JS: &str = r#"
    const rifle = defineEntity({ canonicalName: "reference_rifle", components: { weapon: WEAPON } });
    const mounts = [{ item: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } }];
    const base = { model: "models/exo_red/model.gltf", mounts: mounts };
    const sharedMesh = { model: "models/exo_red/model.gltf", mounts: mounts };
    const sharedComponents = { mesh: { model: "models/exo_red/model.gltf", mounts: mounts } };
    const entities = [
        rifle,
        defineEntity({ canonicalName: "direct_a", components: { mesh: { model: "models/a.gltf", mounts: mounts } } }),
        defineEntity({ canonicalName: "direct_b", components: { mesh: { model: "models/b.gltf", mounts: mounts } } }),
        defineEntity({ canonicalName: "spread_a", components: { mesh: { ...base, shadowOnly: true } } }),
        defineEntity({ canonicalName: "spread_b", components: { mesh: { ...base } } }),
        defineEntity({ canonicalName: "mesh_a", components: { mesh: sharedMesh } }),
        defineEntity({ canonicalName: "mesh_b", components: { mesh: sharedMesh } }),
        defineEntity({ canonicalName: "components_a", components: sharedComponents }),
        defineEntity({ canonicalName: "components_b", components: sharedComponents }),
    ];
    if (mounts[0].item !== rifle || base.mounts !== mounts || sharedMesh.mounts !== mounts
        || sharedComponents.mesh.mounts !== mounts) {
        throw new Error("defineEntity changed the authored weapon-mount table");
    }
    globalThis.__postretroModManifest = defineMod({ name: "Shared", id: "shared", version: "1", entities });
"#;

const SHARED_LUAU: &str = r#"
    local rifle = defineEntity({ canonicalName = "reference_rifle", components = { weapon = WEAPON } })
    local mounts = { { item = rifle, attachments = { hand_r = "models/cyberpunk_weapons/rifle/exo_red.gltf" } } }
    local base = { model = "models/exo_red/model.gltf", mounts = mounts }
    local sharedMesh = { model = "models/exo_red/model.gltf", mounts = mounts }
    local sharedComponents = { mesh = { model = "models/exo_red/model.gltf", mounts = mounts } }
    local spreadA = table.clone(base)
    spreadA.shadowOnly = true
    local entities = {
        rifle,
        defineEntity({ canonicalName = "direct_a", components = { mesh = { model = "models/a.gltf", mounts = mounts } } }),
        defineEntity({ canonicalName = "direct_b", components = { mesh = { model = "models/b.gltf", mounts = mounts } } }),
        defineEntity({ canonicalName = "spread_a", components = { mesh = spreadA } }),
        defineEntity({ canonicalName = "spread_b", components = { mesh = table.clone(base) } }),
        defineEntity({ canonicalName = "mesh_a", components = { mesh = sharedMesh } }),
        defineEntity({ canonicalName = "mesh_b", components = { mesh = sharedMesh } }),
        defineEntity({ canonicalName = "components_a", components = sharedComponents }),
        defineEntity({ canonicalName = "components_b", components = sharedComponents }),
    }
    if mounts[1].item ~= rifle or base.mounts ~= mounts or sharedMesh.mounts ~= mounts
        or sharedComponents.mesh.mounts ~= mounts then
        error("defineEntity changed the authored weapon-mount table")
    end
    return defineMod({ name = "Shared", id = "shared", version = "1", entities = entities })
"#;

// L5: one table shared by several `defineEntity` calls (directly, through a
// spread or shared `mesh`, or through a shared `components`) lowers to the
// same names for every definition, and the authored table keeps its handles.
#[test]
fn a_shared_weapon_mounts_table_lowers_the_same_for_every_definition_in_both_runtimes() {
    let js = built(
        "shared_js",
        &[("start-script.js", &SHARED_JS.replace("WEAPON", WEAPON_TS))],
    );
    let luau = built(
        "shared_luau",
        &[(
            "start-script.luau",
            &SHARED_LUAU.replace("WEAPON", WEAPON_LUAU),
        )],
    );
    assert_eq!(js.entities, luau.entities);
    for name in [
        "direct_a",
        "direct_b",
        "spread_a",
        "spread_b",
        "mesh_a",
        "mesh_b",
        "components_a",
        "components_b",
    ] {
        assert_eq!(
            mounts_of(&js, name),
            Some(vec![mount("reference_rifle", RIFLE_BAKE)]),
            "`{name}` lowers the shared table"
        );
    }
}

// The builders reject a weapon-mount reference that is not a weapon
// descriptor by value, as they do a loadout entry.
#[test]
fn the_sdk_rejects_a_non_weapon_mount_reference_by_value_in_both_runtimes() {
    let cases = [
        ("number", "42", "42", "must reference an entity descriptor"),
        (
            "bare name",
            r#""reference_rifle""#,
            r#""reference_rifle""#,
            "must reference an entity descriptor",
        ),
        (
            "descriptor without weapon",
            r#"{ canonicalName: "crate", components: {} }"#,
            r#"{ canonicalName = "crate", components = {} }"#,
            "must reference a descriptor with a weapon block",
        ),
        (
            "non-object weapon block",
            r#"{ canonicalName: "crate", components: { weapon: [] } }"#,
            r#"{ canonicalName = "crate", components = { weapon = 42 } }"#,
            "must reference a descriptor with a weapon block",
        ),
        (
            "weapon without canonical name",
            r#"{ components: { weapon: {} } }"#,
            r#"{ components = { weapon = {} } }"#,
            "must reference a descriptor with a canonical name",
        ),
    ];
    for (label, js_weapon, luau_weapon, reason) in cases {
        let needle = format!("components.mesh.mounts[0].item {reason}");
        let js = format!(
            r#"defineEntity({{ canonicalName: "player", components: {{ mesh: {{ model: "m.gltf",
                 mounts: [{{ item: {js_weapon}, attachments: {{ hand_r: "r.gltf" }} }}] }} }} }});
               globalThis.__postretroModManifest = defineMod({{ name: "M", id: "m", version: "1" }});"#
        );
        let luau = format!(
            r#"defineEntity({{ canonicalName = "player", components = {{ mesh = {{ model = "m.gltf",
                 mounts = {{ {{ item = {luau_weapon}, attachments = {{ hand_r = "r.gltf" }} }} }} }} }} }})
               return defineMod({{ name = "M", id = "m", version = "1" }})"#
        );
        assert_fails_mentioning(
            &format!("QuickJS {label}"),
            &build(&format!("reject_js_{label}"), &[("start-script.js", &js)]),
            &needle,
        );
        assert_fails_mentioning(
            &format!("Luau {label}"),
            &build(
                &format!("reject_luau_{label}"),
                &[("start-script.luau", &luau)],
            ),
            &needle,
        );
    }
}

// L1: a misspelled `mesh` key fails the mod load in both runtimes, naming the
// key and the character, rather than silently opting the character out.
#[test]
fn a_misspelled_mesh_key_fails_load_naming_the_key_and_the_character() {
    let needle = "entity `player`: `components.mesh.weaponMount` is not a mesh field";
    let js = build(
        "misspelled_js",
        &[(
            "start-script.js",
            r#"globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 entities: [defineEntity({ canonicalName: "player", components: { mesh: {
                   model: "models/exo_red/model.gltf", weaponMount: [] } } })] });"#,
        )],
    );
    let luau = build(
        "misspelled_luau",
        &[(
            "start-script.luau",
            r#"return defineMod({ name = "M", id = "m", version = "1",
                 entities = { defineEntity({ canonicalName = "player", components = { mesh = {
                   model = "models/exo_red/model.gltf", weaponMount = {} } } }) } })"#,
        )],
    );
    assert_fails_mentioning("QuickJS", &js, needle);
    assert_fails_mentioning("Luau", &luau, needle);
}

const REUSED_JS: &str = r#"
    const rifle = defineEntity({ canonicalName: "reference_rifle", components: { weapon: WEAPON } });
    const player = defineEntity({ canonicalName: "player", components: {
        mesh: { model: "models/exo_red/model.gltf",
                mounts: [{ item: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } }] },
        inventory: { loadout: [rifle] },
    } });
    const entities = [
        rifle,
        player,
        defineEntity({ canonicalName: "components_direct", components: player.components }),
        defineEntity({ canonicalName: "components_spread", components: { ...player.components } }),
        defineEntity({ canonicalName: "mesh_direct", components: { mesh: player.components.mesh } }),
        defineEntity({ canonicalName: "mesh_spread", components: { mesh: { ...player.components.mesh, shadowOnly: true } } }),
        defineEntity({ ...player, canonicalName: "entity_spread" }),
    ];
    globalThis.__postretroModManifest = defineMod({ name: "Reused", id: "reused", version: "1", entities });
"#;

const REUSED_LUAU: &str = r#"
    local rifle = defineEntity({ canonicalName = "reference_rifle", components = { weapon = WEAPON } })
    local player = defineEntity({ canonicalName = "player", components = {
        mesh = { model = "models/exo_red/model.gltf",
                 mounts = { { item = rifle, attachments = { hand_r = "models/cyberpunk_weapons/rifle/exo_red.gltf" } } } },
        inventory = { loadout = { rifle } },
    } })
    local meshSpread = table.clone(player.components.mesh)
    meshSpread.shadowOnly = true
    local entitySpread = table.clone(player)
    entitySpread.canonicalName = "entity_spread"
    local entities = {
        rifle,
        player,
        defineEntity({ canonicalName = "components_direct", components = player.components }),
        defineEntity({ canonicalName = "components_spread", components = table.clone(player.components) }),
        defineEntity({ canonicalName = "mesh_direct", components = { mesh = player.components.mesh } }),
        defineEntity({ canonicalName = "mesh_spread", components = { mesh = meshSpread } }),
        defineEntity(entitySpread),
    }
    return defineMod({ name = "Reused", id = "reused", version = "1", entities = entities })
"#;

// A defined character's `components`, its `mesh` (direct or spread), or the
// defined entity itself fed into another `defineEntity` lowers to the same
// names: the builders pass through what they already lowered.
#[test]
fn reusing_a_defined_characters_components_or_mesh_lowers_the_same_in_both_runtimes() {
    let js = built(
        "reused_js",
        &[("start-script.js", &REUSED_JS.replace("WEAPON", WEAPON_TS))],
    );
    let luau = built(
        "reused_luau",
        &[(
            "start-script.luau",
            &REUSED_LUAU.replace("WEAPON", WEAPON_LUAU),
        )],
    );
    assert_eq!(js.entities, luau.entities, "both runtimes produce one set");
    for name in [
        "player",
        "components_direct",
        "components_spread",
        "mesh_direct",
        "mesh_spread",
        "entity_spread",
    ] {
        assert_eq!(
            mounts_of(&js, name),
            Some(vec![mount("reference_rifle", RIFLE_BAKE)]),
            "`{name}` lowers the reused table"
        );
    }
    for name in [
        "player",
        "components_direct",
        "components_spread",
        "entity_spread",
    ] {
        let entity = js
            .entities
            .iter()
            .find(|entity| entity.canonical_name.as_deref() == Some(name))
            .unwrap();
        assert_eq!(
            entity.inventory.as_ref().unwrap().loadout,
            ["reference_rifle"],
            "`{name}` keeps the reused loadout"
        );
    }
}

// Passing lowered data through never admits an authored name: a string
// `weapon` beside reused entries, or a string loadout entry, still rejects.
#[test]
fn an_authored_string_weapon_still_rejects_beside_reused_entries_in_both_runtimes() {
    let player_js = r#"const rifle = defineEntity({ canonicalName: "reference_rifle", components: { weapon: WEAPON } });
        const player = defineEntity({ canonicalName: "player", components: { mesh: { model: "m.gltf",
            mounts: [{ item: rifle, attachments: { hand_r: "r.gltf" } }] }, inventory: { loadout: [rifle] } } });"#
        .replace("WEAPON", WEAPON_TS);
    let player_luau = r#"local rifle = defineEntity({ canonicalName = "reference_rifle", components = { weapon = WEAPON } })
        local player = defineEntity({ canonicalName = "player", components = { mesh = { model = "m.gltf",
            mounts = { { item = rifle, attachments = { hand_r = "r.gltf" } } } }, inventory = { loadout = { rifle } } } })"#
        .replace("WEAPON", WEAPON_LUAU);
    let cases = [
        (
            "mount",
            r#"{ mesh: { model: "m.gltf", mounts: [...player.components.mesh.mounts,
                 { item: "reference_rifle", attachments: { hand_r: "p.gltf" } }] } }"#,
            r#"{ mesh = { model = "m.gltf", mounts = { player.components.mesh.mounts[1],
                 { item = "reference_rifle", attachments = { hand_r = "p.gltf" } } } } }"#,
            "components.mesh.mounts[1].item must reference an entity descriptor",
        ),
        (
            "loadout",
            r#"{ mesh: player.components.mesh, inventory: { loadout: ["reference_rifle"] } }"#,
            r#"{ mesh = player.components.mesh, inventory = { loadout = { "reference_rifle" } } }"#,
            "components.inventory.loadout[0] must reference an entity descriptor",
        ),
    ];
    for (label, js_components, luau_components, needle) in cases {
        let js = format!(
            r#"{player_js}
               defineEntity({{ canonicalName: "second", components: {js_components} }});
               globalThis.__postretroModManifest = defineMod({{ name: "M", id: "m", version: "1" }});"#
        );
        let luau = format!(
            r#"{player_luau}
               defineEntity({{ canonicalName = "second", components = {luau_components} }})
               return defineMod({{ name = "M", id = "m", version = "1" }})"#
        );
        assert_fails_mentioning(
            &format!("QuickJS {label}"),
            &build(
                &format!("reused_reject_js_{label}"),
                &[("start-script.js", &js)],
            ),
            needle,
        );
        assert_fails_mentioning(
            &format!("Luau {label}"),
            &build(
                &format!("reused_reject_luau_{label}"),
                &[("start-script.luau", &luau)],
            ),
            needle,
        );
    }
}
