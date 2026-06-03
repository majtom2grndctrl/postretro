# M10 Model Pipeline — Thin Vertical Slice

## Goal

Drive one real skinned glTF model end-to-end through the **live** render path — loaded, animated by one clip, portal-culled, drawn at an entity's transform (flat-lit) — as production code that survives, not a throwaway spike. One hardcoded model, one clip, one instance.

A slice that runs ahead of its consumers is an instrument, not a standard: it retires risk and surfaces unknowns, it does not ratify an ABI. So the deliverable is three things, not a frozen contract:

1. **A proven path** — one glTF loaded → posed → culled → drawn, in production code, in the durable module layout.
2. **A reversibility-tiered contract proposal** — cheap, art-budget-bound layouts *committed* now; consumer-bound layouts (instancing, lighting, shadows) *named provisional* until their consumer is in the room.
3. **Two measured tripwires** — runtime glTF load time and per-frame pose-sampling cost — that confirm (or refute) two deferrals the roadmap already leans toward: no offline mesh bake, no baked pose buffer.

The lock comes when the consumer arrives. This slice proposes; the broadening tasks that consume each layout do the locking.

## Scope

### In scope
- New top-level `model` module (CPU-side): glTF parse → engine structs for mesh geometry, skinning attributes, skeleton, and one animation clip. No wgpu here (subsystem boundary).
- New skinned mesh vertex format (rigid = degenerate single-bone case), mirroring `WorldVertex` encoding conventions. Attribute set and widths are **committed** (Task 1) — they bind on the art budget, not on a deferred consumer.
- New `render` mesh pass (GPU-side): uploads the model to GPU buffers in the committed vertex layout, draws **flat-lit**, portal/frustum-culls one instance via point→leaf lookup, draws at the entity transform via a **direct draw call**. A shared bone-palette buffer with a per-instance base index exists (the broadening pass scales it), but the slice draws one instance the simple way and does **not** claim the wave-scale instance layout is validated at N=1. Leaves an additive bind slot for lighting so the broadening pass adds it without breaking the pass shape.
- Bone-matrix palette: shared GPU buffer, per-instance base index; filled each frame from a CPU-sampled pose.
- Single-clip animation sampling (CPU): sample the clip at frame time → local poses → world bone matrices (apply inverse-bind) → palette. No state machine, no blending.
- `ComponentKind::Mesh` / `ComponentValue::Mesh(MeshComponent)`; a render-frame **mesh collector** that walks mesh entities during the Render phase and feeds the pass; **one** entity spawned through a single hardcoded seam carrying a model handle.
- Material resolves through the existing `.prm` → `LoadedTexture` → material bind-group GPU path, keyed by an **offline-baked** cache key (no runtime hashing — see non-goals).
- Coordinate-system de-risk: confirm the glTF → engine basis conversion yields an upright, un-mirrored, correctly-scaled model with animation playing forward (the first-model time-sink — see acceptance criteria).
- Two measured tripwires (measure-and-report, not pass/fail): runtime glTF load time, and per-frame CPU pose-sampling cost projected to wave scale.
- `gltf` crate added as a dependency.

