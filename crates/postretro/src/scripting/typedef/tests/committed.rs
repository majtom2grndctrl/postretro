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

#[test]
fn committed_sdk_types_contain_mod_bloom_render_profile() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ScriptCtx::new());
    let generated_ts = generate_typescript(&registry);
    let generated_luau = generate_luau(&registry);
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

    for output in [&generated_ts, &committed_ts] {
        assert!(output.contains("export type BloomResolution ="));
        assert!(output.contains("\"half\""));
        assert!(output.contains("| \"quarter\""));
        assert!(output.contains("| \"eighth\""));
        assert!(output.contains("export type BloomRenderProfile = {"));
        assert!(output.contains("resolution?: BloomResolution;"));
        assert!(output.contains("pixelated?: boolean;"));
        assert!(output.contains("export type RenderProfile = {"));
        assert!(output.contains("bloom?: BloomRenderProfile;"));
        assert!(output.contains("render?: RenderProfile;"));
    }
    for output in [&generated_luau, &committed_luau] {
        assert!(output.contains("export type BloomResolution ="));
        assert!(output.contains("\"half\""));
        assert!(output.contains("| \"quarter\""));
        assert!(output.contains("| \"eighth\""));
        assert!(output.contains("export type BloomRenderProfile = {"));
        assert!(output.contains("resolution: BloomResolution?,"));
        assert!(output.contains("pixelated: boolean?,"));
        assert!(output.contains("export type RenderProfile = {"));
        assert!(output.contains("bloom: BloomRenderProfile?,"));
        assert!(output.contains("render: RenderProfile?,"));
    }
}

#[test]
fn committed_sdk_types_contain_weapon_ammo_resource() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ScriptCtx::new());
    let generated_ts = generate_typescript(&registry);
    let generated_luau = generate_luau(&registry);
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

    for output in [&generated_ts, &committed_ts] {
        assert!(output.contains("export type AmmoResource = {"));
        assert!(output.contains("| ({ kind: \"ammo\" } & AmmoResource);"));
        assert!(output.contains("resource?: WeaponResource;"));
        assert!(output.contains("costPerShot?: number;"));
        assert!(output.contains("reloadMs?: number;"));
        assert!(output.contains("export type ReloadStyle ="));
        assert!(output.contains("\"magazine\""));
        assert!(output.contains("| \"perShell\""));
        assert!(output.contains("reloadStyle?: ReloadStyle;"));
    }
    for output in [&generated_luau, &committed_luau] {
        assert!(output.contains("export type AmmoResource = {"));
        assert!(output.contains("(AmmoResource & { kind: \"ammo\" })"));
        assert!(output.contains("resource: WeaponResource?,"));
        assert!(output.contains("costPerShot: number?,"));
        assert!(output.contains("reloadMs: number?,"));
        assert!(output.contains("export type ReloadStyle ="));
        assert!(output.contains("\"magazine\""));
        assert!(output.contains("| \"perShell\""));
        assert!(output.contains("reloadStyle: ReloadStyle?,"));
    }
}

/// Guard the sole behavior-graph SDK surface against both registration loss and
/// legacy vocabulary reappearing. The drift test
/// (`committed_sdk_types_match_current_registry`) only proves that committed
/// files MATCH the generator; it cannot prove which surface either contains.
#[test]
fn committed_sdk_types_contain_behavior_graph_without_legacy_ai() {
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
        for needle in [
            "BehaviorGraphDescriptor",
            "candidateFilter",
            "retained-target stand-down",
        ] {
            assert!(
                generated.contains(needle),
                "{label} generator output missing `{needle}` — behavior graph not registered?"
            );
            assert!(
                committed.contains(needle),
                "committed {label} typedefs missing `{needle}` — regenerate and commit"
            );
        }
        for legacy in [
            "AiDescriptor",
            "AiStateNames",
            "leashRange",
            "components.ai",
        ] {
            assert!(
                !generated.contains(legacy),
                "{label} generator output still advertises retired `{legacy}`"
            );
            assert!(
                !committed.contains(legacy),
                "committed {label} typedefs still advertise retired `{legacy}`"
            );
        }
    }

    // EntityTypeComponents exposes the behavior graph and no retired `ai` slot
    // (TS uses `?:`, Luau uses a trailing `?`).
    assert!(
        committed_ts.contains("behavior?: BehaviorGraphDescriptor | null;"),
        "committed TS typedefs missing the behavior component slot"
    );
    assert!(
        committed_luau.contains("behavior: BehaviorGraphDescriptor?,"),
        "committed Luau typedefs missing the behavior component slot"
    );
    assert!(
        !committed_ts.contains("ai?:"),
        "committed TS typedefs still contain an `ai` component slot"
    );
    assert!(
        !committed_luau.contains("ai:"),
        "committed Luau typedefs still contain an `ai` component slot"
    );
}

