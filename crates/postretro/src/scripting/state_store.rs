// State-store primitive registration and compatibility exports.
// See: context/lib/scripting.md §5 "Durable State Store"

use postretro_entities::{ScriptCtx, ScriptError};
use postretro_scripting_core::primitive_adapters::{
    Any, ScriptSlotValue, StoreDefinition, StoreSchemaJson,
};
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
#[allow(unused_imports)]
pub(crate) use postretro_scripting_core::store_bridge::{
    TextEdit, apply_store_slot_batch, apply_text_edit, read_store_slot, store_declaration,
    store_declaration_from_manifest_value, store_declaration_set_from_values,
    write_state_slot_json, write_store_slot,
};

const DEFINE_STORE_DOC: &str = "Build a typed state-store declaration for ModManifest.stores. \
     Every mod-owned slot requires a default. Supported types are number, boolean, string, enum, and array. \
     Calling this builder does not mutate engine state. Returned declarations commit atomically after the mod manifest succeeds. \
     Returns { declaration, state }, where state leaves are stable { slot } references. Definition context.";

const STORE_READ_DOC: &str = "Read the current value of an engine-global state slot by stable dotted name. \
     Available in definition and data contexts.";

const STORE_WRITE_DOC: &str = "Write an engine-global state slot by stable dotted name. \
     The value must exactly match the declared slot type. Finite numbers are clamped to the declared inclusive range. \
     Readonly slots reject script writes with a warning and remain unchanged. Available in definition and data contexts.";

