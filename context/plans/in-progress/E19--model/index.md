# postretro-model

> Epic: `E19--render-stack-decomposition`. The CPU glTF loader, extracted out of the binary so model edits stay off the renderer compile unit.

## Goal

Extract the wgpu-free model subtree (CPU glTF loader, skinned-mesh / skeleton / animation data) into a CPU-only crate, so the renderer crate depends on it as a lower leaf and editing model code does not rebuild the GPU renderer compile unit.

## Scope

### In scope
- `postretro-model`: move the `crates/postretro/src/model/` subtree (`mod.rs`, `mesh.rs`, `anim.rs`, `skeleton.rs`, `sample_params.rs`, `gltf_loader.rs`) — the CPU glTF loader and its data types. `animation_reactions.rs` does **not** move into the crate; it relocates to `scripting/reactions` (see Decision). Public boundary types the move must widen include (non-exhaustive): `ModelHandle`, `BonePaletteEntry`, `SkinnedMesh`, `SkinnedVertex`, `MAX_JOINTS`, the `skeleton` / `anim` / `sample_params` types, and `gltf_loader`'s `LoadedModel`, `Submesh`, `JointZone`, `ModelLoadError`. Confirmed wgpu-free in code (only doc-comment mentions of `wgpu`).
- Expose mesh local-space bounds as an intentional public contract via a `pub fn bounds(&self) -> Aabb` accessor on `SkinnedMesh` (the field is currently `pub(crate) bounds: Aabb`). Renderer upload consumes the bound for CPU-side mesh planning/culling across the crate boundary.
- Depend on `postretro-render-data` (required — `mesh.rs` uses `cone_frustum::Aabb` for `SkinnedMesh` bounds), `postretro-level-format` (features `["gltf-resolve"]` — `gltf_loader.rs` uses `gltf_resolve::resolve_material_base_color_path` and `octahedral`), `glam`, `gltf`, `serde`, `serde_json`, `blake3`, `thiserror`, `bytemuck`, and `log`. No `postretro-entities`/`postretro-scripting-core` dep — the only file that pulled them (`animation_reactions.rs`) relocates out (see Decision), keeping the crate a VM-free CPU leaf.
- Update consumers to import from `postretro-model`.
- Review hardening: keep the extraction safe for existing CPU consumers. Loader validation may reject malformed model data that would poison bounds or palettes. Game-side hit zones must stay pose-compatible with visible renderer semantics; when exact parity is unavailable, they degrade to authored AABB when present, else the model's derived reach bound.

