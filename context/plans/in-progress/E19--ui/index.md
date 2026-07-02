# postretro-ui

> Epic: `E19--render-stack-decomposition`. Supersedes `compile-time-reduction` Task 6 (the CPU UI-model crate) — this is the full CPU UI subtree, not just descriptor/layout/style.

## Goal

Extract the wgpu-free UI subtree into a CPU-only crate so the largest inbound consumer surface (menus, HUD, focus, scripted presentation cells) depends on pure UI data/logic, and editing UI code recompiles a small leaf crate rather than the whole `postretro` monolith.

## Scope

### In scope
- `postretro-ui`: move the CPU `render::ui` subtree —
  - descriptor surface (already `pub use postretro_scripting_core::ui::descriptor::*` — keep the dependency; `descriptor/mod.rs` now holds only that re-export shim plus its wire-format round-trip tests, the dead local files having already been dropped per `E19--leaf-hygiene-and-boundary-prep` — preserve the shim and its tests on the move),
  - `modal_stack.rs`, `layout.rs`, `theme`/`style_ranges`, `actions.rs` (pure-CPU reserved action constants, e.g. `COMMIT_TEXT_ENTRY_ACTION`),
  - `tree/*` (`ui_tree`, `build`, `bindings`, `ui_tree_collect`, `widget_meta`, `style`, `ui_tree_focus`, `node_context`, `draw`, `predicate`, and the `tree/tests/*` suite),
  - `tree_asset.rs`, `keyboard_asset.rs`, `demo.rs` (menu name constants + `build_frontend_menu_descriptor`) — their `#[cfg(test)]` `CARGO_MANIFEST_DIR` path anchors keep the same `../..` workspace-root depth, since `postretro-ui` is a sibling crate under `crates/`,
  - the CPU text helpers (`build_font_system`, `measure_run`, `font_family_is_registered`, `read_font_file`),
  - the CPU output/wire types: `UiInstance`, `UiDrawList`, `UiUniform`, `UiReadSnapshot`, `UiTreeEntry` (co-located in `ui/mod.rs` with the GPU pass; `UiTreeEntry` carries `capture_mode: descriptor::CaptureMode` per `E19--leaf-hygiene-and-boundary-prep`), and `UiText` (co-located in `ui/text.rs` with `UiTextRenderer`) — `UiDrawData`/`FocusRect`/`FocusRectList`/`FocusGroup`/`FocusNeighbors`/`NodeInteraction` already live in `tree/draw.rs` and move with `tree/*` above,
  - the CPU-only gate test files: `demo_ui_gate_test.rs`, `gameplay_ui_gate_test.rs`, `theme_gate_test.rs` — all three reproduce a renderer decision headless with no GPU adapter and no `wgpu` call (verified: zero `wgpu::` references in any of the three),
  - `UiTexture` (from the crate-root `ui_texture.rs` — `crate::ui_texture`, a `pub struct`, not under `render/ui/`) — lands here. Neither `postretro-ui` nor `postretro-renderer` exists yet, but the splash path already consumes `UiTexture` today; per the epic's Decision 4 the future renderer crate will depend on `postretro-ui` regardless, so a separate 12-line crate buys nothing.
