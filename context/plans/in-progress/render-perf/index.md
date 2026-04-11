# Renderer Frame-Loop Performance

> **Status:** ready
> **Depends on:** none. All changes are in the `postretro` crate. No level compiler, PRL format, or engine interface changes required.
> **Related:** `postretro/src/portal_vis.rs` · `postretro/src/render.rs` · `postretro/src/visibility.rs` · `postretro/src/main.rs` · `context/plans/drafts/input-perf/`

---

## Context

Line-by-line audit of the render hot path surfaced three categories of unnecessary per-frame work:

1. **Portal traversal allocations.** `flood()` in `portal_vis.rs` clones the current-chain path `Vec` at every recursive hop (lines 291–293). A block comment at lines 285–290 acknowledges that push/pop backtracking would be equivalent and cheaper, and explicitly invites the switch as a profile-driven follow-up. `clip_polygon_to_frustum` allocates two temporary `Vec<Vec3>` buffers per portal per frame (lines 319–320).

2. **Redundant work in the draw submission pass.** `collect_visible_leaf_indices` runs twice per frame when the wireframe overlay is active — once in `draw_visible_textured` (line 892), once in `draw_visible_wireframe` (line 938) — performing an identical scan over the same input. Inside those helpers, `set_bind_group` is called per draw call with no deduplication (line 904), so adjacent sub-ranges that share a texture redundantly re-bind. `build_uniform_data` (lines 94–105) allocates an 80-byte `Vec<u8>` every frame just to feed a queue write.

3. **Per-frame visibility allocations.** `visibility.rs` has two separate per-frame entry points, one per world type:
   - **`collect_visible_faces`** at line 267 (BSP path, called from `determine_visibility:353`, which is reached from `main.rs:499`). Allocates `Vec::new()` at line 273 and iterates `leaf.face_indices` twice per visible leaf — once to count non-zero faces into `pvs_face_count` (lines 285–294), once to push `DrawRange`s (lines 303–312).
   - **`determine_prl_visibility`** at line 434 (PRL path, called directly from `main.rs:501`). Allocates a fresh `Vec::new()` in each of four branches: solid-leaf fallback (467), portal path (519), no-PVS fallback (580), PVS path (617). The portal path and PVS path also double-iterate `face_meta.iter().skip(start).take(count)` per visible leaf (534–550 and 636 onwards). In the PVS branch, the face-count scan runs *before* the AABB-frustum cull, so culled leaves pay for a full face iteration and throw the result away.

None of these cause visible sluggishness on the current test map. Category 1 scales with portal chain depth; category 3 scales with face count and leaf count; category 2 is pure redundancy removal.

**Methodology note.** Every finding was identified by source audit, not by profiling. These are high-confidence allocation and redundancy patterns in per-frame paths where the fixes are cheap enough that measured evidence is not needed to justify them. Task A in particular takes up the explicit invitation from the in-code comment at `portal_vis.rs:285–290` ("Profile-driven follow-up can switch to push/pop or `SmallVec` if allocation shows up") on the strength of static analysis rather than a profiling run. The shortcut is acceptable because the fix is small, localized, and reversible.

A companion plan (`input-perf`) covers per-frame allocations outside the render path — input polling and the window title diagnostic — which are small enough to live separately.

---

## Goal

Eliminate identified per-frame allocations and redundant CPU work in the render loop without changing any observable rendering behavior or public interfaces. `cargo test -p postretro` passes unchanged throughout.

---

## Approach

Four tasks. Task A is file-isolated and parallel with everything. Tasks B and C both touch `render.rs` and ship bundled. Task D touches `visibility.rs` plus the `main.rs` call site where the scratch buffer is owned; schedule after B to avoid churn on the draw-helper signatures.

```
A (portal_vis.rs)        ────────────────────────── merge

B+C (render.rs)          ─── merge ─── D (visibility.rs + main.rs) ── merge
```

---

### Task A — Portal traversal: push/pop path tracking + clip scratch buffers

**Crate:** `postretro` · **File:** `src/portal_vis.rs`

**Problem 1: path Vec clone per hop.** The `flood` recursive helper clones the path `Vec` at every hop:

