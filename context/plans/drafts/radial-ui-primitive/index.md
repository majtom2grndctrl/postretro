# Radial UI Primitive (Ring / Arc)

## Goal

Add a `Ring` widget to the UI vocabulary: an anti-aliased annulus/arc **shape** whose geometric properties — radius, thickness, start angle, sweep — are each independently a literal or bound 1:1 to a state slot. It gives modders the first radial HUD primitive (reticle rings, arcs, expanding crosshairs) that the axis-aligned quad + text vocabulary cannot express. This is the deferred "Radial / ring / arc UI primitive" the roadmap names (Epic 13 deferred set). It is deliberately a *dumb shape*, not a value gauge: mapping a game-state value into a property's units (spread → px, cooldown → degrees) is a computed binding that belongs on the Behavior IR (Epic 14), spec'd separately — this primitive is that spec's first consumer.

## Scope

### In scope

- A `Ring` `Widget` variant + wire format: a fixed-size annulus-arc shape.
- Geometry authored per property, each `literal | bound`: `radius`, `thickness`, `startAngle`, `sweep`. A bound property reads its slot **1:1** in the property's own units (px / degrees), with an optional presentation tween on the bound ref.
- A static layout box (`diameter`) so bound draw values are appearance-only and never relayout.
- `fill` color and an optional full-circle `track` color.
- A CPU radial draw instance (`UiRingInstance`), a `UiDrawData` ring stream, a `UiPaintOp::Ring`, and an SDF renderer pipeline + shader (`ui_ring.wgsl`) slotted into the existing single-encode composition.
- SDK factories (TypeScript + Luau) `Ring(...)`, typedefs, the JS + Luau bridge arms, shared validation.
- A static reticle-ring demo in the dev mod.

### Out of scope — deferred to a successor spec

- **UI computed bindings via the Behavior IR** — resolving a bound value the IR derives from game state (`radius = map(player.spread, [0,1]→[12,60])`). This is the general home for value transforms (and likely subsumes `styleRanges` and `Bar::max`); an Epic-14 primitive-consolidation item, its own `/draft-plan`. Without it, no live slot is in UI-property units, so the *dynamic* consumers below are unblocked by that spec, not this one.
- **Bullet-spread / cooldown / charge gauges** — they need the IR mapping above (and, for spread, a gameplay producer that does not exist: no `player.spread` in `BUILTIN_ENGINE_STATE`). This spec ships the shape + binding hooks they will consume.
- Bindable `fill`/`track` colors, a value→color `styleRanges` on the ring, counter-clockwise winding, an in-ring text label, an `exitFade`. All additive follow-ups.
- Focusability/interaction (a ring is passive), and any change to `UiInstance`'s locked ABI or the `ui_quad.wgsl` pipeline.

## Direction

**Problem.** The UI widget vocabulary emits only axis-aligned quads (`UiInstance`: rect/uv/color/margin) and glyphon text through one shader (`ui_quad.wgsl`); there is no way to draw a ring or an arc. Radial HUD indicators — the observation that produced this — are unbuildable.

**Model — a shape, not a value visualizer.** A `Bar`'s `bind` prop encodes an assumption: the widget exists to visualize one number. That is wrong for a ring, which is a shape with several properties that vary independently and are often all static (a plain reticle ring binds nothing). So the ring has no privileged bound value: every geometric property is its own named prop, each `literal | bound`, generalizing the existing `max: number | ComputedRef<number>` precedent to the whole shape. A bound property reads its slot **1:1** — the widget performs no value transform.

**Prior commitments.** Honors "primitive surface is a contract" (`index.md` §2): a new kind updates SDK types, validation, and factories in one pass. Preserves the Goal-B locked wire format (byte-identical round-trip; additive fields skip-serialize). Keeps "renderer owns GPU" (shader/pipeline in `crates/renderer`). The one deliberate divergence from the `Bar` precedent — a second UI pipeline (SDF fragment fill) instead of the shared quad pipeline — is what the roadmap means by "a shader path beyond `ui_quad.wgsl`"; it slots into the `UiComposition`/`encode` single-encode contract as a new batch stream (diagrammed in `research.md`).

