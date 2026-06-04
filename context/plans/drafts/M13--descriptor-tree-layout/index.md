# M13 Goal B — Descriptor Model + Retained Tree + Layout

## Goal

Build the Rust-owned UI descriptor model, retained widget tree, and `taffy`-driven layout — the second spec of the UI milestone (Milestone 13). Goal A shipped a real UI render pass and reimplemented the splash on it; this goal replaces the splash's hand-assembled element list with a real descriptor tree + flex layout, and **locks two pulled-forward contracts**: the descriptor wire format (discriminated-union-per-kind) and the `taffy` ↔ `glyphon` text-measure seam. Built in Rust and tests only — no script ingestion, no state slots.

## Scope

### In scope

- **Module split (A follow-up #1).** Extract the `glyphon` half of `render/ui/mod.rs` into `render/ui/text.rs` (font system, atlas, shaping, text render). `mod.rs` keeps the quad pass + pass orchestration. Done first — every later task edits these files.
- **Add `taffy` dependency** to the workspace (absent today). The descriptor model and layout build on it.
- **Descriptor model + wire format.** A serde discriminated union, one variant per widget kind, internally tagged on a `kind` field, camelCase wire. Seven kinds: `text`, `panel`, `image`, `vstack`, `hstack`, `grid`, `spacer`. Deserialize from JSON, re-serialize identically, reject unknown kinds. This is the locked wire contract — see *Boundary inventory*.
- **Rust-owned retained tree.** A node tree keyed to mirror `taffy`'s `NodeId` shape, owned by the renderer's UI module, walked every frame. Replaces the manual element accumulation in `record_splash_ui` (`render/mod.rs`).
- **`taffy` layout.** Map descriptor nodes → `taffy` styles (flex direction from stack kind, gap, padding, align, sizing; grid cols/gap) → compute layout → read computed rects back into the existing `UiDrawList` via the `layout::project`-style projection. Flexbox for stacks, CSS grid for `grid`.
- **Anchor + offset as a post-layout transform.** Reuse the existing `Anchor` enum (`layout.rs`, nine variants, currently `#[allow(dead_code)]` — this goal activates them). Top-level trees anchor into the 1280×720 reference box with a pixel offset, applied to `taffy`'s root rect *after* layout. Aspect-ratio-stable, as in A.
- **Measure seam (`taffy` ↔ `glyphon`).** Size text nodes from real shaped-glyph metrics. Wire the existing `glyphon` shaping (`UiPass::shape_text`, now in `text.rs`) into `taffy`'s per-tree measure closure via a per-node text context. The slice proved the shaped-text path in A; this feeds its measured width/height back to layout.
- **Dirty-tree relayout.** Recompute layout only when the tree structure changes or the viewport resizes, via `taffy`'s intrinsic `mark_dirty` + cached subtree layout.
- **Splash on the tree (the A→B handoff).** Reimplement `build_splash_descriptor` (`render/ui/splash.rs` — the named seam A left) to emit a descriptor tree (panel + image + text nodes) instead of hand-built `UiElement`s. Splash visual output unchanged.
- **Render integration.** Widen `UiReadSnapshot` to carry the laid-out draw output; populate `UiPass::encode`'s `batches`/`texts` on the gameplay path from a retained tree (empty today). Add the empty-tree encode early-out (A follow-up #3). Center splash version text from measured run width (A follow-up #4). Call `TextAtlas::trim()` once per frame (A follow-up #2).
- **CPU layout/draw-list test gate.** Extend A's pure-CPU assertion harness to cover the new vocab: flex distribution, grid placement, measured-text sizing, anchor-against-letterbox, integer pixel snapping.
- **Boundary inventory artifact** pinning casing for all seven kinds and their cross-boundary fields.

### Out of scope

- **State system** — `defineState`, `StateValue<T>`, the slot table, bindings, value-diffing-driven invalidation. → **Goal C**. B's descriptors carry no `bind` field; all content is literal. *Highest scope-creep risk: keep bindings out and dirty triggers structural-only.*
- **Widgets needing slots or events** — `bar`, `button`, `input`, `list`, `slider`, `viewport`. → C / F.
- **`format` text templates** (`"{}/{max}"`) — interpolate bound values; no inputs without slots. → C.
- **`styleRanges`, `onStateCrossing`** — per-frame value→style. → E.
- **Input** — hit-testing, focus ring, nav intents, hold-to-repeat, modal stack, gamepad. → F. A's `input/ui_dispatch.rs` seam is sufficient; B does not own input.
- **Script ingestion** — QuickJS/Luau `from_*_value` for descriptors, the JSX/factory SDK, localization. → G1. B defines the serde target type the bridges will later deserialize into; it does not touch the VM bridges.
- **Theme tokens, multi-font registration.** → D. B uses A's single registered font and literal RGBA colors.
- **Screen-space effects, egui retirement.** → SE / BIS.
- **Arbitrary runtime asset-key streaming for `image`.** B resolves image nodes through the existing UI-texture upload path the splash logo already uses (a small key→bind-group registry); script-driven dynamic asset loading is G1 / resource-management.

## Acceptance criteria

- [ ] A descriptor tree of all seven kinds round-trips through serde JSON: `{"kind":"vstack", ...}` deserializes and re-serializes to identical JSON; an unknown `kind` deserializes to an error, not a panic.
- [ ] `taffy` lays out a nested `vstack`/`hstack`/`grid` tree; child rects match expected flex/grid distribution at the 1280×720 reference and scale uniformly at 4K (mirrors A's resolution tests).
- [ ] A `text` node's computed size comes from `glyphon` shaped-run metrics, not a glyph-count estimate; changing its `content` changes its measured width.
- [ ] A top-level anchored tree centers against the letterbox on a non-16:9 viewport (reuses A's anchor assertion).
- [ ] The boot splash renders through the retained descriptor tree — `build_splash_descriptor` returns a node tree — with panel, logo, and version text visually unchanged; version text centers via measured width.
- [ ] Layout recomputes only on tree-structure change or viewport resize; a no-change frame performs no `taffy` recompute (verifiable via a recompute counter or dirty flag in a test).
- [ ] The gameplay render path populates a non-empty UI draw list from a descriptor tree; an empty tree early-outs the UI pass (no `begin_render_pass`).
- [ ] Computed quad/panel rects snap to integer device pixels; text glyphs remain exempt (reuses A).
- [ ] `taffy` is a workspace dependency; no QuickJS/Luau ingestion or `StateValue`/slot code is added.
- [ ] The Boundary inventory table pins Rust/wire/JS/Luau casing for every kind and its fields.

## Tasks

### Task 1: Module split + `taffy` dependency
Extract `glyphon` state and shaping out of `render/ui/mod.rs` into a new `render/ui/text.rs`; `mod.rs` retains the quad pipeline and pass orchestration. Behavior-identical move (A follow-up #1). Add `taffy` to the workspace `Cargo.toml` and pull it into the `postretro` crate. Land `TextAtlas::trim()` once per frame in the text path (A follow-up #2). No new UI behavior — this is the seam every later task builds on.

### Task 2: Descriptor model + wire format
Define the serde descriptor types: an internally-tagged enum (`#[serde(tag = "kind", rename_all = "camelCase")]`) with struct variants for the seven widget kinds, plus their field structs (camelCase wire, snake-case Rust). Children are positional `Vec`s on container kinds. Round-trip + unknown-kind-error tests. Produce the Boundary inventory. Pure data; no rendering. Diverges deliberately from the manual key-presence discrimination used by `ReactionDescriptor` (`scripting/data_descriptors.rs`) — see *Rough sketch*.

### Task 3: Retained tree + `taffy` layout
Build the Rust-owned node tree and the descriptor→`taffy` mapping: stack kind → flex direction, gap/padding/align/sizing → style, grid → CSS-grid columns/gap. Compute layout, read `taffy::Layout` rects back into `UiDrawList` through the existing projection path. Apply the `Anchor` + offset post-layout transform on the root rect. Wire `taffy` `mark_dirty` so relayout runs only on structural or viewport change. Text nodes get a placeholder intrinsic size here; real measurement lands in Task 4.

### Task 4: Measure seam (`taffy` ↔ `glyphon`)
Replace the text-node placeholder size with real measurement: attach a per-node text context carrying the `glyphon` shaping handle, and supply `taffy`'s measure closure so it shapes through `text.rs` and returns the measured `Size`. Layout now sizes text from glyph metrics. Consumes Task 3's tree and Task 1's `text.rs`.

### Task 5: Splash-on-tree + render integration + gate
Reimplement `build_splash_descriptor` to emit a descriptor tree (panel + image + text); the splash logo resolves as an `image` node through the existing UI-texture path. Widen `UiReadSnapshot` to carry the laid-out output; populate `UiPass::encode` `batches`/`texts` on the gameplay path from a retained tree; add the empty-tree early-out (A follow-up #3); center version text from measured width (A follow-up #4). Extend A's CPU assertion harness to the full vocab as the hard gate (flex, grid, measured text, anchor, snapping). Splash output stays visually unchanged.

## Sequencing

This is a layered foundation — each task consumes the prior, so the chain is mostly sequential.

**Phase 1 (sequential):** Task 1 — module split + `taffy` dep. Blocks all; every later task edits `render/ui/`.
**Phase 2 (sequential):** Task 2 — descriptor model. Consumes Task 1's module skeleton; defines the types Task 3 maps.
**Phase 3 (sequential):** Task 3 — retained tree + layout. Consumes Task 2's descriptor types and Task 1's `taffy` dep.
**Phase 4 (sequential):** Task 4 — measure seam. Consumes Task 3's `taffy` tree and Task 1's `text.rs`.
**Phase 5 (sequential):** Task 5 — splash + render integration + test gate. Consumes Tasks 3 and 4.

## Rough sketch

**Wire format / discriminated union.** Make the Rust enum the source of truth with `#[serde(tag = "kind", rename_all = "camelCase")]`; `serde_json::from_value` does the dispatch — no third hand-rolled discriminator. This diverges from `ReactionDescriptor` (`scripting/data_descriptors.rs`), which discriminates by manual `contains_key` on payload keys, duplicated across the QuickJS and Luau bridges. Reactions stay as-is; descriptors adopt serde tagging because it is less code and matches the research's `{ kind: ... }` shape. Constraint: internally-tagged serde rejects tuple variants and buffers through `serde_json::Value` on deserialize — use **struct variants only** (already how the repo round-trips via `conv.rs`'s `js_to_json`/`lua_to_json` → `serde_json::from_value`). G1 later feeds VM-produced JSON through that same bridge into these types; B does not touch the bridges.

**`taffy`.** Use `0.10`. Hold the tree in a `taffy::TaffyTree<NodeContext>` where `NodeContext` carries per-node data (text shaping handle for `text` nodes, image handle for `image`). Run `compute_layout_with_measure` with a single global measure closure; the closure shapes text nodes via `text.rs` and returns their `Size<f32>`. Grid maps to `taffy`'s CSS-grid track support.

**Anchor.** Keep `Anchor` as the existing nine-variant enum and a reference-space concept (the A-locked native-render + 1280×720 logical-reference scaling model), not a `taffy` style. After `taffy` computes the root's content size, place that root in the reference box per anchor + offset, then project to device pixels through the existing `layout` path.

**Tree multiplicity.** One `taffy` tree per top-level descriptor. F's modal stack will want independent trees; choosing per-tree now avoids that rework.

**Render handle.** `UiReadSnapshot` (currently `version_line: String`) widens to carry the frame's laid-out output (or a handle the renderer resolves to `UiBatch`/`UiText`). Content stays static/literal in B — C owns slot-driven content. The snapshot remains the game-logic→render contract A established.

**Key files.** `render/ui/mod.rs` (split source, quad pass), new `render/ui/text.rs` (glyphon), `render/ui/layout.rs` (`Anchor`, `project`), `render/ui/splash.rs` (`build_splash_descriptor` seam), `render/mod.rs` (`record_splash_ui`, `UiReadSnapshot`, `set_ui_snapshot`), new descriptor + tree modules under `render/ui/`. Governing doc for wire/casing conventions: `context/lib/scripting.md`.

## Boundary inventory

UI descriptors cross Rust ↔ wire (JSON) ↔ JS/TS ↔ Luau (script ingestion lands in G1, but the casing is locked here). No FGD surface. Rust fields are snake_case; wire/JS/Luau are camelCase via `#[serde(rename_all = "camelCase")]`.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| discriminant | `enum Widget` variants | `"kind"` field: `"text"`/`"panel"`/`"image"`/`"vstack"`/`"hstack"`/`"grid"`/`"spacer"` | same string literals | same string literals |
| text content | `content: String` | `content` | `content` | `content` |
| text size | `font_size: f32` | `fontSize` | `fontSize` | `fontSize` |
| color | `color: [f32; 4]` | `color` (RGBA array) | `color` | `color` |
| panel fill | `fill: [f32; 4]` | `fill` | `fill` | `fill` |
| panel border | `border: Option<Border>` (9-slice) | `border` | `border` | `border` |
| image asset | `asset: String` (key) | `asset` | `asset` | `asset` |
| container gap | `gap: f32` | `gap` | `gap` | `gap` |
| container padding | `padding: f32` (or rect) | `padding` | `padding` | `padding` |
| container align | `align: Align` | `align` | `align` | `align` |
| grid columns | `cols: u32` | `cols` | `cols` | `cols` |
| spacer grow | `flex_grow: f32` | `flexGrow` | `flexGrow` | `flexGrow` |
| children | `children: Vec<Widget>` | `children` (positional array) | positional args | positional args |
| top-level anchor | `anchor: Anchor` | `anchor` (e.g. `"topLeft"`, `"center"`) | `anchor` | `anchor` |
| top-level offset | `offset: [f32; 2]` | `offset` | `offset` | `offset` |

Exact field set per kind is the implementer's call within these casing rules and the *Rough sketch* constraints; the table pins every cross-boundary name and its encoding.

## Open questions

- **`grid` test depth.** B has no grid screen to exercise (splash is panel + image + text), but the kind must ship to lock the vocab/wire enum. Recommendation: include the variant + `taffy` grid mapping, CPU-assert track placement only — no golden image without a consumer. Confirm at review.
- **`UiReadSnapshot` shape.** Scoped to carry static laid-out output, no typed screen/slot handle (that is C's content contract). If de-risking C by pre-baking a typed tree/screen handle is wanted, decide at promotion.
- **`padding` scalar vs. per-edge.** Research shows a single `padding` value; `taffy` supports per-edge `Rect`. Recommendation: scalar in B, widen later if a screen needs asymmetric padding — low-cost additive change. Confirm.
