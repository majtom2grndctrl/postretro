# Phase 4 — Lighting Foundation

> **Status:** draft — architectural direction locked. Stages 1–3 (FGD, translator, canonical format) are implementation-ready. Stages 4–5 (SH irradiance volume baker and output format) are decided in this document; remaining refinement is sizing choices (probe density, ray count) that belong in the execution plan, not the format spec.
> **Phase:** 4 (Lighting Foundation) — see `context/plans/roadmap.md`. Covers the full lighting pipeline end-to-end: baked indirect (SH irradiance volume) and dynamic direct (clustered forward+ with shadow maps). One spec, one pipeline.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `context/reference/light-entities-across-engines.md`
> **Prerequisite:** Phase 3.5 (Rendering Foundation Extension) — vertex format with packed normals + tangents, per-cell draw chunks, and GPU-driven indirect draws must ship first. The lighting path layers onto that architecture.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the format spec. Research happens in-draft; decisions land in the spec as they're made; implementation consumes the plan; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Specify Postretro's lighting system end-to-end *and* the spatial-structure refactor that lighting needs. Lighting rides on top of a global BVH that replaces Phase 3.5's cell-chunk compute cull — one structure serving runtime visibility culling and compile-time baker ray casting.

**This phase splits into two parts with a check-in gate between them:**

- **Part A — BVH Foundation.** Replaces Phase 3.5's cell-chunk compute cull with a global BVH. Retires the `CellChunks` PRL section. Ships with visual parity to Phase 3.5 — flat ambient, no lighting changes, same frame output from a different spatial structure. Validates before Part B begins.
- **Part B — Lighting.** Layers the full lighting pipeline onto the BVH foundation: FGD authoring, canonical light format, SH irradiance volume bake, normal maps, clustered forward+ direct lights, shadow maps. The SH baker ray-casts through the Part A BVH — one structure, two consumers, no second design pass.

Five axes, one plan:

1. **Spatial** — global BVH in prl-build; flat PRL section; WGSL compute traversal at runtime; CPU `bvh` crate traversal at bake time. Replaces Phase 3.5's `CellChunks` section and cell-chunk compute cull entirely.
2. **Input** — mapper authoring (FGD) → parse → translator → canonical light format.
3. **Indirect bake** — SH L2 irradiance volume, ray-cast through the Part A BVH over static geometry.
4. **Direct shading** — clustered forward+ consumption of canonical lights plus transient gameplay lights, with shadow maps for dynamic shadow-casters.
5. **Output** — PRL sections for BVH and SH volume; runtime sampling paths.

The translator is decoupled from the parser: future map-format support (e.g., UDMF) adds a sibling module against the same canonical types, so the baker and everything downstream never learn the source format.

---

## Spatial structure — why BVH, why now

Phase 3.5 shipped a per-cell chunk table and compute-cull shader. That works, but Part B needs a compile-time acceleration structure for baker ray casts anyway — and Phase 3.5's review flagged finding #1 (`FaceMetaV3.index_offset/index_count` stale after chunk reordering) which reworks the same compiler data path. Doing the BVH refactor now gives us:

- One acceleration structure for runtime cull *and* bake-time ray casts. No second design pass in Part B.
- A clean place to land Phase 3.5 review finding #1 — the compiler's face→index pipeline gets rewritten end-to-end, so the stale-metadata bug evaporates as a free side effect.
- No backward compat shim. `CellChunks` section id retires, `chunk_grouping.rs` deletes, `compute_cull.rs` rewrites, `POSTRETRO_FORCE_LEGACY` diagnostic deletes with it. Pre-release, own the refactor.

**Architectural commitments:**

