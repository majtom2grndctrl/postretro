# Research — M13 Goal B (Descriptor model + retained tree + layout)

Investigation notes behind `index.md`. Decisions live in the spec; this is the grounding.

## Roadmap placement

Goal B is the second spec of Milestone 13 (UI). Roadmap line: "B — Descriptor model + retained tree + layout. serde descriptor structs ↔ Rust enum variants, Rust-owned retained tree, taffy layout, anchor + offset, dirty-tree relayout, core widget vocab (text / panel / image / vstack / hstack / grid / spacer). Built in Rust and tests, no script ingestion yet. Locks (pulled forward): the descriptor wire format … and the discriminated-union-per-kind decision … Integrates the measure seam against glyphon shaped-text metrics. Depends on A."

Adjacent goals (boundaries B must respect):
- **A (done)** — `M13--ui-render-pass-slice`. Shipped the UI pass, instanced 9-slice quad pipeline, glyphon text, splash reimplementation, the `input/ui_dispatch.rs` seam, `UiReadSnapshot` read handle, CPU layout-assertion harness.
- **C** — state system (`defineState`, `StateValue<T>`, slot table, value-diffing). Owns bindings and slot-driven invalidation. B carries no bindings.
- **D** — theming / fonts. **E** — `styleRanges` / reactions. **F** — input breadth (focus, nav, modal, gamepad). **G1** — SDK + script ingestion. **SE** — screen-space effects.

## Why the open questions resolved to the chosen defaults

One test sorts them: *does the decision shape a contract expensive to reverse, or is it owned by a later goal?* Expensive-to-reverse → decide to the endpoint now (anchor reuse, complete the vocab enum incl. grid, per-tree multiplicity, serde-tagged wire). Owned-by-later-goal → narrowest real thing, defer the contract (static snapshot → C, raw colors → D, grid golden deferred to first consumer).

## Wire-format / discriminated-union grounding

Repo precedent (`scripting/data_descriptors.rs`): `ReactionDescriptor` (`Progress`/`Primitive`/`Sequence`) is discriminated by **manual key-presence**, not a tag field — `obj.contains_key("progress"/"primitive"/"sequence")` (JS path ~lines 419–437) duplicated as `table.contains_key(...)` (Luau path ~lines 1058–1078). Both bridge through `conv.rs` (`js_to_json` ~L768, `lua_to_json` ~L840) → `serde_json::Value` → `serde_json::from_value`.

Decision for UI descriptors: adopt serde **internally-tagged** (`#[serde(tag = "kind")]`) instead. The research already prescribes a `{ kind: "text" }` shape, which *is* internally-tagged; using serde's own dispatch removes the third hand-rolled discriminator and is less code. Constraints (serde docs, https://serde.rs/enum-representations.html): internally-tagged forbids tuple variants and buffers into `Value` on deserialize — both already true/acceptable here (struct variants, existing `Value` round-trip). Untagged was rejected: poor error messages and ambiguous matches, bad for a modder-facing wire format. Reactions keep their legacy manual pattern; only descriptors diverge.

## taffy (web research)

- Latest **0.10.x** (crates.io / DioxusLabs/taffy CHANGELOG). Measure API stable since 0.4's redesign: a global measure function plus per-node user context. Signature shape: closure `(known_dimensions, available_space, node_id, node_context, style) -> Size<f32>`, driven by `compute_layout_with_measure` (DioxusLabs/taffy `examples/measure.rs`).
- Grid is production-grade (named lines/areas matured through 0.9/0.10). Used by Bevy UI, Zed/GPUI, Blitz/Dioxus — the standard Rust retained-UI layout crate.
- Text integration pattern: carry a shaped-text handle in the node context, return its measured size from the closure. This is exactly the glyphon measure-seam B needs; approach confirmed sound.
- Absent from the workspace today — B adds it. MSRV well under the repo's toolchain.

## glyphon / wgpu pairing

- Workspace pins `glyphon = "0.11"`, `wgpu = "29"`. glyphon 0.11 targets wgpu 29 (glyphon Cargo.toml). A's open question about glyphon-vs-wgpu-29 compatibility is **resolved** — no version risk for B.
- Measure seam attaches at `UiPass::shape_text(&mut self, texts: &[UiText], viewport: [u32; 2]) -> Vec<TextBuffer>` (`render/ui/mod.rs` ~L669), which today shapes text but feeds no size back to any layout. After Task 1 this lives in `render/ui/text.rs`.

