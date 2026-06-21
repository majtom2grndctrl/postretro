// Mod-init manifest parsing: descriptors, reactions, crossings, catalogs.

use super::*;

#[test]
#[cfg(debug_assertions)]
fn mod_init_missing_start_script_debug_returns_none() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("missing");
    rt.run_mod_init(&dir).unwrap();
    assert!(rt.mod_manifest().is_none());
}

#[test]
#[cfg(not(debug_assertions))]
fn mod_init_missing_start_script_release_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("missing_release");
    let err = rt
        .run_mod_init(&dir)
        .expect_err("release builds must require a start-script");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(
                reason.contains("no `start-script"),
                "expected missing-start-script error, got: {reason}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    assert!(rt.mod_manifest().is_none());
}

#[test]
fn mod_init_quickjs_manifest_carries_entity_descriptor() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_register");
    // start-script.js exports a default manifest carrying a player entity
    // descriptor. Boot-side ingestion drains the field into
    // `DataRegistry`; this test asserts the manifest shape.
    std::fs::write(
        dir.join("start-script.js"),
        r#"
            globalThis.__postretroModManifest = {
                name: "TestMod",
                entities: [{ canonicalName: "smoke_pillar" }],
            };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "TestMod");
    assert!(
        manifest
            .entities
            .iter()
            .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
        "the default manifest export's `entities` field must carry the descriptor"
    );
}

#[test]
fn mod_init_quickjs_manifest_carries_global_reactions_and_crossings() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_global_reactions");
    fs::write(
        dir.join("start-script.js"),
        r#"
            globalThis.__postretroModManifest = {
                name: "GlobalBehavior",
                reactions: scopeReactions(["campaign"], [
                    defineReaction("levelLoad", {
                        primitive: "moveGeometry",
                        tag: "reactor",
                    }),
                    defineReaction("objectiveLoad", {
                        primitive: "playSound",
                        args: { sound: "objective" },
                    }),
                ]),
                crossings: [
                    {
                        slot: "player.health",
                        below: 25,
                        max: 100,
                        fire: ["lowHealth"],
                        levels: ["campaign"],
                    },
                ],
            };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.reactions.len(), 2);
    assert_eq!(manifest.reactions[0].reaction.name, "levelLoad");
    assert_eq!(manifest.reactions[0].levels, vec!["campaign"]);
    assert_eq!(manifest.reactions[1].reaction.name, "objectiveLoad");
    assert_eq!(manifest.reactions[1].levels, vec!["campaign"]);
    assert_eq!(manifest.crossings.len(), 1);
    assert_eq!(manifest.crossings[0].crossing.slot, "player.health");
    assert_eq!(manifest.crossings[0].crossing.fire, vec!["lowHealth"]);
    assert_eq!(manifest.crossings[0].levels, vec!["campaign"]);
}

#[test]
fn mod_init_luau_manifest_carries_global_reactions_and_crossings() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_global_reactions");
    fs::write(
        dir.join("start-script.luau"),
        r#"
            return {
                name = "GlobalBehavior",
                reactions = scopeReactions({ "campaign" }, {
                    defineReaction("levelLoad", {
                        primitive = "moveGeometry",
                        tag = "reactor",
                    }),
                    defineReaction("objectiveLoad", {
                        primitive = "playSound",
                        args = { sound = "objective" },
                    }),
                }),
                crossings = {
                    {
                        slot = "player.health",
                        below = 25,
                        max = 100,
                        fire = { "lowHealth" },
                        levels = { "campaign" },
                    },
                },
            }
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.reactions.len(), 2);
    assert_eq!(manifest.reactions[0].reaction.name, "levelLoad");
    assert_eq!(manifest.reactions[0].levels, vec!["campaign"]);
    assert_eq!(manifest.reactions[1].reaction.name, "objectiveLoad");
    assert_eq!(manifest.reactions[1].levels, vec!["campaign"]);
    assert_eq!(manifest.crossings.len(), 1);
    assert_eq!(manifest.crossings[0].crossing.slot, "player.health");
    assert_eq!(manifest.crossings[0].crossing.fire, vec!["lowHealth"]);
    assert_eq!(manifest.crossings[0].levels, vec!["campaign"]);
}

#[test]
fn mod_init_quickjs_get_game_state_executes_and_hides_bridge() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_game_state");
    fs::write(
        dir.join("start-script.js"),
        r#"
            const refs = getGameState();
            if (getGameState() !== refs) throw new Error("getGameState must be idempotent");
            if (refs.player.health.slot !== "player.health") throw new Error("bad health slot");
            if (typeof globalThis.__postretroGameStateRefs !== "undefined")
                throw new Error("bridge global leaked");
            globalThis.__postretroModManifest = { name: "GameStateMod" };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    assert_eq!(rt.mod_manifest().unwrap().name, "GameStateMod");
}

