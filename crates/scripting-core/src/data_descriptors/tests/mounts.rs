// Tests: `components.mesh` key set and the `mounts` table, parsed by
// both descriptor readers.
// See: context/lib/entity_model.md §2 (MeshComponent)

use super::super::*;
use super::common::*;
use postretro_entities::data_descriptors::MountDescriptor;

const KNOWN_MESH_KEYS: &str =
    "model, attachments, shadowBiasScale, shadowOnly, animations, defaultState, locomotion, mounts";

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

fn mount(weapon: &str, attachments: &[(&str, &str)]) -> MountDescriptor {
    MountDescriptor {
        item: weapon.to_string(),
        attachments: Some(
            attachments
                .iter()
                .map(|(socket, model)| (socket.to_string(), model.to_string()))
                .collect(),
        ),
        muzzle_socket: None,
    }
}

#[test]
fn weapon_mounts_parse_identically_in_both_runtimes() {
    let js = eval_js(
        &js_player(
            r#"mounts: [
                { item: "reference_rifle", attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
                { item: "reference_pistol", attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf", back: "models/holster/exo_red.gltf" } },
            ]"#,
        ),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_player(
            r#"mounts = {
                { item = "reference_rifle", attachments = { hand_r = "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
                { item = "reference_pistol", attachments = { hand_r = "models/cyberpunk_weapons/pistol/exo_red.gltf", back = "models/holster/exo_red.gltf" } },
            }"#,
        ),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    assert_eq!(js, lua, "both runtimes produce one descriptor");
    let mesh = js.mesh.expect("mesh parses");
    assert_eq!(
        mesh.mounts,
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
        mesh.mount("reference_pistol")
            .map(|entry| entry.attachments.as_ref().map(|a| a.len())),
        Some(Some(2))
    );
    assert!(mesh.mount("reference_shotgun").is_none());
    let mut models: Vec<_> = mesh.mount_models().collect();
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
    let undefined = eval_js(&js_player("mounts: undefined"), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let lua_absent = eval_lua(&lua_player(""), |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    for descriptor in [&absent, &undefined, &lua_absent] {
        assert_eq!(descriptor.mesh.as_ref().unwrap().mounts, None);
    }
    let js_empty = eval_js(&js_player("mounts: []"), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let lua_empty = eval_lua(&lua_player("mounts = {}"), |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    assert_eq!(js_empty.mesh.unwrap().mounts, Some(Vec::new()));
    assert_eq!(lua_empty.mesh.unwrap().mounts, Some(Vec::new()));
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
            r#"mounts: [{ item: "reference_rifle", attachments: { hand_r: "models/rifle/exo_red.gltf" }, socket: "hand_r" }]"#,
            r#"mounts = {{ item = "reference_rifle", attachments = { hand_r = "models/rifle/exo_red.gltf" }, socket = "hand_r" }}"#,
            "entity `player`: `components.mesh.mounts[0].socket` is not a weapon-mount field; expected `item`, `attachments` or `muzzleSocket`".to_string(),
        ),
        (
            "duplicated weapon",
            r#"mounts: [
                { item: "reference_rifle", attachments: { hand_r: "models/rifle/exo_red.gltf" } },
                { item: "reference_rifle", attachments: { hand_r: "models/rifle/other.gltf" } },
            ]"#,
            r#"mounts = {
                { item = "reference_rifle", attachments = { hand_r = "models/rifle/exo_red.gltf" } },
                { item = "reference_rifle", attachments = { hand_r = "models/rifle/other.gltf" } },
            }"#,
            "entity `player`: `components.mesh.mounts[1].item` repeats weapon `reference_rifle`; each weapon takes one entry".to_string(),
        ),
        (
            "empty attachment map",
            r#"mounts: [{ item: "reference_rifle", attachments: {} }]"#,
            r#"mounts = {{ item = "reference_rifle", attachments = {} }}"#,
            "entity `player`: `components.mesh.mounts[0].attachments` for weapon `reference_rifle` is empty; an entry mounts at least one socket".to_string(),
        ),
        (
            "empty weapon name",
            r#"mounts: [{ item: "", attachments: { hand_r: "models/rifle/exo_red.gltf" } }]"#,
            r#"mounts = {{ item = "", attachments = { hand_r = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.mounts[0].item` must be a non-empty weapon name".to_string(),
        ),
        (
            "empty socket name",
            r#"mounts: [{ item: "reference_rifle", attachments: { "": "models/rifle/exo_red.gltf" } }]"#,
            r#"mounts = {{ item = "reference_rifle", attachments = { [""] = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.mounts[0].attachments` must not contain an empty socket name".to_string(),
        ),
        (
            "weapon still a handle",
            r#"mounts: [{ item: { canonicalName: "reference_rifle" }, attachments: { hand_r: "models/rifle/exo_red.gltf" } }]"#,
            r#"mounts = {{ item = { canonicalName = "reference_rifle" }, attachments = { hand_r = "models/rifle/exo_red.gltf" } }}"#,
            "entity `player`: `components.mesh.mounts[0].item` must be a weapon name; `defineEntity` lowers a weapon descriptor to its canonical name".to_string(),
        ),
        (
            "table not a list",
            r#"mounts: { reference_rifle: { hand_r: "models/rifle/exo_red.gltf" } }"#,
            r#"mounts = { reference_rifle = { hand_r = "models/rifle/exo_red.gltf" } }"#,
            "entity `player`: `components.mesh.mounts` must be an array of `{ item, attachments }` entries".to_string(),
        ),
        (
            "entry not an object",
            r#"mounts: ["reference_rifle"]"#,
            r#"mounts = { "reference_rifle" }"#,
            "entity `player`: `components.mesh.mounts[0]` must be an object `{ item, attachments }`".to_string(),
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

// The table was renamed with no alias: the retired mesh key and the retired
// entry key reject in both runtimes, naming the key and the character, and
// the same table under the new names loads.
#[test]
fn retired_mount_table_keys_reject_while_the_renamed_table_loads_in_both_runtimes() {
    let model = "models/rifle/exo_red.gltf";
    let cases = [
        (
            "retired mesh key",
            format!(r#"weaponMounts: [{{ item: "reference_rifle", attachments: {{ hand_r: "{model}" }} }}]"#),
            format!(r#"weaponMounts = {{ {{ item = "reference_rifle", attachments = {{ hand_r = "{model}" }} }} }}"#),
            format!(
                "entity `player`: `components.mesh.weaponMounts` is not a mesh field; expected one of {KNOWN_MESH_KEYS}"
            ),
        ),
        (
            "retired entry key",
            format!(r#"mounts: [{{ weapon: "reference_rifle", attachments: {{ hand_r: "{model}" }} }}]"#),
            format!(r#"mounts = {{ {{ weapon = "reference_rifle", attachments = {{ hand_r = "{model}" }} }} }}"#),
            "entity `player`: `components.mesh.mounts[0].weapon` is not a weapon-mount field; expected `item`, `attachments` or `muzzleSocket`".to_string(),
        ),
    ];
    for (label, js_fields, lua_fields, expected) in cases {
        let js = js_error(&js_player(&js_fields));
        let lua = lua_error(&lua_player(&lua_fields));
        assert!(
            js.contains(&expected),
            "QuickJS {label}: expected {expected:?}, got {js}"
        );
        assert_eq!(js, lua, "{label}: both runtimes reject with one message");
    }

    let js = eval_js(
        &js_player(&format!(
            r#"mounts: [{{ item: "reference_rifle", attachments: {{ hand_r: "{model}" }} }}]"#
        )),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_player(&format!(
            r#"mounts = {{ {{ item = "reference_rifle", attachments = {{ hand_r = "{model}" }} }} }}"#
        )),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let expected = Some(vec![mount("reference_rifle", &[("hand_r", model)])]);
    assert_eq!(js.mesh.unwrap().mounts, expected);
    assert_eq!(lua.mesh.unwrap().mounts, expected);
}

// An entry takes exactly one of `attachments` and `muzzleSocket`; each shape
// loads, and a muzzle entry carries no attachments.
#[test]
fn an_entry_takes_exactly_one_of_attachments_and_muzzle_socket_in_both_runtimes() {
    let model = "models/rifle/exo_red.gltf";
    let cases = [
        (
            "both",
            format!(
                r#"mounts: [{{ item: "arm_cannon", attachments: {{ hand_r: "{model}" }}, muzzleSocket: "cannon_tip" }}]"#
            ),
            format!(
                r#"mounts = {{ {{ item = "arm_cannon", attachments = {{ hand_r = "{model}" }}, muzzleSocket = "cannon_tip" }} }}"#
            ),
            "entity `player`: `components.mesh.mounts[0]` names both `attachments` and `muzzleSocket`; an entry takes exactly one",
        ),
        (
            "neither",
            r#"mounts: [{ item: "arm_cannon" }]"#.to_string(),
            r#"mounts = { { item = "arm_cannon" } }"#.to_string(),
            "entity `player`: `components.mesh.mounts[0]` names neither `attachments` nor `muzzleSocket`; an entry takes exactly one",
        ),
        (
            "empty muzzle socket",
            r#"mounts: [{ item: "arm_cannon", muzzleSocket: "" }]"#.to_string(),
            r#"mounts = { { item = "arm_cannon", muzzleSocket = "" } }"#.to_string(),
            "entity `player`: `components.mesh.mounts[0].muzzleSocket` must be a non-empty socket name",
        ),
        (
            "non-string muzzle socket",
            r#"mounts: [{ item: "arm_cannon", muzzleSocket: 7 }]"#.to_string(),
            r#"mounts = { { item = "arm_cannon", muzzleSocket = 7 } }"#.to_string(),
            "entity `player`: `components.mesh.mounts[0].muzzleSocket` must be a socket name string",
        ),
        (
            "present and empty attachments",
            r#"mounts: [{ item: "arm_cannon", attachments: {} }]"#.to_string(),
            r#"mounts = { { item = "arm_cannon", attachments = {} } }"#.to_string(),
            "entity `player`: `components.mesh.mounts[0].attachments` for weapon `arm_cannon` is empty; an entry mounts at least one socket",
        ),
    ];
    for (label, js_fields, lua_fields, expected) in cases {
        let js = js_error(&js_player(&js_fields));
        let lua = lua_error(&lua_player(&lua_fields));
        assert!(
            js.contains(expected),
            "QuickJS {label}: expected {expected:?}, got {js}"
        );
        assert_eq!(js, lua, "{label}: both runtimes reject with one message");
    }

    let js = eval_js(
        &js_player(&format!(
            r#"mounts: [{{ item: "arm_cannon", muzzleSocket: "cannon_tip" }}, {{ item: "rifle", attachments: {{ hand_r: "{model}" }} }}]"#
        )),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_player(&format!(
            r#"mounts = {{ {{ item = "arm_cannon", muzzleSocket = "cannon_tip" }}, {{ item = "rifle", attachments = {{ hand_r = "{model}" }} }} }}"#
        )),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let expected = Some(vec![
        MountDescriptor {
            item: "arm_cannon".to_string(),
            attachments: None,
            muzzle_socket: Some("cannon_tip".to_string()),
        },
        mount("rifle", &[("hand_r", model)]),
    ]);
    assert_eq!(js.mesh.as_ref().unwrap().mounts, expected);
    assert_eq!(lua.mesh.as_ref().unwrap().mounts, expected);
    let mesh = js.mesh.unwrap();
    assert_eq!(mesh.mount_models().collect::<Vec<_>>(), vec![model]);
}

// A model path that breaks the content-relative rule weapon models follow
// rejects load in both runtimes, as `components.weapon.viewmodel` does.
#[test]
fn weapon_mount_model_paths_follow_the_weapon_model_path_rule() {
    let expected = "entity `player`: `components.mesh.mounts[0].attachments.hand_r` must be a non-empty, content-relative model path using forward slashes with no parent traversal";
    for path in [
        "",
        "../models/rifle/exo_red.gltf",
        "models/../../rifle.gltf",
        "/models/rifle/exo_red.gltf",
        "C:/models/rifle/exo_red.gltf",
        r"models\\rifle\\exo_red.gltf",
    ] {
        let js = js_error(&js_player(&format!(
            r#"mounts: [{{ item: "reference_rifle", attachments: {{ hand_r: "{path}" }} }}]"#
        )));
        let lua = lua_error(&lua_player(&format!(
            r#"mounts = {{ {{ item = "reference_rifle", attachments = {{ hand_r = "{path}" }} }} }}"#
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
               locomotion: { speedScale: false }, mounts: []"#,
        ),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua_known = eval_lua(
        &lua_player(
            r#"attachments = { hand_l = "models/shield.gltf" }, shadowBiasScale = 2, shadowOnly = true,
               animations = { idle = { clip = "idle", loop = true } }, defaultState = "idle",
               locomotion = { speedScale = false }, mounts = {}"#,
        ),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    assert_eq!(js_known, lua_known);
}

// `thirdPersonModel` is retired: the character's `mounts` table is the
// only third-person source, so a weapon carrying the old field is rejected as
// an unknown field rather than silently ignored.
#[test]
fn retired_third_person_model_rejects_as_an_unknown_weapon_field_in_both_runtimes() {
    let js = js_error(
        r#"({ components: { weapon: {
            damage: 12, range: 64, primary: { trigger: "press", recoveryMs: 180, steps: [{ kind: "shot" }] }, resolution: "hitscan",
            thirdPersonModel: "models/smg/model.gltf"
        } } })"#,
    );
    let lua = lua_error(
        r#"return { components = { weapon = {
            damage = 12, range = 64, primary = { trigger = "press", recoveryMs = 180, steps = { { kind = "shot" } } }, resolution = "hitscan",
            thirdPersonModel = "models/smg/model.gltf"
        } } }"#,
    );
    for error in [&js, &lua] {
        assert!(
            error.contains("unknown field `thirdPersonModel`"),
            "{error}"
        );
    }
}
