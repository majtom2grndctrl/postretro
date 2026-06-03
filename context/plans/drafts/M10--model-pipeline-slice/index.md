# M10 Model Pipeline — Thin Vertical Slice

## Goal

Drive one real skinned glTF model end-to-end through the **live** render path — loaded, animated by one clip, portal-culled, drawn at an entity's transform (flat-lit for now; lighting integration waits on the in-flight lighting rewrite) — as production code that survives, not a throwaway spike. The slice is narrow on purpose: one hardcoded model, one clip, one instance. Its deliverable is the locked runtime contracts (skinned vertex layout, bone-palette layout, mesh-pass shape, `MeshComponent`) every later M10 render task builds on, plus two measured findings that decide whether a mesh bake and a baked pose buffer are warranted.

## Scope

### In scope
- New top-level `model` module (CPU-side): glTF parse → engine structs for mesh geometry, skinning attributes, skeleton, and one animation clip. No wgpu here (subsystem boundary).
- New skinned mesh vertex format (rigid = degenerate single-bone case), mirroring `WorldVertex` encoding conventions where they apply.
- New `render` mesh pass (GPU-side): uploads the model to GPU buffers in the locked layout, draws **flat-lit** (lighting integration deferred — see non-goals), portal/frustum-culls one instance via point→leaf lookup, draws at the entity transform. Built **instance-friendly**: per-instance transform + a palette index into a shared bone-matrix buffer, even at one instance. Leaves an additive bind slot for lighting so the broadening pass adds it without breaking the locked pass shape.
- Bone-matrix palette: shared GPU buffer, per-instance base index; filled each frame from a CPU-sampled pose.
- Single-clip animation sampling (CPU): sample the clip at frame time → local poses → world bone matrices (apply inverse-bind) → palette. No state machine, no blending.
- `MeshComponent` enum variant + value, a `mesh_bridge` that walks mesh entities and feeds the renderer, and **one** entity spawned through a single hardcoded seam carrying a model handle.
- Material resolves through the existing `.prm` → `LoadedTexture` → material bind-group GPU path.
- Two measured findings (measure-and-report, not pass/fail): runtime glTF load time, and per-frame pose-sampling cost projected to wave scale.
- `gltf` crate added as a dependency.

### Out of scope (non-goals)
- Classname spawning from a map. The asset is hardcoded behind one named seam where classname KVP resolution lands later.
- Build-time automation of model-PNG → `.prm` baking. The slice consumes a **pre-baked** `.prm` for the model's texture; Blender-authored-PNG auto-bake is the *glTF mesh loading* broadening task.
- Many-instance draw / GPU-driven indirect integration. The pass data shape is instance-friendly and continuous with the Milestone 3.5 indirect path, but the slice draws one instance directly.
- SH-lit / dynamic-entity lighting integration. The dynamic-entity lighting interface is mid-rewrite and not yet settled (lands soon, isolated to lighting modules). Binding against it now would target an undecided interface. The slice renders flat-lit; lighting is a fast-follow in the broadening *Mesh render pass* task, against the settled interface.
- Shadow casting. The pass is built depth-reusable so a later skinned-depth variant can render NPCs into the 12-light real-time pixmap pool (`lighting/spot_shadow.rs`) — the *Dynamic mesh shadow casting* task. That pool is currently consumer-less (the SDF rewrite is moving static-geometry shadowing off it); NPC casting is its intended next consumer. Shadows are deferred from the slice for scope. No shadow renders this slice.
- Animation state machine, clip blending/crossfade, animation time-slicing.
- LOD / `meshopt`.
- Multiple archetypes, hit zones, navigation, AI.
- Per-entity transform interpolation beyond the player. The slice draws at the entity's tick transform; the pass accepts a final per-instance matrix so interpolation layers in later.

## Acceptance criteria

**Automated / honesty gates** (a non-author can verify without reading the implementation):
- [ ] `cargo build` and `cargo test` pass with the `gltf` dependency added.
- [ ] Loading a malformed or unsupported glTF logs a warning and degrades gracefully (no panic, slice continues with the model absent) — not a hard error.
- [ ] A unit test confirms the skinned vertex struct and bone-palette entry sizes/alignment are GPU-upload-safe (`bytemuck` Pod/Zeroable round-trip), and that a rigid (no-skin) model loads as the single-bone degenerate case with identity-weighted joint 0.
- [ ] A test confirms point→leaf lookup placing the entity in a cell outside the current visible set excludes it from the draw list (culling behaves; verified against a closed-portal arrangement or a synthetic visible-set).
- [ ] Findings note exists at `findings.md` with both measured values and a recommendation.

**Manual-visual** (per honest-visual-acceptance-criteria — a human confirms by running):
- [ ] The hardcoded model renders in the level at its entity's position, flat-lit (lighting integration deferred to the broadening mesh pass).
- [ ] The model plays its single animation clip (visible skeletal motion), not frozen in bind pose.
- [ ] Walking the camera so a closed portal occludes the model's cell makes it disappear (portal culling reads visually correct).

