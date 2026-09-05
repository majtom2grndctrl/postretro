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
        assert!(output.contains("export type MoverDefaults = {"));
        assert!(output.contains("autoCloseMs?: number;"));
        assert!(output.contains("movers?: MoverDefaults;"));
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
        assert!(output.contains("export type MoverDefaults = {"));
        assert!(output.contains("autoCloseMs: number?,"));
        assert!(output.contains("movers: MoverDefaults?,"));
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
            "PatrolDescriptor",
            "PatrolMode",
            "moveToAnchor",
            "pingPong",
            "BehaviorGraphEnvelope",
            "BehaviorActivityDescriptor",
            "BehaviorLayers",
            "GuardedRow",
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
            "BehaviorStateDescriptor",
            "TransitionDescriptor",
            "interrupts",
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

#[test]
fn sdk_attack_params_discriminate_weapon_and_contact_entries() {
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

    let ts_attack_union = "export type AttackParams = { weapon?: never; damage: number; maxRange: number; cooldownMs: number; engagementRadius?: number; standoffDistance?: number } | { weapon: string; damage?: never; maxRange?: never; cooldownMs?: never; engagementRadius?: number; standoffDistance?: number };";
    let luau_attack_union = "export type AttackParams = { weapon: never?, damage: number, maxRange: number, cooldownMs: number, engagementRadius: number?, standoffDistance: number? } | { weapon: string, damage: never?, maxRange: never?, cooldownMs: never?, engagementRadius: number?, standoffDistance: number? }";

    assert!(
        generated_ts.contains(ts_attack_union) && committed_ts.contains(ts_attack_union),
        "TypeScript must expose mutually exclusive contact and weapon attack entries"
    );
    assert!(
        generated_luau.contains(luau_attack_union) && committed_luau.contains(luau_attack_union),
        "Luau must expose mutually exclusive contact and weapon attack entries"
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

/// E18 Task 2: `wait`/`fire` control-step types and builders must reach both
/// SDK surfaces the same way `armTrigger`/`disarmTrigger` do (the boundary
/// inventory's settled wire shapes).
#[test]
fn wait_and_fire_step_types_are_emitted_in_both_sdk_surfaces() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    for (label, output) in [("ts", &ts), ("luau", &luau)] {
        for needle in [
            "WaitArgs",
            "FireArgs",
            "WaitStep",
            "FireStep",
            "\"@wait\"",
            "\"@fire\"",
            "wait",
            "fire",
        ] {
            assert!(
                output.contains(needle),
                "{label} typedef output missing wait/fire control-step type `{needle}`"
            );
        }
    }

    // Pin the exact settled signatures (Boundary inventory): `fire` takes
    // `Reaction<{}>`, not `Reaction<S>` — the phantom scope gate that makes
    // firing a scoped reaction a TS compile error (O30).
    assert!(
        ts.contains("export function wait(durationMs: number, opts?: { interruptible?: boolean }): SequenceStep[];"),
        "ts must declare the settled `wait` signature:\n{ts}"
    );
    assert!(
        ts.contains("export function fire(reaction: Reaction<{}> | string): SequenceStep[];"),
        "ts `fire` must accept `Reaction<{{}}>`, not a scoped reaction:\n{ts}"
    );
    assert!(
        luau.contains("declare function wait(durationMs: number, opts: { interruptible: boolean? }?): {SequenceStep}"),
        "luau must declare the settled `wait` signature:\n{luau}"
    );
    assert!(
        luau.contains("declare function fire(reaction: Reaction<any> | string): {SequenceStep}"),
        "luau `fire` relies on the V4b engine gate, not a type gate:\n{luau}"
    );
    assert!(
        luau.contains("wait: typeof(wait),") && luau.contains("fire: typeof(fire),"),
        "luau `require(\"postretro\")` virtual module must export wait/fire:\n{luau}"
    );
}

/// `defineStore` returns a pure flattened store handle. The generator
/// special-cases it (like `worldQuery`) so the static SDK block's generic
/// `defineStore<const S>` supplies its schema-keyed top-level refs and the
/// `defineMod` input accepts the opaque handle.
#[test]
fn define_store_emits_flattened_handle_and_converged_refs() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);

    // TypeScript supports both binding-name sugar and the explicit namespace.
    assert!(
        ts.contains(
            "export function defineStore<const S extends Record<string, StoreSlotSchema>>(\n    schema: S,\n  ): StoreDefinition<S>;\n  export function defineStore<const S extends Record<string, StoreSlotSchema>>(\n    namespace: string,\n    schema: S,"
        ),
        "ts defineStore must expose both binding-sugar and explicit-name arities:\n{ts}"
    );
    assert!(
        ts.contains("readonly [K in keyof S]: StateValueForSlot<S[K]>;"),
        "ts StoreDefinition missing flattened schema-keyed refs"
    );
    assert!(
        ts.contains("readonly [storeDefinitionBrand]: S;"),
        "ts StoreDefinition missing opaque store-handle identity"
    );
    assert!(
        ts.contains("Slot extends { readonly: true } ? StoreComputedRef<T> : StoreRef<T>;"),
        "ts StoreStateRefForSlot must preserve readonly schema capability"
    );
    assert!(
        ts.contains("Slot extends { type: \"number\" } ? StoreStateRefForSlot<Slot, number>"),
        "ts StateValueForSlot must route through readonly-aware ref selection"
    );
    assert!(
        ts.contains("accumulate: (t: TickParams) => RuntimeValue")
            && ts.contains("type: \"number\"; readonly?: boolean;")
            && ts.contains("readonly?: false; network?: \"shared\"; perOwner?: false; accumulate:")
            && ts.contains("perOwner?: false")
            && ts.contains("network?: \"ownerPrivate\"; perOwner: true; persist?: boolean; accumulate?: never")
            && !ts.contains("network?: \"shared\" | \"ownerPrivate\"; perOwner: true")
            && ts.contains("byPlayer(owner: SourceHandle): OwnerAddressedComputedRef<T>")
            && ts.contains("readonly owner: \"@impact.source\"")
            && ts.contains("export type TickParams = Readonly<{ dt: RuntimeRead }>")
            && ts.contains("read(name: string | ComputedRef<unknown>): RuntimeRead;")
            && ts.contains("export function read(ref: StateRef<number>): NumberRef;")
            && ts.contains("export function set(ref: Ref<number> | OwnerAddressedRef<number>, value: NumberValue): Effect;")
            && ts.contains("export function update(ref: Ref<number> | OwnerAddressedRef<number>, build: (cur: NumberRef) => NumberValue): Effect;")
            && ts.contains("export function when(cond: BoolRef, effects: readonly Effect[]): GatedEffect;"),
        "ts must expose accumulator tracing and state-ref runtime reads:\n{ts}"
    );
    // The old uniform registry-driven handle map must be gone.
    assert!(
        !ts.contains("export function defineStore(namespace: string, schema: unknown)"),
        "ts must not emit the registry-driven uniform StateValue<string> defineStore"
    );
    assert!(
        !ts.contains("readonly declaration: StoreDeclaration;")
            && !ts
                .contains("readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };"),
        "ts must not retain the old declaration/state store shape"
    );

    let luau = generate_luau(&r);
    assert!(
        luau.contains("declare function defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition"),
        "luau missing StoreDefinition defineStore declaration:\n{luau}"
    );
    assert!(
        luau.contains("export type StoreStateRef<T> = StoreComputedRef<T> | StoreRef<T>")
            && luau.contains("export type StoreDefinition = {\n  [string]: StoreStateRef<any>,\n}")
            && luau.contains("declare function defineMod(config: ModManifestInput): ModManifest"),
        "luau StoreDefinition must expose the flattened handle:\n{luau}"
    );
    assert!(
        luau.contains("accumulate: (TickParams) -> RuntimeValue")
            && luau.contains("type: \"number\", readonly: boolean?")
            && luau.contains("readonly: false?, network: \"shared\"?, perOwner: false?, accumulate:")
            && luau.contains("perOwner: false?")
            && luau.contains("network: \"ownerPrivate\"?, perOwner: true, persist: boolean?, accumulate: nil?")
            && !luau.contains("network: (\"shared\" | \"ownerPrivate\")?, perOwner: true")
            && luau.contains("byPlayer: (self: StoreComputedRef<T>, owner: SourceHandle) -> OwnerAddressedComputedRef<T>")
            && luau.contains("owner: \"@impact.source\"")
            && luau.contains("export type TickParams = { dt: RuntimeRead }")
            && luau.contains("read: (name: string | ComputedRef<any>) -> RuntimeRead")
            && luau.contains("declare function set(ref: Ref<number> | OwnerAddressedRef<number>, value: NumberValue): Effect")
            && luau.contains("declare function update(ref: Ref<number> | OwnerAddressedRef<number>, build: (cur: NumberRef) -> NumberValue): Effect"),
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
            && ts.contains("export type SwitchingDescriptor = {")
            && ts.contains("commitOnDirectSelect: boolean;")
            && ts.contains("cycleCommitDwellMs: number;")
            && ts.contains("blockDuringReload: boolean;")
            && ts.contains("switching?: SwitchingDescriptor;")
            && ts.contains("blockDuringReload?: boolean;")
            && ts.contains("defaultWeaponPlacement?: WeaponPlacementDescriptor;")
            && ts.contains("export function defineMod(config: ModManifestInput): ModManifest;")
            && ts.contains(
                "export function defineMapCatalog(entries: ModMapEntry[]): ModMapEntry[];"
            )
            && ts.contains(
                "export function defineWeaponPlacement(desc: WeaponPlacementDescriptor): WeaponPlacementDescriptor;"
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
            && luau.contains("export type SwitchingDescriptor = {")
            && luau.contains("commitOnDirectSelect: boolean,")
            && luau.contains("cycleCommitDwellMs: number,")
            && luau.contains("blockDuringReload: boolean,")
            && luau.contains("switching: SwitchingDescriptor?")
            && luau.contains("blockDuringReload: boolean?")
            && luau.contains("defaultWeaponPlacement: WeaponPlacementDescriptor?")
            && luau.contains("declare function defineMod(config: ModManifestInput): ModManifest")
            && luau.contains(
                "declare function defineMapCatalog(entries: {ModMapEntry}): {ModMapEntry}"
            )
            && luau.contains("defineMod: typeof(defineMod),")
            && luau.contains("defineMapCatalog: typeof(defineMapCatalog),")
            && luau.contains(
                "declare function defineWeaponPlacement(desc: WeaponPlacementDescriptor): WeaponPlacementDescriptor"
            )
            && luau.contains("defineWeaponPlacement: typeof(defineWeaponPlacement),"),
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
        ts.contains("readonly player: {\n      readonly ammo: ComputedRef<number>;\n      readonly ammoReserve: ComputedRef<number>;\n      readonly health: ComputedRef<number>;\n      readonly maxHealth: ComputedRef<number>;")
            && ts.contains("readonly textEntry: Ref<string>;"),
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
        luau.contains("ammo: ComputedRef<number>,")
            && luau.contains("ammoReserve: ComputedRef<number>,")
            && luau.contains("health: ComputedRef<number>,")
            && luau.contains("maxHealth: ComputedRef<number>,")
            && luau.contains("textEntry: Ref<string>,"),
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