```rust
// portal_vis.rs:291–293
let mut next_path = Vec::with_capacity(path.len() + 1);
next_path.extend_from_slice(path);
next_path.push(portal_idx);
flood(state, neighbor, &narrowed, &next_path);
```

At a chain depth of 10 with a branching factor of 4, this is ~40 allocations per leaf reached in the current frame.

**Fix: push/pop backtracking.** Change `path: &[usize]` to `path: &mut Vec<usize>` throughout `flood` and its callers. Before each recursive call, push the portal index; after the call returns, pop it. The Vec is allocated once at the top of `portal_traverse` and reused for the entire traversal.

```rust
// After change — path is &mut Vec<usize>
path.push(portal_idx);
flood(state, neighbor, &narrowed, path);
path.pop();
```

The cycle-detection membership test (`path.contains(&portal_idx)`) remains a linear scan — correct and fast for realistic chain depths (5–20).

**Comment update.** The block comment at `portal_vis.rs:285–290` defers push/pop as a "profile-driven follow-up." When this fix lands, replace that comment with a brief note describing the new behavior ("Reuses a single path Vec via push/pop backtracking."). Do not leave the follow-up language in place after the follow-up ships.

**Problem 2: clip scratch buffers.** `clip_polygon_to_frustum` allocates two `Vec<Vec3>` per call:

```rust
// portal_vis.rs:319–320
let mut input: Vec<Vec3> = polygon.to_vec();
let mut output: Vec<Vec3> = Vec::with_capacity(polygon.len() + frustum.planes.len());
```

**Fix.** Add a `scratch: (&mut Vec<Vec3>, &mut Vec<Vec3>)` parameter to `clip_polygon_to_frustum`. The caller clears and reuses them across calls. Replace `polygon.to_vec()` with `scratch.0.clear(); scratch.0.extend_from_slice(polygon)`. Allocate the two scratch Vecs once at the top of `portal_traverse` and thread them down through `flood` into each `clip_polygon_to_frustum` call.

**Signature note.** `clip_polygon_to_frustum` is `pub(crate)`. All call sites are within `portal_vis.rs` and `portal_vis::tests`. Update all of them.

---

### Task B — Render loop: compute visible leaves once per frame

**Crate:** `postretro` · **File:** `src/render.rs`

**Problem.** `draw_visible_textured` (line 892) and `draw_visible_wireframe` (line 938) both call `self.collect_visible_leaf_indices(ranges)`. When the wireframe overlay is active, this is two full scans over identical input producing identical output.

**Fix.** Lift the call out of both helpers and into their shared caller — the dispatcher that matches on `VisibleFaces::Culled(ranges)` and then invokes the textured and wireframe paths. Pass the computed `Vec<usize>` to both helpers as a `&[usize]` parameter, replacing the internal calls. `draw_visible_textured` and `draw_visible_wireframe` receive `visible_leaves: &[usize]` instead of deriving it themselves.

Pure refactor. Ships bundled with Task C (both in `render.rs`).

---

### Task C — Render loop: bind group dedup + stack-alloc uniform data

**Crate:** `postretro` · **File:** `src/render.rs` · **Bundle with:** Task B

Two independent `render.rs` fixes grouped because they live in the same file.

**Fix 1: bind group deduplication.** In `draw_visible_textured` (line 904), `set_bind_group(1, bind_group, &[])` is called for every draw call regardless of whether the texture changed. Track the last-set texture index in a local `Option<usize>` and call `set_bind_group` only when `tex_idx` differs from the previous iteration:

```rust
let mut last_tex: Option<usize> = None;
for leaf_idx in visible_leaves {
    for sub_range in sub_ranges {
        let tex_idx = sub_range.texture_index as usize;
        if last_tex != Some(tex_idx) {
            render_pass.set_bind_group(1, &self.gpu_textures[tex_idx].bind_group, &[]);
            last_tex = Some(tex_idx);
        }
        render_pass.draw_indexed(...);
    }
}
```

The `last_tex` state persists across the outer leaf loop, so cross-leaf transitions where leaf N's last texture matches leaf N+1's first texture also get deduped.

