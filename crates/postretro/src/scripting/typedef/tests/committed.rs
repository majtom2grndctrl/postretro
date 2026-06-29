use super::*;

/// Guard against drift between the registry-driven type generator and the
/// committed SDK type files. Runs unconditionally so CI catches a missed
/// `gen-script-types` regeneration. Paths are resolved relative to
/// `CARGO_MANIFEST_DIR` so the test works from any CWD.
#[test]
fn committed_sdk_types_match_current_registry() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    let ts_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/types/postretro.d.ts"
    );
    let luau_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/types/postretro.d.luau"
    );

    let committed_ts = fs::read_to_string(ts_path).expect("read committed postretro.d.ts");
    let committed_luau = fs::read_to_string(luau_path).expect("read committed postretro.d.luau");

    assert_eq!(
        committed_ts, ts,
        "sdk/types/postretro.d.ts is out of date — re-run `cargo run -p postretro --bin gen-script-types` and commit the result"
    );
    assert_eq!(
        committed_luau, luau,
        "sdk/types/postretro.d.luau is out of date — re-run `cargo run -p postretro --bin gen-script-types` and commit the result"
    );
}

/// Positive-content guard for the `AiDescriptor` typedef registration.
/// The drift test (`committed_sdk_types_match_current_registry`) only proves
/// the committed files MATCH the generator — it would pass vacuously if
/// `AiDescriptor` were never registered and therefore never emitted on
/// either side. This test asserts the type, its closed `states` block, and
/// the `ai` component slot are actually present in the committed typedefs and
/// in fresh generator output, so a regression that drops the registration
/// fails loudly rather than silently passing.
#[test]
fn committed_sdk_types_contain_ai_descriptor() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    let committed_ts = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/types/postretro.d.ts"
    ))
    .expect("read committed postretro.d.ts");
    let committed_luau = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/types/postretro.d.luau"
    ))
    .expect("read committed postretro.d.luau");

    for (label, generated, committed) in
        [("ts", &ts, &committed_ts), ("luau", &luau, &committed_luau)]
    {
        for needle in ["AiDescriptor", "AiStateNames", "detectionRange"] {
            assert!(
                generated.contains(needle),
                "{label} generator output missing `{needle}` — AiDescriptor not registered?"
            );
            assert!(
                committed.contains(needle),
                "committed {label} typedefs missing `{needle}` — regenerate and commit"
            );
        }
    }

    // The `ai` component slot must be wired into EntityTypeComponents on
    // both surfaces (TS uses `?:`, Luau uses a trailing `?`).
    assert!(
        committed_ts.contains("ai?: AiDescriptor | null;"),
        "committed TS typedefs missing the `ai` component slot"
    );
    assert!(
        committed_luau.contains("ai: AiDescriptor?,"),
        "committed Luau typedefs missing the `ai` component slot"
    );
}

/// `defineStore` returns a pure `{ declaration, state }` builder result.
/// The generator special-cases it (like `worldQuery`) so the static SDK
/// block's generic `defineStore<const S>` supplies the schema-keyed
/// `state` map and declaration type. The old registry-driven
/// `StateValue<string>` handle map must NOT be emitted.
#[test]
fn define_store_emits_returned_declaration_and_state_refs() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);

    // The generic declaration that returns declaration + state refs.
    assert!(
        ts.contains(
            "export function defineStore<const S extends Record<string, StoreSlotSchema>>("
        ),
        "ts missing generic defineStore declaration:\n{ts}"
    );
    assert!(
        ts.contains("readonly declaration: StoreDeclaration;"),
        "ts StoreDefinition missing declaration field"
    );
    assert!(
        ts.contains("readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };"),
        "ts StoreDefinition missing schema-keyed state refs"
    );
    assert!(
        ts.contains("Slot extends { readonly: true } ? ReadonlyStateRef<T> : WritableStateRef<T>;"),
        "ts StoreStateRefForSlot must preserve readonly schema capability"
    );
    assert!(
        ts.contains("Slot extends { type: \"number\" } ? StoreStateRefForSlot<Slot, number>"),
        "ts StateValueForSlot must route through readonly-aware ref selection"
    );
    // The old uniform registry-driven handle map must be gone.
    assert!(
        !ts.contains("export function defineStore(namespace: string, schema: unknown)"),
        "ts must not emit the registry-driven uniform StateValue<string> defineStore"
    );
    assert!(
        !ts.contains("): { readonly [K in keyof S]: StateValueForSlot<S[K]> };"),
        "ts must not return the old top-level StateValue handle map"
    );

    let luau = generate_luau(&r);
    assert!(
        luau.contains("declare function defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition"),
        "luau missing StoreDefinition defineStore declaration:\n{luau}"
    );
    assert!(
        luau.contains("export type StoreStateRef<T> = ReadonlyStateRef<T> | WritableStateRef<T>")
            && luau.contains("state: { [string]: StoreStateRef<any> },"),
        "luau StoreDefinition must not type every store slot as writable:\n{luau}"
    );
}

