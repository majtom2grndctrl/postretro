# s6 — postretro-ui

> Epic: `render-stack-decomposition`. Supersedes `compile-time-reduction` Task 6 (the CPU UI-model crate) — this is the full CPU UI subtree, not just descriptor/layout/style.

## Goal

Extract the wgpu-free UI subtree into a CPU-only crate so the largest inbound consumer surface (menus, HUD, focus, scripted presentation cells) depends on pure UI data/logic, and editing UI code does not recompile the renderer or VMs.

## Scope

### In scope
- `postretro-ui`: move the CPU `render::ui` subtree —
  - descriptor surface (already `pub use postretro_scripting_core::ui::descriptor::*` — keep the dependency, drop the dead local files per `s1`),
  - `modal_stack.rs`, `layout.rs`, `theme`/`style_ranges`,
  - `tree/*` (`ui_tree`, `build`, `bindings`, `ui_tree_collect`, `widget_meta`, `style`, `ui_tree_focus`, `node_context`, `draw`, and the tree tests),
  - `tree_asset.rs`, `keyboard_asset.rs`, `demo.rs` (menu name constants + `build_frontend_menu_descriptor`),
  - the CPU text helpers (`build_font_system`, `measure_run`, `font_family_is_registered`, `read_font_file`),
  - the CPU output/wire types currently co-located in `ui/mod.rs`: `UiInstance`, `UiDrawList`, `UiDrawData`, `UiUniform`, `UiText`, `FocusRect`, `FocusRectList`, `FocusGroup`, `FocusNeighbors`, `NodeInteraction`, `UiReadSnapshot` (carrying `descriptor::CaptureMode` per `s1`),
  - `UiTexture` (from `ui_texture.rs`).
- Hoist `UiInstance`/`UiDrawList`/`UiUniform`/`UiText` **out** of `ui/mod.rs` (today co-located with the GPU pass) into the CPU crate.
- Depend on `postretro-scripting-core` (descriptor model), `taffy`, `glyphon` (`FontSystem` for CPU measurement only), `cosmic-text`, `glam`. Depend on `postretro-entities` only if tree bindings reference entity handles (confirm at implementation). **No `crate::input`, no wgpu.**
- Update consumers — `main.rs`, `startup/lifecycle.rs`, `session/mod.rs`, `input/ui_focus.rs`, `scripting/systems/presentation_cells.rs`, `scripting/typedef/tests/surface.rs` — to import from `postretro-ui`.

### Out of scope
- The GPU UI pass: `ui/mod.rs` `UiPass` (pipeline/BGL/sampler/buffers/`upload_ui_texture`/`UiImageRegistry`) and `ui/text.rs` `UiTextRenderer` (glyphon GPU atlas/renderer/viewport). These stay in `postretro-renderer` (`s8`), depending on `postretro-ui`.
- The GPU upload of `UiTexture` (renderer-side, unchanged pattern).

## Acceptance criteria
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; UI tree/focus/layout/theming/golden tests pass from their relocated home.
- [ ] `cargo tree -p postretro-ui` shows no `wgpu`/`winit`/`kira`; `glyphon` appears only for `FontSystem`; no `mlua`/`rquickjs` beyond what `scripting-core` pulls.
- [ ] `UiPass` + `UiTextRenderer` remain in the renderer crate and compile against `postretro-ui`.
- [ ] No `postretro-ui` → `crate::input` edge; `UiReadSnapshot` carries `descriptor::CaptureMode`.
- [ ] The typedef drift test stays byte-identical.

## Tasks

### Task 1: Hoist CPU output types out of the GPU pass
Move `UiInstance`/`UiDrawList`/`UiUniform`/`UiText` out of `ui/mod.rs` into a CPU module, leaving `UiPass`/`UiTextRenderer` referencing them.

### Task 2: Extract postretro-ui
Create the crate, move the CPU subtree + `UiTexture`, wire deps, update all consumers.

### Task 3: Resolve the FontSystem ownership seam
glyphon's CPU `FontSystem` is co-located with its GPU atlas in `UiTextRenderer`, and the retained gameplay-tree measure closure needs it. Decide ownership: `postretro-ui` owns the `FontSystem` and the renderer's `UiTextRenderer` borrows it, or a thin renderer-side shim. The CPU `measure_run`/`build_font_system` path must stay in `postretro-ui`.

## Sequencing
**Phase 1:** Task 1, then Task 2, then Task 3. Needs `s1` (UiCaptureMode inversion). Independent of `s2`–`s5`. Milestone 2.
