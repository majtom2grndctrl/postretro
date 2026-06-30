# postretro-render-data

> Epic: `E19--render-stack-decomposition`. The CPU leaf data crate under the level loader — geometry, material, and shared frustum/AABB math in one crate.

## Goal

Extract the small, dependency-free CPU type modules into one workspace crate so the loader, visibility, render-cpu, model, lighting, and renderer crates depend on shared leaf data and the shared geometry/frustum math instead of binary-internal modules. This is the universal lower leaf: both the CPU cone path and the GPU cull pipelines (in `postretro-renderer`) depend *down* on it for the shared frustum-plane row-math, so neither needs to reach across into the other.

## Scope

### In scope
- `postretro-render-data`: one crate holding the leaf modules.
  - `geometry.rs` — `WorldVertex`, `BvhNode`, `BvhLeaf`, `BvhTree`, `BucketRange`, `BVH_NODE_FLAG_LEAF`, `BvhTree::derive_bucket_ranges`.
  - `material.rs` — `Material`, `MaterialProperties`, `Material::{shininess,properties}`, `parse_prefix`, `derive_material` (`lookup_material` stays a private helper).
  - `cone_frustum.rs` — `Aabb`, `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb` (geometry/AABB math). render-data moves this file out of `lighting/` into the crate. The shared frustum-plane row-math (`extract_frustum_planes_for_gpu`) was already relocated out of `compute_cull.rs` (a GPU module) into a CPU home beside `cone_frustum` by `E19--leaf-hygiene-and-boundary-prep`; render-data re-grounds to find that home, carries it into the crate, and re-points both callers — the CPU cone path and the renderer's GPU cull path — to the crate path. One implementation, called from both directions, no `lighting → renderer` reach-across. wgpu-free.
  - Zero internal deps; `glam` + `bytemuck` only (`Aabb` is `Pod`/`Zeroable`; `cone_frustum` uses glam). No serde — none of the three modules uses it.
- **Boundary self-sufficiency (absorbs review B-1/B-3 — see Amendment).** Do not assume `E19--leaf-hygiene-and-boundary-prep` widened the exact cross-crate set. Re-ground against the post-leaf-hygiene tree and widen `pub(crate)`→`pub` every symbol an out-of-crate consumer imports — confirmed needs: `Aabb` and its methods (`empty`/`expand`/`transformed`/`from_points`), `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb`, and `extract_frustum_planes_for_gpu` (the renderer calls it cross-crate). Relocate the `cone_frustum` tests into the crate, dropping their `crate::lighting::spot_shadow` / `crate::prl` imports (construct inputs inline); any test that genuinely needs a light-space matrix or `MapLight` stays behind in the binary as an integration test.
- Update importers to the crate paths. Three distinct consumer sets — re-ground each against the live tree (counts indicative, taken at review time):
  - **geometry** (`WorldVertex`/`BvhNode`/`BvhLeaf`/`BvhTree`/`BucketRange`/`BVH_NODE_FLAG_LEAF`): `rg -l 'crate::geometry' crates/postretro/src` — ~21 files (`compute_cull`, `candidate_cull_mirror`, `shadow_cull`, `model/mesh`, `portal_vis`, `prl`, `prl_loader`, `visibility`, `startup/lifecycle`, the `render/renderer_*` set, `render/animated_lightmap`, `scripting/systems/{mesh_render,particle_render}`, tests).
  - **material** (`Material`/`MaterialProperties`/`parse_prefix`/`derive_material`): `rg -l 'crate::material' crates/postretro/src` — ~7 files.
  - **`Aabb`/`cone_frustum`**: `model/mesh.rs`, `weapon/mod.rs` (hit-zones), `scripting/systems/hit_zones.rs`, `render/mesh_instances.rs`, `render/mesh_pass.rs`, `render/renderer_shadow_passes.rs`, `shadow_cull.rs`, `lighting/spot_shadow.rs`. The renderer's GPU cull path (`compute_cull`) consumes the **row-math**, not `Aabb`.
- `scripting/systems/hit_zones.rs` consumes `Aabb` by depending *down* on render-data (one-way). render-data stays VM-free — no `script-ffi` feature; per `scripting.md §12` only marshalling *wiring* takes that gate, and there is none here.
- Workspace wiring per `scripting.md §12` conventions (naming `postretro-<role>`, `[workspace.package]` inheritance, workspace deps).

