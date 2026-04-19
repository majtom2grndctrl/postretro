# Per-Cascade BVH Cull for CSM Passes

> **Status:** draft
> **Depends on:** Milestone 4 BVH (`context/plans/done/bvh-foundation/`) — specifically `ComputeCullPipeline` in `postretro/src/compute_cull.rs` and the flat node/leaf storage format documented in `context/plans/done/bvh-foundation/2-runtime-bvh.md`. Lighting sub-plan 5 (`context/plans/in-progress/lighting-foundation/5-shadow-maps.md`) — CSM pipeline and `fit_cascade_bounds` / `cascade_ortho_matrix` in `postretro/src/lighting/shadow.rs`.
> **Related:** `postretro/src/render/shadow_pass.rs` · `postretro/src/shaders/bvh_cull.wgsl` · `postretro/src/lighting/shadow.rs` · `postretro/src/compute_cull.rs`

---

## Context

### Current state — no shadow-side culling

`ShadowResources::render_csm_passes` in `postretro/src/render/shadow_pass.rs:290–344` draws the entire world index buffer into every cascade layer:

```rust
// shadow_pass.rs:338–342
pass.set_pipeline(&self.shadow_pipeline);
pass.set_bind_group(0, &self.shadow_uniform_bind_group, &[dyn_offset]);
pass.set_vertex_buffer(0, vertex_buffer.slice(..));
pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
pass.draw_indexed(0..index_count, 0, 0..1);
```

`index_count` is the full static-world index count — no BVH cull, no frustum cull, no portal cull. This is the explicit design in sub-plan 5:

> "Draw the entire world index buffer in one indexed draw. The shadow pass does not use the per-frame BVH cull state — it renders all static geometry from the light's viewpoint." (`5-shadow-maps.md:49`)

> "No shadow-specific BVH cull. The initial cut renders all world geometry into every cascade. This is simple and correct. If profiling shows CSM passes are expensive, add per-cascade frustum culling as a follow-up — the BVH is already there." (`5-shadow-maps.md:242–243`)

With 2 directional lights × 3 cascades = 6 depth passes per frame, each drawing the full static vertex count. Sub-plan 5 flags this as "the dominant CPU cost in this sub-plan" (`5-shadow-maps.md:228`).

### Why per-cascade cull pays

The camera-frustum BVH cull (`ComputeCullPipeline::dispatch` in `postretro/src/compute_cull.rs:318`) produces a per-leaf indirect draw list scoped to the camera view. That list is the wrong set for a shadow pass — a shadow caster behind the camera still shadows into the frame — but the *shape* of the work is the same: frustum-test every leaf AABB, emit indirect draws for survivors.

Each cascade has a tight, rotation-invariant bounding volume (`fit_cascade_bounds` produces a light-space sphere + AABB; `shadow.rs:53–68, 75–166`). The near cascade covers a small slice of the world — on indoor maps, far fewer leaves than the full index buffer. Culling against that volume before drawing removes vertex throughput proportional to `(1 - visible_leaf_fraction)`.

Far cascade is the weak case: it covers most of the world by construction. The cull cost may exceed the savings on the last cascade.

---

## Goal

Add per-cascade frustum culling to the CSM pipeline. Each cascade's depth pass draws only the BVH leaves whose AABB intersects that cascade's bounding volume. Shadows stay bit-identical (goldens); CSM vertex throughput drops on test maps in rough proportion to the near cascade's volume fraction.

---

## Approach

Four tasks. A establishes the cull volume per cascade. B wires a per-cascade GPU BVH dispatch and its own indirect buffer. C rewrites the shadow draw to use indirect commands. D decides whether the far cascade gets culled at all.

```
A (cull volume)  ──────── B (GPU dispatch + indirect)  ──── C (indirect draw)  ──── D (far-cascade policy)
```

---

### Task A — Per-cascade cull volume

**Crate:** `postretro` · **File:** `src/lighting/shadow.rs`

Expose the cascade's cull volume separately from the ortho matrix. `fit_cascade_bounds` already computes the bounding sphere as a by-product (`shadow.rs:105–117`) but throws the `center, radius` away after building the AABB. Expose them.

Add a `CascadeCullVolume` struct carrying `center_world: Vec3`, `radius: f32`, and `light_dir: Vec3`. `center_world` is the unprojected frustum-slice centroid — *before* the light-view transform — so it's directly a world-space sphere. Populate it in `fit_cascade_bounds` and return it alongside `CascadeBounds` (or on `CascadeBounds` itself).

