// Tests: map-catalog drain parsing.

use super::super::*;
use super::common::*;

#[test]
fn drain_maps_js_defaults_absent_and_null_tags_to_empty() {
    let maps = eval_js(
        r#"({
            maps: [
                { id: "valid", path: "maps/valid.prl", name: "Valid", tags: ["campaign"] },
                { id: "", path: "maps/empty-id.prl", name: "Empty Id", tags: ["broken"] },
                { id: "absolute", path: "/tmp/escape.prl", name: "Absolute", tags: ["broken"] },
                { id: "parent", path: "../escape.prl", name: "Parent", tags: ["broken"] },
                { id: "nestedParent", path: "maps/../escape.prl", name: "Nested Parent", tags: ["broken"] },
                { id: "nullTags", path: "maps/null-tags.prl", name: "Null Tags", tags: null },
                { id: "absentTags", path: "maps/absent-tags.prl", name: "Absent Tags" },
                { id: "alsoValid", path: "maps/also-valid.prl", name: "Also Valid", tags: [] },
            ],
        })"#,
        |_ctx, v| {
            let obj = Object::from_value(v).expect("manifest must be an object");
            drain_maps_js(&obj, "test").expect("malformed map entries should degrade")
        },
    );

    assert_eq!(
        maps,
        vec![
            ModMapEntry {
                id: "valid".to_string(),
                path: "maps/valid.prl".to_string(),
                name: "Valid".to_string(),
                tags: vec!["campaign".to_string()],
            },
            ModMapEntry {
                id: "nullTags".to_string(),
                path: "maps/null-tags.prl".to_string(),
                name: "Null Tags".to_string(),
                tags: Vec::new(),
            },
            ModMapEntry {
                id: "absentTags".to_string(),
                path: "maps/absent-tags.prl".to_string(),
                name: "Absent Tags".to_string(),
                tags: Vec::new(),
            },
            ModMapEntry {
                id: "alsoValid".to_string(),
                path: "maps/also-valid.prl".to_string(),
                name: "Also Valid".to_string(),
                tags: Vec::new(),
            },
        ]
    );
}

#[test]
fn drain_maps_lua_keeps_dense_prefix_and_skips_non_prefix_entries() {
    // Regression: keyed/sparse Luau catalog tables were either silently omitted
    // or caused the whole catalog to be dropped.
    let cases = [
        (
            r#"return {
                maps = {
                    { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                    extra = { id = "dm1", path = "maps/dm1.prl", name = "Arena", tags = { "deathmatch" } },
                },
            }"#,
            "extra",
        ),
        (
            r#"return {
                maps = {
                    { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                    [3] = { id = "dm1", path = "maps/dm1.prl", name = "Arena", tags = { "deathmatch" } },
                },
            }"#,
            "sparse",
        ),
        (
            r#"return {
                maps = {
                    { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                    [0] = { id = "zero", path = "maps/zero.prl", name = "Zero", tags = {} },
                },
            }"#,
            "zero",
        ),
        (
            r#"return {
                maps = {
                    { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                    [1.5] = { id = "float", path = "maps/float.prl", name = "Float", tags = {} },
                },
            }"#,
            "float",
        ),
    ];

    for (source, label) in cases {
        let maps = eval_lua(source, |v| {
            let table = lua_table(v, "manifest").expect("manifest must be a table");
            drain_maps_lua(&table, "test").expect("malformed maps field should degrade")
        });
        assert_eq!(
            maps,
            vec![ModMapEntry {
                id: "e1m1".to_string(),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: vec!["campaign".to_string()],
            }],
            "{label} case should keep the dense prefix"
        );
    }
}

#[test]
fn drain_maps_lua_skips_map_shaped_catalog_without_dense_prefix() {
    let maps = eval_lua(
        r#"return {
            maps = {
                named = { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                [2] = { id = "dm1", path = "maps/dm1.prl", name = "Arena", tags = { "deathmatch" } },
            },
        }"#,
        |v| {
            let table = lua_table(v, "manifest").expect("manifest must be a table");
            drain_maps_lua(&table, "test").expect("malformed maps field should degrade")
        },
    );

    assert!(maps.is_empty(), "no dense prefix means no catalog entries");
}

#[test]
fn drain_maps_lua_defaults_absent_and_nil_tags_to_empty() {
    let maps = eval_lua(
        r#"return {
            maps = {
                { id = "valid", path = "maps/valid.prl", name = "Valid", tags = { "campaign" } },
                { id = "keyedTags", path = "maps/keyed.prl", name = "Keyed", tags = { named = "campaign" } },
                { id = "sparseTags", path = "maps/sparse.prl", name = "Sparse", tags = { [2] = "campaign" } },
                { id = "nilTags", path = "maps/nil.prl", name = "Nil", tags = nil },
                { id = "absentTags", path = "maps/absent-tags.prl", name = "Absent Tags" },
                { id = "alsoValid", path = "maps/also-valid.prl", name = "Also Valid", tags = {} },
            },
        }"#,
        |v| {
            let table = lua_table(v, "manifest").expect("manifest must be a table");
            drain_maps_lua(&table, "test").expect("malformed map entries should degrade")
        },
    );

    assert_eq!(
        maps,
        vec![
            ModMapEntry {
                id: "valid".to_string(),
                path: "maps/valid.prl".to_string(),
                name: "Valid".to_string(),
                tags: vec!["campaign".to_string()],
            },
            ModMapEntry {
                id: "nilTags".to_string(),
                path: "maps/nil.prl".to_string(),
                name: "Nil".to_string(),
                tags: Vec::new(),
            },
            ModMapEntry {
                id: "absentTags".to_string(),
                path: "maps/absent-tags.prl".to_string(),
                name: "Absent Tags".to_string(),
                tags: Vec::new(),
            },
            ModMapEntry {
                id: "alsoValid".to_string(),
                path: "maps/also-valid.prl".to_string(),
                name: "Also Valid".to_string(),
                tags: Vec::new(),
            },
        ]
    );
}
