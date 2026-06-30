# s1 — Leaf Hygiene & Boundary Prep

> Epic: `render-stack-decomposition`. Small behavior-preserving refactors that unblock `s4` (visibility) and `s6` (ui). No crate boundaries yet.

## Goal

Remove dead code and break two in-place couplings so later crate cuts are clean module moves, not refactors-under-transplant.

## Scope

### In scope
- Delete the verified-dead duplicate files `render/ui/descriptor/{widgets,values,focus,accessibility}.rs` (no `mod` declarations; the live surface is `render/ui/descriptor/mod.rs` → `pub use postretro_scripting_core::ui::descriptor::*`).
- Widen `Frustum` and `FrustumPlane` (`visibility.rs:117`,`:128`) from `pub(crate)` to `pub` (needed across the `s4` crate boundary).
- Invert the `UiCaptureMode` dependency so `render::ui` stops importing `crate::input`: `UiReadSnapshot` (`render/ui/mod.rs:264`) carries `scripting-core`'s `descriptor::CaptureMode`; the `From<descriptor::CaptureMode> → input::UiCaptureMode` conversion (`render/ui/mod.rs:273`) moves to the binary/input layer (which depends on both). `input::UiCaptureMode` itself stays in `input/ui_dispatch.rs`.

### Out of scope
- Moving any file into a new crate (that begins at `s2`).
- Changing capture/passthrough dispatch behavior.

## Acceptance criteria
- [ ] The four dead descriptor files are gone; `cargo build --workspace` + `cargo test --workspace` green (proves they were unreferenced).
- [ ] `Frustum`/`FrustumPlane` are `pub`; no other visibility change.
- [ ] `rg "use crate::input" crates/postretro/src/render/ui` returns nothing for `UiCaptureMode`; `UiReadSnapshot` carries `descriptor::CaptureMode`; the conversion lives binary-side.
- [ ] The typedef drift test (`scripting/typedef/tests/committed.rs`) is byte-identical; UI capture/passthrough behavior unchanged (existing UI dispatch tests pass).

## Tasks

### Task 1: Delete dead descriptor duplicates
Remove the four orphaned files; confirm the build proves them dead.

### Task 2: Widen frustum types
`pub(crate)` → `pub` on `Frustum`/`FrustumPlane`.

### Task 3: UiCaptureMode inversion
Change `UiReadSnapshot` to carry `descriptor::CaptureMode`; relocate the `From` conversion to the binary; update the `main.rs` read site to convert there before feeding input dispatch.

## Sequencing
**Phase 1:** Tasks 1–3 are independent; fan out. Milestone 1, alongside `s0`.
