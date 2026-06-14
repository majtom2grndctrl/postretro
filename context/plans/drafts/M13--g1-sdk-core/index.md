# M13 G1a — UI SDK core: typed handles, factory layer, ingestion bridge

> Wave plan 1 of 3 (concurrent with **G1-infra**; both precede **G1b**; all ship in one /orchestrate).
> Prereqs: B/C/D/E/F/TW shipped (`done/M13--*`). Grounding: `research/ui-layer.md` §15, `lib/ui.md`, `render/ui/descriptor.rs`, `sdk/lib/entities/emitters.ts`, `done/mod-state-store/`.

## Goal

Give mod authors the script-side authoring surface for the UI widget tree that prior goals built only the Rust/wire half of. `render/ui/descriptor.rs` ships the full serde model (10-kind `Widget` enum, `AnchoredTree` envelope, binds, tweens, styleRanges, focus) with no factory functions and no VM→Rust deserialization path. This plan redeems the static-authoring half: TypeScript and Luau factories producing descriptor objects, value-typed slot handles, branded-handle ergonomics, the VM-value→`AnchoredTree` bridge functions, the `sdk/lib/ui/` layout, and the single `LocalizedText` text-alias chokepoint.

This is the contract-locking convergence surface G2 and BIS bind against, so casing, the typed-handle contract, and the factory shape settle here. G1a ships no registration, no lifecycle, no render-loop wiring (→ G1-infra, G1b): a factory-produced tree is proven by passing it through the bridge functions in a direct test call, not by mounting or rendering it.

## Scope

### In scope

