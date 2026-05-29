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
- Shadow casting. The pass is built depth-reusable (mirrors `depth_prepass` shape) but renders no shadow this slice. Shadows land with the lighting rewrite, not here.
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
Define the locked contracts and create the empty/thin module files they live in, so later tasks fill files in place (no dump-and-split). Skinned vertex format (position, UV, octahedral normal per `WorldVertex` convention, plus joint indices ×4 and weights ×4; rigid = joint 0 weight 1). Bone-palette entry layout (shared buffer of bone matrices; per-instance base index). CPU model struct shapes in `model/` (mesh, skeleton, clip). `MeshComponent` variant added to `ComponentKind` / `ComponentValue`. State each as a *constraint* (sizes, alignment, instance-friendliness), not prescribed byte offsets. Blocks everything.

### Task 2: glTF loader (CPU)
Parse one glTF via the `gltf` crate into the Task 1 structs: mesh geometry + skinning attributes (positions, normals, UVs, joint indices, weights), the joint hierarchy + inverse-bind matrices, and one animation clip's keyframes. Resolve the material's external PNG reference to a `blake3` cache key. No wgpu. Narrow: one model, one clip; multi-mesh/multi-clip generality is the broadening task.

### Task 3: Mesh render pass (GPU)
New pass in `render/`: upload the Task 2 mesh into GPU buffers in the locked vertex layout; allocate the shared bone-palette buffer; build the pipeline binding camera uniforms and the material bind group (via existing `.prm`→`LoadedTexture` path, pre-baked texture). Flat-lit — no lighting bind group this slice; leave the slot additive. Cull one instance: `find_leaf(entity_position)` → test membership in the current visible-cell set → draw or skip. Draw at the per-instance model matrix. Depth-reusable pipeline shape (mirror `depth_prepass`), but no shadow target this slice. Wire construction + per-frame `record_draw` into the renderer; keep edits to the large `render/mod.rs` / `main.rs` render loop minimal (construct + call only).

### Task 4: Animation sampling → palette (CPU)
Sample the Task 2 clip at the frame's animation time → local joint poses → world bone matrices (compose hierarchy, apply inverse-bind) → write the palette buffer the pass uploads. Raw single-clip sampling; no blend, no state machine. Lives in `model/` (CPU math); palette upload stays in the pass (renderer owns GPU).

### Task 5: Entity wiring + hardcoded seam
`mesh_bridge` walks `iter_with_kind(MeshComponent)`, reads each entity's `Transform`, and supplies the per-instance matrix + model handle to the pass. Spawn exactly one entity carrying a `MeshComponent` through a single named seam that hardcodes the model path/handle — the chokepoint a future classname handler resolves. Small `main.rs` render-stage edit to run the bridge, mirroring the emitter/particle bridge call sites.

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
- `src/model/` (new, CPU-only, no wgpu): `gltf_loader.rs` (parse), `mesh.rs` (skinned vertex type + CPU mesh), `skeleton.rs` (joints, inverse-bind, clip), `anim.rs` (clip sampling → bone matrices). Broadening tasks *glTF mesh loading*, *glTF skeleton + clip loading*, and *Skinned animation runtime* fill these in place.
- `src/render/mesh_pass.rs` (new): GPU buffers, bone-palette buffer, pipeline, `new()` + `record_draw()` — same shape as `SmokePass` (`render/smoke.rs`). Owns all wgpu for meshes. Broadening tasks *Mesh render pass + MeshComponent* and *Dynamic mesh shadow casting* fill it in place.
- `src/scripting/components/mesh.rs` (new) + `MeshComponent` variant in `scripting/registry.rs`; `src/scripting/systems/mesh_bridge.rs` (new), mirroring `systems/emitter_bridge.rs` / `systems/particle_render.rs`.

**Key reuse points (confirmed in source):**
- Point→cell: `LevelWorld::find_leaf(position: Vec3) -> usize` (`prl.rs:263`), already general over arbitrary points.
- Visible set: `enum VisibleCells { Culled(Vec<u32>), DrawAll }` (`visibility.rs:13`); bitmask membership in `compute_cull.rs`.
- Lighting: deferred. The mesh shader is flat-lit this slice; the broadening pass adds the lighting bind group against the settled (rewritten) dynamic-entity lighting interface.
- Material: `load_textures` / `LoadedTexture` (`render/loaded_texture.rs:295`, `:24`) → `build_material_bind_group` (`render/mod.rs:391`).
- Pass pattern: `SmokePass::new()/record_draw()` (`render/smoke.rs`); depth shape from `depth_prepass` (`render/mod.rs:1703`).
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

- **Lighting + shadows deferred to the in-flight rewrite.** The dynamic-entity lighting interface is being rewritten — not yet settled, lands soon, isolated to lighting modules. The slice therefore renders flat-lit and casts no shadow; both integrate in the broadening *Mesh render pass* / *Dynamic mesh shadow casting* tasks against the settled lighting system. (Roadmap M5's "CSM sun shadows" no longer reflects the tree — only the spot-shadow pool + baked lightmaps exist — but that is subsumed by the rewrite, not a separate concern.) The slice keeps the pass depth-reusable and leaves an additive lighting bind slot; adding a bind group later does not break the locked vertex / palette / pass-shape contracts.
- **Vertex layout vs settled lighting inputs.** The skinned vertex layout is a locked contract. Confirm the settling lighting interface needs no per-vertex inputs beyond position / normal / tangent / UV before treating the layout as final. Low risk — indirect SH and the direct-light loop use normal + world position — and lighting lands in days.
- **Leaf-index vs cell-id.** `find_leaf` returns a leaf index; `VisibleCells::Culled` holds cell ids. The implementer must confirm the mapping (camera_leaf is stored as the leaf index cast to `u32`) so the membership test compares like for like. Specified behaviorally in AC; mechanism confirmed at implementation.
- **`gltf` crate version.** Pin at implementation to the current stable release; note it pulls `image` (already a dependency) for embedded textures, though the slice uses external PNG references.
- **Pre-baked `.prm` provenance.** The slice needs one `.prm` for the model's texture in the cache. Acceptable to produce it via the existing offline baker as a one-time manual step? (Alternative: bind the model to an existing baked world texture for the slice.) Decision affects only how the one texture is staged, not the GPU binding path.