**Note on sub-range order.** Per-leaf sub-ranges are already sorted by `texture_index` by construction at load time: `build_leaf_texture_sub_ranges` in `prl.rs:228` groups consecutive same-texture faces into sub-ranges, and the documented invariant (`prl.rs:227` — "Assumes the index buffer is already sorted by (leaf_index, texture_index)") guarantees same-texture faces within a leaf are contiguous. No load-time sort is needed — the existing invariant makes within-leaf dedup fire automatically. Cross-leaf dedup is map-dependent and also needs no sort.

**Fix 2: stack-alloc uniform data.** `build_uniform_data` (lines 94–105) allocates an 80-byte `Vec<u8>` every frame:

```rust
fn build_uniform_data(view_proj: &Mat4, ambient_light: [f32; 3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(UNIFORM_SIZE);  // 80 bytes every frame
    ...
}
```

Replace the `Vec<u8>` return with a `[u8; UNIFORM_SIZE]` stack array, or build the bytes directly into a `#[repr(C)]` struct and `bytemuck::cast_slice(&[data])` at the `queue.write_buffer` call site. Zero heap traffic for the view-projection uniform.

---

### Task D — Visibility: scratch buffer + merge face iterations across both visibility entry points

**Crate:** `postretro` · **Files:** `src/visibility.rs`, `src/main.rs` · **After:** Task B

**Scope.** Two separate per-frame entry points in `visibility.rs` need the same treatment. They're related by role (per-frame visibility determination) and by allocation pattern, not by file position:

- **`collect_visible_faces`** at `visibility.rs:267` — the BSP-path helper. Takes `&BspWorld`. Called from `determine_visibility:353`, which is reached at `main.rs:499`.
- **`determine_prl_visibility`** at `visibility.rs:434` — the PRL-path entry point. Takes `&LevelWorld`. Called directly from `main.rs:501`.

Both allocate fresh `Vec<DrawRange>` per frame and double-iterate face metadata per visible leaf. All fixes follow the same shape: receive a scratch `Vec<DrawRange>` from the caller, and merge double loops into single passes.

**Fix 1: scratch `Vec<DrawRange>` owned by `App`.** Add a `scratch_ranges: Vec<DrawRange>` field on the winit `ApplicationHandler` struct (`App`) in `main.rs`. Initialize empty at construction. Pass `&mut self.scratch_ranges` into whichever visibility entry point the per-frame match arm at `main.rs:497–517` selects. Both `collect_visible_faces` and `determine_prl_visibility` accept a `scratch: &mut Vec<DrawRange>` parameter, call `scratch.clear()` on entry, and push into it instead of a local. Because BSP and PRL cannot be active on the same frame, a single scratch Vec is sufficient.

The return-type reshape — whether `CollectedFaces` / `VisibleFaces::Culled` continues to wrap an owned `Vec<DrawRange>` or is restructured to reference the scratch via lifetimes — is an implementation judgment call. The invariant is that no `Vec::new()` or `Vec::with_capacity` for `DrawRange` runs in the per-frame call path.

**Fix 2: BSP path — merge count and push loops.** `collect_visible_faces` iterates `leaf.face_indices` twice per visible leaf: once at lines 285–294 to count non-zero faces into `pvs_face_count`, once at lines 303–312 to push `DrawRange`s. Merge into a single loop, accumulating both counters simultaneously. Preserve the existing semantic that `pvs_face_count` reflects the count *before* frustum culling — the BSP path applies its AABB cull (lines 297–301) after the count by design. Do not reorder the count and the cull on this path.

**Fix 3: PRL portal path — merge count and push loops.** `determine_prl_visibility` at lines 534–550 does the same double iteration over `face_meta.iter().skip(start).take(count)`. Merge into a single loop. The portal path has no separate AABB cull — portal traversal already clips against narrowed frustums — so there is no cull step to reorder.

**Fix 4: PRL PVS path — reorder and merge.** Lines 636 onwards have the double iteration plus a subtle pessimization: the face count runs *before* the AABB-frustum test, so leaves that end up culled pay for a full face scan whose result is discarded. Two-step fix:

1. Move the AABB-frustum test ahead of any face iteration in the per-leaf block.
2. Merge the count and push loops into a single pass, running only on leaves that survive the cull.

`raw_pvs_faces` is already computed separately by `raw_pvs_face_count` at line 503, so `pvs_faces` here is the post-cull counter and the reorder is safe.

