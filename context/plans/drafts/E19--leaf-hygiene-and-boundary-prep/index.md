# Leaf Hygiene & Boundary Prep

> Epic: `E19--render-stack-decomposition`. Small behavior-preserving refactors that unblock `E19--visibility` and `E19--ui`. No crate boundaries yet.

## Goal

Remove dead code and break two in-place couplings so later crate cuts are clean module moves, not refactors-under-transplant.

## Scope

### In scope
- Delete the verified-dead duplicate files `render/ui/descriptor/{widgets,values,focus,accessibility,envelope}.rs` — five orphans, no `mod` declaration anywhere in the crate, each a stale copy of `crates/scripting-core/src/ui/descriptor/*`. The live surface is `render/ui/descriptor/mod.rs` (`pub use postretro_scripting_core::ui::descriptor::*` at `:5`). `envelope.rs` holds the stale twin of `CaptureMode`/`AnchoredTree` and is dead by the same criteria — it sits in the same directory and goes with the other four.
- Widen `Frustum`, `FrustumPlane`, and `extract_frustum_planes` (`crates/postretro/src/visibility.rs` — `FrustumPlane` at `:117`, `Frustum` at `:128`, `extract_frustum_planes` at `:171`; the file is at the crate root, not under `render/`) from `pub(crate)` to `pub` (needed across the `E19--visibility` crate boundary). `extract_frustum_planes` is the `Frustum` constructor called binary-side at `main.rs:2164`; after the cut that becomes a cross-crate call, so widening the struct without its constructor would leave `E19--visibility` doing a `pub(crate)→pub` edit mid-transplant — the exact refactor-under-transplant this spec exists to absorb. The other `pub(crate)` items (`NEAR_PLANE_INDEX`, `slide_near_plane_to`, `is_aabb_outside_frustum`) are consumed only by `portal_vis.rs`, which co-moves into the crate, so they stay intra-crate and are intentionally left `pub(crate)`.
- Invert the `UiCaptureMode` dependency so `render::ui` stops importing `crate::input`. Today `UiTreeEntry.capture_mode` (`render/ui/mod.rs:264`) is `crate::input::UiCaptureMode`, populated via `.into()` in `modal_stack.rs:199`,`:489`, and `top_capture_mode()` (`modal_stack.rs:435`) returns `UiCaptureMode` (also via `.into()`); the `From<descriptor::CaptureMode> for input::UiCaptureMode` impl lives at `render/ui/mod.rs:273`. After: `UiTreeEntry.capture_mode` and `top_capture_mode()` carry/return `scripting-core`'s `postretro_scripting_core::ui::descriptor::CaptureMode` directly (drop the `.into()`s and the `use crate::input::UiCaptureMode` at `modal_stack.rs:16`); the `From` impl moves to the binary/input layer beside `UiCaptureMode` (`input/ui_dispatch.rs`). `input::UiCaptureMode` itself stays in `input/ui_dispatch.rs`. `UiReadSnapshot` (`render/ui/mod.rs:297`) carries the mode only transitively, via `trees: Vec<UiTreeEntry>` — it has no `capture_mode` field of its own.

### Out of scope
- Moving any file into a new crate (that begins at `E19--render-data`).
- Changing capture/passthrough dispatch behavior.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] The five dead descriptor files (`widgets,values,focus,accessibility,envelope`) are gone; `cargo build --workspace` + `cargo test --workspace` green (proves they were unreferenced).
- [ ] `Frustum`, `FrustumPlane`, and `extract_frustum_planes` are `pub` and the build is green. Completeness is a **review gate, not an emptiness check**: `rg "visibility::" crates/postretro/src --glob '!visibility.rs' --glob '!portal_vis.rs'` returns ~40 hits — confirm by inspection that each resolves to an already-`pub` item (`VisibleCells`/`determine_visible_cells`/etc.), so no other `pub(crate)` item is reached binary-side. No other visibility change — `portal_vis.rs`-only items stay `pub(crate)`.
- [ ] `rg "UiCaptureMode" crates/postretro/src/render/ui` returns nothing (catches both the `use` imports and the full-path references the narrower `use crate::input` grep would miss); `UiTreeEntry.capture_mode` and `top_capture_mode()` carry/return `descriptor::CaptureMode`; the `From` conversion lives binary-side. The binary-side partition (Task 3 step 4) is forced by the `top_capture_mode()` return-type change and verified by `cargo build --workspace`; the variant isomorphism keeps any mis-placed `.into()` behavior-neutral. The grep clears only once **both** Task 1 (deletes `descriptor/envelope.rs`, whose `:18` doc comment names `UiCaptureMode`) and Task 3 land — verify it epic-globally after the wave merges, not per-task.
- [ ] The typedef drift test (`crates/postretro/src/scripting/typedef/tests/committed.rs`, fn `committed_sdk_types_match_current_registry`) is byte-identical; UI capture/passthrough behavior unchanged (existing UI dispatch tests pass). `descriptor::CaptureMode` and `input::UiCaptureMode` have identical variants (`Capture`/`Passthrough`, `Passthrough` default), so the relocated `From` stays total and lossless and the change is below the SDK surface the typedef emits.

