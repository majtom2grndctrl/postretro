# Phase 3.5 — Rendering Foundation Extension

> **Status:** draft
> **Phase:** 3.5 — between Phase 3 (Textured World) and Phase 4 (Lighting Foundation). See `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §5–§8 · `context/lib/build_pipeline.md` §PRL Compilation

---

## Goal

Replace the CPU per-leaf draw loop with GPU-driven indirect draws and per-cell chunking. Upgrade the vertex format to carry packed normals and tangents. No lighting changes — Phase 4 layers lighting onto this foundation.

Visual output identical to Phase 3. Different architecture underneath.

**Post-BSP runtime.** Cells are the runtime draw-dispatch unit, deliberately decoupled from BSP leaves. The BSP tree is a compiler intermediate that produces cells, portals, and chunk tables — the runtime renderer consumes those outputs without knowing BSP exists. A future compiler could generate cells from a different spatial structure; the runtime wouldn't change. See `rendering_pipeline.md` §5 for the cell abstraction and `build_pipeline.md` §Pipeline Overview for the compiler's role.

---

## Scope

### In scope

- Vertex format upgrade (`GeometryV3`): packed normals and tangents per vertex, per-vertex color removed. Spec: `rendering_pipeline.md` §6.
- Per-cell draw chunks: geometry grouped by (cell, material bucket) with AABB and cell→chunk-range index. Spec: `rendering_pipeline.md` §5.
- GPU-driven indirect draw path: compute cull → `multi_draw_indexed_indirect`. Spec: `rendering_pipeline.md` §7.1.
- World shader updated for new vertex attributes. Flat ambient stays.

### Out of scope

- Normal map sampling, SH irradiance, dynamic lights, shadow maps, clustered light list (Phase 4).
- Mesh shaders, bindless descriptors, hardware ray tracing (not baseline wgpu).
- Billboard sprites, emissive surfaces, fog (Phase 5).
- Pipeline cache persistence. Follow-up if shader compile stutter surfaces.
- Phase 4 bind group pre-wiring. Phase 3.5 ships the minimal bind group for what it uses; Phase 4 reworks the layout when it adds lighting resources.

---

## Shared Context

Every agent working this plan should read:
- `context/lib/rendering_pipeline.md` §5 (Cells and Draw Chunks), §6 (Vertex Format), §7 (Rendering Stages)
- `context/lib/build_pipeline.md` §PRL Compilation
- `context/lib/development_guide.md`

### Key constraints

- **No lighting changes.** Flat ambient is the only shading term. The world shader receives normal/tangent attributes but does not use them — Phase 4 plugs them in.
- **Visual parity with Phase 3.** Same textures, same geometry, same flat ambient. A human cannot tell the two apart side-by-side. Tolerance: SSIM ≥ 0.99 or per-channel per-pixel diff ≤ 2/255 against a Phase 3 reference capture. Note: Phase 3 uses flat-white per-vertex color; verify test maps use flat white before baselining, since vertex color is removed in this phase.
- **Visibility subsystem unchanged.** Portal traversal and all fallback paths still produce a visible cell list. The compute culling pass consumes that list regardless of which path produced it.
- **Perf at least on par.** Frame time within 5% of Phase 3 on a cell-heavy test map (20+ visible cells).

### Architectural decisions

- **`cell_id` is opaque.** The chunk table keys on cell id. Current compiler assigns one cell per empty BSP leaf; a future compiler could assign differently. Runtime never interprets cell_id as a leaf index. See `rendering_pipeline.md` §5.
- **Per-material indirect calls.** wgpu lacks uniform bindless descriptor support. Renderer issues one `multi_draw_indexed_indirect` per material bucket (10–50 per level). Revisitable if wgpu exposes bindless or texture arrays.
- **Chunk table access pattern.** Chunks sorted by cell_id with a cell→chunk-range index. Compute shader reads one contiguous range per cell, emits one `DrawIndexedIndirect` per chunk.
- **Migration: bump section ID.** `GeometryV3 = 17` supersedes `GeometryV2 = 3`. `CellChunks = 18`. Old `.prl` files fail with a clear version error. No backward compatibility needed (pre-stable format).
- **`multi_draw_indexed_indirect` fallback.** Probe `Features::MULTI_DRAW_INDIRECT` at adapter init. If absent, CPU iterates the indirect buffer and issues singular `draw_indexed_indirect` per command. Same compute shader, same buffer, two dispatch modes.
- **Indirect buffer fixed slots.** Every chunk gets a permanent slot in the indirect buffer; the buffer is sized to `total_chunks` at level load and the compute shader writes through the slot for each chunk it processes. Overflow is architecturally impossible — there is no cap to exceed. Typical retro FPS sizing: 100–500 cells × 2–10 materials = 200–5,000 draws.
- **BSP runtime dependencies remaining after this phase.** `BspNodes` (12) and `BspLeaves` (13) still emitted and consumed for camera-leaf lookup. These are the last BSP artifacts in the runtime path. Replacing them with a cell-location section is a future step — not this phase.

### Data flow

```
prl-build                                       runtime
─────────                                       ───────

