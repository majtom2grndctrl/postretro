# Flat Leaf Camera Cull

## Goal

Replace the camera path's serial BVH tree walk with a flat, parallel leaf
sieve. Runtime portal traversal stays authoritative. Each world BVH leaf gets
one GPU invocation that tests frustum and visible-cell bit, then writes or zeros
that leaf's existing indirect draw slot. After implementation, flat leaf cull is
the default camera cull path. Tree walk remains a dev fallback and comparison
path only via dev-tools/manual strategy switch or debug env setting. Comparison
means manual mode switching plus CPU estimator diagnostics, not running both GPU
paths every frame.

Recent `stress-warren` diagnostics motivate the pivot. A bad wall-facing view
visited about 5033 BVH nodes, tested 2386 leaves, rejected 2383 leaves by
visible-cell bit, and submitted only 3 leaves. The bottleneck is serial descent
to late leaf rejection, not submitted geometry.

## Scope

### In scope

- Add a flat camera cull compute path for static world geometry.
- Keep runtime portal traversal and `VisibleCells` semantics unchanged.
- Keep the existing per-leaf indirect slot layout and material-bucket draw flow.
- Keep leaf AABB frustum testing and visible-cell bit testing as the exact gates.
- Write every camera indirect slot each frame so stale commands cannot survive.
- Preserve current cull-status overlay meanings.
- Add CPU mirror tests that prove flat leaf cull submits the same leaves and
  indices as the current BVH walk for the same inputs.
- Add diagnostics that compare current tree-walk estimates with flat leaf work.
- Keep verification runnable on CPU-only machines.

### Out of scope

- Compiler changes.
- PRL section changes.
- Visibility regions, baked PVS, per-region BVHs, or BVH layout changes.
- Replacing runtime portal traversal.
- Shadow cone cull migration. Shadows keep the current BVH walk in this plan.
- Hardware occlusion queries, Hi-Z, software occlusion raster, or depth readback.
- GPU timing as an acceptance gate. GPU timing may be collected manually.

## Acceptance Criteria

- [ ] Runtime `VisibleCells` still comes from the existing portal traversal or
      existing fallback paths.
- [ ] The camera cull path dispatches work proportional to BVH leaf count, not
      BVH node count or tree depth.
- [ ] Every camera indirect slot is written each frame. Culled leaves have
      `index_count = 0` in existing material-bucket slot order; submitted leaves
      carry the same full indirect command fields the current BVH walk would
      write.
- [ ] CPU mirror tests prove flat leaf cull and current BVH walk produce the
      same submitted leaf set, submitted index count, material bucket spans, and
      per-leaf indirect command fields for representative frustum-visible,
      frustum-rejected, visible-cell rejected, and `DrawAll` cases.
- [ ] The recorded bad `stress-warren` camera probe remains a
      manual/diagnostic target: about 5033 BVH node visits, 2386 leaf tests,
      2383 visible-cell rejects, and 3 submitted leaves is tree-walk context.
      Flat diagnostics report `flat_work = loaded_bvh_leaf_count` while
      preserving the same 3 final submissions. No checked-in `stress-warren`
      fixture is required.
- [ ] Cull-status wireframe remains meaningful: visible-cell rejects reuse the
      existing not-submitted/cyan status, frustum rejects use frustum status, and
      submitted leaves use rendered status. Frustum test runs first; a leaf that
      fails both gates is frustum/red because the shader exits at the first
      failed gate. Do not add a portal status code.
- [ ] PRLs with missing BVH data continue to fail load if they fail load today.
      PRLs with zero BVH leaves keep the existing zero-leaf behavior.
- [ ] Shadow cull output and dynamic spot shadow behavior are unchanged.
- [ ] No acceptance criterion requires a GPU, adapter timestamp support, or
      `POSTRETRO_GPU_TIMING=1`.
- [ ] `cargo test -p postretro cpu_bvh_diagnostics` passes or is replaced by an
      equally focused CPU-only cull-equivalence test name.
- [ ] New WGSL shader has CPU-only parse/validation coverage with
      `naga::front::wgsl::parse_str` if project shader test patterns support it;
      otherwise renderer run verifies pipeline creation manually.
- [ ] `cargo check -p postretro --features dev-tools` passes.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Split Camera Cull Plumbing

Split `crates/postretro/src/compute_cull.rs` before extending it further. Keep
public behavior unchanged. Move CPU diagnostic helpers and tests into a focused
module, and keep the renderer-facing camera cull owner small enough that adding
a second compute strategy does not deepen the monolith. Preserve existing
exports used by shadow cull.

### Task 2: Add Flat Leaf Cull Shader

Add a WGSL shader for camera culling that dispatches one invocation per BVH
leaf. It reads the existing leaf storage buffer, visible-cell bitmask, frustum
planes, and `leaf_count`. Put `leaf_count` in a separate flat-cull params
uniform/buffer unless implementation intentionally extends Rust/WGSL layouts for
camera-only without changing shadow/tree-walk shared layout; prefer separate
params. `VisibleCells::Culled` uploads the sparse visible-cell bitmask.
`VisibleCells::DrawAll` uploads an all-ones bitmask by default. Use uniform
bypass only if buffer sizing or source reality makes all-ones invalid. For each
leaf, it tests leaf AABB against the camera frustum first, then tests `cell_id`
against the visible-cell contract, writes that leaf's existing indirect slot in
the current material-bucket order, and writes the existing cull-status code. It
does not read BVH nodes or `skip_index`.

