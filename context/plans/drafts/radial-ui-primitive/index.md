# Radial UI Primitive (Ring / Arc)

## Goal

Add a `Ring` widget to the UI vocabulary: an anti-aliased annulus/arc whose bound numeric value drives either its angular fill (a progress arc) or its radius (an expanding ring). It gives modders the first radial HUD primitive — cooldown arcs, charge/reload rings, and the crosshair-anchored bullet-spread ring — none of which the axis-aligned quad + text vocabulary can express. This is the deferred "Radial / ring / arc UI primitive" the roadmap names (Epic 13 deferred set); the Weapon-Feel spread ring is its motivating consumer.

## Scope

### In scope

- A `Ring` `Widget` variant + wire format, following the `Bar` widget's shape (passive, bound, numeric, fixed-size).
- Two value→geometry modes on one widget: `sweep` (fraction → angular arc extent) and `radius` (fraction → ring radius).
- A CPU radial draw instance (`UiRingInstance`), a `UiDrawData` ring stream, and a `UiPaintOp::Ring`.
- A renderer SDF ring pipeline + shader (`ui_ring.wgsl`) integrated into the existing single-encode UI composition with correct painter ordering.
- Retained-tree reactivity: bound value/max/tween/styleRanges/visibleWhen, reusing the Bar drivers; appearance-only (never relayout).
- SDK factories (TypeScript + Luau) `Ring(...)`, typedefs, the JS + Luau bridge arms, and shared validation.
- A demoable consumer bound to a live slot (a cooldown or reload arc) in the dev mod, plus a documented spread-ring example.

### Out of scope

- Any gameplay producer of a dynamic spread/bloom value (a future Weapon-Feel spec owns `player.spread`; no such slot exists today — `BUILTIN_ENGINE_STATE` in `crates/entities/src/engine_state_catalog.rs`).
- Counter-clockwise winding (`clockwise: false`), per-instance label/`fontSize` text inside the ring, and an `exitFade` (documented Bar-only). All are additive follow-ups.
- Focusability/interaction — a ring is passive like a bar.
- Changing `UiInstance`'s locked 64-byte ABI or the `ui_quad.wgsl` pipeline.

## Direction

**Problem.** The UI widget vocabulary emits only axis-aligned quads (`UiInstance`: rect/uv/color/margin) and glyphon text through one shader (`ui_quad.wgsl`); there is no way to draw a ring, an arc, or a value-driven radius. Radial HUD indicators — the observation that produced this — are therefore unbuildable, and the Weapon-Feel crosshair spread ring has no widget to consume.

**Prior commitments.** This honors the "primitive surface is a contract" invariant (`context/lib/index.md` §2): a new widget kind updates the SDK types, validation, and factories in the same pass. It preserves the Goal-B locked wire format — the `Ring` descriptor round-trips byte-identically and every additive field skip-serializes when default (`crates/scripting-core/src/ui/descriptor/widgets.rs` idiom). It keeps "renderer owns GPU": the shader/pipeline live in `crates/renderer`. It mirrors the `Bar` widget precedent (passive, numeric, fixed-size, `styleRanges` recolor, tween easing) and reuses `drive_bar_binding`/`drive_bar_max` verbatim. **Divergence:** the Bar draws through the shared quad pipeline; the Ring needs a *second* UI pipeline (SDF fragment fill), which slots into the `UiComposition`/`encode` single-encode contract as a new batch stream — argued in "Alternatives rejected" and diagrammed in `research.md`.

**Alternatives rejected.** (1) Compose rings from many small quads in mod code — no anti-aliasing, dozens of quads per ring, cannot express smooth arcs or a value-driven radius; the roadmap explicitly calls for an engine primitive. (2) A pre-baked ring texture + the `image` widget with UV animation — fixed resolution (blurry when scaled, against the AA-crisp-at-any-resolution rendering model) and cannot do continuous sweep or arbitrary radius. (3) Extend the locked 64-byte `UiInstance` + `ui_quad.wgsl` with radial branches — couples two unrelated primitives on one test-asserted ABI and the 9-slice vertex path; a separate SDF pipeline is what the roadmap means by "a shader path beyond `ui_quad.wgsl`." (4) Ship only `sweep` mode and defer `radius`/spread — the closest rival on scope; rejected because bullet spread is the stated motivation and radius mode rides nearly all the shared machinery (see `research.md` "Scope alternative").

