# M13 G1a — UI SDK core: ingestion bridge + factory layer

> Wave plan 1 of 2 (sequenced before **G1b — lifecycle + localState**; both ship in one `/orchestrate`).
> Prereqs: B (`done/M13--descriptor-tree-layout`), C (`done/M13--state-system`), D (`done/M13--fonts-theming`), E (`done/M13--hud-dynamics`), F (`done/M13--input-breadth`), TW (`done/M13--ui-value-tweening`) — all shipped. Grounding: `research/ui-layer.md` §15, `lib/ui.md`, `render/ui/descriptor.rs`, `sdk/lib/entities/emitters.ts`.

## Goal

Give mod authors a script-side authoring surface for the UI widget tree that prior goals built only the Rust/wire half of. B–F and TW shipped the full serde descriptor model (`render/ui/descriptor.rs`: the 10-kind `Widget` enum, the `AnchoredTree` envelope, binds, tweens, styleRanges, focus) and the engine evaluators — but no factory functions and no VM→Rust ingestion path. Every prior goal routed its script-facing seam **→ G1**. This plan redeems the *static authoring* half: TypeScript and Luau factory functions that produce descriptor objects, the deserialization bridge that turns a VM-returned descriptor into a typed `AnchoredTree`, branded-handle ergonomics over store slots, the `sdk/lib/ui/` file layout, and the single text-alias chokepoint that all user-facing text routes through from day one.

This is the contract-locking convergence point G2 and BIS bind against, so the authoring surface and casing settle here. The companion plan (G1b) builds the dynamic half — the register→VM-drop lifecycle that places these descriptors into engine-global vs. per-level registries, plus `ui.localState()`. G1a ships no registration and no lifecycle: a factory-produced tree is proven by deserializing it through the bridge, not by mounting it.

## Scope

### In scope

- **Component factory functions.** Capitalized constructors for all 10 widget kinds in `sdk/lib/ui/widgets.{ts,luau}` (Text, Panel, Image, Button, Slider, Bar, Spacer) and `sdk/lib/ui/layout.{ts,luau}` (VStack, HStack, Grid). Props object first, positional `children` after for containers (research §15, Compose/SwiftUI lineage — **not** React). Each validates synchronously and throws naming the offending field, matching the `sdk/lib/entities/emitters.ts` exemplar.
- **Placement-envelope builder.** A factory producing the `AnchoredTree` wire shape (`anchor`, `offset`, `root`, optional `captureMode`/`initialFocus`/`textEntryTarget`) — the top-level wrapper a tree is authored as.
- **Branded-handle ergonomics.** `.get()` / `.set()` accessor wrappers over the `StateValue<T>` dotted-name slot handles `defineStore` returns (the `audio.master.get()/.set()` seam C routed here): `.set(v)` produces a `setState` reaction descriptor (reusing the shipped `setState` primitive); `.get()` yields the bound-slot reference a widget binds to. Keeps `StateValue<T>`'s brand so binding a boolean handle to a numeric widget stays a type error.
- **Tween + styleRanges + bind authoring.** Factory props that emit the shipped `bind` / `tween` / `styleRanges` descriptor fields (TW's empty JS/Luau boundary columns; E's inline `styleRanges` field) — authoring sugar over fields the wire format already carries.
- **Deserialization bridge.** A VM-returned descriptor value → typed `AnchoredTree` path, reusing the existing manifest conversion (`scripting/staged_manifest.rs` `manifest_from_js_value` + its Luau twin) and `Widget`/`AnchoredTree` serde `Deserialize`. Malformed trees produce a named load-time error, never a panic.
- **Text-alias chokepoint.** A single SDK text type alias (`LocalizedText`, resolving to `string` today) that every user-facing text prop on every factory consumes from its first line, so the future localization swap is a type-alias change + regenerate, not a rewrite (research §15 i18n note).
- **SDK file layout + emission.** The `sdk/lib/ui/` files above alongside the existing `reactions.{ts,luau}`; barrel re-exports in `sdk/lib/index.ts`; `TS_SDK_LIB_BLOCK` / `LUAU_SDK_LIB_BLOCK` updated in `crates/postretro/src/scripting/typedef.rs`; generated typedefs (`sdk/types/postretro.d.ts` / `.d.luau`) regenerated.

