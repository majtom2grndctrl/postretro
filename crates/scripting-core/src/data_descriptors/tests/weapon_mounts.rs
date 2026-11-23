// Tests: `components.mesh` key set and the `weaponMounts` table, parsed by
// both descriptor readers.
// See: context/lib/entity_model.md §2 (MeshComponent)

use super::super::*;
use super::common::*;
use postretro_entities::data_descriptors::WeaponMountDescriptor;

const KNOWN_MESH_KEYS: &str = "model, attachments, shadowBiasScale, shadowOnly, animations, defaultState, locomotion, weaponMounts";

fn js_player(mesh_fields: &str) -> String {
    format!(
        r#"({{ canonicalName: "player", components: {{ mesh: {{ model: "models/exo_red/model.gltf", {mesh_fields} }} }} }})"#
    )
}

fn lua_player(mesh_fields: &str) -> String {
    format!(
        r#"return {{ canonicalName = "player", components = {{ mesh = {{ model = "models/exo_red/model.gltf", {mesh_fields} }} }} }}"#
    )
}

fn js_error(source: &str) -> String {
    eval_js(source, |ctx, value| {
        entity_descriptor_from_js(ctx, value)
            .expect_err("QuickJS must reject the descriptor")
            .to_string()
    })
}

fn lua_error(source: &str) -> String {
    eval_lua(source, |value| {
        entity_descriptor_from_lua(value)
            .expect_err("Luau must reject the descriptor")
            .to_string()
    })
}

fn mount(weapon: &str, attachments: &[(&str, &str)]) -> WeaponMountDescriptor {
    WeaponMountDescriptor {
        weapon: weapon.to_string(),
        attachments: attachments
            .iter()
            .map(|(socket, model)| (socket.to_string(), model.to_string()))
            .collect(),
    }
}