## Tasks

### Task 1: Delete dead descriptor duplicates
Remove the five orphaned files **under `crates/postretro/src/render/ui/descriptor/`** — `widgets.rs`, `values.rs`, `focus.rs`, `accessibility.rs`, `envelope.rs`. Delete only these: the bare stems collide with live modules elsewhere (e.g. `input/focus.rs`, `render/ui/tree/tests/focus.rs`) that must NOT be touched. Confirm the build proves them dead: none is `mod`-declared, and `rg` each path before deletion to confirm no `mod`/`include!`/path reference survives anywhere (a build alone would not catch an uncompiled non-`mod` file).

### Task 2: Widen frustum boundary surface
`pub(crate)` → `pub` on `Frustum`, `FrustumPlane`, and `extract_frustum_planes` (`:117`,`:128`,`:171`) in `crates/postretro/src/visibility.rs` — the three items reached from outside `visibility.rs`+`portal_vis.rs` (the constructor `extract_frustum_planes` is called at `main.rs:2164`). Leave `NEAR_PLANE_INDEX`/`slide_near_plane_to`/`is_aabb_outside_frustum` `pub(crate)` (only `portal_vis.rs` uses them, and it co-moves). No cross-crate consumer arrives until `E19--visibility`; the workspace does not deny `unreachable_pub`, so the as-yet-unused `pub` compiles clean — no `#[allow]` needed.

### Task 3: UiCaptureMode inversion
1. `UiTreeEntry.capture_mode` (`render/ui/mod.rs:264`) → `descriptor::CaptureMode`; drop the `.into()` at `modal_stack.rs:199`,`:489`.
2. `top_capture_mode()` (`modal_stack.rs:435`) returns `descriptor::CaptureMode` — drop the `.into()` and change the fallback `UiCaptureMode::Passthrough` → `descriptor::CaptureMode::Passthrough`; drop `use crate::input::UiCaptureMode` at `modal_stack.rs:16`. Update the `render/ui` tests that name `UiCaptureMode` (`modal_stack.rs` unit tests, `demo.rs:223+`, `gameplay_ui_gate_test.rs:205`) to `descriptor::CaptureMode` — required for the AC-3 grep to come back empty.
3. Move the `From<descriptor::CaptureMode> for input::UiCaptureMode` impl from `render/ui/mod.rs` to `input/ui_dispatch.rs` (beside `UiCaptureMode`). Move the impl **and its doc comment** (`mod.rs:270`–`:280`) — leaving the doc behind keeps a `UiCaptureMode` token at `:271` and AC-3's grep fails. In `ui_dispatch.rs`, import the descriptor enum straight from the data floor — `use postretro_scripting_core::ui::descriptor::CaptureMode;` — NOT via `crate::render::ui::descriptor`, which would create an `input → render::ui` edge that defeats the inversion.
4. Binary side: every `top_capture_mode()` consumer flips its comparison enum to `descriptor::CaptureMode::Capture` — `main.rs:753`,`:3052`,`:4463`,`:4902` (plus the in-binary tests at `:4911`,`:5586`) and `startup/lifecycle.rs:1892`,`:1940`,`:2056`. The single `→ input::UiCaptureMode` conversion runs only where the mode feeds dispatch (`main.rs:4454`, `ui_dispatch.set_mode`). The return-type change makes the compiler enforce this completeness — a missed site fails to build. The `ui_dispatch.mode()` comparison sites (`main.rs:1220`,`:1421`,`:1477`,`:3167`) read the dispatcher's own stored mode and stay input-space — unchanged.

## Sequencing
**Phase 1:** Tasks 1–3 are independent; fan out. Task 3's `descriptor::CaptureMode` resolves through the surviving `render/ui/descriptor/mod.rs` glob (`postretro_scripting_core::ui::descriptor::CaptureMode`), not the dead `envelope.rs` copy Task 1 deletes — so the two don't collide. Milestone 1, alongside `E19--baseline-and-cargo-config`.