### Out of scope

- The register→VM-drop lifecycle, named-tree script registration, theme/font script registration. → **G1b** and (theme) the lifecycle plan.
- `ui.localState()` and modder-defined-component instance state. → **G1b**.
- JSX-via-SWC, discriminated-union type narrowing per kind, a11y compile-preconditions (`label`/`labelledBy` required, `Announce`), template-literal nav-intent types. → **G2**.
- The localization *mechanism* (per-locale string tables). Only the type-alias chokepoint lands here. → deferred (research §19).
- Any new widget kind, the drawn cursor, screen-space effects. → **F follow-ups / SE / BIS**.

## Acceptance criteria

- [ ] Every factory in `sdk/lib/ui/widgets.ts` and `layout.ts` produces an object that deserializes through the bridge into the matching `render/ui/descriptor.rs` `Widget` variant, and the re-serialized JSON is byte-identical to the hand-authored fixture in `descriptor.rs`'s round-trip tests (e.g. `Text({...})` → the `{"kind":"text","content":...,"fontSize":...}` form). A factory emitting a stray or snake_case key fails this — keys are camelCase (`fontSize`, `flexGrow`, `onPress`).
- [ ] The TS and Luau factory for a given kind emit identical JSON for identical inputs, asserted by a cross-runtime parity test (the `done/M7--movement-scripts` parity-test precedent). A divergence in field order or defaults fails.
- [ ] A factory called with an invalid prop (missing required field, out-of-range number, wrong arity) throws an `Error` naming the offending field and the factory — and a valid call with all optionals omitted succeeds, filling documented defaults.
- [ ] A `Tree(...)`-built envelope with `captureMode: "capture"` deserializes to `AnchoredTree { capture_mode: CaptureMode::Capture }`; the same envelope with capture mode omitted deserializes to `Passthrough` and re-serializes **without** the `captureMode` key (pre-F wire identity).
- [ ] A store handle's `.set(v)` produces a descriptor byte-identical to the shipped `setState(slot, v)` reaction; `.get()` produces a bind reference a `Text`/`Bar` factory accepts and that resolves to the same `TextBind`/`SliderBind { slot }` wire shape as a hand-authored bind.
- [ ] A `bind` carrying a `tween` authored via the factory deserializes to `TextTween`/`PanelTween` byte-identically to the `descriptor.rs` tween fixtures; a `bind` without a tween emits **no** `tween` key.
- [ ] Binding a `StateValue<boolean>` handle to a numeric-only widget prop is a TypeScript compile error (brand mismatch); binding a `StateValue<number>` to the same prop compiles.
- [ ] A malformed VM-returned tree (unknown `kind`, misspelled field) surfaces a named load-time error through the bridge and does not panic; a well-formed tree converts cleanly.
- [ ] Every user-facing text prop across all factories is typed `LocalizedText`; grep confirms no factory takes a bare `string` for displayed text. Changing the `LocalizedText` alias to a distinct branded type produces compile errors only at unmigrated call sites (verified by a temporary local edit, reverted).
- [ ] `sdk/lib/index.ts` re-exports every new factory and type; the generated typedefs regenerate clean and `gen-script-types` (or `scripts-build --prelude`) reports no drift between `index.ts` and `TS_SDK_LIB_BLOCK`/`LUAU_SDK_LIB_BLOCK`.

## Tasks

### Task 1: Text-alias chokepoint
Add the `LocalizedText` type alias (= `string`) to a single SDK location (e.g. `sdk/lib/ui/text.ts` + Luau twin), exported through the barrel and emitted into the typedef blocks. Document it as the mandated chokepoint for all displayed text. No runtime value — type-only. Lands first so every factory in Tasks 2–4 consumes it. `Depends on` nothing.

