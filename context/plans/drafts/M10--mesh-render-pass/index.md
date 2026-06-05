# Mesh Render Pass + `MeshComponent`

> **Status:** draft.
> **Milestone:** 10 (Animated Enemies) — render foundation track. Follows the shipped *Thin vertical slice* and *glTF mesh loading*.
> **Related:** `context/lib/rendering_pipeline.md` §9 · `context/lib/entity_model.md` · `context/lib/build_pipeline.md` §Built-in Classname Routing · `context/plans/done/M10--gltf-mesh-loading/` · `context/plans/done/scripting-foundation/plan-3-emitter-entity.md` (structural template) · sibling `research.md` (code-grounding citations).

## Goal

Generalize the slice's single hardcoded skinned draw into the general per-entity mesh pass: many instances of many models, each at its own interpolated transform, SH-lit (indirect baseline), portal/frustum-culled by camera-leaf lookup. Mesh-bearing entities spawn from map data by classname, retiring the hardcoded asset seam. This is the reusable per-entity render spine the rest of Milestone 10 (shadows, direct lighting, animation runtime, enemies) builds on.

## Target & constraints

Retro aesthetic, modern-but-lean tech (`index.md` §1 northstar). Hardware envelope per `rendering_pipeline.md` §10 — perf floor NVIDIA GTX 16-series (Turing / GTX 1660 Super), compatibility floor AMD Radeon Pro 5500M (RDNA1, 2020 16" MBP, Metal). Assume the **baseline wgpu feature set**; capability-gate anything above it (e.g. `MULTI_DRAW_INDIRECT` / `INDIRECT_FIRST_INSTANCE` are not assumed). Stack: wgpu 29.0.1, naga 29.0.1, gltf 1.4.1.

## Scope

### In scope

- **Many-instance draw.** N instances per model, each with a per-instance transform and a palette base index into the shared bone-matrix buffer. Drawn via instanced `draw_indexed`, per-instance data in a storage buffer indexed by `@builtin(instance_index)`. Removes the slice's `mesh_draws.len() <= 1` assert.
- **Multi-model cache.** Renderer-side handle→GPU-model cache keyed by `MeshComponent.model`. Draws grouped by model. Loaded once at level load.
- **SH-lit indirect baseline.** Mesh fragment samples the depth-aware octahedral irradiance atlas (the indirect term), replacing the slice's `FLAT_AMBIENT = 1.0`. Group 2 stays **unallocated** (reserved for the dynamic-direct sibling).
- **Per-entity transform interpolation.** General mechanism: entities carry a previous-tick transform; the render-collection stage lerps previous→current (slerp rotation) at the frame alpha. The mesh collector is its first consumer.
- **Camera-leaf culling, generalized.** Per-instance CPU leaf-membership cull (`mesh_visible`) over all instances.
- **Classname spawning.** A built-in `prop_mesh` classname handler spawns a mesh-bearing entity from map KVPs; FGD entry added. Retires the hardcoded `main.rs` asset seam.
- **Depth-reusable shape.** The pass stays shaped so a depth-only skinned variant (the deferred shadow task) reuses position/joints/weights — built reusable, variant not built.

### Out of scope (deferred to sibling Milestone 10 tasks)

- **Dynamic direct lighting** — the group-2 slot stays unallocated. *Dynamic mesh direct lighting* fills it.
- **Skinned-depth / shadow pass variant** — *Dynamic mesh shadow casting*.
- **Animation state machine, blending, time-slicing** — *Skinned animation runtime*. This task keeps the slice's raw single-clip sampling.
- **glTF skeleton + clip loading and glTF mesh loading** — already shipped.
- **Non-uniform scale on mesh entities** — the normal transform is rotation/uniform-scale-only (see Open questions). Mesh entities use uniform scale; non-uniform-scale normal correctness is a non-goal here.
- **Full GPU-driven indirect for meshes** — the per-instance SSBO + arg layout are shaped to drop into `multi_draw_indexed_indirect` later, but this task draws with instanced `draw_indexed` + CPU cull.

## Acceptance criteria

- [ ] Two mesh entities of the **same** model render simultaneously, each at its own transform (positions verified distinct on screen / in the draw list).
- [ ] Two mesh entities of **different** models render simultaneously, each its own geometry and material.
- [ ] A mesh entity whose origin lands in a non-visible cell is not drawn; one in a visible cell is drawn (`DrawAll` always draws). The `mesh_draws.len() <= 1` assert is gone.
- [ ] A moving mesh entity renders at the interpolated transform between ticks: at frame alpha 0.5 its rendered position is the midpoint of its previous- and current-tick positions; rotation follows the shortest-path slerp. An entity spawned on the current tick renders at its current transform with no pop.
- [ ] A mesh under spatially varying baked irradiance reads the **local** indirect color (a mesh in a dim cell is dimmer than one in a bright cell — not uniform full-bright). Group 2 remains unallocated in the mesh pipeline layout.
- [ ] A mesh near a wall does not pick up indirect light bleeding through the wall (depth-aware / Chebyshev visibility applied).
- [ ] A map entity `classname "prop_mesh"` with `model "<path>"` spawns an entity that renders that model — no script, no hardcoded asset. Absent/invalid `model` logs a warning and the load continues.
- [ ] A single map referencing two distinct `model` paths loads and renders both; each model is uploaded exactly once.
- [ ] The hardcoded model seam (boot-time load + single-instance spawn) is removed; `prop_mesh` is the only spawn path. The FGD declares `prop_mesh`.
- [ ] Palette/instance overflow beyond the documented budget drops the excess instances with a rate-limited warning rather than corrupting the palette or panicking.
- [ ] `skinned_mesh.wgsl` passes naga validation; `cargo test -p postretro` passes; `cargo clippy -p postretro -- -D warnings` clean.
- [ ] **Measured findings (report, not gate):** per-frame per-instance pose-sampling cost at representative instance counts (the `ozz`-pose-buffer question), and the palette buffer's VRAM at the chosen budget. Record in the PR.

## Tasks

### Task A: Per-entity render-transform interpolation

Add a general per-entity transform interpolation mechanism to the entity model. Each entity retains its previous-tick transform alongside the current `Transform`; a tick-boundary step snapshots current→previous before game logic mutates transforms for the new tick. Expose a render-stage accessor that returns the interpolated transform — `position`/`scale` component-lerped, `rotation` shortest-path slerped — at the frame alpha from `frame_timing`. Entities first seen on the current tick interpolate against themselves (previous == current), so no pop on spawn. The mesh collector (Task B) is the first consumer; the accessor is general so future per-entity mesh/prop rendering reuses it. Mirrors the player-camera interpolation already in `frame_timing.rs`, lifted to per-entity and extended to full TRS.

### Task B: Multi-instance draw path + model cache

Generalize the render integration. The collector (`mesh_render.rs`) emits, per surviving instance, the model handle plus the interpolated world transform (Task A) — it currently drops the handle and reads `Transform` directly. The renderer keeps a handle→`UploadedModel` cache (mirroring the texture cache) plus per-model skeleton+clip state; `set_mesh_draws` accepts handle-tagged instances. Each frame the renderer groups instances by model, assigns each a contiguous bone-palette run (base index packed into a **per-instance storage-buffer entry**, addressed by `@builtin(instance_index)` — never `first_instance`, which is unreliable across backends), samples each instance's clip into its run at a per-instance phase, and records one instanced `draw_indexed` per model (per submesh) over its instances. CPU leaf-cull (`mesh_visible`) stays the gate. The per-instance SSBO and any arg layout are shaped to drop into `multi_draw_indexed_indirect` later without a contract change. Palette buffer is a fixed budget (documented instance×joint cap); overflow drops with a rate-limited warning. Removes the `len() <= 1` assert.

### Task C: SH-lit indirect baseline

Make the mesh fragment SH-lit. Add a world-position varying to `skinned_mesh.wgsl` (`instance.model * skinned_pos`), declare the SH atlas bindings at a **new group 4** (instance data stays at the locked group 3; group 2 stays unallocated), and sample the depth-aware octahedral irradiance atlas per-fragment using the skinned world-space normal already computed in the vertex stage. Mirror `forward.wgsl`'s SH lookup preamble (normal offset, grid-coord clamp) and call `sample_sh_indirect_corners_depth_aware` with `reject_backface = false` (entities are not static surfaces — matches the billboard precedent) and Chebyshev probe-occlusion enabled. The renderer reuses the existing `ShVolumeResources` bind group + layout, bound at mesh slot 4. Replaces `FLAT_AMBIENT`.

### Task D: `prop_mesh` classname + FGD + level-load model loading

Add a built-in `prop_mesh` classname handler mirroring `billboard_emitter`: a `CLASSNAME` const, a `model` KVP reader (log-and-fallback), spawn a Transform entity at `entity.origin` with `rotation_quat()`, attach `MeshComponent { model }`. Register it in `register_builtins`. Add a `prop_mesh` `@PointClass` to the FGD with a `model(string)` key. At level load, after dispatch, sweep distinct `MeshComponent.model` handles across spawned entities and load+upload each into the renderer cache once (load-time, not mid-frame — boot/no-hitch northstar). Delete the hardcoded boot-time asset load and single-instance spawn seam.

## Sequencing

**Phase 1 (sequential):** Task A — the interpolated-transform accessor the collector consumes.
**Phase 2 (sequential):** Task B — consumes Task A's accessor; rebuilds `set_mesh_draws` + the collector + the draw path.
**Phase 3 (concurrent):** Task C, Task D — both depend on Task B (the generalized pass and model cache); independent of each other.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Mesh component | `ComponentValue::Mesh(MeshComponent)` / `ComponentKind::Mesh` | `"mesh"` (serde tag) | n/a |
| Mesh classname | handler `CLASSNAME` | `MapEntity.classname == "prop_mesh"` | `= prop_mesh` |
| Model handle | `MeshComponent.model: String` | `"model"` (map KVP value, a content-relative glTF path) | `model(string)` |

No new PRL section — mesh entities ride the existing `MapEntity` section and built-in classname dispatch. (Note: the FGD `model` *key* is distinct from TrenchBroom's `model()` editor-preview *directive*; the key sets the runtime model handle.)

## Rough sketch

- **Locked contracts honored (do not break):** the 32-byte skinned vertex layout, `MAX_JOINTS = 256`, the shared bone-palette + per-instance base-index scheme, instance data at **group 3**, the per-instance uniform/SSBO `model: mat4 + base index` shape, the depth-test-`Less`-and-write dedicated render pass after opaque forward / before billboards. See `rendering_pipeline.md` §9 and `research.md`.
- **Group map (mesh pipeline):** 0 camera · 1 material · **2 unallocated (reserved)** · 3 instance data (palette SSBO + per-instance SSBO) · **4 SH atlas** (reused `ShVolumeResources` bind group). Group 2 staying empty is what keeps the dynamic-direct sibling additive.
- **Per-instance data:** moves from the slice's per-draw uniform to a storage buffer; `vs_main` indexes it by `@builtin(instance_index)` to read `model` and `base_index`. Palette base never travels through `first_instance`/`base_instance` (DX12 reads it as 0 — gfx-rs/wgpu#2471).
- **Animation:** raw single-clip `sample_clip` per visible instance, at a per-instance phase derived deterministically from `EntityId` so a wave isn't lock-step. No state machine (deferred). Sampling cost is a measured finding.
- **Doc reconciliation (at promotion, not now):** `rendering_pipeline.md` §9 currently bundles "SH ambient + dynamic direct" into the reserved group 2. This task splits them — SH lands at group 4 (indirect baseline), group 2 reserved for the direct sibling. Update §9 when this promotes.

## Open questions

- **Non-uniform scale.** The vertex shader's upper-3×3 normal transform is correct only for rotation + uniform scale (`skinned_mesh.wgsl:144-147`). Decision in this spec: mesh entities use uniform scale; non-uniform-scale normal correctness is a non-goal. If a later need appears, switch to the inverse-transpose — additive, no contract break. Confirm no current consumer needs non-uniform scale.
- **Palette budget number.** The fixed instance×joint cap is a constraint, not yet a number — the implementer sizes it from the measured-findings pass (representative wave counts × real joint counts, which are ≪ `MAX_JOINTS`). Overflow policy (drop + rate-limited warn) mirrors the billboard `MAX_SPRITES` precedent.
- **Per-instance phase source.** Deriving phase from `EntityId` is the proposed seed; if it reads too regular, spawn-time offset is the fallback. Cosmetic — does not affect contracts.
