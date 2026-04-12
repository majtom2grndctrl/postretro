# Phase 3.5 — Rendering Foundation Extension

> **Status:** draft — ready for review. No blocking research; decisions are locked in `context/lib/rendering_pipeline.md`.
> **Phase:** 3.5 — inserted between shipped Phase 3 (Textured World) and Phase 4 (Lighting Foundation). See `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §3, §6, §7, §8 · `context/lib/build_pipeline.md` §PRL Compilation · `context/plans/drafts/phase-4-baked-lighting/`
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5. This plan is ephemeral — durable decisions already live in `context/lib/rendering_pipeline.md` §6 and §7. Ship, then delete.

---

## Goal

Bring the rendering architecture up to the target clustered forward+ pipeline (GPU-driven indirect draws, per-cell chunks, HiZ occlusion culling, vertex format with packed normals and tangents) *without* touching lighting. Phase 4 layers SH irradiance volume sampling, normal-map shading, dynamic direct lights, and shadow maps onto the foundation this phase delivers.

The split exists to isolate architectural risk. Lighting changes the visual output; Phase 3.5 changes only the rendering architecture beneath it. A working Phase 3.5 should look identical to Phase 3 — same textures, same flat ambient — while proving the new draw-dispatch and culling paths.

---

## Context

Phase 3 ships textured world rendering with flat uniform lighting. Draw calls are issued per-leaf × texture pair from the CPU, using a vertex format of position + UV + per-vertex color. Visibility is computed per frame via portal traversal (`context/lib/rendering_pipeline.md` §2).

The target pipeline (`context/lib/rendering_pipeline.md` §4, §7) is clustered forward+ with GPU-driven indirect draws, HiZ occlusion, and a vertex format that carries packed normals and tangents for normal mapping. The lighting stages on top of that foundation live in Phase 4.

This phase delivers the foundation — everything Phase 4 needs from the rendering and geometry pipeline, with no lighting semantics changed.

---

## Scope

### In scope

- **Vertex format upgrade.** Extend the `postretro-level-format` Geometry section (or introduce `GeometryV3`) to carry packed normals (octahedral `u16 × 2`) and packed tangents (octahedral `u16 × 2` plus bitangent sign bit) per vertex. Remove per-vertex color. prl-build generates normals from face planes and tangents from UV projection axes during brush-side projection.
- **Per-cell draw chunks.** Replace per-leaf draw batching with per-portal-cell chunks. Each chunk owns a contiguous index range and an AABB, keyed by cell id. prl-build emits the chunk table; engine loader reads it and hands it to the renderer.
- **GPU-driven indirect draw path.** A compute prepass consumes the visible cell list from portal traversal, runs per-cell frustum culling (and HiZ when available), and writes `draw_indexed_indirect` commands into a GPU buffer. The main render pass issues a single `multi_draw_indexed_indirect` call. CPU no longer issues per-cell draws.
- **HiZ depth pyramid.** A compute pass builds a hierarchical-Z pyramid from the previous frame's depth buffer each frame. The cell-culling compute pass tests each surviving cell's AABB against it before emitting draws. First frame skips HiZ (pass-all).
- **World shader updated for new vertex format.** Fragment shader still applies flat ambient — no lighting changes. Vertex shader reads the new attributes and passes world-space normal and tangent through to the fragment stage so Phase 4 can plug in.

### Out of scope

- Normal map sampling (Phase 4).
- SH irradiance volume sampling (Phase 4).
- Dynamic direct lighting (Phase 4).
- Shadow maps (Phase 4).
- Clustered light list compute prepass (Phase 4).
- Mesh shaders. wgpu does not expose them; compute + indirect draws cover the GPU culling role.
- Hardware ray tracing. Not in baseline wgpu.
- Pipeline cache persistence (shader compile-time mitigation). If Phase 3.5 surfaces shader compile stutter on first load, file as a follow-up.
- Billboard sprites, emissive surfaces, fog. Phase 5.

---

## Design

### Vertex format

Custom vertex format lives in `context/lib/rendering_pipeline.md` §6 — this plan implements it.

Per vertex:
- Position: `f32 × 3`, world-space meters, Y-up.
- UV: `f32 × 2`, texture-space.
- Normal: `u16 × 2`, octahedral-encoded.
- Tangent: `u16 × 2`, octahedral-encoded. The most-significant bit of the first component carries the bitangent sign.

Total: **24 bytes per vertex** (`12 + 8 + 4 + 4`).

Comparison to Phase 3:

| | On-disk (PRL) | Runtime (GPU vertex buffer) |
|---|---|---|
| Phase 3 | `GeometryV2`: 20 bytes (f32 × 5, position + UV) | `TexturedVertex`: 36 bytes (position + UV + RGBA vertex color) |
| Phase 3.5 | `GeometryV3`: 24 bytes (position + UV + oct normal + oct tangent) | `TexturedVertex`: 24 bytes (same layout, direct upload) |

On-disk grows 4 bytes/vertex (~400 KB on a 100k-vertex level); runtime shrinks 12 bytes/vertex as the 16-byte RGBA vertex color is displaced. Net GPU-side win. The vertex color was a Phase 3 placeholder for per-vertex lighting, replaced by per-fragment shading (§4) — removing it now avoids shipping a dead attribute.

**Octahedral encoding** packs a unit vector into two components by mapping the sphere to an octahedron and unfolding. Reconstruction is four multiplies and an abs per component. Accuracy is better than spherical coordinates at the same bit depth and fits cleanly into a `u16` pair.

**Bitangent sign** lives in one bit of the tangent encoding so the vertex shader can rebuild the bitangent as `cross(normal, tangent) * sign` and assemble the full TBN matrix. This avoids storing the bitangent explicitly.

**Tangent generation** runs in prl-build during brush-side projection. For each face, the tangent is derived from the face's UV `s`-axis projected onto the face plane and renormalized. The bitangent sign is determined by comparing `cross(normal, tangent)` against the `t`-axis — a mirrored UV flips the sign. This matches MikkTSpace convention closely enough for world geometry; full MikkTSpace is overkill for brush faces that never share vertices across materials.

### Per-cell draw chunks

Each portal cell owns a contiguous range of indices in the shared index buffer. The chunk table records, per cell:

- Cell id
- Index buffer offset
- Index count
- Material bucket id (which albedo/normal pair)
- World-space AABB (for GPU culling)

Today's per-leaf grouping already batches geometry by leaf; this phase re-keys grouping by **portal cell** (which in the current implementation is equivalent to an empty BSP leaf — the portal graph's nodes). The change is structural, not semantic: cells are the natural grouping unit for GPU culling because portal traversal already produces a visible-cell set.

prl-build emits the chunk table as a new PRL section. The loader reads it and hands the raw chunk table to the renderer alongside the vertex and index buffers.

**PRL section ID allocation.** Current highest allocated ID is `TextureNames = 16` (see `postretro-level-format/src/lib.rs`). Phase 3.5 claims the next two:

- `GeometryV3 = 17` — position + UV + packed normal + packed tangent vertex format. Supersedes `GeometryV2 = 3`.
- `CellChunks = 18` — per-cell draw chunk table.

Phase 4's SH irradiance volume will claim `19` when it ships. The `LightProbes = 5` reserved-ID comment in `lib.rs` predates the SH spec and should be removed when Phase 3.5 lands (it would mislead a code search for the new SH section).

**Material bucketing.** Within a cell, indices are further ordered by material bucket so a single cell produces a small number of draws (one per material it touches). This preserves the benefit of Phase 3's per-leaf × texture grouping without needing to emit millions of tiny draws. A typical cell in a retro FPS map touches 2–10 materials; that's the indirect draw count per visible cell.

### GPU-driven indirect draw path

Each frame:

1. **CPU portal traversal** produces a visible cell id list (§2 of `rendering_pipeline.md`, unchanged).
2. **Compute: cell culling.** Kernel reads the visible cell list, tests each cell's AABB against the frustum and the HiZ pyramid, and for each surviving cell emits one `DrawIndexedIndirect` command per material bucket the cell touches. Commands land in a GPU indirect buffer. A draw count counter tracks how many commands were written.
3. **Render pass: world geometry.** Renderer issues `multi_draw_indexed_indirect` against the indirect buffer and count. Vertex + index buffers are bound once; the indirect command supplies per-draw index offsets, counts, and instance ids.

**Material bind groups.** The material bucket id indexes into a material array uploaded once at level load. Two options exist for binding:

- **Bindless-style.** All materials in a single array texture (or bindless descriptor array where supported). Shader selects by bucket id read from a storage buffer. Single bind group for the whole pass.
- **Per-material bind groups.** Each bucket has its own bind group; the indirect pass can't switch bind groups, so emit a separate indirect call per bucket (still GPU-driven, just many dispatches).

wgpu's current baseline does not expose bindless descriptor arrays uniformly across backends. The plan is **per-material indirect calls** — acceptable because bucket counts are small (10–50 per level), and each call is a single `multi_draw_indexed_indirect` over the subset of the indirect buffer belonging to that bucket. This can be revisited if wgpu exposes bindless, or collapsed into texture arrays as a simpler alternative.

**Indirect buffer sizing.** Upper bound is `(visible_cells × max_buckets_per_cell)`. Concrete cap: 65,536 draws per frame. Compute shader refuses to write past the cap and logs once.

### HiZ depth pyramid

Standard hierarchical-Z occlusion culling:

1. **Build.** Compute kernel downsamples the previous frame's depth buffer into a mip chain. Each mip texel stores the max depth of the four texels in the level below (max because reversed-Z is not in use; a farther depth means more likely occluded). Mip count matches the depth buffer's log₂ dimensions.
2. **Test.** In the cell-culling compute shader, each cell's AABB is projected to screen space to produce a 2D rectangle plus a nearest-Z value. The shader picks the HiZ mip level whose texel size covers the rectangle in one sample, reads the max depth there, and compares against the cell's nearest Z. If the cell's nearest Z is farther than the HiZ max, the cell is occluded — skip.

**Temporal coherence.** Using the previous frame's depth means a newly-visible cell might miss one frame of occlusion testing and be drawn once as an over-draw. That's acceptable — the draw is the correct one, just mildly wasteful. No popping.

**First frame.** The depth buffer from the "previous frame" doesn't exist on the first frame or after a level load. Bind a pass-all HiZ (cleared to farthest depth) so every cell survives.

**Depth buffer format.** Must be readable as a storage resource by the HiZ compute kernel. wgpu supports depth buffer copies; the simplest path is to copy the depth texture into a `r32float` storage texture after the opaque pass ends, then run the HiZ build kernel on that copy.

---

## Data flow

```
prl-build                                                       runtime
─────────                                                       ───────