**Fix 5: PRL solid-leaf fallback and no-PVS fallback.** The solid-leaf branch at line 467 and the no-PVS branch at line 580 each allocate their own `Vec::new()` for `ranges`. Both flow through the shared `scratch` parameter introduced by Fix 1. Neither contains a double iteration or a count-before-cull, so no loop restructuring is needed — just the scratch buffer swap.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/portal_vis.rs` | A | Push/pop path tracking; scratch buffers for `clip_polygon_to_frustum`; comment update |
| `postretro/src/render.rs` | B, C | Lift `collect_visible_leaf_indices` call; bind group dedup; stack-alloc uniform data |
| `postretro/src/visibility.rs` | D | Scratch parameter on `collect_visible_faces` and `determine_prl_visibility`; merge count+push loops in BSP path, PRL portal path, and PRL PVS path; reorder cull ahead of count in PVS branch |
| `postretro/src/main.rs` | D | Add `scratch_ranges: Vec<DrawRange>` field on `App`; pass into both visibility entry points at lines 499 and 501 |

---

## Acceptance Criteria

1. `cargo test -p postretro` passes after each task lands. No test count regression. No new tests required; the existing test suite exercises the changed paths.
2. No new `unsafe` blocks.
3. **Task A:** `flood` and `clip_polygon_to_frustum` no longer allocate in their per-frame call path. Verification: read the final function bodies and confirm the `Vec<usize>` path and both `Vec<Vec3>` clip buffers are threaded through as `&mut` parameters from `portal_traverse`. The "profile-driven follow-up" comment is replaced with a brief description of the new scheme.
4. **Task B:** When the wireframe overlay is active, `collect_visible_leaf_indices` runs exactly once per frame. Verification: add a temporary counter or trace log, observe a single call per `RedrawRequested`, remove before merge.
5. **Task C Fix 1:** On a single-texture map, zero redundant `set_bind_group` calls after the first. On a multi-texture map, bind group sets equal the number of distinct texture transitions in the visible draw list, not the number of draw calls. Verification: trace log in the dedup branch counting skipped vs. executed sets, removed before merge. **Fix 2:** `build_uniform_data` returns a stack type; no heap allocation occurs in its body (read the final signature and body to confirm).
6. **Task D:** Per-frame execution of `collect_visible_faces` and `determine_prl_visibility` performs no `Vec<DrawRange>` allocation — verify by reading each final function body and confirming all `ranges` references resolve to the `scratch` parameter. Face iteration (`leaf.face_indices` in the BSP path, `face_meta.iter().skip(start).take(count)` in the PRL path) runs at most once per visible leaf at all double-iteration sites (BSP 285/303, PRL portal 534/543, PRL PVS 636+). In the PRL PVS branch, leaves culled by the AABB-frustum test do not enter any face iteration loop. The `App` struct holds a persistent `scratch_ranges: Vec<DrawRange>` field passed into both visibility entry points.
7. Visual output is identical before and after all tasks. Spot-check `test.prl` and any other available maps in both textured and wireframe modes.
8. Framerate on the test map does not regress. This is expected to improve or hold steady; any regression indicates a logic error, not an optimization cost.

---

## Out of scope

- **The O(L × R) overlap scan in `collect_visible_leaf_indices` at `render.rs:978–983`.** Task B halves its cost by only running it once per frame instead of twice, but the per-frame cost inside the function remains O(L × R) because the inner `ranges.iter().any(...)` scan is per-leaf. Eliminating the L × R factor requires either tagging each emitted `DrawRange` with its source leaf (so this function becomes an O(R) walk with a visited-set) or a sort-merge formulation over precomputed leaf extents. Both are design decisions beyond this plan's scope. The per-leaf span derivation at lines 972–976 is already O(1) via `first()/.last()` and does not need further work.
- Allocator tuning or `SmallVec` adoption (Task A's push/pop already removes the need for `SmallVec` on the path Vec).
- Parallelizing portal traversal across leaves.
- Moving visibility computation to the GPU.
- Any change to the level compiler, PRL format, or engine load interface.
- Input polling and window title allocations — see `context/plans/drafts/input-perf/`.
- Exterior leaf geometry packing — see `context/plans/ready/exterior-leaf-cull/`.

---

## Open Questions

None. Promote to ready when scheduled.