### Out of scope
- Any GPU upload of model data (mesh-instance GPU packing, vertex/joint buffer upload) — stays in `postretro-renderer` (`E19--renderer-gpu`).
- The CPU mesh-frame planner (`mesh_instances`) and `mesh_visible`/`ClipMetadata` — those land in `postretro-render-cpu` (`E19--render-cpu`).
- `animation_reactions.rs` (the `setAnimationState` reaction primitive) — relocates to `scripting/reactions`, not into `postretro-model` (see Decision).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; loader/skeleton/animation tests pass from their relocated home.
- [ ] `cargo tree -p postretro-model` (default features) shows no `wgpu`/`winit`/`glyphon`/`kira`/`mlua`/`rquickjs`.
- [ ] The `postretro` binary (current renderer home) depends on `postretro-model`; no `wgpu` appears in the model crate. (Review/grep gate: no `wgpu` in the Cargo dep tree and no `use wgpu`/`wgpu::` code path — the surviving doc-comment mentions of `wgpu` are expected and fine.) (The `postretro-renderer` → `postretro-model` edge lands with the renderer crate in `E19--renderer-gpu`.)
- [ ] Renderer upload can read each `SkinnedMesh` local-space bound through the `pub fn bounds(&self) -> Aabb` accessor; no same-crate visibility or accidental broad public data is required.
- [ ] Editing `postretro-model` and running its in-crate tests (`cargo test -p postretro-model`) recompiles no `wgpu`/`naga`/`winit`. Note: the renderer still lives in the `postretro` binary in Milestone 1, so a full `cargo check -p postretro` after a model edit still rebuilds it — the firewall for renderer-facing loops closes at `E19--renderer-gpu`; at this task's scope the win is the isolated in-crate test loop.
- [ ] Warm-edit win (manual PR-time measurement, global AC #8): targeted loop is a `model/gltf_loader.rs` (or `anim.rs`) touch. Quote before/after vs. the `E19--baseline-and-cargo-config` baseline (in `context/plans/done/`). The baseline case matrix has no model-touch case, so capture the pre-extraction "before" in the same PR (`touch crates/postretro/src/model/gltf_loader.rs` then `cargo check -p postretro`) before the move, and compare against the post-extraction `cargo test -p postretro-model` loop.

## Tasks

### Task 1: Extract postretro-model
Create the crate and move the `model/` subtree. Widen visibility: the five moving submodules (`mesh`, `anim`, `skeleton`, `sample_params`, `gltf_loader`) are `pub(crate) mod` and `ModelHandle` (plus its `String` field and `as_str`) and the entire `sample_params` public-type set are `pub(crate)` — all must become `pub`. (`animation_reactions` does not widen — it relocates out of `model/`; see below and Decision 2.) Wire deps: `postretro-render-data`, `postretro-level-format` (features `["gltf-resolve"]`), `glam`, `gltf`, `serde`, `serde_json`, `blake3`, `thiserror`, `bytemuck`, `log` (no `postretro-entities`/`postretro-scripting-core` — see Decision 2). Rewrite every `crate::model::` reference inside the moved files — doc comments AND functional code and `#[cfg(test)]` imports (e.g. `use crate::model::anim::Loop` in `sample_params.rs`; `use crate::model::skeleton::Joint` / `crate::model::gltf_loader::load_model` in `anim.rs` tests; `crate::render` doc path in `sample_params.rs`) — to the new crate root (`crate::`); leave `super::` paths untouched (they resolve within the module tree). Update external consumers — ~19 files, ~74 sites across `render/`, `scripting/`, `weapon/`, `startup/lifecycle.rs`, and `main.rs` — rewriting `crate::model::` to `postretro_model::`. Expose `SkinnedMesh` bounds via the `pub fn bounds(&self) -> Aabb` accessor (keep the field non-`pub`; `Aabb` is `Copy`, so return by value), and switch its one cross-crate field-read consumer, `render/mesh_pass.rs:1448` (`mesh.bounds` → `mesh.bounds()`). Relocate `animation_reactions.rs` out of `model/` into `scripting/reactions/` (it has no back-dependency on model data): drop its `pub(crate) mod animation_reactions;` from `model/mod.rs`, declare it under `scripting/reactions/mod.rs`, and repoint the re-export (`scripting/reactions/mod.rs`) and the registrar call (`scripting/reactions/registry.rs`) to the new path.

## Decisions

**1. Extract a `postretro-model` crate** (was: leave in the binary as a renderer dep). Principle: clean one-way boundaries + the firewall goal — the renderer crate cannot depend up into the binary, and a CPU glTF loader must not live inside the GPU crate or model edits rebuild the whole renderer compile unit. A separate CPU crate keeps the boundary one-way and off the wgpu path.

**2. `animation_reactions.rs` stays with scripting/reactions.** `animation_reactions.rs` is a scripting reaction primitive (`register_mesh_reaction_primitives` → `setAnimationState`): it imports `postretro_scripting_core::reaction_registry` and `postretro_entities::components::mesh::{SwitchResult, switch_animation_state}`, is registered from `scripting/reactions/registry.rs` (re-exported by `scripting/reactions/mod.rs`), and its tests call `crate::scripting::reactions::log_capture`. It is the only subtree file that pulls `entities`/`scripting-core` — and both pull `mlua`/`rquickjs` non-optionally, so moving it into `postretro-model` would make `cargo tree -p postretro-model` show the VM crates, violating the inherited cpu-only AC.

**Resolved: relocate it to `scripting/reactions`.** It operates on entity mesh components, not model data, and has no back-dependency on any sibling `model/` module — so it moves out cleanly and `postretro-model` stays a pure CPU-data leaf with no `entities`/`scripting-core` dep. Principle: the CPU-only firewall goal — a VM-touching registrar has no place in the wgpu-free model leaf. (Rejected: keeping it in the crate behind an optional `script-ffi` feature per epic §12 / Decision 1 — heavier, and the primitive is not model-owned.)

## Sequencing
**Phase 1:** Task 1. Low-risk CPU prerequisite, independent. Milestone 1.