.map parse                                      PRL loader
  ↓                                               ↓
brush-volume BSP                          vertex / index buffers
  ↓                                               ↓
brush-side projection                     renderer GPU upload
  (emits per-vertex normal + tangent)             ↓
  ↓                                       per-cell chunk table → GPU storage buffer
portal generation                                 ↓
  ↓                                         portal traversal
per-cell chunk grouping                           ↓
  (group by cell × material bucket)       visible cell list
  ↓                                               ↓
pack .prl                                 cell culling (compute)
  GeometryV3 (17)                                 ↓
  CellChunks (18)                         indirect draw buffer
                                          (N commands per material bucket)
                                                  ↓
                                          multi_draw_indexed_indirect
                                            (one call per bucket)
                                                  ↓
                                             world shader
                                             (flat ambient)
```

---

## Tasks

### Task 1: PRL format — GeometryV3 and CellChunks sections

**Description:** Add two new sections to `postretro-level-format`:

- `GeometryV3` (section id 17): vertex layout per `rendering_pipeline.md` §6 — position (`f32 × 3`) + UV (`f32 × 2`) + octahedral normal (`u16 × 2`) + octahedral tangent with bitangent sign (`u16 × 2`). 28 bytes/vertex. Implement octahedral encode/decode helpers.
- `CellChunks` (section id 18): per-cell draw chunk table per `rendering_pipeline.md` §5. Each record: cell_id, AABB, index_offset, index_count, material_bucket_id. Include a cell→chunk-range index header so consumers can look up all chunks for a given cell in O(1).

**Acceptance criteria:**
- [ ] `SectionId::GeometryV3 = 17` and `SectionId::CellChunks = 18` defined
- [ ] Octahedral encode/decode round-trips axis vectors, near-pole vectors, and random samples within < 0.001 max angular error
- [ ] GeometryV3 write → read round-trip preserves all fields
- [ ] CellChunks write → read round-trip preserves all fields including cell→chunk-range index
- [ ] `cargo test -p postretro-level-format` passes

**Depends on:** none

---

### Task 2: prl-build — vertex format upgrade

**Description:** Extend prl-build's brush-side projection to compute per-vertex normals (from face plane) and tangents (from UV s-axis projected onto face plane, renormalized). Populate bitangent sign from `cross(normal, tangent) · uv_t_axis`. Update the pack stage to write `GeometryV3` instead of `GeometryV2`. Remove the `GeometryV2` write path.

**Acceptance criteria:**
- [ ] Every emitted vertex has an orthonormal tangent-space basis (within float tolerance)
- [ ] Bitangent sign is consistent across each face
- [ ] prl-build writes `GeometryV3` section; old maps recompile cleanly
- [ ] `cargo test -p postretro-level-compiler` passes

**Depends on:** Task 1 (GeometryV3 format definition)

---

### Task 3: prl-build — per-cell chunk grouping

**Description:** After portal generation, group faces by (cell, material bucket). Sort indices so each bucket within each cell is contiguous. Compute per-chunk AABB. Build the cell→chunk-range index. Emit as `CellChunks` section.

**Acceptance criteria:**
- [ ] One chunk record per (cell, material bucket) pair in the output
- [ ] Index ranges are contiguous and non-overlapping within the shared index buffer
- [ ] AABBs tightly bound each chunk's geometry
- [ ] Cell→chunk-range index correctly maps each cell_id to its chunk range
- [ ] CSG face clipping runs before chunk grouping (ordering verified)
- [ ] `cargo test -p postretro-level-compiler` passes

**Depends on:** Task 1 (CellChunks format definition), Task 2 (geometry pipeline — chunk grouping operates on the face/index data Task 2 produces)

---

### Task 4: Engine loader and renderer vertex format

**Description:** Update the engine PRL loader to parse `GeometryV3` and `CellChunks` sections. Update the renderer vertex layout and world shader to consume the new format. Decode octahedral normal and tangent in the vertex shader; pass to fragment stage as interpolants. Fragment shader still applies flat ambient — normal/tangent are present but unused until Phase 4. Remove the per-leaf draw loop and per-vertex color.

**Acceptance criteria:**
- [ ] Engine loads `GeometryV3` and `CellChunks` from compiled test maps
- [ ] World shader vertex input matches 28-byte layout; octahedral decode in vertex shader
- [ ] Fragment shader outputs flat ambient (no lighting changes)
- [ ] Per-leaf draw batching code removed
- [ ] Renders correctly on all test maps (visual comparison with Phase 3)
- [ ] `cargo test -p postretro` passes; `cargo check` clean

**Depends on:** Task 2 (compiler emits GeometryV3), Task 3 (compiler emits CellChunks)

---

### Task 5: Compute culling and indirect draw path

**Description:** Implement the GPU-driven draw path. Upload chunk table to a GPU storage buffer at level load. Write a cell-culling compute shader: reads visible cell ids, looks up each cell's chunk range via the cell→chunk-range index, tests AABB against frustum, emits `DrawIndexedIndirect` commands per surviving chunk. Allocate indirect draw buffer (cap: 8,192) and draw count counter. Render pass issues `multi_draw_indexed_indirect` per material bucket. If `MULTI_DRAW_INDIRECT` feature is absent, fall back to singular `draw_indexed_indirect` from CPU iterating the same buffer.

**Acceptance criteria:**
- [ ] Compute shader dispatched each frame; render pass issues zero CPU per-cell draws
- [ ] All visibility fallback paths feed the compute cull pass: portal traversal, SolidLeafFallback, ExteriorCameraFallback (preserves X-ray behavior from commit 2a83108), NoPvsFallback, EmptyWorldFallback
- [ ] Empty visible set → `multi_draw_indexed_indirect` with count 0, no GPU errors
- [ ] `MULTI_DRAW_INDIRECT` absent → fallback dispatch mode, same visual output
- [ ] Visual parity with Phase 3 on all test maps
- [ ] `cargo test -p postretro` passes

**Depends on:** Task 4 (renderer consumes chunk table)

---

### Task 6: Validation and benchmarking

**Description:** Golden-image regression tests against Phase 3 baseline. Render every `assets/maps/` test map from a fixed camera, compare. Microbenchmark frame time on a cell-heavy test map. Exercise edge cases.

**Acceptance criteria:**
- [ ] Visual parity: SSIM ≥ 0.99 or per-pixel diff ≤ 2/255 on all test maps
- [ ] Frame time within 5% of Phase 3 on cell-heavy map (20+ visible cells)
- [ ] Edge cases exercised: camera in solid leaf, exterior camera, empty visible set, first frame
- [ ] All `cargo test` passes; `cargo clippy -- -D warnings` clean

**Depends on:** Task 5

---

## Sequencing

All phases are sequential — each builds directly on the prior phase's output.

**Phase 1 (sequential):**
- Task 1 — PRL format foundations. Must complete before compiler tasks consume the new types.

**Phase 2 (sequential, after Phase 1):**
- Task 2 — vertex format upgrade in prl-build. Consumes GeometryV3 format from Task 1.
- Task 3 — chunk grouping in prl-build. Consumes CellChunks format from Task 1 and the geometry pipeline from Task 2. Runs after Task 2 because chunk grouping operates on the face/index data the vertex upgrade produces.

**Phase 3 (sequential, after Phase 2):**
- Task 4 — engine loader and renderer. Consumes compiled PRL files with both new sections.

**Phase 4 (sequential, after Phase 3):**
- Task 5 — compute culling and indirect draws. Depends on Task 4's renderer plumbing and chunk table upload.

**Phase 5 (sequential, after Phase 4):**
- Task 6 — validation. Depends on all prior tasks.

---

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| `multi_draw_indexed_indirect` unavailable on backend | Medium | Feature probe + singular `draw_indexed_indirect` fallback. Same compute shader, different dispatch loop. |
| Shader compile stutter from new compute kernels | Low | Eager pipeline compilation at level load. Pipeline cache persistence as follow-up if measurable. |
| Octahedral precision insufficient for Phase 4 specular | Low | No specular in this phase. Bump to f16 or sign-remap encoding in Phase 4 if needed. |
| Tangent convention disagrees with Phase 4 normal maps | Medium | Convention documented (UV s-axis projection, MikkTSpace-adjacent). Phase 4 normal maps must match. Flagged in Phase 4 plan. |

---

## Notes

- The `LightProbes = 5` reserved-ID comment in `postretro-level-format/src/lib.rs` predates the SH spec and is misleading. Clean up as a drive-by, not a tracked task.
- `rendering_pipeline.md` §5 and `build_pipeline.md` §Pipeline Overview were updated during this phase's prep to reflect the post-BSP cell abstraction and chunk table design. Verify they still match shipped code at plan completion.

---

## When this plan ships

- Delete this plan file.
- Verify `context/lib/rendering_pipeline.md` and `context/lib/build_pipeline.md` still match the shipped implementation. Fix any drift.
- Update the roadmap: Phase 3.5 gets `✓`.
