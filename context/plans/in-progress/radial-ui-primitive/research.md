# Radial UI primitive — research notes

Derivation and investigation behind `index.md`. Not an execution contract; the spec captures decisions.

## Source grounding (verified this session)

- **Draw-list ABI.** `UiInstance` (`crates/ui/src/output.rs`) is a byte-stable `#[repr(C)] Pod` of four `vec4` (rect, uv_rect, color, margin), asserted 64 bytes / align 4 by `ui_instance_byte_layout_is_64_bytes_no_padding`. It expresses only axis-aligned quads. The renderer converts it to `GpuUiInstance` (adds painter depth) in `crates/renderer/src/render/ui/mod.rs`.
- **One shader, two pipelines.** `crates/renderer/src/shaders/ui_quad.wgsl` vertex-expands one instance into 9-slice regions (54 verts). `UiPass` builds `opaque_pipeline` / `translucent_pipeline` from it — identical but for `depth_write_enabled`. Panels sample a 1×1 white texel; images their own texture; text is glyphon's separate pipeline.
- **Collector.** `UiTree::collect_node` (`crates/ui/src/tree/ui_tree_collect.rs`) matches `NodeContext` and emits into `UiDrawData` (`crates/ui/src/tree/draw.rs`) via `push_quad`/`push_image`/`push_text`, each appending a `UiPaintOp` to `paint_order`. A `Bar` emits a background quad then a fill quad whose width is `fraction * rect_width` (`fraction = clamp(value/max, 0, 1)`); `styleRanges` recolors the fill from that fraction.
- **Retained diff.** `UiTree::resolve_bindings` (`crates/ui/src/tree/ui_tree.rs`) matches `NodeContext` mutably per frame; `drive_bar_binding` + `drive_bar_max` (`crates/ui/src/tree/bindings.rs`) ease the numeric value/max and flag `appearance_changed` (a Bar is fixed-size → never relayout). Tween mechanics live in `drive_tween_f32`.
- **Composition + encode.** `UiComposition::from_layer_draws` folds each layer's `UiDrawData` `paint_order` into ordered single-instance batches (`OrderedUiBatch`) + concatenated text, assigning a monotonic `order`. `UiPass::encode` draws every quad batch first (each into a disjoint instance-buffer region at its `painter_depth`), then one glyphon text draw, all against a private UI depth target (`Depth24Plus`, cleared each encode). The encode boundary is the whole composition — the per-layer loop is unrepresentable (the historical glyphon clobber guard).
- **Blast radius.** `Widget` (`crates/scripting-core/src/ui/descriptor/widgets.rs`) is matched exhaustively (no wildcard, breaks compile) at: `build_node` (`crates/ui/src/tree/build.rs`), `focus_meta` + `widget_visible_when` (`crates/ui/src/tree/widget_meta.rs`), `implicit_role` (`crates/scripting-core/.../accessibility.rs`). Wildcard `Widget` matches that a passive leaf falls through correctly: `widget_interaction`, `container_local_scope`, `widget_a11y_state`, `container_focus_policy`, `any_restore_on_return`, `widget_children` (all `widget_meta.rs`); `widget_local_state` + `widget_children` (`crates/postretro/.../presentation_cells.rs`). Script bridges match on the `"kind"` string, not the enum: `widget_from_js` / `widget_from_lua` (`crates/scripting-core/src/data_descriptors/{js,lua}/ui_widgets.rs`) — a new kind is unconstructable from script until an arm is added. `NodeContext` (`crates/ui/src/tree/node_context.rs`, in-crate only) is matched exhaustively at `collect_node`; wildcard at `measure_node`, `harvest_visibility`, `resolve_bindings` (two sites).
- **SDK path.** `Bar(...)` factories in `sdk/lib/ui/widgets.{ts,luau}` use shared `buildBind`/`buildBarMax`/`requireColor`/`buildStyleRanges`/`applyA11yFields`; types in the same files + generated template `crates/scripting-core/src/typedef/templates/ui_sdk_module.d.ts`. Raw serde asset loads run the same validation via `#[serde(try_from = "BarWidgetWire")]` + `BarWidget::validate`. Bridge tests: `crates/scripting-core/src/data_descriptors/tests/ui_bridge.rs`.
- **Consumer.** The crosshair today is `content/dev/scripts/hud.ts` `reticle` = `defineUiTree({ name: "hud.reticle", alwaysOn: true, tree: Tree({anchor:"center"}, Text({content:"+"})) })`. No radial widget exists.
- **Data.** `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`) has NO `player.spread`/`bloom`/`accuracy`. `player.weaponCooldownMs` and `player.reloadProgress` DO exist and are live. Weapon `spreadDegrees` (`crates/foundation/src/data_descriptors/types/combat.rs`) is a static per-shot cone half-angle, not a per-frame value, and is not exposed through `getGameState`. A live "current spread/bloom" producer is genuinely future Weapon-Feel work.
- **Angle convention.** No angle handling exists under `crates/ui/`. House style (`view_feel.rs`, `spreadDegrees`, `movement`): author in degrees, `.to_radians()` at the boundary. UI is Y-down device space (`Anchor` table, HUD `offset:[24,-24]`).