#[test]
fn weapon_mounts_parse_identically_in_both_runtimes() {
    let js = eval_js(
        &js_player(
            r#"weaponMounts: [
                { weapon: "reference_rifle", attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
                { weapon: "reference_pistol", attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf", back: "models/holster/exo_red.gltf" } },
            ]"#,
        ),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_player(
            r#"weaponMounts = {
                { weapon = "reference_rifle", attachments = { hand_r = "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
                { weapon = "reference_pistol", attachments = { hand_r = "models/cyberpunk_weapons/pistol/exo_red.gltf", back = "models/holster/exo_red.gltf" } },
            }"#,
        ),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    assert_eq!(js, lua, "both runtimes produce one descriptor");
    let mesh = js.mesh.expect("mesh parses");
    assert_eq!(
        mesh.weapon_mounts,
        Some(vec![
            mount(
                "reference_rifle",
                &[("hand_r", "models/cyberpunk_weapons/rifle/exo_red.gltf")]
            ),
            mount(
                "reference_pistol",
                &[
                    ("hand_r", "models/cyberpunk_weapons/pistol/exo_red.gltf"),
                    ("back", "models/holster/exo_red.gltf"),
                ]
            ),
        ])
    );
    assert_eq!(
        mesh.weapon_mount("reference_pistol")
            .map(|entry| entry.attachments.len()),
        Some(2)
    );
    assert!(mesh.weapon_mount("reference_shotgun").is_none());
    let mut models: Vec<_> = mesh.weapon_mount_models().collect();
    models.sort();
    assert_eq!(
        models,
        [
            "models/cyberpunk_weapons/pistol/exo_red.gltf",
            "models/cyberpunk_weapons/rifle/exo_red.gltf",
            "models/holster/exo_red.gltf",
        ]
    );
}

// No table opts out silently; an empty table is still a declared one.
#[test]
fn an_absent_table_is_none_and_an_empty_one_is_declared() {
    let absent = eval_js(&js_player(""), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let undefined = eval_js(&js_player("weaponMounts: undefined"), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let lua_absent = eval_lua(&lua_player(""), |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    for descriptor in [&absent, &undefined, &lua_absent] {
        assert_eq!(descriptor.mesh.as_ref().unwrap().weapon_mounts, None);
    }
    let js_empty = eval_js(&js_player("weaponMounts: []"), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let lua_empty = eval_lua(&lua_player("weaponMounts = {}"), |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    assert_eq!(js_empty.mesh.unwrap().weapon_mounts, Some(Vec::new()));
    assert_eq!(lua_empty.mesh.unwrap().weapon_mounts, Some(Vec::new()));
}

// Each load rule rejects in both runtimes with the same text, naming the
// character.
#[test]
fn weapon_mount_load_rules_reject_identically_naming_the_character() {
    let cases = [
        (
            "misspelled mesh key",
            r#"weaponMount: []"#,
            r#"weaponMount = {}"#,
            format!(
                "entity `player`: `components.mesh.weaponMount` is not a mesh field; expected one of {KNOWN_MESH_KEYS}"
            ),
        ),
        (
            "unknown entry key",
            r#"weaponMounts: [{ weapon: "reference_rifle", attachments: { hand_r: "models/rifle/exo_red.gltf" }, socket: "hand_r" }]"#,
            r#"weaponMounts = {{ weapon = "reference_rifle", attachments = { hand_r = "models/rifle/exo_red.gltf" }, socket = "hand_r" }}"#,
            "entity `player`: `components.mesh.weaponMounts[0].socket` is not a weapon-mount field; expected `weapon` and `attachments`".to_string(),
        ),
        (
            "duplicated weapon",
            r#"weaponMounts: [
                { weapon: "reference_rifle", attachments: { hand_r: "models/rifle/exo_red.gltf" } },
                { weapon: "reference_rifle", attachments: { hand_r: "models/rifle/other.gltf" } },
            ]"#,
            r#"weaponMounts = {
                { weapon = "reference_rifle", attachments = { hand_r = "models/rifle/exo_red.gltf" } },
                { weapon = "reference_rifle", attachments = { hand_r = "models/rifle/other.gltf" } },
            }"#,
            "entity `player`: `components.mesh.weaponMounts[1].weapon` repeats weapon `reference_rifle`; each weapon takes one entry".to_string(),
        ),
        (
            "empty attachment map",
            r#"weaponMounts: [{ weapon: "reference_rifle", attachments: {} }]"#,
            r#"weaponMounts = {{ weapon = "reference_rifle", attachments = {} }}"#,
            "entity `player`: `components.mesh.weaponMounts[0].attachments` for weapon `reference_rifle` is empty; an entry mounts at least one socket".to_string(),
        ),
        (
            "empty weapon name",
            r#"weaponMounts: [{ weapon: "", attachments: { hand_r: "models/rifle/exo_red.gltf" } }]"#,
            r#"weaponMounts = {{ weapon = "", attachments = { hand_r = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.weaponMounts[0].weapon` must be a non-empty weapon name".to_string(),
        ),
        (
            "empty socket name",
            r#"weaponMounts: [{ weapon: "reference_rifle", attachments: { "": "models/rifle/exo_red.gltf" } }]"#,
            r#"weaponMounts = {{ weapon = "reference_rifle", attachments = { [""] = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.weaponMounts[0].attachments` must not contain an empty socket name".to_string(),
        ),
        (
            "weapon still a handle",
            r#"weaponMounts: [{ weapon: { canonicalName: "reference_rifle" }, attachments: { hand_r: "models/rifle/exo_red.gltf" } }]"#,
            r#"weaponMounts = {{ weapon = { canonicalName = "reference_rifle" }, attachments = { hand_r = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.weaponMounts[0].weapon` must be a weapon name; `defineEntity` lowers a weapon descriptor to its canonical name".to_string(),
        ),
        (
            "table not a list",
            r#"weaponMounts: { reference_rifle: { hand_r: "models/rifle/exo_red.gltf" } }"#,
            r#"weaponMounts = { reference_rifle = { hand_r = "models/rifle/exo_red.gltf" } }"#,
            "entity `player`: `components.mesh.weaponMounts` must be an array of `{ weapon, attachments }` entries".to_string(),
        ),
        (
            "entry not an object",
            r#"weaponMounts: ["reference_rifle"]"#,
            r#"weaponMounts = { "reference_rifle" }"#,
            "entity `player`: `components.mesh.weaponMounts[0]` must be an object `{ weapon, attachments }`".to_string(),
        ),
    ];
    for (label, js_fields, lua_fields, expected) in cases {
        let js = js_error(&js_player(js_fields));
        let lua = lua_error(&lua_player(lua_fields));
        assert!(
            js.contains(&expected),
            "QuickJS {label}: expected {expected:?}, got {js}"
        );
        assert_eq!(js, lua, "{label}: both runtimes reject with one message");
    }
}

// A model path that breaks the content-relative rule weapon models follow
// rejects load in both runtimes, as `components.weapon.viewmodel` does.
#[test]
fn weapon_mount_model_paths_follow_the_weapon_model_path_rule() {
    let expected = "entity `player`: `components.mesh.weaponMounts[0].attachments.hand_r` must be a non-empty, content-relative model path using forward slashes with no parent traversal";
    for path in [
        "",
        "../models/rifle/exo_red.gltf",
        "models/../../rifle.gltf",
        "/models/rifle/exo_red.gltf",
        "C:/models/rifle/exo_red.gltf",
        r"models\\rifle\\exo_red.gltf",
    ] {
        let js = js_error(&js_player(&format!(
            r#"weaponMounts: [{{ weapon: "reference_rifle", attachments: {{ hand_r: "{path}" }} }}]"#
        )));
        let lua = lua_error(&lua_player(&format!(
            r#"weaponMounts = {{ {{ weapon = "reference_rifle", attachments = {{ hand_r = "{path}" }} }} }}"#
        )));
        assert!(js.contains(expected), "{path:?}: {js}");
        assert_eq!(js, lua, "{path:?}");
    }
}

// Every other mesh key also rejects, and a descriptor without a canonical name
// is named by its absence. The mesh keys content uses today still parse.
#[test]
fn unknown_mesh_keys_reject_in_both_runtimes_and_known_ones_parse() {
    let js = js_error(r#"({ components: { mesh: { model: "m", thirdPersonModel: "x" } } })"#);
    let lua =
        lua_error(r#"return { components = { mesh = { model = "m", thirdPersonModel = "x" } } }"#);
    assert!(
        js.contains(&format!(
            "an entity without a canonicalName: `components.mesh.thirdPersonModel` is not a mesh field; expected one of {KNOWN_MESH_KEYS}"
        )),
        "{js}"
    );
    assert_eq!(js, lua);

    let js_known = eval_js(
        &js_player(
            r#"attachments: { hand_l: "models/shield.gltf" }, shadowBiasScale: 2, shadowOnly: true,
               animations: { idle: { clip: "idle", loop: true } }, defaultState: "idle",
               locomotion: { speedScale: false }, weaponMounts: []"#,
        ),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua_known = eval_lua(
        &lua_player(
            r#"attachments = { hand_l = "models/shield.gltf" }, shadowBiasScale = 2, shadowOnly = true,
               animations = { idle = { clip = "idle", loop = true } }, defaultState = "idle",
               locomotion = { speedScale = false }, weaponMounts = {}"#,
        ),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    assert_eq!(js_known, lua_known);
}