**Why 1:1 binding, and the IR seam.** The UI layer cannot compute per-frame (the VM drops; bindings are name→value). A bound geometric prop therefore needs its slot already in the property's units *or* a widget-side mapping. Baking a per-prop `from→to` range would work but would multiply exactly the pre-IR special case Epic 14's "primitive consolidation" exists to retire. So value mapping is placed on the Behavior IR as a **successor spec**, and this primitive stays a dumb 1:1 shape. The binding hooks are the seam that spec consumes.

**Alternatives rejected.** (1) The Bar-style privileged `bind`+`max`+`fillMode` value-visualizer — wrong frame for a shape with independently-varying, often-static properties (an earlier draft; see `research.md`). (2) A baked per-prop range — defers to IR (above). (3) Compose rings from many quads in mod code — no AA, no smooth arcs, dozens of quads; the roadmap calls for an engine primitive. (4) A pre-baked ring texture + `image` widget — fixed-resolution (against the AA-crisp-at-any-resolution model), no continuous arc/radius. (5) Extend the locked 64-byte `UiInstance` + `ui_quad.wgsl` — couples two primitives on one test-asserted ABI and the 9-slice path. (6) Ship a static-only shape with no binding — rejected: the hooks are cheap (reuse `drive_tween_f32`) and are the seam the IR spec lands on; omitting them forces a second breaking pass. (7) Defer the whole primitive and co-locate with the Weapon-Feel producer — foreclosed by the roadmap's "cross-cutting UI-layer work, reusable across features" framing.

**Foreclosures.** The durable commitments are the wire shape (`kind:"ring"`, its field names, the `literal|bound` scalar form) and the `UiRingInstance` ABI; removal later is content-breaking (pre-stable, acceptable). The first-in-codebase angle convention (0° = up, clockwise) is inherited by any future radial widget — named in Open Questions. The shader/pipeline are internal and cheap to change.

## Acceptance criteria

