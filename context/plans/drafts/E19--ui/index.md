# postretro-ui

> Epic: `E19--render-stack-decomposition`. Supersedes `compile-time-reduction` Task 6 (the CPU UI-model crate) — this is the full CPU UI subtree, not just descriptor/layout/style.

## Goal

Extract the wgpu-free UI subtree into a CPU-only crate so the largest inbound consumer surface (menus, HUD, focus, scripted presentation cells) depends on pure UI data/logic, and editing UI code does not recompile the renderer or VMs.

## Scope

### In scope
- `postretro-ui`: move the CPU `render::ui` subtree —
  - descriptor surface (already `pub use postretro_scripting_core::ui::descriptor::*` — keep the dependency; `descriptor/mod.rs` now holds only that re-export shim plus its wire-format round-trip tests, the dead local files having already been dropped per `E19--leaf-hygiene-and-boundary-prep` — preserve the shim and its tests on the move),
  - `modal_stack.rs`, `layout.rs`, `theme`/`style_ranges`, `actions.rs` (pure-CPU reserved action constants, e.g. `COMMIT_TEXT_ENTRY_ACTION`),
  - `tree/*` (`ui_tree`, `build`, `bindings`, `ui_tree_collect`, `widget_meta`, `style`, `ui_tree_focus`, `node_context`, `draw`, `predicate`, and the `tree/tests/*` suite),
  - `tree_asset.rs`, `keyboard_asset.rs`, `demo.rs` (menu name constants + `build_frontend_menu_descriptor`),
  - the CPU text helpers (`build_font_system`, `measure_run`, `font_family_is_registered`, `read_font_file`),
  - the CPU output/wire types: `UiInstance`, `UiDrawList`, `UiUniform`, `UiReadSnapshot`, `UiTreeEntry` (co-located in `ui/mod.rs` with the GPU pass; `UiTreeEntry` carries `capture_mode: descriptor::CaptureMode` per `E19--leaf-hygiene-and-boundary-prep`), and `UiText` (co-located in `ui/text.rs` with `UiTextRenderer`) — `UiDrawData`/`FocusRect`/`FocusRectList`/`FocusGroup`/`FocusNeighbors`/`NodeInteraction` already live in `tree/draw.rs` and move with `tree/*` above,
  - the CPU-only gate/lifecycle test files: `lifecycle_render_test.rs`, `demo_ui_gate_test.rs`, `gameplay_ui_gate_test.rs`, `theme_gate_test.rs` — all four reproduce a renderer decision headless with no GPU adapter and no `wgpu` call (verified: zero `wgpu::` references in any of the four),
  - `UiTexture` (from `ui_texture.rs`) — lands here. Neither `postretro-ui` nor `postretro-renderer` exists yet, but the splash path already consumes `UiTexture` today; per the epic's Decision 4 the future renderer crate will depend on `postretro-ui` regardless, so a separate 12-line crate buys nothing.
- Hoist `UiInstance`/`UiDrawList`/`UiUniform`/`UiTreeEntry` **out** of `ui/mod.rs` (today co-located with the GPU pass), and `UiText` **out** of `ui/text.rs` (today co-located with `UiTextRenderer`), into the CPU crate.
- Depend on `postretro-scripting-core` (descriptor model), `postretro-entities` (unconditional — `SlotValue`, imported across `tree/*` and the gate tests), `taffy`, `glyphon` (`FontSystem` for CPU measurement only), `cosmic-text`, `serde`, `serde_json`, `log`. Dev-dep on `postretro-foundation` for `ModThemeTokens` (`lifecycle_render_test.rs`). **No `crate::input`, no wgpu, no `glam`** (zero usage anywhere in `render/ui/`).
- Update consumers — `main.rs`, `startup/lifecycle.rs`, `startup/splash_lifecycle.rs` (`render::ui::modal_stack::ScopeTier`), `session/mod.rs`, `input/ui_focus.rs`, `scripting/systems/presentation_cells.rs`, `scripting/typedef/tests/surface.rs` — to import from `postretro-ui`.
- Delete the dead generator-bin shims `render/ui/_gen_layout_shim.rs` and `render/ui/_gen_tree_shim.rs` — no `mod`/`#[path]` reference to either exists anywhere in the repo (confirmed by repo-wide grep); their own header comments claiming inclusion by `src/bin/gen_script_types.rs` are stale — that bin only pulls in `scripting::{entity_world_primitives, primitives, state_store}`.

### Out of scope
- The GPU UI pass: `ui/mod.rs` `UiPass` (pipeline/BGL/sampler/buffers/`upload_ui_texture`/`UiImageRegistry`) and `ui/text.rs` `UiTextRenderer` (glyphon GPU atlas/renderer/viewport). These stay in `postretro-renderer` (`E19--renderer-gpu`), depending on `postretro-ui`.
- The GPU upload of `UiTexture` (renderer-side, unchanged pattern).
- The GPU-coupled UI tests: `gpu_test_harness.rs`, `multi_batch_test.rs`, `multi_layer_text_golden_test.rs` — these drive `UiPass` against a headless wgpu adapter and stay in `postretro-renderer` alongside it; they do not relocate.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; UI tree/focus/layout/theming/gate tests pass from their relocated home in `postretro-ui`. The GPU-coupled golden/harness tests (`gpu_test_harness.rs`, `multi_batch_test.rs`, `multi_layer_text_golden_test.rs`) do not relocate — they continue to pass unchanged in `postretro-renderer`.
- [ ] `cargo tree -p postretro-ui` shows no `wgpu`/`winit`/`kira`; `glyphon` appears only for `FontSystem`; no `mlua`/`rquickjs` beyond what `scripting-core` pulls.
- [ ] `UiPass` + `UiTextRenderer` remain in the renderer crate and compile against `postretro-ui`.
- [ ] No `postretro-ui` → `crate::input` edge; `UiReadSnapshot` carries `descriptor::CaptureMode`.
- [ ] The typedef drift test stays byte-identical.

## Tasks

### Task 1: Hoist CPU output types out of the GPU pass
Move `UiInstance`/`UiDrawList`/`UiUniform`/`UiTreeEntry` out of `ui/mod.rs`, and `UiText` out of `ui/text.rs`, into a CPU module, leaving `UiPass`/`UiTextRenderer` referencing them.

### Task 2: Extract postretro-ui
Create the crate, move the CPU subtree + `UiTexture`, wire deps, update all consumers.

### Task 3: Resolve the FontSystem ownership seam
glyphon's CPU `FontSystem` is co-located with its GPU atlas in `UiTextRenderer`, and the retained gameplay-tree measure closure needs it. Decide ownership: `postretro-ui` owns the `FontSystem` and the renderer's `UiTextRenderer` borrows it, or a thin renderer-side shim. The CPU `measure_run`/`build_font_system` path must stay in `postretro-ui`.

## Sequencing
**Phase 1:** Task 1, then Task 2, then Task 3. Needs `E19--leaf-hygiene-and-boundary-prep` (UiCaptureMode inversion) — done (`context/plans/done/`). Independent of the other Milestone 1–2 CPU specs — `E19--render-data`, `E19--level-loader`, `E19--lighting-cpu` are done; `E19--visibility` is in progress (`context/plans/in-progress/`). Milestone 2.