**Foreclosures / one-way doors.** The wire shape (`kind:"ring"`, its field names, the `fillMode` values) and the `UiRingInstance` ABI are the durable commitments; removing the widget later is a content-breaking change (pre-stable, acceptable, but real). The shader/pipeline are internal and cheap to change. The parameterization is the reversibility risk this spec resolves up front. Nothing else material.

## Acceptance criteria

- [ ] A `ring` descriptor with the full field set round-trips byte-identically through serde; every optional field (`track`, `startAngle`, `sweep`, `fillMode`, `minRadius`, `styleRanges`, `visibleWhen`, `id`, `role`) omits its key when absent/default, so a minimal ring is byte-stable.
- [ ] Raw JSON asset loads and both script bridges reject: non-finite or ≤0 `radius`/`thickness`, `thickness > radius`, `sweep` outside `(0, 360]`, `minRadius` that is non-finite/negative or ≥ `radius`, and an unknown `fillMode`. A valid ring from each frontend deserializes to the same `Widget::Ring`.
- [ ] In `sweep` mode, the collected draw data emits a track arc over the full `sweep` (when `track` is set) and a fill arc spanning `clamp(value/max,0,1) * sweep` from `startAngle`; at fraction 0 no fill instance is emitted; at fraction ≥ 1 the fill spans the full `sweep`. A full `sweep` of 360° renders a seamless ring (no gap at the wrap).
- [ ] In `radius` mode, the fill ring spans the full `sweep` at radius `lerp(minRadius, radius, clamp(value/max,0,1))`; increasing the bound value increases the drawn radius; the track (when set) draws at `radius`.
- [ ] `styleRanges` recolors the fill from the same clamped fraction in both modes (a critical band at a low fraction turns the fill its band color); band-color tokens are pre-resolved at build so the per-frame draw walk stays theme-free.
- [ ] A bound value change (including a `bind.tween` easing and a `BarMax::State` denominator change) rebuilds the ring's draw list without a taffy relayout (`recompute_count` stays flat across value/max changes); a settled frame rebuilds nothing.
- [ ] `ui_ring.wgsl` parses and passes naga validation; the ring pipeline is created and a headless composition draws a ring batch at its painter depth (self-skipping when no GPU adapter is present).
- [ ] Rings compose in painter order relative to quads and text within a layer through the single `UiComposition`/`encode` path — the per-layer encode loop remains unrepresentable, and an opaque upper-layer quad occludes a lower-layer ring.
- [ ] The workspace builds: every exhaustive `Widget` match (`build_node`, `focus_meta`, `widget_visible_when`, `implicit_role`) and the exhaustive `NodeContext` match (`collect_node`) has a `Ring` arm; `implicit_role(Ring)` is `progressbar`.
- [ ] TypeScript and Luau `Ring(...)` factories emit a valid `{kind:"ring", …}` descriptor that round-trips to the canonical wire form; the SDK typedef declares `RingProps` and `Ring`.
- [ ] The dev mod renders a live radial indicator (a cooldown or reload arc bound to an existing slot) and the repo carries a documented spread-ring (`radius` mode) authoring example.

## Tasks

### Task 1: Vertical slice — a bound sweep-mode ring end to end

