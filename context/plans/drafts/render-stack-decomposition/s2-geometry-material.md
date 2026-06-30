# s2 — postretro-geometry + postretro-material

> Epic: `render-stack-decomposition`. The CPU leaf data crates under the level loader. (Open question: one combined `postretro-render-data` crate vs. two.)

## Goal

Extract the two small, dependency-free CPU type modules into workspace crates so the loader, visibility, render-cpu, and renderer crates depend on shared leaf data instead of binary-internal modules.

## Scope

### In scope
- `postretro-geometry`: move `geometry.rs` — `WorldVertex`, `BvhNode`, `BvhLeaf`, `BvhTree`, `BucketRange`, `BVH_NODE_FLAG_LEAF`, `BvhTree::derive_bucket_ranges`. Zero internal deps; `glam`/`bytemuck`/`serde` only.
- `postretro-material`: move `material.rs` — `Material`, `MaterialProperties`, `Material::{shininess,properties}`, `parse_prefix`, `derive_material`, `lookup_material`. Zero internal deps.
- Update importers (geometry: ~10 files; material: ~6) to the crate paths. Optional transitional re-export from the old module paths.
- Workspace wiring per `scripting.md §12` conventions (naming `postretro-<role>`, `[workspace.package]` inheritance, workspace deps).

### Out of scope
- Any wgpu/GPU code (neither module has any).
- Behavior or layout change to `WorldVertex` / `Material` (byte layouts are shared with shaders/PRL — keep stable).

## Acceptance criteria
- [ ] Both crates are workspace members; `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] `cargo tree` for each shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] All importers compile against the crate paths; `Material` derivation and any `BvhTree` tests pass from their relocated homes.
- [ ] `WorldVertex`/`BvhNode`/`BvhLeaf` byte layouts unchanged (no PRL/shader drift).

## Tasks

### Task 1: Extract postretro-geometry
New crate, move `geometry.rs`, widen any `pub(crate)` symbols crossing the boundary to `pub`, update importers.

### Task 2: Extract postretro-material
Same for `material.rs`.

## Sequencing
**Phase 1:** Tasks 1–2 independent; fan out. Milestone 1. Blocks `s3`/`s4`/`s7`/`s8`.
