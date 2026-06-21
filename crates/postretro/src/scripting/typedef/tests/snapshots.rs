use super::*;

#[test]
fn typescript_snapshot_matches_mini_registry_with_docs() {
    let got = generate_typescript(&mini_registry_with_docs());
    let expected = ts_with_sdk_lib_block(EXPECTED_TS_WITH_DOCS);
    assert_eq!(got, expected, "TS docs snapshot drift:\n{got}");
}

#[test]
fn luau_snapshot_matches_mini_registry_with_docs() {
    let got = generate_luau(&mini_registry_with_docs());
    let expected = luau_with_sdk_lib_block(EXPECTED_LUAU_WITH_DOCS);
    assert_eq!(got, expected, "Luau docs snapshot drift:\n{got}");
}

#[test]
fn typescript_snapshot_matches_mini_registry() {
    let got = generate_typescript(&mini_registry());
    let expected_prefix = EXPECTED_TS
        .strip_suffix("}\n")
        .expect("expected TS snapshot to end with `}\\n`");
    assert_starts_with_snapshot(&got, expected_prefix, "TS");
}

#[test]
fn luau_snapshot_matches_mini_registry() {
    let got = generate_luau(&mini_registry());
    assert_starts_with_snapshot(&got, EXPECTED_LUAU, "Luau");
}

#[test]
fn sdk_lib_block_is_present_in_full_outputs() {
    // Sanity: SDK-lib symbols must surface in the type files so authors
    // get IDE completions. After the capability-handle refactor, `flicker`
    // / `pulse` / `colorShift` / `sweep` / `fogPulse` / `fogFade` are no
    // longer bare globals — they live on `LightEntityHandle` /
    // `FogVolumeHandle` capability interfaces.
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);
    for name in [
        "world",
        "timeline",
        "sequence",
        "AnimatableScalar",
        "AnimatableVec3",
        "LightEntityHandle",
        "FogVolumeHandle",
    ] {
        assert!(ts.contains(name), "ts missing sdk-lib symbol {name}");
        assert!(luau.contains(name), "luau missing sdk-lib symbol {name}");
    }
}

#[test]
fn underscore_prefixed_names_are_omitted_from_both_outputs() {
    let ts = generate_typescript(&mini_registry());
    let luau = generate_luau(&mini_registry());
    assert!(!ts.contains("__collect_definitions"));
    assert!(!luau.contains("__collect_definitions"));
}

#[test]
fn day_one_primitives_all_appear_in_both_outputs() {
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);
    for name in ["entityExists", "worldQuery"] {
        assert!(ts.contains(name), "ts missing primitive {name}:\n{ts}");
        assert!(
            luau.contains(name),
            "luau missing primitive {name}:\n{luau}"
        );
    }
    // `registerEntity` was removed in favor of `ModManifest.entities`
    // return field; it must not appear as a primitive declaration.
    for line in ts.lines() {
        if line.trim_start().starts_with("//") || line.trim_start().starts_with("*") {
            continue;
        }
        assert!(
            !line.contains("registerEntity"),
            "ts must not declare `registerEntity`; offending line: {line}"
        );
    }
    for line in luau.lines() {
        if line.trim_start().starts_with("--") {
            continue;
        }
        assert!(
            !line.contains("registerEntity"),
            "luau must not declare `registerEntity`; offending line: {line}"
        );
    }
    // Forbidden as exported symbols (declarations / exported types). Doc-
    // comment mentions inside the SDK lib block are not symbols and don't
    // count — the acceptance criterion is about author-visible types and
    // primitives, not free-form prose.
    for forbidden in [
        "spawnEntity",
        "despawnEntity",
        "getComponent",
        "setComponent",
        "emitEvent",
        "sendEvent",
        "registerHandler",
        "ScriptCallContext",
        "HandlerFn",
        "ScriptEvent",
    ] {
        for line in ts.lines() {
            if line.trim_start().starts_with("//") || line.trim_start().starts_with("*") {
                continue;
            }
            assert!(
                !line.contains(forbidden),
                "ts must not declare `{forbidden}`; offending line: {line}"
            );
        }
        for line in luau.lines() {
            if line.trim_start().starts_with("--") {
                continue;
            }
            assert!(
                !line.contains(forbidden),
                "luau must not declare `{forbidden}`; offending line: {line}"
            );
        }
    }
}

#[test]
fn write_type_definitions_creates_both_files() {
    let tmp = std::env::temp_dir().join(format!("postretro-typedef-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write_type_definitions(&mini_registry(), &tmp).unwrap();
    assert!(tmp.join("postretro.d.ts").exists());
    assert!(tmp.join("postretro.d.luau").exists());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn rust_to_ts_known_types() {
    assert_eq!(rust_to_ts("u32"), "number");
    assert_eq!(rust_to_ts("bool"), "boolean");
    assert_eq!(rust_to_ts("alloc::string::String"), "string");
    assert_eq!(rust_to_ts("core::option::Option<u32>"), "number | null");
    assert_eq!(rust_to_ts("alloc::vec::Vec<u32>"), "ReadonlyArray<number>");
    assert_eq!(
        rust_to_ts("core::result::Result<u32, postretro::scripting::error::ScriptError>"),
        "number"
    );
    assert_eq!(rust_to_ts("glam::Vec3"), "Vec3");
}

#[test]
fn rust_to_luau_known_types() {
    assert_eq!(rust_to_luau("u32"), "number");
    assert_eq!(rust_to_luau("bool"), "boolean");
    assert_eq!(rust_to_luau("core::option::Option<u32>"), "number?");
    assert_eq!(rust_to_luau("alloc::vec::Vec<u32>"), "{number}");
}

#[test]
fn generic_brand_emits_exact_contract_without_changing_plain_brands() {
    let mut registry = PrimitiveRegistry::new();
    registry.register_type("EntityId").brand("number").finish();
    registry
        .register_type("StateValue")
        .generic_brand("T", "T")
        .finish();

    let ts = generate_typescript(&registry);
    assert!(ts.contains("  export type EntityId = number & { readonly __brand: \"EntityId\" };"));
    assert!(ts.contains("  export type StateValue<T> = WritableStateRef<T>;"));

    let luau = generate_luau(&registry);
    assert!(luau.contains("export type EntityId = number"));
    assert!(luau.contains("export type StateValue<T> = WritableStateRef<T>"));
}