pub(crate) fn register_store_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("defineStore", {
            move |namespace: String,
                  schema: StoreSchemaJson|
                  -> Result<StoreDefinition, ScriptError> {
                postretro_scripting_core::store_bridge::define_store(&namespace, schema.0)
            }
        })
        .scope(ContextScope::DefinitionOnly)
        .doc(DEFINE_STORE_DOC)
        .param("namespace", "String")
        .param("schema", "Any")
        .finish();

    registry
        .register("storeRead", {
            let ctx = ctx.clone();
            move |name: String| -> Result<Any, ScriptError> {
                read_store_slot(&ctx, &name).map(Any)
            }
        })
        .scope(ContextScope::Both)
        .doc(STORE_READ_DOC)
        .param("name", "String")
        .finish();

    registry
        .register(
            "storeWrite",
            move |name: String, value: ScriptSlotValue| -> Result<(), ScriptError> {
                postretro_scripting_core::store_bridge::write_script_store_slot(&ctx, &name, value)
            },
        )
        .scope(ContextScope::Both)
        .doc(STORE_WRITE_DOC)
        .param("name", "String")
        .param("value", "Any")
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Table as LuaTable;
    use postretro_entities::slot_table::{
        NamespaceInsertError, ReplicationScope, SlotOwnership, SlotTable, SlotType, SlotValue,
        StoreDeclarationSet,
    };
    use postretro_scripting_core::luau::{LuauConfig, LuauSubsystem, Which};
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};
    use postretro_scripting_core::store_bridge::{
        drain_store_declarations_js, drain_store_declarations_lua,
    };
    use rquickjs::{Object as JsObject, Value as JsValue};
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn registry_for(ctx: ScriptCtx) -> PrimitiveRegistry {
        let mut registry = PrimitiveRegistry::new();
        register_store_primitives(&mut registry, ctx);
        registry
    }

    fn commit_store_for_test(
        ctx: &ScriptCtx,
        namespace: &str,
        schema: Value,
    ) -> Result<(), ScriptError> {
        let declaration = store_declaration(namespace, schema)?;
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(&declaration.namespace, declaration.records)
            .map_err(|error| ScriptError::InvalidArgument {
                reason: format!("defineStore: {error}"),
            })
    }

    fn define_runtime_test_store(ctx: &ScriptCtx) {
        commit_store_for_test(
            ctx,
            "test",
            serde_json::json!({
                "number": { "type": "number", "default": 0.5, "range": [0, 1] },
                "boolean": { "type": "boolean", "default": false },
                "string": { "type": "string", "default": "before" },
                "enum": { "type": "enum", "values": ["idle", "active"], "default": "idle" },
                "array": { "type": "array", "default": [0, 1] },
            }),
        )
        .unwrap();
    }

    #[test]
    fn define_store_is_definition_only() {
        let registry = registry_for(ScriptCtx::new());
        let primitive = registry
            .iter()
            .find(|primitive| primitive.name == "defineStore")
            .unwrap();
        assert_eq!(primitive.context_scope, ContextScope::DefinitionOnly);
    }

    #[test]
    fn store_read_and_write_are_both_scoped_and_avoid_reserved_name() {
        let registry = registry_for(ScriptCtx::new());
        for name in ["storeRead", "storeWrite"] {
            let primitive = registry
                .iter()
                .find(|primitive| primitive.name == name)
                .unwrap();
            assert_eq!(primitive.context_scope, ContextScope::Both);
        }
        assert!(
            registry
                .iter()
                .all(|primitive| primitive.name != "setState")
        );
    }

    #[test]
    fn define_store_quickjs_and_luau_return_equivalent_declarations() {
        let js_ctx = ScriptCtx::new();
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                const store = defineStore("audio", {
                    master: { type: "number", default: 0.8, range: [0, 1], persist: true },
                    muted: { type: "boolean", default: false },
                    label: { type: "string", default: "" },
                    mode: { type: "enum", values: ["quiet", "loud"], default: "quiet" },
                    curve: { type: "array", default: [0, 0.5, 1] },
                });
                if (store.declaration.namespace !== "audio") throw new Error("namespace");
                if (store.state.master.slot !== "audio.master") throw new Error("state ref");
                "#,
                "store.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local store = defineStore("audio", {
                master = { type = "number", default = 0.8, range = {0, 1}, persist = true },
                muted = { type = "boolean", default = false },
                label = { type = "string", default = "" },
                mode = { type = "enum", values = {"quiet", "loud"}, default = "quiet" },
                curve = { type = "array", default = {0, 0.5, 1} },
            })
            assert(store.declaration.namespace == "audio")
            assert(store.state.master.slot == "audio.master")
            "#,
            "store.luau",
        )
        .unwrap();

        let js_slots = js_ctx.slot_table.borrow();
        let luau_slots = luau_ctx.slot_table.borrow();
        for name in [
            "audio.master",
            "audio.muted",
            "audio.label",
            "audio.mode",
            "audio.curve",
        ] {
            assert!(js_slots.get(name).is_none(), "js slot {name}");
            assert!(luau_slots.get(name).is_none(), "luau slot {name}");
        }
    }

    #[test]
    fn malformed_schemas_return_errors_without_partial_insertion() {
        let cases = [
            serde_json::json!({ "value": { "type": "number" } }),
            serde_json::json!({ "value": { "type": "number", "default": "one" } }),
            serde_json::json!({ "value": { "type": "number", "default": 2, "range": [0, 1] } }),
            serde_json::json!({ "value": { "type": "number", "default": 1, "range": [2, 1] } }),
            serde_json::json!({ "value": { "type": "boolean", "default": 1 } }),
            serde_json::json!({ "value": { "type": "string", "default": false } }),
            serde_json::json!({ "value": { "type": "enum", "values": [], "default": "a" } }),
            serde_json::json!({ "value": { "type": "enum", "values": ["a"], "default": "b" } }),
            serde_json::json!({ "value": { "type": "array", "default": [1, null] } }),
            serde_json::json!({ "value": { "type": "vector", "default": 1 } }),
        ];

        for (index, schema) in cases.into_iter().enumerate() {
            let err = store_declaration_from_manifest_value(serde_json::json!({
                "namespace": format!("bad{index}"),
                "schema": schema,
            }))
            .expect_err("malformed returned declaration should fail validation");
            assert!(matches!(err, ScriptError::InvalidArgument { .. }));
        }

        let ctx = ScriptCtx::new();
        let err = commit_store_for_test(
            &ctx,
            "mixed",
            serde_json::json!({
                "good": { "type": "number", "default": 1 },
                "bad": { "type": "enum", "values": [], "default": "x" },
            }),
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
        assert!(ctx.slot_table.borrow().get("mixed.good").is_none());
    }

    #[test]
    fn define_store_rejects_engine_namespace_and_prefix_collisions() {
        let ctx = ScriptCtx::new();
        let schema = serde_json::json!({
            "shield": { "type": "number", "default": 100 }
        });

        for (namespace, expected) in [
            ("player", "incompatible schema"),
            ("player.stats", "namespace collision"),
        ] {
            let declaration = store_declaration(namespace, schema.clone()).unwrap();
            let mut declarations = StoreDeclarationSet::default();
            declarations.add(declaration).unwrap();
            let err = ctx
                .slot_table
                .borrow()
                .plan_reconcile(&declarations)
                .unwrap_err();
            match expected {
                "incompatible schema" => assert!(matches!(
                    err,
                    NamespaceInsertError::IncompatibleSchema { .. }
                )),
                "namespace collision" => assert!(matches!(
                    err,
                    NamespaceInsertError::NamespaceCollision { .. }
                )),
                _ => unreachable!("test expectation covers every case"),
            }
        }
        assert!(ctx.slot_table.borrow().get("player.shield").is_none());
        assert!(ctx.slot_table.borrow().get("player.stats.shield").is_none());
    }

    #[test]
    fn duplicate_mod_namespace_is_rejected_without_mutation() {
        let ctx = ScriptCtx::new();
        commit_store_for_test(
            &ctx,
            "audio",
            serde_json::json!({
                "master": { "type": "number", "default": 1 }
            }),
        )
        .unwrap();
        let before = ctx.slot_table.borrow().len();

        let err = commit_store_for_test(
            &ctx,
            "audio",
            serde_json::json!({
                "music": { "type": "number", "default": 0.5 }
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("collides"));
        assert_eq!(ctx.slot_table.borrow().len(), before);
        assert!(ctx.slot_table.borrow().get("audio.music").is_none());
    }

    #[test]
    fn namespace_error_type_remains_matchable() {
        let mut table = SlotTable::new();
        let err = table.insert_namespace("player", Vec::new()).unwrap_err();
        assert!(matches!(
            err,
            NamespaceInsertError::NamespaceCollision { .. }
        ));
    }

    #[test]
    fn define_store_returns_stable_state_refs_in_both_runtimes() {
        let js_ctx = ScriptCtx::new();
        let js_registry = registry_for(js_ctx);
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                const store = defineStore("audio", {
                    master: { type: "number", default: 1 },
                    muted: { type: "boolean", default: false },
                });
                if (store.state.master.slot !== "audio.master") throw new Error("master handle");
                if (store.state.muted.slot !== "audio.muted") throw new Error("muted handle");
                "#,
                "store-handles.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        let luau_registry = registry_for(luau_ctx);
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local store = defineStore("audio", {
                master = { type = "number", default = 1 },
                muted = { type = "boolean", default = false },
            })
            assert(store.state.master.slot == "audio.master")
            assert(store.state.muted.slot == "audio.muted")
            "#,
            "store-handles.luau",
        )
        .unwrap();
    }

    #[test]
    fn malformed_luau_builders_do_not_insert_before_manifest_validation() {
        let cases = [
            r#"{ value = { type = "number" } }"#,
            r#"{ value = { type = "number", default = "one" } }"#,
            r#"{ value = { type = "number", default = 2, range = {0, 1} } }"#,
            r#"{ value = { type = "boolean", default = 1 } }"#,
            r#"{ value = { type = "enum", values = {}, default = "a" } }"#,
            r#"{ value = { type = "array", default = {1, 0 / 0} } }"#,
            r#"{ value = { type = "vector", default = 1 } }"#,
        ];

        for (index, schema) in cases.into_iter().enumerate() {
            let ctx = ScriptCtx::new();
            let registry = registry_for(ctx.clone());
            let luau = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
            let source = format!(r#"defineStore("bad{index}", {schema})"#);
            luau.run_source::<()>(Which::Definition, &source, "bad-store.luau")
                .expect("pure builder does not validate until manifest drain");
            assert!(
                ctx.slot_table
                    .borrow()
                    .get(&format!("bad{index}.value"))
                    .is_none()
            );
        }
    }

    fn drain_luau_store_manifest(source: &str) -> Result<StoreDeclarationSet, ScriptError> {
        let lua = mlua::Lua::new();
        let manifest: LuaTable = lua.load(source).eval().unwrap();
        drain_store_declarations_lua(&manifest)
    }

    fn drain_quickjs_store_manifest(source: &str) -> Result<StoreDeclarationSet, ScriptError> {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|qjs| {
            let value: JsValue = qjs.eval(source).unwrap();
            let manifest = JsObject::from_value(value).unwrap();
            drain_store_declarations_js(&qjs, &manifest)
        })
    }

    #[test]
    fn luau_setup_mod_stores_accepts_dense_arrays() {
        let declarations = drain_luau_store_manifest(
            r#"
            local first = {
                namespace = "audio",
                schema = { master = { type = "number", default = 1 } },
            }
            local second = {
                namespace = "video",
                schema = { brightness = { type = "number", default = 0.5 } },
            }
            return { stores = { first, second } }
            "#,
        )
        .unwrap();

        assert_eq!(declarations.len(), 2);
    }

    #[test]
    fn luau_setup_mod_stores_rejects_non_dense_tables() {
        // Regression: raw_len iteration treated map-shaped and sparse `stores`
        // tables as empty or partial arrays, silently dropping declarations.
        let cases = [
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { audio = declaration } }
                "#,
                "map-shaped table",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { declaration, extra = declaration } }
                "#,
                "extra string key",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [2] = declaration } }
                "#,
                "hole",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [0] = declaration } }
                "#,
                "zero index",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [1.5] = declaration } }
                "#,
                "non-integer index",
            ),
        ];

        for (source, label) in cases {
            let err = match drain_luau_store_manifest(source) {
                Ok(_) => panic!("{label} should be rejected"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("dense array"),
                "{label} produced unexpected error: {err}"
            );
        }
    }

    #[test]
    fn returned_store_declaration_rejects_cyclic_schema_before_commit() {
        let quickjs_err = drain_quickjs_store_manifest(
            r#"
            const schema = { value: { type: "number", default: 1 } };
            schema.value.self = schema;
            ({ stores: [{ namespace: "cyclic", schema }] })
            "#,
        )
        .unwrap_err();
        assert!(
            quickjs_err.to_string().contains("maximum conversion depth"),
            "unexpected QuickJS error: {quickjs_err}"
        );

        let luau_err = drain_luau_store_manifest(
            r#"
            local schema = { value = { type = "number", default = 1 } }
            schema.value.self = schema
            return { stores = { { namespace = "cyclic", schema = schema } } }
            "#,
        )
        .unwrap_err();
        assert!(
            luau_err.to_string().contains("maximum conversion depth"),
            "unexpected Luau error: {luau_err}"
        );
    }

    #[test]
    fn store_read_write_quickjs_and_luau_cover_all_value_kinds_and_clamp() {
        let js_ctx = ScriptCtx::new();
        define_runtime_test_store(&js_ctx);
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                storeWrite("test.number", 5);
                storeWrite("test.boolean", true);
                storeWrite("test.string", "after");
                storeWrite("test.enum", "active");
                storeWrite("test.array", [2, 3.5, 4]);
                if (storeRead("test.number") !== 1) throw new Error("number");
                if (storeRead("test.boolean") !== true) throw new Error("boolean");
                if (storeRead("test.string") !== "after") throw new Error("string");
                if (storeRead("test.enum") !== "active") throw new Error("enum");
                const array = storeRead("test.array");
                if (array.length !== 3 || array[0] !== 2 || array[1] !== 3.5 || array[2] !== 4) {
                    throw new Error("array");
                }
                "#,
                "store-read-write.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        define_runtime_test_store(&luau_ctx);
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            storeWrite("test.number", 5)
            storeWrite("test.boolean", true)
            storeWrite("test.string", "after")
            storeWrite("test.enum", "active")
            storeWrite("test.array", {2, 3.5, 4})
            assert(storeRead("test.number") == 1)
            assert(storeRead("test.boolean") == true)
            assert(storeRead("test.string") == "after")
            assert(storeRead("test.enum") == "active")
            local array = storeRead("test.array")
            assert(#array == 3 and array[1] == 2 and array[2] == 3.5 and array[3] == 4)
            "#,
            "store-read-write.luau",
        )
        .unwrap();

        let js_slots = js_ctx.slot_table.borrow();
        let luau_slots = luau_ctx.slot_table.borrow();
        for name in [
            "test.number",
            "test.boolean",
            "test.string",
            "test.enum",
            "test.array",
        ] {
            assert_eq!(js_slots.get(name), luau_slots.get(name), "slot {name}");
        }
    }

    #[test]
    fn readonly_script_write_is_rejected_but_engine_write_succeeds() {
        for runtime in ["quickjs", "luau"] {
            let ctx = ScriptCtx::new();
            let registry = registry_for(ctx.clone());
            match runtime {
                "quickjs" => {
                    let quickjs =
                        QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
                    quickjs.definition_ctx().with(|qjs| {
                        run_script::<()>(
                            &qjs,
                            r#"storeWrite("player.health", 25);"#,
                            "readonly-store.js",
                        )
                        .unwrap();
                    });
                }
                "luau" => {
                    let luau = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
                    luau.run_source::<()>(
                        Which::Definition,
                        r#"storeWrite("player.health", 25)"#,
                        "readonly-store.luau",
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }

            assert_eq!(
                ctx.slot_table
                    .borrow()
                    .get("player.health")
                    .and_then(|slot| slot.value.as_ref()),
                None
            );
            write_store_slot(&ctx, "player.health", SlotValue::Number(75.0)).unwrap();
            assert_eq!(
                read_store_slot(&ctx, "player.health").unwrap(),
                SlotValue::Number(75.0)
            );
        }
    }

    #[test]
    fn store_write_type_enum_array_and_unknown_name_errors_preserve_values() {
        let js_ctx = ScriptCtx::new();
        define_runtime_test_store(&js_ctx);
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                for (const write of [
                    () => storeWrite("test.number", true),
                    () => storeWrite("test.enum", "missing"),
                    () => storeWrite("test.array", [1, NaN]),
                    () => storeWrite("test.missing", 1),
                ]) {
                    let threw = false;
                    try { write(); } catch (_) { threw = true; }
                    if (!threw) throw new Error("invalid write did not throw");
                }
                "#,
                "invalid-store-write.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        define_runtime_test_store(&luau_ctx);
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local writes = {
                function() storeWrite("test.number", true) end,
                function() storeWrite("test.enum", "missing") end,
                function() storeWrite("test.array", {1, 0 / 0}) end,
                function() storeWrite("test.missing", 1) end,
            }
            for _, write in writes do
                local ok = pcall(write)
                assert(not ok)
            end
            "#,
            "invalid-store-write.luau",
        )
        .unwrap();

        for ctx in [&js_ctx, &luau_ctx] {
            assert_eq!(
                read_store_slot(ctx, "test.number").unwrap(),
                SlotValue::Number(0.5)
            );
            assert_eq!(
                read_store_slot(ctx, "test.enum").unwrap(),
                SlotValue::Enum("idle".to_string())
            );
            assert_eq!(
                read_store_slot(ctx, "test.array").unwrap(),
                SlotValue::Array(vec![0.0, 1.0])
            );
        }
    }

    #[test]
    fn engine_write_validates_types_enum_values_arrays_and_ranges() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);

        write_store_slot(&ctx, "test.number", SlotValue::Number(-10.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(0.0)
        );

        for (name, value) in [
            ("test.number", SlotValue::Boolean(true)),
            ("test.enum", SlotValue::Enum("missing".to_string())),
            ("test.array", SlotValue::Array(vec![1.0, f32::INFINITY])),
        ] {
            assert!(write_store_slot(&ctx, name, value).is_err());
        }
        assert!(read_store_slot(&ctx, "test.missing").is_err());
        assert!(write_store_slot(&ctx, "test.missing", SlotValue::Number(1.0)).is_err());
    }

    // --- M13 Goal F, Task 4: setState readonly-gated JSON write ---

    #[test]
    fn set_state_json_write_applies_to_writable_slot_with_validation() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);

        // A number write coerces and clamps to the declared [0, 1] range.
        write_state_slot_json(&ctx, "test.number", &serde_json::json!(5)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(1.0)
        );
        // Boolean / string / enum / array all coerce from their JSON forms.
        write_state_slot_json(&ctx, "test.boolean", &serde_json::json!(true)).unwrap();
        write_state_slot_json(&ctx, "test.string", &serde_json::json!("after")).unwrap();
        write_state_slot_json(&ctx, "test.enum", &serde_json::json!("active")).unwrap();
        write_state_slot_json(&ctx, "test.array", &serde_json::json!([2.0, 3.5])).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.boolean").unwrap(),
            SlotValue::Boolean(true)
        );
        assert_eq!(
            read_store_slot(&ctx, "test.enum").unwrap(),
            SlotValue::Enum("active".to_string())
        );
        assert_eq!(
            read_store_slot(&ctx, "test.array").unwrap(),
            SlotValue::Array(vec![2.0, 3.5])
        );
    }

    #[test]
    fn set_state_json_write_rejects_readonly_slot_and_leaves_value_unchanged() {
        // `player.health` is an engine-owned readonly-to-scripts slot. setState
        // warns and no-ops; the value is unchanged. Distinct from the engine
        // bypass (`write_store_slot`), which succeeds — proven below.
        let ctx = ScriptCtx::new();
        write_store_slot(&ctx, "player.health", SlotValue::Number(50.0)).unwrap();

        write_state_slot_json(&ctx, "player.health", &serde_json::json!(25.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(50.0),
            "readonly slot is unchanged after setState"
        );

        // The engine bypass still writes the readonly slot.
        write_store_slot(&ctx, "player.health", SlotValue::Number(75.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(75.0)
        );
    }

    // --- M13 Text Entry, Task 1: text-edit reactions ---

    fn read_string(ctx: &ScriptCtx, name: &str) -> String {
        match read_store_slot(ctx, name).unwrap() {
            SlotValue::String(value) => value,
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn ui_text_entry_is_engine_writable_string_slot() {
        let ctx = ScriptCtx::new();
        let table = ctx.slot_table.borrow();
        let slot = table.get("ui.textEntry").expect("ui.textEntry exists");
        assert_eq!(slot.schema.slot_type, SlotType::String);
        assert!(!slot.schema.readonly);
        assert_eq!(slot.schema.ownership, SlotOwnership::Engine);
        assert_eq!(slot.value, Some(SlotValue::String(String::new())));
    }

    #[test]
    fn append_text_appends_to_target_slot() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("ab")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("c")).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "abc");
    }

    #[test]
    fn backspace_text_removes_last_char() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("abc")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "ab");
    }

    #[test]
    fn backspace_text_on_empty_is_noop() {
        // No-op and (by design) no warning/no write: the value stays empty.
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "");
    }

    #[test]
    fn backspace_text_removes_one_precomposed_multibyte_char() {
        // `é` as U+00E9 is a single `char` but two UTF-8 bytes. The char-pop
        // floor removes it whole, never splitting the UTF-8 sequence — the
        // result is valid UTF-8 ("a"), not a truncated byte sequence.
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("a\u{00E9}")).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry").chars().count(), 2);
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "a");
    }

    #[test]
    fn clear_text_empties_the_slot() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("hello")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Clear).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "");
    }

    #[test]
    fn text_edits_reject_readonly_slot_and_leave_value_unchanged() {
        // `input.mode` is an engine-owned readonly slot. Text edits ride the
        // same readonly-gated write as setState, so they warn and no-op. (It is
        // an enum, but readonly is checked before type coercion, so the write is
        // rejected on the readonly gate, leaving the value unchanged.)
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "input.mode", TextEdit::Append("x")).unwrap();
        apply_text_edit(&ctx, "input.mode", TextEdit::Clear).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "input.mode").unwrap(),
            SlotValue::Enum("focus".to_string()),
            "readonly slot unchanged after text edits"
        );
    }

    #[test]
    fn text_edit_unknown_slot_errors() {
        let ctx = ScriptCtx::new();
        assert!(apply_text_edit(&ctx, "ui.missing", TextEdit::Append("x")).is_err());
    }

    #[test]
    fn set_state_json_write_errors_on_unknown_slot_and_type_mismatch() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);
        assert!(write_state_slot_json(&ctx, "test.missing", &serde_json::json!(1)).is_err());
        // A boolean into a number slot is a type mismatch.
        assert!(write_state_slot_json(&ctx, "test.number", &serde_json::json!(true)).is_err());
        // The number slot is unchanged after the rejected write.
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(0.5)
        );
    }

    // --- M15 Phase 3.5 Task 5: `defineStore` `network` opt-in -------------------

    #[test]
    fn define_store_network_shared_maps_to_shared_global_scope() {
        // `network: "shared"` opts a mod slot into `SharedGlobal` replication; an
        // omitted `network` field stays local-only `None`.
        let declaration = store_declaration(
            "netFixture",
            serde_json::json!({
                "objectiveProgress": { "type": "number", "default": 0, "network": "shared" },
                "localOnly": { "type": "number", "default": 0 },
            }),
        )
        .expect("shared + local-only schema is valid");

        let scopes: BTreeMap<&str, ReplicationScope> = declaration
            .records
            .iter()
            .map(|(name, record)| (name.as_str(), record.schema.network))
            .collect();
        assert_eq!(
            scopes.get("objectiveProgress"),
            Some(&ReplicationScope::SharedGlobal),
            "network: \"shared\" maps to SharedGlobal"
        );
        assert_eq!(
            scopes.get("localOnly"),
            Some(&ReplicationScope::None),
            "an omitted network field is local-only None"
        );
    }

    #[test]
    fn define_store_rejects_owner_private_network_for_mod_stores() {
        // Mod-declared owner-private per-player slots are out of scope: `"ownerPrivate"`
        // must be rejected with a clear, author-facing error.
        let err = store_declaration(
            "netFixture",
            serde_json::json!({
                "secret": { "type": "number", "default": 0, "network": "ownerPrivate" },
            }),
        )
        .expect_err("ownerPrivate is rejected for mod stores");
        let message = err.to_string();
        assert!(
            message.contains("ownerPrivate") && message.contains("not supported"),
            "error names the rejected value and that it is unsupported: {message}"
        );
    }

    #[test]
    fn define_store_rejects_unknown_network_value() {
        let err = store_declaration(
            "netFixture",
            serde_json::json!({
                "weird": { "type": "number", "default": 0, "network": "everyone" },
            }),
        )
        .expect_err("an unknown network value is rejected");
        assert!(
            err.to_string().contains("unknown `network` value"),
            "error names the unknown network value: {err}"
        );
    }

    #[test]
    fn define_store_network_shared_round_trips_in_both_runtimes() {
        // The same `network: "shared"` opt-in works through the real QuickJS and Luau
        // `defineStore` paths and commits a `SharedGlobal` slot in each.
        let js_ctx = ScriptCtx::new();
        commit_store_for_test(
            &js_ctx,
            "netFixture",
            serde_json::json!({
                "objectiveProgress": { "type": "number", "default": 0, "network": "shared" },
            }),
        )
        .unwrap();
        assert_eq!(
            js_ctx
                .slot_table
                .borrow()
                .get("netFixture.objectiveProgress")
                .unwrap()
                .schema
                .network,
            ReplicationScope::SharedGlobal,
        );

        let luau_registry = registry_for(ScriptCtx::new());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        let manifest: LuaTable = luau
            .run_source::<LuaTable>(
                Which::Definition,
                r#"
                local store = defineStore("netFixture", {
                    objectiveProgress = { type = "number", default = 0, network = "shared" },
                })
                return { stores = { store.declaration } }
                "#,
                "net-fixture.luau",
            )
            .unwrap();
        let declarations = drain_store_declarations_lua(&manifest).unwrap();
        // The drained declaration carries the SharedGlobal scope.
        let mut table = SlotTable::new();
        for declaration in declarations.iter() {
            table
                .insert_namespace(&declaration.namespace, declaration.records.clone())
                .unwrap();
        }
        assert_eq!(
            table
                .get("netFixture.objectiveProgress")
                .unwrap()
                .schema
                .network,
            ReplicationScope::SharedGlobal,
        );
    }
}