#[test]
fn mod_init_luau_get_game_state_executes_and_hides_bridge() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_game_state");
    fs::write(
        dir.join("start-script.luau"),
        r#"
            local refs = getGameState()
            assert(getGameState() == refs, "getGameState must be idempotent")
            assert(refs.player.health.slot == "player.health", "bad health slot")
            assert(type(__postretroGameStateRefs) == "nil", "bridge global leaked")
            return { name = "GameStateMod" }
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    assert_eq!(rt.mod_manifest().unwrap().name, "GameStateMod");
}

#[test]
fn mod_init_quickjs_define_mod_and_map_catalog_are_identity_helpers() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_define_mod");
    fs::write(
        dir.join("start-script.js"),
        r#"
            const maps = defineMapCatalog([
                { id: "e1m1", path: "maps/e1m1.prl", name: "Entryway", tags: ["campaign"] },
            ]);
            if (maps[0].id !== "e1m1" || maps[0].tags[0] !== "campaign") {
                throw new Error("defineMapCatalog changed entry wire data");
            }
            const manifest = {
                name: "CatalogMod",
                fonts: { primary: "fonts/inter.ttf" },
                maps,
            };
            const defined = defineMod(manifest);
            if (defined !== manifest || defined.maps !== maps) {
                throw new Error("defineMod or defineMapCatalog must be identity helpers");
            }
            globalThis.__postretroModManifest = defined;
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "CatalogMod");
    assert_eq!(manifest.fonts.families["primary"], "fonts/inter.ttf");
    assert_eq!(
        manifest.maps,
        vec![ModMapEntry {
            id: "e1m1".to_string(),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: vec!["campaign".to_string()],
        }]
    );
}

#[test]
fn mod_init_luau_define_mod_and_map_catalog_are_identity_helpers() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_define_mod");
    fs::write(
            dir.join("start-script.luau"),
            r#"
            local maps = defineMapCatalog({
                { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
            })
            assert(maps[1].id == "e1m1" and maps[1].tags[1] == "campaign", "defineMapCatalog changed entry wire data")
            local manifest = {
                name = "CatalogMod",
                fonts = { primary = "fonts/inter.ttf" },
                maps = maps,
            }
            local defined = defineMod(manifest)
            assert(defined == manifest and defined.maps == maps, "defineMod or defineMapCatalog must be identity helpers")
            return defined
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "CatalogMod");
    assert_eq!(manifest.fonts.families["primary"], "fonts/inter.ttf");
    assert_eq!(
        manifest.maps,
        vec![ModMapEntry {
            id: "e1m1".to_string(),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: vec!["campaign".to_string()],
        }]
    );
}

#[test]
fn mod_init_map_catalog_skips_empty_paths_and_duplicate_ids() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_map_catalog_validation");
    fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = {
                name: "CatalogValidation",
                maps: [
                    { id: "e1m1", path: "maps/e1m1.prl", name: "Entryway", tags: ["campaign"] },
                    { id: "empty", path: "", name: "Empty", tags: ["broken"] },
                    { id: "e1m1", path: "maps/duplicate.prl", name: "Duplicate", tags: ["duplicate"] },
                    { id: "dm1", path: "maps/dm1.prl", name: "Arena", tags: ["deathmatch"] },
                ],
            };
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();

    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(
        manifest.maps,
        vec![
            ModMapEntry {
                id: "e1m1".to_string(),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: vec!["campaign".to_string()],
            },
            ModMapEntry {
                id: "dm1".to_string(),
                path: "maps/dm1.prl".to_string(),
                name: "Arena".to_string(),
                tags: vec!["deathmatch".to_string()],
            },
        ]
    );
}

#[test]
fn mod_init_luau_map_catalog_skips_empty_paths_and_duplicate_ids() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_map_catalog_validation");
    fs::write(
            dir.join("start-script.luau"),
            r#"
            return {
                name = "CatalogValidation",
                maps = {
                    { id = "e1m1", path = "maps/e1m1.prl", name = "Entryway", tags = { "campaign" } },
                    { id = "empty", path = "", name = "Empty", tags = { "broken" } },
                    { id = "e1m1", path = "maps/duplicate.prl", name = "Duplicate", tags = { "duplicate" } },
                    { id = "dm1", path = "maps/dm1.prl", name = "Arena", tags = { "deathmatch" } },
                },
            }
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();

    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(
        manifest.maps,
        vec![
            ModMapEntry {
                id: "e1m1".to_string(),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: vec!["campaign".to_string()],
            },
            ModMapEntry {
                id: "dm1".to_string(),
                path: "maps/dm1.prl".to_string(),
                name: "Arena".to_string(),
                tags: vec!["deathmatch".to_string()],
            },
        ]
    );
}