### Out of scope (non-goals)
- Classname spawning from a map. The asset is hardcoded behind one named seam where classname KVP resolution lands later.
- **Runtime texture hashing.** The runtime never hashes a PNG — `blake3` cache keys are baked offline by the level compiler and looked up at runtime (baked-over-computed). The slice consumes a **pre-baked** `.prm` whose cache key is staged offline; adding a runtime PNG → key path is a deliberate architectural decision deferred to the *glTF mesh loading* broadening task, not smuggled in here.
- Build-time automation of model-PNG → `.prm` baking. The one model's texture is baked once, offline, by the existing baker. Auto-bake of Blender-authored PNGs is the *glTF mesh loading* broadening task.
- **Wave-scale instance draw and its measurement.** Many-instance / GPU-driven indirect draw is deferred. The real wave risk — GPU vertex-skinning throughput and bone-palette upload bandwidth at N instances — is **not measurable at N=1** and is owned by the many-instance broadening task, not this slice. The slice draws one instance directly; the shared-palette layout is provisional until that task validates it at scale.
- SH-lit / dynamic-entity lighting integration. The dynamic-entity lighting interface is mid-rewrite and not yet settled (lands soon, isolated to lighting modules). Binding against it now would target an undecided interface. The slice renders flat-lit; lighting is a fast-follow in the broadening *Mesh render pass* task, against the settled interface.
- Shadow casting. No shadow renders this slice. Pipeline structure should *prefer* (non-binding design note, not a contract) a shape where a position-only depth-only skinned variant is a natural later addition for the *Dynamic mesh shadow casting* task — but no depth variant is built or tested here, and the slice couples to nothing in the (currently consumer-less, possibly reshaped) spot-shadow pool.
- Animation state machine, clip blending/crossfade, animation time-slicing.
- LOD / `meshopt`.
- Multiple archetypes, hit zones, navigation, AI.
- Per-entity transform interpolation beyond the player. The slice draws at the entity's tick transform; the pass accepts a final per-instance matrix so interpolation layers in later.

## Acceptance criteria

**Automated / honesty gates** (a non-author can verify without reading the implementation):
- [ ] `cargo build` and `cargo test` pass with the `gltf` dependency added.
- [ ] Loading a malformed or unsupported glTF logs a warning and degrades gracefully — the loader returns an error value, the spawn seam skips the missing model, no panic, the slice continues. ("Absent" = the entity is skipped this slice; the broadening mod-spawn task upgrades this to a loud magenta placeholder, the right degrade once models are mod-fed.)
- [ ] A unit test confirms the skinned vertex struct and bone-palette entry sizes/alignment are GPU-upload-safe (`bytemuck` Pod/Zeroable round-trip), the layout carries a tangent attribute, and a rigid (no-skin) model loads as the single-bone degenerate case with identity-weighted joint 0.
- [ ] A test confirms point→leaf lookup placing the entity in a cell outside the current visible set excludes it from the draw list (culling behaves; verified against a closed-portal arrangement or a synthetic visible-set).
- [ ] Findings note exists at `findings.md` with both measured values, the coordinate-system read, and a recommendation.

**Manual-visual** (per honest-visual-acceptance-criteria — a human confirms by running):
- [ ] The hardcoded model renders in the level at its entity's position, flat-lit, **upright, un-mirrored, and correctly scaled** — confirming the glTF → engine basis conversion (glTF is Y-up / right-handed / meters; the engine world basis differs).
- [ ] The model plays its single animation clip (visible skeletal motion playing **forward**), not frozen in bind pose and not running mirrored or backward.
- [ ] Walking the camera so a closed portal occludes the model's cell makes it disappear (portal culling reads visually correct).

**Measured tripwires** (recorded, not gated — each confirms or refutes a deferral already leaned toward):
- [ ] Runtime glTF parse+upload time recorded as a startup/level timing stage and logged; reported against the near-instant-boot northstar. Confirms or refutes "no offline mesh bake at this poly count."
- [ ] Per-frame CPU cost to sample one skeleton's clip into the palette is logged and projected to wave scale (×N agents). Confirms or refutes "no `ozz`-style baked pose buffer." Note in `findings.md` that this is the CPU side only; the GPU-skinning / palette-upload cost at N instances is the actual wave risk and is the many-instance task's measurement, not this slice's.

## Tasks

### Task 1: Contracts + module skeleton
Define the contracts at the right confidence tier and create the empty/thin module files they live in, so later tasks fill files in place (no dump-and-split). State each as a *constraint* (attribute set, widths, alignment, instance-friendliness), never a byte offset.

**Committed** (art-budget-bound — decide now, downstream builds against these):

| Contract | Constraint |
|---|---|
| Skinned vertex attributes | position; UV; octahedral normal **and** octahedral tangent + bitangent-sign, per `WorldVertex` convention; joint indices ×4; weights ×4. Tangent is included now — a known-imminent lighting consumer may want it, and a touched-once vertex pays 4 bytes cheaply versus re-baking every model after a layout break. |
| Vertex widths | joint indices `u8`×4 (256 bones, ample for low-poly); weights `u8`×4 normalized; UV and normal/tangent quantized to `u16` per `WorldVertex`. Rigid = joint 0, weight 1. |
| Bone-palette entry | one bone matrix per joint in a shared storage buffer; per-instance base index selects an instance's contiguous run. |
| Component | `ComponentKind::Mesh` / `ComponentValue::Mesh(MeshComponent)`, serde tag `"mesh"`. |