## Why a second pipeline, not an extended `ui_quad.wgsl` instance

`UiInstance` is a locked 64-byte ABI (test-asserted) shared by panels and images through the 9-slice vertex expansion. A radial primitive needs (a) different instance parameters (radii, angles) and (b) a fragment-SDF fill instead of 9-slice geometry. Overloading the quad instance/shader with mode branches couples two unrelated primitives on one ABI and muddies the "panels + images" pipeline. The roadmap names "a shader path beyond `ui_quad.wgsl`" — a separate translucent SDF pipeline. Rings are always anti-aliased-edged (translucent), so one ring pipeline (depth-test, no depth-write) suffices; no opaque variant needed.

## Encode painter-order lifecycle (the cross-seam risk)

Adding a ring batch stream to `UiComposition` must not break the single-encode contract or painter ordering. Rings draw after quads, before text, each at its `painter_depth(order)` into the shared UI depth target. Opaque lower-layer quads write depth before rings draw, so an opaque upper panel correctly occludes a lower ring; rings (translucent, no depth write) never hard-erase lower text — the same source-over caveat the existing translucent-quad path already carries.

```mermaid
sequenceDiagram
    participant C as UiComposition::from_layer_draws
    participant E as UiPass::encode
    participant Q as quad instance buffer
    participant R as ring instance buffer
    participant T as glyphon
    participant D as UI depth target

    Note over C: fold each layer's paint_order → order N
    C->>C: Quad{i} → OrderedUiBatch(order)
    C->>C: Ring{i} → OrderedRingBatch(order)
    C->>C: Text{i} → text_orders.push(order)
    C->>E: batches + ring_batches + texts + order_count

    E->>D: begin pass, clear depth=1.0
    loop each quad batch
        E->>Q: write region @ offset
        E->>D: draw at painter_depth(order); opaque writes depth
    end
    loop each ring batch
        E->>R: write region @ offset
        E->>D: draw at painter_depth(order); test-only (translucent)
    end
    E->>T: one render() over shaped text at their depths
    E->>D: discard depth
```

Every arrow here has a read/write call site in `encode`; the diagram drives the code-grounding for Task 1's composition/encode changes.

## Geometry model decision (revised — pure shape primitive)

**Superseded model (recorded so the reversal is legible in the diff, not the spec body):** an earlier draft modeled the Ring on `Bar` — one privileged bound `value`/`max` → `fraction`, a `fillMode` selecting whether the fraction drove sweep or radius, `styleRanges` recoloring the fill, reusing `drive_bar_binding`/`drive_bar_max`. The owner rejected the framing: `bind` presumes the widget *exists to visualize one number*, which is true of a Bar but wrong for a ring — a ring is a **shape** with several independently-authorable properties (radius, thickness, sweep, start angle, fill), any of which may be static or track state, and often none is privileged. A static reticle ring has no value at all, which the Bar-style model cannot even express (Bar requires a bind).

**Chosen model.** The Ring is an annulus-arc shape. Its layout box is authored statically (`diameter`, reserving a fixed square leaf — so bound draw values never relayout). Each geometric draw property — `radius`, `thickness`, `startAngle`, `sweep` — is uniformly `literal | bound`, passed straight into its own named prop (the `max: number | ComputedRef<number>` precedent generalized to every property). A bound property reads its slot **1:1** in the property's own units (px for radius/thickness, degrees for angles), with an optional presentation `tween` riding the bound ref. `fill` and optional `track` (a full-360 background annulus) are colors. There is **no `bind`, no `max`, no `fillMode`, no `styleRanges`** — the ambiguity `fillMode` resolved is gone once you bind the property you mean.