## Codebase seams (verified against source)

- `render/ui/mod.rs`: `UiPass` (quad pipeline + glyphon members), `UiInstance` (`rect`/`uv_rect`/`color`/`margin`, all `[f32;4]`, 64 B, `Pod`), `UiDrawList { instances: Vec<UiInstance> }`, `UiReadSnapshot { version_line: String }`, `UiText { content, position:[f32;2], font_size, color:[u8;4] }`, `UiBatch<'a> { list, bind_group }`. `UiPass::encode(device, queue, encoder, view, viewport:[u32;2], load, batches:&[UiBatch], texts:&[UiText])`.
- `render/ui/layout.rs`: `Anchor` (nine variants, enum-level `#[allow(dead_code)]`), `UiElement { anchor, offset:[f32;2], size:[f32;2], uv_rect:[f32;4], color:[f32;4], margin:[f32;4] }`, `REFERENCE_WIDTH/HEIGHT = 1280/720`, `device_scale([u32;2])->f32`, `project(&[UiElement],[u32;2])->UiDrawList`, private `canvas_origin`/`project_element`.
- `render/ui/splash.rs`: `SplashDescriptor { border, fill, logo: UiElement, capture_mode }`, `build_splash_descriptor(logo_aspect: f32) -> SplashDescriptor` (**the named A→B seam**), methods `panel_elements()->[UiElement;2]`, `logo_element()`, `background_element(color)`, `text_line(content, device_size, scale)->UiText`.
- `render/mod.rs`: `record_splash_ui(&mut self, encoder, view, viewport)` assembles draw lists by hand — projects background + `panel_elements` into a panel `UiDrawList`, projects `logo_element` into a logo list, pushes `text_line` if the snapshot version line is non-empty, then `ui.encode(... LoadOp::Clear(BLACK), &batches, &texts)`. Renderer UI fields: `ui`, `active_splash: Option<SplashDescriptor>`, `splash_logo: Option<SplashLogo>`, `ui_snapshot`. Methods: `install_splash_from_loaded`, `splash_capture_mode`, `set_ui_snapshot`, `render_splash_frame`, `clear_splash`.
- Pass order: UI pass runs after world/fog/debug, before timing-resolve/submit. Renderer owns passes as plain struct fields. Renderer-owns-GPU invariant holds — all wgpu stays in the renderer module.

## A follow-ups folded into B

From `context/plans/done/M13--ui-render-pass-slice/follow-ups.md`:
1. Split `render/ui/mod.rs` (~800 lines) → extract glyphon into `render/ui/text.rs`. → **Task 1.**
2. `TextAtlas::trim()` once per frame (atlas grows unbounded once varied text renders). → **Task 1** (text path).
3. Empty-batch encode early-out (gameplay path records an empty UI pass today to hold frame-order position). → **Task 5.**
4. Splash version-text centering via measured run width (currently glyph-count estimate). → **Task 5** (depends on the measure seam, Task 4).

## Research-doc section drift

`context/research/ui-layer.md` headings don't line up with the roadmap's parenthetical section refs. Actual: §5 = Crate Dependencies (taffy listed: "Flexbox/grid/block math on a descriptor tree. No rendering, no input."), §6 = Widget Vocabulary, §7 = Layout Model, §9 = State API, §13 = Rendering Pipeline Placement ("Layout (taffy) — runs on dirty trees only"), §15 = Modder SDK Shape. There is **no** dedicated retained-tree section and **no** "Boundary Inventory" table in the doc — the retained-tree intent is stated in the preamble/§2/§3 ("Declare, don't drive. Scripts emit widget descriptors at load time. Rust owns the live tree."); the Boundary inventory is a B deliverable, not lifted from the doc. The doc does not enumerate per-widget taffy `Style` mappings or a text measure-function contract — those are B's to define.

Widget fields the doc does name: `text` (`content`, optional `format`, `bind`), `panel` (background, optional 9-slice), `image` (asset key), `vstack`/`hstack` (`gap`, `align`, `padding`), `grid` (`cols`, `focus`), `spacer` (flex). `bar`/`button`/`input`/`list`/`viewport`/`slider` are named in the doc but deferred to C/F (need slots or events).