/// `worldQuery` exposes raw snapshots; the `world.query` SDK vocabulary is
/// the layer that attaches light/fog capability methods plus mover and trigger
/// commands.
/// Keeping those declarations distinct prevents the bare primitive from
/// promising methods that its JSON serialization never includes.
#[test]
fn world_query_raw_snapshots_and_sdk_handles_remain_distinct() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    assert!(
        ts.contains(
            "export function worldQuery<T extends WorldQueryComponent>(filter: { component: T; tag?: string | null }): ReadonlyArray<RawEntityForComponent<T>>;"
        ) && ts.contains("T extends \"kinematic_mover\" ? MoverEntity :")
            && ts.contains("T extends \"kinematic_mover\" ? MoverEntityHandle :")
            && ts.contains("T extends \"trigger_volume\" ? TriggerVolumeEntity :")
            && ts.contains("T extends \"trigger_volume\" ? TriggerVolumeHandle :"),
        "TypeScript must distinguish raw worldQuery mover/trigger snapshots from world.query handles:\n{ts}"
    );
    assert!(
        luau.contains("((filter: { component: \"kinematic_mover\", tag: string? }) -> {MoverEntity})")
            && luau.contains(
                "((self: World, filter: { component: \"kinematic_mover\", tag: string? }) -> {MoverEntityHandle})"
            )
            && luau.contains(
                "((filter: { component: \"trigger_volume\", tag: string? }) -> {TriggerVolumeEntity})"
            )
            && luau.contains(
                "((self: World, filter: { component: \"trigger_volume\", tag: string? }) -> {TriggerVolumeHandle})"
            ),
        "Luau must distinguish raw worldQuery mover/trigger snapshots from world:query handles:\n{luau}"
    );
}

#[test]
fn trigger_command_step_types_are_emitted_in_both_sdk_surfaces() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    for (label, output) in [("ts", &ts), ("luau", &luau)] {
        for needle in [
            "ArmTriggerArgs",
            "DisarmTriggerArgs",
            "ArmTriggerStep",
            "DisarmTriggerStep",
            "armTrigger",
            "disarmTrigger",
        ] {
            assert!(
                output.contains(needle),
                "{label} typedef output missing trigger command type `{needle}`"
            );
        }
    }
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
    assert!(
        ts.contains("accumulate: (t: TickParams) => RuntimeValue")
            && ts.contains("type: \"number\"; readonly?: boolean;")
            && ts.contains("readonly?: false; network?: \"shared\"; accumulate:")
            && ts.contains("export type TickParams = Readonly<{ dt: RuntimeRead }>")
            && ts.contains("read(name: string | ReadonlyStateRef<unknown>): RuntimeRead;"),
        "ts must expose accumulator tracing and state-ref runtime reads:\n{ts}"
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
    assert!(
        luau.contains("accumulate: (TickParams) -> RuntimeValue")
            && luau.contains("type: \"number\", readonly: boolean?")
            && luau.contains("readonly: false?, network: \"shared\"?, accumulate:")
            && luau.contains("export type TickParams = { dt: RuntimeRead }")
            && luau.contains("read: (name: string | ReadonlyStateRef<any>) -> RuntimeRead"),
        "luau must expose accumulator tracing and state-ref runtime reads:\n{luau}"
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
        ts.contains("readonly player: {\n      readonly ammo: ReadonlyStateRef<number>;\n      readonly ammoReserve: ReadonlyStateRef<number>;\n      readonly health: ReadonlyStateRef<number>;\n      readonly maxHealth: ReadonlyStateRef<number>;")
            && ts.contains("readonly textEntry: WritableStateRef<string>;"),
        "ts GameStateRefs missing catalog path/capability refs:\n{ts}"
    );
    assert!(
        !ts.contains("postretro/game-state") && !ts.contains("ReadonlyStateValue"),
        "legacy game-state module/value handles must be gone"
    );

    let luau = generate_luau(&r);
    assert!(
        luau.contains("declare function getGameState(): GameStateRefs"),
        "luau missing getGameState declaration:\n{luau}"
    );
    assert!(
        luau.contains("ammo: ReadonlyStateRef<number>,")
            && luau.contains("ammoReserve: ReadonlyStateRef<number>,")
            && luau.contains("health: ReadonlyStateRef<number>,")
            && luau.contains("maxHealth: ReadonlyStateRef<number>,")
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
