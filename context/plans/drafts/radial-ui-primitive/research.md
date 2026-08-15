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

## Geometry model decision

One `Ring` widget, one bound numeric `value`/`max` → `fraction = clamp(value/max, 0, 1)`. `fillMode` selects what `fraction` drives:

- `sweep` (default): fill arc spans `startAngle … startAngle + fraction*sweep`; track (if present) spans the full `sweep`. Radius fixed at `radius`. Consumers: cooldown arc (`player.weaponCooldownMs`), charge/reload ring (`player.reloadProgress`) — **live today**.
- `radius`: fill ring spans the full `sweep` at radius `lerp(minRadius, radius, fraction)`; track (if present) at `radius`. Consumer: bullet-spread crosshair — **data source is future Weapon-Feel work**.

`styleRanges` evaluate `fraction` in both modes (a bloomed crosshair can turn red at max spread), matching the Bar contract. The Ring reuses `drive_bar_binding`/`drive_bar_max` verbatim — it is numeric-and-fixed-size exactly like a Bar, so the retained diff is appearance-only with no new tween code.

v1 is **clockwise-only** and **no exit-fade** (exit-fade is documented Bar-only). Counter-clockwise winding (`clockwise: false`) and a per-instance label/`fontSize` are trivial additive follow-ups if a consumer needs them.

## Angle convention decision

Author `startAngle`/`sweep` in **degrees**; `0° = 12 o'clock (straight up)`, **positive = clockwise**; convert with `.to_radians()` in the collector before building the ring instance. This matches the degrees-authored house style and is the intuitive HUD/gauge convention. The collector accounts for UI Y-down device space when mapping angle → screen direction.

## Scope alternative considered (recorded for /validate-plan)

Building only `sweep` mode (the radial Bar) and deferring `radius`/spread was the closest rival. Rejected as the primary because the user's motivating consumer (bullet spread) needs radius drive, and radius mode rides ~all of the shared machinery (descriptor variant, NodeContext, SDF shader, pipeline) — the marginal cost is a `fillMode` enum, a `minRadius` field, and one collector branch. The thin slice still proves the boundary against `sweep` mode (live data), so the speculative radius mode ships on validated, tested machinery — the same static-proxy decoupling pattern Epic 13 used against Epic 10.