**Measured findings** (recorded, not gated):
- [ ] Runtime glTF parse+upload time recorded as a startup/level timing stage and logged; reported against the near-instant-boot northstar with a recommendation on whether a mesh bake is warranted.
- [ ] Per-frame CPU cost to sample one skeleton's clip into the palette is logged and projected to wave scale (×N agents), with a recommendation on whether an `ozz`-style baked pose buffer is warranted.

## Tasks

### Task 1: Contracts + module skeleton
Define the locked contracts and create the empty/thin module files they live in, so later tasks fill files in place (no dump-and-split). Skinned vertex format (position, UV, octahedral normal per `WorldVertex` convention, plus joint indices ×4 and weights ×4; rigid = joint 0 weight 1). Bone-palette entry layout (shared buffer of bone matrices; per-instance base index). CPU model struct shapes in `model/` (mesh, skeleton, clip). `Mesh` variant added to `ComponentKind` / `ComponentValue` (`ComponentKind::Mesh` / `ComponentValue::Mesh(MeshComponent)`). State each as a *constraint* (sizes, alignment, instance-friendliness), not prescribed byte offsets. Blocks everything.

### Task 2: glTF loader (CPU)
Parse one glTF via the `gltf` crate into the Task 1 structs: mesh geometry + skinning attributes (positions, normals, UVs, joint indices, weights), the joint hierarchy + inverse-bind matrices, and one animation clip's keyframes. Resolve the material's external PNG reference to a `blake3` cache key. The one hardcoded model must reference an external PNG; embedded-texture glTF is out of scope this slice. No wgpu. Narrow: one model, one clip; multi-mesh/multi-clip generality is the broadening task.

### Task 3: Mesh render pass (GPU)
New pass in `render/`: upload the Task 2 mesh into GPU buffers in the locked vertex layout; allocate the shared bone-palette buffer; build the pipeline binding camera uniforms and the material bind group (via existing `.prm`→`LoadedTexture` path, pre-baked texture). Flat-lit — no lighting bind group this slice; leave the slot additive. Cull one instance: `find_leaf(entity_position)` → test membership in the current visible-cell set → draw or skip. Membership is `cells.contains(&(find_leaf(pos) as u32))` — cell ids equal leaf indices in the current compiler (`visibility.rs:462`); on `VisibleCells::DrawAll` the instance always draws. Draw at the per-instance model matrix. Depth-reusable pipeline shape — the pass must support a depth-only skinned variant (position + bone palette, no material) so NPC shadow casting drops in later; no shadow target this slice, and no coupling to the (currently consumer-less, possibly reshaped) spot-shadow pool's API. Wire construction + per-frame `record_draw` into the renderer; keep edits to the large `render/mod.rs` / `main.rs` render loop minimal (construct + call only).

### Task 4: Animation sampling → palette (CPU)
Sample the Task 2 clip at the frame's animation time → local joint poses → world bone matrices (compose hierarchy, apply inverse-bind) → write the palette buffer the pass uploads. Raw single-clip sampling; no blend, no state machine. Lives in `model/` (CPU math); palette upload stays in the pass (renderer owns GPU).

### Task 5: Entity wiring + hardcoded seam
`mesh_bridge` walks `iter_with_kind(ComponentKind::Mesh)`, reads each entity's `Transform`, and supplies the per-instance matrix + model handle to the pass. Spawn exactly one entity carrying a `MeshComponent` through a single named seam that hardcodes the model path/handle — the chokepoint a future classname handler resolves. Small `main.rs` render-stage edit to run the bridge, mirroring the emitter/particle bridge call sites.

### Task 6: Measurements + findings note
Instrument the two findings: a startup/level timing stage around glTF parse+upload, and a per-frame log of pose-sampling CPU cost. Write `findings.md`: both measured values, the manual-visual read, and recommendations on the mesh-bake and baked-pose-buffer questions.

## Sequencing

