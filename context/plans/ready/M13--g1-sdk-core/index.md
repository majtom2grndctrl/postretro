# M13 G1a — UI SDK core: typed handles, factory layer, ingestion bridge

> Wave plan 1 of 2 (precedes **G1b**; both ship in one /orchestrate).
> Prereqs: B/C/D/E/F/TW shipped (`done/M13--*`). Grounding: `research/ui-layer.md` §15, `lib/ui.md`, `render/ui/descriptor.rs`, `sdk/lib/entities/emitters.ts`, `scripting/data_descriptors.rs`, `done/mod-state-store/`.

## Goal

Give mod authors the script-side authoring surface for the UI widget tree that prior goals built only the Rust/wire half of. `render/ui/descriptor.rs` ships the full serde model (10-kind `Widget` enum, `AnchoredTree` envelope, binds, tweens, styleRanges, focus) with no factory functions and no VM→Rust deserialization path. This plan redeems the static-authoring half: TypeScript and Luau factories producing descriptor objects, value-typed slot handles, read-only engine-slot handles (`postretro/game-state`), typed reaction handles, branded-handle ergonomics, the VM-value→`AnchoredTree` bridge functions, the `sdk/lib/ui/` layout, and the single `LocalizedText` text-alias chokepoint.

This is the contract-locking convergence surface G2 and BIS bind against, so casing, the typed-handle contract, and the factory shape settle here. G1a ships no registration, no lifecycle, no engine-side runtime work (→ G1b): a factory-produced tree is proven by passing it through the bridge functions in a direct test call, not by mounting or rendering it. G1b follows and consumes G1a's factories and bridge functions.

## Scope

### In scope