**Cull shape: capsule (sphere extended 500 units along `-light_dir`).** The bare bounding sphere at `shadow.rs:105–117` bounds only the frustum slice in world space. Shadow casters up to 500 units toward the light lie *outside* the sphere but must cast shadows into the slice — this is exactly the margin in `cascade_ortho_matrix:199` (`-bounds.max.z - 500.0`). A plain sphere is therefore unsafe as the default. The correct default is a **capsule**: the sphere swept 500 units along `-light_dir`. AABB-vs-capsule test: find the closest point on the capsule's axis segment to the AABB, then test `sq_dist(aabb_closest_point_to_that_pt, axis_pt) > radius²`. This strictly bounds every world-space AABB that can contribute fragments to the cascade depth buffer — any AABB intersecting the capsule is a potential caster, and conservativity is verified by the 500-unit margin in `cascade_ortho_matrix`.

For AABB-vs-sphere rejection: `sq_dist(aabb_closest_point(sphere_center), sphere_center) > radius²` culls. For the capsule, clamp the sphere center projection onto the 500-unit axis segment to get the nearest axis point, then apply the same test.

**Considered and rejected:** plain sphere (unsafe — misses casters behind the slice); 6-plane light-space frustum (correct but more expensive and more complex to implement; keep as fallback if capsule causes unexpected artifacts).

---

### Task B — Per-cascade GPU BVH dispatch

**Crate:** `postretro` · **Files:** `src/compute_cull.rs`, `src/shaders/bvh_cull.wgsl`, new `src/shaders/bvh_cull_shadow.wgsl` (if needed)

Current `ComputeCullPipeline` has one fixed indirect buffer sized to `total_leaves` (`compute_cull.rs:152–160`) and one cull uniforms buffer with 6 planes (`compute_cull.rs:66–69, 140–146`). Both are single-shot.

**Option B1: reuse the compute pipeline with per-cascade dispatches.** Allocate `CSM_TOTAL_LAYERS` (= 6) extra indirect buffers, each sized `total_leaves * DRAW_INDIRECT_SIZE`. Allocate `CSM_TOTAL_LAYERS` extra cull uniforms buffers. For each cascade: write its cull volume, dispatch `cull_main`. **Every leaf workgroup invocation must write its indirect slot** — writing `index_count = 0` on reject. This matches the existing `ComputeCullPipeline` invariant (no `encoder.clear_buffer` needed; the shader owns the zeroing).

**Option B2: new shader that takes a capsule.** `bvh_cull.wgsl:67–82` has `is_aabb_outside_frustum` hardcoded to 6 planes. A capsule test is cheaper and matches Task A's volume. Fork the shader: `bvh_cull_shadow.wgsl` with `is_aabb_outside_capsule(center, radius, light_dir)` as the reject test. Share node/leaf bindings. Same every-leaf-writes invariant applies.

Prefer B2 for the near cascades (tighter rejection); keep B1 available if the capsule approximation causes visible artifacts.

**Shadow cull ignores the cell bitmask.** Portal visibility is camera-relative. Shadow passes must draw off-camera casters, so `visible_cells` binding is not meaningful. Either (a) pass a bitmask of all-ones (matches `VisibleCells::DrawAll` — `compute_cull.rs:309–313`), or (b) strip the cell check from `bvh_cull_shadow.wgsl`. Prefer (b) — one branch fewer per leaf.

**Memory cost.** `total_leaves * 20 bytes * 6 cascades`. A 10k-leaf map costs 1.2 MB of indirect buffer per frame. Acceptable.

---

### Task C — Indirect shadow draw

**Crate:** `postretro` · **File:** `src/render/shadow_pass.rs`

Replace the single `draw_indexed` at `shadow_pass.rs:342` with `multi_draw_indexed_indirect` (or the fallback loop) against the cascade's dedicated indirect buffer. The shadow depth-only pipeline layout (`shadow_pass.rs:182–186`) already binds only group 0 (the dynamic-offset uniform); no texture bind group. This matches the `set_texture_fn: None` case in `ComputeCullPipeline::draw_indirect` (`compute_cull.rs:416–444`) — the depth pre-pass path already supports it.

The depth-only shader reads only position (`shadow_pass.rs:166–174`). No per-bucket work is needed inside the shadow pass; the existing bucket iteration in `draw_indirect` does the right thing even when bucketing is semantically irrelevant to depth output — as the comment at `compute_cull.rs:411–415` notes.

