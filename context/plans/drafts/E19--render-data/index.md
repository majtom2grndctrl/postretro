# postretro-render-data

> Epic: `E19--render-stack-decomposition`. The CPU leaf data crate under the level loader — geometry, material, and shared frustum/AABB math in one crate.

## Goal

Extract the small, dependency-free CPU type modules into one workspace crate so the loader, visibility, render-cpu, model, lighting, and renderer crates depend on shared leaf data and the shared geometry/frustum math instead of binary-internal modules. This is the universal lower leaf: both the CPU cone path and the GPU cull pipelines (in `postretro-renderer`) depend *down* on it for the shared frustum-plane row-math, so neither needs to reach across into the other.

## Scope

### In scope
- `postretro-render-data`: one crate holding the leaf modules.
  - `geometry.rs` — `WorldVertex`, `BvhNode`, `BvhLeaf`, `BvhTree`, `BucketRange`, `BVH_NODE_FLAG_LEAF`, `BvhTree::derive_bucket_ranges`.
  - `material.rs` — `Material`, `MaterialProperties`, `Material::{shininess,properties}`, `parse_prefix`, `derive_material`, `lookup_material`.
  - `cone_frustum.rs` — `Aabb`, `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb` (geometry/AABB math; relocated here from `lighting/`). Plus the shared frustum-plane row-math currently named `extract_frustum_planes_for_gpu`, relocated out of `compute_cull.rs` (a GPU module) into a CPU home it shares with `cone_frustum`. The CPU cone path and the GPU cull pipelines (in `postretro-renderer`) both depend *down* on this crate for that row-math — one implementation, called from both directions, no `lighting → renderer` reach-across. Still wgpu-free.
  - Zero internal deps; `glam`/`bytemuck`/`serde` only.
- Update importers (geometry: ~10 files; material: ~6; `cone_frustum`/`Aabb`: model, weapon hit-zones, scripting hit-zones, render-cpu, and the renderer's GPU cull path) to the crate paths. Optional transitional re-export from the old module paths.
- Workspace wiring per `scripting.md §12` conventions (naming `postretro-<role>`, `[workspace.package]` inheritance, workspace deps).

### Out of scope
- Any wgpu/GPU code (none of these modules has any — the frustum row-math is pure matrix math).
- Behavior or layout change to `WorldVertex` / `Material` / `Aabb` (byte layouts are shared with shaders/PRL — keep stable).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] `cargo tree -p postretro-render-data` shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] All importers compile against the crate paths; `Material` derivation, any `BvhTree` tests, and the `cone_frustum` tests pass from their relocated homes.
- [ ] `WorldVertex`/`BvhNode`/`BvhLeaf` byte layouts unchanged (no PRL/shader drift).
- [ ] The shared frustum-plane row-math lives here; the CPU cone path and the GPU cull pipelines (in `postretro-renderer`) both call into it — no `cone_frustum → compute_cull` import remains.

## Tasks

### Task 1: Extract postretro-render-data
New crate. Move `geometry.rs`, `material.rs`, and `cone_frustum.rs` (with the shared frustum-plane row-math already relocated into a CPU home by `E19--leaf-hygiene-and-boundary-prep`) into it as modules. Widen any `pub(crate)` symbols crossing the boundary to `pub`, update importers (geometry/material consumers plus the `cone_frustum`/`Aabb` consumers — model, weapon hit-zones, scripting hit-zones, render-cpu, and the renderer's GPU cull path).

## Decision

**One crate, not two** (was: geometry vs. material as separate crates). Principle: lean — two dependency-free leaf modules gain no recompile isolation from splitting; one crate is fewer workspace members and one dependency edge for every consumer.

## Sequencing
**Phase 1:** Task 1. Milestone 1. Blocks `E19--level-loader`, `E19--visibility`, `E19--render-cpu`, and `E19--renderer-gpu`.