.map parse                                              PRL loader
  ↓                                                       ↓
brush-volume BSP                                    vertex / index buffers
  ↓                                                       ↓
brush-side projection ──── emits per-vertex      renderer GPU upload
  ↓                         normal + tangent            ↓
portal generation                                per-cell chunk table
  ↓                                                       ↓
per-cell chunk grouping                          ┌────────────┴────────────┐
  ↓                                              ↓                         ↓
pack ─── emits cell chunk section     portal traversal       previous-frame depth
                                              ↓                         ↓
                                       visible cell list       HiZ pyramid build (compute)
                                              ↓                         ↓
                                              └─────────→ cell culling (compute)
                                                                         ↓
                                                           indirect draw buffer
                                                                         ↓
                                                  multi_draw_indexed_indirect
                                                                         ↓
                                                               world shader
                                                                 (flat ambient)
```

---

## Migration strategy

The PRL format is pre-stable (`context/lib/index.md` orientation). Breaking the Geometry section is acceptable.

**Plan A: Bump `GeometryV2` → `GeometryV3`.** Add a new section id for the expanded vertex format. prl-build writes the new section; old `.prl` files built with GeometryV2 fail to load with a clear version error. This matches the existing precedent (`GeometryV2` replaced `Geometry`).

**Plan B: Extend GeometryV2 in-place with a format flag.** More complex, buys nothing given no shipped `.prl` files need to survive.

Go with Plan A. Old test maps get recompiled.

Existing Phase 3 per-leaf draw batching code is replaced, not dual-coded. The flat-ambient shader is the only part of the render path that carries forward semantically unchanged — the vertex format and draw dispatch underneath it are rewritten.

---

## Acceptance criteria

1. **Vertex format.** New `GeometryV3` section (or equivalent) carries position, UV, packed normal, packed tangent per vertex. Round-trip tests in `postretro-level-format` cover write → read fidelity, including octahedral encoding precision bounds.

2. **Tangent generation.** prl-build produces tangents from UV projection axes for every vertex. Tangent-space basis is orthonormal (within floating-point tolerance) and the bitangent sign is consistent across a face.

3. **Per-cell chunk table.** prl-build emits one chunk entry per (cell, material bucket) pair with AABB, index offset, index count. Loader reads the table; renderer consumes it.

4. **GPU-driven indirect path.** Compute culling kernel writes to an indirect draw buffer each frame; render pass issues `multi_draw_indexed_indirect`; CPU issues zero per-cell draws during the opaque pass.

5. **HiZ pyramid.** Previous-frame depth is downsampled to a max-depth pyramid via a compute kernel each frame. Cell-culling kernel samples the appropriate mip level and rejects occluded cells.

6. **Visual parity with Phase 3.** On all existing test maps, Phase 3.5 produces output visually indistinguishable from Phase 3 (flat ambient, same textures, same geometry). Tolerance is perceptual: SSIM ≥ 0.99 against a Phase 3 reference capture, or maximum per-channel per-pixel diff ≤ 2/255, whichever is easier to automate. Strict bit-for-bit equality is not required — the vertex format change reorders floating-point math through the vertex shader and small diffs are expected; the acceptance bar is "a human reviewer cannot tell the two apart on a side-by-side."

7. **Visibility subsystem unchanged.** §2 of `rendering_pipeline.md` is untouched. Portal traversal and every fallback path (`SolidLeafFallback`, `ExteriorCameraFallback`, `NoPvsFallback`, `EmptyWorldFallback`) still produce a visible cell list. The compute cell-culling pass consumes that list blind to which path produced it.

8. **Perf at least on par.** On a cell-heavy test map (20+ visible cells), GPU frame time is within 5% of Phase 3 or better. This isn't a perf win phase — the architectural change pays off in Phase 4 when per-fragment clustered lighting needs fine culling — but it must not regress.

9. **Visibility fallback paths and edge cases.** Every fallback still flows cells through the compute cull pass:
   - **Portal traversal** (primary) — traversal output → compute cull → indirect draws.
   - **`SolidLeafFallback`** — all cells in the visible list, compute cull runs.
   - **`ExteriorCameraFallback`** — preserves the X-ray view from commit 2a83108. All interior cells go into the visible list; back-face culling (CCW front, back culled) makes near walls see-through and far walls' interior faces visible. Pipeline cull state unchanged from Phase 3.
   - **`NoPvsFallback` / `EmptyWorldFallback`** — same rule, cell list feeds compute cull.
   - **Genuinely empty set** — `multi_draw_indexed_indirect` called with count 0, no GPU errors.
   - **First frame / level load** — HiZ pass-all. Exterior transition frame may use stale HiZ; one frame of over-cull is acceptable.
   - **Indirect buffer overflow** — compute shader refuses to write past the cap, logs once.

10. **No lighting changes.** Flat ambient is still the only shading term. The world shader reads but does not use the new normal/tangent attributes yet (they pass through to Phase 4).

11. **Bind group layout reserves Phase 4 lighting slots.** World-shader bind group 0 reserves slots for the SH volume texture, shadow map atlas / cube array, and clustered light index buffer. Phase 3.5 binds dummy 1×1 resources; the shader does not sample them. Prevents a layout rework when Phase 4 plugs in real resources.

12. **Documentation already up to date.** `context/lib/rendering_pipeline.md` §3, §6, §7, §8 were updated in this phase's prep. This plan should not need to re-update them on ship — only verify they still match the shipped code and delete the plan.

---

## Implementation tasks

Sequencing matters. The vertex format change is the structural break; everything else layers on.

### Stage 1 — Format and compiler

1. Introduce `GeometryV3` section in `postretro-level-format/src/geometry.rs`. Define vertex layout (position + UV + packed normal + packed tangent). Add `SectionId::GeometryV3`. Round-trip tests.

2. Implement octahedral encode/decode helpers. Unit tests covering the axis vectors, near-pole vectors, and random samples round-tripping within an acceptable error bound.

3. Extend prl-build's brush-side projection to compute per-vertex normals (face plane) and tangents (UV `s`-axis projected onto the face plane, renormalized). Populate the bitangent sign from `cross(normal, tangent) · uv_t_axis`.

4. Update prl-build pack stage to write `GeometryV3` instead of `GeometryV2`. Delete `GeometryV2` write path (it becomes a read-only legacy format loaders can still parse if desired; otherwise fail old files with a clear error).

### Stage 2 — Per-cell chunk table

5. Design the chunk table PRL section. Header + record layout. Round-trip tests in `postretro-level-format`.

6. Group faces by (cell, material bucket) in prl-build after portal generation. Sort indices so each bucket within each cell is contiguous. Emit chunk records with AABB, index offset, index count, material bucket id.

7. Engine loader parses the chunk table and hands it to the renderer.

### Stage 3 — Renderer plumbing

8. Update engine vertex shader input layout to the new format. Decode octahedral normal and tangent in the vertex shader. Pass world-space normal + tangent to the fragment shader as interpolants.

9. Fragment shader: still applies flat ambient. Receives normal/tangent but does not use them yet. This is intentional — Phase 4 will plug them in.

10. Remove per-leaf draw loop on the CPU. Keep the Phase 3 shader-state and bind-group setup that still applies.

### Stage 4 — GPU culling and indirect draws

11. Upload the per-cell chunk table to a GPU storage buffer at level load.

12. Write the cell-culling compute shader: reads visible cell ids, tests AABB against frustum (and HiZ when present), writes indirect commands. One thread per cell.

13. Allocate the indirect draw buffer and the draw count counter. Cap the draw count at a configurable maximum (65,536 initial).

14. Renderer frame loop: dispatch cell-culling compute pass → issue `multi_draw_indexed_indirect` for each material bucket against its slice of the indirect buffer.

### Stage 5 — HiZ pyramid

15. After the opaque pass, copy the depth buffer into a `r32float` storage texture.

16. Build the HiZ pyramid via a compute kernel. Each mip takes `max()` over its 2×2 footprint in the level below. Mip count derived from depth buffer dimensions.

17. Extend the cell-culling compute shader to sample the HiZ pyramid: compute screen-space AABB and nearest-Z, pick the mip level, test.

18. First-frame handling: bind a pass-all HiZ on frame 0 and after any level load.

### Stage 6 — Validation

19. Golden-image regression tests: render every `assets/maps/` test map from a fixed camera path, compare against Phase 3 baseline. Visual parity is the acceptance bar.

20. Microbenchmark frame time on a cell-heavy test map before (Phase 3) and after (Phase 3.5). Record in the PR description — not gated on a specific delta, but regressions need a root cause.

21. Edge case coverage: camera in solid leaf, exterior camera, empty visible set, first frame, indirect buffer overflow.

---

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Shader compile-time stutter on first load from added compute kernels | Low | Compile all pipelines eagerly at level load. If measurable stutter appears, add wgpu pipeline cache persistence as a follow-up. |
| `multi_draw_indexed_indirect` not uniformly available on wgpu backends | Medium | Probe `Features::MULTI_DRAW_INDIRECT` at adapter selection. If absent, fall back to issuing N singular `draw_indexed_indirect` calls from the CPU, iterating the chunk table directly. The compute culling pass still runs and still writes the indirect buffer — only the dispatch loop changes. No GPU→CPU readback, no pipeline stall. Singular `draw_indexed_indirect` is baseline wgpu. Same compute shader, same buffer, two dispatch modes selected at startup. |
| Octahedral encoding precision loss visible on highly specular surfaces | Low | No specular shading in Phase 3.5; precision concerns defer to Phase 4. If Phase 4 lighting reveals artifacts, bump to 16-bit float per component or sign-remapped encoding. |
| HiZ false negatives (incorrectly occluding visible cells) at grazing angles | Low | Use a conservative AABB projection (extents as if no rotation) and the next-coarser mip; minor over-inclusion is acceptable. |
| Prl-build tangent generation disagrees with Phase 4 normal maps baked elsewhere | Medium | Document the tangent convention explicitly (MikkTSpace-ish from UV `s`-axis projection). Phase 4 normal maps must be baked against the same convention. Flag in Phase 4 plan. |
| Per-cell chunk boundary invalidates CSG face clipping from Phase 3 | Low | Verify CSG clipping runs before chunk grouping. The grouping is purely a reordering of the output indices — it should not affect correctness. |

---

## Non-goals revisited

- Not a lighting phase.
- Not a perf win phase, though perf parity is required.
- Not a wgpu feature showcase — mesh shaders and bindless are explicitly deferred.
- Not an asset pipeline change beyond what the new PRL sections need.

---

## When this plan ships

- Delete this plan file.
- Verify `context/lib/rendering_pipeline.md` §3, §6, §7, §8 still match the shipped implementation. Fix any drift.
- Update the roadmap: Phase 3.5 gets a `✓` and `**Testable outcome:** ... ✓`.
- No new context/lib doc needed — the durable layer was updated as prep work.
- Phase 4 unblocks.