### Task 3: Wire Camera Cull Strategy

Add the renderer-side pipeline and bind group plumbing for the flat shader.
Reuse the existing leaf buffer, visible-cell buffer, camera frustum data,
indirect buffer, and cull-status buffer. Add separate flat-cull params plumbing
for `leaf_count` if Task 2 keeps shared `CullUniforms` unchanged. Keep the
existing tree-walk shader available as a dev-tools-only/manual strategy switch
or debug/env setting, but make flat leaf cull the default camera cull path.
Dispatch enough workgroups to cover all leaves, with an in-shader
`leaf_index >= leaf_count` guard for the final partial group. `arrayLength` may
be an extra safety check only if available.

### Task 4: Preserve Draw And Debug Contracts

Keep `draw_indirect_buckets` and material bucket ranges unchanged. Ensure the
depth pre-pass, forward pass, and wireframe cull-status overlay consume the same
indirect and cull-status buffers as before. Confirm no caller needs to know
whether camera cull used tree-walk or flat leaf mode. The flat shader writes the
same per-leaf command slot and order used by the current material-bucket
indirect layout. It must not rebuild bucket ranges or remap leaves.

### Task 5: Add CPU Equivalence Coverage

Add CPU mirror coverage for flat leaf cull. Compare flat leaf results against
the current BVH walk estimator for small synthetic BVHs and for deterministic
stress-map camera probes where data is available. If `stress-warren` fixture
data is unavailable, use representative deterministic probes and keep the
recorded `stress-warren` values as a manual/diagnostic target. Cover
`VisibleCells::Culled` sparse bitmask behavior and `VisibleCells::DrawAll`
all-ones behavior. `DrawAll` coverage may be synthetic/direct helper coverage
for nonzero-leaf tests: current runtime usually skips camera cull for empty
zero-leaf worlds, and solid/exterior/no-portals use `Culled` fallback sets.
Current `BvhCullDiagnostics` exposes counts only. Add or refactor a CPU helper
that returns submitted leaf indices, bucket span identities, and per-leaf
indirect commands so equivalence can compare full commands and culled slots.

### Task 6: Update Diagnostics And Context

Update the Spatial diagnostics readout so it reports tree-walk estimates and
flat leaf work side by side. The data source is CPU-only estimator data:
tree-walk estimate fields plus `flat_work = loaded_bvh_leaf_count`,
visible-cell rejects, frustum rejects, submitted leaves, submitted indices, and
bucket spans. Update `render/debug_ui/mod.rs` only for Spatial tab text changes.
After implementation, update `context/lib/rendering_pipeline.md` to describe
camera cull as default flat per-leaf compute.

## Sequencing

**Phase 1 (sequential):** Task 1 — split-before-extend for oversized camera cull code.
**Phase 2 (sequential):** Task 2 — adds the new shader contract.
**Phase 3 (sequential):** Task 3 — wires the shader into the camera cull owner.
**Phase 4 (concurrent):** Task 4, Task 5 — preserve draw/debug contracts and prove equivalence.
**Phase 5 (sequential):** Task 6 — updates diagnostics and durable docs after behavior settles.

## Rough Sketch

Camera path:

```text
runtime portal traversal
-> VisibleCells
-> upload visible-cell bitmask
-> flat leaf cull compute
     leaf_index = global_invocation_id.x
     if leaf_index >= leaf_count: return
     leaf = leaves[leaf_index]
     if leaf AABB outside camera frustum: write index_count = 0, status = frustum
     else if !visible_cells[leaf.cell_id]: write index_count = 0, status = not-submitted/cyan
     else write DrawIndexedIndirect from leaf, status = rendered
-> multi_draw_indexed_indirect per material bucket
```

Likely source touch points:

- `crates/postretro/src/compute_cull.rs`
- new `crates/postretro/src/shaders/flat_leaf_cull.wgsl`
- `crates/postretro/src/render/renderer_diagnostics.rs`
- `crates/postretro/src/render/debug_ui/mod.rs`
- `context/lib/rendering_pipeline.md` after implementation

The flat shader can use a workgroup size such as 64 or 128. The exact size is an
implementation choice, but the shader must guard against explicit `leaf_count`
from flat-cull params plumbing. `arrayLength` may be an extra safety check only
if available. The current tree-walk shader remains useful as a manual
dev-tools/debug strategy while this lands.

## Open Questions

- Does the shadow cone cull show the same serial-walk bottleneck after camera
  cull improves? This plan leaves shadow migration for a follow-up.
- At much larger leaf counts, flat work may become bandwidth-bound. Keep
  diagnostics so a later plan can decide whether to reintroduce hierarchy,
  region grouping, or a hybrid threshold.
