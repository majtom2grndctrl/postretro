// Mod-init structural error paths and entities-field validation.

use super::*;

#[test]
fn mod_init_quickjs_missing_default_manifest_export_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("no_setup");
    std::fs::write(dir.join("start-script.js"), "var x = 1;\n").unwrap();
    let err = rt.run_mod_init(&dir).expect_err("missing default export");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("default mod manifest export"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_quickjs_default_manifest_missing_name_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("no_name");
    std::fs::write(
        dir.join("start-script.js"),
        "globalThis.__postretroModManifest = {};\n",
    )
    .unwrap();
    let err = rt.run_mod_init(&dir).expect_err("missing name");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("name"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_quickjs_default_manifest_initialization_throws_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("throws");
    std::fs::write(dir.join("start-script.js"), "throw new Error('boom');\n").unwrap();
    let err = rt
        .run_mod_init(&dir)
        .expect_err("default export init throws");
    match err {
        ScriptError::ScriptThrew { msg, .. } => {
            assert!(msg.contains("boom"), "{msg}");
            assert!(msg.contains("default mod manifest export"), "{msg}");
        }
        other => panic!("expected ScriptThrew, got {other:?}"),
    }
}

#[test]
fn mod_init_quickjs_default_manifest_non_object_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("non_obj");
    std::fs::write(
        dir.join("start-script.js"),
        "globalThis.__postretroModManifest = 42;\n",
    )
    .unwrap();
    let err = rt.run_mod_init(&dir).expect_err("non-object return");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(
                reason.contains("object"),
                "expected 'object' in error reason, got: {reason}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_quickjs_default_manifest_undefined_is_present_non_object() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("undefined_manifest");
    std::fs::write(
        dir.join("start-script.js"),
        "globalThis.__postretroModManifest = undefined;\n",
    )
    .unwrap();
    let err = rt.run_mod_init(&dir).expect_err("undefined manifest");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("object"), "{reason}");
            assert!(!reason.contains("missing"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_luau_missing_returned_manifest_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_no_setup");
    std::fs::write(dir.join("start-script.luau"), "local x = 1\n").unwrap();
    let err = rt.run_mod_init(&dir).expect_err("missing manifest return");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("returned mod manifest"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_luau_returned_manifest_initialization_throws_errors() {
    // Regression: mlua wraps Lua errors in a traceback whose format is
    // implementation-defined. Assert only the variant — not the message
    // text — so an mlua version bump can't break this test.
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_throws");
    std::fs::write(dir.join("start-script.luau"), "error(\"boom\")\n").unwrap();
    let err = rt
        .run_mod_init(&dir)
        .expect_err("returned manifest init throws");
    match err {
        ScriptError::ScriptThrew { msg, .. } => {
            assert!(msg.contains("returned mod manifest"), "{msg}");
        }
        other => panic!("expected ScriptThrew, got {other:?}"),
    }
}

#[test]
fn mod_init_luau_returned_manifest_non_table_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_non_table");
    std::fs::write(dir.join("start-script.luau"), "return 42\n").unwrap();
    let err = rt.run_mod_init(&dir).expect_err("non-table return");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(
                reason.contains("table"),
                "expected 'table' in error reason, got: {reason}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_luau_returned_manifest_missing_name_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_no_name");
    std::fs::write(dir.join("start-script.luau"), "return {}\n").unwrap();
    let err = rt.run_mod_init(&dir).expect_err("missing name");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("name"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_both_js_and_lua_errors() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("both");
    std::fs::write(
        dir.join("start-script.js"),
        "globalThis.__postretroModManifest = { name: 'A' };\n",
    )
    .unwrap();
    std::fs::write(dir.join("start-script.luau"), "return { name = 'A' }\n").unwrap();
    let err = rt.run_mod_init(&dir).expect_err("both present");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("both"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
#[cfg(debug_assertions)]
fn mod_init_both_ts_and_luau_errors_without_writing_js() {
    // Regression: previously the debug TS->JS auto-compile ran before the
    // both-present check, so a user with `start-script.ts` + `.luau`
    // would get an unwanted `start-script.js` materialized on disk and
    // have to delete it manually to switch to the Luau path. The check
    // must short-circuit before any compilation.
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("both_ts_luau");
    std::fs::write(
        dir.join("start-script.ts"),
        "export default { name: 'A' };\n",
    )
    .unwrap();
    std::fs::write(dir.join("start-script.luau"), "return { name = 'A' }\n").unwrap();

    let err = rt.run_mod_init(&dir).expect_err("both present");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains("both"), "{reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    assert!(
        !dir.join("start-script.js").exists(),
        "both-present error must short-circuit before TS->JS compile writes start-script.js",
    );
}

#[test]
fn mod_init_quickjs_entities_field_parses_descriptor() {
    // The default manifest export carries an `entities` array; each
    // element should parse into an `EntityTypeDescriptor`. Ingestion into
    // `DataRegistry` is handled by the boot caller; this test covers only
    // the parse path.
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_entities_field");
    std::fs::write(
        dir.join("start-script.js"),
        r#"
            globalThis.__postretroModManifest = {
                name: "EntitiesMod",
                entities: [{ canonicalName: "smoke_pillar" }],
            };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "EntitiesMod");
    assert_eq!(manifest.entities.len(), 1);
    assert_eq!(
        manifest.entities[0].canonical_name.as_deref(),
        Some("smoke_pillar"),
    );
}

#[test]
fn mod_init_quickjs_entities_missing_key_gives_empty_vec() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_entities_missing");
    std::fs::write(
        dir.join("start-script.js"),
        r#"
            globalThis.__postretroModManifest = { name: "NoEntitiesMod" };
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert!(manifest.entities.is_empty());
}

#[test]
fn mod_init_quickjs_entities_not_array_gives_error() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_entities_bad");
    std::fs::write(
        dir.join("start-script.js"),
        r#"
            globalThis.__postretroModManifest = { name: "Bad", entities: "bad" };
            "#,
    )
    .unwrap();

    let err = rt.run_mod_init(&dir).expect_err("entities must be array");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(
                reason.contains("entities"),
                "expected 'entities' in reason, got: {reason}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn mod_init_luau_entities_field_parses_descriptor() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_entities_field");
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            return {
                name = "EntitiesMod",
                entities = { { canonicalName = "smoke_pillar" } },
            }
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "EntitiesMod");
    assert_eq!(manifest.entities.len(), 1);
    assert_eq!(
        manifest.entities[0].canonical_name.as_deref(),
        Some("smoke_pillar"),
    );
}

#[test]
fn mod_init_luau_entities_missing_key_gives_empty_vec() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_entities_missing");
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            return { name = "NoEntitiesMod" }
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert!(manifest.entities.is_empty());
}

#[test]
fn mod_init_luau_entities_not_array_gives_error() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_entities_bad");
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            return { name = "Bad", entities = "bad" }
            "#,
    )
    .unwrap();

    let err = rt.run_mod_init(&dir).expect_err("entities must be array");
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(
                reason.contains("entities"),
                "expected 'entities' in reason, got: {reason}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}
