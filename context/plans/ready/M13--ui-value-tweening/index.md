# M13 Goal TW — UI Value Tweening

> Grounding code anchors: `research.md`. Design source: roadmap M13 TW entry; `context/research/ui-layer.md` §9 (display vs. authoritative values).

## Goal

Time-driven eased display values for bound UI widgets: when a bound slot's value changes, the widget's *displayed* value eases toward the new target over a configured duration and easing curve instead of snapping. The authoritative slot is always the target; a renderer-owned display value chases it; the widget renders the display value and never the slot directly. This is the decoupling seam's payoff — the "systems booting up" count-up on level load with zero game-logic involvement.

**Decision (resolves the roadmap's open coordination):** tweening is a **UI-owned animated-value primitive**, renderer-local in the retained gameplay tree — the same locality class as E's `styleRanges`. It introduces **no mod-facing slot writes**; the store stays authoritative-only and C's deferred `setState` decision remains deferred to E/F, untouched.

## Scope

### In scope

- **Tween wire format.** Optional `tween` sub-object on the existing binds — `TextBind` and `PanelBind` (`render/ui/descriptor.rs`): `{ durationMs, easing, from? }`. Two concrete structs (text `from: Option<f32>`, panel `from: Option<[f32; 4]>`) sharing one `Easing` enum — not one generic type; the wire shape differs only in `from`'s JSON type. `easing` is a closed enum: `linear`, `easeIn`, `easeOut`, `easeInOut` (cubic). `from` is an optional initial display value — a number on a text bind, a length-4 linear RGBA array on a panel bind. Tween-less binds keep their exact current wire form (`skip_serializing_if`).
- **Time source.** `UiReadSnapshot` gains `time_seconds: f64`, filled by the App from its existing dt-accumulated `script_time` (deterministic, never wall-clock — the `StaticUiProxy` precedent). It stays `f64` end-to-end, matching `App::script_time` — deliberately not narrowed to the `f32` the GPU-uniform time path uses. `0.0` default on the splash path. Threaded into the retained build (`UiPass::layout_gameplay_tree` → `UiTree::build_draw_data_retained`); CPU tests feed synthetic times.
- **Tween runtime in the retained diff.** Per-bound-node display state in `NodeContext` (beside `last_resolved`): current display value, tween start value, start time, and target. `resolve_bindings` becomes the tween driver each retained frame:
  - **Target change** → retarget: tween restarts from the *current display value* toward the new target at the frame's time (mid-flight retargeting never snaps).
  - **First resolution** → with `from` present, display starts at `from` and tweens to the target (the level-load flourish); with `from` absent, display snaps to the target (no tween on first sight).
  - **In flight** → the eased display value advances from snapshot time; the node reports changed (text → content-changed/relayout, since the rendered string re-measures; panel → appearance-only/redraw), so the existing relayout/redraw split drives the visible animation.
  - **Settle** → at `t ≥ duration` the display value equals the target exactly; subsequent no-change frames return the cached draw list with no rebuild and no relayout (C's settled-frame guarantee still holds).
  - **Display value reaches the draw** → `collect_node` today re-resolves bound nodes against the raw slot (`resolve_text` / `resolve_panel_fill`), which would bypass the eased value entirely. For a tweened bind, the text and panel branches of `collect_node` render the node's display state (the eased string / fill held in `NodeContext` beside `last_resolved`) instead of re-resolving. Untweened binds and the fresh path keep the existing raw resolution — which is exactly what makes the fresh path inert.
- **Tweenable value rule.** A text tween applies only when the slot resolves to `SlotValue::Number`; a panel tween only to a length-4 `Array`. A tween declared on any other resolved shape snaps through the existing C resolution path with one `log::warn!` per tree build. In-flight text values render through the integral rule (rounded to the nearest integer, formatted via the existing single-`{}` template); on settle the exact target renders through the unchanged C formatting.
- **Fresh-path inertness.** The splash/fresh path (`UiPass::layout_tree` → `build_draw_data`) carries no cross-frame state; a `tween` reaching it resolves directly to the target (inert, no error). Tweening is a retained-gameplay-path feature by construction.
- **Demo + CPU gate.** `demo::build_demo_descriptor`: the health text bind gains `tween { from: 0, durationMs: 1200, easing: easeOut }` — the proxy's `player.health` target is a constant `100`, so the visible 0 → 100 count-up is purely the first-resolve `from` flourish, not an authoritative ramp (the CPU test feeds a constant health with advancing synthetic times); the flash panel bind gains `tween { durationMs: 150, easing: easeInOut }` (the 500 ms proxy toggle eases instead of stepping). The `demo_ui_gate_test` assertions are updated for the tweened behavior. CPU tests with synthetic times cover: first-resolve `from` flourish, mid-flight retarget, easing monotonicity, exact-target settle, post-settle no-rebuild, the non-numeric snap-with-warn, and unbound/tween-less nodes unaffected.

### Out of scope

- **The `bar` widget and the literal eased health bar** — `bar` is F; the eased health bar lands as a BIS built-in once TW + F both exist (owner decision, 2026-06). TW's demo proves the same model on `text` + `panel`.
- **Mod-facing slot writes / `setState`** — explicitly not introduced here (the Decision above); stays deferred to E/F.
- **`styleRanges` / `onStateCrossing`** — E. TW animates *values*; E maps values to styles and crossings to reactions. Composition (a tweened display value driving a `styleRange`) is E's concern against this shipped primitive.
- **Timeline/keyframe curves, springs, repeat/yoyo** — the four-curve cubic set is the whole D1 surface; richer curves are additive enum variants later (research §19 "animation richness").
- **Tweening layout properties** (size, spacing, position) — only bound display values tween. Layout animation has no consumer and fights the dirty-gate.
- **Tween on `ContainerWidget` fills** — container backdrops never bind (C invariant); nothing to tween.
- **Wall-clock time** — snapshot time is dt-accumulated like `script_time`; pausing game logic pauses tweens, which is the wanted behavior for a presentational layer.

## Acceptance criteria

- [ ] A text bind with `"tween": {"durationMs": 1200, "easing": "easeOut", "from": 0.0}` and a panel bind with `"tween": {"durationMs": 150, "easing": "easeInOut"}` round-trip byte-identically; a tween-less bind keeps its exact pre-TW wire form (existing B/C fixtures unchanged).
- [ ] `UiReadSnapshot` carries a frame time in seconds sourced from the App's dt-accumulated clock; the splash path publishes `0.0`; no wall-clock read exists in the tween path (the no-wall-clock clause is a review/grep gate).
- [ ] On the first frame a tweened slot resolves, a bind with `from` renders the `from` value and subsequent frames advance monotonically along the easing curve toward the target, reaching it exactly at `durationMs`; a bind without `from` renders the target immediately.
- [ ] While a text tween is in flight, each advancing frame re-measures and relays out (recompute counter increments) and the rendered string is the rounded-integer form through the bind's format template; while a panel tween is in flight, frames rebuild the draw list with **no** relayout (recompute counter flat).
- [ ] After a tween settles, a no-change frame performs no draw-list rebuild and no relayout — C's settled-frame AC still passes with tweens present.
- [ ] A target change mid-flight retargets from the current display value (no snap, no restart-from-`from`); the displayed value is continuous across the retarget frame.
- [ ] A tween declared on a slot that resolves to a non-tweenable shape (e.g. a string on a text bind) renders via the unchanged C resolution (CPU assertion); the warn-once clause is a review/manual gate.
- [ ] A tweened descriptor driven through the fresh/splash path renders the target directly (inert) with no cross-frame state and no warning.
- [ ] The demo HUD shows HP counting up 0 → 100 over ~1.2 s on level load and an eased flash swatch; verification reuses A/B/C's approach — pure-CPU synthetic-time assertions plus a manual run per the project build/run commands.
- [ ] No store write originates from the UI module: tween state lives in the renderer's retained tree; `write_store_slot` call sites are unchanged. Review/grep gate, not a runnable test.

## Tasks

### Task 1: Tween wire format
In `render/ui/descriptor.rs`: the `tween` sub-structs on `TextBind` (`from: Option<f32>`) and `PanelBind` (`from: Option<[f32; 4]>`) sharing one `Easing` serde enum (camelCase wire), all `skip_serializing_if` so tween-less binds round-trip unchanged. `duration_ms` is `u32` — integer milliseconds on the wire — so the fixture `"durationMs":1200` round-trips byte-identically (an `f32` would re-emit `1200.0` and break round-trip identity). Round-trip tests per shape plus the no-wire-break fixture assertions. Pure data.

### Task 2: Snapshot time plumbing
`UiReadSnapshot` (`render/ui/mod.rs`) gains `time_seconds: f64` (default `0.0`); `with_gameplay_tree` gains the time argument — its only callers are the publish site in `main.rs` (passes `self.script_time`) and `gameplay_ui_gate_test.rs`. `demo_ui_gate_test` does not call `with_gameplay_tree`; it drives `build_draw_data_retained` directly and feeds synthetic times there (Tasks 3/4). `UiPass::layout_gameplay_tree` and `UiTree::build_draw_data_retained` gain the time parameter; the gameplay record block in `render/mod.rs` passes `self.ui_snapshot.time_seconds`. The fresh path (`layout_tree`/`build_draw_data`) takes no time — inertness falls out structurally.

### Task 3: Tween runtime in the retained diff
In `render/ui/tree.rs`: extend `NodeContext::Text`/`Panel` with the display state (display value, tween start value/time, target, warn-once latch); the four cubic easing functions as module-local helpers; rewrite `resolve_bindings` into the tween driver per the Scope mechanics (retarget / first-resolve / in-flight advance / exact settle), classifying each frame's change through the existing `BindingDiff` so the relayout/redraw split and `mark_dirty` path are reused, not duplicated. Two correctness seams: (1) **Target tracking and display state are separate fields.** The raw resolved target is stored in its own field for retarget detection and is never overwritten by the eased value. For text, the eased display string flows through `last_resolved` — so `measure_node` shapes the displayed value unchanged — while the raw target lives separately; for panel, the eased fill is a new field, and `collect_node`'s text and panel branches (which today re-resolve the raw slot via `resolve_text` / `resolve_panel_fill`) render the display state for tweened binds instead: the load-bearing change that puts the eased value on screen. A driver that overwrites the target with the eased value breaks retarget detection. (2) **In flight means dirty every frame, independent of target equality.** The existing classifier compares values — but a tween against a constant target (the demo's `player.health` = 100) sees no value change, so naive value-comparison would never animate the count-up. The driver reports the node's class (relayout for text, redraw for panel) each frame from tween-clock state until settle, then stops. Panel tweens ease per-channel: four independent eased lerps from start to target linear RGBA, alpha included, no rounding. When editing the `NodeContext` literals, follow the concurrency note's rule: list fields explicitly, no `..` in struct literals. Depends on Tasks 1 + 2.

### Task 4: Demo + CPU gate
Add the two tweens to `demo::build_demo_descriptor`; rework `demo_ui_gate_test` for the tweened frame sequence — the existing numeric expectations are now **wrong, not merely incomplete** (the panel change eases over 150 ms instead of rebuilding once; `HP 100` is no longer drawn on the first frame), and every `build_draw_data_retained` call site in that file gains the Task 2 time argument. Extend the CPU harness with the synthetic-time suite from the AC list. The `StaticUiProxy` is untouched — its step-toggle output is exactly what the panel tween smooths.

## Sequencing

**Phase 1 (concurrent):** Task 1 (wire) and Task 2 (time plumbing) — disjoint files except the `mod.rs` snapshot struct, which only Task 2 edits.
**Phase 2 (sequential):** Task 3 — consumes Task 1's tween types and Task 2's time parameter.
**Phase 3 (sequential):** Task 4 — consumes Task 3.

## Concurrency note (D ‖ TW orchestrate wave)

TW and `M13--fonts-theming` are planned as one concurrent wave. Unavoidable shared files: `render/ui/descriptor.rs` (TW edits the bind structs; D edits widget field types), `render/ui/tree.rs` (TW adds tween state to `NodeContext` and rewrites `resolve_bindings`; D adds resolved theme values to `NodeContext` and `build_node`), and `render/ui/mod.rs` (TW widens signatures with time; D widens them with the theme). Conflicts are additive — new fields, new parameters — not semantic. The tightest spot is the shared `NodeContext::Text`/`Panel` variants: both specs add fields to them, so at merge every struct-literal constructor (in `build_node`) must list both specs' fields explicitly (no `..` in literals), while match arms keep `..` rest-patterns to tolerate the other spec's fields. Run the two in isolated worktrees and merge-coordinate those three files at integration; whichever lands second rebases its signature changes mechanically.

## Boundary inventory

The `tween` object crosses Rust ↔ wire (JSON); JS/Luau ingestion lands at G1 with casing locked here. Rust snake_case; wire camelCase.

| Name | Rust | Wire / serde | JS / TS (G1) | Luau (G1) |
|---|---|---|---|---|
| tween object | `tween: Option<…>` on `TextBind`/`PanelBind` | `tween` (`{ durationMs, easing, from? }`; omitted when absent) | `tween` | `tween` |
| duration | `duration_ms: u32` | `durationMs` (integer milliseconds) | `durationMs` | `durationMs` |
| easing | `enum Easing` | `"linear"` / `"easeIn"` / `"easeOut"` / `"easeInOut"` | same literals | same literals |
| initial value (text) | `from: Option<f32>` | `from` (number; omitted when absent) | `from` | `from` |
| initial value (panel) | `from: Option<[f32; 4]>` | `from` (length-4 linear RGBA array; omitted when absent) | `from` | `from` |
| snapshot time | `time_seconds: f64` | n/a (engine-internal, never serialized) | n/a | n/a |

## Open questions

- **Easing curve family.** Cubic in/out/in-out plus linear is the pinned v1 set. If review wants a different polynomial (quad/quint) the enum names stay and only the exponents move — decide at review, default cubic.
- **In-flight integer rounding.** Rounded-nearest is specified for in-flight text. If a future fractional authoritative value (e.g. `12.5`) needs fractional in-flight rendering, that widens to a format-aware rule — out of scope until a consumer exists; the settled frame already renders fractional targets exactly.
- **Tween state across descriptor rebuilds.** A structural rebuild (new descriptor or D's theme-generation bump) discards `NodeContext`, so an in-flight tween snaps to target on rebuild — accepted: structural changes are rare, authored events. Confirm at review.
