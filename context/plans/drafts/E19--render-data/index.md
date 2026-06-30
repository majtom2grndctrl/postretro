# postretro-render-data

> Epic: `E19--render-stack-decomposition`. The CPU leaf data crate under the level loader — geometry + material types in one crate.

## Goal

Extract the two small, dependency-free CPU type modules into one workspace crate so the loader, visibility, render-cpu, and renderer crates depend on shared leaf data instead of binary-internal modules.

## Scope

### In scope
- `postretro-render-data`: one crate holding both modules.
  - `geometry.rs` — `WorldVertex`, `BvhNode`, `BvhLeaf`, `BvhTree`, `BucketRange`, `BVH_NODE_FLAG_LEAF`, `BvhTree::derive_bucket_ranges`.
  - `material.rs` — `Material`, `MaterialProperties`, `Material::{shininess,properties}`, `parse_prefix`, `derive_material`, `lookup_material`.
  - Zero internal deps; `glam`/`bytemuck`/`serde` only.
- Update importers (geometry: ~10 files; material: ~6) to the crate paths. Optional transitional re-export from the old module paths.
- Workspace wiring per `scripting.md §12` conventions (naming `postretro-<role>`, `[workspace.package]` inheritance, workspace deps).

### Out of scope
- Any wgpu/GPU code (neither module has any).
- Behavior or layout change to `WorldVertex` / `Material` (byte layouts are shared with shaders/PRL — keep stable).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] `cargo tree -p postretro-render-data` shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] All importers compile against the crate paths; `Material` derivation and any `BvhTree` tests pass from their relocated homes.
- [ ] `WorldVertex`/`BvhNode`/`BvhLeaf` byte layouts unchanged (no PRL/shader drift).

## Tasks

### Task 1: Extract postretro-render-data
New crate. Move `geometry.rs` and `material.rs` into it as two modules. Widen any `pub(crate)` symbols crossing the boundary to `pub`, update importers.

## Decision

**One crate, not two** (was: geometry vs. material as separate crates). Principle: lean — two dependency-free leaf modules gain no recompile isolation from splitting; one crate is fewer workspace members and one dependency edge for every consumer.

## Sequencing
**Phase 1:** Task 1. Milestone 1. Blocks `E19--level-loader`, `E19--visibility`, `E19--render-cpu`, and `E19--renderer-gpu`.