**Provisional** (consumer-bound — named, not frozen; the slice does not pretend to validate these):

| Contract | Why provisional |
|---|---|
| Per-instance / per-draw struct's alignment to the M3.5 indirect shape | The many-instance task gets the vote. The slice draws one instance directly and does **not** pre-fit the indirect layout. |
| Depth-only skinned variant shape | The *Dynamic mesh shadow casting* task gets the vote. Non-binding design note only; nothing built this slice. |
| Lighting bind group | The settled dynamic-entity lighting interface gets the vote; additive slot left open. |

CPU model struct shapes in `model/` (mesh, skeleton, clip). Blocks everything.

### Task 2: glTF loader (CPU)
Parse one glTF via the `gltf` crate into the Task 1 structs: mesh geometry + skinning attributes (positions, normals, tangents, UVs, joint indices, weights), the joint hierarchy + inverse-bind matrices, and one animation clip's keyframes. The one hardcoded model must reference an external PNG; embedded-texture glTF is out of scope this slice. Resolve the material's texture to a **pre-staged** `blake3` cache key (baked offline, looked up via the existing `TextureCacheKeys` path) — the loader does **not** hash the PNG at runtime. The loader returns an error value on malformed/unsupported input (no panic); the caller handles absence. No wgpu. Narrow: one model, one clip; multi-mesh/multi-clip generality is the broadening task.

### Task 3: Mesh render pass (GPU)
New pass in `render/`: upload the Task 2 mesh into GPU buffers in the committed vertex layout; allocate the shared bone-palette buffer; build the pipeline binding camera uniforms and the material bind group (via existing `.prm`→`LoadedTexture` path, pre-baked texture). Flat-lit — no lighting bind group this slice; leave the slot additive. Cull one instance: `find_leaf(entity_position)` → test membership in the current visible-cell set → draw or skip. Membership is `cells.contains(&(find_leaf(pos) as u32))` — cell ids equal leaf indices in the current compiler (`visibility.rs:462`); on `VisibleCells::DrawAll` the instance always draws. Draw the one instance with a **direct draw call** at the per-instance model matrix — do not contort the path to pre-fit the wave-scale indirect layout (that abstraction is validated at scale by the many-instance task, never at N=1). Prefer a pipeline structure where a position-only depth-only variant would be a natural later add (design note, not a contract); build no depth variant and couple to nothing in the spot-shadow pool. Wire construction + per-frame `record_draw` into the renderer; keep edits to the large `render/mod.rs` / `main.rs` render loop minimal (construct + call only).

### Task 4: Animation sampling → palette (CPU)
Sample the Task 2 clip at the frame's animation time → local joint poses → world bone matrices (compose hierarchy, apply inverse-bind) → write the palette buffer the pass uploads. Raw single-clip sampling; no blend, no state machine. Lives in `model/` (CPU math); palette upload stays in the pass (renderer owns GPU).

### Task 5: Entity wiring + hardcoded seam
A render-frame **mesh collector** runs in the Render phase, walks `iter_with_kind(ComponentKind::Mesh)`, read-only-borrows each entity's `Transform`, and supplies the per-instance matrix + model handle to the pass. It is a render-phase collector, **not** a game-logic-tick bridge — game logic never touches the renderer (`Renderer owns GPU` invariant). Mirror the particle render-frame collector (`systems/particle_render.rs`), not a tick-stage bridge. Spawn exactly one entity carrying a `MeshComponent` through a single named seam that hardcodes the model path/handle — the chokepoint a future classname handler resolves; if the load returned an error, this seam skips the spawn. Small `main.rs` render-stage edit to run the collector, mirroring the particle render-collector call site.