Refactor `render_csm_passes` to:
1. For each cascade: pick the matching cascade indirect buffer (produced by Task B's compute dispatch).
2. Begin the cascade's render pass as today (`shadow_pass.rs:324–336`).
3. Bind the shadow uniform with dynamic offset, bind vertex+index buffers as today.
4. Instead of `draw_indexed`, call `draw_indirect` / `multi_draw_indexed_indirect` against that cascade's indirect buffer.

Ordering: the compute dispatches for all cascades must run before the first shadow render pass (buffer barrier semantics — wgpu handles it via usage flags). Encode all shadow cull dispatches first, then all shadow render passes.

---

### Task D — Far-cascade policy

**Crate:** `postretro` · **File:** `src/render/shadow_pass.rs` (policy lives next to the dispatch loop)

The far cascade's bounding sphere may cover ≥50% of the level's AABB. BVH traversal at that density rejects few leaves; the cull dispatch may cost more than it saves. Options:

1. **Always cull every cascade.** Simplest; accept the potential loss on the far cascade.
2. **Skip cull on the last cascade.** Fall back to the current `draw_indexed(0..index_count)` path for cascade index `CSM_CASCADE_COUNT - 1`. Saves one compute dispatch and one indirect buffer allocation per directional light. Risks: if the far cascade's volume is small on a given map (tiny level), we waste throughput.
3. **Runtime heuristic (preferred).** At dispatch time, test whether the cascade's capsule AABB intersects less than 50% of the global BVH root AABB volume. If yes, run the cull dispatch. If no (cascade covers ≥50% of the world), skip the dispatch and draw everything — same as today. This saves compute on the far cascade when it covers the whole world, while still culling when it doesn't. Level AABB is known at load time from the BVH root node.

**Decision: use option 3.** This is the implementation target for Task D.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/lighting/shadow.rs` | A | Expose `CascadeCullVolume` from `fit_cascade_bounds`; world-space center and radius from the existing bounding-sphere computation |
| `postretro/src/compute_cull.rs` | B | Per-cascade indirect buffers, per-cascade uniforms buffers; `dispatch_shadow` entry that takes a cull volume and target cascade slot |
| `postretro/src/shaders/bvh_cull_shadow.wgsl` (new) | B | Capsule-reject variant of `bvh_cull.wgsl`; no visible-cell binding |
| `postretro/src/render/shadow_pass.rs` | C, D | Replace full-buffer `draw_indexed` with indirect draw against cascade's indirect buffer; encode all compute dispatches before all render passes; far-cascade runtime heuristic (option 3) |

---

## Acceptance

1. CSM shadow depth output is **bit-identical** to the pre-change golden render. Because the capsule is a strict superset of the current draw set (it only culls; it adds nothing), no shadow caster present today is removed. Verify with a pixel-exact diff of depth buffer dumps before/after.
2. No missing shadows and no extra draws beyond the capsule's conservatism on all test maps.
3. On `test.prl` (or any map with meaningful occlusion), the near cascade's per-frame shadow-pass vertex throughput drops measurably — target ≥30% reduction in the near cascade's submitted index count. Verify via a trace log summing `indirect_draws[leaf].index_count` for the cascade's survivors.
4. `cargo test -p postretro` passes.
5. `cargo clippy -p postretro -- -D warnings` clean.
6. No new `unsafe`.
7. GPU timing (`POSTRETRO_GPU_TIMING=1`) shows the sum of CSM pass times does not regress on small maps (where cull overhead could dominate) and drops on larger maps.

---

## Out of scope

- Cascade count changes (stays at 3; sub-plan 5 allows a 4th as a follow-up, not here).
- Cascade fit algorithm changes (`fit_cascade_bounds` stays as-is; this plan only exposes existing intermediates).
- CPU-side BVH walk optimizations — the SH baker's BVH traversal (sibling plan `perf-per-region-bvh`).
- Partitioning the BVH itself — see sibling `perf-per-region-bvh`.
- SDF shadow path (sub-plan 8 / 9) — separate pipeline, separate plan.
- Shadow caching across frames.
- Cube or spot shadow maps — Postretro doesn't render them.
- Dynamic shadow casters — static world only.

---

## Open Questions

1. **Reuse compute pipeline or fork?** Task B1 vs. B2. The shader fork (B2) is cleaner (no bitmask plumbing, no visible-cell branch) but adds a second shader to maintain. Compare shader source size before committing.
2. **GPU timing budget.** Before landing, measure: is the total CSM cull compute time (6 dispatches) smaller than the shadow-pass vertex-throughput savings on the target test maps? If not, fall back to culling only the nearest 1–2 cascades.

---

### Decided

- **Cull volume shape:** capsule (sphere extended 500 units along `-light_dir`). Plain sphere is unsafe (misses casters behind the slice). 6-plane frustum is fallback only. See Task A.
- **Far-cascade policy:** runtime heuristic (option 3) — skip cull when cascade capsule AABB covers ≥50% of the BVH root AABB. See Task D.
- **Boundary casters:** AABB-capsule intersection handles them correctly; covered by test-coverage requirement in acceptance item 2.