### Task 2: Leaf + container widget factories
Implement `widgets.{ts,luau}` (Text, Panel, Image, Button, Slider, Bar, Spacer) and `layout.{ts,luau}` (VStack, HStack, Grid). Props-first, positional children for containers, capitalized names. Each mirrors the `emitter()` shape: a `Props` type, synchronous validation throwing a field-named `Error`, documented defaults, camelCase output keys matching the `Widget` wire form. Text-bearing props use `LocalizedText`. TS and Luau mirror exactly (Luau as a returned module table). `Depends on` Task 1.

### Task 3: Placement envelope + handle ergonomics
Implement the `Tree(...)` envelope factory (`anchor`/`offset`/`root` + optional `captureMode`/`initialFocus`/`textEntryTarget`) and the `.get()`/`.set()` accessor wrappers over `defineStore` handles. `.set()` delegates to the shipped `setState` builder; `.get()` returns the bind reference Task 2's bound widgets accept. Author tween/styleRanges helper shapes consumed by the bound-widget props. `Depends on` Task 2.

### Task 4: Deserialization bridge
Extend the manifest conversion (`scripting/staged_manifest.rs` `manifest_from_js_value` + Luau twin) with a path that turns a VM-returned UI-tree descriptor value into a typed `AnchoredTree` via the existing serde `Deserialize`, surfacing a named load-time error on malformed input. No registration/routing here — the bridge only proves a factory-produced value crosses to a typed tree. `Depends on` Task 3 (so tests feed real factory output through it).

### Task 5: Barrel, typedef emission, parity + round-trip tests
Wire all new exports into `sdk/lib/index.ts`; update `TS_SDK_LIB_BLOCK`/`LUAU_SDK_LIB_BLOCK` in `typedef.rs`; regenerate typedefs. Add the cross-runtime parity tests and the factory→bridge→round-trip tests covering every kind and the envelope. `Depends on` Tasks 2–4.

## Sequencing

**Prereq:** B/C/D/E/F/TW shipped.
**Phase 1 (sequential):** Task 1 — the alias every factory consumes.
**Phase 2 (sequential):** Task 2 — factories; then Task 3 — envelope + handle ergonomics consume the leaf/container factories.
**Phase 3 (sequential):** Task 4 — bridge consumes factory output for its tests.
**Phase 4 (sequential):** Task 5 — barrel/typedef/tests consume all of the above.

## Rough sketch

**Factory shape.** Each factory follows `sdk/lib/entities/emitters.ts`: a `Props` type (required bare, optional `?`), a synchronous validator throwing `Error` that names the factory and field, documented defaults, and a flat returned object tagged on `kind`. The one casing difference from entity components: **UI widgets emit camelCase keys** (`fontSize`, `flexGrow`, `onPress`, `captureMode`) because the `Widget` wire form is camelCase, whereas `emitter()` emits snake_case for the component wire form. Capture this in the boundary inventory so it is decided once.

**Export surface.** Component factories are bare capitalized exports (`VStack({ gap: "m" }, Text({ content: "HP" }))`, research §15). State/lifecycle helpers live under a `ui` namespace (`ui.localState()` lands in G1b; G1a may seat the namespace object). Reactions stay lowercase bare exports (`playSound`, unchanged).

**Bridge.** The factories produce plain objects; `setupMod`/`setupLevel` return them; `manifest_from_js_value` already lowers a VM value to `serde_json::Value`. The bridge adds the UI-tree arm: `serde_json::from_value::<AnchoredTree>` with the error mapped to the manifest's named-diagnostic form. Registration/scoping is G1b's job; here the bridge is exercised directly in tests.

**Handle ergonomics.** `defineStore` returns branded dotted-name string handles (`scripting.md` §3 — "namespaced `.get()`/`.set()` wrappers remain SDK work"). G1a adds the methods without changing the brand: `.set()` is sugar over `setState`, `.get()` is a bind-reference producer.

**Key files:** `sdk/lib/ui/{widgets,layout,text}.{ts,luau}`, `sdk/lib/ui/reactions.{ts,luau}` (existing), `sdk/lib/index.ts`, `sdk/types/postretro.d.ts` / `.d.luau`, `crates/postretro/src/scripting/typedef.rs`, `crates/postretro/src/scripting/staged_manifest.rs`, `crates/postretro/src/render/ui/descriptor.rs` (wire target, unchanged).

## Boundary inventory