Establish the whole seam for `sweep` mode so the renderer/painter-order boundary is falsified before fan-out. Add the `Widget::Ring(RingWidget)` variant and `RingWidget` struct to `crates/scripting-core/src/ui/descriptor/widgets.rs` with the full field set: `bind: SliderBind`, `max: BarMax`, `fill: ColorValue`, `radius: f32`, `thickness: f32`, `track: Option<ColorValue>`, `start_angle: f32` (default 0.0), `sweep: f32` (default 360.0), `fill_mode: RingFillMode` (default `Sweep`), `min_radius: Option<f32>`, `id`, `style_ranges`, `visible_when`, `role` — all optionals `skip_serializing_if`, mirroring `BarWidget`. Add the `RingFillMode { Sweep, Radius }` enum (camelCase wire) and a `RingWidget::validate` enforcing the AC-2 rules, wired through a `#[serde(try_from = "RingWidgetWire")]` like `BarWidget`. Add `NodeContext::Ring` to `crates/ui/src/tree/node_context.rs` carrying the resolved colors (`fill`, `track: Option<[f32;4]>`), resolved geometry (`radius`, `thickness`, `start_angle`, `sweep` — degrees stored, converted at draw), `fill_mode`, `min_radius`, `bind`/`bind_scope`/`max`, and the Bar-shaped retained fields (`last_resolved`, `last_max_resolved`, `tween`, `style_ranges`, `style_state`). Add `build_ring` to `crates/ui/src/tree/build.rs` (a fixed-size taffy leaf whose style size is the ring's bounding box `= 2*radius` per axis, resolving `fill`/`track` and `style_ranges` tokens against the theme like `build_bar`), and its `Widget::Ring` arm in `build_node`. Add a CPU `UiRingInstance` to `crates/ui/src/output.rs` (`#[repr(C)] Pod`, byte-stable like `UiInstance`: device-pixel bounding rect, linear-RGBA color, and resolved radial geometry in device px + radians — state the size as a constraint with a `size_of` assertion test, do not pin field offsets). Add a `rings` stream to `UiDrawData` and a `UiPaintOp::Ring { index }` + `push_ring` in `crates/ui/src/tree/draw.rs`. Add the `NodeContext::Ring` arm to `collect_node` in `crates/ui/src/tree/ui_tree_collect.rs`: project the device rect (via `project_rect`), compute `fraction = clamp(value/max,0,1)` (bar_slot_value/bar_max_value), convert angles with `.to_radians()`, and push a track ring instance (full sweep, when `track` is `Some`) then a fill ring instance spanning `fraction*sweep` (emit no fill at fraction 0, per Invariant "fraction contract"). In `crates/renderer/src/render/ui/mod.rs` add a `GpuUiRingInstance` (+ painter depth), a `ring_pipeline` (translucent, depth-test/no-write) and its shader `crates/renderer/src/shaders/ui_ring.wgsl` (bounding-quad vertex expansion → SDF annulus + angular-wedge fragment, anti-aliased via `fwidth`/`smoothstep`, seamless at 360°); the ring pass needs only the viewport uniform. Extend `UiComposition::from_layer_draws` to fold `UiPaintOp::Ring` into an ordered ring-batch stream and `UiPass::encode` to draw ring batches after quads and before text at their `painter_depth` into the shared UI depth target (per the `research.md` encode diagram and Invariant "single-encode + painter order"). Add the compile-forced `Widget::Ring` arms so the workspace builds: `focus_meta` (id-only, `FocusNeighbors::default()`, like `Bar`) and `widget_visible_when` in `crates/ui/src/tree/widget_meta.rs`, and `implicit_role` → `Role::Progressbar` in `crates/scripting-core/src/ui/descriptor/accessibility.rs`. Ship the wire round-trip test, a `ui_ring.wgsl` naga parse/validate test (mirroring `ui_quad_wgsl_parses_and_validates`), and a CPU draw-data test proving a `sweep` ring emits track+fill ring instances whose fill arc extent tracks the bound value.

### Task 2: Reactive binding, radius mode, and styleRanges (CPU)

Make the ring reactive and complete both modes, entirely at the CPU tree level. Add the `NodeContext::Ring` arm to `resolve_bindings` in `crates/ui/src/tree/ui_tree.rs`, calling `drive_bar_binding` (value/tween) and `drive_bar_max` (denominator) exactly as the `Bar` arm does and flagging `appearance_changed` only — a ring is a fixed-size leaf and must never mark itself dirty for relayout (Invariant "appearance-only"). In `collect_node` (extending the file Task 1 created), add the `radius` `fill_mode` branch: the fill ring spans the full `sweep` at radius `lerp(min_radius.unwrap_or(0.0), radius, fraction)` in device px, with the track (when `Some`) at `radius`; emit no fill when the computed radius ≤ 0. Wire `style_ranges` into both modes: evaluate the widget-agnostic `style_ranges::evaluate` against the clamped `fraction` (the value the widget renders) to recolor the fill, matching the Bar collector, with band tokens already pre-resolved to literals in `build_ring` so the walk is theme-free. Read the eased display value from `last_resolved` when a tween is active (the Bar precedent), so a tweened fraction eases the arc extent / radius. Add tree tests mirroring `crates/ui/src/tree/tests/bar.rs`: sweep fraction → arc extent, radius mode → radius lerp, `styleRanges` recolor, `BarMax::State` denominator change is appearance-only (`recompute_count` flat, `draw_rebuild_count` increments), and a bind-tween eases the displayed fraction across frames.

### Task 3: SDK factories, typedefs, and script bridge

Give scripts a `Ring(...)` authoring surface and a validating bridge, in lockstep TypeScript and Luau. Add the `Ring` factory to `sdk/lib/ui/widgets.ts` and `sdk/lib/ui/widgets.luau`, reusing the shared helpers the `Bar` factory uses (`buildBind`/`buildBarMax`/`requireColor`/`buildStyleRanges`/`applyA11yFields`) plus new `requireFiniteNumber` reads for `radius`/`thickness`/`startAngle`/`sweep`/`minRadius` and a `fillMode` string check; emit `{ kind: "ring", bind, max, fill, radius, thickness, … }` with optional keys appended only when supplied (so factory output matches the locked wire form). Declare `RingProps` and the `Ring` function in the SDK types alongside `BarProps` in `widgets.ts`/`widgets.luau` and in the generated typedef template `crates/scripting-core/src/typedef/templates/ui_sdk_module.d.ts`. Add the Rust bridge arms: a `"ring"` case dispatching to `ring_widget_from_js` in `crates/scripting-core/src/data_descriptors/js/ui_widgets.rs` and `ring_widget_from_lua` in `.../lua/ui_widgets.rs`, each reading the fields into `RingWidget` and calling `ring.validate()` → `DescriptorError::InvalidShape` on failure (the `bar_widget_from_*` precedent). Add bridge tests to `crates/scripting-core/src/data_descriptors/tests/ui_bridge.rs` asserting a valid ring parses from both JS and Luau and that each invalid-geometry case (AC 2) is rejected on both paths, plus a factory-output round-trip case in the descriptor round-trip suites (`crates/scripting-core/src/ui/descriptor/mod.rs` and the `crates/ui` mirror).

### Task 4: Live consumer demo + documented spread-ring example

Prove the primitive against real data and document the motivating consumer. In the dev mod (`content/dev/scripts/hud.ts`), author a radial indicator bound to a slot that is live today — a cooldown arc bound to `player.weaponCooldownMs` or a reload ring bound to `player.reloadProgress` (both exist in `BUILTIN_ENGINE_STATE`) — as an always-on or HUD-composed tree, so a running engine shows the ring animating. Add a spread-ring authoring example (in `radius` mode, binding a `player.spread`-style numeric slot) to the scripting reference docs (`docs/scripting-reference.md` and any SDK usage doc), explicitly noting that its live data source is future Weapon-Feel work and the widget is authored against the bindable slot name now (the decoupling-seam pattern). Do not add a gameplay spread producer or a new engine slot. Update the `Widget`-kind lists in the descriptor doc comments and `docs/scripting-reference.md` to include `ring`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the second-pipeline / painter-order / instance-ABI boundary assumptions and lands the shared descriptor + collector + renderer shape every later task extends.
**Phase 2 (concurrent):** Task 2, Task 3 — independent. Task 2 extends the CPU tree (`ui_tree.rs`, `collect_node`, tree tests); Task 3 extends the SDK + scripting-core bridge. Both consume Task 1's `RingWidget` field set and `NodeContext::Ring`; neither shares a source file with the other.
**Phase 3 (sequential):** Task 4 — consumes the `Ring` factory (Task 3) and the reactive draw behavior (Task 2) to author a live demo and the documented example.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Ring kind | `Widget::Ring(RingWidget)` | `"ring"` | `Ring(props)` → `kind:"ring"` | `Ring(props)` → `kind:"ring"` | n/a |
| Fill mode | `RingFillMode::{Sweep,Radius}` | `"sweep"` / `"radius"` | `fillMode: "sweep"\|"radius"` | same | n/a |
| Start angle | `start_angle: f32` (degrees) | `"startAngle"` | `startAngle` | `startAngle` | n/a |
| Sweep | `sweep: f32` (degrees) | `"sweep"` | `sweep` | `sweep` | n/a |
| Radius | `radius: f32` (logical px) | `"radius"` | `radius` | `radius` | n/a |
| Thickness | `thickness: f32` (logical px) | `"thickness"` | `thickness` | `thickness` | n/a |
| Min radius | `min_radius: Option<f32>` | `"minRadius"` | `minRadius` | `minRadius` | n/a |
| Track color | `track: Option<ColorValue>` | `"track"` | `track` | `track` | n/a |
| Value / max / fill / styleRanges / visibleWhen / id / role | mirror `BarWidget` (`bind: SliderBind`, `max: BarMax`, `fill: ColorValue`, …) | `bind` / `max` / `fill` / `styleRanges` / `visibleWhen` / `id` / `role` | same | same | n/a |

Angles are authored in **degrees** (`0° = 12 o'clock, positive = clockwise`), converted with `.to_radians()` in the collector (house style; `research.md`). `radius`/`thickness`/`minRadius` are logical-reference px, projected to device px like every other rect. The `UiRingInstance` carries device-px geometry and radians.

## Wire format

`UiRingInstance` (CPU, `crates/ui/src/output.rs`) is a new `#[repr(C)] Pod` draw record byte-stable for app→renderer handoff, mirroring `UiInstance`'s discipline (a `size_of`/align assertion test guards the ABI). It carries a device-pixel bounding rect, a linear-RGBA color, and the resolved radial geometry (center-relative outer radius, thickness, start angle, sweep — device px and radians). State the stride/size as the constraint; the implementer places fields. The renderer's `GpuUiRingInstance` appends the painter depth, exactly as `GpuUiInstance` does for quads.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Wire round-trip byte-identity; additive fields skip-serialize when default | Task 1 (`RingWidget` serde) | Every field added or reordered; the SDK factory must emit the canonical form | AC 1, AC 9 |
| Ring is appearance-only — a bound value/max/visibility change never triggers a taffy relayout (fixed-size leaf) | Task 1 (`build_ring` explicit style size), Task 2 (`resolve_bindings` arm flags `appearance_changed` only) | `resolve_bindings` must not `mark_dirty` a ring; `collect_node` must not resize the leaf from the value | AC 6 |
| Single encode per composition + painter order across quad/ring/text; per-layer encode loop unrepresentable | Task 1 (`UiComposition`/`encode` ring stream) | Ring batches must ride the whole-composition encode at their `painter_depth` into the shared depth target | AC 7, AC 8 |
| Fraction contract — `fraction = clamp(value/max,0,1)`; `sweep` drives arc extent, `radius` drives radius lerp; `styleRanges` evaluate this same fraction | Task 1 (sweep), Task 2 (radius, styleRanges) | Both `collect_node` mode branches and the styleRanges value source | AC 3, AC 4, AC 5 |
| Theme-free per-frame draw walk — `fill`/`track`/styleRanges band tokens pre-resolved to literals at build | Task 1 (`build_ring` token resolution), Task 2 (styleRanges pre-resolve) | `build_ring` must resolve every token; `collect_node` never looks a token up | AC 5 |

## Script syntax examples

```ts
// Reload ring — live today (binds the existing `player.reloadProgress` slot,
// as the dev mod's reloadMeterTree already does). A 270° arc that fills as the
// reload completes. Colors shown as literal RGBA (a token name string also works).
Ring({
  bind: bindState(player.reloadProgress),
  max: 1,
  fill: [0.1, 0.8, 1.0, 1.0],
  track: [0.1, 0.1, 0.1, 0.6],
  radius: 40,
  thickness: 6,
  startAngle: 135,
  sweep: 270,
  fillMode: "sweep",
  styleRanges: {
    max: 1,
    entries: [{ upTo: 0.25, color: [1, 0, 0, 1] }, { color: [0, 1, 0, 1] }],
  },
})

// Bullet-spread crosshair — radius mode. Binds a `player.spread` slot (0..1)
// that does NOT exist yet; its gameplay producer is later Weapon-Feel work, so
// this authors against the bindable slot name now (the decoupling seam). The
// ring expands from minRadius to radius as spread grows.
Ring({
  bind: bindState(player.spread),
  max: 1,
  fill: [1, 0.2, 0.2, 1],
  radius: 60,      // bloom radius at spread = 1
  minRadius: 12,   // rest radius at spread = 0
  thickness: 2,
  sweep: 360,
  fillMode: "radius",
  role: "none",
})
```

## Open questions

- **Angle zero/winding convention** is decided (`0° = up, clockwise`) but is the first radial convention in the UI layer; if a future in-world or minimap radial widget wants a different zero, it becomes a shared decision. Recorded, not blocking.
- **`track` semantics in `radius` mode** — the spec draws it at `radius` (a max-bound indicator); most spread crosshairs will omit it. If a consumer wants a different radius-mode track, that is an additive change.
