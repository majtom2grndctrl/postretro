# Leaf Hygiene & Boundary Prep

> Epic: `E19--render-stack-decomposition`. Small behavior-preserving refactors that unblock `E19--visibility` and `E19--ui`. No crate boundaries yet.

## Goal

Remove dead code and break two in-place couplings so later crate cuts are clean module moves, not refactors-under-transplant.

## Scope

### In scope
- Delete the verified-dead duplicate files `render/ui/descriptor/{widgets,values,focus,accessibility,envelope}.rs` — five orphans, no `mod` declaration anywhere in the crate, each a stale copy of `crates/scripting-core/src/ui/descriptor/*`. The live surface is `render/ui/descriptor/mod.rs` (`pub use postretro_scripting_core::ui::descriptor::*` at `:5`). `envelope.rs` holds the stale twin of `CaptureMode`/`AnchoredTree` and is dead by the same criteria — it sits in the same directory and goes with the other four.
- Widen `Frustum` and `FrustumPlane` (`crates/postretro/src/visibility.rs` — `FrustumPlane` at `:117`, `Frustum` at `:128`, both `pub(crate) struct`; the file is at the crate root, not under `render/`) from `pub(crate)` to `pub` (needed across the `E19--visibility` crate boundary).
- Invert the `UiCaptureMode` dependency so `render::ui` stops importing `crate::input`. Today `UiTreeEntry.capture_mode` (`render/ui/mod.rs:264`) is `crate::input::UiCaptureMode`, populated via `.into()` in `modal_stack.rs:199`,`:489`, and `top_capture_mode()` (`modal_stack.rs:435`) returns `UiCaptureMode` (also via `.into()`); the `From<descriptor::CaptureMode> for input::UiCaptureMode` impl lives at `render/ui/mod.rs:273`. After: `UiTreeEntry.capture_mode` and `top_capture_mode()` carry/return `scripting-core`'s `postretro_scripting_core::ui::descriptor::CaptureMode` directly (drop the `.into()`s and the `use crate::input::UiCaptureMode` at `modal_stack.rs:16`); the `From` impl moves to the binary/input layer beside `UiCaptureMode` (`input/ui_dispatch.rs`). `input::UiCaptureMode` itself stays in `input/ui_dispatch.rs`. `UiReadSnapshot` (`render/ui/mod.rs:297`) carries the mode only transitively, via `trees: Vec<UiTreeEntry>` — it has no `capture_mode` field of its own.

### Out of scope
- Moving any file into a new crate (that begins at `E19--render-data`).
- Changing capture/passthrough dispatch behavior.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] The five dead descriptor files (`widgets,values,focus,accessibility,envelope`) are gone; `cargo build --workspace` + `cargo test --workspace` green (proves they were unreferenced).
- [ ] `Frustum`/`FrustumPlane` are `pub`; no other visibility change.
- [ ] `rg "UiCaptureMode" crates/postretro/src/render/ui` returns nothing (catches both the `use` imports and the full-path references the narrower `use crate::input` grep would miss); `UiTreeEntry.capture_mode` and `top_capture_mode()` carry/return `descriptor::CaptureMode`; the `From` conversion lives binary-side.
- [ ] The typedef drift test (`crates/postretro/src/scripting/typedef/tests/committed.rs`, fn `committed_sdk_types_match_current_registry`) is byte-identical; UI capture/passthrough behavior unchanged (existing UI dispatch tests pass). `descriptor::CaptureMode` and `input::UiCaptureMode` have identical variants (`Capture`/`Passthrough`, `Passthrough` default), so the relocated `From` stays total and lossless and the change is below the SDK surface the typedef emits.

## Tasks

### Task 1: Delete dead descriptor duplicates
Remove the five orphaned files (`widgets,values,focus,accessibility,envelope`); confirm the build proves them dead. None are `mod`-declared; `rg` the five stems before deletion to confirm no `mod`/`include!`/path reference survives anywhere (a build alone would not catch an uncompiled non-`mod` file).

### Task 2: Widen frustum types
`pub(crate)` → `pub` on `Frustum`/`FrustumPlane` in `crates/postretro/src/visibility.rs`. No cross-crate consumer arrives until `E19--visibility`; the workspace does not deny `unreachable_pub`, so the as-yet-unused `pub` compiles clean — no `#[allow]` needed.

### Task 3: UiCaptureMode inversion
1. `UiTreeEntry.capture_mode` (`render/ui/mod.rs:264`) → `descriptor::CaptureMode`; drop the `.into()` at `modal_stack.rs:199`,`:489`.
2. `top_capture_mode()` (`modal_stack.rs:435`) returns `descriptor::CaptureMode` — drop the `.into()` and change the fallback `UiCaptureMode::Passthrough` → `descriptor::CaptureMode::Passthrough`; drop `use crate::input::UiCaptureMode` at `modal_stack.rs:16`. Update the `render/ui` tests that name `UiCaptureMode` (`modal_stack.rs` unit tests, `demo.rs:223+`, `gameplay_ui_gate_test.rs:205`) to `descriptor::CaptureMode` — required for the AC-3 grep to come back empty.
3. Move the `From<descriptor::CaptureMode> for input::UiCaptureMode` impl from `render/ui/mod.rs:273` to `input/ui_dispatch.rs` (beside `UiCaptureMode`).
4. Binary side: the `top_capture_mode()` comparison sites (`main.rs:753`,`:3052`,`:4463`,`:4902`) compare against `descriptor::CaptureMode::Capture`; the single `→ input::UiCaptureMode` conversion runs only where the mode feeds dispatch (`main.rs:4454`, `ui_dispatch.set_mode`). The `ui_dispatch.mode()` comparison sites (`:1220`,`:1421`,`:1477`,`:3167`) read the dispatcher's own stored mode and stay input-space — unchanged.

## Sequencing
**Phase 1:** Tasks 1–3 are independent; fan out. Task 3's `descriptor::CaptureMode` resolves through the surviving `render/ui/descriptor/mod.rs` glob (`postretro_scripting_core::ui::descriptor::CaptureMode`), not the dead `envelope.rs` copy Task 1 deletes — so the two don't collide. Milestone 1, alongside `E19--baseline-and-cargo-config`.