- **Value-typed slot handles.** Make `defineStore` return handles carrying the slot's declared value type — `StateValue<number>` / `StateValue<boolean>` / `StateValue<string>` — instead of the uniform `StateValue<string>` it returns today. Mechanism (per review): a static generic `defineStore<const S>` declaration in the SDK lib block that infers slot types from the schema literal, plus a `worldQuery`-style special-case in the typedef generator — **not** registry-threaded types (per-slot types live only in the runtime `schema` argument, absent at typedef-emission). Update the `ModManifest`/`ModManifestResult` parity guard. The foundation G1a's typed bind ergonomics and the brand-mismatch contract rest on.
- **Component factory functions.** Capitalized constructors for all 10 widget kinds — `widgets.{ts,luau}` (Text, Panel, Image, Button, Slider, Bar, Spacer) and `layout.{ts,luau}` (VStack, HStack, Grid). Props object first, positional `children` after for containers (Compose/SwiftUI lineage, not React). Synchronous validation throwing a field-named `Error`, matching `sdk/lib/entities/emitters.ts`. Bound props (`bind`, `tween`, `styleRanges`) authored as factory props emitting the shipped descriptor fields.
- **Placement-envelope builder.** A `Tree(...)` factory producing the `AnchoredTree` shape (`anchor`, `offset`, `root`, optional `captureMode`/`initialFocus`/`textEntryTarget`).
- **Branded-handle ergonomics.** `.get()`/`.set()` accessor wrappers over the value-typed store-slot handles: `.set(v)` produces a `setState` reaction descriptor (typed to the slot's `T`), `.get()` yields the typed bind reference a widget binds to. The store-handle wrapper type is exported through the barrel for general SDK use. (G1b's `ui.createLocalState()` uses a *distinct* presentation handle — see G1b — not this store-writing type.)
- **Engine-owned slot handles (`postretro/game-state`).** A generator-emitted, **read-only** typed handle group for the engine-owned state namespaces (`player.*`, `world.*`, `match.*`), imported named: `import { player } from "postretro/game-state"`. Same typed-handle mechanism as `defineStore` minus `.set()` — engine slots are read-only to mods, so `player.health.get()` is `ReadonlyStateValue<number>` and `player.health.set(...)` is a type error. Emitted by the typedef generator from the engine slot registry (the same source the rest of the typedefs come from). Resolves how an author binds an engine slot without a raw slot-name string.
- **Typed reaction handles.** Widen reaction-reference authoring props (`Button`'s `onPress`, crossing targets) to accept the `NamedReactionDescriptor` that `defineReaction` returns — the factory reads its `.name` and emits the unchanged `onPress: string` wire form — so authors reference reactions by a typed handle (go-to-definition, no silent name typos) instead of a bare string. A bare string stays valid (the shipped path). `defineReaction`'s `name` arg becomes optional; when omitted a **deterministic** id is auto-generated (registration-order/content-derived — must be stable across runs, since crossings and the wire reference it). **Note:** additive type-surface change to the shipped `defineReaction`/`ButtonWidget` authoring types; wire form and runtime unchanged.
- **Deserialization bridge.** `anchored_tree_from_js_value` + `anchored_tree_from_lua_value` (Rust), converting a VM-returned descriptor value into a typed `AnchoredTree`, defined in `scripting/data_descriptors.rs` beside `entity_descriptor_from_js`/`entity_descriptor_from_lua` (the established per-runtime field-reader pattern — no `serde_json::Value` lowering, no single Luau twin), re-exported via `conv.rs` if that matches the existing pattern. Named load-time error on malformed input, never a panic. Test-callable directly; G1b wires them into the manifest drains.
- **Text-alias chokepoint.** A single `LocalizedText` type alias (= `string` today) consumed by every user-facing text prop from its first line.
- **SDK layout + emission.** `sdk/lib/ui/{widgets,layout,text}.{ts,luau}` beside the existing `reactions.{ts,luau}`; barrel re-exports in `sdk/lib/index.ts`; the typedef blocks updated in `crates/postretro/src/scripting/typedef.rs`; generated typedefs regenerated.

### Out of scope

- All registration, lifecycle, and engine-side runtime work: manifest arms, the tiered tree registry, the always-on compose mechanism, theme/font script registration, `ui.createLocalState()`. → **G1b**.
- JSX-via-SWC, discriminated-union narrowing per kind, a11y compile-preconditions, template-literal nav types. → **G2**.
- The localization mechanism (per-locale string tables). → deferred.
- The `descriptor.rs` wire model is consumed unchanged here; G1b adds the additive `localState` cell-declaration field.

## Acceptance criteria

- [ ] The generated `.d.ts`/`.d.luau` (via the static SDK block) declare `defineStore` so a `number` slot's handle is typed `StateValue<number>` and a `boolean` slot `StateValue<boolean>` — asserted by a Rust snapshot test over the emitted typedef block (the `gen-script-types` drift mechanism). The `StateValue<boolean>`-to-numeric-prop mismatch ships as a documented author-facing `@ts-expect-error` fixture (a review gate / author example — the repo has no `tsc` CI to run it).
- [ ] `import { player } from "postretro/game-state"` yields read-only typed handles for engine-owned slots: `player.health.get()` is a `ReadonlyStateValue<number>` bind ref and `.set(...)` is absent from the type — asserted via the generated-typedef snapshot (the `game-state` module declares `.get()` only and is emitted from the engine slot registry, not hand-written).
- [ ] `defineReaction(body)` with the name omitted returns a handle carrying a deterministic, run-stable id; `Button({ onPress: handle })` emits `onPress: "<id>"` byte-identical to `Button({ onPress: "<id>" })`, and a bare-string `onPress` still compiles and round-trips. Re-running registration yields the same auto-id (determinism check).
- [ ] Every factory in `widgets.ts`/`layout.ts` produces an object that, passed through `anchored_tree_from_js_value`, yields the matching `Widget` variant and re-serializes byte-identically to the `descriptor.rs` round-trip fixture. Keys are camelCase (`fontSize`, `flexGrow`, `onPress`).
- [ ] TS and Luau factories emit identical JSON for identical inputs (cross-runtime parity test, the `done/M7--movement-scripts` precedent).
- [ ] A factory with an invalid prop throws an `Error` naming the factory and field; a valid call with optionals omitted succeeds with documented defaults.
- [ ] A `bind` authored with a `tween` deserializes to `TextTween`/`PanelTween` byte-identically; a `styleRanges`-bearing `text`/`panel`/`bar` deserializes to the `StyleRanges` fixture; a bind without a tween emits no `tween` key and a widget without styleRanges emits no `styleRanges` key.
- [ ] `Tree(...)` with `captureMode: "capture"` deserializes to `CaptureMode::Capture`; omitted → `Passthrough`, re-serialized without the key; explicit `"passthrough"` accepted and round-trips to omission.
- [ ] A store handle's `.set(v)` produces a descriptor byte-identical to the shipped `setState(slot, v)`; `.get()` produces a typed bind reference a `Text`/`Bar`/`Slider` factory accepts, resolving to the same `TextBind`/`SliderBind { slot }` wire shape. Bind capability per kind: `text`→`TextBind`, `panel`→`PanelBind`, `slider`/`bar`→`SliderBind`; other kinds none.
- [ ] `anchored_tree_from_js_value` and `anchored_tree_from_lua_value` (in `data_descriptors.rs`) surface a named load-time error on a malformed tree and do not panic; a well-formed tree converts cleanly. Both mirror `entity_descriptor_from_js`/`_from_lua`.
- [ ] Every user-facing text prop is typed `LocalizedText` (= `string`), verified by a grep/review gate over the factory signatures (the repo has no `tsc` CI to assert it as a compile error). The alias is a single definition, so a future swap is one edit; an author-facing fixture documents the intended compile-time effect.
- [ ] `sdk/lib/index.ts` re-exports every new factory/type; typedefs regenerate clean and `gen-script-types` reports no drift.

## Tasks

### Task 1: Typed handle surfaces
Three type-surface changes to shipped contracts, all in the generator + static SDK block (no runtime change):
(1) **Value-typed slot handles.** Replace the hardcoded `StateValue<string>` handle map at **both** emission sites — `typedef.rs:148` (TS `StoreHandles` mapping) and `typedef.rs:257` (Luau) — and give `defineStore` a `worldQuery`-style special-case in the generator (skip the registry-driven emission, ~`typedef.rs:450`) so a hand-written generic `defineStore<const S>` in the static SDK lib block supplies the type. The generic maps each schema slot's `type` discriminant to its value type (`{type:"number"}` → `StateValue<number>`, `"boolean"` → `StateValue<boolean>`, else `StateValue<string>`).
(2) **Read-only engine-slot handles.** Emit a `postretro/game-state` module exposing read-only typed handles (`ReadonlyStateValue<T>`, `.get()` only) for the engine-owned slot namespaces, generated from the engine slot registry the generator already reads.
(3) **Typed reaction handles.** Make `defineReaction`'s `name` optional (deterministic auto-id when omitted), keep its `NamedReactionDescriptor` return, and widen the reaction-reference authoring types (`ButtonWidget.onPress`, crossing targets) to `NamedReactionDescriptor | string` (the bare-string shipped path stays valid). Wire form (`onPress: string`) unchanged.
`Depends on` nothing. **Note:** edits the shipped `done/mod-state-store` type surfaces (`defineStore`, `defineReaction`, `ButtonWidget`); runtime behavior unchanged — type-surface only. The `ModManifest` parity guard is untouched here (it concerns `name`/`entities`, not handles; the manifest-field update lives in G1b Task 1).

### Task 2: Text-alias chokepoint
Add `LocalizedText` (= `string`) in `sdk/lib/ui/text.{ts,luau}`, exported through the barrel and emitted into the typedef blocks. Type-only, lands before factories consume it. `Depends on` nothing.

### Task 3: Widget + layout factories
Implement `widgets.{ts,luau}` and `layout.{ts,luau}` mirroring `emitter()`: `Props` type, field-named validation, documented defaults, camelCase output keys. Text props use `LocalizedText`; bound props accept Task 1's typed handles and emit `bind`/`tween`/`styleRanges`. Bind capability is per kind: `text`→`TextBind`, `panel`→`PanelBind`, `slider`/`bar`→`SliderBind`; `image`/`spacer`/`button` and the containers take no bind. `Button`'s `onPress` accepts a Task 1 reaction handle (or a bare name string), emitting the unchanged `onPress: string` wire form from the handle's `.name`. TS bare capitalized exports; Luau the same factories as members of a returned module table (the `UiReactionsSdk` precedent). `Depends on` Tasks 1, 2.

### Task 4: Envelope + handle ergonomics
Implement the `Tree(...)` envelope factory and the `.get()`/`.set()` wrappers over Task 1's typed handles; export the store-handle wrapper type through the barrel. `.set()` delegates to the shipped `setState` builder. Read-only engine-slot handles (Task 1, `postretro/game-state`) expose `.get()` only — no `.set()`. `Depends on` Task 3.

### Task 5: Deserialization bridge
Implement `anchored_tree_from_js_value` / `anchored_tree_from_lua_value` in `data_descriptors.rs` beside `entity_descriptor_from_js`/`_from_lua` (no `serde_json::Value` lowering), re-exported via `conv.rs` per the existing pattern; return a typed `AnchoredTree` or a named error. Not wired into any manifest parser — test-callable only; G1b wires them. `Depends on` Task 4.

### Task 6: Barrel, typedef emission, tests
Wire exports into `index.ts`; update the typedef blocks; regenerate typedefs. Add parity, factory→bridge→round-trip, and generated-typedef snapshot tests (asserting `defineStore` emits per-slot `StateValue<T>`); the typed-handle and `LocalizedText` compile-fail cases ship as documented author-facing `@ts-expect-error` fixtures (review gates — no `tsc` CI exists). `Depends on` Tasks 1–5.

## Sequencing

**Prereq:** B/C/D/E/F/TW shipped.
**Phase 1 (concurrent):** Task 1 (typed handles), Task 2 (text alias).
**Phase 2 (sequential):** Task 3 → Task 4 → Task 5.
**Phase 3 (sequential):** Task 6.

## Rough sketch

**Factory shape** follows `emitter()`; UI-widget factories emit camelCase keys (the `Widget` wire form) whereas `emitter()` emits snake_case. **Export surface:** TS bare capitalized exports; Luau module-table members (the `reactions.luau` `UiReactionsSdk` pattern); equivalence enforced by the parity test. **Typed handles:** the heavy task — the TS mapped/generic type is straightforward, but the typedef generator must special-case `defineStore` (mirroring `worldQuery` at `typedef.rs:450`) since per-slot schema types are a runtime arg, absent at emission. **Bridge** mirrors `entity_descriptor_from_js`/`entity_descriptor_from_lua` (defined in `data_descriptors.rs`), reading the VM value into the typed `AnchoredTree`, mapping errors to a named diagnostic.

**Key files:** `sdk/lib/ui/{widgets,layout,text}.{ts,luau}`, `sdk/lib/index.ts`, `sdk/types/postretro.d.ts`/`.d.luau`, `crates/postretro/src/scripting/typedef.rs`, `scripting/data_descriptors.rs` (bridge fns beside `entity_descriptor_from_*`), `scripting/conv.rs` (re-export), `scripting/primitives/mod.rs` (parity guard), `render/ui/descriptor.rs` (wire target, unchanged).

## Boundary inventory

Casing: Rust snake_case ↔ wire/JS/TS/Luau camelCase. Widget `kind` tags lowercase/camel. Factory output keys camelCase (distinct from entity-component factories' snake_case).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Text … Bar (10 kinds) | `Widget::Text` … `Widget::Bar` | `"text"` … `"bar"` (`"vstack"`/`"hstack"` lowercase) | `Text(props)` bare | `Ui.Text(props)` table member |
| Envelope | `AnchoredTree` | `{anchor,offset,root,...}` | `Tree(props, root)` | `Ui.Tree` |
| font size / flex grow / on-press / capture mode / tween dur | `font_size`/`flex_grow`/`on_press`/`capture_mode`/`duration_ms` | `fontSize`/`flexGrow`/`onPress`/`captureMode`/`durationMs` | same | same |
| typed handle | (typedef only) | n/a | `StateValue<T>` | `StateValue<T>` |
| engine slot (read-only) | engine slot registry | binds by slot name | `import { player } from "postretro/game-state"`; `player.health.get()` (`ReadonlyStateValue<T>`) | `player.health:get()` |
| reaction handle | `NamedReactionDescriptor` | `onPress: "<id>"` (string) | `defineReaction(body)` → handle; `onPress: handle` | `defineReaction(body)` → handle |
| displayed text | n/a (`String`) | JSON string | `LocalizedText` (= `string`) | `LocalizedText` |
| slot set | `setState` primitive | `{"primitive":"setState",...}` | `handle.set(v)` | `handle:set(v)` |
| bridge | `anchored_tree_from_js_value` / `_from_lua_value` (in `data_descriptors.rs`) | n/a | n/a | n/a |

**Bind capability:** `text` (`TextBind`), `panel` (`PanelBind`), `slider`/`bar` (`SliderBind`); `image`/`spacer`/`button` + containers take no bind. Pinned by `descriptor.rs`.

## Decisions

- **Slot handles are value-typed via a static generic + generator special-case, not registry-threaded types.** Per-slot types are a runtime call arg, absent at typedef emission; the feasible path is a generic `defineStore<const S>` in the static SDK block plus a `worldQuery`-style carve-out in the generator. Rejected: threading schema types through the Rust generator (structurally impossible); leaving `StateValue<string>` and validating only at runtime (loses the contract-locking type safety).
- **The bridge mirrors `entity_descriptor_from_js/_from_lua` and lives in `data_descriptors.rs`.** That is the established per-runtime field-reader pattern; there is no `serde_json::Value` lowering and no single Luau twin. Rejected: the prior "in `staged_manifest.rs` via serde Deserialize" framing (wrong module, wrong mechanism).
- **`ui.createLocalState()` uses a distinct handle, so G1a exports no shared handle type for it.** G1a's store-handle `.set()` writes the authoritative store (`setState`); `localState` must never write the store, so it needs its own presentation handle (G1b). Rejected: a single shared handle type (would give `localState` store-writing semantics).
- **`LocalizedText` lands in G1a**; it must exist before the first factory types a text prop. Verified by a grep/review gate (the repo has no `tsc` CI); the alias is one definition so a future `LocalizedText` swap is a single edit.
- **UI-widget factories emit camelCase; TS bare / Luau module-table; equivalence by parity.** Each matches its own wire form and shipped per-runtime convention.
- **Engine-owned slots are read via a generated `postretro/game-state` handle group, not raw slot-name strings.** Owner direction: `import { player } from "postretro/game-state"` reads better and gives typed, IDE-navigable binds; handles are `ReadonlyStateValue<T>` (engine slots are read-only to mods). Resolves a hand-wave in the prior surface (how a mod referenced an engine slot). Named export, not default (the SDK has no default exports). Rejected: binding by raw `{ slot: "player.health" }` string (stringly-typed, no safety).
- **`defineReaction` returns a typed reaction handle that reaction-reference props accept; bare strings stay valid.** The same stringly-typed→typed win as slot handles, eliminating silent `onPress` name typos. `name` becomes optional with a deterministic auto-id. Wire form unchanged (`onPress: string`). Rejected: dropping the bare-string path (breaks shipped content); a live `onPress: () => …` callback — see the convention below.
- **SDK naming + the no-live-callbacks rule.** `define*` = things the engine **registers** (manifest-drained, engine-side lifetime): `defineStore`, `defineReaction`. `create*` = things **constructed inline**, not registered: `createLocalState` (G1b). And reactions are **data, not callbacks**: `choice.set("easy")` is a descriptor; a `() => …` form is rejected because the VM drops after registration — there is no live function to call back into (`scripting.md` §11). This also forecloses read-then-compute behavior (e.g. a boolean *toggle*) until the M14 behavior-IR.

## Open questions

None. Typed-handle mechanism and bridge location resolved against source; remaining items resolved by owner.