- **Global BVH, not per-region.** Single flat hierarchy over all static geometry. Per-region is the pivot path if Stage A5 validation shows global doesn't hit frame-time parity on cell-heavy maps — designed for as a fallback, not as day-one scope.
- **Software traversal only.** No hardware ray tracing. Target is pre-RTX hardware and wgpu (which doesn't expose hardware RT regardless). Runtime traversal is a WGSL compute shader over a flat node/leaf storage buffer. Bake-time traversal is CPU through the `bvh` crate. Same structure, two traversal implementations, zero hardware assumptions.
- **Portals stay.** Portal DFS still produces the visible-cell set — BVH replaces per-chunk frustum culling, not occlusion culling. Portal output feeds the BVH traversal compute shader; the integration shape lands in Stage A4.
- **Fixed-slot indirect buffer preserved.** Phase 3.5's no-atomic-counter design survives Part A: each BVH leaf gets a permanent indirect-buffer slot, so overflow stays architecturally impossible.

---

## Context

Phase 3 ships flat uniform lighting with no baked light sources. Phase 3.5 adds GPU-driven indirect draws, per-cell chunks, and the vertex format (position + UV + packed normal + packed tangent) that lighting depends on — but continues to apply flat ambient. Phase 4 replaces flat ambient with the full lighting pipeline: SH L2 irradiance volume for indirect + clustered forward+ dynamic lights for direct + shadow maps + tangent-space normal maps.

Phase 4 is *two deliverables* behind one check-in gate: Part A lands the BVH refactor and validates visual/perf parity with Phase 3.5 output; Part B lands lighting on top of Part A's BVH. Part A keeps the Phase 3.5 vertex format and indirect-draw architecture; it only swaps the cell-chunk compute cull for a BVH traversal compute cull and retires the `CellChunks` PRL section.

The current FGD (`assets/postretro.fgd`) defines `env_fog_volume`, `env_cubemap`, and `env_reverb_zone`. No light entities exist yet — Phase 3 was runnable without them. This plan adds them as the front of the pipeline.

Postretro's convention is to research established solutions before inventing. Several references inform the spec:

- **id Tech 4 (Doom 3 / Quake 4)** — irradiance volumes with per-area probe storage. GDC talks and open-source id Tech 4 ports document the approach end-to-end. The closest architectural match.
- **Frostbite and modern AAA** — SH L1/L2 probe storage in regular 3D grids, trilinear interpolation, validity masks. The direct precedent for the chosen SH L2 + regular grid approach.
- **Source Engine** — ambient cubes (six-axis per-probe RGB). Considered and rejected: SH L2 gives smoother reconstruction at similar storage cost.
- **ericw-tools `LIGHTGRID_OCTREE`** — sparse octree probe samples as a BSPX lump. The long-standing Quake-family answer. Considered and rejected: adaptive density is more complex for modest gain when the target map size is small and indoor.
- **PBR lighting schemas** — structural alignment target for the canonical light format (falloff, cone, intensity axes). Postretro is not shipping PBR materials, but matching the structural shape means the format isn't painted into a corner.
- **Rust ecosystem** — crates for SH projection (`spherical_harmonics`, hand-rolled per rendering_pipeline.md lineage) and ray-triangle intersection (`bvh` — already the Part A choice, reused here). Preferred over writing from scratch if solid. Hardware-accelerated options (`embree-rs`, native OptiX bindings) are out of scope — no RT target, no native wrappers, pre-RTX commitment.

---

## Scope

### In scope

**Part A — BVH Foundation:**
- Global BVH construction in `prl-build` using the `bvh` crate. BVH leaves carry `(material_bucket_id, index_range, AABB)` — no cell/region tagging in the initial cut.
- New `Bvh` PRL section: header + flat node array + flat leaf array, serialized in tree order for direct GPU upload.
- Retirement of the `CellChunks` section id, `chunk_grouping.rs`, the `CellChunkTable` runtime type, and all `chunks_for_cell` helpers. No compat shim.
- Rewrite of `compute_cull.rs` to a WGSL BVH traversal compute shader. Preserves Phase 3.5's fixed-slot indirect-buffer design, `MULTI_DRAW_INDIRECT` feature probe, and singular `draw_indexed_indirect` fallback path.
- Portal DFS output → BVH traversal integration: portal DFS still produces the visible-cell set; BVH traversal consumes it as a per-leaf filter. Exact integration shape decided in Stage A4.
- Deletion of `POSTRETRO_FORCE_LEGACY` diagnostic mode, `determine_prl_visibility`, and the CPU-side draw-range reconstruction path. Part A's GPU path is the only path.
- Part A validation: visual parity with Phase 3.5 reference capture (SSIM ≥ 0.99 or per-pixel diff ≤ 2/255), frame time within 5% of Phase 3.5 on cell-heavy maps, edge-case coverage. Check-in gate before Part B begins.

**Part B — Lighting:**
- Full lighting pipeline spec: FGD entities, Quake-map translator, canonical light format, SH irradiance volume baker, runtime SH sampling, clustered forward+ dynamic lighting, shadow maps, normal map rendering, PRL section shape.
- FGD file at `assets/postretro.fgd` adding `light`, `light_spot`, `light_sun`.
- `postretro-level-compiler/src/format/quake_map.rs` translator module. Pattern is `format/<name>.rs` per source format; each format's internal structure is its own decision.
- Parser wiring: `prl-build` extracts light entity properties into a property bag and dispatches to the translator.
- Validation rules (errors block compilation; warnings log and proceed).
- Quake `style` integer → `LightAnimation` preset conversion. Canonical format is preset-free; the translator owns the Quake style table.
- SH irradiance volume baker: probe placement, radiance evaluation with shadow raycasting, SH L2 projection, validity masking, PRL section writer.
- Runtime SH volume sampling: parse PRL section to 3D texture, trilinear sampling in world shader.
- Normal map loading and tangent-space shading in the world shader.
- Clustered light list compute prepass: cluster grid definition, per-cluster light index lists, fragment-shader walk.
- Shadow map pipeline: CSM for directional, cube shadow maps for point/spot, sampling in the world shader.
- Test map coverage extending `assets/maps/test.map`.
- Documentation update to `context/lib/build_pipeline.md` §Custom FGD table.

### Out of scope

- **Per-region / spatially-partitioned BVH.** Global BVH is the day-one commitment. Per-region is the Stage A5 pivot path if global underperforms — designed-for as a fallback, not as initial scope.
- **Hardware ray tracing, anywhere.** Target is pre-RTX hardware and wgpu (which does not expose hardware RT). Runtime BVH traversal is WGSL compute; bake-time BVH traversal is CPU `bvh` crate. Shadow maps cover runtime shadowing.
- **Mesh shaders.** wgpu does not expose them; GPU culling uses compute + indirect draws throughout both Part A and Part B.
- **Dynamic BVH rebuilds.** The BVH is baked once at compile time and read-only at runtime. Dynamic entity geometry (Phase 5+) is handled separately — it is not added to the static BVH.
- UDMF map-format support. Separate initiative. The `format/<name>.rs` architecture established here accommodates it without refactor.
- `env_projector` and texture-projecting lights.
- IES profiles / photometric data.
- Area lights (rectangle, disk). Point / spot / directional cover the target feature set.
- Second-bounce indirect. The SH volume captures direct-to-static bounces; multi-bounce is a follow-up if visuals demand it.
- Runtime dynamic probe updates (DDGI-style). The SH volume is baked, read-only at runtime.
- Runtime evaluation of light animation curves. The baker bakes animation into probe sample curves at compile time; runtime evaluation of dynamic light animations is a Phase 5 follow-up.
- Exhaustive academic literature review. "Survey what's known and pick a direction," not "produce a novel contribution."
- Benchmarking probe baking performance. Decisions are made on design grounds; benchmarking belongs in the execution plan once sizing questions come up.

---

## Pipeline

```
┌─── Part A: Spatial ────────────────────────────────────────────────┐
│                                                                    │
│  prl-build (compile time)              postretro (runtime)         │
│  ────────────────────────              ─────────────────────       │
│                                                                    │
│  geometry + portals                    .prl loader                 │
│    ↓                                     ↓                         │
│  global BVH build (bvh crate)          BVH storage buffer upload   │
│    ↓                                     ↓                         │
│  flatten → node[] + leaf[]             portal DFS → visible cells  │
│    ↓                                     ↓                         │
│  write Bvh PRL section                 WGSL BVH traversal compute  │
│                                          ↓                         │
│                                        indirect draw buffer        │
│                                          ↓                         │
│                                        multi_draw_indexed_indirect │
│                                          (one call per bucket)     │
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
                         │                        │
                         │ (same BVH,             │ (Part A ships here;
                         │  two consumers)        │  Part B layers on top)
                         ↓                        ↓
┌─── Part B: Lighting ───────────────────────────────────────────────┐
│                                                                    │
│  TrenchBroom authoring (FGD)                                       │
│    → .map file                                                     │
│      → prl-build parser (extract property bag)                     │
│        → format::quake_map::translate_light (validate, convert)    │
│          → CanonicalLight in MapData.lights                        │
│            ├─→ SH irradiance volume baker                          │
│            │     (ray-casts through Part A BVH via bvh crate,      │
│            │      SH L2 projection, validity mask)                 │
│            │     → SH section in .prl                              │
│            │       → runtime trilinear sample (fragment shader,    │
│            │          indirect term)                               │
│            └─→ runtime direct light buffer                         │
│                (canonical lights + transient gameplay lights)      │
│                  → clustered light list compute prepass            │
│                    → cluster walk in fragment shader (direct term) │
│                      → shadow map sampling per shadow-casting light│
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
```

Each stage below is one section of the spec. Part A stages (A1–A5) come first; Part B stages (1–6) match the original lighting spec and assume Part A has shipped.

---

## Part A — BVH Foundation

Part A replaces Phase 3.5's cell-chunk compute cull with a global BVH. It ships with visual parity to Phase 3.5 (flat ambient, identical rendered output) and is validated before Part B begins. Part A's stages are `## Stage A1` through `## Stage A5`; they read as peers of Part B's `## Stage 1` through `## Stage 6` below.

---

## Stage A1 — CPU BVH construction in prl-build

**Description:** After portal generation, collect all face-indices into BVH primitives. Each primitive carries `(material_bucket_id, index_range, AABB)` where `index_range` is a contiguous slice of the shared index buffer containing triangles for that `(face, material_bucket)` pair. Build a global BVH over these primitives using the `bvh` crate (SAH-driven, CPU, deterministic). Flatten the resulting tree into two arrays: a dense node array (internal nodes + links) and a dense leaf array (primitive metadata + AABB).

This stage deletes `postretro-level-compiler/src/chunk_grouping.rs` entirely. Its role is subsumed by the BVH primitive collection step — there is no `(cell, material_bucket)` grouping anymore, only BVH leaves.

**Finding #1 drive-by:** The compiler's face → index pipeline is rewritten end-to-end in this stage. `FaceMetaV3.index_offset/index_count` either get rewritten consistently during BVH construction or are retired from the `GeometryV3` section entirely (decided during implementation). Either way, the Phase 3.5 review's critical finding lands here as a side effect, not as a separate task.

**Acceptance criteria:**
- [ ] BVH builds deterministically: identical input geometry produces identical flattened node/leaf arrays byte-for-byte
- [ ] Every triangle in the source geometry maps to exactly one BVH leaf
- [ ] BVH leaf AABBs tightly bound each leaf's triangle set
- [ ] `chunk_grouping.rs` removed; no references remain in the compiler
- [ ] `FaceMetaV3.index_offset/index_count` staleness bug is no longer reachable (either fixed or retired)
- [ ] `cargo test -p postretro-level-compiler` passes
- [ ] `cargo clippy -p postretro-level-compiler -- -D warnings` clean

**Depends on:** none (Phase 3.5 GeometryV3 vertex format is reused unchanged)

---

## Stage A2 — PRL section for BVH

**Description:** Add a new `Bvh` section to `postretro-level-format`. Layout:

```
Header (fixed):
  u32        node_count
  u32        leaf_count
  u32        root_node_index
  u32        padding

Node array (node_count entries):
  f32 × 6    aabb_min.xyz, aabb_max.xyz
  u32        left_child_or_leaf_index
  u32        right_child_or_leaf_index
  u32        flags          (bit 0: is_leaf)
  u32        padding

Leaf array (leaf_count entries):
  f32 × 6    aabb_min.xyz, aabb_max.xyz
  u32        material_bucket_id
  u32        index_offset
  u32        index_count
  u32        padding
```

Allocate a new section id. Retire `SectionId::CellChunks = 18` — delete the variant and all read/write code. No backward compat: maps compiled before Part A will fail to load.

**Acceptance criteria:**
- [ ] New `Bvh` section id allocated in `postretro-level-format/src/lib.rs`
- [ ] `SectionId::CellChunks` variant and all supporting code deleted
- [ ] `Bvh` section write → read round-trip preserves all fields byte-for-byte
- [ ] Truncated-section and malformed-header inputs reject cleanly with a clear error
- [ ] `cargo test -p postretro-level-format` passes

**Depends on:** A1 (needs BVH primitive shape to pin the leaf layout)

---

## Stage A3 — Runtime loader + GPU upload

**Description:** Update `postretro/src/prl.rs` to parse the new `Bvh` section and expose `node_array` and `leaf_array` as `wgpu::Buffer` storage buffers. Delete the `CellChunkTable` type, all `chunks_for_cell` helpers, and the CPU-side draw-range reconstruction in `determine_prl_visibility`. Delete the `POSTRETRO_FORCE_LEGACY` diagnostic mode — the BVH path is the only path.

**Acceptance criteria:**
- [ ] `Bvh` section parses into GPU-ready storage buffers at level load
- [ ] `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, and `POSTRETRO_FORCE_LEGACY` deleted; no references remain
- [ ] Legacy V1/V2 `.prl` files (if any) either load cleanly or fail with a clear version error — no half-broken state
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

**Depends on:** A2

---

## Stage A4 — WGSL compute BVH traversal

**Description:** Rewrite `postretro/src/compute_cull.rs` as a BVH traversal compute shader. The shader:

1. Reads the node array and leaf array as storage buffers.
2. Reads the portal DFS output (visible-cell bitmask or equivalent) as another storage buffer.
3. Reads the view frustum as a uniform.
4. Walks the BVH top-down from the root, stack-based with a fixed maximum depth (initial cap: 64 — revisit if deep trees surface).
5. For each node, rejects subtrees whose AABB fails the frustum test. For survivors that are leaves, emits a `DrawIndexedIndirect` command into the fixed-slot indirect buffer.

**Open question for implementation, resolved in this stage:** exactly how portal DFS output filters BVH leaves. Two candidates — (a) per-leaf check against a visible-cell bitmask, requiring BVH leaves to carry a cell id (cheap metadata, doesn't fragment the BVH); (b) traverse BVH once per frustum narrowed by portal DFS, union results (tighter cull, more traversals). Pick one, document the choice in a comment at the top of the shader, note the other as the fallback.

**Preserved from Phase 3.5:**
- Fixed-slot indirect buffer — each BVH leaf gets a permanent slot; no atomic counter, no overflow.
- `MULTI_DRAW_INDIRECT` feature probe; singular `draw_indexed_indirect` fallback when absent.
- One `multi_draw_indexed_indirect` call per material bucket in the render pass.
- Per-leaf cull-status buffer feeding the wireframe overlay (`Alt+Shift+\\`).

**Acceptance criteria:**
- [ ] Compute shader dispatches each frame; render pass issues zero CPU per-leaf draws
- [ ] All visibility fallback paths feed the BVH traversal: portal traversal, SolidLeafFallback, ExteriorCameraFallback (preserves X-ray behavior), EmptyWorldFallback — portal DFS stays, BVH replaces the cell-chunk cull only
- [ ] Empty visible set → `multi_draw_indexed_indirect` with count 0, no GPU errors
- [ ] `MULTI_DRAW_INDIRECT` absent → fallback dispatch mode produces identical visual output
- [ ] Stack depth cap reached → compute shader aborts traversal cleanly (no invalid writes) and logs once
- [ ] Visual parity with Phase 3.5 on all test maps — by eye first, formal parity check in Stage A5
- [ ] `cargo test -p postretro` passes

**Depends on:** A3

---

## Stage A5 — Part A validation (check-in gate)

**Description:** Validate that Part A ships with visual and performance parity to Phase 3.5 before Part B begins. This is the pivot gate: if global BVH fails to hit parity on cell-heavy maps, the decision point is whether to pivot to per-region BVH or revisit the traversal strategy.

**Acceptance criteria:**
- [ ] Visual parity: SSIM ≥ 0.99 or per-pixel diff ≤ 2/255 against Phase 3.5 reference captures on every `assets/maps/` test map
- [ ] Frame time within 5% of Phase 3.5 on a cell-heavy test map (20+ visible cells)
- [ ] Edge cases exercised: camera in solid leaf, exterior camera, empty visible set, first frame, degenerate BVH (single-leaf map), deeply unbalanced BVH (thin corridor map)
- [ ] All `cargo test` passes; `cargo clippy --workspace -- -D warnings` clean
- [ ] `context/lib/rendering_pipeline.md` and `context/lib/build_pipeline.md` updated to describe the BVH architecture (cell/chunk language replaced throughout)
- [ ] **Check-in gate:** Part B does not begin until this stage is signed off. If parity fails, document the gap and decide between per-region pivot, traversal strategy change, or scope adjustment before continuing.

**Depends on:** A4

---

## Part B — Lighting

Part B layers the lighting pipeline onto the BVH foundation shipped in Part A. The SH baker ray-casts through the Part A BVH (via the `bvh` crate on the CPU); one acceleration structure, two consumers. All Part B stages assume Part A is shipped and validated.

---

## Stage 1 — Mapper authoring (FGD)

Three entities: `light`, `light_spot`, `light_sun`. Mappers author with familiar Quake FGD syntax; SmartEdit renders `_color` as a color picker, `delay` as a dropdown, `_fade` as a text field.

### Property → canonical mapping

| FGD Property | Type | Maps to Canonical | Default / Required |
|--------------|------|-------------------|-----|
| `light` | integer | `intensity` | 300 |
| `_color` | color255 (0–255 RGB) | `color` (normalized to 0–1 linear) | 255 255 255 |
| `_fade` | integer (map units) | `falloff_range` | **Required** for Point/Spot; ignored for Directional |
| `delay` | choices | `falloff_model` (0=Linear, 1=InverseDistance, 2=InverseSquared) | 0 (Linear) |
| `_cone` | integer (degrees) | `cone_angle_inner` (converted to radians) | 30 (Spot only) |
| `_cone2` | integer (degrees) | `cone_angle_outer` (converted to radians) | 45 (Spot only) |
| `style` | integer (0–11) | `animation` (preset → sample curves) | 0 (no animation) |
| `mangle` or `target` | vector or target name | `cone_direction` | **Required for Spot; error if missing** |

### FGD template

```fgd
@BaseClass = Light
[
    light(integer) : "Intensity" : 300
    _color(color255) : "Color" : "255 255 255"
    _fade(integer) : "Falloff Distance" : 60000
    delay(choices) : "Falloff Model" : 0 =
    [
        0 : "Linear"
        1 : "Inverse Distance (1/x)"
        2 : "Inverse Squared (1/x²)"
    ]
    style(integer) : "Animation Style" : 0
]

@PointClass base(Light)
    color(255 200 0)
    size(-8 -8 -8, 8 8 8)
    = light : "Point Light"
[
    origin(origin)
]

@PointClass base(Light)
    color(255 150 0)
    size(-8 -8 -8, 8 8 8)
    = light_spot : "Spotlight"
[
    origin(origin)
    _cone(integer) : "Inner Cone Angle" : 30
    _cone2(integer) : "Outer Cone Angle" : 45
    mangle(string) : "Direction (pitch yaw roll)" : ""
    target(target_destination) : "Target Entity" : ""
]

@PointClass base(Light)
    color(255 100 0)
    size(-8 -8 -8, 8 8 8)
    = light_sun : "Directional Light"
[
    origin(origin)
    mangle(string) : "Direction (pitch yaw roll)" : ""
]
```

`assets/postretro.fgd` mirrors the texture pipeline precedent. TrenchBroom game configuration references this path.

---

## Stage 2 — Parse and translate

### Architecture

```
postretro-level-compiler/src/
  format/
    mod.rs              — module root
    quake_map.rs        — translate_light() for Quake-family .map entities
  map_data.rs           — CanonicalLight and canonical types (shared)
  parse.rs              — extracts property bag from shambler, dispatches to translator
```

The parser performs thin property extraction only: for each light entity, pull key-value pairs from shambler into `HashMap<String, String>` and hand off. The translator has no shambler dependency — it operates on the property bag plus origin plus classname. Future formats (`format/udmf.rs`) add sibling modules against the same canonical types; the parser dispatches by source format.

Translator signature:

```rust
pub fn translate_light(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<CanonicalLight, TranslateError>;
```

### Validation rules

Errors block compilation. Warnings log and proceed with defaults.

| Case | Error / Warning | Handling |
|------|-----------------|----------|
| Point/Spot light missing `_fade` | **Error** | Compilation fails. Mapper must specify falloff distance. |
| Spot missing both `mangle` and `target` | **Error** | Compilation fails. Mapper must aim spotlight. |
| Spot with `target="nonexistent"` | **Error** | Compilation fails: "target entity 'X' not found." |
| Invalid property format (non-numeric `_fade`, malformed `mangle`) | **Error** | Compilation fails with property name. |
| `light` = 0 | **Warning** | Intensity is zero; light contributes nothing. |
| Missing `_color` | **Warning** | Defaults to white. |
| Spot missing `_cone` / `_cone2` | **Warning** | Defaults to 30° / 45°. |
| Missing `style` | **Warning** | Defaults to no animation. |
| Spot with `_cone` > `_cone2` | **Warning** | Outer smaller than inner; proceed as specified. |

### Translator notes

- `light` is unitless. Typical Quake-family range is 0–300; the baker (Stage 4) may normalize against chosen bake output, but the range is translator convention for Quake source maps, not a canonical format constraint.
- `_fade` required for deterministic baking. Guideline: `_fade ≈ light × 200` (e.g., `light 300` → `_fade 60000`); adjust per map scale.
- Spotlight direction via `mangle` (pitch yaw roll in degrees, engine space) or `target` (entity name). If both provided, `target` takes precedence.
- Cone degrees → radians conversion happens at the translation boundary. Canonical format is radians-only.
- `style` 0–11 map to `LightAnimation` sample curves. The translator owns the Quake style table (classic preset strings like `aaaaaaaaaa` for constant, `mmnmmommommnonmmonqnmmo` for flicker) and converts them to normalized brightness sample vectors. Styles 12+ reserved for future use.
- Property name variation: accept both `light` and `_light` (Quake community naming variations across tools).

### Test map content

`assets/maps/test.map` gains:

```
// spotlight — inverse-squared falloff, warm color
{
"classname" "light_spot"
"origin" "800 200 96"
"light" "200"
"_color" "255 200 100"
"_fade" "40000"
"_cone" "25"
"_cone2" "50"
"mangle" "-45 0 0"
"delay" "2"
}

// directional — cool light
{
"classname" "light_sun"
"origin" "960 128 500"
"light" "150"
"_color" "200 200 255"
"mangle" "-60 45 0"
}

// red point light — inverse-distance falloff
{
"classname" "light"
"origin" "1100 128 96"
"light" "250"
"_color" "255 50 50"
"_fade" "50000"
"delay" "1"
}
```

---

## Stage 3 — Canonical light format

The compiler translates every supported map format into this canonical form. The baker has no source-format awareness; it sees only `Vec<CanonicalLight>`.

Structural shape aligns with PBR lighting conventions (light type, position/direction, color, intensity, falloff, cone) so the format isn't painted into a corner. Units are not physical — retro aesthetic allows non-physical falloff models — but the axes are the same axes a PBR format would use.

```rust
pub enum LightType {
    Point,          // omnidirectional
    Spot,           // cone, uses cone_angle_inner/outer + direction
    Directional,    // parallel directional (e.g., sunlight); ignores falloff_range
}

pub enum FalloffModel {
    Linear,          // brightness = 1 - (distance / falloff_range)
    InverseDistance, // brightness = 1 / distance, clamped at falloff_range
    InverseSquared,  // brightness = 1 / (distance²), clamped at falloff_range
}

/// Primitive animation: time-sampled curves over a cycle.
/// Format-agnostic — Quake light styles, Doom sector effects, or hand-authored
/// curves all translate into this shape. `None` fields mean "constant across cycle."
pub struct LightAnimation {
    pub period: f32,                  // cycle duration in seconds
    pub phase: f32,                   // 0-1 offset within cycle
    pub brightness: Option<Vec<f32>>, // multipliers sampled uniformly over cycle
    pub hue_shift: Option<Vec<f32>>,  // HSL hue offset 0-1, sampled uniformly over cycle
    pub saturation: Option<Vec<f32>>, // saturation multipliers sampled uniformly over cycle
}

pub struct CanonicalLight {
    // Spatial
    pub origin: DVec3,                    // position (engine space, meters)
    pub light_type: LightType,

    // Appearance
    pub intensity: f32,                   // brightness scalar, unitless
    pub color: [f32; 3],                  // linear RGB, 0-1

    // Falloff (Point and Spot only; ignored for Directional)
    pub falloff_model: FalloffModel,
    pub falloff_range: f32,               // distance at which light reaches zero; must be > 0

    // Spotlight parameters (Spot only; None for Point and Directional)
    pub cone_angle_inner: Option<f32>,    // radians, full brightness
    pub cone_angle_outer: Option<f32>,    // radians, fade edge
    pub cone_direction: Option<[f32; 3]>, // normalized aim vector; Directional uses this too

    // Animation (None = constant light)
    pub animation: Option<LightAnimation>,
}
```

`MapData` gains `lights: Vec<CanonicalLight>`.

### Design notes

- **`LightAnimation` as a format primitive.** Quake light styles (`Flicker`, `Candle`, `FastStrobe`) are translator output, not canonical format vocabulary. Each style preset becomes a brightness/hue/saturation sample vector. Future formats translate into the same primitive. The canonical format has no awareness of any specific source format's vocabulary.
- **Cone angles in radians.** Canonical format is engine-internal; FGDs expose degrees to mappers and the translator converts.
- **`falloff_range`.** PBR-conventional naming.
- **`FalloffModel` enum retained despite PBR alignment.** PBR uses physical inverse-square only; the retro aesthetic needs linear and inverse-distance as well for authored looks. Structural alignment with PBR is about *axes*, not *physics*.
- **`LightType::Directional` (not `Sun`).** Directional is a graphics primitive; "Sun" implies a specific use case. Does not require or imply global illumination — probe-based lighting samples directional lights the same way it samples point lights.
- **Intensity unitless.** Baker may normalize against chosen bake output; refined after probe visuals.
- **`bake_only` / `cast_shadows` split.** Canonical lights feed both the bake (static-only, raycast-shadowed) and the runtime direct path (dynamic, shadow-mapped). A follow-up may add per-light flags if authors need a bake-only or runtime-only subset — not in the initial scope.

---

## Stage 4 — SH irradiance volume baker

Postretro bakes indirect illumination into a regular 3D grid of SH L2 probes. Direct illumination is *not* baked — it is evaluated at runtime by the clustered forward+ path. This split lets dynamic lights co-exist with baked indirect without lightmap complexity, and keeps probe data read-only at runtime.

### Spatial layout: regular 3D grid

Probes sit on an axis-aligned grid that spans the level's AABB with a configurable cell size (default: 1 meter). The grid is the full coverage — no sparse octree, no per-leaf alignment. Rationale:

- Trivial to index: `(x, y, z)` grid coordinate maps directly to a 3D texture texel.
- Hardware trilinear filtering in the fragment shader, zero work in shader code.
- Target maps are small and indoor — octree adaptivity saves little.
- Probes inside solid geometry are flagged invalid; invalid probes are never sampled (see validity mask below).

Compiler flag `--probe-spacing <meters>` overrides the default. Tighter spacing near floors is handled by a second vertical-tier override in future work, not in the initial cut.

### Per-probe storage: SH L2

Nine SH basis coefficients per color channel × three channels = **27 f32 per probe**. SH L2 captures directional incoming radiance with enough fidelity for smooth indirect shading, and the reconstruction math is a single dot product per channel in the fragment shader.

Rejected alternatives:
- **Plain RGB** — loses directional information; flat indirect looks wrong on curved or angled surfaces.
- **Ambient cube** — 18 f32 per probe for comparable quality; SH L2 wins on smoothness.
- **SH L1** — 12 f32 per probe; cheaper but noticeably blurrier on test scenes with colored directional indirect.

### Validity mask

Each probe has a `u8` validity flag: `0` = invalid (inside solid), `1` = valid (usable). Validity is determined at bake time by sampling the BSP tree at the probe position — solid leaves produce invalid probes. Runtime sampling uses the mask to fall back to nearby valid probes when the trilinear footprint crosses a wall.

**Leak mitigation.** A mean-distance-to-nearest-surface field per probe direction (as used in DDGI) is a follow-up if simple validity masking proves insufficient on the test maps. The initial cut ships validity-only.

### Bake algorithm

For each valid probe:

1. Fire **N stratified sample rays** from the probe (default `N = 256`) distributed over the sphere.
2. For each ray, traverse the **Part A BVH** (via the `bvh` crate on the CPU) to find the closest triangle hit. Miss → sky/ambient. Hit → evaluate direct light at the hit point (shadow raycasts from each canonical light traversing the same BVH, sum Lambert contributions), then attenuate by surface albedo approximation to approximate one bounce.
3. Project the incoming radiance samples into SH L2 coefficients.
4. Store coefficients in the probe grid; write validity flag.

Ray count and parallelism strategy are execution details — the plan fixes the algorithm shape, not the sizing. Parallelism: `rayon` over probes. Acceleration: no separate baker BVH — the Part A BVH is the acceleration structure. Same tree, same crate, same traversal code that Part A ships; the baker just calls it from the CPU side instead of the GPU compute shader.

### Animation baking

Lights with animation curves bake into a sample vector per probe: `period` seconds discretized into `sample_count` entries (default 11 samples/cycle). At runtime, the shader reads the current sample and blends with the next. Memory overhead: `probes × animated_lights × samples × 4 bytes`. A 60 × 60 × 20 grid (72k probes) with 5 animated lights and 11 samples is ~16 MB — acceptable upper bound for a large level. Small levels pay proportionally less.

The initial cut may defer animation baking — a static-only first revision that ignores `LightAnimation` is acceptable if it simplifies the first end-to-end path. Execution plan decides.

### Shadow strategy

Bake-time raycast occlusion. Each canonical light contribution at a probe is modulated by a shadow ray from the light position (or direction, for `Directional`) to the probe. Visible → full contribution; occluded → zero. This is the full cost during the bake, but the bake happens once per compile.

Runtime dynamic lights rely on shadow maps (see Stage 6), not probe data.

---

## Stage 5 — PRL section shape

New PRL section for the SH irradiance volume. Section ID to be allocated in `postretro-level-format/src/lib.rs` alongside existing section IDs.

### Layout

All little-endian. Header, then packed probe records.

```
Header (32 bytes):
  f32 × 3    grid_origin      (world-space min corner, meters)
  f32 × 3    cell_size        (meters per cell along x/y/z)
  u32 × 3    grid_dimensions  (probe count along x/y/z)
  u32        probe_stride     (bytes per probe record; 112 for static-only, more with animation)

Probe records (probe_stride bytes each, iterated z-major then y, then x):
  f32 × 27   sh_coefficients  (9 bands × 3 channels)
  u8         validity         (0 = invalid, 1 = valid)
  u8 × 3     padding          (align to 4 bytes)
```

Total static-only probe record: `27 × 4 + 4 = 112 bytes`.

### Runtime upload

27 scalars per probe don't fit in one `Rgba16Float` texel (4 scalars) — need `ceil(27 / 4) = 7` texels minimum. The loader splits probe data across multiple 3D textures at probe-grid resolution, sampled with hardware trilinear. Preferred layout (Unity/Frostbite/DDGI lineage): three slab textures per color channel (9 total), each slab holding three SH bands. Alternative: 7 textures interleaving all 27 scalars. Either is a renderer implementation detail.

The **PRL section is the source of truth**: 27 f32 per probe, contiguous, in baker write order. Runtime splits as it prefers.

Invalid probes upload as zeroed SH coefficients so the trilinear filter degrades across wall boundaries.

### Compatibility

Missing section is not an error. The world shader degrades to flat white ambient when the section is absent, matching Phase 3.5 behavior.

---

## Stage 6 — Runtime direct lighting and shadow maps

Covered here for completeness; the full architectural write-up lives in `context/lib/rendering_pipeline.md` §4 and §7.

### Clustered forward+ light list

Compute prepass runs each frame:

1. Iterate active lights (canonical lights from `MapData::lights` + transient gameplay lights from the entity system).
2. For each cluster in the view-space grid (screen tiles × depth slices), test light volumes against the cluster AABB.
3. Write a packed per-cluster index list to a storage buffer.

Grid sizing and tile dimensions are execution details refined during implementation.

### Shadow maps

- **Directional lights** — cascaded shadow maps (CSM). 3 or 4 cascades; resolution intentionally modest (e.g., 1024² per cascade) to match the aesthetic.
- **Point lights** — cube shadow maps rendered in a single pass via layered rendering where supported, or six passes otherwise.
- **Spot lights** — single shadow map per light.

Not every dynamic light casts shadows. A `cast_shadows: bool` flag on the runtime light struct (not the canonical light) gates rendering a shadow map; static canonical lights derived from FGD may default to true, transient gameplay lights to false.

### Normal maps

Albedo + normal map per texture. Normal maps load as BC5 (RG) when available, interpreted as tangent-space `(x, y)` with `z` reconstructed. Missing normal map falls back to `(0, 0, 1)` — neutral. The vertex shader reconstructs TBN from packed normal and tangent; the fragment shader applies the normal-map perturbation before direct and indirect shading.

---

## Resolved questions

These questions were open in the prior draft and are now decided:

| Question | Decision | Rationale |
|----------|----------|-----------|
| **Part A: BVH spatial strategy** | **Global BVH, not per-region** | Try global first. Per-region is the Stage A5 pivot path if global underperforms on cell-heavy maps. Own the refactor; don't pre-optimize for a scaling problem that may never arrive. |
| **Part A: BVH / CellChunks coexistence** | **BVH replaces CellChunks entirely** | No backward compat shim. `CellChunks` section id retires; `chunk_grouping.rs`, `CellChunkTable`, `chunks_for_cell`, `POSTRETRO_FORCE_LEGACY`, and `determine_prl_visibility` all delete. Pre-release — own it. |
| **Part A: Runtime BVH traversal** | **WGSL compute shader over flat storage buffers** | No Rust crate ships GPU BVH traversal for wgpu. Pre-RTX hardware target + wgpu doesn't expose hardware RT regardless. Custom shader is the established pattern. |
| **Part A: Bake-time BVH traversal** | **CPU `bvh` crate — same tree, different traversal** | One acceleration structure, two consumers. Baker calls the `bvh` crate directly; no GPU round-trip at compile time. |
| **Part A: Phase 3.5 review finding #1** | **Folded into Stage A1 as a drive-by** | Stage A1 rewrites the compiler's face → index pipeline end-to-end. `FaceMetaV3.index_offset/index_count` staleness becomes unreachable as a side effect, no separate fix task needed. |
| Baker approach | Probes only for indirect; dynamic direct at runtime | Lightmaps add a bake stage, an atlas, a UV channel, and a two-texture sampling path in the shader. SH volume + dynamic direct is lighter. |
| Spatial layout | Regular 3D grid | Trivial indexing, hardware trilinear, low complexity. Octree adaptivity wins little on small indoor maps. |
| Per-probe storage | SH L2 (27 f32/probe) | Smooth reconstruction, small shader cost, industry-standard. Ambient cube rejected as needing nearly as much storage for less smoothness. |
| Probe evaluation | Trilinear interpolation on a 3D texture | Hardware-accelerated; zero shader complexity. |
| Shadow strategy (bake) | Raycast at bake time, per light per probe — traverses Part A BVH | Bake is expensive but runs once. Runtime shadow estimation on the volume is not worth the complexity. |
| Shadow strategy (runtime) | Shadow maps per dynamic shadow-caster | CSM for directional, cube for point/spot. Matches aesthetic (chunky edges at modest resolution). Not hardware ray tracing. |
| Animation baking | Bake per-probe sample vectors; defer to execution plan | Animation support may be cut from the initial revision if it complicates the first pass. |
| PRL section shape (SH) | Header + probe records; see Stage 5 | Fixed layout, forward-compatible via `probe_stride`. |

---

## Acceptance criteria

### For draft → ready

- Canonical format confirmed stable (Part B stages 1–3 unchanged from prior draft, already reviewed).
- SH irradiance volume design confirmed against the new rendering pipeline architecture (Part B stage 4).
- PRL section shape sketched concretely enough that the execution plan can anchor on it (Part B stage 5).
- Direct-lighting and shadow-map integration points match `context/lib/rendering_pipeline.md` §4 and §7.
- Part A BVH refactor scope locked: global BVH, WGSL compute traversal, `CellChunks` retirement, `bvh` crate for both runtime (via flattened buffer) and bake-time (via direct CPU calls).
- Rust crate options for BVH and SH projection listed; detailed fitness assessment happens in the execution plan.

### For implementation — Part A (Stages A1–A5)

1. **BVH construction ships:** `prl-build` emits a `Bvh` PRL section for every test map. Build is deterministic — identical input → identical flattened buffer byte-for-byte.

2. **`CellChunks` retired:** `SectionId::CellChunks`, `chunk_grouping.rs`, `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, `POSTRETRO_FORCE_LEGACY` all deleted. No references remain anywhere in the workspace.

3. **Finding #1 unreachable:** `FaceMetaV3.index_offset/index_count` either consistent with the new index buffer ordering or removed from `GeometryV3` entirely. The multi-texture UV normalization path is verified correct on a two-texture test map with different `(w, h)` dimensions.

4. **Runtime BVH traversal correct:** compute shader walks the BVH, frustum-culls, emits `DrawIndexedIndirect` commands into the fixed-slot buffer, one material bucket per `multi_draw_indexed_indirect` call. `MULTI_DRAW_INDIRECT` absent → singular `draw_indexed_indirect` fallback produces identical visual output.

5. **Portal integration decided and documented:** the Stage A4 shader header comment explains how portal DFS output filters BVH leaves, with the other candidate approach noted as the fallback.

6. **Stage A5 validation passes:**
   - Visual parity: SSIM ≥ 0.99 or per-pixel diff ≤ 2/255 against Phase 3.5 reference captures on every `assets/maps/` test map
   - Frame time within 5% of Phase 3.5 on a cell-heavy test map (20+ visible cells)
   - Edge cases: camera in solid leaf, exterior camera, empty visible set, first frame, degenerate BVH (single-leaf map), deeply unbalanced BVH (thin corridor map)
   - All `cargo test` passes; `cargo clippy --workspace -- -D warnings` clean

7. **Documentation updated:** `context/lib/rendering_pipeline.md` and `context/lib/build_pipeline.md` describe the BVH architecture — cell/chunk language replaced throughout. Phase 3.5 plan entry in `roadmap.md` notes that its spatial structure was superseded by Part A.

8. **Check-in gate signed off** before Part B begins. If parity fails, the pivot decision (per-region BVH, traversal strategy change, or scope adjustment) is documented in the plan before work on Part B starts.

### For implementation — Part B stages 1–3 (FGD, translator, canonical format)

1. **FGD file created and verified:** `assets/postretro.fgd` defines `light`, `light_spot`, `light_sun` with Quake-standard properties in TrenchBroom-compatible FGD syntax. FGD loads in TrenchBroom without errors; entity browser shows all three; SmartEdit renders color picker / dropdown / text field correctly.

2. **Translator module:** `postretro-level-compiler/src/format/quake_map.rs` implements `translate_light()` per the signature above. No shambler dependency. Owns Quake `style` → `LightAnimation` preset conversion. Converts cone angles degrees → radians at the translation boundary.

3. **Parser integration:** `parse.rs` recognizes `light`, `light_spot`, `light_sun`, extracts properties into `HashMap<String, String>`, calls the translator, populates `MapData::lights`. Errors block compilation; warnings log.

4. **`MapData` extended:** `lights: Vec<CanonicalLight>` field added. Canonical types defined in `map_data.rs`.

5. **Validation:** Every row of the validation table is covered. Errors block; warnings log.

6. **Test map coverage:** `assets/maps/test.map` includes point + spot + directional lights, non-white color, and non-zero `delay`. Map compiles without errors.

7. **Documentation:** `context/lib/build_pipeline.md` §Custom FGD table includes rows for `light`, `light_spot`, `light_sun` (already updated).

8. **Unit tests for translator:**
   - Valid point / spot (via target) / spot (via mangle) / directional → canonical conversion.
   - Point/spot missing `_fade` → error.
   - Spot missing both `mangle` and `target` → error.
   - `mangle` with non-numeric values → error.
   - Multi-naming: both `light` and `_light` property names accepted.
   - `style` = 1 → `LightAnimation` with non-None brightness curve; `style` = 0 → `animation: None`.

9. **No runtime engine changes in stages 1–3.** Canonical lights available via `MapData::lights` for the Stage 4 baker and Stage 6 runtime direct path.

### For implementation — Part B stages 4–5 (SH baker + PRL section)

1. **PRL section allocated:** new section ID in `postretro-level-format/src/lib.rs` for the SH irradiance volume, with read/write round-trip tests matching the existing section pattern. (Part A owns the `Bvh` section id; Part B adds only the SH volume section.)

2. **Baker stage in prl-build:** runs after Part A BVH construction and before pack. Ray traversal goes through the Part A BVH via the `bvh` crate — no separate baker BVH. Produces a 3D grid of probe records following the Stage 4 algorithm: stratified sphere sampling, BVH-traversed raycasts, SH L2 projection, validity masking.

3. **Determinism:** identical input `.map` produces identical SH coefficients. Stratified sampling uses a fixed seed.

4. **Default probe spacing** (1 m) and CLI override (`--probe-spacing`) implemented.

5. **Probe validity mask populated** from BSP solid/empty classification.

6. **Bake parallelism** via `rayon` — one task per probe or per probe slab.

### For implementation — Part B stage 6 (runtime lighting)

1. **SH volume loader:** parse PRL section, upload to a 3D texture.

2. **World shader extended:** sample SH volume trilinearly, reconstruct irradiance via SH L2 dot product, replace flat ambient with the result.

3. **Normal map path:** load normal maps alongside albedo, reconstruct TBN in vertex shader, perturb fragment normal before shading.

4. **Clustered light list compute prepass:** iterate active lights, build per-cluster index lists in a storage buffer.

5. **World shader direct term:** walk the fragment's cluster, accumulate direct contributions from each light, sample shadow maps for shadow-casting lights.

6. **Shadow map passes:** CSM for directional, cube map for point, single map for spot. Run before the opaque pass each frame.

7. **Visual validation:** lighting test maps (point, spot, directional; bright and dark corners; curved walls; normal-mapped surfaces) look correct. Indirect light bleeds around corners; direct falloff matches the falloff model; shadows are crisp at the chosen resolution.

---

## Implementation tasks

All stages are now concrete. `/orchestrate` can break this plan into execution chunks. Each task below is sized to fit one execution-agent dispatch. Part A's five tasks run sequentially and must pass Stage A5 validation before any Part B task starts.

### Part A — BVH Foundation

#### Stage A1 — CPU BVH construction in prl-build

A1. Add `bvh = "..."` to `postretro-level-compiler/Cargo.toml`. Implement BVH primitive collection in the geometry pipeline: walk face/index data, emit one primitive per `(face, material_bucket)` pair with its `index_range` and `AABB`. Feed into `bvh::Bvh::build`. Flatten the built tree into a dense node array and a dense leaf array for PRL serialization.

A2. Delete `postretro-level-compiler/src/chunk_grouping.rs` and all references. Rewrite `FaceMetaV3` so `index_offset/index_count` either stay consistent through the BVH construction pipeline or get removed from `GeometryV3` entirely (decided during implementation — the review finding #1 fix lands here as a side effect).

A3. Unit tests: deterministic build (two identical input sets → byte-identical flattened buffers), leaf-primitive-coverage (every source triangle in exactly one leaf), AABB tightness on each leaf, single-face and multi-texture test fixtures.

#### Stage A2 — PRL section for BVH

A4. Add `SectionId::Bvh` in `postretro-level-format/src/lib.rs`. Retire `SectionId::CellChunks` (delete the variant, delete `cell_chunks.rs`, delete all read/write callers).

A5. Implement `BvhSection` with the layout spelled out in Stage A2. Write/read round-trip tests (byte-identical), truncation rejection, malformed header rejection. Match the existing section test patterns in `postretro-level-format`.

A6. Wire `BvhSection` into the `prl-build` pack stage. Confirm every test map compiles and emits a `Bvh` section.

#### Stage A3 — Runtime loader + GPU upload

A7. Extend `postretro/src/prl.rs`: parse `Bvh` section, allocate node-array and leaf-array `wgpu::Buffer` storage buffers, expose them via `LevelWorld`. Delete `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, `POSTRETRO_FORCE_LEGACY`, and the CPU-side draw-range reconstruction path.

A8. Update `postretro/src/visibility.rs` to stop producing `DrawRange` output. Portal DFS still produces a visible-cell set; the BVH traversal compute shader consumes it directly.

A9. Verify legacy V1/V2 `.prl` files either load cleanly or fail with a clear version error. Run the full test suite; `cargo clippy -p postretro -- -D warnings` clean.

#### Stage A4 — WGSL compute BVH traversal

A10. Rewrite `postretro/src/compute_cull.rs` as a BVH traversal compute shader. Top-down stack-based traversal (fixed max depth 64); frustum test on internal nodes; emit `DrawIndexedIndirect` per surviving leaf into the fixed-slot indirect buffer.

A11. Decide and implement the portal integration — either per-leaf visible-cell bitmask check (requires BVH leaves to carry cell id) or multi-frustum traversal (one pass per portal-narrowed frustum, union results). Document the choice in a header comment at the top of the WGSL file; note the rejected alternative.

A12. Preserve Phase 3.5 invariants: `MULTI_DRAW_INDIRECT` feature probe + singular `draw_indexed_indirect` fallback, one `multi_draw_indexed_indirect` call per material bucket, per-leaf cull-status buffer feeding the wireframe overlay.

A13. Edge-case tests: empty visible set, degenerate BVH (single leaf), deeply unbalanced BVH (simulate thin corridor map), first-frame dispatch before steady state.

#### Stage A5 — Part A validation (check-in gate)

A14. Capture Phase 3.5 reference screenshots from fixed cameras on every `assets/maps/` test map *before* Part A code lands. Store them under `assets/validation/phase-3-5/` (or equivalent) as the parity baseline.

A15. Implement or invoke an SSIM comparison harness. Run parity checks against Part A output on every test map. Document any diffs that exceed the thresholds; classify each as accept / investigate / block.

A16. Frame time capture on a cell-heavy test map (20+ visible cells). Compare against Phase 3.5 baseline. Must land within ±5%.

A17. Update `context/lib/rendering_pipeline.md` and `context/lib/build_pipeline.md` to describe the BVH architecture; replace cell/chunk language throughout.

A18. **Check-in gate.** Sign off Part A. If parity fails, document the pivot decision (per-region BVH, traversal strategy change, or scope adjustment) in this plan before any Part B task starts.

---

### Part B — Lighting

#### Stage 1 — FGD file

1. Create `assets/postretro.fgd` with `light`, `light_spot`, `light_sun` per the template above.
2. Verify in TrenchBroom: copy to game config folder, open editor, confirm entity browser + SmartEdit property widgets behave without errors.

#### Stage 2 — Parse and translate

3. Create `postretro-level-compiler/src/format/mod.rs` and `format/quake_map.rs`. Implement `translate_light()` per signature. Include Quake style preset table and degrees-to-radians conversion.
4. Extend `postretro-level-compiler/src/parse.rs`: recognize light classnames, extract property bag, dispatch to translator, propagate errors and warnings.
5. Extend `assets/maps/test.map` with the three example entities above.
6. Write translator unit tests covering every validation rule and the style preset conversion.

#### Stage 3 — Canonical format

7. Add canonical types (`LightType`, `FalloffModel`, `LightAnimation`, `CanonicalLight`) to `postretro-level-compiler/src/map_data.rs`.
8. Add `lights: Vec<CanonicalLight>` field to `MapData`.

#### Stage 4 — SH baker

9. Allocate a new PRL section ID for the SH irradiance volume (separate from the Part A `Bvh` section id).
10. Add probe record and section types to `postretro-level-format` with read/write + round-trip tests.
11. Implement probe placement: regular grid over map AABB at configurable spacing; solidity query against BSP.
12. Implement radiance sampling: stratified sphere rays, traverse the **Part A BVH via the `bvh` crate** for closest-triangle hits, per-light shadow raycasts through the same BVH, Lambert evaluation at hit points. No separate baker BVH.
13. Implement SH L2 projection from radiance samples.
14. Parallelize with `rayon` over probes; expose `--probe-spacing` CLI flag.

#### Stage 5 — PRL section

15. Wire the SH volume section into the prl-build pack stage.
16. Engine loader parses the section and produces a GPU-ready upload descriptor (no wgpu calls in the loader).

#### Stage 6 — Runtime lighting and normal maps

17. Renderer: create SH volume 3D texture, upload from loader data, bind in the world shader.
18. World shader: trilinear SH sample → irradiance reconstruction → replaces flat ambient.
19. Normal map loading: albedo + normal texture pair per material; BC5 preferred, fallback RG8.
20. Vertex shader: reconstruct TBN from packed normal + tangent + bitangent sign.
21. Fragment shader: sample normal map, apply TBN transform, shade with SH irradiance.
22. Clustered light list compute prepass: implement tile/slice grid, per-cluster index list build.
23. World shader direct term: cluster walk, Lambert/Phong direct evaluation, shadow map sampling.
24. Shadow map passes: CSM for directional, cube for point, single map for spot.
25. Lighting test maps: author scenes that exercise indirect bleed, direct falloff, shadow crispness, and normal-map angle variation.

#### Docs

26. On ship, migrate the canonical format and pipeline sections into `context/lib/rendering_pipeline.md` §4 (already updated in this refactor).

---

## When this plan ships

Durable architectural decisions migrate to `context/lib/rendering_pipeline.md` (`context/lib/lighting.md` if the section outgrows §4). Candidates for migration:

**From Part A (BVH Foundation):**
- Global BVH rationale and the per-region pivot condition — document the decision and the fallback, so a future contributor knows why we chose global and what would trigger a pivot.
- `Bvh` PRL section layout (header + flat node/leaf arrays, byte shape, endianness).
- WGSL BVH traversal shader structure — the stack-based pattern, depth cap, portal integration shape — lands as a new section in `rendering_pipeline.md` §5 (replacing the cell-chunk description).
- `bvh` crate usage for compile-time primitive build; flatten-to-buffer lowering.
- Finding #1 postmortem note: `FaceMetaV3` stale-index-range bug and how it dissolved under the pipeline rewrite.

**From Part B (Lighting):**
- Canonical format struct shape and design rationale.
- `format/<name>.rs` architecture for multi-format source support.
- SH volume spatial layout, per-probe storage, validity masking.
- SH volume PRL section shape.
- Clustered forward+ cluster grid parameters and shadow map defaults.
- Baker ↔ BVH sharing pattern: one acceleration structure, two consumers (bake-time CPU, runtime GPU).

The plan document itself is ephemeral per `development_guide.md` §1.5.
