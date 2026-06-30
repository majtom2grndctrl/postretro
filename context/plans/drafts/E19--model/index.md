# postretro-model

> Epic: `E19--render-stack-decomposition`. The CPU glTF loader, extracted out of the binary so model edits stay off the renderer compile unit.

## Goal

Extract the wgpu-free model subtree (CPU glTF loader, skinned-mesh / skeleton / animation data) into a CPU-only crate, so the renderer crate depends on it as a lower leaf and editing model code does not rebuild the GPU renderer compile unit.

## Scope

### In scope
- `postretro-model`: move `crates/postretro/src/model/` — the CPU glTF loader and its data types: `ModelHandle`, `SkinnedMesh`, the skeleton / animation / `sample_params` types, and `gltf_loader`. Confirmed wgpu-free by contract.
- Depend on `postretro-render-data` if it references those types (confirm at implementation), `glam`, and the glTF deps the loader already uses.
- Update consumers to import from `postretro-model`.

### Out of scope
- Any GPU upload of model data (mesh-instance GPU packing, vertex/joint buffer upload) — stays in `postretro-renderer` (`E19--renderer-gpu`).
- The CPU mesh-frame planner (`mesh_instances`) and `mesh_visible`/`ClipMetadata` — those land in `postretro-render-cpu` (`E19--render-cpu`).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; loader/skeleton/animation tests pass from their relocated home.
- [ ] `cargo tree -p postretro-model` shows no `wgpu`/`winit`/`glyphon`/`kira`.
- [ ] `postretro-renderer` depends on `postretro-model`; no `wgpu` appears in the model crate.
- [ ] Editing `postretro-model` does not recompile `wgpu`/`naga`.

## Tasks

### Task 1: Extract postretro-model
Create the crate, move `model/`, widen boundary symbols, wire deps (`postretro-render-data` if referenced), update consumers.

## Decision

**Extract a `postretro-model` crate** (was: leave in the binary as a renderer dep). Principle: clean one-way boundaries + the firewall goal — the renderer crate cannot depend up into the binary, and a CPU glTF loader must not live inside the GPU crate or model edits rebuild the whole renderer compile unit. A separate CPU crate keeps the boundary one-way and off the wgpu path.

## Sequencing
**Phase 1:** Task 1. Low-risk CPU prerequisite, independent. Milestone 1.