- Hoist `UiInstance`/`UiDrawList`/`UiUniform`/`UiReadSnapshot`/`UiTreeEntry` **out** of `ui/mod.rs` (today co-located with the GPU pass), and `UiText` **out** of `ui/text.rs` (today co-located with `UiTextRenderer`), into the CPU crate.
- Depend on `postretro-scripting-core` (descriptor model), `postretro-entities` (unconditional — `SlotValue`, imported across `tree/*` and the gate tests), `taffy`, `cosmic-text` (direct, for `FontSystem`/measurement — pinned to `0.18.2` (glyphon's), so the `FontSystem` type identity holds across the ownership seam with `UiTextRenderer`), `bytemuck` (`UiInstance`/`UiUniform` derive `Pod`/`Zeroable`), `serde`, `serde_json`, `log`. **No `crate::input`, no wgpu, no `glyphon`, and no direct `glam` dependency or source use in `postretro-ui`**. Transitive `glam` through the required `postretro-entities` / `postretro-scripting-core` edge is accepted; splitting that out would be a separate dependency-surface refactor and is not worth expanding this UI extraction.
- Update consumers — `main.rs`, `startup/lifecycle.rs`, `startup/splash_lifecycle.rs` (`render::ui::modal_stack::ScopeTier`), `session/mod.rs`, `input/ui_focus.rs`, `scripting/systems/presentation_cells.rs`, `scripting/typedef/tests/surface.rs` — to import from `postretro-ui`.
- Delete the dead generator-bin shims `render/ui/_gen_layout_shim.rs` and `render/ui/_gen_tree_shim.rs` — no `mod`/`#[path]` reference to either exists anywhere in the repo (confirmed by repo-wide grep); their own header comments claiming inclusion by `src/bin/gen_script_types.rs` are stale — that bin only pulls in `scripting::{entity_world_primitives, primitives, state_store}`.

### Out of scope
- The GPU UI pass: `ui/mod.rs` `UiPass` (pipeline/BGL/sampler/buffers/`upload_ui_texture`/`UiImageRegistry`) and `ui/text.rs` `UiTextRenderer` (glyphon GPU atlas/renderer/viewport). These stay in the renderer crate — `postretro` today, depending on `postretro-ui`; they migrate to `postretro-renderer` only in `E19--renderer-gpu`.
- The GPU upload of `UiTexture` (renderer-side, unchanged pattern).
- The GPU-coupled UI tests: `gpu_test_harness.rs`, `multi_batch_test.rs`, `multi_layer_text_golden_test.rs` — these drive `UiPass` against a headless wgpu adapter and stay in `postretro` alongside it; they do not relocate.
- `lifecycle_render_test.rs` stays in `postretro`: it's an end-to-end lifecycle test driving postretro-only scripting primitives (`scripting::primitives::register_all`, `write_store_slot`), which don't descend into a leaf crate. wgpu-free is necessary but not sufficient for relocation — it stays with the crate that owns those primitives.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; UI tree/focus/layout/theming/gate tests pass from their relocated home in `postretro-ui`. The GPU-coupled golden/harness tests (`gpu_test_harness.rs`, `multi_batch_test.rs`, `multi_layer_text_golden_test.rs`) do not relocate — they continue to pass unchanged in `postretro` — and `lifecycle_render_test.rs` continues to pass in `postretro`.
- [ ] `cargo tree -p postretro-ui` (default features) shows no `wgpu`/`winit`/`glyphon`/`kira`. `mlua`/`rquickjs` are pulled only transitively via `postretro-scripting-core` (accepted — epic Decision 13).
- [ ] `postretro-ui` has no direct `glam` dependency and no `glam` source imports. `cargo tree -p postretro-ui` may still show transitive `glam` via required `postretro-entities` / `postretro-scripting-core`; that is accepted for this spec.
- [ ] `UiPass` + `UiTextRenderer` remain in the renderer crate (`postretro` today) and compile against `postretro-ui`, with `cosmic-text` version-unified across both crates so the `&mut FontSystem` borrow typechecks.
- [ ] No `postretro-ui` → `crate::input` edge; `UiReadSnapshot` carries `descriptor::CaptureMode` transitively, on its `UiTreeEntry.capture_mode` entries (it has no `capture_mode` field of its own).
- [ ] The dead shims `render/ui/_gen_layout_shim.rs` and `render/ui/_gen_tree_shim.rs` no longer exist in-tree.
- [ ] The typedef drift test (`scripting/typedef/tests/committed.rs`) stays byte-identical; the `surface.rs` consumer update above is an import-only source edit and does not change generated output.

## Tasks

### Task 1: Hoist CPU output types out of the GPU pass
Move `UiInstance`/`UiDrawList`/`UiUniform`/`UiReadSnapshot`/`UiTreeEntry` out of `ui/mod.rs`, and `UiText` out of `ui/text.rs`, into a CPU module, leaving `UiPass`/`UiTextRenderer` referencing them.

### Task 2: Extract postretro-ui
Create the crate; move the CPU subtree + `UiTexture`; update all consumers. Dependencies: `postretro-scripting-core`, `postretro-entities`, `taffy`, `cosmic-text` (pinned to `0.18.2`, the version glyphon pulls renderer-side, so `FontSystem` type identity holds at the Task 3 seam), `bytemuck`, `serde`, `serde_json`, `log`. The move rewrites `glyphon::FontSystem` to `cosmic_text::FontSystem` on moved text-touching code — `demo_ui_gate_test.rs`, `theme_gate_test.rs`, `gameplay_ui_gate_test.rs`, `tree/ui_tree.rs`, `tree/widget_meta.rs`, `tree/tests/common.rs`, `tree/tests/local_state.rs` (`lifecycle_render_test.rs` stays in `postretro`, keeps the `glyphon::` path). Beyond the glyphon rewrite, the move re-points intra-subtree `crate::render::ui::…` absolute paths (e.g. `CellValues` in the gate tests) to the new crate root and strips now-stale `glyphon` doc-comments. AC 2's `cargo tree -p postretro-ui` is the move-completeness gate; a missing consumer or unrewritten path surfaces as a `cargo build --workspace` break.

### Task 3: Resolve the FontSystem ownership seam
`FontSystem` (`cosmic_text::FontSystem`, which glyphon merely re-exports) is co-located with the GPU atlas in `UiTextRenderer` today, and the retained gameplay-tree measure closure needs it. `postretro-ui` owns the `FontSystem`, built by `build_font_system` and held on `Session` with the rest of the UI subsystem cluster (`modal_stack`/`ui_focus`/…). The boot splash is font-free by design (`splash_pass` runs no `UiPass`/glyphon), so no font state is needed before `Session` exists. Because the once-per-frame `UiReadSnapshot` is read-only, the `&mut FontSystem` cannot ride it: the renderer's `UiTextRenderer` takes it as an explicit `&mut FontSystem` argument on its text-prepare entry point, alongside the read-only snapshot, to prepare glyphon text. `postretro-ui`'s `cosmic-text` must stay version-unified with the `cosmic-text` glyphon pulls renderer-side, so the `FontSystem` type identity holds across the borrow. The CPU `measure_run`/`build_font_system` path stays in `postretro-ui`. Threading chain: render entry `UiPass::encode` → `UiTextRenderer::shape_text`/`::prepare_text` (`render/ui/mod.rs`, `render/ui/text.rs`); measure path `UiPass::layout_gameplay_tree` → `UiTextRenderer::font_system_mut` → `UiTree::build_draw_data`/`build_draw_data_retained` (`render/ui/tree/ui_tree.rs`) → `widget_meta::measure_node` (`render/ui/tree/widget_meta.rs`); runtime install `UiPass::register_font` → `UiTextRenderer::register_font`. Each renderer-side entry — text-prepare, `layout_gameplay_tree`, and `register_font` — gains a `&mut FontSystem` parameter fed by the `Session` caller, replacing today's `UiTextRenderer`-owned field access (`build_draw_data`/`measure_node` already take it explicitly).

## Sequencing
**Phase 1:** Task 1, then Task 2, then Task 3. Needs `E19--leaf-hygiene-and-boundary-prep` (UiCaptureMode inversion) — done (`context/plans/done/`). Independent of the other Milestone 1–2 CPU specs — `E19--render-data`, `E19--level-loader`, `E19--lighting-cpu` are done; `E19--visibility` is in progress (`context/plans/in-progress/`). Milestone 2.