- [ ] A `ring` descriptor round-trips byte-identically through serde; every optional field (`radius`, `thickness`, `startAngle`, `sweep`, `track`, `id`, `visibleWhen`, `role`) omits its key when absent/default, and each scalar property round-trips in both its literal (bare number) and bound (`{slot}`/`{local}`, optional `tween`) form.
- [ ] Raw JSON asset loads and both script bridges reject a literal that is out of range (`diameter`/`thickness`/`radius` non-finite or ≤ 0, `radius > diameter/2`, `thickness > radius`, `sweep` outside `(0, 360]`, non-finite `startAngle`) and accept the same fields in bound form (runtime-clamped, not load-validated). A valid ring from each frontend deserializes to the same `Widget::Ring`.
- [ ] With all-literal geometry, the collected draw data emits one ring instance for the shape (plus a full-360 track instance when `track` is set), with device-pixel radius/thickness and radians angles projected from the logical-reference values; a `sweep` of 360° renders a seamless ring (no wrap gap), a `sweep` < 360° an open arc.
- [ ] A geometric property bound to a numeric slot resolves **1:1** (the slot value is used directly in the property's units, no widget-side transform); changing the slot changes the drawn shape; an out-of-range resolved value is clamped at draw, not rejected.
- [ ] A bound geometric property with a `tween` eases its drawn value toward each new slot target (reusing the retained tween runtime); a bound-value or `visibleWhen` change rebuilds the ring's draw list **without a taffy relayout** (`recompute_count` stays flat), and a settled frame rebuilds nothing.
- [ ] `ui_ring.wgsl` parses and passes naga validation; the ring pipeline is created and a headless composition draws a ring batch at its painter depth (self-skipping when no GPU adapter is present); rings compose in painter order relative to quads and text through the single `UiComposition`/`encode` path (the per-layer encode loop stays unrepresentable, an opaque upper-layer quad occludes a lower-layer ring).
- [ ] The workspace builds: every exhaustive `Widget` match (`build_node`, `focus_meta`, `widget_visible_when`, `implicit_role`) and the exhaustive `NodeContext` match (`collect_node`) has a `Ring` arm; a ring is passive (id-only in `focus_meta`, `None` interaction).
- [ ] TypeScript and Luau `Ring(...)` factories accept each geometric prop as either a literal number or a bound ref (the `max: number | ComputedRef<number>` shape), emit a valid `{kind:"ring", …}` descriptor that round-trips to the canonical wire form, and the SDK typedef declares `RingProps` and `Ring`.
- [ ] The dev mod renders a static reticle ring (replacing or beside the current `"+"`), and the repo documents that value-mapped dynamic consumers (cooldown arc, spread crosshair) await the successor IR-bindings spec.

## Tasks

### Task 1: Vertical slice — a static ring shape end to end

Establish the whole render seam with an all-literal ring so the second-pipeline / painter-order / instance-ABI boundary is falsified before any binding work. Add the `Widget::Ring(RingWidget)` variant and `RingWidget` struct to `crates/scripting-core/src/ui/descriptor/widgets.rs`: `diameter: f32` (required, logical-ref px — the fixed layout box), `radius`/`thickness`/`startAngle`/`sweep` as the new `ScalarValue` type (this task's literal path only), `fill: ColorValue`, `track: Option<ColorValue>`, `id`/`visible_when`/`role` — optionals `skip_serializing_if`, mirroring `BarWidget`'s idiom. Add `ScalarValue` to `crates/scripting-core/src/ui/descriptor/values.rs` as an untagged `Literal(f32) | Bound(BoundScalar)` where `BoundScalar` flattens a `BindSource` (`{slot}`/`{local}`) beside an optional `tween` (the `SliderBind` shape) — declaration order `Literal` first so a bare JSON number lands on it (the `BarMax`/`ColorValue` precedent). Add `RingWidget::validate` enforcing the AC-2 literal-range rules, wired through `#[serde(try_from = "RingWidgetWire")]` like `BarWidget`. Add `NodeContext::Ring` to `crates/ui/src/tree/node_context.rs` carrying the resolved `fill`/`track` colors and, per geometric property, a resolved scalar (this task: the literal value; Task 2 adds the bound driver state). Add `build_ring` to `crates/ui/src/tree/build.rs` (a taffy leaf whose explicit style size is `[diameter, diameter]`, resolving `fill`/`track` tokens against the theme like `build_bar`) and its `Widget::Ring` arm in `build_node`. Add a byte-stable `#[repr(C)] Pod` `UiRingInstance` to `crates/ui/src/output.rs` (device-pixel bounding rect, linear-RGBA color, resolved radial geometry in device px + radians — state size as a constraint with a `size_of` assertion, do not pin field offsets), a `rings` stream + `UiPaintOp::Ring { index }` + `push_ring` to `crates/ui/src/tree/draw.rs`, and the `NodeContext::Ring` arm to `collect_node` in `crates/ui/src/tree/ui_tree_collect.rs`: project the device rect, convert angles with `.to_radians()`, push a full-360 track instance (when `track` is `Some`) then the shape instance at its resolved `sweep`; skip degenerate output (radius or thickness ≤ 0). In `crates/renderer/src/render/ui/mod.rs` add `GpuUiRingInstance` (+ painter depth), a translucent depth-test/no-write `ring_pipeline` and its shader `crates/renderer/src/shaders/ui_ring.wgsl` (bounding-quad vertex expansion → SDF annulus + angular-wedge fragment, anti-aliased via `fwidth`/`smoothstep`, seamless at 360°; needs only the viewport uniform); extend `UiComposition::from_layer_draws` to fold `UiPaintOp::Ring` into an ordered ring-batch stream and `UiPass::encode` to draw ring batches after quads and before text at their `painter_depth` into the shared UI depth target (per the `research.md` encode diagram). Add the compile-forced `Widget::Ring` arms: `focus_meta` (id-only, `FocusNeighbors::default()`) and `widget_visible_when` in `crates/ui/src/tree/widget_meta.rs`, and `implicit_role` → `Role::None` (a shape is decorative by default; a gauge author sets `role`) in `crates/scripting-core/src/ui/descriptor/accessibility.rs`. Ship the wire round-trip + literal-validation tests, a `ui_ring.wgsl` naga parse/validate test (mirroring `ui_quad_wgsl_parses_and_validates`), and a CPU draw-data test proving an all-literal ring emits the expected track+shape instances with correct device geometry and a seamless 360° vs. open-arc case.

### Task 2: Per-property 1:1 binding + tween (CPU)

Make each geometric property bindable, entirely at the CPU tree level; no renderer change. Extend `NodeContext::Ring` so each geometric property is either a resolved literal or a bound driver (the `BoundScalar` source + its resolved `bind_scope`, a `last_resolved: Option<f32>`, and a `tween: Option<TweenState<f32>>`) — the per-property analog of the Bar's single-value driver. In `build_ring` (extending Task 1's file), resolve each `ScalarValue`: a literal is stored directly; a bound scalar seeds a driver (scope resolved like `build_bar`'s `bind_scope`, tween born on first resolution). Add the `NodeContext::Ring` arm to `resolve_bindings` in `crates/ui/src/tree/ui_tree.rs`: for each bound property, resolve its slot to a `Number` and drive it 1:1 through `drive_tween_f32` (the existing f32 driver — target is the raw slot value, no transform), flagging `appearance_changed` only (a ring is a fixed-`diameter` leaf and must never mark itself dirty for relayout — Invariant "appearance-only"). In `collect_node`, read each geometric property's resolved value (literal, or the eased `last_resolved` when a bound driver is active), clamp to the property's valid range at draw (`radius` to `[0, diameter/2]`, `sweep` to `(0, 360]`, `thickness` to `[0, radius]`), and emit as in Task 1. Add tree tests mirroring `crates/ui/src/tree/tests/bar.rs`: a bound `sweep` (or `radius`) tracks its slot 1:1, a `tween` eases the drawn value across frames, an out-of-range resolved value clamps at draw, and a bound-value change is appearance-only (`recompute_count` flat, `draw_rebuild_count` increments).

### Task 3: SDK factories, typedefs, and script bridge

Give scripts a `Ring(...)` authoring surface where every geometric prop is passed directly (literal or bound ref), in lockstep TypeScript and Luau. Add the `Ring` factory to `sdk/lib/ui/widgets.ts` and `sdk/lib/ui/widgets.luau` with a shared `buildScalar(value, name, factory)` helper generalizing `buildBarMax`/`stateSlot`: a `number` becomes a literal, a state ref becomes `{ slot }` (reading the ref's slot and any tween the way `bindState` attaches it), a `LocalBindRef` becomes `{ local }`. `diameter` is a required number; `fill`/`track` reuse `requireColor`; `visibleWhen`/`role` reuse `applyA11yFields`. Emit `{ kind: "ring", diameter, radius, thickness, startAngle, sweep, fill, … }` with optional keys appended only when supplied (so factory output matches the locked wire form). Declare `RingProps` (each geometric prop typed `number | ComputedRef<number>`) and the `Ring` function alongside `BarProps` in `widgets.ts`/`widgets.luau` and in the generated typedef template `crates/scripting-core/src/typedef/templates/ui_sdk_module.d.ts`. Add the Rust bridge arms: a `"ring"` case dispatching to `ring_widget_from_js` in `crates/scripting-core/src/data_descriptors/js/ui_widgets.rs` and `ring_widget_from_lua` in `.../lua/ui_widgets.rs`, each reading the fields (a scalar prop parsed as a bare number → `ScalarValue::Literal`, else the bound object) into `RingWidget` and calling `ring.validate()` → `DescriptorError::InvalidShape` on failure (the `bar_widget_from_*` precedent). Add bridge tests to `crates/scripting-core/src/data_descriptors/tests/ui_bridge.rs` asserting a valid ring (mixing literal and bound props) parses from both JS and Luau and that each invalid-literal case (AC 2) is rejected on both paths, plus a factory-output round-trip case in the descriptor round-trip suites (`crates/scripting-core/src/ui/descriptor/mod.rs` and the `crates/ui` mirror).

### Task 4: Static reticle-ring demo + successor-spec note

Prove the shape in a running engine and record the deferred work. In the dev mod (`content/dev/scripts/hud.ts`), author a static reticle ring (all-literal geometry) as the `hud.reticle` tree — replacing or framing the current `Text({ content: "+" })` — so a running engine shows the primitive. Add a short note to the scripting reference docs (`docs/scripting-reference.md` and any SDK usage doc) that `Ring` geometry props are `literal | bound (1:1)`, and that value-mapped dynamic consumers (a cooldown arc, a bullet-spread crosshair) require the successor **UI-computed-bindings (Behavior IR)** spec — and, for spread, a gameplay producer that does not exist yet — so authoring those against a raw slot today would need the value already in px/degrees. Do not add a gameplay producer, a new engine slot, or the IR mapping. Update the `Widget`-kind lists in the descriptor doc comments and `docs/scripting-reference.md` to include `ring`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; a static ring through the renderer falsifies the second-pipeline / painter-order / instance-ABI boundary and lands the shared descriptor + `ScalarValue` + collector + renderer shape every later task extends.
**Phase 2 (concurrent):** Task 2, Task 3 — independent. Task 2 extends the CPU tree (`node_context.rs`, `build.rs`, `ui_tree.rs`, `collect_node`, tree tests); Task 3 extends the SDK + scripting-core bridge. Both consume Task 1's `RingWidget`/`ScalarValue`/`NodeContext::Ring`; neither shares a source file with the other.
**Phase 3 (sequential):** Task 4 — consumes the `Ring` factory (Task 3) to author the demo and record the successor note.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Ring kind | `Widget::Ring(RingWidget)` | `"ring"` | `Ring(props)` → `kind:"ring"` | `Ring(props)` → `kind:"ring"` |
| Layout box | `diameter: f32` (logical px) | `"diameter"` (number) | `diameter: number` | `diameter: number` |
| Scalar geometry | `radius`/`thickness`/`start_angle`/`sweep`: `ScalarValue` | bare number, or `{slot}`/`{local}` + optional `tween` | `number \| ComputedRef<number>` | same |
| Scalar value type | `ScalarValue::{Literal(f32), Bound(BoundScalar)}` | untagged: number \| bind object | literal or `bindState(ref)` | literal or `bindState(ref)` |
| Fill / track | `fill: ColorValue`, `track: Option<ColorValue>` | `"fill"` / `"track"` | `WidgetColor` / `WidgetColor?` | same |
| id / visibleWhen / role | mirror `BarWidget` | `id` / `visibleWhen` / `role` | same | same |

Angles authored in **degrees** (`0° = 12 o'clock, positive = clockwise`), converted with `.to_radians()` in the collector (house style — `research.md`). `diameter`/`radius`/`thickness` are logical-reference px, projected to device px like every other rect. The `UiRingInstance` carries device-px geometry and radians.

## Wire format

`ScalarValue` (new, `crates/scripting-core/src/ui/descriptor/values.rs`) is the reusable "literal-or-bound scalar" the ring's geometric props use; its untagged wire form is a bare JSON number (`Literal`) or a bind object carrying `{slot}`/`{local}` and an optional `tween` (`Bound`). It generalizes `BarMax` (which is literal-or-`{slot}`) with a `local` source and a tween, and is available to future widgets wanting per-property binding.

`UiRingInstance` (CPU, `crates/ui/src/output.rs`) is a new `#[repr(C)] Pod` draw record byte-stable for app→renderer handoff, mirroring `UiInstance`'s discipline (a `size_of`/align assertion guards the ABI). It carries a device-pixel bounding rect, a linear-RGBA color, and the resolved radial geometry (center-relative outer radius, thickness, start angle, sweep — device px and radians). State the stride/size as the constraint; the implementer places fields. `GpuUiRingInstance` appends the painter depth, as `GpuUiInstance` does for quads.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Wire round-trip byte-identity; each scalar prop round-trips in both literal and bound form; optionals skip-serialize | Task 1 (`RingWidget`/`ScalarValue` serde) | Every field added or reordered; the SDK factory must emit the canonical form | AC 1, AC 8 |
| **1:1 binding — a bound geometric prop resolves its slot value directly in the property's units, with no widget-side transform** (out-of-range clamps at draw). This is the seam the successor IR spec consumes; a baked `from→to` mapping must NOT creep in | Task 2 (`resolve_bindings` drives `drive_tween_f32` on the raw slot value) | `resolve_bindings` + `collect_node` — no scaling/offset between slot and drawn property | AC 4 |
| Ring is appearance-only — a bound value/visibility change never triggers a taffy relayout (fixed-`diameter` leaf) | Task 1 (`build_ring` explicit `[diameter,diameter]` size), Task 2 (`resolve_bindings` flags `appearance_changed` only) | `resolve_bindings` must not `mark_dirty` a ring; `collect_node` must not resize the leaf from a bound value | AC 5 |
| Single encode per composition + painter order across quad/ring/text; per-layer encode loop unrepresentable | Task 1 (`UiComposition`/`encode` ring stream) | Ring batches ride the whole-composition encode at their `painter_depth` into the shared depth target | AC 6 |
| Theme-free per-frame draw walk — `fill`/`track` tokens pre-resolved to literals at build | Task 1 (`build_ring` token resolution) | `build_ring` resolves every token; `collect_node` never looks one up | AC 3 |

## Script syntax examples

```ts
// Static reticle ring — no binding at all (the case a Bar-style model cannot
// express). A thin full-circle outline as a crosshair.
Ring({ diameter: 24, radius: 10, thickness: 2, fill: [0.8, 0.95, 0.98, 0.9] })

// A partial arc, all literal — e.g. a decorative 270° gauge frame with a dim
// full-circle track behind it. `sweep` < 360 leaves the arc open.
Ring({
  diameter: 96, radius: 44, thickness: 6,
  startAngle: 135, sweep: 270,
  fill: [0.1, 0.8, 1.0, 1.0],
  track: [0.1, 0.1, 0.1, 0.5],
})

// A bound geometric property — 1:1, no transform. `player.spreadRadiusPx` is a
// hypothetical slot ALREADY IN PIXELS; mapping a raw gameplay spread value into
// pixels is the successor IR-bindings spec's job, not this widget's. The ring's
// drawn radius follows the slot, eased.
Ring({
  diameter: 120, thickness: 2, sweep: 360,
  radius: bindState(player.spreadRadiusPx, { tween: { durationMs: 90, easing: "easeOut" } }),
  fill: [1, 0.2, 0.2, 1],
})
```

## Open questions

- **Angle zero/winding convention** is decided (`0° = up, clockwise`) but is the first radial convention in the UI layer; a future in-world or minimap radial widget would inherit or re-litigate it. Recorded, not blocking.
- **Successor spec ownership** — "UI computed bindings via Behavior IR" is named here (Out of scope) but unwritten. It is the thing that makes the dynamic consumers real; whoever picks up bullet spread will need it first. Flag for roadmap sequencing, not this spec.