**Why 1:1 and not a baked `from→to` range.** The UI layer cannot compute per-frame (the VM drops; bindings are name→value with no script math). So a bound geometric prop needs either a slot already in the property's units or a widget-side mapping. A per-prop `from→to` range would work, but the owner correctly placed value mapping (spread → px, cooldown-ms → degrees) on the **Behavior IR** (Epic 14, `scripting.md` §11): authored `f(state)` evaluated Rust-side each tick. That is the general "UI reads a computed value from game state" substrate; baking a bespoke range into every widget prop is exactly the pre-IR special case Epic 14's "primitive consolidation" exists to retire. So this spec keeps the ring a dumb 1:1 shape and defers all value transforms to a **successor spec** (see below).

**Consequence for consumers.** No live slot is in UI-property units today (`reloadProgress` is 0..1, `weaponCooldownMs` is ms), so the compelling *dynamic* consumers — cooldown arc, bullet-spread crosshair — are unblocked by the successor IR-mapping spec (and, for spread, the gameplay producer), not by this one. v1 ships the shape + 1:1 binding hooks + a static reticle-ring demo. This is honest and lean: the shape and its render pipeline land now; the computed bindings land on the IR.

**Binding sources — why both `{slot}` and `{local}`.** `BoundScalar` flattens the shared `BindSource` (`{slot}`/`{local}`), the same source set `SliderBind` carries — not a new decision to make per-widget. Once the wire type accepts `{local}`, the Rust bridge deserializes it and AC-1 round-trips it, so Task 2 must resolve local sources regardless of the SDK surface. Given that, exposing `{local}` on `RingProps` (mirroring `SliderBindProp`) is the only choice that keeps the SDK type and the wire symmetric; withholding it would let raw-asset authors write a source the typed factory can't. A passive ring binding its radius 1:1 to a container's local cell is a legitimate no-transform use, so there is no reason to special-case the ring narrower than the shared type. Not an open question.

v1 is **clockwise-only**. Counter-clockwise winding (`clockwise: false`), a per-instance label, and bindable `fill`/`track` colors are trivial additive follow-ups.

## Successor spec (owner's call, recorded — not drafted here)

**UI computed bindings via Behavior IR.** Let a UI bind resolve a value the IR computes from game state (`radius = map(player.spread, [0,1] → [12,60])`, `sweep = map(cooldownFrac, [0,1] → [0,270])`) instead of only a raw slot. This is the general home for value transforms and likely subsumes `styleRanges` (value → color) and `Bar::max` normalization too — an Epic-14 "primitive consolidation" item, its own `/draft-plan`. The Ring is its first real consumer. Out of scope for the ring primitive itself.

## Angle convention decision

Author `startAngle`/`sweep` in **degrees**; `0° = 12 o'clock (straight up)`, **positive = clockwise**; convert with `.to_radians()` in the collector before building the ring instance. This matches the degrees-authored house style and is the intuitive HUD/gauge convention. The collector accounts for UI Y-down device space when mapping angle → screen direction.

## Alternatives considered

- **Bar-style value-visualizer (privileged `bind` + `max` + `fillMode`).** Rejected by the owner — see "Geometry model decision." Wrong frame for a shape whose properties vary independently and are often all static.
- **Per-prop `from→to` range baked into the widget.** Solves the no-compute mapping, but multiplies the pre-IR special case Epic 14 exists to consolidate. Deferred to the successor IR-bindings spec.
- **Defer the whole primitive, ship co-located with the Weapon-Feel producer.** Rejected by the roadmap's own framing — the Epic 13 deferred entry names this "cross-cutting UI-layer work, reusable across features," i.e. UI-owned infrastructure built ahead of any single consumer.
- **Ship only a static shape, no binding at all.** Rejected — the 1:1 binding hooks are cheap (reuse `drive_tween_f32`), and a shape with zero dynamic capability would force a second breaking pass the moment the IR spec lands. The hooks are the seam the successor consumes.
