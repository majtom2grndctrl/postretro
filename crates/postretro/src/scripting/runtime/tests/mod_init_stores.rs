// Mod-init store reconciliation, bundling, and imported-domain manifests.

use super::*;

#[test]
#[cfg(debug_assertions)]
fn mod_init_typescript_import_get_game_state_bundles_and_executes() {
    if !install_scripts_build_next_to_current_exe() {
        eprintln!("skipping: could not install scripts-build next to test binary");
        return;
    }

    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("ts_game_state");
    fs::write(
        dir.join("start-script.ts"),
        r#"
            import { getGameState } from "postretro";

            const refs = getGameState();
            if (refs.player.health.slot !== "player.health") {
              throw new Error("bad health slot");
            }
            if ((globalThis as any).__postretroGameStateRefs !== undefined) {
              throw new Error("bridge global leaked");
            }

            export default { name: "TypeScriptGameStateMod" };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    assert_eq!(rt.mod_manifest().unwrap().name, "TypeScriptGameStateMod");

    let bundled = fs::read_to_string(dir.join("start-script.js")).unwrap();
    assert!(
        !bundled.contains("from \"postretro\"") && !bundled.contains("from 'postretro'"),
        "bundled output must strip the postretro import: {bundled}"
    );
    assert!(
        bundled.contains("getGameState"),
        "bundled output must preserve the getGameState call site: {bundled}"
    );
}

#[test]
fn failed_mod_init_rolls_back_quickjs_and_luau_store_declarations() {
    for (name, file_name, source) in [
        (
            "js",
            "start-script.js",
            r#"
                const attempt = defineStore("attempt", {
                    value: { type: "number", default: 1 },
                });
                globalThis.__postretroModManifest = { stores: [attempt.declaration] };
                "#,
        ),
        (
            "luau",
            "start-script.luau",
            r#"
                local attempt = defineStore("attempt", {
                    value = { type = "number", default = 1 },
                })
                return { stores = { attempt.declaration } }
                "#,
        ),
    ] {
        let (mut rt, ctx) = runtime();
        let dir = temp_mod_root(&format!("{name}_store_rollback"));
        fs::write(dir.join(file_name), source).unwrap();

        rt.run_mod_init(&dir)
            .expect_err("missing manifest name must fail");
        assert!(
            ctx.slot_table.borrow().get("attempt.value").is_none(),
            "{name} declaration leaked from failed mod init"
        );
    }
}

#[test]
fn unreturned_store_declaration_does_not_commit() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("unreturned_store");
    fs::write(
        dir.join("start-script.js"),
        r#"
            const attempt = defineStore("attempt", {
                value: { type: "number", default: 1 },
            });
            globalThis.__postretroModManifest = { name: "Unreturned" };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    assert!(ctx.slot_table.borrow().get("attempt.value").is_none());
}

#[test]
fn repeated_mod_init_preserves_live_store_values() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("repeat_store");
    fs::write(
            dir.join("start-script.js"),
            r#"
            const session = defineStore("session", {
                volume: { type: "number", default: 1, persist: true },
            });
            globalThis.__postretroModManifest = { name: "RepeatStore", stores: [session.declaration] };
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    crate::scripting::primitives::store::write_store_slot(
        &ctx,
        "session.volume",
        crate::scripting::slot_table::SlotValue::Number(0.25),
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    assert_eq!(
        crate::scripting::primitives::store::read_store_slot(&ctx, "session.volume").unwrap(),
        crate::scripting::slot_table::SlotValue::Number(0.25)
    );
}

#[test]
fn mod_init_quickjs_imported_domain_script_manifest_carries_entity_descriptor() {
    // Acceptance criterion: an entity type defined in a domain script that
    // was bundled into start-script.js by `scripts-build` (not defined
    // directly in start-script itself) is carried on the mod manifest
    // after mod-init. `scripts-build` inlines all imports at build time,
    // so the fixture is a single JS file whose intent — a descriptor
    // exported from a bundled domain script and aggregated into the
    // default manifest export — is made explicit by the inlined-comment
    // markers.
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_imported_domain");
    std::fs::write(
            dir.join("start-script.js"),
            r#"
            /* inlined from actors/player.ts */
            const playerEntity = { canonicalName: "smoke_pillar" };
            /* end inlined actors/player.ts */
            globalThis.__postretroModManifest = { name: "ImportedDomainMod", entities: [playerEntity] };
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "ImportedDomainMod");
    assert!(
        manifest
            .entities
            .iter()
            .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
        "entity type from bundled domain script must appear on the mod manifest"
    );
}

#[test]
fn mod_init_luau_manifest_carries_entity_descriptor() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_register");
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            return {
                name = "TestMod",
                entities = { { canonicalName = "smoke_pillar" } },
            }
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
        "the returned mod manifest's `entities` field must carry the descriptor"
    );
}