**Phase 1 (sequential):** Task 1 — contracts + module skeleton block everything; the locked layouts are shared by all downstream tasks.
**Phase 2 (sequential):** Task 2 — glTF loader produces the CPU data the GPU pass and animation consume.
**Phase 3 (sequential):** Task 3 — mesh pass consumes Task 2 data; touches the shared renderer/render-loop files.
**Phase 4 (sequential):** Task 4 — animation sampling consumes the loaded clip and writes the pass's palette buffer.
**Phase 5 (sequential):** Task 5 — entity wiring consumes the pass and the `MeshComponent`; small shared `main.rs` edit (serialized after Task 3's render-loop edit to avoid conflict).
**Phase 6 (sequential):** Task 6 — measurements depend on the full path running.

Mostly sequential by nature: a thin vertical slice is one tightly-coupled path, and Tasks 3/5 both edit the large `render/mod.rs` / `main.rs` files. Parallelism is deferred to the broadening tasks, which fan out across the now-decomposed modules.

## Rough sketch

**Module decomposition (the durable deliverable):**
- `crates/postretro/src/model/` (new, CPU-only, no wgpu): `gltf_loader.rs` (parse), `mesh.rs` (skinned vertex type + CPU mesh), `skeleton.rs` (joints, inverse-bind, clip), `anim.rs` (clip sampling → bone matrices). Broadening tasks *glTF mesh loading*, *glTF skeleton + clip loading*, and *Skinned animation runtime* fill these in place.
- `crates/postretro/src/render/mesh_pass.rs` (new): GPU buffers, bone-palette buffer, pipeline, `new()` + `record_draw()` — same shape as `SmokePass` (`render/smoke.rs`). Owns all wgpu for meshes. Broadening tasks *Mesh render pass + MeshComponent* and *Dynamic mesh shadow casting* fill it in place.
- `crates/postretro/src/scripting/components/mesh.rs` (new) + `Mesh` variant in `scripting/registry.rs`; `crates/postretro/src/scripting/systems/mesh_bridge.rs` (new), mirroring `systems/emitter_bridge.rs` / `systems/particle_render.rs`.

**Key reuse points (confirmed in source):**
- Point→cell: `LevelWorld::find_leaf(position: Vec3) -> usize` (`prl.rs:301`), already general over arbitrary points.
- Visible set: `enum VisibleCells { Culled(Vec<u32>), DrawAll }` (`visibility.rs:13`); bitmask membership in `compute_cull.rs`.
- Lighting: deferred. The mesh shader is flat-lit this slice; the broadening pass adds the lighting bind group against the settled (rewritten) dynamic-entity lighting interface.
- Material: `load_textures` / `LoadedTexture` (`render/loaded_texture.rs:295`, `:26`) → `build_material_bind_group` (`render/mod.rs:434`).
- Pass pattern: `SmokePass::new()/record_draw()` (`render/smoke.rs`); depth shape from the depth-prepass pipeline (`render/mod.rs:1710`). Eventual shadow consumer for the skinned-depth variant: the spot-shadow pool (`lighting/spot_shadow.rs`), currently consumer-less — not wired this slice.
- Vertex convention: `WorldVertex` (`geometry.rs:11`), octahedral normal, u16×2 UVs.
- Timing: `StartupTimings::record()` (`startup/mod.rs`).

**Bone palette & instancing:** one shared bone-matrix storage buffer; each instance has a base index. The slice writes one instance's palette; the layout admits N contiguous palettes so the broadening pass scales without a layout change. Constraint: per-instance data = (model matrix, palette base index), continuous with the M3.5 indirect per-draw shape.

**Material for the slice:** the model's texture is resolved through the existing `.prm`→`LoadedTexture` GPU binding using a **pre-baked** `.prm` (produced offline by the existing baker for the one model). Build-time automation of model-PNG baking is deferred; the seam is the loader's PNG-reference → cache-key → `.prm` resolution, identical in mechanism to world textures.

## Boundary inventory

Only the Rust ↔ serde boundary is crossed this slice (scripting/FGD spawning deferred). Pin the tag now so the broadening task inherits it:

| Name | Rust | Wire / serde (`ComponentValue` tag) | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `MeshComponent` | `ComponentValue::Mesh` | `"mesh"` | deferred (broadening) | deferred | deferred |

## Open questions

- **Lighting deferred to the rewrite; shadow casting uses the existing pool.** Two distinct systems, deferred for different reasons. (1) How an NPC is *lit* — indirect SH + direct light — rides the dynamic-entity lighting interface, which is mid-rewrite (static lights / static-geometry SDF shadows) and unsettled. The slice renders flat-lit; the broadening *Mesh render pass* adds lighting against the settled interface, additively. (2) How an NPC *casts* a shadow — render into the 12-light real-time pixmap pool (`lighting/spot_shadow.rs`) — is deferred to the *Dynamic mesh shadow casting* task. That pool is currently consumer-less (the SDF rewrite moved static-geometry shadowing off it) and NPC casting is its intended next consumer, so the pool may be reshaped when that task lands. The slice therefore couples to nothing in the pool: its only locked contract is that the mesh pass supports a depth-only skinned variant (position + bone palette, no material). It also leaves an additive lighting bind slot. Neither breaks the locked contracts.
- **Vertex layout vs settled lighting inputs.** The skinned vertex layout is a locked contract. Confirm the settling lighting interface needs no per-vertex inputs beyond position / normal / tangent / UV before treating the layout as final. Low risk — indirect SH and the direct-light loop use normal + world position — and lighting lands in days.
- **Leaf-index vs cell-id.** `find_leaf` returns a leaf index; `VisibleCells::Culled` holds cell ids. The implementer must confirm the mapping (camera_leaf is stored as the leaf index cast to `u32`) so the membership test compares like for like. Specified behaviorally in AC; mechanism confirmed at implementation.
- **`gltf` crate version.** Pin at implementation to the current stable release; note it pulls `image` (already a dependency) for embedded textures, though the slice uses external PNG references.
- **Pre-baked `.prm` provenance.** The slice needs one `.prm` for the model's texture in the cache. Acceptable to produce it via the existing offline baker as a one-time manual step? (Alternative: bind the model to an existing baked world texture for the slice.) Decision affects only how the one texture is staged, not the GPU binding path.