#[test]
fn mod_manifest_catalog_helpers_are_covered_by_typedefs() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    assert!(
        ts.contains("export type ModMapEntry = {")
            && ts.contains("export type MenuCamera = {")
            && ts.contains("position: readonly [number, number, number];")
            && ts.contains("export type Frontend = {")
            && ts.contains("menuTree: string;")
            && ts.contains("backgroundLevel?: string;")
            && ts.contains("camera: MenuCamera;")
            && ts.contains("tags?: ReadonlyArray<string>;")
            && ts.contains("maps?: ReadonlyArray<ModMapEntry>;")
            && ts.contains("frontend?: Frontend;")
            && ts.contains("reactions?: ReadonlyArray<NamedReactionDescriptor>;")
            && ts.contains("crossings?: ReadonlyArray<CrossingDescriptor>;")
            && ts.contains("export function defineMod(config: ModManifest): ModManifest;")
            && ts.contains(
                "export function defineMapCatalog(entries: ModMapEntry[]): ModMapEntry[];"
            ),
        "ts output missing mod map catalog helper/type coverage:\n{ts}"
    );
    assert!(
        luau.contains("export type ModMapEntry = {")
            && luau.contains("export type MenuCamera = {")
            && luau.contains("position: {number},")
            && luau.contains("export type Frontend = {")
            && luau.contains("menuTree: string,")
            && luau.contains("backgroundLevel: string?,")
            && luau.contains("camera: MenuCamera,")
            && luau.contains("tags: {string}?")
            && luau.contains("maps: {ModMapEntry}?")
            && luau.contains("frontend: Frontend?")
            && luau.contains("reactions: {NamedReactionDescriptor}?")
            && luau.contains("crossings: {CrossingDescriptor}?")
            && luau.contains("declare function defineMod(config: ModManifest): ModManifest")
            && luau.contains(
                "declare function defineMapCatalog(entries: {ModMapEntry}): {ModMapEntry}"
            )
            && luau.contains("defineMod: typeof(defineMod),")
            && luau.contains("defineMapCatalog: typeof(defineMapCatalog),"),
        "luau output missing mod map catalog helper/type coverage:\n{luau}"
    );
}

/// The main `postretro` module exposes a generated `GameStateRefs` tree.
/// Leaves are direct `{ slot }` reference descriptors with readonly/writable
/// capability in the type only.
#[test]
fn game_state_refs_emit_catalog_paths_and_capabilities() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;
    use postretro_entities::engine_state_catalog::engine_state_catalog;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);

    assert!(
        ts.contains("export function getGameState(): GameStateRefs;"),
        "ts missing getGameState declaration:\n{ts}"
    );
    assert!(
        ts.contains("readonly player: {\n      readonly health: ReadonlyStateRef<number>;\n      readonly maxHealth: ReadonlyStateRef<number>;")
            && ts.contains("readonly textEntry: WritableStateRef<string>;"),
        "ts GameStateRefs missing catalog path/capability refs:\n{ts}"
    );
    assert!(
        !ts.contains("postretro/game-state")
            && !ts.contains("ReadonlyStateValue")
            && !ts.contains("readonly ammo:"),
        "legacy game-state module/value handles must be gone"
    );

    let luau = generate_luau(&r);
    assert!(
        luau.contains("declare function getGameState(): GameStateRefs"),
        "luau missing getGameState declaration:\n{luau}"
    );
    assert!(
        luau.contains("health: ReadonlyStateRef<number>,")
            && luau.contains("maxHealth: ReadonlyStateRef<number>,")
            && !luau.contains("ammo: ReadonlyStateRef<number>,")
            && luau.contains("textEntry: WritableStateRef<string>,"),
        "luau GameStateRefs missing catalog path/capability refs"
    );

    let catalog = engine_state_catalog().unwrap();
    for entry in catalog.entries() {
        let leaf = entry
            .sdk_path
            .last()
            .expect("catalog validation requires nonempty SDK paths");
        let expected_ts = format!(
            "readonly {leaf}: {};",
            state_ref_ts(entry.capability, entry.value_type)
        );
        assert!(
            ts.contains(&expected_ts),
            "ts GameStateRefs missing catalog leaf `{}` as `{expected_ts}`",
            entry.sdk_path.join(".")
        );

        let expected_luau = format!(
            "{leaf}: {},",
            state_ref_luau(entry.capability, entry.value_type)
        );
        assert!(
            luau.contains(&expected_luau),
            "luau GameStateRefs missing catalog leaf `{}` as `{expected_luau}`",
            entry.sdk_path.join(".")
        );
    }

    for forbidden in [
        "postretro/game-state",
        "declare const gameState",
        "declare gameState",
        "export const gameState",
        "declare const playerState",
        "declare playerState",
        "export const playerState",
        "ReadonlyStateValue",
        "WritableStateValue",
        "storeHandle",
    ] {
        for line in ts.lines() {
            if line.trim_start().starts_with("//") || line.trim_start().starts_with("*") {
                continue;
            }
            assert!(
                !line.contains(forbidden),
                "ts must not declare legacy game-state surface `{forbidden}`; offending line: {line}"
            );
        }
        for line in luau.lines() {
            if line.trim_start().starts_with("--") {
                continue;
            }
            assert!(
                !line.contains(forbidden),
                "luau must not declare legacy game-state surface `{forbidden}`; offending line: {line}"
            );
        }
    }
}