### Task 6: Measurements + findings note
Instrument the two tripwires: a startup/level timing stage around glTF parse+upload, and a per-frame log of pose-sampling CPU cost. Write `findings.md`: both measured values, the coordinate-system / orientation read (upright, un-mirrored, forward-playing), the manual-visual read, and recommendations on the mesh-bake and baked-pose-buffer questions. Record explicitly that the wave-scale GPU-skinning / palette-upload cost is unmeasured here and owned by the many-instance task.

## Sequencing

**Phase 1 (sequential):** Task 1 — contracts + module skeleton block everything; the committed layouts are shared by all downstream tasks.
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
- `crates/postretro/src/scripting/components/mesh.rs` (new) + `Mesh` variant in `scripting/registry.rs`; `crates/postretro/src/scripting/systems/mesh_render.rs` (new render-frame collector), mirroring `systems/particle_render.rs`.

**Key reuse points (confirmed in source):**
- Point→cell: `LevelWorld::find_leaf(position: Vec3) -> usize` (`prl.rs:301`), already general over arbitrary points.
- Visible set: `enum VisibleCells { Culled(Vec<u32>), DrawAll }` (`visibility.rs:13`); bitmask membership in `compute_cull.rs`.
- Lighting: deferred. The mesh shader is flat-lit this slice; the broadening pass adds the lighting bind group against the settled (rewritten) dynamic-entity lighting interface.
- Material: `load_textures` / `LoadedTexture` (`render/loaded_texture.rs:295`, `:26`) → `build_material_bind_group` (`render/mod.rs:434`). Runtime looks up baked keys via `TextureCacheKeys` — it does not hash PNGs.
- Pass pattern: `SmokePass::new()/record_draw()` (`render/smoke.rs`); depth shape from the depth-prepass pipeline (`render/mod.rs:1710`). Eventual shadow consumer for the skinned-depth variant: the spot-shadow pool (`lighting/spot_shadow.rs`), currently consumer-less — not wired this slice.
- Render-frame collector pattern: `systems/particle_render.rs` (walks entities in the Render phase, hands packed draw data to a pass) — the correct analog for the mesh collector. The tick-stage `systems/emitter_bridge.rs` is **not** the analog; game logic must not touch the renderer.
- Vertex convention: `WorldVertex` (`geometry.rs:11`), octahedral normal + tangent, u16×2 UVs.
- Timing: `StartupTimings::record()` (`startup/mod.rs`).

**Bone palette & instancing:** one shared bone-matrix storage buffer; each instance has a base index. The slice writes one instance's palette and draws it with a direct draw call. The layout *admits* N contiguous palettes, but a shared-buffer indirection behaves differently at N=1 versus N=200 (alignment, dynamic-offset stride, upload coalescing) — so the slice treats the multi-instance layout as **provisional**, validated for real by the many-instance broadening task, not claimed validated here.

**Material for the slice:** the model's texture is resolved through the existing `.prm`→`LoadedTexture` GPU binding using a **pre-baked** `.prm` (produced offline by the existing baker for the one model) whose `blake3` cache key is staged so the runtime looks it up exactly like a world texture. No runtime hashing — the baked-over-computed invariant holds. Build-time automation of model-PNG baking, and any runtime PNG → key path, are deferred to the broadening *glTF mesh loading* task.

## Boundary inventory

Only the Rust ↔ serde boundary is crossed this slice (scripting/FGD spawning deferred). Pin the tag now so the broadening task inherits it:

| Name | Rust | Wire / serde (`ComponentValue` tag) | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `MeshComponent` | `ComponentValue::Mesh` | `"mesh"` | deferred (broadening) | deferred | deferred |

## Open questions

- **Tangent confirmation (low-stakes).** Tangent is committed into the vertex layout by default. Confirm with the lighting-rewrite owner whether dynamic meshes get tangent-space normal mapping. A "yes" is already satisfied; a "no" wastes 4 bytes per vertex — cheap either way, so this is a courtesy check, not a blocker.
- **`gltf` crate version.** Pin at implementation to the current stable release; note it pulls `image` (already a dependency) for embedded textures, though the slice uses external PNG references.