Casing rule (uniform across M13): Rust snake_case ↔ wire/JS/TS/Luau camelCase. Widget `kind` tags are lowercase/camel (`"vstack"`, `"easeInOut"`). UI-widget factory output keys are camelCase — distinct from entity-component factories (snake_case).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Text widget | `Widget::Text(TextWidget)` | `{"kind":"text",...}` | `Text(props)` | `Ui.Text(props)` |
| Panel | `Widget::Panel` | `"panel"` | `Panel(props)` | `Ui.Panel` |
| Image | `Widget::Image` | `"image"` | `Image(props)` | `Ui.Image` |
| Button | `Widget::Button(ButtonWidget)` | `"button"` | `Button(props)` | `Ui.Button` |
| Slider | `Widget::Slider` | `"slider"` | `Slider(props)` | `Ui.Slider` |
| Bar | `Widget::Bar` | `"bar"` | `Bar(props)` | `Ui.Bar` |
| Spacer | `Widget::Spacer` | `"spacer"` | `Spacer(props)` | `Ui.Spacer` |
| VStack | `Widget::VStack` | `"vstack"` | `VStack(props, ...kids)` | `Ui.VStack` |
| HStack | `Widget::HStack` | `"hstack"` | `HStack(props, ...kids)` | `Ui.HStack` |
| Grid | `Widget::Grid(GridWidget)` | `"grid"` | `Grid(props, ...kids)` | `Ui.Grid` |
| Envelope | `AnchoredTree` | `{anchor,offset,root,...}` | `Tree(props, root)` | `Ui.Tree` |
| font size | `TextWidget::font_size` | `"fontSize"` | `fontSize` | `fontSize` |
| flex grow | `SpacerWidget::flex_grow` | `"flexGrow"` | `flexGrow` | `flexGrow` |
| on-press | `ButtonWidget::on_press` | `"onPress"` | `onPress` | `onPress` |
| capture mode | `AnchoredTree::capture_mode` | `"captureMode"` | `captureMode` | `captureMode` |
| tween dur | `TextTween::duration_ms` | `"durationMs"` | `durationMs` | `durationMs` |
| displayed text | n/a (`String`) | JSON string | `LocalizedText` (= `string`) | `LocalizedText` |
| slot set | `setState` primitive | `{"primitive":"setState",...}` | `handle.set(v)` | `handle:set(v)` |

(Exact `ui` namespace name for Luau — `Ui` table vs. flat exports — pinned in Decisions.)

## Decisions

- **UI-widget factories emit camelCase; entity-component factories emit snake_case.** The two wire forms already differ in `descriptor.rs` vs. `data_descriptors`. Forcing one casing would break round-trip identity for one side. Rejected: a single casing convention across all factories.
- **Text-alias chokepoint lands in G1a, not G1b.** G1a owns the factories that consume it; the alias must exist before the first factory types a text prop. G1b's lifecycle/localState work touches no displayed text. Rejected: deferring the alias to G1b (would force a retrofit of every factory). Resolves the owner's flagged coupling question.
- **The bridge proves trees by deserialization, not mounting.** Registration and scoping belong to G1b's lifecycle; G1a keeps a clean dependency edge by testing the bridge in isolation. Rejected: building the registry here (pulls G1b's lifecycle forward into the wrong plan).
- **Component factories are bare capitalized exports; state helpers are namespaced.** Matches research §15's authored examples (`VStack(...)`) while giving `ui.localState()` (G1b) a home. Rejected: namespacing components (`ui.VStack`) — diverges from the research authoring form.

## Open questions

- Luau export shape for components: a single `Ui` module table (`Ui.VStack`) vs. flat returned functions matching the TS bare-export feel. Parity test must pin whichever lands.
- `.get()` semantics in a declare-then-drop world: it can only yield a *binding reference* for the renderer to resolve (there is no live VM read during gameplay). Confirm no author expects a synchronous value read; document `.get()` as bind-only.
- Whether the `Tree(...)` envelope validates `textEntryTarget`/`initialFocus` referential integrity (node ids exist) at factory time or defers to load-time. Lean: defer to the bridge/load, factory does shape-only validation.