- **Value-typed slot handles.** Change `defineStore` so the handles it returns carry the slot's declared value type — `StateValue<number>` / `StateValue<boolean>` / `StateValue<string>` — instead of the uniform `StateValue<string>` it returns today. This threads per-slot types from the schema into the returned handle map (TS mapped type; Luau twin) and into the typedef generator, and updates the `ModManifest`/registered-type parity guard. The foundation G1a's typed bind ergonomics and the brand-mismatch contract rest on.
- **Component factory functions.** Capitalized constructors for all 10 widget kinds — `widgets.{ts,luau}` (Text, Panel, Image, Button, Slider, Bar, Spacer) and `layout.{ts,luau}` (VStack, HStack, Grid). Props object first, positional `children` after for containers (Compose/SwiftUI lineage, not React). Synchronous validation throwing a field-named `Error`, matching `sdk/lib/entities/emitters.ts`. Bound props (`bind`, `tween`, `styleRanges`) are authored here as factory props that emit the shipped descriptor fields.
- **Placement-envelope builder.** A `Tree(...)` factory producing the `AnchoredTree` shape (`anchor`, `offset`, `root`, optional `captureMode`/`initialFocus`/`textEntryTarget`).
- **Branded-handle ergonomics.** `.get()`/`.set()` accessor wrappers over the value-typed slot handles: `.set(v)` produces a `setState` reaction descriptor (typed to the slot's `T`), `.get()` yields the typed bind reference a widget binds to. The wrapper handle type is exported through the barrel as a named type so G1b's `ui.localState()` returns the identical shape.
- **Deserialization bridge.** `anchored_tree_from_js_value` + `anchored_tree_from_lua_value` (Rust) converting a VM-returned descriptor value into a typed `AnchoredTree`, mirroring the existing `entity_descriptor_from_js`/`entity_descriptor_from_lua` shape (these are the established pattern — there is no `serde_json::Value` lowering and no single `manifest_from_lua_value` twin). Named load-time error on malformed input, never a panic. Test-callable directly; G1b wires them into the manifest drains.
- **Text-alias chokepoint.** A single `LocalizedText` type alias (= `string` today) consumed by every user-facing text prop from its first line.
- **SDK layout + emission.** `sdk/lib/ui/{widgets,layout,text}.{ts,luau}` beside the existing `reactions.{ts,luau}`; barrel re-exports in `sdk/lib/index.ts`; `TS_SDK_LIB_BLOCK`/`LUAU_SDK_LIB_BLOCK` updated in `crates/postretro/src/scripting/typedef.rs`; generated typedefs regenerated.

### Out of scope

- Manifest registration, named-tree/theme/font script registration, `ui.localState()`, the register→VM-drop lifecycle. → **G1b**.
- The per-level vs engine-global tree-registry split, the owned override-merged `UiTheme`, wiring `layout_gameplay_tree` into the frame loop. → **G1-infra**.
- JSX-via-SWC, discriminated-union narrowing per kind, a11y compile-preconditions, template-literal nav types. → **G2**.
- The localization mechanism (per-locale string tables). → deferred.

## Acceptance criteria

- [ ] `defineStore` returns handles whose type parameter matches the declared slot type: a `number` slot yields `StateValue<number>`, a `boolean` slot `StateValue<boolean>`. Binding a `StateValue<boolean>` handle to a numeric-only widget prop is a TypeScript compile error; a `StateValue<number>` to the same prop compiles. The `ModManifest`/`ModManifestResult` parity guard (`primitives/mod.rs`) still passes.
- [ ] Every factory in `widgets.ts`/`layout.ts` produces an object that, passed through `anchored_tree_from_js_value`, yields the matching `Widget` variant and re-serializes byte-identically to the `descriptor.rs` round-trip fixture. Keys are camelCase (`fontSize`, `flexGrow`, `onPress`) — a snake_case or stray key fails.
- [ ] TS and Luau factories emit identical JSON for identical inputs (cross-runtime parity test, the `done/M7--movement-scripts` precedent).
- [ ] A factory with an invalid prop throws an `Error` naming the factory and field; a valid call with optionals omitted succeeds with documented defaults.
- [ ] A `bind` authored with a `tween` deserializes to `TextTween`/`PanelTween` byte-identically; a `styleRanges`-bearing `text`/`panel`/`bar` deserializes to the `StyleRanges` fixture; a bind without a tween emits no `tween` key and a widget without styleRanges emits no `styleRanges` key.
- [ ] `Tree(...)` with `captureMode: "capture"` deserializes to `CaptureMode::Capture`; omitted → `Passthrough` and re-serializes without the `captureMode` key. An explicit `"passthrough"` is accepted and round-trips to omission (documented).
- [ ] A store handle's `.set(v)` produces a descriptor byte-identical to the shipped `setState(slot, v)`; `.get()` produces a typed bind reference a `Text`/`Bar`/`Slider` factory accepts, resolving to the same `TextBind`/`SliderBind { slot }` wire shape as a hand-authored bind. Which kinds/props accept a bind is pinned in the Boundary inventory.
- [ ] `anchored_tree_from_js_value` and `anchored_tree_from_lua_value` surface a named load-time error on a malformed tree (unknown `kind`, misspelled field) and do not panic; a well-formed tree converts cleanly. Both mirror `entity_descriptor_from_js`/`_from_lua`.
- [ ] Every user-facing text prop is typed `LocalizedText`; changing the alias to a distinct branded type produces compile errors only at unmigrated call sites (verified by a temporary edit, reverted).
- [ ] `sdk/lib/index.ts` re-exports every new factory/type; typedefs regenerate clean and `gen-script-types` reports no drift.

## Tasks

### Task 1: Value-typed slot handles
Change `defineStore` to thread per-slot value types into the returned handle map: TS mapped type over the schema, Luau twin, and the typedef generator (`typedef.rs` — today hardcodes `StateValue<string>` at the TS and Luau emission sites). Update the `ModManifest` registered-type vs `ModManifestResult` parity guard (`primitives/mod.rs`). `Depends on` nothing. **Note:** edits a shipped `done/mod-state-store` contract surface — keep `defineStore`'s runtime behavior unchanged; this is a type-surface change only.

### Task 2: Text-alias chokepoint
Add `LocalizedText` (= `string`) in `sdk/lib/ui/text.{ts,luau}`, exported through the barrel and emitted into the typedef blocks. Type-only. Lands before the factories consume it. `Depends on` nothing.

### Task 3: Widget + layout factories
Implement `widgets.{ts,luau}` and `layout.{ts,luau}`. Mirror `emitter()`: a `Props` type, synchronous validation throwing a field-named `Error`, documented defaults, camelCase output keys. Text props use `LocalizedText`; bound props accept the typed handles from Task 1 and emit `bind`/`tween`/`styleRanges` fields. TS bare capitalized exports; Luau the same factories as members of a returned module table (the `UiReactionsSdk` precedent). `Depends on` Tasks 1, 2.

### Task 4: Envelope + handle ergonomics
Implement the `Tree(...)` envelope factory and the `.get()`/`.set()` wrappers over Task 1's typed handles; export the wrapper handle type through the barrel for G1b reuse. `.set()` delegates to the shipped `setState` builder. `Depends on` Task 3.

### Task 5: Deserialization bridge
Implement `anchored_tree_from_js_value` / `anchored_tree_from_lua_value` mirroring `entity_descriptor_from_js`/`_from_lua` (no `serde_json::Value` lowering), returning a typed `AnchoredTree` or a named error. Not wired into any manifest parser — test-callable only; G1b wires them. `Depends on` Task 4 (tests feed factory output through it).

### Task 6: Barrel, typedef emission, tests
Wire exports into `index.ts`; update the typedef blocks; regenerate typedefs. Add the parity, factory→bridge→round-trip, and typed-handle compile-fail tests. `Depends on` Tasks 1–5.

## Sequencing

**Prereq:** B/C/D/E/F/TW shipped.
**Phase 1 (concurrent):** Task 1 (typed handles), Task 2 (text alias) — independent foundations.
**Phase 2 (sequential):** Task 3 (factories) → Task 4 (envelope + ergonomics) → Task 5 (bridge).
**Phase 3 (sequential):** Task 6 (barrel/typedef/tests).

## Rough sketch

**Factory shape** follows `sdk/lib/entities/emitters.ts` — one difference: UI-widget factories emit **camelCase** keys (the `Widget` wire form) whereas `emitter()` emits snake_case (the component wire form). **Export surface:** TS components are bare capitalized exports; Luau exposes them as members of a returned module table (matching `reactions.luau`'s `UiReactionsSdk`); equivalence is enforced by the parity test, not token identity. **Bridge:** mirror `entity_descriptor_from_js`/`entity_descriptor_from_lua` — read the VM value into the typed `AnchoredTree` directly, map errors to a named load-time diagnostic. **Typed handles:** the heaviest task is the `defineStore` typedef-generator change; the TS mapped type is straightforward, the generator emission and parity guard are the real work.

**Key files:** `sdk/lib/ui/{widgets,layout,text}.{ts,luau}`, `sdk/lib/index.ts`, `sdk/types/postretro.d.ts`/`.d.luau`, `crates/postretro/src/scripting/typedef.rs`, `scripting/staged_manifest.rs` (the `entity_descriptor_from_*` siblings), `scripting/primitives/mod.rs` (parity guard), `render/ui/descriptor.rs` (wire target, unchanged).

## Boundary inventory

Casing: Rust snake_case ↔ wire/JS/TS/Luau camelCase. Widget `kind` tags lowercase/camel. Factory output keys camelCase (distinct from entity-component factories' snake_case).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Text … Bar (10 kinds) | `Widget::Text` … `Widget::Bar` | `"text"` … `"bar"` (`"vstack"`/`"hstack"` lowercase) | `Text(props)` bare | `Ui.Text(props)` table member |
| Envelope | `AnchoredTree` | `{anchor,offset,root,...}` | `Tree(props, root)` | `Ui.Tree` |
| font size / flex grow / on-press / capture mode / tween dur | `font_size`/`flex_grow`/`on_press`/`capture_mode`/`duration_ms` | `fontSize`/`flexGrow`/`onPress`/`captureMode`/`durationMs` | same | same |
| typed handle | (typedef only) | n/a | `StateValue<T>` | `StateValue<T>` |
| displayed text | n/a (`String`) | JSON string | `LocalizedText` (= `string`) | `LocalizedText` |
| slot set | `setState` primitive | `{"primitive":"setState",...}` | `handle.set(v)` | `handle:set(v)` |
| bridge | `anchored_tree_from_js_value` / `_from_lua_value` | n/a | n/a | n/a |

**Bind capability** (which kinds accept a `.get()` bind reference): `text` (`TextBind`), `panel` (`PanelBind`), `slider`/`bar` (`SliderBind`). `image`/`spacer`/`button` and the containers take no bind. Pinned by `descriptor.rs`.

## Decisions

- **Slot handles are value-typed (`StateValue<T>`), per owner direction.** Threads per-slot types from `defineStore`'s schema so bind mismatches are compile errors. Accepted cost: edits the shipped `mod-state-store` typedef surface + parity guard. Rejected: leaving `StateValue<string>` and validating at runtime only (loses the contract-locking type safety G2/BIS want).
- **The bridge mirrors `entity_descriptor_from_js/_from_lua`, not a `serde_json::Value` path.** Source has no `manifest_from_lua_value` twin and never lowers to `serde_json::Value`; the established pattern is per-runtime descriptor readers. Rejected: the original "reuse `manifest_from_js_value` + serde Deserialize" framing (does not match the codebase).
- **`LocalizedText` lands in G1a.** It must exist before the first factory types a text prop; G1b/G1-infra touch no displayed text. Resolves the owner's coupling question.
- **UI-widget factories emit camelCase; entity-component factories snake_case.** Each matches its own wire form; one casing would break round-trip identity for one side.
- **TS bare exports / Luau module-table members; equivalence by parity.** Matches the shipped `reactions` convention per runtime; the parity test pins it, not literal token identity.

## Open questions

None. Fork resolved by owner (typed handles); remaining candidates resolved against source and the established `entity_descriptor_from_*` pattern.