### Out of scope
- Any wgpu/GPU code (none of these modules has any — the frustum row-math is pure matrix math).
- Behavior change to `WorldVertex` / `BvhNode` / `BvhLeaf` / `Material` / `Aabb`.
- Byte-layout change to the `#[repr(C)]` types `WorldVertex` and `Aabb` (shared with shaders / the GPU model struct). `BvhNode`/`BvhLeaf`/`BvhTree` keep their field shape (behavior-preserving) but carry no `repr(C)` pin and are not the on-disk types — `prl_loader` converts level-format's on-disk `Vertex`/`BvhNode` into the runtime types, so the layout contract is a correspondence across crates, not one shared type. Pin the constraint (no drift vs. the shader/PRL side), not byte offsets. (`Material` is a CPU enum — no byte contract.)

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] `cargo tree -p postretro-render-data` shows no wgpu/winit/glyphon/kira/mlua/rquickjs, and no edge back to `postretro-renderer` (acyclic — the renderer depends *down* on render-data, never the reverse).
- [ ] All importers compile against the crate paths; `Material` derivation, the `BvhTree` tests, and the relocated `cone_frustum` tests pass from inside the crate (no `lighting`/`prl` deps remain in render-data's tests).
- [ ] `WorldVertex` and `Aabb` (`#[repr(C)]`) byte layouts unchanged; `BvhNode`/`BvhLeaf`/`BvhTree` field shape unchanged. No shader/PRL drift on the GPU-facing geometry.
- [ ] Every cross-crate symbol is reachable: `Aabb` + its methods, `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb`, and `extract_frustum_planes_for_gpu` are `pub`; the renderer's GPU cull path imports the row-math from `postretro-render-data` (not a re-declared copy) — no `cone_frustum → compute_cull` import remains.
- [ ] No moved module is a WGSL packer — geometry/material/frustum-math carry no binding-index/stride constants (the frustum fn is pure `Mat4 → [[f32;4];6]`), so the WGSL packer-constant clause of the global ACs is vacuously satisfied here.
- [ ] Warm-edit win: editing `geometry.rs` (or `material.rs`) and running a downstream consumer's tests recompiles no `wgpu`/`naga`/`winit`; the PR quotes before/after vs. the `E19--baseline-and-cargo-config` baseline.

## Tasks

### Task 1: Extract postretro-render-data
New crate. Move `geometry.rs`, `material.rs`, and `cone_frustum.rs` (out of `lighting/`) into it as modules; carry along the shared frustum-plane row-math from the CPU home `E19--leaf-hygiene-and-boundary-prep` gave it (re-ground to locate it — `cone_frustum.rs` or `geometry.rs`). Widen to `pub` exactly the symbols crossing the boundary — re-ground the set rather than trusting a prior widen: confirmed needs are `Aabb` + its methods (`empty`/`expand`/`transformed`/`from_points`), `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb`, `extract_frustum_planes_for_gpu`, and the geometry/material symbols any out-of-crate file imports (`lookup_material` stays private — only `parse_prefix`/`derive_material` are called externally). Relocate the `cone_frustum` tests into the crate, dropping their `crate::lighting::spot_shadow`/`crate::prl` imports (construct inputs inline); leave any test needing a real light-space matrix or `MapLight` behind in the binary as an integration test. Re-point all three consumer sets (geometry ~21, material ~7, `Aabb`/cone_frustum sites + the renderer's row-math call) to the crate paths.

## Decision

**One crate, not two** (was: geometry vs. material as separate crates). Principle: lean — two dependency-free leaf modules gain no recompile isolation from splitting; one crate is fewer workspace members and one dependency edge for every consumer.

## Amendment — boundary self-sufficiency (2026-06-30 review)

The first `review-draft-spec` pass surfaced three boundary issues whose natural home was the prereq `E19--leaf-hygiene-and-boundary-prep` — but that spec is in final review and locking, so render-data absorbs them and is written to stay correct regardless of leaf-hygiene's exact widen set:
- **B-1 (test relocation):** `cone_frustum`'s tests import `lighting::spot_shadow` + `prl`, invisible to a leaf crate. render-data relocates/rewrites them (Scope + Task 1).
- **B-2 (importer map):** the GPU cull path (`compute_cull`) consumes the row-math, not `Aabb`; the `Aabb` consumers are the mesh/shadow/hit-zone sites. Importer lists corrected.
- **B-3 (visibility):** `Aabb`'s methods and `extract_frustum_planes_for_gpu` are still `pub(crate)`; render-data widens them. The re-ground-and-widen rule keeps render-data correct even if leaf-hygiene's widen set differs.

Mechanical corrections folded the same pass: dep list `glam`+`bytemuck` (serde unused); frozen-layout set pinned to the `#[repr(C)]` types; transitional re-export dropped (all importers re-pointed in-spec); warm-edit-loop and acyclicity ACs added.

## Sequencing
**Phase 1:** Task 1. Milestone 1. Built after `E19--leaf-hygiene-and-boundary-prep` lands, then re-grounded against the post-leaf-hygiene tree. Blocks `E19--level-loader`, `E19--visibility`, `E19--render-cpu`, and `E19--renderer-gpu`.
